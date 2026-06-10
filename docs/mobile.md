# ClipMind Pro — Mobile Execution Guide
## iOS (VideoToolbox + Metal) and Android (MediaCodec + Vulkan)

---

## iOS Implementation

### Architecture Overview

```
React Native / Capacitor (UI)
         │
         ▼
   Swift Bridge (Tauri Mobile)
         │
    ┌────┴────────────────────┐
    │                         │
VideoToolbox              Metal API
(Hardware encode/decode)  (GPU shaders)
    │                         │
H.264/HEVC ASIC           GPU Filters
(Apple Silicon encode)    (Color grade,
                           scale, grain)
```

### VideoToolbox Zero-Copy Encode (Swift)

```swift
// Sources/ClipMindCore/VideoProcessor.swift
import VideoToolbox
import CoreMedia
import Metal
import MetalPerformanceShaders

class ClipMindVideoProcessor {

    // ── Metal device and pipeline ──────────────────────────────────────
    private let device: MTLDevice
    private let commandQueue: MTLCommandQueue
    private var colorGradePipeline: MTLComputePipelineState?

    // ── VideoToolbox compression session ──────────────────────────────
    private var compressionSession: VTCompressionSession?

    init() throws {
        guard let dev = MTLCreateSystemDefaultDevice() else {
            throw ClipMindError.noMetalDevice
        }
        self.device       = dev
        self.commandQueue = dev.makeCommandQueue()!
        try setupColorGradePipeline()
    }

    // ── 1. Create hardware compression session ─────────────────────────
    func createCompressionSession(width: Int32, height: Int32, fps: Int32) throws {
        var session: VTCompressionSession?

        let encoderSpec: CFDictionary = [
            kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: kCFBooleanTrue!
        ] as CFDictionary

        let status = VTCompressionSessionCreate(
            allocator:                  kCFAllocatorDefault,
            width:                      width,
            height:                     height,
            codecType:                  kCMVideoCodecType_HEVC,  // H.265 on A-series
            encoderSpecification:       encoderSpec,
            imageBufferAttributes:      nil,
            compressedDataAllocator:    nil,
            outputCallback:             nil,
            refcon:                     nil,
            compressionSessionOut:      &session
        )

        guard status == noErr, let sess = session else {
            throw ClipMindError.compressionSessionFailed(status)
        }

        // Configure bitrate and frame rate
        VTSessionSetProperty(sess, key: kVTCompressionPropertyKey_RealTime,        value: kCFBooleanFalse)
        VTSessionSetProperty(sess, key: kVTCompressionPropertyKey_AllowFrameReordering, value: kCFBooleanTrue)
        VTSessionSetProperty(sess, key: kVTCompressionPropertyKey_AverageBitRate,   value: 8_000_000 as CFNumber)
        VTSessionSetProperty(sess, key: kVTCompressionPropertyKey_ExpectedFrameRate, value: fps as CFNumber)
        VTSessionSetProperty(sess, key: kVTCompressionPropertyKey_ProfileLevel,     value: kVTProfileLevel_HEVC_Main_AutoLevel)

        VTCompressionSessionPrepareToEncodeFrames(sess)
        self.compressionSession = sess
    }

    // ── 2. Metal color grade shader ────────────────────────────────────
    // The shader runs entirely in the GPU — zero CPU copies
    private func setupColorGradePipeline() throws {
        let library = try device.makeDefaultLibrary(bundle: .module)
        let function = library.makeFunction(name: "colorGradeKernel")!
        colorGradePipeline = try device.makeComputePipelineState(function: function)
    }

    func applyColorGrade(
        pixelBuffer: CVPixelBuffer,
        style: String
    ) -> CVPixelBuffer {
        let params = colorGradeParams(for: style)

        // Create Metal texture from CVPixelBuffer (zero-copy via IOSurface)
        var inputTexture: MTLTexture?
        var outputTexture: MTLTexture?

        CVMetalTextureCacheCreateTextureFromImage(
            kCFAllocatorDefault,
            textureCache,
            pixelBuffer,
            nil,
            .bgra8Unorm,
            CVPixelBufferGetWidth(pixelBuffer),
            CVPixelBufferGetHeight(pixelBuffer),
            0,
            &metalTextureRef
        )

        let cmdBuffer  = commandQueue.makeCommandBuffer()!
        let encoder    = cmdBuffer.makeComputeCommandEncoder()!
        encoder.setComputePipelineState(colorGradePipeline!)
        encoder.setTexture(inputTexture,  index: 0)
        encoder.setTexture(outputTexture, index: 1)

        var p = params
        encoder.setBytes(&p, length: MemoryLayout<ColorGradeParams>.size, index: 0)

        let w = colorGradePipeline!.threadExecutionWidth
        let h = colorGradePipeline!.maxTotalThreadsPerThreadgroup / w
        let threadsPerGroup   = MTLSize(width: w, height: h, depth: 1)
        let threadgroupsPerGrid = MTLSize(
            width:  (inputTexture!.width  + w - 1) / w,
            height: (inputTexture!.height + h - 1) / h,
            depth:  1
        )
        encoder.dispatchThreadgroups(threadgroupsPerGrid, threadsPerThreadgroup: threadsPerGroup)
        encoder.endEncoding()
        cmdBuffer.commit()
        cmdBuffer.waitUntilCompleted()

        return outputPixelBuffer
    }

    // ── 3. Encode a frame (zero-copy) ──────────────────────────────────
    func encodeFrame(_ pixelBuffer: CVPixelBuffer, presentationTime: CMTime) {
        guard let session = compressionSession else { return }

        let frameProperties: CFDictionary = [
            kVTEncodeFrameOptionKey_ForceKeyFrame: kCFBooleanFalse!
        ] as CFDictionary

        VTCompressionSessionEncodeFrame(
            session,
            imageBuffer:        pixelBuffer,
            presentationTimeStamp: presentationTime,
            duration:           .invalid,
            frameProperties:    frameProperties,
            infoFlagsOut:       nil,
            outputHandler: { [weak self] status, flags, sampleBuffer in
                guard let sb = sampleBuffer else { return }
                self?.muxer.append(sampleBuffer: sb)
            }
        )
    }

    // ── 4. Battery-aware scheduling ────────────────────────────────────
    // Use ProcessInfo thermal state to throttle on sustained load
    func adjustForThermalState() {
        let state = ProcessInfo.processInfo.thermalState
        switch state {
        case .critical:
            // Drop to 1080p 30fps to prevent thermal throttle
            maxResolution = CGSize(width: 1920, height: 1080)
            targetFps = 30
        case .serious:
            maxResolution = CGSize(width: 2560, height: 1440)
            targetFps = 60
        default:
            break
        }
    }
}
```

