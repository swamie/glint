// Browser-only preview shim. Inside Glint, window.__TAURI__ exists and this file does nothing.
if (!window.__TAURI__) {
  const now = Date.now();
  const clip = (i, title, game, mins, dur, extra = {}) => ({
    id: "c" + i, path: "", title, game, created: now - mins * 60000, duration: dur, size: dur * 2.4e6,
    width: 2160, height: 1440, fps: 60, favorite: false, kind: "clip", thumb: "", audio_tracks: 3, folder: "", ...extra,
  });
  const folders = ["Best of"];
  const games = [{ name: "Overwatch", exe: "", icon: "" }];
  const listeners = {};
  window.__mockCalls = [];
  const state = {
    settings: {
      clips_dir: "C:\\Users\\you\\Videos\\Glint", game_folders: false, monitor: 0, height: 0, fps: 60, quality: "high", bitrate: 0, encoder: "auto", method: "auto", buffer: "mixed", buffer_ram_secs: 30, buffer_dir: "",
      cursor: true, desktop_audio: true, mic: true, mic_denoise: false, game_audio: false, record_on_start: true, sound: true, sound_name: "chirp", sound_volume: 40, sound_file: "",
      toast: true, toast_pos: "tr", toast_opacity: 96, toast_scale: 100, toast_secs: 3.5, toast_length: true, toast_game: true, toast_mic: true,
      ui_zoom: 100, blur_parallel: 2, language: "en", aspect: "auto", theme: "glint", custom_css: "", icon_theme: false, icon_accent: "", icon_accent_2: "", discord_rpc: false, discord_app_id: "", color_presets: [{ name: "Mine", brightness: 1.05, contrast: 1.1, saturation: 1.2, gamma: 1 }], editor_keys: {}, toast_compact: false, show_empty_main: false, renders_button: "auto", autostart: false, close_to_tray: true, update_check: true, update_auto: true,
      compress_mb: 10, ui_sounds: false, ui_sounds_hover: false, ui_sounds_volume: 50,
      compress: { sizes: [10, 20, 50, 500], preset: "veryfast", audio_kbps: 0, max_height: 0, max_fps: 0, audio: "first", dest: "beside", dest_dir: "", copy: true }, theme_colors: {},
      ffmpeg_path: "", blur_path: "",
      keybinds: [
        { id: "a", ctrl: false, shift: false, alt: false, win: false, key: "F8", action: "clip", seconds: 30, monitor: -1 },
        { id: "b", ctrl: false, shift: true, alt: false, win: false, key: "F8", action: "clip", seconds: 600, monitor: -1 },
        { id: "c", ctrl: true, shift: false, alt: true, win: false, key: "KeyR", action: "toggle", seconds: 30, monitor: -1 },
      ],
      blur: {
        blur: true, amount: 1, output_fps: 60, weighting: "equal", gamma: 1, interpolate: true, interpolated_fps: "5x", interpolation_method: "svp",
        pre_interpolate: false, pre_interpolated_fps: "360", deduplicate: true, deduplicate_method: "svp", encode_preset: "h264", quality: 16, copy_dates: false,
        gpu_decoding: true, gpu_interpolation: true, gpu_encoding: false, gpu_type: "nvidia", rife_gpu: -1,
        timescale: false, input_timescale: 1, output_timescale: 1, adjust_audio_pitch: false, filters: false, brightness: 1, saturation: 1, contrast: 1,
        advanced: false, deduplicate_range: 2, deduplicate_threshold: 0.001, container: "mp4", ffmpeg_filters: "", debug: false,
        gaussian_std_dev: 1, gaussian_mean: 2, gaussian_bound: "[0,2]", svp_preset: "weak", svp_algorithm: 13, block_size: 8, mask_area: 0, rife_model: "rife-v4.26_ensembleFalse",
      },
    },
    monitors: [
      { index: 0, name: "DISPLAY1", width: 2160, height: 1440, hz: 280, primary: true, x: 0, y: 0, native_width: 2560, native_height: 1440 },
      { index: 1, name: "DISPLAY2", width: 1680, height: 1050, hz: 60, primary: false, x: 2160, y: 0, native_width: 1680, native_height: 1050 },
    ],
    status: { recording: true, encoder: "nvenc", method: "wgc", width: 2560, height: 1440, fps: 60, fps_actual: 59.9, buffer_secs: 412, buffer_max: 600, buffer_bytes: 3.1e7, buffer_disk_bytes: 2.8e8, ram_total: 2.4e8, cpu_pct: 0.9, ram_app: 3.4e7, ram_ffmpeg: 1.4e8, max_mbps: 15, error: "", game: "Overwatch", discord: "off", border: "off" },
    // capture2.rs AutoInfo: what each "Auto" resolves to (get_state.auto, auto_info).
    auto: { encoder: "nvenc", method: "wgc", method_why: "", aspect_width: 2560, aspect_height: 1440, mode_width: 2160, mode_height: 1440, widened: true, auto_mbps: 17.7, native_width: 2160, native_height: 1440, ffmpeg: "C:\\tools\\ffmpeg\\bin\\ffmpeg.exe", blur: "C:\\tools\\blur\\blur-cli.exe", border: { build: 26300, configurable: true, main: "Allow", desktop_apps: "Allow", device: "Allow", can_remove: true } },
    // What's new opens by itself after an update: localStorage["glint.seenVersion"] = "0.1.7", then reload.
    ffmpeg: "C:\\tools\\ffmpeg\\bin\\ffmpeg.exe", blur: "C:\\tools\\blur\\blur-cli.exe", version: "0.1.8",
    hardware: { gpus: [{ name: "NVIDIA GeForce RTX 5060 Ti", vendor: "nvidia" }], cpu: "11th Gen Intel(R) Core(TM) i5-11600", gpu: { name: "NVIDIA GeForce RTX 5060 Ti", vendor: "nvidia" }, encoder: "nvenc" },
  };
  // status.captures: one entry per monitor buffered, the capture monitor first (primary), then every
  // other monitor a clip keybind names, as the backend does. window.__mockCapError = "text": the
  // other monitors' captures fail with it (the status bar's warning mark).
  const syncCaptures = () => {
    const st = state.status, set = state.settings;
    const cap = state.monitors.find((m) => m.index === set.monitor) || state.monitors[0];
    const named = set.keybinds.filter((k) => k.action === "clip" && k.monitor >= 0 && k.monitor !== cap.index).map((k) => k.monitor);
    const others = state.monitors.filter((m) => named.includes(m.index));
    st.captures = [cap, ...others].map((m) => {
      const primary = m === cap;
      const fps = Math.min(set.fps, m.hz);
      return { monitor: m.index, name: m.name, width: primary ? st.width : m.width, height: primary ? st.height : m.height, fps: primary ? st.fps : fps, fps_actual: primary ? st.fps_actual : fps - 0.1, buffer_secs: st.buffer_secs, buffer_max: st.buffer_max, error: primary ? st.error : window.__mockCapError || "", primary };
    });
    return st;
  };
  let clips = [
    clip(1, "osu! 2026-11-12 21-14-03", "osu!", 12, 31, { favorite: true, folder: "Best of" }),
    // "/__test.mp4" is a real 20 s video the preview server (serve.js) hands out, so the player can be exercised.
    clip(2, "Overwatch 2026-11-12 20-02-44", "Overwatch", 84, 20, { path: "/__test.mp4" }),
    clip(3, "osu! 2026-11-12 19-40-10 (blur)", "osu!", 106, 31, { kind: "blur" }),
    clip(4, "Deadlock 2026-11-11 23-51-09", "Deadlock", 1300, 30),
    clip(5, "Risk of Rain 2 2026-11-10 18-22-31", "Risk of Rain 2", 2900, 121),
    clip(6, "Desktop 2026-11-09 14-05-56", "Desktop", 4400, 30),
    clip(7, "osu! 2026-11-08 22-10-40 (trim)", "osu!", 5800, 14, { kind: "trim" }),
    clip(8, "Minecraft 2026-11-07 20-11-02", "Minecraft", 7200, 60),
    clip(9, "Valorant 2026-11-06 21-40-55", "Valorant", 8700, 30),
    clip(10, "Valorant 2026-11-06 21-40-55 (discord)", "Valorant", 8690, 30, { kind: "discord", size: 9.4e6 }),
  ];
  const MOCK_VIDEO_SECS = 20; // the length of "/__test.mp4" (clip c2's video), the only real one
  let drafts = [];            // "Edit together" drafts: never in list_clips, removed by discard_draft
  // Editor projects, as projects.rs: { id, name, created, modified, clips, exported, state }; a
  // project's draft is in `drafts` with the project's id. An add is a "join" job (one at a time,
  // about 2 s); its parts lie inside the real 20 s video, one MOCK_SLOT (1 s) each, so 20 clips fit
  // and an add only appends (old joins never move, as in projects.rs).
  // window.__mockAddFails = true: the next add jobs fail and leave the project as it was.
  const MOCK_SLOT = MOCK_VIDEO_SECS / 20;
  const mockProj = {
    list: [],
    seq: 0,
    changed: () => window.__mockEmit("projects-changed", null),
    active: (j) => ["queued", "running"].includes(j.state),
    busy: (id) => jobs.some((j) => j.kind === "join" && j.project === id && mockProj.active(j)),
    view: (p) => {
      const clip = drafts.find((d) => d.id === p.id) || null;
      return { id: p.id, name: p.name, created: p.created, modified: p.modified, clip: clip && { ...clip, joins: [...clip.joins], join_names: [...clip.join_names] }, clips: p.clips.map((c) => ({ ...c })), exported: p.exported, size: clip ? clip.size : 0, busy: mockProj.busy(p.id) };
    },
    make: (name) => {
      const now = Date.now();
      const p = { id: "p" + now + "-" + mockProj.seq++, name: String(name || "").trim().slice(0, 100) || "New project", created: now, modified: now, clips: [], exported: false, state: null };
      mockProj.list.push(p);
      return p;
    },
    remove: (id) => { mockProj.list = mockProj.list.filter((p) => p.id !== id); drafts = drafts.filter((d) => d.id !== id); },
    add: (p, ids, fromJoin) => {
      const uniq = [...new Set(ids)];
      if (!uniq.length) throw "Pick at least one clip to add";
      const hasDraft = drafts.some((d) => d.id === p.id);
      const pending = jobs.filter((j) => j.kind === "join" && j.project === p.id && mockProj.active(j));
      const have = (hasDraft ? p.clips.length : 0) + pending.reduce((s, j) => s + j.picked.length, 0);
      if (have + uniq.length > 20) throw "A project takes at most 20 clips";
      const picked = uniq.map((id) => clips.find((c) => c.id === id));
      if (picked.some((c) => !c)) throw "Clip not found";
      const secs = (hasDraft ? p.clips.reduce((s, c) => s + c.duration, 0) : 0) + pending.reduce((s, j) => s + j.picked.reduce((a, c) => a + c.duration, 0), 0);
      if (secs + picked.reduce((s, c) => s + c.duration, 0) > 1800) throw "A project takes at most 30 minutes of clips";
      const key = "join:" + (hasDraft ? [p.id, ...uniq] : uniq).join(",");
      if (jobs.some((j) => j.clip_id === key && mockProj.active(j))) throw "Those clips are already being joined";
      const job = { id: "j" + Date.now() + jobs.length, clip_id: key, title: p.name, kind: "join", state: "queued", pct: 0, message: "", result_id: "", queued: Date.now(), project: p.id, picked, fromJoin };
      jobs.push(job);
      const answer = { ...job };
      delete answer.project; delete answer.picked; delete answer.fromJoin;
      mockProj.pump();
      mockProj.changed();
      return answer;
    },
    pump: () => {
      if (jobs.some((j) => j.kind === "join" && j.state === "running")) return;
      const job = jobs.find((j) => j.kind === "join" && j.state === "queued");
      if (!job) { window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x }))); return; }
      job.state = "running";
      const tick = setInterval(() => {
        if (job.state !== "running") { clearInterval(tick); mockProj.pump(); mockProj.changed(); return; }
        job.pct = Math.min(100, job.pct + 10);
        if (job.pct >= 100) {
          clearInterval(tick);
          mockProj.finish(job);
          mockProj.pump();
        }
        window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x })));
      }, 200);
      window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x })));
    },
    finish: (job) => {
      const p = mockProj.list.find((x) => x.id === job.project);
      if (!p || window.__mockAddFails) {
        job.state = "error";
        job.message = p ? `The video file of "${job.picked[0].title}" is missing` : "Project not found";
        if (p && job.fromJoin && !drafts.some((d) => d.id === p.id)) mockProj.remove(p.id);
        mockProj.changed();
        return;
      }
      const old = drafts.find((d) => d.id === p.id);
      const round = (x) => Math.round(x * 1000) / 1000;
      const titles = job.picked.map((c) => c.title);
      // Every part gets its own slot of MOCK_SLOT seconds of the real video and the draft is as long
      // as its parts, so an add only appends: old joins, names and source seconds never move.
      let joins, names, starts;
      const before = old ? old.join_names.length : 0;
      starts = titles.map((_, i) => round((before + i) * MOCK_SLOT));
      if (!old) {
        joins = starts.slice(1);
        names = titles;
      } else {
        joins = [...old.joins, ...starts];
        names = [...old.join_names, ...titles];
      }
      const length = round(names.length * MOCK_SLOT);
      const gen = ++mockProj.seq;
      const first = old || job.picked[0];
      const made = {
        ...first, id: p.id, title: p.name, kind: "joined", draft: true, created: old ? old.created : Date.now(),
        duration: length, size: (old ? old.size : 0) + job.picked.reduce((s, c) => s + c.size, 0), joins, join_names: names,
        path: `/__test.mp4?draft=${p.id}-${gen}`, favorite: false, folder: first.folder || "",
      };
      drafts = drafts.filter((d) => d.id !== p.id);
      drafts.push(made);
      const added = job.picked.map((c) => ({ id: c.id, title: c.title, duration: c.duration }));
      if (!old) p.clips = [];
      p.clips.push(...added);
      p.modified = Math.max(Date.now(), p.modified + 1);
      p.exported = false;
      job.state = "done";
      job.pct = 100;
      job.result_id = p.id;
      window.__mockEmit("project-changed", { project: mockProj.view(p), job_id: job.id, added, mode: "copy", secs: 2, starts, titles });
      mockProj.changed();
    },
  };
  window.__mockProjects = mockProj; // for checks in the console
  // A pretend render queue: each job takes about 4 s. Blur jobs run at most settings.blur_parallel
  // at once; compress jobs ("Compress for Discord") one at a time in their own lane, as in jobs.rs.
  let jobs = [];
  const pumpJobs = () => {
    const limit = state.settings.blur_parallel || 2;
    for (const j of jobs) {
      if (j.state !== "queued" || j.kind === "join") continue; // joins: mockProj.pump
      const squeeze = j.kind === "compress";
      if (jobs.filter((x) => x.state === "running" && (x.kind === "compress") === squeeze).length >= (squeeze ? 1 : limit)) continue;
      j.state = "running";
      const tick = setInterval(() => {
        if (j.state !== "running") { clearInterval(tick); return; }
        j.pct = Math.min(100, j.pct + 5);
        if (j.pct >= 100) {
          clearInterval(tick);
          const src = clips.find((x) => x.id === j.clip_id);
          if (squeeze) {
            // The copy aims a little under the size asked for; window.__mockCopyFails = true: the clipboard refuses.
            const size = Math.round(j.mb * 0.94e6);
            // A file from outside the library (compress_file, j.file): saved beside it, not added to the library.
            const outPath = (j.file || (src && src.path) || "C:\\Users\\you\\Videos\\Glint\\clip.mp4").replace(/(\.[^.\\]+)?$/, ` (Discord ${j.mb}MB).mp4`);
            const made = j.file ? null : { ...(src || clips[0]), id: "d" + Date.now(), title: j.title + ` (Discord ${j.mb}MB)`, kind: "discord", size, created: Date.now(), path: outPath };
            if (made) clips.push(made);
            j.state = "done";
            j.result_id = made ? made.id : "";
            if (made) window.__mockEmit("clips-changed", made.id);
            window.__mockEmit("clip-compressed", { clip: made, mb: j.mb, size, copied: !window.__mockCopyFails && state.settings.compress?.copy !== false, secs: 4.1, path: outPath, job_id: j.id, source: j.clip_id });
          } else {
            const made = { ...(src || clips[0]), id: "b" + Date.now(), title: j.title + " (blur)", kind: "blur", created: Date.now() };
            clips.push(made);
            j.state = "done";
            j.result_id = made.id;
            window.__mockEmit("blur-progress", { id: j.clip_id, pct: 100, state: "done", message: made.id });
            window.__mockEmit("clips-changed", made.id);
          }
          pumpJobs();
        } else if (!squeeze) {
          window.__mockEmit("blur-progress", { id: j.clip_id, pct: j.pct, state: "running", message: "" });
        }
        window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x })));
      }, 200);
    }
    window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x })));
  };
  // Settings → Components: what deps.rs reports, and a pretend 3 s download for "install_dep".
  const NOT_FOUND = "ffmpeg.exe was not found. Install ffmpeg or set its path in Settings.";
  const mockDeps = {
    have: { ffmpeg: true, blur: false },
    busy: "",
    cancel: false,
    started: false,
    sync() {
      if (!this.started) {
        this.started = true;
        let off = window.__mockNoFfmpeg;
        try { off = off || localStorage.getItem("glint.mockNoFfmpeg") === "1"; } catch {}
        if (off) this.have.ffmpeg = false;
      }
      state.ffmpeg = this.have.ffmpeg ? "C:\\Users\\you\\AppData\\Local\\Programs\\Glint\\ffmpeg.exe" : null;
      if (!this.have.ffmpeg) state.status.error = NOT_FOUND;
      else if (state.status.error === NOT_FOUND) state.status.error = "";
      state.blur = this.have.blur ? "C:\\Users\\you\\AppData\\Local\\Glint\\blur\\blur-cli.exe" : null;
    },
    items() {
      const f = this.have.ffmpeg, dir = "C:\\Users\\you\\AppData\\Local\\Programs\\Glint\\";
      return [
        { id: "ffmpeg", name: "ffmpeg", required: true, installable: true, state: f ? "ok" : "missing", path: f ? dir + "ffmpeg.exe" : "", version: f ? "8.0.1-essentials_build-www.gyan.dev" : "", note: "Records the replay buffer and saves clips. Required." },
        { id: "ffprobe", name: "ffprobe", required: true, installable: true, state: f ? "ok" : "missing", path: f ? dir + "ffprobe.exe" : "", version: f ? "8.0.1-essentials_build-www.gyan.dev" : "", note: "Reads clip lengths and sizes for the library. Installed together with ffmpeg." },
        { id: "blur", name: "Blur", required: false, installable: true, state: this.have.blur ? "ok" : "missing", path: this.have.blur ? state.blur : "", version: "", note: "Optional: motion-blur renders of finished clips. Windows may ask for permission while it installs." },
        { id: "curl", name: "curl.exe", required: false, installable: false, state: "ok", path: "C:\\Windows\\System32\\curl.exe", version: "8.13.0", note: "Part of Windows 10 version 1803 and newer. Only needed to install the parts above." },
        { id: "tar", name: "tar.exe", required: false, installable: false, state: "ok", path: "C:\\Windows\\System32\\tar.exe", version: "3.7.7", note: "Part of Windows 10 version 1803 and newer. Only needed to install the parts above." },
      ];
    },
    async install(id) {
      if (this.busy) throw "Another install is already running.";
      if (!(id in this.have)) throw "Nothing to install for that.";
      this.busy = id;
      this.cancel = false;
      const total = id === "ffmpeg" ? 101e6 : 412e6;
      try {
        for (let i = 1; i <= 15; i++) {
          await new Promise((r) => setTimeout(r, 200));
          if (this.cancel) throw "Cancelled.";
          if (window.__mockOffline && i === 3) throw "The download failed. Check your internet connection and try again.";
          window.__mockEmit("deps-progress", { id, phase: "download", done: Math.round((total * i) / 15), total, error: "" });
        }
        window.__mockEmit("deps-progress", { id, phase: "unpack", done: total, total, error: "" });
        await new Promise((r) => setTimeout(r, 400));
        window.__mockEmit("deps-progress", { id, phase: "install", done: total, total, error: "" });
        await new Promise((r) => setTimeout(r, 300));
        this.have[id] = true;
        this.sync();
        window.__mockEmit("deps-progress", { id, phase: "done", done: 0, total: 0, error: "" });
        return this.items();
      } catch (e) {
        window.__mockEmit("deps-progress", { id, phase: "error", done: 0, total: 0, error: String(e) });
        throw e;
      } finally {
        this.busy = "";
      }
    },
  };
  window.__TAURI__ = {
    core: {
      convertFileSrc: (p) => p,
      invoke: async (cmd, args) => {
        window.__mockCalls.push([cmd, args]);
        switch (cmd) {
          case "list_games": return games;
          case "pick_game_icon": return null;
          case "find_game_icon": return null;
          case "bug_report_info": return { configured: true, version: "0.1.6", system: "Glint 0.1.6\nWindows 11 (build 26300)\nNVIDIA GeForce RTX 5060 Ti\nCapture 2560x1440 @ 120 fps, nvenc, wgc, buffer mixed", log: "2026-11-13 09:13:54 hotkey kb-30s -> clip 30s\n2026-11-13 09:13:56 clip 30s: saved 30.4s, 184.2 MB in 1743 ms -> ~\\Videos\\Glint\\Risk of Rain 2\\Risk of Rain 2 2026-11-13 09-13-54.mp4" };
          case "send_bug_report": await new Promise((r) => setTimeout(r, 600)); if (/fail/i.test(args.text)) throw "Discord did not accept the report (HTTP 400)"; return null;
          case "editor_audio_tracks": return [{ index: 0, title: "Mix", path: "/__test.mp4" }, { index: 1, title: "Desktop", path: "/__test.mp4" }, { index: 2, title: "Microphone", path: "/__test.mp4" }];
          case "editor_waveforms": return [0, 1, 2].map((index) => ({ index, per_second: 50, peaks: Array.from({ length: 1000 }, (_, i) => Math.abs(Math.sin(i / (7 + index * 3))) * (0.25 + 0.75 * Math.abs(Math.sin(i / 90 + index)))) }));
          case "editor_thumbs": return Array.from({ length: args.count }, (_, i) => `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="128" height="72"><rect width="128" height="72" fill="hsl(${(i * 37) % 360} 45% 30%)"/><text x="64" y="44" font-size="24" fill="white" text-anchor="middle">${i + 1}</text></svg>`)}`);
          case "editor_audio_info":
            if (/noaudio/.test(args.path)) throw "That file has no audio in it";
            return { path: "/__test.mp4", title: args.path.split(/[\\/]/).pop().replace(/\.[^.]+$/, ""), duration: 8, per_second: 50, peaks: Array.from({ length: 400 }, (_, i) => Math.abs(Math.sin(i / 5)) * 0.8) };
          case "pick_audio_file": return "C:\\Music\\victory.mp3";
          case "export_clip": {
            const src = clips.find((x) => x.id === args.id) || drafts.find((x) => x.id === args.id);
            if (!src) throw "Clip not found";
            for (const pct of [20, 60, 100]) { await new Promise((r) => setTimeout(r, 150)); window.__mockEmit("export-progress", { id: args.id, pct }); }
            const made = { ...src, id: "e" + Date.now(), title: src.title + " (edit)", kind: "edit", created: Date.now(), draft: false, joins: [], join_names: [] };
            clips.push(made);
            const proj = src.draft && mockProj.list.find((p) => p.id === src.id);
            if (proj) { proj.exported = true; mockProj.changed(); } // the project shows "Exported"
            return made;
          }
          case "import_sources": return [
            { id: "shadowplay", name: "NVIDIA ShadowPlay", path: "C:\\Users\\you\\Videos\\NVIDIA", found: true },
            { id: "medal", name: "Medal", path: "C:\\Users\\you\\Videos\\Medal", found: true },
            { id: "outplayed", name: "Outplayed", path: "C:\\Users\\you\\Videos\\Overwolf\\Outplayed", found: false },
            { id: "steelseries", name: "SteelSeries GG Moments", path: "C:\\Users\\you\\Videos\\SteelSeries Moments", found: true },
          ];
          case "pick_import_folder": return "D:\\Old clips";
          case "import_scan": {
            await new Promise((r) => setTimeout(r, 600));
            if (/empty|SteelSeries/.test(args.path)) return [];
            if (/bad/.test(args.path)) throw "That folder could not be read";
            const gamesFound = ["Overwatch", "Risk of Rain 2", "osu!", "Desktop", "Imported"];
            return Array.from({ length: 14 }, (_, i) => ({ path: `${args.path}\\${gamesFound[i % 5]}\\clip ${i}.mp4`, title: `${gamesFound[i % 5]} clip ${i}`, game: gamesFound[i % 5], size: 4e7 + i * 3e6, modified: now - i * 86400000, known: i === 3 || i === 8 }));
          }
          case "import_run": {
            const n = args.items.length;
            for (let i = 1; i <= n; i++) { await new Promise((r) => setTimeout(r, 100)); window.__mockEmit("import-progress", { done: i, total: n, title: args.items[i - 1].title }); }
            window.__mockEmit("import-progress", { done: n, total: n, title: "", skipped: 1 });
            // place: "games" | "one" (args.folder) | "none"; absent = the game folders setting (0.1.16).
            const place = args.place || (state.settings.game_folders ? "games" : "none");
            const folderFor = (g) => { const f = place === "one" ? String(args.folder || "").trim() : place === "games" ? g : ""; const have = folders.find((x) => x.toLowerCase() === f.toLowerCase()); if (f && !have) folders.push(f); return have || f; };
            args.items.slice(0, n - 1).forEach((it, i) => clips.push(clip(100 + clips.length + i, it.title, it.game, 60 * 24 * (i + 1), 30, { kind: "import", folder: folderFor(it.game) })));
            window.__mockEmit("clips-changed", "");
            return n - 1;
          }
          // Updates: the first manual check finds nothing new, the second finds 0.2.0 (window.__mockUpdate = true skips ahead).
          case "update_state": return { current: "0.1.8", latest: "0.1.8", available: false, error: "", checked: 0 };
          case "update_check": {
            await new Promise((r) => setTimeout(r, 700));
            window.__mockUpdateChecks = (window.__mockUpdateChecks || 0) + 1;
            const found = window.__mockUpdate || window.__mockUpdateChecks > 1;
            return { current: "0.1.8", latest: found ? "0.2.0" : "0.1.8", available: !!found, error: "", checked: Date.now() / 1000 };
          }
          case "update_install": {
            for (const pct of [10, 35, 60, 85, 100]) { await new Promise((r) => setTimeout(r, 300)); window.__mockEmit("update-progress", { pct }); }
            return null;
          }
          case "list_jobs": return jobs.map((j) => ({ ...j }));
          case "cancel_job": {
            const j = jobs.find((x) => x.id === args.id);
            if (!j || !["queued", "running"].includes(j.state)) return false;
            j.state = "cancelled";
            window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x })));
            if (j.kind !== "compress") window.__mockEmit("blur-progress", { id: j.clip_id, pct: 0, state: "cancelled", message: "" });
            pumpJobs();
            return true;
          }
          case "clear_jobs": jobs = jobs.filter((j) => j.state === "queued" || j.state === "running"); window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x }))); return null;
          case "compress_clip": {
            // Refused like the backend: a clip already being compressed, or one already small enough.
            const src = clips.find((x) => x.id === args.id);
            if (!src) throw "Clip not found";
            if (jobs.some((j) => j.clip_id === args.id && j.kind === "compress" && ["queued", "running"].includes(j.state))) throw "That clip is already being compressed";
            if (src.size <= args.mb * 1e6) throw `This clip is already under ${args.mb} MB, so you can send it to Discord as it is`;
            const job = { id: "j" + Date.now() + jobs.length, clip_id: args.id, title: src.title, kind: "compress", state: "queued", pct: 0, message: "", result_id: "", queued: Date.now(), mb: args.mb };
            jobs.push(job);
            const answer = { ...job }; // as queued, like the backend's answer
            pumpJobs();
            return answer;
          }
          // The Compress page (compress.rs). A picked file is 80 MB, 1920x1080 at 60 fps, 45 s, two audio tracks.
          case "compress_info": return { min_mb: 5, max_mb: 1000, default_sizes: [10, 20, 50, 500], presets: ["ultrafast", "superfast", "veryfast", "faster", "fast", "medium", "slow"], default_preset: "veryfast", auto: { audio_kbps: 96, audio_low_kbps: 64, audio_low_below: 600, max_height: 1440, max_fps: 60 } };
          case "pick_compress_files": return ["D:\\Recordings\\match point.mkv"];
          case "compress_copy": case "compress_reveal": return null;
          case "compress_file": {
            if (!(args.mb >= 5 && args.mb <= 1000)) throw "Pick a size from 5 to 1000 MB";
            if (jobs.some((j) => j.clip_id === args.path && ["queued", "running"].includes(j.state))) throw "That clip is already being compressed";
            if (80e6 <= args.mb * 1e6) throw `This clip is already under ${args.mb} MB (it is 80.0 MB)`;
            const title = args.path.split(/[\\/]/).pop().replace(/\.[^.]+$/, "");
            const job = { id: "j" + Date.now() + jobs.length, clip_id: args.path, title, kind: "compress", state: "queued", pct: 0, message: "", result_id: "", queued: Date.now(), mb: args.mb, file: args.path };
            jobs.push(job);
            const answer = { ...job };
            pumpJobs();
            return answer;
          }
          case "compress_plan": {
            // A rough copy of compress.rs's budget and size rule, enough to show what each clip becomes.
            const c = state.settings.compress || {};
            const mb = Math.min(1000, Math.max(5, args.mb));
            const plan = (key, id, path, title, size, dur, w, h, fps, tracks) => {
              const row = { key, id, path, title, size, duration: dur, src_w: w, src_h: h, src_fps: fps, audio_tracks: tracks, width: 0, height: 0, fps: 0, video_kbps: 0, audio_kbps: 0, audio_mix: 0, small: size <= mb * 1e6, error: "" };
              const total = ((mb * 1e6 * 0.96 - 30000) * 0.99 * 8) / dur / 1000;
              const audio = tracks ? (c.audio_kbps || (total < 600 ? 64 : 96)) : 0;
              const video = Math.min(40000, total - audio);
              if (video < 150) { row.error = `This clip is too long for ${mb} MB. Trim it or pick a bigger size.`; return row; }
              const ladder = [[12000, 1440, 60], [6000, 1080, 60], [3500, 720, 60], [2000, 720, 30], [1000, 480, 30], [0, 360, 30]];
              for (const [need, lh, cap] of ladder) {
                const hh = Math.min(lh, c.max_height || 1e9), f = Math.min(fps || 60, cap, c.max_fps || 1e9);
                if (video >= need * (hh / lh) ** 2 * f / cap) { const k = Math.min(1, Math.sqrt((hh * hh * 16 / 9) / (w * h))); Object.assign(row, { width: Math.round(w * k / 2) * 2, height: Math.round(h * k / 2) * 2, fps: f }); break; }
              }
              Object.assign(row, { video_kbps: video, audio_kbps: audio, audio_mix: tracks ? (c.audio === "all" ? tracks : 1) : 0 });
              return row;
            };
            const rows = (args.ids || []).map((id) => { const k = clips.find((x) => x.id === id); return k ? plan(id, id, k.path, k.title, k.size, k.duration, k.width, k.height, k.fps, k.audio_tracks) : { key: id, id, error: "Clip not found" }; });
            for (const p of args.paths || []) rows.push(plan(p, "", p, p.split(/[\\/]/).pop().replace(/\.[^.]+$/, ""), 80e6, 45, 1920, 1080, 60, 2));
            await new Promise((r) => setTimeout(r, 150));
            return rows;
          }
          // Editor projects (projects.rs): kept in memory. "Edit together" makes one too.
          case "join_clips": {
            // Refused like join.rs; window.__mockJoinMissing = true: a clip's file is gone.
            const picked = [...new Set(args.ids)].map((id) => clips.find((c) => c.id === id)).filter(Boolean);
            if (picked.length < 2) throw "Pick at least two clips to edit together";
            if (picked.length > 20) throw "Edit together takes at most 20 clips";
            if (picked.reduce((s, c) => s + c.duration, 0) > 1800) throw "Edit together takes at most 30 minutes of clips";
            if (window.__mockJoinMissing) throw `The video file of "${picked[picked.length - 1].title}" is missing`;
            if (jobs.some((j) => j.clip_id === "join:" + picked.map((c) => c.id).join(",") && ["queued", "running"].includes(j.state))) throw "Those clips are already being joined";
            const p = mockProj.make(`${picked[0].title} + ${picked.length - 1} more`);
            try { return mockProj.add(p, args.ids, true); } catch (e) { mockProj.remove(p.id); throw e; }
          }
          case "discard_draft": case "delete_project": {
            // As projects.rs: false while a job builds or reads it; removes draft, thumbnail, state.
            const p = mockProj.list.find((x) => x.id === args.id);
            if (!p) {
              if (cmd === "delete_project" || !drafts.some((d) => d.id === args.id)) return false;
              if (jobs.some((j) => j.clip_id === args.id && ["queued", "running"].includes(j.state))) return false;
              drafts = drafts.filter((d) => d.id !== args.id);
              return true;
            }
            if (mockProj.busy(p.id)) return false;
            mockProj.remove(p.id);
            mockProj.changed();
            return true;
          }
          case "list_projects":
            // "Edit together" cancelled before its first build: no empty project is left (as projects.rs).
            jobs.filter((j) => j.kind === "join" && j.fromJoin && j.state === "cancelled" && !drafts.some((d) => d.id === j.project)).forEach((j) => mockProj.remove(j.project));
            return mockProj.list.map(mockProj.view).sort((a, b) => b.modified - a.modified);
          case "create_project": { const p = mockProj.make(args.name); mockProj.changed(); return mockProj.view(p); }
          case "project_add_clips": {
            const p = mockProj.list.find((x) => x.id === args.id);
            if (!p) throw "Project not found";
            return mockProj.add(p, args.ids || [], false);
          }
          case "project_save_state": {
            const p = mockProj.list.find((x) => x.id === args.id);
            if (!p) throw "Project not found";
            p.state = String(args.state);
            p.modified = Math.max(Date.now(), p.modified);
            p.exported = false;
            mockProj.changed();
            return mockProj.view(p);
          }
          case "project_state": { const p = mockProj.list.find((x) => x.id === args.id); return p && p.state != null ? p.state : null; }
          case "rename_project": {
            const p = mockProj.list.find((x) => x.id === args.id);
            if (!p) throw "Project not found";
            const name = String(args.name || "").trim().slice(0, 100);
            if (!name) throw "Type a name for the project";
            p.name = name;
            const d = drafts.find((x) => x.id === p.id);
            if (d) d.title = name;
            mockProj.changed();
            return mockProj.view(p);
          }
          case "blur_clip": {
            if (jobs.some((j) => j.clip_id === args.id && j.kind !== "compress" && ["queued", "running"].includes(j.state))) throw "That clip is already rendering";
            const src = clips.find((x) => x.id === args.id);
            jobs.push({ id: "j" + Date.now() + jobs.length, clip_id: args.id, title: src?.title || "Clip", kind: "blur", state: "queued", pct: 0, message: "", result_id: "", queued: Date.now() });
            window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x })));
            window.__mockEmit("blur-progress", { id: args.id, pct: 0, state: "queued", message: "" });
            pumpJobs();
            return null;
          }
          case "list_sounds": return [{ id: "chirp", name: "Chirp" }, { id: "ding", name: "Ding" }, { id: "pop", name: "Pop" }];
          case "set_game": clips.forEach((c) => { if (args.ids.includes(c.id)) c.game = args.game; }); return null;
          case "sort_by_game": { let n = 0; clips.forEach((c) => { if (!c.folder) { c.folder = c.game; if (!folders.includes(c.game)) folders.push(c.game); n++; } }); return n; }
          case "list_folders": return [...folders];
          case "create_folder": if (folders.includes(args.name)) throw `A folder named "${args.name}" already exists`; folders.push(args.name); return args.name;
          case "rename_folder": folders[folders.indexOf(args.from)] = args.to; clips.forEach((c) => { if (c.folder === args.from) c.folder = args.to; }); return args.to;
          case "delete_folder": folders.splice(folders.indexOf(args.name), 1); clips.forEach((c) => { if (c.folder === args.name) c.folder = ""; }); return null;
          case "move_clips": { let n = 0; clips.forEach((c) => { if (args.ids.includes(c.id) && c.folder !== args.folder) { c.folder = args.folder; n++; } }); return n; }
          case "delete_clips": { const n = clips.length; clips = clips.filter((c) => !args.ids.includes(c.id)); return n - clips.length; }
          case "copy_clips": return args.ids.length;
          case "set_favorites": clips.forEach((c) => { if (args.ids.includes(c.id)) c.favorite = args.favorite; }); return null;
          case "get_state": mockDeps.sync(); syncCaptures(); return state;
          case "auto_info": return state.auto;
          case "recommended_settings": return {
            input: { gpu: "NVIDIA GeForce RTX 5060 Ti", encoder: "nvenc", width: 2160, height: 1440, hz: 280, ram_gb: 31.9, threads: 12, build: 26300, has_wgc: true, border_ok: true, longest_secs: 120 },
            items: [
              { key: "encoder", value: "auto", why: "Auto uses {encoder}, which passed a test encode on this PC", vars: { encoder: "nvenc" } },
              { key: "method", value: "auto", why: "Auto uses Windows Graphics Capture, the smoothest at high frame rates", vars: {} },
              { key: "aspect", value: "auto", why: "Fixes stretched resolutions and leaves everything else alone", vars: {} },
              { key: "height", value: 0, why: "Your screen's own size, which {encoder} encodes comfortably at {fps} fps", vars: { encoder: "nvenc", fps: 120 } },
              { key: "fps", value: 120, why: "Your {hz} Hz screen and {encoder} have room for 120 fps, which suits Blur", vars: { hz: 280, encoder: "nvenc" } },
              { key: "bitrate", value: 0, why: "Uses only as much as the picture needs, which keeps the buffer small", vars: {} },
              { key: "quality", value: "high", why: "High looks clean and the graphics card does the work", vars: {} },
              { key: "buffer", value: "ram", why: "Your {ram} GB of RAM holds the whole buffer (at most {size} GB), so nothing is written to disk", vars: { ram: 32, size: 0.4 } },
            ],
          };
          case "get_status": case "toggle_recording": mockDeps.sync(); return syncCaptures();
          case "save_settings": state.settings = args.settings; return syncCaptures();
          case "list_clips": case "rescan_library": return clips;
          case "set_favorite": { const c = clips.find((x) => x.id === args.id); c.favorite = args.favorite; return c; }
          case "rename_clip": { const c = clips.find((x) => x.id === args.id); c.title = args.title; return c; }
          // Settings → Components (deps.js). Blur starts missing. localStorage["glint.mockNoFfmpeg"] = "1" (then reload):
          // ffmpeg and ffprobe are missing too and the status bar shows the "not found" error with its
          // Install button. window.__mockOffline = true: installs fail as with no internet.
          case "check_deps": await new Promise((r) => setTimeout(r, 400)); mockDeps.sync(); return { items: mockDeps.items(), installing: mockDeps.busy };
          case "install_dep": return mockDeps.install(args.id);
          case "cancel_dep_install": mockDeps.cancel = true; return !!mockDeps.busy;
          default: return null;
        }
      },
    },
    event: { listen: async (name, fn) => { (listeners[name] ||= []).push(fn); return () => {}; } },
  };
  // Fire a backend event by hand: window.__mockEmit("blur-progress", { id, pct, state, message }).
  window.__mockEmit = (name, payload) => (listeners[name] || []).forEach((fn) => fn({ payload }));
}
