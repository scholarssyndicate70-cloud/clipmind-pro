// ClipMind Pro — React Frontend (Tauri Desktop)
// File selection via native OS dialog. No file uploads. No browser Blobs.

import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen }  from "@tauri-apps/api/event";

// ── Static config ─────────────────────────────────────────────────────────────
const STYLES = [
  { id: "freefire",  label: "Free Fire",    emoji: "🔥" },
  { id: "warzone",   label: "Warzone",      emoji: "💀" },
  { id: "apex",      label: "Apex",         emoji: "⚡" },
  { id: "valorant",  label: "Valorant",     emoji: "🎯" },
  { id: "fortnite",  label: "Fortnite",     emoji: "🏆" },
  { id: "cinematic", label: "Cinematic",    emoji: "🎬" },
  { id: "social",    label: "Shorts/Reels", emoji: "📱" },
  { id: "vlog",      label: "Vlog",         emoji: "🎙️" },
];

const ALL_OPTIONS = [
  { id: "cuts",      label: "Smart Cuts",    icon: "✂️",  defaultOn: true  },
  { id: "zooms",     label: "Zoom Punches",  icon: "🔍",  defaultOn: true  },
  { id: "color",     label: "Color Grade",   icon: "🎨",  defaultOn: true  },
  { id: "captions",  label: "Title Cards",   icon: "📝",  defaultOn: true  },
  { id: "fades",     label: "Fade In/Out",   icon: "🎬",  defaultOn: true  },
  { id: "grain",     label: "Film Grain",    icon: "📻",  defaultOn: false },
  { id: "letterbox", label: "Letterbox",     icon: "🖼️",  defaultOn: false },
  { id: "music",     label: "Music",         icon: "🎵",  defaultOn: true  },
];

const ALL_RESOLUTIONS = [
  { id: "720p",  label: "720p",   note: "" },
  { id: "1080p", label: "1080p",  note: "Best" },
  { id: "1440p", label: "1440p",  note: "⚡ Mid+" },
  { id: "4k",    label: "4K",     note: "⚡ Mid+" },
  { id: "8k",    label: "8K",     note: "🚀 High" },
  { id: "9:16",  label: "9:16",   note: "Vertical" },
  { id: "1:1",   label: "1:1",    note: "Square" },
];

const ALL_FPS    = [24, 30, 60, 120];
const QUALITIES  = [
  { id: "draft", label: "Draft" },
  { id: "good",  label: "Good"  },
  { id: "high",  label: "High"  },
  { id: "ultra", label: "Ultra" },
  { id: "max",   label: "Max"   },
];

const PIPELINE_STEPS = [
  "Idle",
  "Probing video",
  "Building filter graph",
  "Starting encoder",
  "Encoding",
  "Post-processing",
  "Packaging",
  "Done",
];

