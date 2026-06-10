// ClipMind Pro — Tauri Rust Core Engine
// Hardware-accelerated local video processing pipeline

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

// ── Job registry ───────────────────────────────────────────────────────────────
type JobRegistry = Arc<Mutex<HashMap<String, JobState>>>;

#[derive(Debug, Clone, Serialize)]
struct JobState {
    status: String,
    progress: u8,
    output_path: Option<String>,
    error: Option<String>,
}

// ── Hardware capability schema ─────────────────────────────────────────────────
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareProfile {
    pub cpu_cores: usize,
    pub ram_gb: f32,
    pub gpu_vendor: String,         // "nvidia" | "amd" | "intel" | "apple" | "unknown"
    pub gpu_name: String,
    pub encoder: String,            // "h264_nvenc" | "h264_amf" | "h264_qsv" | "h264_videotoolbox" | "libx264"
    pub hw_decode: String,          // "cuda" | "d3d11va" | "videotoolbox" | "none"
    pub max_resolution: String,     // "1080p" | "4k" | "8k"
    pub max_fps: u32,               // 60 | 120
    pub tier: String,               // "low" | "mid" | "high"
    pub unlocked_resolutions: Vec<String>,
    pub unlocked_fps: Vec<u32>,
    pub warnings: Vec<String>,
    pub cuda_available: bool,
    pub metal_available: bool,
}

