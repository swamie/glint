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
      ui_zoom: 100, blur_parallel: 2, language: "en", aspect: "auto", theme: "glint", custom_css: "", icon_theme: false, icon_accent: "", icon_accent_2: "", discord_rpc: false, discord_app_id: "", color_presets: [{ name: "Mine", brightness: 1.05, contrast: 1.1, saturation: 1.2, gamma: 1 }], editor_keys: {}, toast_compact: false, show_empty_main: false, renders_button: "auto", autostart: false, close_to_tray: true,
      compress_mb: 10, ui_sounds: false, ui_sounds_hover: false, ui_sounds_volume: 50,
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
    status: { recording: true, encoder: "nvenc", method: "wgc", width: 2560, height: 1440, fps: 60, fps_actual: 59.9, buffer_secs: 412, buffer_max: 600, buffer_bytes: 3.1e7, buffer_disk_bytes: 2.8e8, ram_total: 2.4e8, cpu_pct: 0.9, ram_app: 3.4e7, ram_ffmpeg: 1.4e8, max_mbps: 15, error: "", game: "Overwatch", discord: "off" },
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
  // A pretend render queue: each job takes about 4 s. Blur jobs run at most settings.blur_parallel
  // at once; compress jobs ("Compress for Discord") one at a time in their own lane, as in jobs.rs.
  let jobs = [];
  const pumpJobs = () => {
    const limit = state.settings.blur_parallel || 2;
    for (const j of jobs) {
      if (j.state !== "queued") continue;
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
            const made = { ...(src || clips[0]), id: "d" + Date.now(), title: j.title + " (discord)", kind: "discord", size, created: Date.now() };
            clips.push(made);
            j.state = "done";
            j.result_id = made.id;
            window.__mockEmit("clips-changed", made.id);
            window.__mockEmit("clip-compressed", { clip: made, mb: j.mb, size, copied: !window.__mockCopyFails, secs: 4.1 });
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
            const src = clips.find((x) => x.id === args.id);
            for (const pct of [20, 60, 100]) { await new Promise((r) => setTimeout(r, 150)); window.__mockEmit("export-progress", { id: args.id, pct }); }
            const made = { ...src, id: "e" + Date.now(), title: src.title + " (edit)", kind: "edit", created: Date.now() };
            clips.push(made);
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
            args.items.slice(0, n - 1).forEach((it, i) => clips.push(clip(100 + clips.length + i, it.title, it.game, 60 * 24 * (i + 1), 30, { kind: "import" })));
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
            state.settings.compress_mb = args.mb;
            const job = { id: "j" + Date.now() + jobs.length, clip_id: args.id, title: src.title, kind: "compress", state: "queued", pct: 0, message: "", result_id: "", queued: Date.now(), mb: args.mb };
            jobs.push(job);
            const answer = { ...job }; // as queued, like the backend's answer
            pumpJobs();
            return answer;
          }
          case "join_clips": {
            // "Edit together", refused like join.rs; window.__mockJoinMissing = true: a clip's file is gone.
            // Runs on its own (about 2 s), then "clip-joined" with a split where each clip starts.
            const picked = clips.filter((c) => args.ids.includes(c.id)).sort((a, b) => a.created - b.created);
            if (picked.length < 2) throw "Pick at least two clips to edit together";
            if (picked.length > 20) throw "Edit together takes at most 20 clips";
            if (picked.reduce((s, c) => s + c.duration, 0) > 1800) throw "Edit together takes at most 30 minutes of clips";
            if (window.__mockJoinMissing) throw `The video file of "${picked[picked.length - 1].title}" is missing`;
            const key = "join:" + picked.map((c) => c.id).join(",");
            if (jobs.some((j) => j.clip_id === key && ["queued", "running"].includes(j.state))) throw "Those clips are already being joined";
            const title = `${picked[0].title} + ${picked.length - 1} more`;
            const job = { id: "j" + Date.now() + jobs.length, clip_id: key, title, kind: "join", state: "running", pct: 0, message: "", result_id: "", queued: Date.now() };
            jobs.push(job);
            const answer = { ...job };
            const tick = setInterval(() => {
              if (job.state !== "running") { clearInterval(tick); return; }
              job.pct = Math.min(100, job.pct + 10);
              if (job.pct >= 100) {
                clearInterval(tick);
                let at = 0;
                const joins = picked.slice(0, -1).map((c) => (at += c.duration));
                // The preview's real video is 20 s long, so splits past that are not shown in the editor.
                const made = { ...picked[0], id: "g" + Date.now(), title, kind: "joined", created: Date.now(), duration: at + picked[picked.length - 1].duration, joins, path: "/__test.mp4", favorite: false };
                clips.push(made);
                window.__mockEmit("clips-changed", made.id);
                window.__mockEmit("clip-joined", { clip: made, joins, mode: "copy", secs: 2 });
                job.state = "done";
                job.result_id = made.id;
              }
              window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x })));
            }, 200);
            window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x })));
            return answer;
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