### Metal Color Grade Shader

```metal
// Sources/ClipMindCore/Metal/ColorGradeKernel.metal
#include <metal_stdlib>
using namespace metal;

struct ColorGradeParams {
    float brightness;
    float contrast;
    float saturation;
    float gamma_r;
    float gamma_g;
    float gamma_b;
};

kernel void colorGradeKernel(
    texture2d<float, access::read>  inTexture  [[texture(0)]],
    texture2d<float, access::write> outTexture [[texture(1)]],
    constant ColorGradeParams&      params     [[buffer(0)]],
    uint2 gid                                  [[thread_position_in_grid]]
) {
    if (gid.x >= outTexture.get_width() || gid.y >= outTexture.get_height()) return;

    float4 color = inTexture.read(gid);

    // Brightness
    color.rgb += params.brightness;

    // Contrast (around 0.5 midpoint)
    color.rgb = (color.rgb - 0.5) * params.contrast + 0.5;

    // Saturation via luminance
    float luma = dot(color.rgb, float3(0.2126, 0.7152, 0.0722));
    color.rgb  = mix(float3(luma), color.rgb, params.saturation);

    // Per-channel gamma
    color.r = pow(max(color.r, 0.0), 1.0 / params.gamma_r);
    color.g = pow(max(color.g, 0.0), 1.0 / params.gamma_g);
    color.b = pow(max(color.b, 0.0), 1.0 / params.gamma_b);

    color = clamp(color, 0.0, 1.0);
    outTexture.write(color, gid);
}
```

---

## Android Implementation

### Architecture Overview

```
React Native / Capacitor (UI)
         │
         ▼
   Kotlin Bridge (Tauri Android)
         │
    ┌────┴────────────────────┐
    │                         │
MediaCodec API            Vulkan / OpenGL ES
(Hardware encode/decode)  (GPU shaders via GLSL)
    │                         │
SoC Codec ASIC            GPU Filters
(Qualcomm, Samsung,       (Color grade, scale)
 MediaTek, etc.)
```

### MediaCodec Hardware Encode (Kotlin)