// ── Edit plan (from AI analyzer) ──────────────────────────────────────────────
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditPlan {
    pub style: String,
    pub options: Vec<String>,
    pub cuts: Vec<Cut>,
    pub captions: Vec<Caption>,
    pub resolution: String,
    pub fps: u32,
    pub quality: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cut {
    pub cut_type: String,
    pub timestamp: f64,
    pub duration: Option<f64>,
    pub speed_in: Option<f64>,
    pub speed_out: Option<f64>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Caption {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

// ── Progress event payload ─────────────────────────────────────────────────────
#[derive(Debug, Clone, Serialize)]
struct ProgressEvent {
    job_id: String,
    step: i32,
    progress: u8,
    status: String,
    download_path: Option<String>,
    error: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// COMMAND: detect_hardware
// Probes system hardware and returns the compatibility matrix.
// ─────────────────────────────────────────────────────────────────────────────
#[tauri::command]
async fn detect_hardware() -> Result<HardwareProfile, String> {
    let cpu_cores = num_cpus::get();
    let ram_gb = get_system_ram_gb();
    let (gpu_vendor, gpu_name, encoder, hw_decode, cuda_available, metal_available) =
        detect_gpu_capabilities().await;

    // Map capabilities → tier
    let (tier, max_resolution, max_fps, unlocked_resolutions, unlocked_fps, mut warnings) =
        compute_tier(
            cpu_cores,
            ram_gb,
            &gpu_vendor,
            &encoder,
            cuda_available,
            metal_available,
        );

    // Warn if detected encoder will fall back to software
    if encoder == "libx264" {
        warnings.push("No hardware encoder detected. Software encoding (libx264) will be used — expect slower export times.".into());
    }

    Ok(HardwareProfile {
        cpu_cores,
        ram_gb,
        gpu_vendor,
        gpu_name,
        encoder,
        hw_decode,
        max_resolution,
        max_fps,
        tier,
        unlocked_resolutions,
        unlocked_fps,
        warnings,
        cuda_available,
        metal_available,
    })
}

async fn detect_gpu_capabilities() -> (String, String, String, String, bool, bool) {
    // ── NVIDIA via nvidia-smi ──────────────────────────────────────────────
    if let Ok(out) = Command::new("nvidia-smi")
        .args(["--query-gpu=name", "--format=csv,noheader"])
        .output()
        .await
    {
        if out.status.success() {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !name.is_empty() {
                // Check NVENC availability with a quick probe
                let nvenc_ok = probe_ffmpeg_encoder("h264_nvenc").await;
                let encoder = if nvenc_ok {
                    "h264_nvenc".to_string()
                } else {
                    "libx264".to_string()
                };
                return (
                    "nvidia".to_string(),
                    name,
                    encoder,
                    "cuda".to_string(),
                    true,
                    false,
                );
            }
        }
    }

    // ── Apple Silicon / macOS VideoToolbox ────────────────────────────────
    #[cfg(target_os = "macos")]
    {
        let vt_ok = probe_ffmpeg_encoder("h264_videotoolbox").await;
        if vt_ok {
            let gpu_name = get_macos_gpu_name().await;
            let is_apple_silicon = gpu_name.contains("Apple");
            return (
                "apple".to_string(),
                gpu_name,
                "h264_videotoolbox".to_string(),
                "videotoolbox".to_string(),
                false,
                is_apple_silicon,
            );
        }
    }

    // ── AMD via rocm-smi or wmic ──────────────────────────────────────────
    if let Ok(out) = Command::new("wmic")
        .args(["path", "win32_VideoController", "get", "name"])
        .output()
        .await
    {
        let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
        if text.contains("amd") || text.contains("radeon") {
            let amf_ok = probe_ffmpeg_encoder("h264_amf").await;
            let encoder = if amf_ok {
                "h264_amf".to_string()
            } else {
                "libx264".to_string()
            };
            return (
                "amd".to_string(),
                "AMD Radeon GPU".to_string(),
                encoder,
                "d3d11va".to_string(),
                false,
                false,
            );
        }
        if text.contains("intel") {
            let qsv_ok = probe_ffmpeg_encoder("h264_qsv").await;
            let encoder = if qsv_ok {
                "h264_qsv".to_string()
            } else {
                "libx264".to_string()
            };
            return (
                "intel".to_string(),
                "Intel Graphics".to_string(),
                encoder,
                "qsv".to_string(),
                false,
                false,
            );
        }
    }

    // ── Linux fallback — lspci ─────────────────────────────────────────────
    if let Ok(out) = Command::new("lspci").output().await {
        let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
        if text.contains("nvidia") {
            let nvenc_ok = probe_ffmpeg_encoder("h264_nvenc").await;
            return (
                "nvidia".to_string(),
                "NVIDIA GPU".to_string(),
                if nvenc_ok { "h264_nvenc".into() } else { "libx264".into() },
                "cuda".to_string(),
                nvenc_ok,
                false,
            );
        }
    }

    // ── No hardware acceleration found ────────────────────────────────────
    (
        "unknown".to_string(),
        "CPU / Integrated Graphics".to_string(),
        "libx264".to_string(),
        "none".to_string(),
        false,
        false,
    )
}

async fn probe_ffmpeg_encoder(encoder: &str) -> bool {
    // Try encoding a 1-frame black video; if FFmpeg exits 0 the encoder works
    let result = Command::new("ffmpeg")
        .args([
            "-f", "lavfi", "-i", "color=black:s=128x72:d=0.1",
            "-c:v", encoder,
            "-frames:v", "1",
            "-f", "null", "-",
            "-loglevel", "error",
        ])
        .output()
        .await;
    result.map(|o| o.status.success()).unwrap_or(false)
}

#[cfg(target_os = "macos")]
async fn get_macos_gpu_name() -> String {
    if let Ok(out) = Command::new("system_profiler")
        .args(["SPDisplaysDataType"])
        .output()
        .await
    {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if line.contains("Chipset Model") || line.contains("GPU Model") {
                if let Some(name) = line.split(':').nth(1) {
                    return name.trim().to_string();
                }
            }
        }
    }
    "Apple GPU".to_string()
}

fn get_system_ram_gb() -> f32 {
    // ── Linux: read /proc/meminfo ──────────────────────────────────────────
    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
            for line in content.lines() {
                if line.starts_with("MemTotal:") {
                    if let Some(kb) = line.split_whitespace().nth(1) {
                        if let Ok(kb_val) = kb.parse::<f32>() {
                            return kb_val / (1024.0 * 1024.0);
                        }
                    }
                }
            }
        }
    }

    // ── macOS: sysctl hw.memsize ───────────────────────────────────────────
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
        {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Ok(bytes) = s.trim().parse::<f64>() {
                return (bytes / (1024.0 * 1024.0 * 1024.0)) as f32;
            }
        }
    }

    // ── Windows: wmic ComputerSystem get TotalPhysicalMemory ──────────────
    #[cfg(target_os = "windows")]
    {
        if let Ok(out) = std::process::Command::new("wmic")
            .args(["ComputerSystem", "get", "TotalPhysicalMemory"])
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                let trimmed = line.trim();
                if let Ok(bytes) = trimmed.parse::<f64>() {
                    return (bytes / (1024.0 * 1024.0 * 1024.0)) as f32;
                }
            }
        }
    }

    // Fallback: safe default
    8.0
}