// ── CSS ───────────────────────────────────────────────────────────────────────
const css = `
  *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
  :root {
    --bg:      #080810;
    --bg2:     #0f0f1a;
    --bg3:     #181828;
    --bg4:     #212134;
    --border:  #2a2a40;
    --text:    #e4e4f0;
    --muted:   #60607a;
    --red:     #e63c50;
    --red2:    #c42e40;
    --green:   #22c55e;
    --blue:    #3b82f6;
    --yellow:  #eab308;
    --amber:   #f59e0b;
    --radius:  10px;
    --font: 'DM Sans', -apple-system, sans-serif;
  }
  html, body, #root { height: 100%; background: var(--bg); color: var(--text); font-family: var(--font); overflow: hidden; }
  ::-webkit-scrollbar { width: 5px; }
  ::-webkit-scrollbar-track { background: var(--bg2); }
  ::-webkit-scrollbar-thumb { background: var(--border); border-radius: 3px; }

  .app { display: flex; flex-direction: column; height: 100vh; }
  .titlebar { display: flex; align-items: center; justify-content: space-between; padding: 0 1.25rem; height: 52px; background: var(--bg2); border-bottom: 1px solid var(--border); -webkit-app-region: drag; user-select: none; flex-shrink: 0; }
  .titlebar-logo { font-size: 1.1rem; font-weight: 700; letter-spacing: 0.14em; }
  .titlebar-logo span { color: var(--red); }
  .titlebar-right { display: flex; align-items: center; gap: 0.5rem; -webkit-app-region: no-drag; }
  .badge { padding: 3px 8px; border-radius: 6px; font-size: 0.65rem; font-weight: 700; letter-spacing: 0.08em; }
  .badge-gpu { background: var(--bg4); color: var(--blue); border: 1px solid #1e3a5f; }
  .badge-tier-high   { background: #0a1f0a; color: #4ade80; border: 1px solid #1a4a1a; }
  .badge-tier-mid    { background: #1a1500; color: var(--amber); border: 1px solid #3a3000; }
  .badge-tier-low    { background: #1a0a0a; color: #f87171; border: 1px solid #3a1a1a; }
  .badge-tier-unknown { background: var(--bg4); color: var(--muted); border: 1px solid var(--border); }

  .workspace { display: grid; grid-template-columns: 270px 1fr 290px; flex: 1; overflow: hidden; }

  /* Left panel */
  .panel-left { background: var(--bg2); border-right: 1px solid var(--border); overflow-y: auto; display: flex; flex-direction: column; }
  .section { padding: 0.8rem 0.9rem; border-bottom: 1px solid var(--border); }
  .section-label { font-size: 0.6rem; font-weight: 700; letter-spacing: 0.13em; color: var(--muted); text-transform: uppercase; margin-bottom: 0.55rem; }
  .style-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 0.35rem; }
  .style-btn { display: flex; flex-direction: column; align-items: center; gap: 0.2rem; padding: 0.55rem 0.3rem; background: var(--bg3); border: 1px solid var(--border); border-radius: 9px; cursor: pointer; font-size: 0.68rem; color: var(--muted); transition: all 0.15s; }
  .style-btn:hover { border-color: var(--red); color: var(--text); }
  .style-btn.active { background: #160a0d; border-color: var(--red); color: var(--text); }
  .style-btn .em { font-size: 1.3rem; }
  .opt-row { display: flex; align-items: center; justify-content: space-between; padding: 0.38rem 0; }
  .opt-label { font-size: 0.75rem; color: var(--text); display: flex; align-items: center; gap: 0.35rem; }
  .toggle { width: 34px; height: 18px; background: var(--bg4); border-radius: 9px; cursor: pointer; position: relative; transition: background 0.2s; border: none; flex-shrink: 0; }
  .toggle.on { background: var(--red); }
  .toggle::after { content:''; position:absolute; width:12px; height:12px; background:#fff; border-radius:50%; top:3px; left:3px; transition:transform 0.2s; }
  .toggle.on::after { transform:translateX(16px); }
  .toggle:disabled { opacity: 0.35; cursor: not-allowed; }

  /* Hardware panel */
  .hw-row { display: flex; align-items: center; justify-content: space-between; padding: 0.3rem 0; font-size: 0.72rem; }
  .hw-label { color: var(--muted); }
  .hw-value { color: var(--text); font-weight: 500; max-width: 140px; text-align: right; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .hw-warn { background: #160a0a; border: 1px solid #3a1a1a; border-radius: 7px; padding: 0.45rem 0.6rem; font-size: 0.68rem; color: #f87171; line-height: 1.45; margin-top: 0.4rem; }
  .hw-scanning { display: flex; align-items: center; gap: 0.5rem; font-size: 0.75rem; color: var(--muted); }
  .spin { animation: spin 1s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }

  /* Center panel */
  .panel-center { background: var(--bg); display: flex; flex-direction: column; overflow: hidden; }
  .dropzone { flex: 1; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 0.85rem; border: 2px dashed var(--border); margin: 1.25rem; border-radius: 18px; cursor: pointer; transition: all 0.2s; padding: 2rem; }
  .dropzone:hover { border-color: var(--red); background: #13080a; }
  .dropzone-icon { font-size: 2.8rem; }
  .dropzone-title { font-size: 1.05rem; font-weight: 600; }
  .dropzone-sub { font-size: 0.74rem; color: var(--muted); text-align: center; line-height: 1.5; }
  .dropzone-btn { padding: 0.55rem 1.4rem; background: var(--red); color: #fff; border: none; border-radius: 8px; font-size: 0.82rem; font-weight: 600; cursor: pointer; transition: background 0.15s; }
  .dropzone-btn:hover { background: var(--red2); }
  .dropzone-note { font-size: 0.68rem; color: var(--muted); margin-top: 0.25rem; }

  .file-bar { display: flex; align-items: center; gap: 0.7rem; padding: 0.65rem 1.25rem; background: var(--bg2); border-bottom: 1px solid var(--border); flex-shrink: 0; }
  .file-icon { width: 36px; height: 36px; background: var(--bg4); border-radius: 7px; display: flex; align-items: center; justify-content: center; font-size: 1.1rem; flex-shrink: 0; }
  .file-info { flex: 1; min-width: 0; }
  .file-name { font-size: 0.8rem; font-weight: 500; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .file-path { font-size: 0.66rem; color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .file-remove { background: none; border: none; color: var(--muted); cursor: pointer; font-size: 0.75rem; padding: 4px 8px; border-radius: 5px; }
  .file-remove:hover { background: var(--bg4); color: var(--text); }

  .progress-card { margin: 1.25rem; background: var(--bg2); border: 1px solid var(--border); border-radius: 16px; padding: 1.25rem; flex-shrink: 0; }
  .progress-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.9rem; }
  .progress-title { font-size: 0.85rem; font-weight: 600; }
  .progress-timer { font-size: 0.7rem; color: var(--muted); }
  .ring-wrap { display: flex; justify-content: center; margin-bottom: 1rem; }
  .prog-ring { width: 92px; height: 92px; }
  .ring-bg { fill: none; stroke: var(--bg4); stroke-width: 7; }
  .ring-fg { fill: none; stroke-width: 7; stroke-linecap: round; transform: rotate(-90deg); transform-origin: 50% 50%; transition: stroke-dashoffset 0.5s, stroke 0.5s; }
  .prog-status { font-size: 0.73rem; color: var(--muted); text-align: center; margin-bottom: 0.9rem; }
  .steps-list { display: flex; flex-direction: column; gap: 0.28rem; }
  .step-row { display: flex; align-items: center; gap: 0.55rem; font-size: 0.7rem; color: var(--muted); }
  .step-row.done { color: var(--green); }
  .step-row.active { color: var(--text); font-weight: 500; }
  .step-dot { width: 6px; height: 6px; border-radius: 50%; background: var(--bg4); flex-shrink: 0; }
  .step-dot.done { background: var(--green); }
  .step-dot.active { background: var(--red); box-shadow: 0 0 5px var(--red); }

  .action-bar { padding: 0.9rem 1.25rem; flex-shrink: 0; }
  .process-btn { width: 100%; padding: 0.8rem; background: var(--red); color: #fff; border: none; border-radius: 10px; font-size: 0.9rem; font-weight: 700; cursor: pointer; transition: all 0.15s; letter-spacing: 0.03em; }
  .process-btn:hover:not(:disabled) { background: var(--red2); transform: translateY(-1px); }
  .process-btn:disabled { opacity: 0.45; cursor: not-allowed; transform: none; }
  .process-btn.running { animation: pulse 1.4s ease-in-out infinite; }
  @keyframes pulse { 0%,100% { opacity:1; } 50% { opacity:0.7; } }

  .center-msg { flex: 1; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 0.6rem; color: var(--muted); font-size: 0.8rem; }
  .center-msg .big { font-size: 2.2rem; }

  /* Right panel */
  .panel-right { background: var(--bg2); border-left: 1px solid var(--border); overflow-y: auto; }
  .export-section { padding: 0.7rem 0.8rem; border-bottom: 1px solid var(--border); }
  .export-label { font-size: 0.6rem; font-weight: 700; letter-spacing: 0.12em; color: var(--muted); text-transform: uppercase; margin-bottom: 0.45rem; }
  .btn-grid { display: grid; gap: 0.3rem; }
  .btn-grid.c2 { grid-template-columns: 1fr 1fr; }
  .btn-grid.c3 { grid-template-columns: 1fr 1fr 1fr; }
  .sel-btn { padding: 0.4rem 0.25rem; background: var(--bg3); border: 1px solid var(--border); border-radius: 7px; color: var(--muted); font-size: 0.7rem; cursor: pointer; text-align: center; transition: all 0.15s; position: relative; }
  .sel-btn:hover:not(:disabled) { border-color: var(--red); color: var(--text); }
  .sel-btn.active { background: #160a0d; border-color: var(--red); color: var(--text); }
  .sel-btn:disabled { opacity: 0.3; cursor: not-allowed; }
  .sel-btn .note { display: block; font-size: 0.58rem; color: var(--muted); margin-top: 1px; }
  .sel-btn.active .note { color: #f87171; }
  .sel-btn .locked-icon { position: absolute; top: 3px; right: 4px; font-size: 0.55rem; color: var(--muted); }

  .result-empty { display: flex; flex-direction: column; align-items: center; justify-content: center; padding: 3rem 1.5rem; gap: 0.6rem; color: var(--muted); text-align: center; }
  .result-empty .ei { font-size: 2.2rem; opacity: 0.35; }
  .dl-section { padding: 0.7rem 0.8rem; border-bottom: 1px solid var(--border); }
  .dl-btn { width: 100%; padding: 0.7rem; background: var(--green); color: #fff; border: none; border-radius: 10px; font-size: 0.85rem; font-weight: 700; cursor: pointer; margin-top: 0.35rem; }
  .dl-btn:hover { filter: brightness(1.1); }
  .reveal-btn { width: 100%; padding: 0.5rem; background: var(--bg3); border: 1px solid var(--border); color: var(--text); border-radius: 8px; font-size: 0.73rem; cursor: pointer; margin-top: 0.35rem; }
  .reveal-btn:hover { border-color: var(--red); }
  .size-badge { display: inline-flex; align-items: center; gap: 0.25rem; padding: 2px 8px; background: var(--bg4); border: 1px solid var(--border); border-radius: 20px; font-size: 0.65rem; color: var(--muted); }
  .result-block { border-bottom: 1px solid var(--border); }
  .result-hdr { padding: 0.55rem 0.75rem; font-size: 0.6rem; font-weight: 700; letter-spacing: 0.11em; color: var(--muted); text-transform: uppercase; }
  .result-body { padding: 0 0.75rem 0.65rem; }
  .result-text { font-size: 0.72rem; color: var(--text); line-height: 1.5; }
`;

