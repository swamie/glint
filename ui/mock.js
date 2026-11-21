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
  // polls: the developer's polls as the server would answer them (results only once voted, or ended).
  const mockPolls = (() => {
    const sec = Math.floor(now / 1000);
    const polls = [
      { id: 101, question: "Which feature should come next?", options: [{ id: 1, text: "Trim clips right in the overlay" }, { id: 2, text: "Backups to my own cloud drive" }, { id: 3, text: "More editor effects" }, { id: 4, text: "Better GIF export" }],
        multiple: false, allow_comment: false, opens: sec - 3600, closes: sec + 86400 + 4 * 3600 + 120, closed: false, voted: null, base: { 1: 41, 2: 27, 3: 33, 4: 12 } },
      { id: 102, question: "Which games do you clip the most?", options: [{ id: 5, text: "Valorant" }, { id: 6, text: "Counter-Strike 2" }, { id: 7, text: "Overwatch 2" }, { id: 8, text: "Minecraft" }, { id: 9, text: "Something else" }],
        multiple: true, allow_comment: true, opens: sec - 7200, closes: sec + 2 * 86400 - 600, closed: false, voted: null, base: { 5: 58, 6: 44, 7: 39, 8: 21, 9: 30 }, totalBase: 117 },
      { id: 103, question: "Should clips be saved as HEVC by default?", options: [{ id: 10, text: "Yes, smaller files" }, { id: 11, text: "No, keep H.264" }, { id: 12, text: "I don't mind" }],
        multiple: false, allow_comment: false, opens: sec - 5 * 86400, closes: sec - 86400, closed: true, voted: null, base: { 10: 96, 11: 61, 12: 43 } },
    ];
    const view = (p) => {
      const show = p.voted || p.closed;
      const counts = Object.fromEntries(p.options.map((o) => [String(o.id), (p.base[o.id] || 0) + (p.voted?.includes(o.id) ? 1 : 0)]));
      const total = (p.totalBase ?? Object.values(p.base).reduce((a, b) => a + b, 0)) + (p.voted ? 1 : 0);
      const { base, totalBase, ...rest } = p;
      return { ...rest, options: p.options.map((o) => ({ ...o })), voted: p.voted ? [...p.voted] : null, results: show ? { total, counts } : null };
    };
    const api = {
      polls,
      list: () => polls.map(view),
      vote: async ({ poll, options, comment }) => {
        await new Promise((r) => setTimeout(r, 500));
        if (/fail/i.test(comment || "")) throw "Could not reach the polls server. Try again later.";
        const p = polls.find((x) => x.id === poll);
        if (!p || p.closed) throw "This poll has ended.";
        p.voted = [...options];
        const out = view(p);
        window.__mockEmit("polls-changed", polls.map(view));
        return out;
      },
    };
    window.__mockPolls = api;
    return api;
  })();
  const state = {
    settings: {
      clips_dir: "C:\\Users\\you\\Videos\\Glint", game_folders: false, clip_name: [{ kind: "game" }, { kind: "date" }, { kind: "time" }], monitor: 0, height: 0, fps: 60, quality: "high", bitrate: 0, encoder: "auto", method: "auto", buffer: "mixed", buffer_ram_secs: 30, buffer_dir: "",
      cursor: true, desktop_audio: true, mic: true, mic_denoise: false, game_audio: false, desktop_device: "", game_desktop_audio: false, desktop_gain_db: -3, mic_gain_db: 0, game_gain_db: 0, mic_device: "Microphone (Old Webcam)", capture_mode: "desktop", game_auto: true, game_pick: "", game_pick_path: "", game_exes: [], not_game_exes: [], record_on_start: true, sound: true, sound_name: "chirp", sound_volume: 40, sound_file: "",
      toast: true, toast_pos: "tr", toast_opacity: 96, toast_scale: 100, toast_secs: 3.5, toast_length: true, toast_game: true, toast_mic: true,
      ui_zoom: 100, blur_parallel: 2, language: "en", aspect: "auto", theme: "glint", custom_css: "", icon_theme: false, icon_accent: "", icon_accent_2: "", card_tags: [], nav_opacity: 60, theme_order: [], gif: { width: 480, fps: 20, max_mb: 0, copy: false }, upscale: { height: 2160, filter: "sharp", quality: "high", encoder: "auto" }, rules: [], rules_after_export: "", canvas_presets: [], discord_app_id: "", color_presets: [{ name: "Mine", brightness: 1.05, contrast: 1.1, saturation: 1.2, gamma: 1 }], editor_keys: {}, toast_compact: false, show_empty_main: false, renders_button: "auto", autostart: false, close_to_tray: true, update_check: true, polls: true /* polls */, update_auto: true,
      compress_mb: 10, ui_sounds: false, ui_sounds_hover: false, ui_sounds_volume: 50,
      compress: { sizes: [10, 20, 50, 500], preset: "veryfast", audio_kbps: 0, max_height: 0, max_fps: 0, audio: "first", dest: "beside", dest_dir: "", copy: true }, theme_colors: {}, theme_presets: [{ id: "tp-mock1", name: "Night pink", base: "midnight", colors: { accent: "#ff7ab8" }, css: "" }], theme_preset: "", backdrop: "" /* backdrop */, hw_decode: "off", export_priority: "low", folder_colors: { "Best of": "#e5484d" },
      ffmpeg_path: "", blur_path: "",
      keybinds: [
        { id: "a", ctrl: false, shift: false, alt: false, win: false, key: "F8", action: "clip", seconds: 30, monitor: -1 },
        { id: "c", ctrl: true, shift: false, alt: true, win: false, key: "KeyR", action: "toggle", seconds: 30, monitor: -1 },
      ],
      blur: {
        blur: true, amount: 1, output_fps: 60, weighting: "equal", gamma: 1, interpolate: true, interpolated_fps: "5x", interpolation_method: "svp",
        pre_interpolate: false, pre_interpolated_fps: "360", deduplicate: true, deduplicate_method: "svp", encode_preset: "h264", quality: 16, copy_dates: false,
        gpu_decoding: true, gpu_interpolation: true, gpu_encoding: true, gpu_type: "nvidia", rife_gpu: -1,
        timescale: false, input_timescale: 1, output_timescale: 1, adjust_audio_pitch: false, filters: false, brightness: 1, saturation: 1, contrast: 1,
        advanced: false, deduplicate_range: 2, deduplicate_threshold: 0.001, container: "mp4", ffmpeg_filters: "", debug: false,
        gaussian_std_dev: 1, gaussian_mean: 2, gaussian_bound: "[0,2]", svp_preset: "weak", svp_algorithm: 13, block_size: 8, mask_area: 0, rife_model: "rife-v4.26_ensembleFalse",
      },
    },
    monitors: [
      { index: 0, name: "DISPLAY1", width: 2160, height: 1440, hz: 280, primary: true, x: 0, y: 0, native_width: 2560, native_height: 1440 },
      { index: 1, name: "DISPLAY2", width: 1680, height: 1050, hz: 60, primary: false, x: 2160, y: 0, native_width: 1680, native_height: 1050 },
    ],
    status: { recording: true, encoder: "nvenc", method: "wgc", width: 2560, height: 1440, fps: 60, fps_actual: 59.9, buffer_secs: 412, buffer_max: 600, buffer_bytes: 3.1e7, buffer_disk_bytes: 2.8e8, ram_total: 2.4e8, cpu_pct: 0.9, ram_app: 3.4e7, ram_ffmpeg: 1.4e8, max_mbps: 15, error: "", game: "Overwatch", discord: "off", border: "off", capture_mode: "desktop", capture_target: "" },
    // capture2.rs AutoInfo: what each "Auto" resolves to (get_state.auto, auto_info).
    auto: { encoder: "nvenc", method: "wgc", method_why: "", aspect_width: 2560, aspect_height: 1440, mode_width: 2160, mode_height: 1440, widened: true, auto_mbps: 17.7, native_width: 2160, native_height: 1440, ffmpeg: "C:\\tools\\ffmpeg\\bin\\ffmpeg.exe", blur: "C:\\tools\\blur\\blur-cli.exe", border: { build: 26300, configurable: true, main: "Allow", desktop_apps: "Allow", device: "Allow", can_remove: true } },
    // What's new opens by itself after an update: localStorage["glint.seenVersion"] = "0.1.7", then reload.
    ffmpeg: "C:\\tools\\ffmpeg\\bin\\ffmpeg.exe", blur: "C:\\tools\\blur\\blur-cli.exe", version: "0.1.8",
    hardware: { gpus: [{ name: "NVIDIA GeForce RTX 5060 Ti", vendor: "nvidia" }], cpu: "11th Gen Intel(R) Core(TM) i5-11600", gpu: { name: "NVIDIA GeForce RTX 5060 Ti", vendor: "nvidia" }, encoder: "nvenc" },
  };
  const mockWhy = (st, set) => {
    if (st.capture_mode !== "game" || !st.recording) return { code: "", game: "", monitor: 0, evidence: [] };
    const pick = set.game_pick || "";
    const ev = pick ? ["pick"] : ["known", "library:blizzard", "fullscreen"];
    const game = pick ? pick.replace(/\.exe$/i, "") : "Overwatch";
    const implied = { window: "window", screen: "no_window_capture", screen_failed: "exclusive" }[st.capture_target]
      || (pick ? "waiting_pick" : set.game_auto === false ? "auto_off" : "waiting");
    const code = window.__mockCapWhy || implied;
    const waiting = ["waiting", "waiting_pick", "auto_off"].includes(code);
    return { code, game: waiting && code !== "waiting_pick" ? "" : game, monitor: window.__mockCapWhy ? 1 : 0, evidence: waiting ? [] : ev };
  };
  // The monitor picker (monpick.js): window.__mockMonitors = 1 | 2 | 3 sets how many monitors there are (reload),
  // the second smaller and to the left and above the main one, the third portrait on the right.
  const MOCK_MONS = [
    { index: 0, name: "DISPLAY1", width: 2560, height: 1440, hz: 280, primary: true, x: 0, y: 0, native_width: 2560, native_height: 1440 },
    { index: 1, name: "DISPLAY2", width: 1680, height: 1050, hz: 60, primary: false, x: -1680, y: -620, native_width: 1680, native_height: 1050 },
    { index: 2, name: "DISPLAY3", width: 1080, height: 1920, hz: 75, primary: false, x: 2560, y: -240, native_width: 1080, native_height: 1920 },
  ];
  const mockMons = () => MOCK_MONS.slice(0, Math.max(1, Math.min(3, +(window.__mockMonitors || 2))));
  /** A colored test picture of monitor `i` (SVG data URL): its number, a moving bar, so a refresh shows. */
  const mockPreview = (i, width) => {
    const m = mockMons()[i];
    if (!m) throw "No such monitor";
    if (window.__mockPreviewFails) throw "The monitor's picture could not be copied";
    const w = Math.max(64, Math.min(640, width || 320)), h = Math.round((w * m.height) / m.width);
    const hue = [210, 140, 25][i] ?? 300, sec = new Date().getSeconds();
    const bar = ((sec % 10) / 10) * w;
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl(${hue},70%,38%)"/><stop offset="1" stop-color="hsl(${hue + 40},60%,18%)"/></linearGradient></defs><rect width="${w}" height="${h}" fill="url(#g)"/><rect x="${bar}" y="${h * 0.78}" width="${w * 0.12}" height="${h * 0.06}" rx="3" fill="#fff" opacity=".8"/><rect x="${w * 0.06}" y="${h * 0.08}" width="${w * 0.5}" height="${h * 0.06}" rx="3" fill="#fff" opacity=".35"/><text x="50%" y="56%" font-family="sans-serif" font-weight="700" font-size="${Math.round(h * 0.32)}" fill="#fff" opacity=".9" text-anchor="middle">${i + 1}</text></svg>`;
    return "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg);
  };
  // status.captures: one entry per monitor buffered, the capture monitor first (primary), then every
  // other monitor a clip keybind names, as the backend does. window.__mockCapError = "text": the
  // other monitors' captures fail with it (the status bar's warning mark).
  const syncCaptures = () => {
    const st = state.status, set = state.settings;
    // The GAME / DESKTOP switch (capmode.js). window.__mockCapTarget = "waiting" | "screen" | "screen_failed"
    // shows the other states of game mode ("window" when unset).
    st.capture_mode = set.capture_mode === "game" ? "game" : "desktop";
    st.capture_target = st.capture_mode === "game" && st.recording ? window.__mockCapTarget || "window" : "";
    // 0.1.41 (gamedetect.rs Why): what game mode decided and why. window.__mockCapWhy = "exclusive" | "window_failed" |
    // "paused_away" | "waiting_pick" | ... overrides the reason the target above implies.
    st.capture_why = mockWhy(st, set);
    if (st.capture_target === "waiting") { st.captures = []; return st; }
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
      if (p.virt) return { id: p.id, name: p.name, created: p.created, modified: p.modified, clip: mockProj.virtual(p), virtual: true, clips: p.clips.map((c) => ({ ...c })), exported: p.exported, size: 0, busy: false };
      const clip = drafts.find((d) => d.id === p.id) || null;
      return { id: p.id, name: p.name, created: p.created, modified: p.modified, clip: clip && { ...clip, joins: [...clip.joins], join_names: [...clip.join_names] }, virtual: false, clips: p.clips.map((c) => ({ ...c })), exported: p.exported, size: clip ? clip.size : 0, busy: mockProj.busy(p.id) };
    },
    // 0.1.38 (CONTRACT_0138): every project made now is VIRTUAL, only its clip list (p.virt). `virtual(p)` is
    // what project_open returns, as projects.rs virtual_clip: the clips end to end, each a source. Every source
    // plays the real test video ("/__test.mp4?src=<clip id>"), so a source is at most MOCK_VIDEO_SECS long.
    // window.__mockMissing = "<clip id>": that clip counts as gone (as one deleted from the library).
    // 0.1.41 (F1): an old project (with a draft) that got clips since has `p.base` (its draft as it was) and
    // `p.baseClips` (how many of p.clips are inside it): source 0 is the draft file, with `inner` = its clips
    // ({ at, title }: seconds in that file), and joins / join_names list every border, the leading 0 included.
    virtual: (p) => {
      if (!p.clips.length && !p.base) return null;
      let at = 0;
      const sources = [], joins = [], names = [];
      if (p.base) {
        const b = p.base, nm = (i) => b.join_names[i] || `Clip ${i + 1}`;
        const inner = [{ at: 0, title: nm(0) }, ...b.joins.map((j, i) => ({ at: j, title: nm(i + 1) }))];
        for (const x of inner) { joins.push(x.at); names.push(x.title); }
        sources.push({ clip: p.id, path: b.path, title: p.name, duration: b.duration, start: 0, width: b.width, height: b.height, fps: b.fps, audio_tracks: b.audio_tracks, thumb: "", missing: false, draft: true, inner });
        at = b.duration;
      }
      for (const pc of p.clips.slice(p.base ? p.baseClips || 0 : 0)) {
        const c = window.__mockMissing === pc.id ? null : clips.find((x) => x.id === pc.id);
        const s = c
          ? { clip: pc.id, path: `/__test.mp4?src=${pc.id}`, title: c.title, duration: pc.duration, start: at, width: c.width, height: c.height, fps: c.fps, audio_tracks: c.audio_tracks, thumb: c.thumb || "", missing: false }
          : { clip: pc.id, path: "", title: pc.title, duration: pc.duration, start: at, width: pc.width, height: pc.height, fps: pc.fps, audio_tracks: pc.audio_tracks, thumb: "", missing: true };
        joins.push(at);
        names.push(s.title);
        at += pc.duration;
        sources.push(s);
      }
      const f = sources[0];
      return {
        id: p.id, draft: true, virtual: true, kind: "joined", title: p.name, game: (p.clips[0] || {}).game || "", created: p.created, duration: at,
        width: f.width, height: f.height, fps: f.fps, path: "", joins, join_names: names,
        audio_tracks: f.audio_tracks, sources, thumb: (sources.find((s) => s.thumb) || {}).thumb || "", size: p.clips.reduce((n, c) => n + (c.size || 0), 0), folder: "", favorite: false,
      };
    },
    /** Project time of a virtual project: the draft source (F1), then the later clips. */
    len: (p) => (p.base ? p.base.duration + p.clips.slice(p.baseClips || 0).reduce((s, c) => s + c.duration, 0) : p.clips.reduce((s, c) => s + c.duration, 0)),
    /** What a project keeps of a library clip (projects.rs ProjectClip). */
    keep: (c) => ({ id: c.id, title: c.title, duration: Math.min(c.duration, MOCK_VIDEO_SECS), size: c.size, width: c.width, height: c.height, fps: c.fps, audio_tracks: c.audio_tracks, game: c.game }),
    /** project_add_clips on a virtual project: at once, no job (projects.rs add_now). */
    addNow: (p, ids) => {
      const uniq = [...new Set(ids)];
      if (!uniq.length) throw "Pick at least one clip to add";
      if (p.clips.length + uniq.length > 500) throw "A project takes at most 500 clips";
      const picked = uniq.map((id) => clips.find((c) => c.id === id));
      if (picked.some((c) => !c)) throw "Clip not found";
      const added = picked.map(mockProj.keep);
      let at = mockProj.len(p);
      if (at + added.reduce((s, c) => s + c.duration, 0) > 14400) throw "A project takes at most 4 hours of clips";
      const starts = added.map((c) => { const s = at; at += c.duration; return s; });
      p.virt = true;
      p.clips.push(...added);
      p.modified = Math.max(Date.now(), p.modified + 1);
      p.exported = false;
      mockProj.changed();
      return { project: mockProj.view(p), added: added.map((c) => ({ ...c })), starts, titles: added.map((c) => c.title), job_id: "", mode: "reference", secs: 0 };
    },
    make: (name, virt = true) => {
      const now = Date.now();
      const p = { id: "p" + now + "-" + mockProj.seq++, name: String(name || "").trim().slice(0, 100) || "New project", created: now, modified: now, clips: [], exported: false, state: null, virt };
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
      if (have + uniq.length > 500) throw "A project takes at most 500 clips";
      const picked = uniq.map((id) => clips.find((c) => c.id === id));
      if (picked.some((c) => !c)) throw "Clip not found";
      const secs = (hasDraft ? p.clips.reduce((s, c) => s + c.duration, 0) : 0) + pending.reduce((s, j) => s + j.picked.reduce((a, c) => a + c.duration, 0), 0);
      if (secs + picked.reduce((s, c) => s + c.duration, 0) > 14400) throw "A project takes at most 4 hours of clips";
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
  // 0.1.41: an export's run, as editor.rs / projexport.rs report it: "export-progress" { id, pct, stage, stage_pct }
  // (stage "join" while a project's clips are joined, 0..40 %, then "export"), about 2 s. cancel_export ends it
  // (it then fails). window.__mockExport = "stall": it stops at 47.9 % and fails after 9 s with the stall message;
  // "fail": it fails at 60 % with an ffmpeg error; undefined: it finishes.
  const mockExportCancel = new Set();
  async function mockExportRun(id, join) {
    mockExportCancel.delete(id);
    const mode = window.__mockExport;
    const step = (pct, stage, stagePct) => window.__mockEmit("export-progress", { id, pct, stage, ...(stagePct != null ? { stage_pct: stagePct } : {}) });
    const wait = async (ms) => {
      await new Promise((r) => setTimeout(r, ms));
      if (mockExportCancel.has(id)) { mockExportCancel.delete(id); throw "The export was cancelled"; }
    };
    if (join) for (const sp of [0, 35, 70, 100]) { await wait(250); step(sp * 0.4, "join", sp); }
    const from = join ? 40 : 0;
    for (let k = 1; k <= 10; k++) {
      await wait(200);
      const pct = from + ((100 - from) * k) / 10;
      if (mode === "fail" && pct >= 60) throw "ffmpeg stopped with exit code 1: Error while filtering: Invalid argument";
      if (mode === "stall" && pct >= 47) {
        step(47.9, "export");
        for (let i = 0; i < 60; i++) await wait(150);
        throw "The export stopped making progress at 47 %. ffmpeg was ended and nothing was saved. What ffmpeg said last is in the log.";
      }
      step(pct, "export");
    }
  }
  // A project of three sources (20 s, 14 s, 20 s) to open from the Editor page (0.1.38).
  {
    const p = mockProj.make("Three sources (mock)");
    p.clips = ["c2", "c7", "c9"].map((id) => mockProj.keep(clips.find((c) => c.id === id)));
    p.created = p.modified = now - 3600000;
  }
  // 0.1.41 (F1): a project made before 0.1.38 that got a clip since: its draft (the 20 s test video, two clips inside
  // it, the second at 8 s) is source 0 with `inner`, then one added clip (B: `inner` pieces and their limits).
  {
    const p = mockProj.make("Old draft project (mock)");
    const k = (id) => mockProj.keep(clips.find((c) => c.id === id));
    p.clips = [k("c4"), k("c8"), k("c9")];
    p.base = { path: "/__test.mp4?draft=old", duration: MOCK_VIDEO_SECS, joins: [8], join_names: [p.clips[0].title, p.clips[1].title], width: 2160, height: 1440, fps: 60, audio_tracks: 3 };
    p.baseClips = 2;
    p.created = p.modified = now - 7200000;
  }
  // A pretend render queue: each job takes about 4 s. Blur jobs run at most settings.blur_parallel
  // at once; compress jobs ("Compress for Discord") one at a time in their own lane, as in jobs.rs.
  let jobs = [];
  let mockUndo = null; // the last bulk rename: { token, items: [[id, old title]] }
  const pumpJobs = () => {
    const limit = state.settings.blur_parallel || 2;
    for (const j of jobs) {
      if (j.state !== "queued" || j.kind === "join") continue; // joins: mockProj.pump
      const squeeze = j.kind === "compress";
      if (jobs.filter((x) => x.state === "running" && (x.kind === "compress") === squeeze).length >= (squeeze ? 1 : limit)) continue;
      j.state = "running";
      // A Blur render says what it does before its first percent (jobs.rs `stage`): two seconds of "start" here.
      let waitTicks = squeeze ? 0 : (window.__mockBlurWait ?? 10);
      if (waitTicks) { j.stage = "start"; window.__mockEmit("blur-progress", { id: j.clip_id, pct: 0, state: "running", message: "", stage: "start" }); }
      const tick = setInterval(() => {
        if (j.state !== "running") { clearInterval(tick); return; }
        if (waitTicks > 0) { waitTicks--; return; }
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
  // 0.1.46 (U3's proof): `await __mockTicks(40, 100)` sends `n` progress ticks `ms` apart, as a render does: a Blur
  // render's "blur-progress" and "jobs-changed" for clip `clip` (default the first), an export's "export-progress"
  // for `exportId` (default the open clip, if any) and a "status-changed". Nothing is made; the pretend job is
  // gone from the list at the end ("cancelled"). Resolves when the last tick is sent.
  // `extra`: more pretend jobs that tick along in "jobs-changed", e.g. { id, kind: "join", result_id: projectId } (a busy
  // project on the Editor page) or { id, kind: "compress", clip_id } (a row of the Compress page). `imports`: also
  // "import-progress"; `deps`: also "deps-progress" (Settings → Components); `update`: also "update-progress".
  // `end`: false leaves the jobs running at the end (default: they end "cancelled").
  window.__mockTicks = async (n = 40, ms = 100, { clip = clips[0]?.id, exportId, extra = [], imports = false, deps = false, update = false, end = true } = {}) => {
    const job = { id: "tick-" + Date.now(), kind: "blur", clip_id: clip, title: "Ticks (mock)", state: "running", pct: 0, message: "", stage: "", queued: Date.now() };
    const more = extra.map((x, k) => ({ title: "Ticks (mock)", message: "", stage: "", queued: Date.now() + k + 1, ...x, state: "running", pct: 0 }));
    const list = () => [...jobs.map((x) => ({ ...x })), { ...job }, ...more.map((x) => ({ ...x }))];
    const ex = exportId ?? (typeof S !== "undefined" ? S.open : null);
    for (let i = 1; i <= n; i++) {
      job.pct = Math.min(99, Math.round((i * 100) / (n + 1)));
      for (const x of more) x.pct = job.pct;
      window.__mockEmit("blur-progress", { id: clip, pct: job.pct, state: "running", message: "" });
      window.__mockEmit("jobs-changed", list());
      if (ex) window.__mockEmit("export-progress", { id: ex, pct: job.pct, stage: "export" });
      if (imports) window.__mockEmit("import-progress", { done: i, total: n, title: "Ticks (mock)" });
      if (deps) window.__mockEmit("deps-progress", { id: "blur", phase: "download", done: i, total: n, error: "" });
      if (update) window.__mockEmit("update-progress", { pct: job.pct });
      window.__mockEmit("status-changed", null);
      await new Promise((r) => setTimeout(r, ms));
    }
    if (!end) return;
    job.state = "cancelled";
    for (const x of more) x.state = "cancelled";
    window.__mockEmit("blur-progress", { id: clip, pct: 0, state: "cancelled", message: "" });
    window.__mockEmit("jobs-changed", [...jobs.map((x) => ({ ...x })), ...more.map((x) => ({ ...x }))]);
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
  // plugins: the marketplace is the repository's own plugins/ folder when the preview is served from the
  // repository's root (node tools/serve.js <repo>, then /ui/index.html); installing fetches the real files and
  // checks their SHA-256 as the backend does. What is installed is kept in localStorage, so the "restart"
  // (a reload here) comes back with it. Served from ui/ alone, the marketplace cannot be reached.
  const mockPlugins = (() => {
    const KEY = "glint.mockPlugins";
    const read = () => { try { return JSON.parse(localStorage.getItem(KEY) || "{}"); } catch { return {}; } };
    const write = (all) => { try { localStorage.setItem(KEY, JSON.stringify(all)); } catch {} };
    const procs = {}, work = {}, dlRun = {};
    const DL_KEY = "glint.mockPluginDownloads";
    let dls = {};
    try { dls = JSON.parse(localStorage.getItem(DL_KEY) || "{}"); } catch {}
    const dlSave = () => { try { localStorage.setItem(DL_KEY, JSON.stringify(dls)); } catch {} };
    let loaded = null;
    const running = (all) => Object.values(all).filter((p) => p.enabled).map((p) => `${p.manifest.id} ${p.manifest.version}`).join(",");
    const index = async () => { const r = await fetch("../plugins/index.json", { cache: "no-store" }); if (!r.ok) throw `Could not reach the plugin list: HTTP ${r.status}`; return r.json(); };
    const hex = async (buf) => [...new Uint8Array(await crypto.subtle.digest("SHA-256", buf))].map((b) => b.toString(16).padStart(2, "0")).join("");
    const api = {
      boot() { const all = read(); loaded = running(all); return Object.values(all).filter((p) => p.enabled).map((p) => ({ id: p.manifest.id, name: p.manifest.name, version: p.manifest.version, author: p.manifest.author, js: p.js, css: p.css || "", actions: p.manifest.actions || [] })); },
      state() {
        const all = read();
        return { installed: Object.values(all).map((p) => ({ id: p.manifest.id, name: p.manifest.name, version: p.manifest.version, description: p.manifest.description, author: p.manifest.author, enabled: p.enabled, official: p.official !== false, problem: "", actions: p.manifest.actions || [] })), restart: loaded !== null && loaded !== running(all), glint: state.version };
      },
      async market() {
        try { const i = await index(); return { plugins: i.plugins.map(({ files, ...l }) => ({ ...l, size: files.reduce((n, f) => n + f.size, 0), too_new: false })), error: "", cached: false }; }
        catch (e) { return { plugins: [], error: String(e?.message ?? e), cached: true }; }
      },
      async install(id) {
        const l = (await index()).plugins.find((x) => x.id === id);
        if (!l) throw "This plugin is no longer in the marketplace";
        const got = {};
        let done = 0;
        for (const f of l.files) {
          const buf = await (await fetch(`../plugins/official/${id}/${f.path}`, { cache: "no-store" })).arrayBuffer();
          if (buf.byteLength !== f.size || (await hex(buf)) !== f.sha256) throw "The download does not match the marketplace's checksum, so it was not installed. If the plugin was just updated, try again in a few minutes.";
          got[f.path] = new TextDecoder().decode(buf);
          done += f.size;
          window.__mockEmit("plugin-progress", { id, pct: 5 + (85 * done) / l.files.reduce((n, x) => n + x.size, 0) });
          await new Promise((r) => setTimeout(r, 250));
        }
        const manifest = JSON.parse(got["plugin.json"]);
        const all = read();
        all[id] = { manifest, js: got[manifest.ui] || "", css: got[manifest.css] || "", enabled: all[id] ? all[id].enabled : true, official: true };
        write(all);
        return api.state();
      },
      set(id, change) { const all = read(); if (change === null) delete all[id]; else if (all[id]) Object.assign(all[id], change); write(all); },
      procStatus: ({ plugin, name }) => ({ plugin, name, running: false, pid: 0, started: 0, error: "", code: null, log: [], ...(procs[`${plugin}/${name}`] || {}) }),
      // A stream key with "bad" in it fails after two seconds, as a refused key does.
      procStart({ plugin, name, args, tool }) {
        const k = `${plugin}/${name}`;
        procs[k] = { running: true, pid: 4242, started: Date.now(), error: "", code: null, log: [] };
        const end = (ms, change) => setTimeout(() => { if (!procs[k].running) return; Object.assign(procs[k], { running: false, code: 0 }, change || {}); window.__mockEmit("plugin-proc", api.procStatus({ plugin, name })); }, ms);
        window.__mockPluginRuns = [...(window.__mockPluginRuns || []), { plugin, name, tool: tool || "", args }];
        if (tool) { // a downloaded program (speech to text): progress in its messages, then its result in the work folder
          if (!dls[`${plugin}/${tool}`]) throw `${tool} is not downloaded yet`;
          const of = args[args.indexOf("-of") + 1] || "", file = of.split(/[\\/]/).pop() + ".json";
          let pct = 0;
          const tick = setInterval(() => { const q = procs[k]; if (!q.running) return clearInterval(tick); pct = Math.min(100, pct + 20); q.log.push(`whisper_print_progress_callback: progress = ${String(pct).padStart(3)}%`); }, 300);
          const lines = ["Okay, push them now.", "He is one shot, behind the box!", "[BLANK_AUDIO]", "Nice, that is the round.", "This subtitle line is long enough that it has to be wrapped onto a second line of the picture", "Let's go again."];
          work[`${plugin}/${file}`] = JSON.stringify({ result: { language: "en" }, transcription: lines.map((text, i) => ({ offsets: { from: 600 + i * 2400, to: 600 + i * 2400 + 1900 }, text: " " + text })) });
          end(1800);
          return api.procStatus({ plugin, name });
        }
        if (args.includes("pcm_s16le")) { end(300); return api.procStatus({ plugin, name }); } // a clip's sound written as a WAV for it
        if (args.includes("-list_devices")) { // ffmpeg's list of cameras: it ends at once, the names are in its messages
          Object.assign(procs[k], { running: false, error: "dummy: Immediate exit requested", log: ['[in#0 @ 000001] "fake_device_0" (video)', '[in#0 @ 000001]   Alternative name "@device_pnp_1"', '[in#0 @ 000001] "Logitech C920" (video)', '[in#0 @ 000001] "Microphone (USB Audio)" (audio)'] });
          setTimeout(() => window.__mockEmit("plugin-proc", api.procStatus({ plugin, name })), 150);
          return api.procStatus({ plugin, name });
        }
        window.__mockPluginArgs = args;
        const tick = setInterval(() => { const p = procs[k]; if (!p.running) return clearInterval(tick); const s = Math.round((Date.now() - p.started) / 1000); p.log.push(`frame=${s * 60} fps= 60 q=21.0 size=${s * 750}KiB time=00:00:${String(s).padStart(2, "0")}.00 bitrate=5998.3kbits/s speed=1x`); p.log = p.log.slice(-40); }, 2000);
        if (/bad/.test(args[args.length - 1])) setTimeout(() => { Object.assign(procs[k], { running: false, error: "rtmp://live.twitch.tv/app/…: Input/output error" }); window.__mockEmit("plugin-proc", api.procStatus({ plugin, name })); }, 2000);
        return api.procStatus({ plugin, name });
      },
      // A plugin's downloads: nothing is fetched here, the bar just fills.
      dlState({ plugin }) { return (read()[plugin]?.manifest.downloads || []).map((d) => ({ id: d.id, name: d.name || d.id, size: d.size, installed: !!dls[`${plugin}/${d.id}`], busy: !!dlRun[`${plugin}/${d.id}`] })); },
      async dlGet({ plugin, id }) {
        const d = (read()[plugin]?.manifest.downloads || []).find((x) => x.id === id), k = `${plugin}/${id}`;
        if (!d) throw "The plugin has no such download";
        dlRun[k] = { cancel: false };
        for (let i = 0; i <= 10; i++) {
          if (dlRun[k].cancel) { delete dlRun[k]; window.__mockEmit("plugin-download", { plugin, id, phase: "error", done: 0, total: d.size, error: "Cancelled." }); throw "Cancelled."; }
          window.__mockEmit("plugin-download", { plugin, id, phase: "download", done: (d.size * i) / 10, total: d.size, error: "" });
          await new Promise((r) => setTimeout(r, 120));
        }
        delete dlRun[k];
        dls[k] = true; dlSave();
        window.__mockEmit("plugin-download", { plugin, id, phase: "done", done: d.size, total: d.size, error: "" });
        return api.dlState({ plugin });
      },
      dlCancel({ plugin, id }) { const r = dlRun[`${plugin}/${id}`]; if (r) r.cancel = true; return !!r; },
      dlRemove({ plugin, id }) { delete dls[`${plugin}/${id}`]; dlSave(); return api.dlState({ plugin }); },
      workRead({ plugin, name }) { const x = work[`${plugin}/${name}`]; if (x === undefined) throw "The plugin's process wrote no result"; return x; },
      workClear({ plugin }) { for (const k of Object.keys(work)) if (k.startsWith(plugin + "/")) delete work[k]; return true; },
      procStop({ plugin, name }) { const p = procs[`${plugin}/${name}`]; if (p) p.running = false; window.__mockEmit("plugin-proc", api.procStatus({ plugin, name })); return api.procStatus({ plugin, name }); },
    };
    return api;
  })();
  window.__TAURI__ = {
    core: {
      convertFileSrc: (p) => p,
      invoke: async (cmd, args) => {
        window.__mockCalls.push([cmd, args]);
        switch (cmd) {
          case "list_games": return games;
          case "pick_game_icon": return null;
          case "find_game_icon": return null;
          case "reload_app_icon": return { window: true, tray: true };
          case "bug_report_info": return { configured: true, version: "0.1.6", system: "Glint 0.1.6\nWindows 11 (build 26300)\nNVIDIA GeForce RTX 5060 Ti\nCapture 2560x1440 @ 120 fps, nvenc, wgc, buffer mixed", log: "2026-11-13 09:13:54 hotkey kb-30s -> clip 30s\n2026-11-13 09:13:56 clip 30s: saved 30.4s, 184.2 MB in 1743 ms -> ~\\Videos\\Glint\\Risk of Rain 2\\Risk of Rain 2 2026-11-13 09-13-54.mp4" };
          case "send_bug_report": await new Promise((r) => setTimeout(r, 600)); if (/fail/i.test(args.text)) throw "Discord did not accept the report (HTTP 400)"; return null;
          case "feedback_info": return { configured: true, version: "0.1.35", language: state.settings.language || "en" };
          // feedback.js: a text with "offline" / "busy" / "fail" shows those errors.
          case "send_feedback": await new Promise((r) => setTimeout(r, 600)); if (/offline/i.test(args.text)) throw "Could not reach Discord. Check the internet connection and try again. (no answer)"; if (/busy/i.test(args.text)) throw "Discord is busy right now (HTTP 429). Try again in a minute."; if (/fail/i.test(args.text)) throw "Discord did not accept the feedback (HTTP 400)."; return null;
          // 0.1.41 (F2/F3): window.__mockSlow = ms makes each source's tracks, waveforms and film strip take that long.
          case "editor_prep_cancel": return null;
          // 0.1.41 (item T): the mock's clips have no thumbnail files; window.__mockThumbArrives = true: one appears.
          // thumbs-refresh: the missing ones (or all) "made" in a second; window.__mockThumbFail = n: n of them fail.
          case "thumbs_refresh": {
            const list = clips.filter((c) => args.all || !c.thumb);
            if (!list.length) return { job: null, count: 0 };
            const failN = Math.min(list.length, +window.__mockThumbFail || 0);
            setTimeout(() => window.__mockEmit("thumbs-done", { all: !!args.all, total: list.length, made: list.length - failN, failed: failN, reasons: failN ? [["the file is missing", failN]] : [], cancelled: false }), 1000);
            return { job: null, count: list.length };
          }
          case "thumb_retry": {
            const c = clips.find((x) => x.id === args.id);
            if (c && window.__mockThumbArrives && !c.thumb) {
              setTimeout(() => { c.thumb = `data:image/svg+xml,${encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="160" height="90"><rect width="160" height="90" fill="#2a6"/></svg>')}`; window.__mockEmit("clips-changed", c.id); }, 400);
            }
            return true;
          }
          case "editor_audio_tracks": if (window.__mockSlow) await new Promise((r) => setTimeout(r, +window.__mockSlow)); return [{ index: 0, title: "Mix", path: "/__test.mp4" }, { index: 1, title: "Desktop", path: "/__test.mp4" }, { index: 2, title: "Microphone", path: "/__test.mp4" }];
          case "editor_waveforms": if (window.__mockSlow) await new Promise((r) => setTimeout(r, +window.__mockSlow)); return [0, 1, 2].map((index) => ({ index, per_second: 50, peaks: Array.from({ length: 1000 }, (_, i) => Math.abs(Math.sin(i / (7 + index * 3))) * (0.25 + 0.75 * Math.abs(Math.sin(i / 90 + index)))) }));
          case "editor_thumbs": if (window.__mockSlow) await new Promise((r) => setTimeout(r, +window.__mockSlow)); return Array.from({ length: args.count }, (_, i) => `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="128" height="72"><rect width="128" height="72" fill="hsl(${(i * 37) % 360} 45% 30%)"/><text x="64" y="44" font-size="24" fill="white" text-anchor="middle">${i + 1}</text></svg>`)}`);
          case "editor_audio_info":
            if (/noaudio/.test(args.path)) throw "That file has no audio in it";
            return { path: "/__test.mp4", title: args.path.split(/[\\/]/).pop().replace(/\.[^.]+$/, ""), duration: 8, per_second: 50, peaks: Array.from({ length: 400 }, (_, i) => Math.abs(Math.sin(i / 5)) * 0.8) };
          case "pick_audio_file": return "C:\\Music\\victory.mp3";
          // media.js: "Add a picture, GIF or clip" (edmedia.rs media_info). The test video stands in for a clip file.
          case "pick_media_file": return "/__test.mp4";
          case "editor_media_info": {
            const p = String(args.path || ""), kind = /^data:image\/gif|\.gif$/i.test(p) ? "gif" : /\.(mp4|mkv|mov|webm)$/i.test(p) ? "video" : "image";
            return { path: p, kind, title: p.startsWith("data:") ? "picture" : p.split(/[\\/]/).pop().replace(/\.[^.]+$/, ""), width: kind === "video" ? 1280 : 320, height: kind === "video" ? 720 : 180, duration: kind === "image" ? 0 : kind === "gif" ? 2 : 20, audio: kind === "video" };
          }
          // overlays.js: the export's text / shape picture as a temp PNG (editor.rs save_overlay_png).
          case "editor_overlay_png":
            if (!/^data:image\/png;base64,/.test(args.data || "")) throw "The overlay picture is not a PNG";
            return `C:\\Users\\you\\AppData\\Roaming\\Glint\\tmp\\overlays\\mock${window.__mockCalls.length}.png`;
          // Upscale for YouTube (upscale.rs): one job each, refused like the backend; 4 s each, then "clip-upscaled".
          case "upscale_clips": {
            const h = args.opts?.height || 2160, label = h === 2160 ? "4K" : `${h}p`;
            const made = [];
            let first = "";
            for (const id of args.ids || []) {
              const src = clips.find((x) => x.id === id);
              const why = !src ? "Clip not found" : jobs.some((j) => j.clip_id === id && j.kind === "upscale" && ["queued", "running"].includes(j.state)) ? "That clip is already being upscaled"
                : (src.height || 1080) >= h ? `This clip is already ${src.height || 1080}p, as tall as ${label} or taller, so upscaling would not help` : "";
              if (why) { first = first || why; continue; }
              const job = { id: "j" + Date.now() + jobs.length, clip_id: id, title: src.title, kind: "upscale", state: "running", stage: args.opts?.encoder === "x264" ? "x264" : "nvenc", pct: 0, message: "", result_id: "", queued: Date.now() };
              jobs.push(job);
              made.push({ ...job, state: "queued" });
              const tick = setInterval(() => {
                if (job.state !== "running") return clearInterval(tick);
                job.pct = Math.min(100, job.pct + 5);
                if (job.pct >= 100) {
                  clearInterval(tick);
                  if (window.__mockUpscaleFails) { job.state = "error"; job.message = "Could not upscale the clip: No capable devices found"; }
                  else {
                    const w = Math.round(((src.width || 1920) * h) / (src.height || 1080) / 2) * 2, size = Math.round((src.size || 50e6) * 4);
                    const clip = { ...src, id: "u" + Date.now(), title: `${src.title} (${label})`, kind: "upscale", width: w, height: h, size, created: Date.now(), path: (src.path || "C:\\clip.mp4").replace(/(\.[^.\\]+)?$/, ` (${label}).mp4`) };
                    clips.push(clip);
                    job.state = "done";
                    job.result_id = clip.id;
                    window.__mockEmit("clips-changed", clip.id);
                    window.__mockEmit("clip-upscaled", { clip, size, encoder: job.stage, secs: 4, job_id: job.id, source: id });
                  }
                }
                window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x })));
              }, 200);
            }
            if (!made.length && first) throw first;
            window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x })));
            return made;
          }
          case "gif_export": {
            // gif.rs: one job per clip, 2 s of progress, then "gif-done". window.__mockGifOver = true: over its limit.
            const made = [];
            for (const id of args.ids || []) {
              const src = clips.find((x) => x.id === id);
              if (!src) { made.missing = true; continue; }
              if (jobs.some((x) => x.clip_id === id && x.kind === "gif" && ["queued", "running"].includes(x.state))) continue;
              const j = { id: "g" + Date.now() + made.length, clip_id: id, title: src.title, kind: "gif", state: "running", pct: 0, message: "", result_id: "", queued: Date.now(), stage: "" };
              jobs.push(j);
              made.push({ ...j });
              const over = !!window.__mockGifOver && args.opts?.max_mb > 0;
              const tick = setInterval(() => {
                if (j.state !== "running") { clearInterval(tick); return; }
                j.pct = Math.min(100, j.pct + 10);
                if (over && j.pct >= 90) j.stage = "smaller";
                if (j.pct >= 100) {
                  clearInterval(tick);
                  const path = (src.path || "C:\\Users\\you\\Videos\\Glint\\clip.mp4").replace(/[^\\]*$/, `${src.title}.gif`);
                  const size = over ? (args.opts.max_mb + 3) * 1e6 : 3.2e6;
                  j.state = "done";
                  j.result_id = path;
                  if (over) j.stage = "over";
                  window.__mockEmit("gif-done", { path, size, source: id, job_id: j.id, copied: !!args.opts?.copy, over, max_mb: args.opts?.max_mb || 0 });
                }
                window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x })));
              }, 200);
            }
            if (!made.length) throw made.missing ? "Clip not found" : "A GIF of that clip is already being made";
            window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x })));
            return made;
          }
          case "rules_run": {
            // rules.rs: one job of kind "rule" per clip, each step about a second (stage = its kind), then "rule-done".
            // A refused clip comes back unqueued (id "", state "error"). window.__mockRuleFail = true: the last step fails.
            const rule = (state.settings.rules || []).find((r) => r.id === args.rule);
            if (!rule) throw "That render rule no longer exists";
            if (!rule.steps?.length) throw "This rule has no blocks yet. Add one first.";
            if (rule.steps.some((st) => st.kind === "blur") && !state.blur) throw "blur-cli.exe was not found. Install Blur or set its path under Blur settings.";
            // 0.1.27 (rules.rs check): a Canvas block names a saved layout by id, else by name.
            const cvStep = rule.steps.find((st) => st.kind === "canvas"), cvList = state.settings.canvas_presets || [];
            if (cvStep && !cvStep.layout && !cvStep.layout_name) throw "Pick a layout for the Canvas block.";
            if (cvStep && !cvList.some((p) => (cvStep.layout && p.id === cvStep.layout) || (cvStep.layout_name && p.name === cvStep.layout_name))) throw `The canvas layout "${cvStep.layout_name || cvStep.layout}" no longer exists. Pick another one in the rule.`;
            const out = [];
            for (const id of args.ids || []) {
              const src = clips.find((x) => x.id === id);
              if (!src) { out.push({ id: "", clip_id: id, kind: "rule", state: "error", message: "Clip not found", pct: 0, title: "", result_id: "", queued: 0, stage: "" }); continue; }
              if (jobs.some((x) => x.clip_id === id && x.kind === "rule" && ["queued", "running"].includes(x.state))) { out.push({ id: "", clip_id: id, kind: "rule", state: "error", message: "That clip is already going through a render rule", pct: 0, title: src.title, result_id: "", queued: 0, stage: "" }); continue; }
              const j = { id: "r" + Date.now() + out.length, clip_id: id, title: src.title, kind: "rule", state: "running", pct: 0, message: "", result_id: "", queued: Date.now(), stage: rule.steps[0].kind };
              jobs.push(j);
              out.push({ ...j });
              const per = 100 / rule.steps.length;
              const tick = setInterval(() => {
                if (j.state !== "running") { clearInterval(tick); return; }
                j.pct = Math.min(100, j.pct + per / 5);
                const k = Math.min(rule.steps.length - 1, Math.floor(j.pct / per - 1e-6));
                j.stage = rule.steps[k].kind;
                if (window.__mockRuleFail && k === rule.steps.length - 1) { clearInterval(tick); j.state = "error"; j.message = `${j.stage}: ffmpeg stopped with exit code 1`; }
                else if (j.pct >= 100) {
                  clearInterval(tick);
                  const gif = rule.steps.some((st) => st.kind === "gif");
                  const copied = rule.steps.some((st) => st.kind === "copy");
                  const made = gif ? null : { ...src, id: "rr" + Date.now(), title: `${src.title} (${rule.name})`, kind: "rule", created: Date.now() };
                  if (made) { clips.push(made); window.__mockEmit("clips-changed", made.id); }
                  j.state = "done";
                  j.result_id = made ? made.id : "";
                  window.__mockEmit("rule-done", { job_id: j.id, source: id, rule: rule.id, name: rule.name, clip: made, path: (src.path || "C:\\clip.mp4").replace(/[^\\]*$/, `${src.title} (${rule.name}).${gif ? "gif" : "mp4"}`), size: 4.2e6, gif, copied, skipped: [], nothing: false, over_limit: false });
                }
                window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x })));
              }, 200);
            }
            if (!out.some((j) => j.id)) throw out[0]?.message || "No clip was picked";
            window.__mockEmit("jobs-changed", jobs.map((x) => ({ ...x })));
            return out;
          }
          case "export_clip": {
            const src = clips.find((x) => x.id === args.id) || drafts.find((x) => x.id === args.id);
            if (!src) throw "Clip not found";
            await mockExportRun(args.id, false);
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
          // 0.1.40: video files dropped into the editor (import.rs import_files). A path already in the library gives
          // its clip (known); a new video becomes a library clip that plays the real test video
          // ("/__test.mp4?src=<id>", 12 s); a name with "broken" is unreadable; anything else is "not a video".
          case "import_files": {
            await new Promise((r) => setTimeout(r, 120));
            const out = [];
            let made = 0;
            for (const path of args.paths || []) {
              const known = clips.find((c) => c.srcPath === path || c.path === path);
              if (known) { out.push({ path, clip: { ...known }, known: true, error: "" }); continue; }
              if (!/\.(mp4|mkv|mov|webm|avi|m4v|flv|ts|wmv)$/i.test(path)) { out.push({ path, clip: null, known: false, error: "not a video" }); continue; }
              if (/broken/i.test(path)) { out.push({ path, clip: null, known: false, error: "not a readable video" }); continue; }
              const id = "d" + (++mockProj.seq) + "-" + clips.length;
              const title = path.split(/[\\/]/).pop().replace(/\.[^.]+$/, "");
              const c = clip(0, title, "Imported", 1, 12, { id, kind: "import", path: `/__test.mp4?src=${id}`, srcPath: path, folder: String(args.folder || ""), width: 1920, height: 1080 });
              clips.push(c);
              made++;
              out.push({ path, clip: { ...c }, known: false, error: "" });
            }
            if (made) window.__mockEmit("clips-changed", "");
            return out;
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
            if (picked.length > 500) throw "A project takes at most 500 clips";
            if (picked.reduce((s, c) => s + c.duration, 0) > 14400) throw "A project takes at most 4 hours of clips";
            if (window.__mockJoinMissing) throw `The video file of "${picked[picked.length - 1].title}" is missing`;
            // 0.1.38: a project of references, made at once and returned (the UI opens it).
            const p = mockProj.make(`${picked[0].title} + ${picked.length - 1} more`);
            p.clips = picked.map(mockProj.keep);
            mockProj.changed();
            return mockProj.view(p);
          }
          case "project_open": {
            const p = mockProj.list.find((x) => x.id === args.id);
            if (!p) throw "Project not found";
            if (p.virt) { const v = mockProj.virtual(p); if (!v) throw "This project has no clips yet"; return v; }
            const d = drafts.find((x) => x.id === p.id);
            if (!d) throw p.clips.length ? "The working copy of this project is missing" : "This project has no clips yet";
            return { ...d, joins: [...d.joins], join_names: [...d.join_names] };
          }
          // Agent A's command (0.1.41): end the running export of clip or project `id` (mockExportRun).
          case "cancel_export": mockExportCancel.add(args.id); return true;
          case "export_project": {
            // Agent A's command (projects.rs): as export_clip, from the project's sources. A missing source the
            // ranges use is an error that names it, before anything is made (here: any missing source).
            const p = mockProj.list.find((x) => x.id === args.id);
            if (!p) throw "Project not found";
            const v = mockProj.virtual(p);
            if (!v) throw "This project has no clips yet";
            const lost = v.sources.find((s) => s.missing);
            if (lost) throw `The clip "${lost.title}" is missing, so the project cannot be exported`;
            await mockExportRun(args.id, v.sources.length > 1); // several sources: the join first (0.1.41: stage "join")
            const first = clips.find((x) => x.id === v.sources[0].clip) || {};
            const made = { ...first, id: "e" + Date.now(), title: p.name + " (edit)", kind: "edit", created: Date.now(), draft: false, duration: v.duration, path: "/__test.mp4", joins: [], join_names: [] };
            clips.push(made);
            p.exported = true;
            mockProj.changed();
            return made;
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
            // A virtual project (or an empty old one) takes the clips at once; one with a draft queues the old job.
            const hasDraft = drafts.some((d) => d.id === p.id);
            if (p.virt || (!hasDraft && !p.clips.length && !mockProj.busy(p.id))) return mockProj.addNow(p, args.ids || []);
            // 0.1.41 (F1): an old project with a draft becomes one of references: its draft is source 0, no job.
            if (hasDraft && !mockProj.busy(p.id)) {
              const d = drafts.find((x) => x.id === p.id);
              p.base = { path: d.path, duration: d.duration, joins: d.joins.slice(), join_names: d.join_names.slice(), width: d.width, height: d.height, fps: d.fps, audio_tracks: d.audio_tracks };
              p.baseClips = p.clips.length;
              return mockProj.addNow(p, args.ids || []);
            }
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
          // Settings → Audio (audiodev.js). The mock's mic_device is not in the list: it shows the "not connected" note.
          case "audio_devices": return {
            outputs: [{ name: "Speakers (Realtek(R) Audio)", default: true }, { name: "Headphones (Arctis 7 Game)", default: false }, { name: "LG ULTRAGEAR (NVIDIA High Definition Audio)", default: false }],
            inputs: [{ name: "Microphone (Arctis 7 Chat)", default: true }, { name: "Microphone (Realtek(R) Audio)", default: false }],
          };
          // The mixer's meters (audiodev.js): made-up levels that move, the game silent.
          case "audio_levels": { const w = Date.now() / 1000, lvl = (f, top) => top - 14 - 12 * Math.sin(w * f) - 6 * Math.abs(Math.sin(w * f * 3.7)); return { desktop: { left: lvl(1.3, -6), right: lvl(1.5, -7) }, mic: { left: lvl(2.1, -18), right: lvl(2.1, -18) }, game: { left: -120, right: -120 } }; }
          case "set_game": clips.forEach((c) => { if (args.ids.includes(c.id)) c.game = args.game; }); return null;
          case "sort_by_game": { let n = 0; clips.forEach((c) => { if (!c.folder) { c.folder = c.game; if (!folders.includes(c.game)) folders.push(c.game); n++; } }); return n; }
          case "list_folders": return [...folders];
          case "create_folder": if (folders.includes(args.name)) throw `A folder named "${args.name}" already exists`; folders.push(args.name); return args.name;
          case "rename_folder": folders[folders.indexOf(args.from)] = args.to; clips.forEach((c) => { if (c.folder === args.from) c.folder = args.to; }); return args.to;
          case "delete_folder": folders.splice(folders.indexOf(args.name), 1); clips.forEach((c) => { if (c.folder === args.name) c.folder = ""; }); return null;
          case "move_clips": { let n = 0; clips.forEach((c) => { if (args.ids.includes(c.id) && c.folder !== args.folder) { c.folder = args.folder; n++; } }); return n; }
          case "take_pending_edit": { const id = window.__mockPendingEdit || null; window.__mockPendingEdit = null; return id; }
          case "clipboard_text": return window.__mockClipboard ?? "pasted text";
          case "replace_clips": {
            // As core.rs replace_clips: the chosen clips go, and with `adopt` the export takes the first one's name, date and favourite.
            const made = clips.find((c) => c.id === args.id);
            if (!made) throw "Clip not found";
            const gone = [...new Set(args.ids)].filter((id) => id !== args.id && clips.some((c) => c.id === id));
            const first = clips.find((c) => c.id === args.ids[0]);
            if (args.adopt && first && gone.includes(first.id)) Object.assign(made, { title: first.title, created: first.created, favorite: first.favorite });
            clips = clips.filter((c) => !gone.includes(c.id));
            window.__mockEmit("clips-changed", "");
            return { clip: { ...made }, replaced: gone, failed: window.__mockReplaceFails ? ["Some clip: it is being rendered"] : [] };
          }
          case "delete_clips": { const n = clips.length; clips = clips.filter((c) => !args.ids.includes(c.id)); return n - clips.length; }
          case "copy_clips": return args.ids.length;
          case "set_favorites": clips.forEach((c) => { if (args.ids.includes(c.id)) c.favorite = args.favorite; }); return null;
          case "get_state": mockDeps.sync(); state.monitors = mockMons(); syncCaptures(); try { state.version = localStorage.getItem("mock.version") || state.version; } catch {} return state; // newtag: localStorage["mock.version"] plays an update
          // newtag: one id per run (app.rs launch_id). localStorage["mock.launch"] = "2" and a reload plays a restart of Glint; a plain reload is a window reopened from the tray.
          case "launch_id": try { return "mock-run-" + (localStorage.getItem("mock.launch") || "1"); } catch { return "mock-run-1"; }
          // The monitor picker's snapshot picture (monprev.rs): a colored test picture per monitor.
          case "monitor_preview": return mockPreview(args.index, args.width);
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
          // The GAME / DESKTOP switch's right-click menu (capmode.rs game_menu / game_mark).
          case "game_menu": {
            const set = state.settings;
            // 0.1.41: each row's full path and why it counts (gamedetect.rs Ev codes), and the last decision.
            const rows = [
              { exe: "overwatch.exe", name: "Overwatch", title: "Overwatch", game: true, path: "C:\\Program Files (x86)\\Overwatch\\_retail_\\Overwatch.exe", why: ["known", "library:blizzard", "fullscreen"] },
              { exe: "hades.exe", name: "Hades", title: "Hades", game: true, path: "D:\\SteamLibrary\\steamapps\\common\\Hades\\x64\\Hades.exe", why: ["library:steam", "seen"] },
              { exe: "chrome.exe", name: "chrome", title: "YouTube - Google Chrome", game: false, path: "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe", why: [] },
              { exe: "discord.exe", name: "Discord", title: "#general | Friends - Discord", game: false, path: "C:\\Users\\you\\AppData\\Local\\Discord\\app-1.0.9\\Discord.exe", why: [] },
              { exe: "emu.exe", name: "emu", title: "My Emulator 2.1", game: false, path: "C:\\Tools\\MyEmu\\emu.exe", why: [] },
            ];
            const auto = set.game_auto !== false;
            const never = (r) => (set.not_game_exes || []).includes(r.exe);
            const current = set.game_pick || (auto ? rows.find((r) => r.game && !never(r))?.exe || "" : "");
            const out = rows.map((r) => {
              const picked = r.exe === set.game_pick, marked = (set.game_exes || []).includes(r.exe);
              const game = picked || (auto && (r.game || marked) && !never(r));
              const why = picked ? ["pick"] : !game ? [] : r.why.length ? r.why : ["marked"];
              return { ...r, game, why, current: r.exe === current, picked, not_game: never(r) };
            });
            out.sort((a, b) => b.current - a.current || b.picked - a.picked || b.game - a.game || a.name.localeCompare(b.name));
            const st = syncCaptures();
            const decision = set.capture_mode === "game" ? st.capture_why : { code: "desktop", game: "", monitor: 0, evidence: [] };
            return { mode: set.capture_mode || "desktop", auto, pick: set.game_pick || "", decision, current: out.find((r) => r.current) || null, programs: out };
          }
          // "Choose a program…" (capmode.rs game_pick_exe): the mock "picks" a made-up exe; window.__mockPickCancel = true: cancelled.
          case "game_pick_exe": {
            if (window.__mockPickCancel) return null;
            const set = state.settings;
            set.game_pick = "mygame.exe"; set.game_pick_path = "d:\\games\\mygame\\mygame.exe";
            set.not_game_exes = (set.not_game_exes || []).filter((e) => e !== "mygame.exe");
            set.game_exes = [...(set.game_exes || []).filter((e) => e !== "mygame.exe"), "mygame.exe"];
            return JSON.parse(JSON.stringify(set));
          }
          case "game_mark": {
            // 0.1.41: a pick may name the full path; marks are kept by file name, the pick keeps the path too.
            const raw = String(args.exe || "").toLowerCase(), path = /[\\/]/.test(raw) ? raw.replace(/\//g, "\\") : "";
            const set = state.settings, exe = path ? path.split("\\").pop() : raw;
            const drop = (list) => (list || []).filter((e) => e !== exe);
            if (args.action === "auto-on") set.game_auto = true;
            else if (args.action === "auto-off") set.game_auto = false;
            else if (args.action === "unpick") { set.game_pick = ""; set.game_pick_path = ""; }
            else if (args.action === "pick") { if (path || set.game_pick !== exe) set.game_pick_path = path; set.game_pick = exe; set.not_game_exes = drop(set.not_game_exes); set.game_exes = [...drop(set.game_exes), exe]; }
            else if (args.action === "not-game") { set.game_exes = drop(set.game_exes); if (set.game_pick === exe) { set.game_pick = ""; set.game_pick_path = ""; } set.not_game_exes = [...drop(set.not_game_exes), exe]; }
            else throw `Unknown choice "${args.action}"`;
            return JSON.parse(JSON.stringify(set));
          }
          case "list_clips": case "rescan_library": return clips;
          case "set_favorite": { const c = clips.find((x) => x.id === args.id); c.favorite = args.favorite; return c; }
          case "rename_clip": { const c = clips.find((x) => x.id === args.id); c.title = args.title; return c; }
          // Bulk rename (naming.js / naming.rs). Names from nmFormat; unique per folder among the other
          // clips' titles; a clip with a queued or running job is refused; an import changes its title only.
          // window.__mockRenameFails = true: rename_clips leaves every clip as it was, with a reason.
          case "rename_plan": case "rename_clips": {
            const o = args.opts || {};
            const picked = [...new Set(args.ids)].map((id) => clips.find((x) => x.id === id)).filter(Boolean);
            const mine = new Set(picked.map((c) => c.id));
            const taken = new Set(clips.filter((c) => !mine.has(c.id)).map((c) => `${c.folder}\\${c.title}`.toLowerCase()));
            let n = o.start ?? 1;
            const rows = picked.map((c) => {
              const busy = jobs.some((j) => j.clip_id === c.id && (j.state === "queued" || j.state === "running"));
              if (busy) return { id: c.id, old: c.title, new: c.title, clash: false, skip: "This clip is being rendered", title_only: false };
              const name = nmFormat(o.blocks, { game: c.game, when: new Date(c.created), secs: c.duration, number: nmNumber(n++, o.pad || 1), current: c.title });
              let i = 1, cand = name;
              while (taken.has(`${c.folder}\\${cand}`.toLowerCase())) cand = `${name} (${++i})`;
              taken.add(`${c.folder}\\${cand}`.toLowerCase());
              return { id: c.id, old: c.title, new: cand, clash: i > 1, skip: "", title_only: c.kind === "import" };
            });
            if (cmd === "rename_plan") return rows;
            if (window.__mockRenameFails) return { renamed: 0, failed: rows.map((r) => `${r.old} (it is open in another program)`), token: "", clips: [] };
            const changed = rows.filter((r) => !r.skip && r.new !== r.old);
            for (const r of changed) clips.find((x) => x.id === r.id).title = r.new;
            const token = changed.length ? "u" + Date.now() : "";
            mockUndo = { token, items: changed.map((r) => [r.id, r.old]) };
            window.__mockEmit("clips-changed", "");
            return { renamed: changed.length, failed: rows.filter((r) => r.skip).map((r) => `${r.old} (${r.skip})`), token, clips: changed.map((r) => clips.find((x) => x.id === r.id)) };
          }
          case "undo_rename_clips": {
            if (!mockUndo || !args.token || mockUndo.token !== args.token) throw "There is nothing to undo";
            for (const [id, title] of mockUndo.items) { const c = clips.find((x) => x.id === id); if (c) c.title = title; }
            const n = mockUndo.items.length;
            mockUndo = null;
            window.__mockEmit("clips-changed", "");
            return { renamed: n, failed: [], token: "", clips: [] };
          }
          // Settings → Components (deps.js). Blur starts missing. localStorage["glint.mockNoFfmpeg"] = "1" (then reload):
          // ffmpeg and ffprobe are missing too and the status bar shows the "not found" error with its
          // Install button. window.__mockOffline = true: installs fail as with no internet.
          case "check_deps": await new Promise((r) => setTimeout(r, 400)); mockDeps.sync(); return { items: mockDeps.items(), installing: mockDeps.busy };
          case "install_dep": return mockDeps.install(args.id);
          case "cancel_dep_install": mockDeps.cancel = true; return !!mockDeps.busy;
          // polls (polls.js): one unanswered single choice, one with several answers and a comment, one ended.
          // A comment with "fail" shows the "could not reach" error. window.__mockPolls holds the state.
          case "polls_list": return state.settings.polls === false ? [] : mockPolls.list();
          case "polls_vote": return mockPolls.vote(args);
          // plugins (plugins.js): see mockPlugins below.
          case "fire_keybind": { // a plugin's action: its process starts or stops with the arguments its script saved, as in plugins.rs
            const k = state.settings.keybinds.find((x) => x.id === args.id);
            const [kind, plugin, action] = String(k?.action || "").split(":");
            if (kind !== "plugin") return null;
            const name = "live", saved = state.settings.plugins?.[plugin]?.procs?.[name];
            if (mockPlugins.procStatus({ plugin, name }).running) mockPlugins.procStop({ plugin, name });
            else if (saved?.args?.length) mockPlugins.procStart({ plugin, name, args: saved.args });
            window.__mockEmit("plugin-action", { plugin, action });
            return null;
          }
          case "plugins_boot": return mockPlugins.boot();
          case "plugins_state": return mockPlugins.state();
          case "plugins_market": return mockPlugins.market();
          case "plugin_install": return mockPlugins.install(args.id);
          case "plugin_set_enabled": mockPlugins.set(args.id, { enabled: args.on }); return mockPlugins.state();
          case "plugin_uninstall": mockPlugins.set(args.id, null); return mockPlugins.state();
          case "plugins_restart": location.reload(); return null;
          case "plugin_file_keep": if (!/\.(png|jpe?g|gif|webp|bmp)$/i.test(args.path)) throw "Only pictures can be added: PNG, JPEG, GIF, WebP or BMP"; return { path: args.path, name: args.path.split(/[\\/]/).pop(), size: 1234 };
          case "plugin_file_drop": return true;
          case "plugin_proc_start": return mockPlugins.procStart(args);
          case "plugin_proc_stop": return mockPlugins.procStop(args);
          case "plugin_proc_status": return mockPlugins.procStatus(args);
          case "plugin_dl_state": return mockPlugins.dlState(args);
          case "plugin_dl_get": return mockPlugins.dlGet(args);
          case "plugin_dl_cancel": return mockPlugins.dlCancel(args);
          case "plugin_dl_remove": return mockPlugins.dlRemove(args);
          case "plugin_work_read": return mockPlugins.workRead(args);
          case "plugin_work_clear": return mockPlugins.workClear(args);
          case "plugin_save_text": window.__mockSavedText = { name: args.name, text: args.text }; return `C:\\Users\\you\\Videos\\${args.name}`;
          default: return null;
        }
      },
    },
    // emitTo (the library window, libwin.js): kept in window.__mockSent; in the browser there is one window only.
    event: { listen: async (name, fn) => { (listeners[name] ||= []).push(fn); return () => {}; }, emitTo: async (target, name, payload) => { (window.__mockSent ||= []).push([target, name, payload]); } },
  };
  // Fire a backend event by hand: window.__mockEmit("blur-progress", { id, pct, state, message }).
  window.__mockEmit = (name, payload) => (listeners[name] || []).forEach((fn) => fn({ payload }));
  // 0.1.40: a file drop, as Tauri sends it: window.__mockDrop(["C:\\clips\\a.mp4"], x, y) with x / y in CSS px of the
  // window (Tauri's position is in physical px: multiplied by devicePixelRatio here). Calls the editor's listeners.
  window.__mockDrop = (paths, x = 0, y = 0) => {
    const k = window.devicePixelRatio || 1, position = { x: x * k, y: y * k };
    window.__mockEmit("tauri://drag-enter", { paths, position });
    window.__mockEmit("tauri://drag-over", { position });
    window.__mockEmit("tauri://drag-drop", { paths, position });
  };
}