fn compute_tier(
    cpu_cores: usize,
    ram_gb: f32,
    gpu_vendor: &str,
    encoder: &str,
    cuda_available: bool,
    metal_available: bool,
) -> (String, String, u32, Vec<String>, Vec<u32>, Vec<String>) {
    let mut warnings = Vec::new();

    // HIGH TIER: RTX 40-series equivalent, 32 GB+ RAM, 12+ cores
    let is_high = (cuda_available || metal_available)
        && ram_gb >= 24.0
        && cpu_cores >= 10
        && (encoder == "h264_nvenc" || encoder == "h264_videotoolbox" || encoder == "h264_amf");

    // MID TIER: Dedicated GPU with HW encoder, 16 GB+ RAM
    let is_mid = !is_high
        && (encoder != "libx264")
        && ram_gb >= 12.0
        && cpu_cores >= 6;

    if is_high {
        (
            "high".to_string(),
            "8k".to_string(),
            120,
            vec!["720p","1080p","1440p","4k","8k","9:16","1:1"].map(String::from).to_vec(),
            vec![24, 30, 60, 120],
            warnings,
        )
    } else if is_mid {
        (
            "mid".to_string(),
            "4k".to_string(),
            60,
            vec!["720p","1080p","1440p","4k","9:16","1:1"].map(String::from).to_vec(),
            vec![24, 30, 60],
            warnings,
        )
    } else {
        // LOW TIER — force draft cap
        warnings.push("Low-end hardware detected. Export is capped at 1080p 60fps. Higher settings may cause crashes.".into());
        if ram_gb < 8.0 {
            warnings.push(format!("Only {:.1} GB RAM available. Close other applications before processing.", ram_gb));
        }
        (
            "low".to_string(),
            "1080p".to_string(),
            60,
            vec!["720p","1080p","9:16","1:1"].map(String::from).to_vec(),
            vec![24, 30, 60],
            warnings,
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// COMMAND: open_file_dialog
// Uses Tauri's native OS file dialog — no browser Blob upload required.
// ─────────────────────────────────────────────────────────────────────────────
#[tauri::command]
async fn open_file_dialog(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    let (tx, rx) = oneshot::channel();

    app.dialog()
        .file()
        .add_filter(
            "Video Files",
            &["mp4", "mov", "mkv", "avi", "webm", "m4v", "flv"],
        )
        .pick_file(move |path| {
            let _ = tx.send(path);
        });

    let path = rx.await.map_err(|e| e.to_string())?;
    Ok(path.and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().to_string()))
}

// ─────────────────────────────────────────────────────────────────────────────
// COMMAND: process_video
// Builds and executes a single-pass FFmpeg filter_complex graph.
// Streams progress events back to the frontend via Tauri events.
// ─────────────────────────────────────────────────────────────────────────────
#[tauri::command]
async fn process_video(
    app: AppHandle,
    job_id: String,
    input_path: String,
    plan: EditPlan,
    hardware: HardwareProfile,
    jobs: State<'_, JobRegistry>,
) -> Result<(), String> {
    // Register job
    {
        let mut registry = jobs.lock().unwrap();
        registry.insert(
            job_id.clone(),
            JobState {
                status: "starting".into(),
                progress: 0,
                output_path: None,
                error: None,
            },
        );
    }

    let app_clone = app.clone();
    let job_id_clone = job_id.clone();
    let jobs_clone = jobs.inner().clone();

    tokio::spawn(async move {
        let result = run_pipeline(&app_clone, &job_id_clone, &input_path, &plan, &hardware).await;

        let mut registry = jobs_clone.lock().unwrap();
        if let Some(job) = registry.get_mut(&job_id_clone) {
            match result {
                Ok(output_path) => {
                    job.status = "done".into();
                    job.output_path = Some(output_path.clone());
                    emit_progress(
                        &app_clone,
                        &job_id_clone,
                        7, 100, "✅ Export complete!",
                        Some(output_path), None,
                    );
                }
                Err(e) => {
                    job.status = "error".into();
                    job.error = Some(e.clone());
                    emit_progress(
                        &app_clone,
                        &job_id_clone,
                        -1, 0, &format!("❌ {}", e),
                        None, Some(e),
                    );
                }
            }
        }
    });

    Ok(())
}

fn emit_progress(
    app: &AppHandle,
    job_id: &str,
    step: i32,
    progress: u8,
    status: &str,
    download_path: Option<String>,
    error: Option<String>,
) {
    let _ = app.emit(
        "pipeline-progress",
        ProgressEvent {
            job_id: job_id.to_string(),
            step,
            progress,
            status: status.to_string(),
            download_path,
            error,
        },
    );
}

async fn run_pipeline(
    app: &AppHandle,
    job_id: &str,
    input_path: &str,
    plan: &EditPlan,
    hw: &HardwareProfile,
) -> Result<String, String> {
    emit_progress(app, job_id, 1, 5, "🔍 Probing video...", None, None);

    // ── Probe input ────────────────────────────────────────────────────────
    let (duration, has_audio) = probe_video(input_path).await?;

    emit_progress(app, job_id, 2, 20, "🎨 Building filter graph...", None, None);

    // ── Resolve output path ────────────────────────────────────────────────
    let input = PathBuf::from(input_path);
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    let output_dir = std::env::temp_dir().join("clipmind");
    std::fs::create_dir_all(&output_dir).map_err(|e| e.to_string())?;
    let output_path = output_dir
        .join(format!("{}_cm_{}.mp4", stem, &job_id[..8]))
        .to_string_lossy()
        .to_string();

    // ── Resolution mapping ─────────────────────────────────────────────────
    let (tw, th) = resolution_to_dims(&plan.resolution);

    // ── Build unified filter_complex ───────────────────────────────────────
    let filter_complex = build_filter_complex(plan, hw, tw, th, duration, has_audio);

    emit_progress(app, job_id, 3, 40, "⚙️ Starting hardware encoder...", None, None);

    // ── Build FFmpeg command ───────────────────────────────────────────────
    let mut cmd = build_ffmpeg_command(
        input_path,
        &output_path,
        &filter_complex,
        plan,
        hw,
        has_audio,
    );

    emit_progress(app, job_id, 4, 50, "🎬 Encoding...", None, None);

    // ── Execute with progress parsing ─────────────────────────────────────
    execute_ffmpeg_with_progress(app, job_id, &mut cmd, duration).await?;

    Ok(output_path)
}

async fn probe_video(path: &str) -> Result<(f64, bool), String> {
    let out = Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-print_format", "json",
            "-show_format",
            "-show_streams",
            path,
        ])
        .output()
        .await
        .map_err(|e| format!("ffprobe failed: {}", e))?;

    let text = String::from_utf8_lossy(&out.stdout);
    let duration = extract_duration_from_probe(&text);
    // Check for audio stream by codec_type field, not a loose substring match
    let has_audio = text.contains("\"codec_type\": \"audio\"") 
        || text.contains("\"codec_type\":\"audio\"");
    Ok((duration, has_audio))
}