// ── Helpers ───────────────────────────────────────────────────────────────────
function fmt(s) {
  return s > 60
    ? `${Math.floor(s / 60)}m ${s % 60}s`
    : `${s}s`;
}

function baseName(path) {
  return path?.split(/[\\/]/).pop() ?? "";
}

// ── Sub-components ────────────────────────────────────────────────────────────
function Toggle({ on, onChange, disabled }) {
  return (
    <button
      className={`toggle ${on ? "on" : ""}`}
      onClick={() => !disabled && onChange(!on)}
      disabled={disabled}
      aria-pressed={on}
    />
  );
}

function ProgressRing({ pct }) {
  const r = 38;
  const c = 2 * Math.PI * r;
  const offset = c - (pct / 100) * c;
  const color = pct === 100 ? "#22c55e" : pct > 60 ? "#e63c50" : "#3b82f6";
  return (
    <svg className="prog-ring" viewBox="0 0 92 92">
      <circle className="ring-bg" cx="46" cy="46" r={r} />
      <circle
        className="ring-fg"
        cx="46" cy="46" r={r}
        stroke={color}
        strokeDasharray={c}
        strokeDashoffset={offset}
      />
      <text
        x="46" y="46"
        textAnchor="middle" dominantBaseline="central"
        fill="var(--text)" fontSize="16" fontWeight="700"
      >
        {pct}%
      </text>
    </svg>
  );
}

