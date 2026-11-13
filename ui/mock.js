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
      cursor: true, desktop_audio: true, mic: true, mic_denoise: false, record_on_start: true, sound: true, sound_name: "chirp", sound_volume: 40, sound_file: "",
      toast: true, toast_pos: "tr", toast_opacity: 96, toast_scale: 100, toast_secs: 3.5, toast_length: true, toast_game: true, toast_mic: true,
      ui_zoom: 100, blur_parallel: 2, language: "en", aspect: "auto", theme: "glint", custom_css: "", discord_rpc: false, discord_app_id: "", color_presets: [{ name: "Mine", brightness: 1.05, contrast: 1.1, saturation: 1.2, gamma: 1 }], editor_keys: {}, toast_compact: false, show_empty_main: false, renders_button: "auto", autostart: false, close_to_tray: true,
      ffmpeg_path: "", blur_path: "",
      keybinds: [
        { id: "a", ctrl: false, shift: false, alt: false, win: false, key: "F8", action: "clip", seconds: 30 },
        { id: "b", ctrl: false, shift: true, alt: false, win: false, key: "F8", action: "clip", seconds: 600 },
        { id: "c", ctrl: true, shift: false, alt: true, win: false, key: "KeyR", action: "toggle", seconds: 30 },
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
    monitors: [{ index: 0, name: "DISPLAY1", width: 2160, height: 1440, hz: 280, primary: true, x: 0, y: 0, native_width: 2560, native_height: 1440 }],
    status: { recording: true, encoder: "nvenc", method: "wgc", width: 2560, height: 1440, fps: 60, fps_actual: 59.9, buffer_secs: 412, buffer_max: 600, buffer_bytes: 3.1e7, buffer_disk_bytes: 2.8e8, ram_total: 2.4e8, cpu_pct: 0.9, ram_app: 3.4e7, ram_ffmpeg: 1.4e8, max_mbps: 15, error: "", game: "Overwatch", discord: "off" },
    ffmpeg: "C:\\tools\\ffmpeg\\bin\\ffmpeg.exe", blur: "C:\\tools\\blur\\blur-cli.exe", version: "0.1.0",
    hardware: { gpus: [{ name: "NVIDIA GeForce RTX 5060 Ti", vendor: "nvidia" }], cpu: "11th Gen Intel(R) Core(TM) i5-11600", gpu: { name: "NVIDIA GeForce RTX 5060 Ti", vendor: "nvidia" }, encoder: "nvenc" },
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
  ];
  // A pretend render queue: each job takes about 4 s, at most settings.blur_parallel at once.
  let jobs = [];
  const pumpJobs = () => {
    const limit = state.settings.blur_parallel || 2;
    for (const j of jobs) {
      if (jobs.filter((x) => x.state === "running").length >= limit) break;
      if (j.state !== "queued") continue;
      j.state = "running";
      const tick = setInterval(() => {
        if (j.state !== "running") { clearInterval(tick); return; }
        j.pct = Math.min(100, j.pct + 5);
        if (j.pct >= 100) {
          clearInterval(tick);
          const src = clips.find((x) => x.id === j.clip_id);
          const made = { ...(src || clips[0]), id: "b" + Date.now(), title: j.title + " (blur)", kind: "blur", created: Date.now() };
          clips.push(made);
          j.state = "done";
          j.result_id = made.id;
          window.__mockEmit("blur-progress", { id: j.clip_id, pct: 100, state: "done", message: made.id });
          window.__mockEmit("clips-changed", made.id);
          pumpJobs();
        } else {
          window.__mockEmit("blur-progress", { id: j.clip_id, pct: j.pct, state: "running", message: "" });
        }
        window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x })));
      }, 200);
    }
    window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x })));
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
          case "update_state": return { current: "0.1.0", latest: "0.1.0", available: false, error: "", checked: 0 };
          case "update_check": {
            await new Promise((r) => setTimeout(r, 700));
            window.__mockUpdateChecks = (window.__mockUpdateChecks || 0) + 1;
            const found = window.__mockUpdate || window.__mockUpdateChecks > 1;
            return { current: "0.1.0", latest: found ? "0.2.0" : "0.1.0", available: !!found, error: "", checked: Date.now() / 1000 };
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
            window.__mockEmit("blur-progress", { id: j.clip_id, pct: 0, state: "cancelled", message: "" });
            pumpJobs();
            return true;
          }
          case "clear_jobs": jobs = jobs.filter((j) => j.state === "queued" || j.state === "running"); window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x }))); return null;
          case "blur_clip": {
            if (jobs.some((j) => j.clip_id === args.id && ["queued", "running"].includes(j.state))) throw "That clip is already rendering";
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
          case "get_state": return state;
          case "get_status": case "toggle_recording": return state.status;
          case "save_settings": state.settings = args.settings; return state.status;
          case "list_clips": case "rescan_library": return clips;
          case "set_favorite": { const c = clips.find((x) => x.id === args.id); c.favorite = args.favorite; return c; }
          case "rename_clip": { const c = clips.find((x) => x.id === args.id); c.title = args.title; return c; }
          default: return null;
        }
      },
    },
    event: { listen: async (name, fn) => { (listeners[name] ||= []).push(fn); return () => {}; } },
  };
  // Fire a backend event by hand: window.__mockEmit("blur-progress", { id, pct, state, message }).
  window.__mockEmit = (name, payload) => (listeners[name] || []).forEach((fn) => fn({ payload }));
}