fn extract_duration_from_probe(json: &str) -> f64 {
    // ffprobe JSON format: "duration": "X.XX" (with space after colon)
    for pattern in &["\"duration\": \"", "\"duration\":\""] {
        if let Some(pos) = json.find(pattern) {
            let rest = &json[pos + pattern.len()..];
            if let Some(end) = rest.find('"') {
                if let Ok(d) = rest[..end].parse::<f64>() {
                    if d > 0.0 {
                        return d;
                    }
                }
            }
        }
    }
    30.0
}

fn build_filter_complex(
    plan: &EditPlan,
    hw: &HardwareProfile,
    tw: u32,
    th: u32,
    duration: f64,
    has_audio: bool,
) -> String {
    let style = &plan.style;
    let options = &plan.options;
    let use_cuda = hw.encoder == "h264_nvenc" && hw.cuda_available;

    // ── Choose scale filter ────────────────────────────────────────────────
    // scale_cuda stays entirely in VRAM; scale is CPU-bound
    let scale_filter = if use_cuda {
        format!("scale_cuda={}:{}", tw, th)
    } else {
        format!("scale={}:{}:flags=lanczos", tw, th)
    };

    // ── Color grade (eq filter — safe across all FFmpeg builds) ───────────
    let grade = color_grade_for_style(style);

    // ── Upload to GPU if CUDA (zero-copy path) ─────────────────────────────
    let mut video_chain: Vec<String> = Vec::new();

    if use_cuda {
        video_chain.push("hwupload_cuda".to_string());
        video_chain.push(scale_filter);
        // GPU color grade via curves_cuda if available; else hwdownload + eq + hwupload
        video_chain.push("hwdownload".to_string());
        video_chain.push("format=nv12".to_string());
        video_chain.push(grade.clone());
    } else {
        // scale to target, then pad/crop to exact dimensions to avoid off-by-one
        video_chain.push(format!(
            "scale={}:{}:force_original_aspect_ratio=increase:flags=lanczos",
            tw, th
        ));
        video_chain.push(format!("crop={}:{}", tw, th));
        video_chain.push(grade.clone());
    }

    // ── Grain ─────────────────────────────────────────────────────────────
    if options.contains(&"grain".to_string())
        || style == "warzone"
        || style == "cinematic"
    {
        video_chain.push("noise=alls=14:allf=t".to_string());
    }

    // ── Letterbox ─────────────────────────────────────────────────────────
    if options.contains(&"letterbox".to_string())
        || style == "warzone"
        || style == "cinematic"
    {
        video_chain.push(
            "drawbox=x=0:y=0:w=iw:h=ih*0.08:color=black@1:t=fill".to_string(),
        );
        video_chain.push(
            "drawbox=x=0:y=ih*0.92:w=iw:h=ih*0.08:color=black@1:t=fill".to_string(),
        );
    }

    // ── Zoom punch from first zoom cut ────────────────────────────────────
    if options.contains(&"zooms".to_string()) {
        if let Some(zoom_cut) = plan.cuts.iter().find(|c| c.cut_type == "zoom") {
            let ts = zoom_cut.timestamp;
            let cdur = zoom_cut.duration.unwrap_or(1.5);
            let end = ts + cdur;
            // Optical-flow-assisted zoom via zoompan
            video_chain.push(format!(
                "zoompan=z='if(between(t,{:.2},{:.2}),1.18,1.0)':d=1:x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)'",
                ts, end
            ));
        }
    }

    // ── Sharpening post-scale ──────────────────────────────────────────────
    if plan.quality == "high" || plan.quality == "ultra" || plan.quality == "max" {
        video_chain.push("unsharp=5:5:1.2:5:5:0.0".to_string());
    }

    // ── Fade in ────────────────────────────────────────────────────────────
    if options.contains(&"fades".to_string()) {
        video_chain.push("fade=t=in:st=0:d=0.4".to_string());
        let fade_out_st = (duration - 1.2).max(0.5);
        video_chain.push(format!(
            "fade=t=out:st={:.3}:d=0.8",
            fade_out_st
        ));
    }

    // ── Speed ramp via setpts ──────────────────────────────────────────────
    // Only applies the first speed_ramp cut for now (complex multi-segment ramps
    // require a trim+concat which adds intermediate files — handled in a future pass)
    let speed_ramp_factor: Option<f64> = plan.cuts.iter()
        .find(|c| c.cut_type == "speed_ramp")
        .map(|sr| {
            let speed_in = sr.speed_in.unwrap_or(0.5);
            let ts = sr.timestamp;
            let cdur = sr.duration.unwrap_or(1.0);
            let end = ts + cdur;
            video_chain.push(format!(
                "setpts='if(between(t,{:.2},{:.2}),PTS*{:.2},PTS)'",
                ts, end, speed_in
            ));
            speed_in
        });

    // ── Frame interpolation (minterpolate) for high-fps output ────────────
    // Only on high tier and fps >= 120; minterpolate is CPU-intensive
    if plan.fps >= 120 && hw.tier == "high" {
        video_chain.push(format!(
            "minterpolate=fps={}:mi_mode=mci:mc_mode=aobmc:vsbmc=1",
            plan.fps
        ));
    }

    let vf = video_chain.join(",");

    // ── Audio filter ───────────────────────────────────────────────────────
    // For speed ramps: apply atempo to keep audio in sync with video setpts.
    // atempo is constrained to [0.5, 2.0]; clamp the value accordingly.
    let af = if has_audio {
        if let Some(factor) = speed_ramp_factor {
            // setpts multiplies PTS, so audio needs inverse tempo adjustment.
            // Clamp to atempo's supported range [0.5, 2.0].
            let tempo = (1.0 / factor).clamp(0.5, 2.0);
            format!("atempo={:.4}", tempo)
        } else {
            "anull".to_string()
        }
    } else {
        // Generate silent stereo audio for the full clip duration.
        // aevalsrc does not use [0:a] — it is a source filter.
        format!("aevalsrc=0:c=stereo:s=44100:d={:.3}", duration)
    };

    // The audio pad input label differs: real audio uses [0:a], generated source has no input.
    let audio_filter_str = if has_audio {
        format!("[0:a]{af}[aout]")
    } else {
        format!("{af}[aout]")
    };

    format!("[0:v]{vf}[vout];{audio_filter_str}")
}