function HardwarePanel({ hw, scanning }) {
  if (scanning) {
    return (
      <div className="section">
        <div className="section-label">Hardware</div>
        <div className="hw-scanning">
          <span className="spin">⟳</span> Scanning hardware…
        </div>
      </div>
    );
  }

  if (!hw) return null;

  const tierBadgeClass = {
    high:    "badge-tier-high",
    mid:     "badge-tier-mid",
    low:     "badge-tier-low",
    unknown: "badge-tier-unknown",
  }[hw.tier] ?? "badge-tier-unknown";

  return (
    <div className="section">
      <div className="section-label" style={{ display:"flex", justifyContent:"space-between" }}>
        <span>Hardware</span>
        <span className={`badge ${tierBadgeClass}`}>
          {{ high:"🚀 HIGH", mid:"⚡ MID", low:"⚠ LOW" }[hw.tier] ?? hw.tier.toUpperCase()}
        </span>
      </div>
      <div className="hw-row"><span className="hw-label">CPU</span><span className="hw-value">{hw.cpu_cores} cores</span></div>
      <div className="hw-row"><span className="hw-label">RAM</span><span className="hw-value">{hw.ram_gb.toFixed(0)} GB</span></div>
      <div className="hw-row"><span className="hw-label">GPU</span><span className="hw-value">{hw.gpu_name}</span></div>
      <div className="hw-row"><span className="hw-label">Encoder</span><span className="hw-value" style={{ color: hw.encoder !== "libx264" ? "var(--green)" : "var(--amber)" }}>{hw.encoder}</span></div>
      <div className="hw-row"><span className="hw-label">Max output</span><span className="hw-value">{hw.max_resolution} @ {hw.max_fps}fps</span></div>
      {hw.warnings.map((w, i) => (
        <div key={i} className="hw-warn">⚠ {w}</div>
      ))}
    </div>
  );
}

