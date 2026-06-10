# ClipMind Pro — Tauri Desktop Architecture
## Project Structure

```
clipmind-pro/
├── .github/
│   └── workflows/
│       └── release.yml          ← CI/CD: cross-platform build + sign + publish
│
├── src-tauri/
│   ├── src/
│   │   └── main.rs              ← Rust core: hardware detection, FFmpeg pipeline,
│   │                               all Tauri command handlers
│   ├── Cargo.toml               ← Rust deps (tauri v2, tokio, serde, num_cpus)
│   ├── tauri.conf.json          ← App config: bundle, updater, security policy
│   └── icons/                   ← App icons (generated from src/icon.png)
│
├── frontend/
│   ├── src/
│   │   ├── main.jsx             ← React entry point
│   │   └── app.jsx              ← Main app: hardware matrix UI, file dialog,
│   │                               progress pipeline, export panel
│   ├── index.html
│   └── package.json             ← React + Vite
│
├── docs/
│   └── mobile.md                ← iOS (VideoToolbox/Metal) + Android (MediaCodec/Vulkan)
│
└── latest.json                  ← Update manifest template (published to GitHub Releases)
```

---

## Key Architecture Decisions

### Why Tauri + Rust (not Python/FastAPI/Cloud)?

| Concern | Cloud (old) | Tauri Desktop (new) |
|---|---|---|
| Vercel 504 timeout | ❌ Hard 10-15s limit | ✅ No limit — local process |
| File upload overhead | ❌ Large uploads for every clip | ✅ Pass path string only |
| GPU access | ❌ No direct GPU on serverless | ✅ Direct NVENC/AMF/VT access |
| 8K/120fps | ❌ Impossible on free tier | ✅ Hardware decides |
| Running cost | ❌ $20-200/month Railway | ✅ $0/month (GitHub Releases) |
| Privacy | ❌ Video leaves user's device | ✅ Never leaves the machine |

### Hardware Encoder Selection Logic

```
detect_hardware()
    │
    ├── nvidia-smi detectable?
    │       └─ probe h264_nvenc → use h264_nvenc + cuda hwaccel (TIER: HIGH/MID)
    │
    ├── macOS target?
    │       └─ probe h264_videotoolbox → use videotoolbox (TIER: HIGH if Apple Silicon)
    │
    ├── wmic shows AMD?
    │       └─ probe h264_amf → use h264_amf + d3d11va (TIER: MID)
    │
    ├── wmic shows Intel?
    │       └─ probe h264_qsv → use h264_qsv + qsv (TIER: MID if discrete, LOW if iGPU)
    │
    └── fallback
            └─ libx264 software encoding (TIER: LOW, capped at 1080p 60fps)
```

### FFmpeg Single-Pass Filter Graph

The entire pipeline compiles into ONE FFmpeg command:

```
ffmpeg
  -hwaccel cuda -hwaccel_output_format cuda   ← GPU decode (zero-copy)
  -i input.mp4
  -r 60
  -filter_complex "
    [0:v]
    hwupload_cuda,                             ← Upload to VRAM
    scale_cuda=3840:2160,                      ← Scale on GPU
    hwdownload, format=nv12,                   ← Download for eq filter
    eq=brightness=0.06:contrast=1.30:...,      ← Color grade
    noise=alls=14:allf=t,                      ← Film grain
    unsharp=5:5:1.2:5:5:0.0,                   ← Sharpening
    fade=t=in:st=0:d=0.4,                      ← Fade in
    fade=t=out:st=28.0:d=0.8                   ← Fade out
    [vout];
    [0:a] anull [aout]"
  -map [vout] -map [aout]
  -c:v h264_nvenc                              ← GPU encode
    -preset p4 -tune hq -rc vbr
    -cq 22 -b:v 8000k
  -c:a aac -b:a 192k
  -pix_fmt yuv420p
  -movflags +faststart
  -progress pipe:1                             ← Live progress to Rust
  output.mp4
```

No intermediate files. No temp splits. One pass.

### Updater Flow

```
App launch
    │
    ▼
tauri-plugin-updater checks endpoints:
  https://releases.clipmind.io/latest.json
  https://raw.githubusercontent.com/.../latest.json
    │
    ├── version matches → continue
    │
    └── new version → show dialog "Update available: v1.x.x"
            ├── User accepts → download .msi/.dmg, verify signature, install
            └── User skips  → continue with current version
```

The `latest.json` file is generated automatically by the GitHub Actions workflow
on every tagged release and pushed as a release asset.

---

## Getting Started (Development)

```bash
# Prerequisites: Rust, Node 20+, FFmpeg on PATH

# Install Tauri CLI
cargo install tauri-cli --version "^2.0" --locked

# Install frontend deps
cd frontend && npm install && cd ..

# Dev mode (hot-reload frontend + Rust recompile on change)
cargo tauri dev

# Production build
cargo tauri build
```

## Generating Updater Keys

```bash
# Generate a keypair for signing updates (run once, store private key in GitHub secrets)
cargo tauri signer generate -w ~/.tauri/clipmind-pro.key

# Output:
#   Private key: ~/.tauri/clipmind-pro.key     → TAURI_SIGNING_PRIVATE_KEY
#   Public key: printed to stdout              → paste into tauri.conf.json "pubkey"
```