fn color_grade_for_style(style: &str) -> String {
    match style {
        "freefire"  => "eq=brightness=0.06:contrast=1.30:saturation=1.55:gamma_r=1.12:gamma_b=0.88",
        "warzone"   => "eq=brightness=-0.04:contrast=1.42:saturation=0.55:gamma=0.95:gamma_r=0.92",
        "apex"      => "eq=brightness=0.02:contrast=1.35:saturation=1.20:gamma=1.02:gamma_b=1.10",
        "valorant"  => "eq=brightness=0.05:contrast=1.48:saturation=0.88:gamma=0.97",
        "fortnite"  => "eq=brightness=0.10:contrast=1.20:saturation=1.75:gamma_r=1.08:gamma_b=0.92",
        "cinematic" => "eq=brightness=0.02:contrast=1.18:saturation=0.80:gamma=0.97",
        "social"    => "eq=brightness=0.07:contrast=1.28:saturation=1.40:gamma_r=1.05",
        "vlog"      => "eq=brightness=0.07:contrast=1.12:saturation=1.20:gamma_r=1.10:gamma_b=0.94",
        _           => "eq=contrast=1.1:saturation=1.2",
    }.to_string()
}

fn build_ffmpeg_command(
    input: &str,
    output: &str,
    filter_complex: &str,
    plan: &EditPlan,
    hw: &HardwareProfile,
    has_audio: bool,
) -> Command {
    let mut cmd = Command::new("ffmpeg");

    // Hardware decode (zero-copy path)
    match hw.hw_decode.as_str() {
        "cuda"         => { cmd.args(["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"]); }
        "d3d11va"      => { cmd.args(["-hwaccel", "d3d11va"]); }
        "videotoolbox" => { cmd.args(["-hwaccel", "videotoolbox"]); }
        "qsv"          => { cmd.args(["-hwaccel", "qsv"]); }
        _ => {}
    }

    cmd.args(["-i", input]);

    // FPS override
    if plan.fps > 0 {
        cmd.args(["-r", &plan.fps.to_string()]);
    }

    // Unified filter graph
    cmd.args(["-filter_complex", filter_complex]);
    cmd.args(["-map", "[vout]"]);

    if has_audio {
        cmd.args(["-map", "[aout]"]);
    }

    // Hardware encoder selection
    let (vcodec, extra_args) = encoder_args(&hw.encoder, plan);
    cmd.args(["-c:v", &vcodec]);
    for arg in extra_args {
        cmd.arg(arg);
    }

    // Audio codec
    if has_audio {
        cmd.args(["-c:a", "aac", "-b:a", "192k"]);
    }

    // MP4 fast-start for web-compatible output
    cmd.args([
        "-pix_fmt", "yuv420p",
        "-movflags", "+faststart",
        "-progress", "pipe:1",   // stdout progress for parsing
        "-nostats",
        "-y",
        output,
    ]);

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    cmd
}

