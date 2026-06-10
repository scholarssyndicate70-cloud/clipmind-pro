# ClipMind Pro — Debugging Report

All bugs found, their root causes, and the fixes applied across every file.

---

## `src-tauri/src/main.rs`

### BUG 1 — `get_system_ram_gb`: Returns 8 GB on macOS and Windows (CRITICAL)
**File:** `main.rs` · `get_system_ram_gb()`  
**Impact:** Tier detection (`compute_tier`) reads RAM as a key input. On macOS and Windows — the primary deployment targets — it always returned `8.0 GB`, causing high-RAM machines to be incorrectly classified as "low" or "mid" tier, locking users out of 4K/8K/120fps options they could actually use.  
**Fix:** Added platform-specific branches:
- **macOS:** `sysctl -n hw.memsize` (returns bytes → convert to GB)
- **Windows:** `wmic ComputerSystem get TotalPhysicalMemory`
- **Linux:** unchanged (`/proc/meminfo`)

---

### BUG 2 — `open_file_dialog`: Blocking call on async Tauri thread (CRASH)
**File:** `main.rs` · `open_file_dialog()`  
**Impact:** `blocking_pick_file()` is a synchronous API. Calling it from within an `async fn` that runs on Tokio's async executor blocks the thread, which can deadlock the Tauri event loop — particularly on single-threaded runtimes or when multiple commands are in-flight.  
**Fix:** Replaced with the async `pick_file(callback)` variant, bridged to Tokio via a `oneshot::channel`.

---

### BUG 3 — `probe_video`: Loose audio stream detection (WRONG RESULTS)
**File:** `main.rs` · `probe_video()`  
**Impact:** `text.contains("\"audio\"")` matches any JSON field whose value happens to contain the word "audio" (e.g. a title tag `"title":"audio track"` or `"handler_name":"SoundHandler"`). This causes the pipeline to try to map `[0:a]` in the filter graph even when no audio stream exists, making FFmpeg error out with `Stream specifier 0:a does not match any streams`.  
**Fix:** Changed to `text.contains("\"codec_type\":\"audio\"")`, which only matches the actual codec_type field set by ffprobe for audio streams.

---

### BUG 4 — `build_filter_complex` (non-CUDA path): Duplicate scale filter (FFMPEG ERROR)
**File:** `main.rs` · `build_filter_complex()`  
**Impact:** When not using CUDA, the code first pushed the `scale_filter` string, then immediately pushed a second `scale=WxH:force_original_aspect_ratio=increase` filter. This produced an invalid filter graph with two consecutive scale filters, causing FFmpeg to error on the second `scale` call (the dimensions from the first pass were already set and conflicted).  
**Fix:** Removed the initial `scale_filter` push in the else-branch. The single `scale=W:H:force_original_aspect_ratio=increase:flags=lanczos` followed by `crop=W:H` is the correct pattern.

---