function ExportPanel({ hw, resolution, setResolution, fps, setFps, quality, setQuality, dlPath, fileMb, onRevealInFinder }) {
  const unlocked_res = hw?.unlocked_resolutions ?? ALL_RESOLUTIONS.map(r => r.id);
  const unlocked_fps = hw?.unlocked_fps ?? ALL_FPS;

  return (
    <div className="panel-right">
      {dlPath && (
        <div className="dl-section">
          {fileMb > 0 && <div className="size-badge">📦 {fileMb} MB</div>}
          <button className="dl-btn" onClick={() => invoke("open_output_folder", { path: dlPath })}>
            📂 Show in Finder / Explorer
          </button>
        </div>
      )}

      <div className="export-section">
        <div className="export-label">Resolution</div>
        <div className="btn-grid c2">
          {ALL_RESOLUTIONS.map(r => {
            const locked = !unlocked_res.includes(r.id);
            return (
              <button
                key={r.id}
                className={`sel-btn ${resolution === r.id ? "active" : ""}`}
                disabled={locked}
                onClick={() => !locked && setResolution(r.id)}
                title={locked ? `Requires ${hw?.tier === "low" ? "mid" : "high"}-tier hardware` : ""}
              >
                {r.label}
                {r.note && <span className="note">{r.note}</span>}
                {locked && <span className="locked-icon">🔒</span>}
              </button>
            );
          })}
        </div>
      </div>

      <div className="export-section">
        <div className="export-label">Frame Rate</div>
        <div className="btn-grid c2">
          {ALL_FPS.map(f => {
            const locked = !unlocked_fps.includes(f);
            return (
              <button
                key={f}
                className={`sel-btn ${fps === f ? "active" : ""}`}
                disabled={locked}
                onClick={() => !locked && setFps(f)}
              >
                {f} fps
                {f === 120 && <span className="note">High tier</span>}
                {locked && <span className="locked-icon">🔒</span>}
              </button>
            );
          })}
        </div>
      </div>

      <div className="export-section">
        <div className="export-label">Quality</div>
        <div className="btn-grid c2">
          {QUALITIES.map(q => (
            <button
              key={q.id}
              className={`sel-btn ${quality === q.id ? "active" : ""}`}
              onClick={() => setQuality(q.id)}
            >
              {q.label}
            </button>
          ))}
        </div>
      </div>

      {!dlPath && (
        <div className="result-empty">
          <div className="ei">🎬</div>
          <div style={{ fontSize: "0.78rem", fontWeight: 600, color: "var(--text)" }}>
            Results appear here
          </div>
          <div style={{ fontSize: "0.68rem" }}>Process a clip to see output</div>
        </div>
      )}
    </div>
  );
}