fn encoder_args(encoder: &str, plan: &EditPlan) -> (String, Vec<String>) {
    let (crf, bitrate) = quality_params(&plan.quality);
    match encoder {
        "h264_nvenc" => (
            "h264_nvenc".to_string(),
            vec![
                "-preset".to_string(), "p4".to_string(),
                "-tune".to_string(), "hq".to_string(),
                "-rc".to_string(), "vbr".to_string(),
                "-cq".to_string(), crf.to_string(),
                "-b:v".to_string(), format!("{}k", bitrate),
                "-maxrate".to_string(), format!("{}k", bitrate * 2),
                "-bufsize".to_string(), format!("{}k", bitrate * 3),
            ],
        ),
        "hevc_nvenc" => (
            "hevc_nvenc".to_string(),
            vec![
                "-preset".to_string(), "p5".to_string(),
                "-rc".to_string(), "vbr".to_string(),
                "-cq".to_string(), crf.to_string(),
                "-b:v".to_string(), format!("{}k", bitrate),
            ],
        ),
        "h264_amf" => (
            "h264_amf".to_string(),
            vec![
                "-quality".to_string(), "quality".to_string(),
                "-rc".to_string(), "vbr_peak".to_string(),
                "-b:v".to_string(), format!("{}k", bitrate),
            ],
        ),
        "h264_qsv" => (
            "h264_qsv".to_string(),
            vec![
                "-preset".to_string(), "slower".to_string(),
                "-global_quality".to_string(), crf.to_string(),
                "-b:v".to_string(), format!("{}k", bitrate),
            ],
        ),
        "h264_videotoolbox" => (
            "h264_videotoolbox".to_string(),
            vec![
                "-b:v".to_string(), format!("{}k", bitrate),
                "-allow_sw".to_string(), "1".to_string(),
            ],
        ),
        _ => (
            "libx264".to_string(),
            vec![
                "-preset".to_string(), "medium".to_string(),
                "-crf".to_string(), crf.to_string(),
                "-b:v".to_string(), format!("{}k", bitrate),
            ],
        ),
    }
}