```kotlin
// android/src/main/java/io/clipmind/VideoEncoder.kt
package io.clipmind

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.media.MediaMuxer
import android.view.Surface
import java.io.File

class ClipMindEncoder(
    private val outputFile: File,
    private val width: Int,
    private val height: Int,
    private val fps: Int,
    private val bitrateBps: Int = 8_000_000
) {
    private lateinit var encoder: MediaCodec
    private lateinit var muxer:   MediaMuxer
    private var trackIndex = -1
    private var muxerStarted = false

    // ── 1. Create hardware codec ───────────────────────────────────────
    fun prepare() {
        val format = MediaFormat.createVideoFormat(
            MediaFormat.MIMETYPE_VIDEO_AVC, width, height
        ).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT,
                MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE,   bitrateBps)
            setInteger(MediaFormat.KEY_FRAME_RATE, fps)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 2)  // GOP every 2s
            // Request hardware-backed codec (available on all modern Android SoCs)
            setInteger(MediaFormat.KEY_PRIORITY, 0)  // realtime
        }

        // Pick the fastest HW encoder on this device
        encoder = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
        encoder.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)

        muxer = MediaMuxer(outputFile.absolutePath, MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4)
    }

    // ── 2. Return Surface for GPU → encoder zero-copy path ─────────────
    // The caller renders into this Surface via OpenGL ES / Vulkan.
    // MediaCodec reads frames directly from the GPU buffer — no CPU copy.
    fun getInputSurface(): Surface = encoder.createInputSurface()

    fun start() { encoder.start() }

    // ── 3. Drain encoder output ────────────────────────────────────────
    fun drainEncoder(endOfStream: Boolean) {
        if (endOfStream) {
            encoder.signalEndOfInputStream()
        }

        val bufferInfo = MediaCodec.BufferInfo()
        while (true) {
            val outputBufferId = encoder.dequeueOutputBuffer(bufferInfo, 10_000L)
            when {
                outputBufferId == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                    val newFormat = encoder.outputFormat
                    trackIndex    = muxer.addTrack(newFormat)
                    muxer.start()
                    muxerStarted = true
                }
                outputBufferId >= 0 -> {
                    val encodedData = encoder.getOutputBuffer(outputBufferId) ?: continue

                    if (bufferInfo.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                        bufferInfo.size = 0
                    }
                    if (bufferInfo.size != 0 && muxerStarted) {
                        encodedData.position(bufferInfo.offset)
                        encodedData.limit(bufferInfo.offset + bufferInfo.size)
                        muxer.writeSampleData(trackIndex, encodedData, bufferInfo)
                    }

                    encoder.releaseOutputBuffer(outputBufferId, false)

                    if (bufferInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) break
                }
                else -> if (endOfStream) break
            }
        }
    }

    fun release() {
        encoder.stop(); encoder.release()
        if (muxerStarted) { muxer.stop(); muxer.release() }
    }
}
```

### OpenGL ES Color Grade Shader (GLSL)

```glsl
// android/src/main/assets/shaders/color_grade.frag
// Fragment shader for OpenGL ES 3.0 — runs entirely on GPU
precision mediump float;

uniform sampler2D uTexture;
uniform float uBrightness;
uniform float uContrast;
uniform float uSaturation;
uniform float uGammaR;
uniform float uGammaG;
uniform float uGammaB;

varying vec2 vTexCoord;

void main() {
    vec4 color = texture2D(uTexture, vTexCoord);

    // Brightness
    color.rgb += uBrightness;

    // Contrast
    color.rgb = (color.rgb - 0.5) * uContrast + 0.5;

    // Saturation
    float luma  = dot(color.rgb, vec3(0.2126, 0.7152, 0.0722));
    color.rgb   = mix(vec3(luma), color.rgb, uSaturation);

    // Per-channel gamma
    color.r = pow(max(color.r, 0.0), 1.0 / uGammaR);
    color.g = pow(max(color.g, 0.0), 1.0 / uGammaG);
    color.b = pow(max(color.b, 0.0), 1.0 / uGammaB);

    gl_FragColor = clamp(color, 0.0, 1.0);
}
```

### Battery & Thermal Management (Android)

```kotlin
// Monitor device temperature — throttle export settings to protect the SoC
class ThermalManager(context: Context) {
    private val powerManager = context.getSystemService(PowerManager::class.java)

    fun getMaxExportProfile(): ExportProfile {
        return when {
            powerManager.isDeviceIdleMode -> ExportProfile.LOW
            Build.VERSION.SDK_INT >= 29 &&
            powerManager.currentThermalStatus >= PowerManager.THERMAL_STATUS_MODERATE
                -> ExportProfile.MID  // 1080p 30fps
            else -> ExportProfile.HIGH  // 4K 60fps on flagship
        }
    }
}
```

---

## Tauri Mobile Configuration

Add to `Cargo.toml` for mobile targets:

```toml
[target.aarch64-apple-ios.dependencies]
tauri = { version = "2", features = ["wry"] }

[target.aarch64-linux-android.dependencies]
tauri = { version = "2", features = ["wry"] }
```

Build commands:
```bash
# iOS
cargo tauri ios build --target aarch64-apple-ios

# Android (arm64 modern phones)
cargo tauri android build --target aarch64-linux-android
```