// ── Main App ──────────────────────────────────────────────────────────────────
export default function App() {
  // Hardware
  const [hw, setHw]             = useState(null);
  const [hwScanning, setHwScanning] = useState(true);

  // File selection (path only — no File object, no upload)
  const [filePath, setFilePath] = useState(null);

  // Settings
  const [style, setStyle]       = useState("freefire");
  const [opts, setOpts]         = useState(
    () => new Set(ALL_OPTIONS.filter(o => o.defaultOn).map(o => o.id))
  );
  const [resolution, setRes]    = useState("1080p");
  const [fps, setFps]           = useState(60);
  const [quality, setQuality]   = useState("high");

  // Pipeline
  const [jobId, setJobId]       = useState(null);
  const [processing, setProcessing] = useState(false);
  const [progress, setProgress] = useState(0);
  const [status, setStatus]     = useState("");
  const [step, setStep]         = useState(0);
  const [elapsed, setElapsed]   = useState(0);

  // Results
  const [dlPath, setDlPath]     = useState(null);
  const [fileMb, setFileMb]     = useState(0);

  const jobIdRef = useRef(null);
  const timerRef  = useRef(null);
  const unlistenRef = useRef(null);

  // ── Hardware detection on mount ──────────────────────────────────────────
  useEffect(() => {
    invoke("detect_hardware")
      .then(profile => {
        setHw(profile);
        // Auto-select best available defaults within hardware limits
        const validRes = ALL_RESOLUTIONS.find(r =>
          profile.unlocked_resolutions.includes(r.id) && r.id === "1080p"
        );
        if (validRes) setRes(validRes.id);
        const validFps = profile.unlocked_fps.includes(60) ? 60 : profile.unlocked_fps[profile.unlocked_fps.length - 1];
        setFps(validFps);
      })
      .catch(e => console.error("Hardware detection failed:", e))
      .finally(() => setHwScanning(false));
  }, []);

  // ── Listen for pipeline progress events from Rust ─────────────────────────
  useEffect(() => {
    let unlisten;
    listen("pipeline-progress", (event) => {
      const d = event.payload;
      // Use ref instead of state to avoid stale closure race condition:
      // setJobId is async, so the first few events would be dropped if we
      // compared against the jobId state variable directly.
      if (d.job_id !== jobIdRef.current && jobIdRef.current !== null) return;

      setProgress(d.progress ?? 0);
      setStatus(d.status ?? "");
      setStep(d.step ?? 0);

      if (d.download_path) {
        setDlPath(d.download_path);
        setProcessing(false);
      }
      if (d.step === -1) {
        setProcessing(false);
      }
    }).then(u => { unlisten = u; unlistenRef.current = u; });

    return () => { if (unlisten) unlisten(); };
  }, []); // mount-only; jobId access via ref

  // ── Elapsed timer ─────────────────────────────────────────────────────────
  useEffect(() => {
    if (processing) {
      setElapsed(0);
      timerRef.current = setInterval(() => setElapsed(e => e + 1), 1000);
    } else {
      clearInterval(timerRef.current);
    }
    return () => clearInterval(timerRef.current);
  }, [processing]);

  // ── Open native file dialog ────────────────────────────────────────────────
  const pickFile = useCallback(async () => {
    try {
      const path = await invoke("open_file_dialog");
      if (path) {
        setFilePath(path);
        setDlPath(null);
        setProgress(0);
        setStep(0);
        setStatus("");
      }
    } catch (e) {
      console.error("File dialog error:", e);
    }
  }, []);

  // ── Start processing ───────────────────────────────────────────────────────
  const startProcess = useCallback(async () => {
    if (!filePath || processing || !hw) return;

    const id = `cm_${Date.now().toString(36)}`;
    jobIdRef.current = id;   // set ref immediately to avoid stale closure in listener
    setJobId(id);
    setProcessing(true);
    setProgress(2);
    setStep(0);
    setStatus("Starting pipeline…");
    setDlPath(null);
    setFileMb(0);

    // Build a minimal edit plan — in production this would come from the AI analyzer.
    // NOTE: fade_out timestamp is intentionally omitted (null/undefined) here because
    // the Rust backend computes it from the probed video duration. The Cut struct
    // has Option<f64> for duration and timestamp fields; passing null serializes to None.
    const plan = {
      style,
      options: [...opts],
      cuts: [
        { cut_type: "fade_in",   timestamp: 0.0,  duration: 0.4 },
        { cut_type: "zoom",      timestamp: 3.0,  duration: 1.5 },
        // fade_out: backend handles this via the `fades` option flag instead
      ],
      captions: [
        { text: "UNREAL PLAY! 🔥", start: 0.5, end: 2.5 },
      ],
      resolution,
      fps,
      quality,
    };

    try {
      await invoke("process_video", {
        jobId: id,
        inputPath: filePath,
        plan,
        hardware: hw,
      });
    } catch (e) {
      setStatus(`❌ ${e}`);
      setProcessing(false);
    }
  }, [filePath, processing, hw, style, opts, resolution, fps, quality]);

  const toggleOpt = (id) => {
    setOpts(prev => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  };

  const canProcess = !!filePath && !processing && !!hw && !hwScanning;

  const tierBadge = hw
    ? <span className={`badge ${{ high:"badge-tier-high", mid:"badge-tier-mid", low:"badge-tier-low" }[hw.tier] ?? "badge-tier-unknown"}`}>
        {hw.encoder !== "libx264" ? hw.encoder : "software"}
      </span>
    : null;

  return (
    <div className="app">
      <style>{css}</style>

      {/* ── Titlebar ─────────────────────────────────────────────────── */}
      <div className="titlebar">
        <div className="titlebar-logo">CLIP<span>MIND</span> PRO</div>
        <div className="titlebar-right">
          {hw && <span className="badge badge-gpu">{hw.gpu_name.split(" ").slice(0,3).join(" ")}</span>}
          {tierBadge}
        </div>
      </div>

      {/* ── Workspace ────────────────────────────────────────────────── */}
      <div className="workspace">

        {/* ── Left: Style + Options + Hardware ── */}
        <div className="panel-left">
          <div className="section">
            <div className="section-label">Game / Style</div>
            <div className="style-grid">
              {STYLES.map(s => (
                <button
                  key={s.id}
                  className={`style-btn ${style === s.id ? "active" : ""}`}
                  onClick={() => setStyle(s.id)}
                >
                  <span className="em">{s.emoji}</span>
                  {s.label}
                </button>
              ))}
            </div>
          </div>

          <div className="section">
            <div className="section-label">Edit Options</div>
            {ALL_OPTIONS.map(o => (
              <div key={o.id} className="opt-row">
                <span className="opt-label">{o.icon} {o.label}</span>
                <Toggle
                  on={opts.has(o.id)}
                  onChange={() => toggleOpt(o.id)}
                  disabled={processing}
                />
              </div>
            ))}
          </div>

          <HardwarePanel hw={hw} scanning={hwScanning} />
        </div>

        {/* ── Center: File + Progress ── */}
        <div className="panel-center">
          {!filePath ? (
            <div className="dropzone" onClick={pickFile}>
              <div className="dropzone-icon">🎬</div>
              <div className="dropzone-title">Select your gaming clip</div>
              <div className="dropzone-sub">
                No upload required — ClipMind Pro processes files directly<br />
                on your machine using your GPU hardware
              </div>
              <button className="dropzone-btn" onClick={e => { e.stopPropagation(); pickFile(); }}>
                Browse Files
              </button>
              <div className="dropzone-note">MP4 · MOV · MKV · AVI · WebM · M4V</div>
            </div>
          ) : (
            <>
              <div className="file-bar">
                <div className="file-icon">🎮</div>
                <div className="file-info">
                  <div className="file-name">{baseName(filePath)}</div>
                  <div className="file-path">{filePath}</div>
                </div>
                <button
                  className="file-remove"
                  onClick={() => { setFilePath(null); setDlPath(null); setProgress(0); setStep(0); }}
                  disabled={processing}
                >
                  ✕ Clear
                </button>
              </div>

              {(processing || progress > 0) ? (
                <div className="progress-card">
                  <div className="progress-header">
                    <span className="progress-title">Processing</span>
                    <span className="progress-timer">{fmt(elapsed)}</span>
                  </div>
                  <div className="ring-wrap">
                    <ProgressRing pct={progress} />
                  </div>
                  <div className="prog-status">{status}</div>
                  <div className="steps-list">
                    {PIPELINE_STEPS.map((s, i) => (
                      <div
                        key={i}
                        className={`step-row ${i < step ? "done" : i === step ? "active" : ""}`}
                      >
                        <div className={`step-dot ${i < step ? "done" : i === step ? "active" : ""}`} />
                        {s}
                      </div>
                    ))}
                  </div>
                </div>
              ) : (
                <div className="center-msg">
                  {dlPath ? (
                    <>
                      <div className="big">✅</div>
                      <div style={{ fontWeight: 600, color: "var(--text)" }}>Clip ready!</div>
                      <div>Open the folder from the right panel</div>
                    </>
                  ) : (
                    <>
                      <div className="big">⚙️</div>
                      <div>Configure settings and hit Process</div>
                    </>
                  )}
                </div>
              )}

              <div className="action-bar">
                <button
                  className={`process-btn ${processing ? "running" : ""}`}
                  onClick={startProcess}
                  disabled={!canProcess}
                >
                  {processing ? "⚙️  Processing…" : "🚀  Process Clip"}
                </button>
              </div>
            </>
          )}
        </div>

        {/* ── Right: Export settings + results ── */}
        <ExportPanel
          hw={hw}
          resolution={resolution}
          setResolution={setRes}
          fps={fps}
          setFps={setFps}
          quality={quality}
          setQuality={setQuality}
          dlPath={dlPath}
          fileMb={fileMb}
          onRevealInFinder={() => dlPath && invoke("open_output_folder", { path: dlPath })}
        />
      </div>
    </div>
  );
}