fn quality_params(quality: &str) -> (u32, u32) {
    match quality {
        "draft" => (30, 1500),
        "good"  => (26, 4000),
        "high"  => (22, 8000),
        "ultra" => (18, 16000),
        "max"   => (14, 30000),
        _       => (22, 8000),
    }
}

fn resolution_to_dims(res: &str) -> (u32, u32) {
    match res {
        "720p"  => (1280,  720),
        "1080p" => (1920, 1080),
        "1440p" => (2560, 1440),
        "4k"    => (3840, 2160),
        "8k"    => (7680, 4320),
        "9:16"  => (1080, 1920),
        "1:1"   => (1080, 1080),
        _       => (1920, 1080),
    }
}

async fn execute_ffmpeg_with_progress(
    app: &AppHandle,
    job_id: &str,
    cmd: &mut Command,
    total_duration: f64,
) -> Result<(), String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to launch FFmpeg: {}", e))?;

    let stdout = child
        .stdout
        .take()
        .ok_or("Could not capture FFmpeg stdout")?;

    // IMPORTANT: stderr MUST be drained concurrently or FFmpeg will deadlock
    // when its stderr pipe buffer fills (common on large files or verbose output).
    let stderr = child
        .stderr
        .take()
        .ok_or("Could not capture FFmpeg stderr")?;

    let app_clone = app.clone();
    let job_id_str = job_id.to_string();

    // Drain stdout: parse FFmpeg -progress pipe:1 output
    let stdout_task = tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut out_time_ms: f64 = 0.0;

        while let Ok(Some(line)) = lines.next_line().await {
            if line.starts_with("out_time_ms=") {
                if let Ok(ms) = line[12..].trim().parse::<f64>() {
                    out_time_ms = ms;
                    let pct = ((out_time_ms / 1_000_000.0) / total_duration * 50.0 + 50.0)
                        .min(99.0) as u8;
                    emit_progress(
                        &app_clone,
                        &job_id_str,
                        5,
                        pct,
                        &format!("⚙️  Encoding… {:.1}%", (out_time_ms / 1_000_000.0 / total_duration * 100.0).min(100.0)),
                        None,
                        None,
                    );
                }
            }
        }
    });

    // Drain stderr: discard output but keep the pipe empty so FFmpeg never blocks
    let stderr_task = tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(_line)) = lines.next_line().await {
            // Could log to tracing::debug! here if desired
        }
    });

    let status = child
        .wait()
        .await
        .map_err(|e| format!("FFmpeg process error: {}", e))?;

    // Join drain tasks (they should finish once the child exits and pipes close)
    let _ = tokio::join!(stdout_task, stderr_task);

    if !status.success() {
        return Err(format!("FFmpeg exited with code {:?}", status.code()));
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// COMMAND: get_job_status
// ─────────────────────────────────────────────────────────────────────────────
#[tauri::command]
fn get_job_status(job_id: String, jobs: State<'_, JobRegistry>) -> Option<JobState> {
    jobs.lock().unwrap().get(&job_id).cloned()
}

// ─────────────────────────────────────────────────────────────────────────────
// COMMAND: open_output_folder
// Opens the export folder in the native file manager
// ─────────────────────────────────────────────────────────────────────────────
#[tauri::command]
async fn open_output_folder(path: String) -> Result<(), String> {
    let dir = PathBuf::from(&path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("clipmind"));

    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(dir.to_string_lossy().as_ref())
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(dir.to_string_lossy().as_ref())
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open")
            .arg(dir.to_string_lossy().as_ref())
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// ENTRY POINT
// ─────────────────────────────────────────────────────────────────────────────
fn main() {
    let job_registry: JobRegistry = Arc::new(Mutex::new(HashMap::new()));

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(job_registry)
        .invoke_handler(tauri::generate_handler![
            detect_hardware,
            open_file_dialog,
            process_video,
            get_job_status,
            open_output_folder,
        ])
        .run(tauri::generate_context!())
        .expect("ClipMind Pro failed to start");
}