### BUG 5 — `build_filter_complex`: A/V desync on speed ramps (AUDIO DRIFT)
**File:** `main.rs` · `build_filter_complex()`  
**Impact:** `setpts` slows or speeds up the video track's timestamps. The audio track was left on `anull` (pass-through), so after a speed ramp the video and audio progressively drift out of sync for the rest of the clip.  
**Fix:** The speed ramp factor is now captured from the cut and used to compute an inverse `atempo` value (clamped to FFmpeg's supported `[0.5, 2.0]` range) applied to the audio filter chain.

---

### BUG 6 — `build_filter_complex`: Invalid audio pad for synthesized silence (FFMPEG ERROR)
**File:** `main.rs` · `build_filter_complex()`  
**Impact:** For clips with no audio stream, the filter graph was:  
`[0:a]aevalsrc=0:...[aout]`  
`aevalsrc` is a **source** filter — it generates audio from nothing. It has no input, so prepending `[0:a]` makes FFmpeg look for a stream that doesn't exist, producing `Filtering for audio stream 0:a failed`.  
Additionally, the duration was hardcoded to `d=300` (5 minutes), padding every silent clip to 5 min.  
**Fix:** The audio source string no longer includes `[0:a]` as input, and uses the probed `duration` variable for the `d=` parameter.

---

### BUG 7 — `execute_ffmpeg_with_progress`: stderr not drained → deadlock on large files (CRITICAL)
**File:** `main.rs` · `execute_ffmpeg_with_progress()`  
**Impact:** FFmpeg writes progress to stdout and diagnostics/warnings to stderr. When the OS pipe buffer for stderr fills (typically 64 KB — easily hit on long encodes or when hardware encoders emit verbose output), FFmpeg blocks waiting for the reader. Since nothing was reading stderr, the child process would hang indefinitely, and `child.wait()` would never return.  
**Fix:** A second `tokio::spawn` task is started to continuously drain stderr. Both tasks are joined after `child.wait()` returns so the pipes close cleanly.

---

## `frontend/src/app.jsx`

### BUG 8 — `pipeline-progress` listener: Stale `jobId` closure drops early events (SILENT DATA LOSS)
**File:** `app.jsx` · `useEffect` for `listen("pipeline-progress", ...)`  
**Impact:** `setJobId(id)` is asynchronous — the state update is batched by React and doesn't propagate to the closure immediately. The listener captured `jobId` at render time (still `null` from the previous render). The guard `if (d.job_id !== jobId && jobId !== null)` evaluated `null !== null` → `false`, so it passed — but as soon as React committed the new state, the closure saw the new `jobId` for subsequent renders. However, any events fired in the gap between `setJobId` and React's re-render were silently filtered. On fast machines the "Probing video" and "Building filter graph" steps were always missed.  
**Fix:** Added a `jobIdRef` (via `useRef`) that is set synchronously before `setJobId`. The listener guard now reads `jobIdRef.current`. The effect dependency array is `[]` (mount-only) since the ref provides the live value without requiring re-subscription.

---

### BUG 9 — `startProcess`: `timestamp: null` in fade_out cut causes Rust deserialization panic (CRASH)
**File:** `app.jsx` · `startProcess()`  
**Impact:** The `Cut` struct in Rust defines `timestamp: f64` (not `Option<f64>`). Sending `null` from JS serializes to JSON `null`, which `serde_json` cannot deserialize into a bare `f64` — it panics with `invalid type: null, expected f64`. This caused the Tauri command to return an error on every single process attempt.  
**Fix:** Removed the `fade_out` cut from the hardcoded plan entirely. Fades are already driven by the `fades` option flag (checked in `build_filter_complex`), making the explicit fade_out cut entry redundant.

---

## `src-tauri/tauri.conf.json`

### BUG 10 — `pubkey` field is a literal placeholder string (STARTUP CRASH on release builds)
**File:** `tauri.conf.json` · `plugins.updater.pubkey`  
**Impact:** `tauri-plugin-updater` validates the public key format at application startup. The string `"YOUR_UPDATER_PUBLIC_KEY_HERE"` is not a valid minisign public key. On release builds the plugin init fails and the app crashes before the window opens. Debug builds skip updater init so the bug is invisible in development.  
**Fix:** Changed the value to a descriptive command string: `"REPLACE_WITH_OUTPUT_OF: cargo tauri signer generate -w ~/.tauri/clipmind-pro.key"` — still obviously a placeholder but formatted as an instruction. **Action required:** Run the command and paste the actual base64 public key before shipping.

---

## `.github/workflows/release.yml`

### BUG 11 — `Generate latest.json`: `cat *.sig` concatenates multiple files on re-runs (CORRUPT SIGNATURES)
**File:** `release.yml` · `Generate latest.json` step  
**Impact:** `cat dist/macos-aarch64-apple-darwin/*.app.tar.gz.sig` expands the glob. If a workflow re-run produces a second `.sig` file in the same artifact directory, `cat` outputs both signatures concatenated into one string. The Tauri updater's signature verification then fails for all users with a cryptographic mismatch error.  
**Fix:** Changed `cat` to `head -1` to always read exactly one line from the first matching file, making signature extraction idempotent across re-runs.

---

### BUG 12 — `Generate latest.json`: NSIS `.exe` has a signature but no corresponding URL entry (BROKEN NSIS UPDATES)
**File:** `release.yml` · `Generate latest.json` step  
**Impact:** `SIG_WIN_EXE` was computed but never used — it was a dead variable. Windows users who installed via the NSIS installer (`.exe`) cannot auto-update because the `latest.json` only contains the MSI path. The updater picks the installer type that matches how the app was installed.  
**Fix:** Added a `"windows-x86_64-nsis"` platform entry in the generated `latest.json` that carries the NSIS `.exe` URL and its separate signature.

---

## Summary Table

| # | File | Function / Location | Severity | Category |
|---|------|-------------------|----------|----------|
| 1 | `main.rs` | `get_system_ram_gb` | 🔴 High | Wrong results on macOS/Windows |
| 2 | `main.rs` | `open_file_dialog` | 🔴 High | Potential deadlock |
| 3 | `main.rs` | `probe_video` | 🟠 Med | False positive audio detection |
| 4 | `main.rs` | `build_filter_complex` (non-CUDA) | 🔴 High | FFmpeg crash |
| 5 | `main.rs` | `build_filter_complex` (speed ramp) | 🟠 Med | A/V desync |
| 6 | `main.rs` | `build_filter_complex` (aevalsrc) | 🔴 High | FFmpeg crash on silent clips |
| 7 | `main.rs` | `execute_ffmpeg_with_progress` | 🔴 High | Deadlock on large files |
| 8 | `app.jsx` | `listen("pipeline-progress")` | 🟠 Med | Stale closure drops events |
| 9 | `app.jsx` | `startProcess` | 🔴 High | Rust deserialization panic |
| 10 | `tauri.conf.json` | `plugins.updater.pubkey` | 🔴 High | Startup crash on release |
| 11 | `release.yml` | `Generate latest.json` | 🟠 Med | Corrupt update signatures |
| 12 | `release.yml` | `Generate latest.json` | 🟡 Low | NSIS update path broken |
