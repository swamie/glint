// Stream: go live from Glint. An official Glint plugin (plugins/official/stream in Glint's repository).
//
// What it does: a live stream of Glint's (src-tauri/src/live.rs), which is two kinds of ffmpeg with
// Glint between them. The SENDER holds the connection to the RTMP server and encodes only the sound.
// The FEEDER captures and encodes the picture; when its capture ends (Desktop Duplication loses the
// screen each time a game enters or leaves exclusive fullscreen) Glint starts another and the
// stream stays up. This file works out both command lines, what the picture is taken from
// (a screen, the game, one window) and what the sound is mixed from, each part with its own volume.
// The picture is a scene: the screen, cameras (DirectShow), pictures (PNG, JPEG, GIF: dropped on
// the preview) and blur boxes, each moved, sized, cropped, turned and stacked on the Stream page's
// preview. A scene that is only the whole screen keeps the short capture chain; anything else is
// put together by ffmpeg's overlay filter.
// Everything is saved in the plugin's settings (`procs.live`), so the keybind "Start / stop
// streaming" also works while Glint's window is closed.
const { ctx, t, esc } = Glint;
const PROC = "live";
const SERVICES = {
  twitch: { name: "Twitch", url: "rtmp://live.twitch.tv/app" },
  youtube: { name: "YouTube", url: "rtmp://a.rtmp.youtube.com/live2" },
  custom: { name: "", url: "" },
};
let status = { running: false, started: 0, error: "", log: [] };
let showKey = false;
let poll = 0;
let live = null; // what the running stream sends ("1080p @ 60fps"), for the top bar
let liveKey = ""; // the size, frame rate and encoder the running stream started with: only a picture of the same kind can replace it while live
let ended = null; // the stream that just ended: { secs, link }

const even = (v) => Math.max(2, Math.floor(v / 2) * 2);
const serviceOf = (s) => (SERVICES[s.service] ? s.service : "twitch");

/** Where the stream goes: the server's address with the stream key. "" while something is missing. */
function target(s) {
  const service = serviceOf(s);
  const base = (service === "custom" ? String(s.url || "") : SERVICES[service].url).trim().replace(/\/+$/, "");
  const key = String(s.key || "").trim();
  if (!/^rtmps?:\/\//i.test(base) || (!key && service !== "custom")) return "";
  return key ? `${base}/${key}` : base;
}

/** The monitor that is streamed: the one picked, else Glint's capture monitor. */
function monitorOf(c) {
  const g = c.glint;
  const list = g.monitors || [];
  return list.find((m) => m.index === c.s.monitor) || list.find((m) => m.index === g.settings.monitor) || list[0] || { index: 0, name: "", width: 1920, height: 1080 };
}

function encoderOf(c) {
  const g = c.glint;
  const e = c.s.encoder && c.s.encoder !== "auto" ? c.s.encoder : (g.hardware && g.hardware.encoder) || (g.status && g.status.encoder) || "x264";
  return ["nvenc", "amf", "qsv", "x264"].includes(e) ? e : "x264";
}

/** The picture's size: the monitor's own, or scaled down to the height picked. */
function sizeOf(c) {
  const mon = monitorOf(c);
  const h = c.s.height > 0 && c.s.height < mon.height ? c.s.height : mon.height;
  const w = even((mon.width * h) / mon.height);
  return { w, h: even(h), scaled: even(h) !== mon.height || w !== mon.width };
}

/* ---------- the scene ---------- */

const num = (v, d) => (Number.isFinite(+v) ? +v : d);
const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
const f4 = (v) => String(+(+v).toFixed(4));
const newScreen = () => ({ id: "screen", type: "screen", x: 0, y: 0, w: 1, h: 1, crop: [0, 0, 0, 0], rot: 0, flip: false, opacity: 100, visible: true });

/** The scene's sources, bottom layer first, put right (settings can be edited by hand): always one screen. */
function sources(c) {
  const sc = c.s.scene && typeof c.s.scene === "object" ? c.s.scene : (c.s.scene = {});
  let list = Array.isArray(sc.sources) ? sc.sources.filter((x) => x && typeof x === "object" && ["screen", "camera", "image", "blur"].includes(x.type)) : [];
  const first = list.find((x) => x.type === "screen");
  list = list.filter((x) => x.type !== "screen" || x === first);
  if (!first) list.unshift(newScreen());
  for (const x of list) {
    if (x.type === "screen") x.id = "screen";
    else x.id = String(x.id || "src-" + Math.random().toString(36).slice(2, 8));
    x.w = clamp(num(x.w, 1), 0.01, 4); x.h = clamp(num(x.h, 1), 0.01, 4);
    x.x = clamp(num(x.x, 0), -4, 4); x.y = clamp(num(x.y, 0), -4, 4);
    const k = Array.isArray(x.crop) ? x.crop : [];
    x.crop = [0, 1, 2, 3].map((i) => clamp(num(k[i], 0), 0, 0.95));
    if (x.crop[0] + x.crop[2] > 0.95) x.crop[2] = 0.95 - x.crop[0];
    if (x.crop[1] + x.crop[3] > 0.95) x.crop[3] = 0.95 - x.crop[1];
    x.rot = clamp(Math.round(num(x.rot, 0)), -180, 180);
    x.opacity = clamp(Math.round(num(x.opacity, 100)), 5, 100);
    x.flip = !!x.flip; x.visible = x.visible !== false;
    if (x.type === "camera") x.device = String(x.device || "");
    if (x.type === "image") { x.file = String(x.file || ""); x.name = String(x.name || ""); }
    if (x.type === "blur") Object.assign(x, { strength: clamp(Math.round(num(x.strength, 40)), 1, 100), crop: [0, 0, 0, 0], rot: 0, flip: false, opacity: 100 });
  }
  sc.sources = list;
  return list;
}
/** What goes out: the sources that are shown and have something to show. */
const shown = (c) => sources(c).filter((x) => x.visible && (x.type === "screen" || x.type === "blur" || x.device || x.file));
const isGif = (x) => /\.gif$/i.test(x.file || "");
/** How far a blur box smears, in the stream's pixels. */
const sigmaOf = (x) => 2 + x.strength * 0.4;
const untouched = (x) => x.x === 0 && x.y === 0 && x.w === 1 && x.h === 1 && !x.rot && !x.flip && x.opacity === 100 && x.crop.every((v) => !v);
/** Only the whole screen, as it is: nothing to put together. */
const plain = (c) => { const v = shown(c); return v.length === 1 && v[0].type === "screen" && untouched(v[0]); };
/** A source's own shape (width / height) before its crop. */
function shapeOf(c, x) {
  if (x.type === "screen") { const m = monitorOf(c); return m.width / m.height; }
  return x.aw > 0 && x.ah > 0 ? x.aw / x.ah : 16 / 9;
}

/** The scene as ffmpeg's inputs and filter graph: every source cropped, sized, turned and laid over a black picture of the stream's size. */
function sceneGraph(c, fps, w, h) {
  const inputs = [], parts = [`color=c=black:s=${w}x${h}:r=${fps}[bg]`];
  let last = "bg", n = 0, cam = 0;
  for (const x of shown(c)) {
    if (x.type === "blur") {
      // The part of the picture so far that lies under the box, blurred and laid back in its place.
      let x0 = Math.max(0, Math.round(w * x.x)), y0 = Math.max(0, Math.round(h * x.y));
      x0 -= x0 % 2; y0 -= y0 % 2;
      const bw = Math.min(w, Math.round(w * (x.x + x.w))) - x0, bh = Math.min(h, Math.round(h * (x.y + x.h))) - y0;
      if (bw < 8 || bh < 8) continue; // outside the picture
      parts.push(`[${last}]split[a${n}][b${n}]`, `[b${n}]crop=${even(bw)}:${even(bh)}:${x0}:${y0},gblur=sigma=${f4(sigmaOf(x))}:steps=2[s${n}]`, `[a${n}][s${n}]overlay=x=${x0}:y=${y0}[o${n}]`);
      last = `o${n++}`;
      continue;
    }
    let from;
    if (x.type === "screen") from = "{source},"; // Glint puts the capture here, as plain frames (live.rs)
    else {
      // The feeder's inputs are the cameras and pictures, in the order of their layers.
      if (x.type === "camera") inputs.push("-f", "dshow", "-rtbufsize", "256M", "-thread_queue_size", "512", "-i", `video=${x.device}`);
      else if (isGif(x)) inputs.push("-stream_loop", "-1", "-i", x.file); // a GIF plays at its own speed, over and over
      else inputs.push("-loop", "1", "-framerate", "2", "-i", x.file); // a still picture: two frames a second are plenty
      from = `[${cam++}:v]`;
    }
    const [l, t, r, b] = x.crop, chain = [];
    if (l || t || r || b) chain.push(`crop=iw*${f4(1 - l - r)}:ih*${f4(1 - t - b)}:iw*${f4(l)}:ih*${f4(t)}`);
    chain.push(`scale=${even(w * x.w)}:${even(h * x.h)}:flags=bilinear`);
    if (x.flip) chain.push("hflip");
    if (x.rot || x.opacity < 100 || x.type === "image") chain.push("format=rgba"); // a picture keeps its see-through parts
    if (x.rot) { const a = f4((x.rot * Math.PI) / 180); chain.push(`rotate=${a}:c=none:ow=rotw(${a}):oh=roth(${a})`); }
    if (x.opacity < 100) chain.push(`colorchannelmixer=aa=${f4(x.opacity / 100)}`);
    parts.push(`${from}${chain.join(",")}[s${n}]`);
    // Placed by its middle, which is where it turns around too.
    parts.push(`[${last}][s${n}]overlay=x=${f4(w * (x.x + x.w / 2))}-w/2:y=${f4(h * (x.y + x.h / 2))}-h/2:eof_action=pass[o${n}]`);
    last = `o${n++}`;
  }
  return { inputs, parts, last };
}

/* ---------- what the picture is taken from, and the sound ---------- */

const KINDS = ["monitor", "game", "window"];
const kindOf = (c) => (KINDS.includes(c.s.source) ? c.s.source : "monitor");
const DISCORD = ["Discord.exe", "DiscordPTB.exe", "DiscordCanary.exe"];
let wins = null;      // the open windows, for the pickers (null: not asked yet)
let winsBusy = false;
let audioDevs = null; // { outputs, inputs }: the sound devices Windows has
function askWindows(c, again) {
  if (winsBusy || (wins && !again) || typeof c.windows !== "function") return;
  winsBusy = true;
  c.windows().then((list) => { wins = list || []; }).catch(() => { wins = wins || []; }).finally(() => { winsBusy = false; c.render(); });
}
function askAudioDevs(c) {
  if (audioDevs) return;
  audioDevs = { outputs: [], inputs: [] };
  c.invoke("audio_devices").then((d) => { if (d) { audioDevs = d; c.render(); } }).catch(() => {});
}
/** The sound's parts, put right: each { on, vol (percent) }, the devices by name ("" = the Windows default). */
function mixOf(c) {
  const g = c.glint.settings;
  const m = c.s.mix && typeof c.s.mix === "object" ? c.s.mix : (c.s.mix = {});
  const part = (key, on) => {
    const x = m[key] && typeof m[key] === "object" ? m[key] : (m[key] = { on });
    x.on = !!x.on; x.vol = clamp(Math.round(num(x.vol, 100)), 0, 150);
    return x;
  };
  // The first time: as Glint's own sound is set, which is what a stream used to send.
  part("desktop", c.s.audio !== false && g.desktop_audio !== false).device = String(m.desktop.device || "");
  part("mic", c.s.audio !== false && !!g.mic).device = String(m.mic.device || "");
  part("game", true); part("discord", true);
  const app = part("app", false);
  app.exe = String(app.exe || "");
  return m;
}
/** The sound as Glint's mixer takes it. The game and the programs only count while desktop audio is off: it holds them already. */
function audioSpec(c) {
  const m = mixOf(c), one = (x, on = x.on) => ({ on, gain: x.vol / 100 });
  const split = !m.desktop.on, apps = [];
  if (split && m.discord.on) for (const exe of DISCORD) apps.push({ exe, ...one(m.discord) });
  if (split && m.app.on && m.app.exe && !DISCORD.includes(m.app.exe)) apps.push({ exe: m.app.exe, ...one(m.app) });
  return { desktop: { ...one(m.desktop), device: m.desktop.device }, mic: { ...one(m.mic), device: m.mic.device }, game: one(m.game, split && m.game.on), apps };
}
/** The window picked for the picture, kept with its program and title so that it is found again another day. */
function syncWindow(c) {
  const w = (wins || []).find((x) => x.hwnd === c.s.window_hwnd);
  if (w) { c.s.window_exe = w.exe; c.s.window_title = w.title; }
}
function sourceSpec(c, raw) {
  const { w, h } = sizeOf(c);
  return { kind: kindOf(c), monitor: monitorOf(c).index, hwnd: +c.s.window_hwnd || 0, exe: String(c.s.window_exe || ""), title: String(c.s.window_title || ""),
    fps: [30, 60].includes(+c.s.fps) ? +c.s.fps : 60, cursor: c.glint.settings.cursor !== false, width: w, height: h, encoder: encoderOf(c), raw };
}

/** The stream for Glint to run: { send, feed, source, audio }, or null while the server or the key is missing. */
function buildLive(c) {
  const s = c.s;
  const url = target(s);
  if (!url) return null;
  const enc = encoderOf(c);
  const fps = [30, 60].includes(+s.fps) ? +s.fps : 60;
  const kbps = Math.min(51000, Math.max(500, +s.bitrate || 6000));
  const { w, h } = sizeOf(c);
  const scene = plain(c) ? null : sceneGraph(c, fps, w, h);
  // Every frame is stamped with the time since the stream began ("{t0}", filled in by Glint), so a
  // feeder started later carries on where the clock is and stays in step with the sound.
  const stamp = "settb=1/90000,setpts=(RTCTIME-{t0})/(TB*1000000)";
  const graph = scene ? `${scene.parts.join(";")};[${scene.last}]format=${enc === "x264" ? "yuv420p" : "nv12"},${stamp}[v]` : `{source},${stamp}[v]`;
  const rate = `${kbps}k`, buf = `${kbps * 2}k`;
  const feed = ["-hide_banner", "-loglevel", "error", "-nostats", "-nostdin"];
  if (scene) feed.push(...scene.inputs);
  feed.push("-filter_complex", graph, "-map", "[v]");
  if (enc === "nvenc") feed.push("-c:v", "h264_nvenc", "-preset", "p4", "-tune", "hq", "-bf", "0", "-rc", "cbr", "-b:v", rate, "-maxrate", rate, "-bufsize", buf, "-forced-idr", "1");
  else if (enc === "amf") feed.push("-c:v", "h264_amf", "-usage", "transcoding", "-quality", "speed", "-bf", "0", "-rc", "cbr", "-b:v", rate, "-maxrate", rate, "-bufsize", buf);
  else if (enc === "qsv") feed.push("-c:v", "h264_qsv", "-preset", "veryfast", "-bf", "0", "-b:v", rate, "-maxrate", rate, "-bufsize", buf);
  else feed.push("-c:v", "libx264", "-preset", "veryfast", "-tune", "zerolatency", "-pix_fmt", "yuv420p", "-b:v", rate, "-maxrate", rate, "-bufsize", buf);
  // A keyframe every two seconds, which is what streaming services ask for (by the clock: a feeder that starts late starts its own count).
  feed.push("-g", String(fps * 2), "-force_key_frames", "expr:if(isnan(prev_forced_t),1,gte(t,prev_forced_t+2))", "-fps_mode", "vfr");
  feed.push("-f", "mpegts", "-flush_packets", "1", "-muxdelay", "0", "-muxpreload", "0", "pipe:1");
  // The sender: the sound from Glint's mixer on stdin, the video from Glint ("{video}") copied as it is.
  const send = ["-hide_banner", "-loglevel", "error", "-stats", "-stats_period", "2", "-nostdin", "-copyts",
    "-f", "s16le", "-ar", "{audio_rate}", "-ac", "2", "-thread_queue_size", "1024", "-i", "pipe:0",
    "-fflags", "+discardcorrupt", "-probesize", "500000", "-analyzeduration", "500000", "-thread_queue_size", "1024", "-f", "mpegts", "-i", "{video}",
    "-map", "1:v", "-map", "0:a", "-c:v", "copy", "-c:a", "aac", "-b:a", "160k", "-ar", "48000", "-f", "flv", url];
  return { send, feed, source: sourceSpec(c, !!scene), audio: audioSpec(c) };
}
/** What a picture must share with the running stream's to take its place while live. */
const shapeKey = (c) => { const { w, h } = sizeOf(c); return [w, h, [30, 60].includes(+c.s.fps) ? +c.s.fps : 60, encoderOf(c), +c.s.bitrate].join(" "); };
/** What the keybind starts: the stream as the settings have it now. */
const savedProcs = (c) => { const spec = buildLive(c); return { [PROC]: spec ? { args: [], audio: false, live: spec } : { args: [], audio: false } }; };
/** While live: the sound follows at once; the picture's source and scene with a new capture (about a second of still picture). */
function pushLive(c) {
  if (!status.running || typeof c.proc.update !== "function") return;
  const spec = buildLive(c);
  if (!spec) return;
  const change = { audio: spec.audio };
  if (liveKey && liveKey === shapeKey(c)) { change.source = spec.source; change.feed = spec.feed; }
  c.proc.update(PROC, change).catch(() => {});
}

const uptime = () => {
  const s = Math.max(0, Math.floor((Date.now() - (status.started || Date.now())) / 1000));
  const p = (n) => String(n).padStart(2, "0");
  return `${Math.floor(s / 3600)}:${p(Math.floor(s / 60) % 60)}:${p(s % 60)}`;
};
/** ffmpeg's newest progress line as "60 fps · 5998 kbps", or "" before the first one. */
function stats() {
  const line = [...(status.log || [])].reverse().find((l) => l.startsWith("frame="));
  if (!line) return "";
  const fps = /fps=\s*([\d.]+)/.exec(line), br = /bitrate=\s*([\d.]+)kbits/.exec(line);
  return [fps && `${Math.round(+fps[1])} fps`, br && `${Math.round(+br[1])} kbps`].filter(Boolean).join(" · ");
}

/** Read the process's state again; the top bar follows, and the page too when the stream started or ended. */
async function refresh(c) {
  const was = status.running;
  try { status = await c.proc.status(PROC); } catch (e) { return; }
  if (status.running && !live) { live = shape(c); liveKey = liveKey || shapeKey(c); }
  if (!status.running) { live = null; liveKey = ""; }
  if (was !== status.running) c.render(); else c.chrome();
}
/** While live, look every two seconds: the stream can end by itself, or by its keybind. */
function watch(c) {
  clearInterval(poll);
  poll = setInterval(() => { if (!status.running) return clearInterval(poll); refresh(c); ytWatch(c); }, 2000);
}
/** What is being sent, as the top bar says it: "1080p @ 60fps". Taken when the stream starts. */
const shape = (c) => `${sizeOf(c).h}p @ ${[30, 60].includes(+c.s.fps) ? +c.s.fps : 60}fps`;

/** The top bar while live: an ordinary top bar button, "Streaming 1080p @ 60fps" (it opens the Stream page), joined to a stop button. */
function topbar(c) {
  if (!status.running) return "";
  const state = (status.live && status.live.state) || "";
  const sending = !!stats() && state !== "reconnecting";
  const tip = state === "reconnecting" ? t("The connection was lost. Connecting again…") : sending ? [t("Streaming for {time}", { time: uptime() }), stats(), serviceOf(c.s) === "youtube" && ytWatching()].filter(Boolean).join(" · ") : t("Connecting to the server…");
  const label = state === "reconnecting" ? t("Reconnecting…") : sending ? t("Streaming") : t("Connecting…");
  return `<span class="top-group${sending ? " live" : ""}"><button class="btn" data-pact="open" title="${esc(tip)}"><i class="top-dot${sending ? "" : " idle"}"></i><span class="top-label">${label}</span><span class="top-muted top-detail top-label">${esc(live || shape(c))}</span></button><button class="btn top-icon" data-pact="stop" title="${esc(t("Stop streaming"))}" aria-label="${esc(t("Stop streaming"))}"><svg viewBox="0 0 24 24"><rect x="6" y="6" width="12" height="12" rx="1.500"/></svg></button></span>`;
}

/** Where viewers watch: the Twitch channel of the account, on YouTube the live stream (or the channel's live page until YouTube shows it), else the link the user gave. */
function linkOf(c) {
  const acc = twAccount(c), yt = ytAccount(c);
  if (serviceOf(c.s) === "twitch" && acc && acc.login) return `https://www.twitch.tv/${acc.login}`;
  if (serviceOf(c.s) === "youtube" && yt && yt.id) { const v = ytLive.run === status.started && (ytLive.id || ytLive.gone); return v ? `https://www.youtube.com/watch?v=${encodeURIComponent(v)}` : `https://www.youtube.com/channel/${encodeURIComponent(yt.id)}/live`; }
  const link = String(c.s.link || "").trim();
  return /^https:\/\/\S+$/i.test(link) ? link : "";
}
/** The page's button: Go live. While live the stop button is the top bar's, and this is the stream's link. */
function headHtml(c) {
  if (status.running) return linkOf(c) ? `<button class="btn" data-pact="copy-link">${t("Copy stream link")}</button>` : "";
  const ready = !!buildLive(c);
  return `<button class="btn primary" data-pact="go"${ready ? "" : " disabled"} title="${esc(ready ? "" : t("Choose where to stream and paste your stream key first"))}">${t("Go live")}</button>`;
}
function statusHtml(c) {
  const lv = status.running ? status.live || {} : {};
  if (lv.state === "reconnecting") return `<div class="note bad"><b>${t("The connection was lost.")}</b> ${t("Glint is connecting again by itself.")} ${esc(lv.note || "")}</div>`;
  if (lv.state === "recovering") return `<div class="note">${t("The screen capture stopped and is starting again. The stream is still up.")}</div>`;
  if (status.running && lv.showing === "card") return `<div class="note">${kindOf(c) === "game" ? `${t("No game is on screen, so viewers see a black picture until it is back.")} ${t("A game in a window that cannot be captured by itself is not shown either: set it to fullscreen or borderless.")}` : t("The window is closed or minimized, so viewers see a black picture until it is back.")}</div>`;
  if (status.error) return `<div class="note bad"><b>${t("The stream stopped.")}</b> ${esc(status.error)}</div>`;
  if (ended && !status.running) return `<div class="note st-ended"><span><b>${t("The stream ended.")}</b> ${t("You were live for {time}.", { time: clock(ended.secs) })}</span>${ended.link ? `<button class="btn sm" data-pact="copy-ended">${t("Copy stream link")}</button>` : ""}</div>`;
  return "";
}
const clock = (s) => { const p = (n) => String(n).padStart(2, "0"); return `${Math.floor(s / 3600)}:${p(Math.floor(s / 60) % 60)}:${p(s % 60)}`; };
/** The stream is over: how long it was, and its link (on Twitch the recording, when the channel keeps them). */
function noteEnded(c, started) {
  const secs = Math.max(0, Math.floor((Date.now() - (started || Date.now())) / 1000));
  const mine = (ended = { secs, link: linkOf(c) });
  const acc = twAccount(c);
  if (serviceOf(c.s) !== "twitch" || !acc || !started) return;
  // Twitch lists the recording a little after the stream ends.
  setTimeout(async () => {
    try {
      const r = await twApi(c, "GET", `videos?user_id=${encodeURIComponent(acc.id)}&type=archive&first=1`);
      const v = r.data.data && r.data.data[0];
      if (ended === mine && v && v.url && Date.parse(v.created_at) > started - 120000) { mine.link = v.url; c.render(); }
    } catch (e) { /* the channel's link stays */ }
  }, 4000);
}

/* ---------- Twitch, without a stream key ----------
   The user signs in on twitch.tv (Twitch's device code flow: Glint shows a code, the user approves it in
   the browser, Glint asks Twitch until that is done) and Glint reads the channel's stream key itself.
   The client ID names Glint to Twitch; it is public by design and there is no client secret. What is kept
   in the plugin's settings: the access and refresh tokens, the account's name, and the key (so the
   keybind can start a stream while the window is closed). */
const TWITCH_CLIENT_ID = "o3o6kapmpm83ffraud9y773x9r75sg"; // Glint's app at dev.twitch.tv. Empty: the Stream page only offers the stream key.
const TWITCH_SCOPE = "channel:read:stream_key channel:manage:broadcast"; // the key, and the stream's title, category and tags
const twClient = (c) => (typeof c.http !== "function" ? "" : TWITCH_CLIENT_ID || String(c.s.twitch_client_id || "").trim()); // a Glint from before this cannot reach Twitch: the key box only
const twAccount = (c) => (c.s.twitch && c.s.twitch.refresh ? c.s.twitch : null);
let twSign = null; // signing in right now: { code, uri, timer }
let twError = "";

async function twCall(c, req) {
  const r = await c.http(req);
  let data = {};
  try { data = JSON.parse(r.body); } catch (e) { /* not JSON: only the status counts */ }
  return { status: r.status, data };
}
const twToken = (c, fields) => twCall(c, { method: "POST", url: "https://id.twitch.tv/oauth2/token", headers: ["Content-Type: application/x-www-form-urlencoded"], body: new URLSearchParams({ client_id: twClient(c), ...fields }).toString() });
const twGet = (c, path, access) => twCall(c, { url: `https://api.twitch.tv/helix/${path}`, headers: [`Authorization: Bearer ${access}`, `Client-Id: ${twClient(c)}`] });

/** With tokens in hand: whose they are and that channel's stream key, into the settings. `pick`: the user just signed in, so Twitch becomes the service. */
async function twLoad(c, tokens, pick) {
  const who = await twGet(c, "users", tokens.access_token);
  const user = who.data.data && who.data.data[0];
  if (who.status !== 200 || !user) throw new Error(t("Twitch did not say whose account this is"));
  const got = await twGet(c, `streams/key?broadcaster_id=${encodeURIComponent(user.id)}`, tokens.access_token);
  const key = got.data.data && got.data.data[0] && got.data.data[0].stream_key;
  if (got.status !== 200 || !key) throw new Error(t("Twitch did not give the stream key"));
  const scope = Array.isArray(tokens.scope) ? tokens.scope.join(" ") : (c.s.twitch && c.s.twitch.id === user.id && c.s.twitch.scope) || "";
  c.s.twitch = { access: tokens.access_token, refresh: tokens.refresh_token, id: user.id, login: user.login, name: user.display_name || user.login, scope, key };
  if (pick) c.s.service = "twitch"; // the user just signed in; a renewed token leaves the chosen service alone
  if (serviceOf(c.s) === "twitch") c.s.key = key;
}
function twStop() { if (twSign) clearTimeout(twSign.timer); twSign = null; }

/* The stream's title, category and tags, set from Glint (Twitch calls this the channel's information; it has
   no public / private switch). Needs the "channel:manage:broadcast" permission: an account connected
   before Glint asked for it is asked to connect once more. */
const twCanManage = (c) => { const acc = twAccount(c); return !!acc && /channel:manage:broadcast/.test(acc.scope || ""); };
let twInfo = { busy: false, error: "", done: 0, loaded: false };
/** A call to Twitch's API as the account. A token that ran out is renewed once and the call made again. */
async function twApi(c, method, path, body) {
  const call = () => twCall(c, { method, url: `https://api.twitch.tv/helix/${path}`, headers: [`Authorization: Bearer ${twAccount(c).access}`, `Client-Id: ${twClient(c)}`, ...(body ? ["Content-Type: application/json"] : [])], body: body ? JSON.stringify(body) : undefined });
  let r = await call();
  if (r.status === 401 && twAccount(c)) {
    const fresh = await twToken(c, { grant_type: "refresh_token", refresh_token: twAccount(c).refresh });
    if (fresh.status === 200 && fresh.data.access_token) { await twLoad(c, fresh.data); c.save(); r = await call(); }
  }
  return r;
}
const twTags = (text) => String(text || "").split(",").map((x) => x.replace(/[^\p{L}\p{N}]/gu, "").slice(0, 25)).filter(Boolean).slice(0, 10);
/** What the channel says now, into the boxes (once per run of Glint, and never over something typed). */
async function twReadInfo(c) {
  if (twInfo.loaded || !twCanManage(c)) return;
  twInfo.loaded = true;
  try {
    const r = await twApi(c, "GET", `channels?broadcaster_id=${encodeURIComponent(twAccount(c).id)}`);
    const ch = r.data.data && r.data.data[0];
    if (r.status !== 200 || !ch) return;
    if (!c.s.tw_title) c.s.tw_title = ch.title || "";
    if (!c.s.tw_category) c.s.tw_category = ch.game_name || "";
    if (!c.s.tw_tags) c.s.tw_tags = (ch.tags || []).join(", ");
    c.s.tw_sent = JSON.stringify([ch.title || "", ch.game_name || "", (ch.tags || []).join(", ")]);
    c.save(); c.render();
  } catch (e) { /* offline: the boxes stay as they are */ }
}
/** Send the title, category and tags to Twitch. `quiet`: on going live, and only when something changed. */
async function twSendInfo(c, quiet) {
  if (!twCanManage(c) || twInfo.busy) return;
  const title = String(c.s.tw_title || "").trim().slice(0, 140), category = String(c.s.tw_category || "").trim(), tags = twTags(c.s.tw_tags);
  const now = JSON.stringify([title, category, tags.join(", ")]);
  if (quiet && (now === c.s.tw_sent || !title)) return;
  twInfo = { ...twInfo, busy: true, error: "" };
  if (!quiet) c.render();
  try {
    if (!title) throw new Error(t("Give the stream a title first"));
    const body = { title, tags };
    if (category) {
      // The category by its exact name, else the first one Twitch finds for the words.
      let g = await twApi(c, "GET", `games?name=${encodeURIComponent(category)}`);
      let hit = g.data.data && g.data.data[0];
      if (!hit) { g = await twApi(c, "GET", `search/categories?first=1&query=${encodeURIComponent(category)}`); hit = g.data.data && g.data.data[0]; }
      if (!hit) throw new Error(t("Twitch has no category called {name}", { name: category }));
      body.game_id = hit.id; c.s.tw_category = hit.name;
    } else body.game_id = "";
    const r = await twApi(c, "PATCH", `channels?broadcaster_id=${encodeURIComponent(twAccount(c).id)}`, body);
    if (r.status !== 204 && r.status !== 200) throw new Error(r.data.message || t("Twitch did not take the change"));
    c.s.tw_title = title; c.s.tw_tags = tags.join(", ");
    c.s.tw_sent = JSON.stringify([title, c.s.tw_category, tags.join(", ")]);
    twInfo = { ...twInfo, busy: false, done: Date.now() };
    c.save();
  } catch (e) { twInfo = { ...twInfo, busy: false, error: String(e.message || e) }; }
  c.render();
}
function infoRows(c) {
  if (serviceOf(c.s) === "youtube" && ytAccount(c) && ytOn(c)) return ytInfoRows(c);
  if (serviceOf(c.s) !== "twitch" || !twAccount(c)) return "";
  if (!twCanManage(c)) return c.row(t("Title and category"), t("To set your stream's title, category and tags from Glint, connect Twitch once more: Glint asks for that permission now"), `<button class="btn sm" data-pact="tw-on">${t("Connect again")}</button>`);
  twReadInfo(c);
  const hint = twInfo.error ? `${t("Twitch did not take the change:")} ${esc(twInfo.error)}` : Date.now() - twInfo.done < 6000 ? t("Updated on Twitch") : c.s.tw_title && JSON.stringify([String(c.s.tw_title).trim(), String(c.s.tw_category || "").trim(), twTags(c.s.tw_tags).join(", ")]) !== c.s.tw_sent ? t("Not on Twitch yet. It is also sent when you go live") : t("Works while you are live too");
  return c.row(t("Title"), t("What viewers see above your stream"), c.text("tw_title", { placeholder: t("Stream title"), width: 340 }))
    + c.row(t("Category"), t("The game or kind of stream, by its name on Twitch"), c.text("tw_category", { placeholder: "Just Chatting", width: 260 }))
    + c.row(t("Tags"), t("Up to 10, with commas between them"), c.text("tw_tags", { placeholder: "English, FPS", width: 340 }))
    + c.row(t("Update on Twitch"), hint, `<button class="btn sm${twInfo.busy ? "" : " primary"}" data-pact="tw-info"${twInfo.busy ? " disabled" : ""}>${twInfo.busy ? t("Updating…") : t("Update")}</button>`);
}
/** Start signing in: ask Twitch for a code, open its page, and ask every few seconds whether the user approved. */
async function twConnect(c) {
  twStop(); twError = "";
  try {
    const dev = await twCall(c, { method: "POST", url: "https://id.twitch.tv/oauth2/device", headers: ["Content-Type: application/x-www-form-urlencoded"], body: new URLSearchParams({ client_id: twClient(c), scopes: TWITCH_SCOPE }).toString() });
    if (dev.status !== 200 || !dev.data.device_code) throw new Error(dev.data.message || t("Twitch did not answer"));
    const sign = (twSign = { code: dev.data.user_code, uri: dev.data.verification_uri, timer: 0 });
    const every = Math.max(1, +dev.data.interval || 5) * 1000, until = Date.now() + Math.max(60, +dev.data.expires_in || 900) * 1000;
    const again = () => { sign.timer = setTimeout(ask, every); };
    const ask = async () => {
      if (twSign !== sign) return;
      if (Date.now() > until) { twStop(); twError = t("The code ran out before it was approved. Try again."); return c.render(); }
      try {
        const r = await twToken(c, { scopes: TWITCH_SCOPE, device_code: dev.data.device_code, grant_type: "urn:ietf:params:oauth:grant-type:device_code" });
        if (twSign !== sign) return;
        if (r.status === 200 && r.data.access_token) { await twLoad(c, r.data, true); twStop(); lastService = serviceOf(c.s); await c.saveNow(); return c.render(); }
        if (/pending|slow_down/i.test(r.data.message || "")) return again();
        throw new Error(r.data.message || t("Twitch refused the sign-in"));
      } catch (e) { if (twSign !== sign) return; twStop(); twError = String(e.message || e); c.render(); }
    };
    again();
    c.render();
    c.open(sign.uri).catch(() => {});
  } catch (e) { twError = String(e.message || e); c.render(); }
}
/** When Glint opens: a fresh token, and the key again in case it was reset on Twitch. A refusal means signing in again. */
async function twRenew(c) {
  const acc = twAccount(c);
  if (!acc || !twClient(c)) return;
  try {
    const r = await twToken(c, { grant_type: "refresh_token", refresh_token: acc.refresh });
    if (r.status === 200 && r.data.access_token) { await twLoad(c, r.data); c.save(); }
    else if (r.status === 400 || r.status === 401) { c.s.twitch = { ...acc, refresh: "", access: "", expired: true }; c.save(); }
    else return; // Twitch could not be reached: what is saved stays
    c.render();
  } catch (e) { /* offline: the saved key still works */ }
}
function twRows(c) {
  if (!twClient(c)) return "";
  const acc = twAccount(c);
  if (twSign) return c.row(t("Approve Glint on Twitch"), t("Your browser opened twitch.tv. Check that it shows this code, then press Authorize there"),
    `<code class="st-code">${esc(twSign.code)}</code><button class="btn sm" data-pact="tw-open">${t("Open Twitch")}</button><button class="btn sm" data-pact="tw-cancel">${t("Cancel")}</button>`);
  if (acc) return c.row(t("Twitch account"), t("Signed in as {name}. Glint reads your stream key from Twitch", { name: acc.name }), `<button class="btn sm" data-pact="tw-off">${t("Disconnect")}</button>`);
  const hint = twError ? `${t("Could not sign in:")} ${twError}` : c.s.twitch && c.s.twitch.expired ? t("Twitch asks you to sign in again") : t("Sign in and Glint gets your stream key by itself. Or paste the key below");
  return c.row(t("Twitch account"), hint, `<button class="btn sm primary" data-pact="tw-on">${t("Connect Twitch")}</button>`);
}

/* ---------- YouTube, without a stream key ----------
   The user signs in on google.com/device (Google's device code flow, as with Twitch above) and Glint reads
   the channel's stream key itself. Google wants the OAuth client's secret when a code is turned into
   tokens and when a token is renewed, and a secret cannot live in a plugin: those two calls go to Glint's
   server (server/share, POST /yt/token), which adds it and passes Google's answer back. Everything else
   goes straight to Google with the user's token. Kept in the plugin's settings: the tokens, the channel's
   name and its key.
   YouTube counts every API call against one daily allowance for all of Glint's users (10,000 units; a
   read is 1, a change is 50, going live is 100), so this asks little: the live stream is looked at once a minute. */
const YT_CLIENT_ID = "1002324857453-dbgq5r5rqdsfrporkkn9cdaj9ttoq4l8.apps.googleusercontent.com"; // Glint's OAuth client (type "TVs and Limited Input devices"). Public. Empty: the Stream page only offers the stream key.
const YT_SCOPE = "https://www.googleapis.com/auth/youtube"; // the channel, its stream key, and the live stream's title
const YT_TOKENS = "https://clips.glintclip.com/yt/token";
const YT_API = "https://www.googleapis.com/youtube/v3/";
const YT_FORM = ["Content-Type: application/x-www-form-urlencoded"];
const ytOn = (c) => typeof c.http === "function" && !!YT_CLIENT_ID;
const ytAccount = (c) => (c.s.youtube && c.s.youtube.refresh ? c.s.youtube : null);
let ytSign = null; // signing in right now: { code, uri, timer }
let ytError = "";
let ytLive = { run: 0, id: "", title: "", viewers: null, at: 0, tries: 0, busy: false, error: "" }; // the live stream as YouTube has it
let ytInfo = { busy: false, error: "", done: 0 };

/** Why Google or YouTube said no, in their words. */
const ytWhy = (r, fallback) => (r.data.error && typeof r.data.error === "object" ? r.data.error.message : r.data.error_description || (typeof r.data.error === "string" ? r.data.error : "")) || fallback;
const ytTokens = (c, fields) => twCall(c, { method: "POST", url: YT_TOKENS, headers: ["Content-Type: application/json"], body: JSON.stringify(fields) });
const ytUntil = (tokens) => Date.now() + (Math.max(120, +tokens.expires_in || 3600) - 60) * 1000;
function ytStop() { if (ytSign) clearTimeout(ytSign.timer); ytSign = null; const pop = document.getElementById("st-pop"); if (pop) pop.remove(); }

/** A fresh access token (Google's last an hour). A refusal means signing in again. */
async function ytFresh(c) {
  const acc = ytAccount(c);
  if (!acc) return false;
  const r = await ytTokens(c, { refresh_token: acc.refresh });
  if (r.status === 200 && r.data.access_token) { c.s.youtube = { ...acc, access: r.data.access_token, until: ytUntil(r.data) }; c.save(); return true; }
  if (r.data.error === "invalid_grant") { c.s.youtube = { ...acc, refresh: "", access: "", expired: true }; c.save(); c.render(); }
  return false;
}
/** A call to YouTube's API as the account. A token that ran out is renewed first, or once after a refusal. */
async function ytApi(c, method, path, body) {
  if (!ytAccount(c) || (!(ytAccount(c).until > Date.now()) && !(await ytFresh(c)))) return { status: 401, data: {} };
  const call = () => twCall(c, { method, url: YT_API + path, headers: [`Authorization: Bearer ${ytAccount(c).access}`, ...(body ? ["Content-Type: application/json"] : [])], body: body ? JSON.stringify(body) : undefined });
  let r = await call();
  if (r.status === 401 && (await ytFresh(c))) r = await call();
  return r;
}
/** Whose account this is and that channel's stream key, into the settings. `pick`: the user just signed in, so YouTube becomes the service. */
async function ytLoad(c, pick) {
  const who = await ytApi(c, "GET", "channels?part=snippet&mine=true");
  if (who.status !== 200) throw new Error(ytWhy(who, t("YouTube did not say whose channel this is")));
  const ch = who.data.items && who.data.items[0];
  if (!ch) throw new Error(t("This Google account has no YouTube channel"));
  // The channel's keys: the one YouTube made by itself ("Default stream key") when there is one, else the first that can be used again.
  const got = await ytApi(c, "GET", "liveStreams?part=snippet,cdn,contentDetails&mine=true&maxResults=50");
  const keys = ((got.status === 200 && got.data.items) || []).filter((x) => x.cdn && x.cdn.ingestionType === "rtmp" && x.cdn.ingestionInfo && x.cdn.ingestionInfo.streamName);
  let one = keys.find((x) => /default/i.test((x.snippet && x.snippet.title) || "")) || keys.find((x) => x.contentDetails && x.contentDetails.isReusable) || keys[0];
  // A channel that never streamed has no key yet: Glint makes one, so nothing has to be set up on YouTube first.
  if (!one && got.status === 200) {
    const made = await ytApi(c, "POST", "liveStreams?part=snippet,cdn,contentDetails", { snippet: { title: "Glint" }, cdn: { ingestionType: "rtmp", resolution: "variable", frameRate: "variable" }, contentDetails: { isReusable: true } });
    if (made.status === 200 && made.data.cdn && made.data.cdn.ingestionInfo && made.data.cdn.ingestionInfo.streamName) one = made.data;
  }
  const key = one ? String(one.cdn.ingestionInfo.streamName) : "";
  const old = c.s.youtube.key || "";
  c.s.youtube = { ...c.s.youtube, id: ch.id, name: (ch.snippet && ch.snippet.title) || ch.id, key, stream: one ? one.id : "", read: Date.now() };
  if (pick) c.s.service = "youtube";
  if (key && serviceOf(c.s) === "youtube") c.s.key = key;
  else if (!key && old && c.s.key === old) c.s.key = "";
}
/** Start signing in: ask Google for a code, open its page, and ask every few seconds whether the user approved. */
async function ytConnect(c) {
  ytStop(); ytError = "";
  ytPopup(c);
  try {
    // Glint's server first: without it the code could be approved and still lead nowhere.
    const up = await ytTokens(c, {});
    if (!document.getElementById("st-pop")) return;
    if (up.data.error !== "invalid_request") throw new Error(t("YouTube sign-in is not set up on Glint's server yet"));
    const dev = await twCall(c, { method: "POST", url: "https://oauth2.googleapis.com/device/code", headers: YT_FORM, body: new URLSearchParams({ client_id: YT_CLIENT_ID, scope: YT_SCOPE }).toString() });
    if (dev.status !== 200 || !dev.data.device_code) throw new Error(ytWhy(dev, t("Google did not answer")));
    if (!document.getElementById("st-pop")) return; // cancelled while the code was on its way
    const sign = (ytSign = { code: dev.data.user_code, uri: dev.data.verification_url || "https://www.google.com/device", timer: 0 });
    let every = Math.max(1, +dev.data.interval || 5) * 1000;
    const until = Date.now() + Math.max(60, +dev.data.expires_in || 1800) * 1000;
    const again = () => { sign.timer = setTimeout(ask, every); };
    const ask = async () => {
      if (ytSign !== sign) return;
      if (Date.now() > until) { ytStop(); ytError = t("The code ran out before it was approved. Try again."); return c.render(); }
      const before = c.s.youtube;
      try {
        const r = await ytTokens(c, { device_code: dev.data.device_code });
        if (ytSign !== sign) return;
        if (r.status === 200 && r.data.access_token && r.data.refresh_token) {
          c.s.youtube = { access: r.data.access_token, until: ytUntil(r.data), refresh: r.data.refresh_token, id: "", name: "", key: "" };
          await ytLoad(c, true);
          ytStop(); lastService = serviceOf(c.s); await c.saveNow(); return c.render();
        }
        if (r.data.error === "authorization_pending") return again();
        if (r.data.error === "slow_down") { every += 5000; return again(); }
        throw new Error(r.data.error === "access_denied" ? t("The sign-in was declined on Google") : r.data.error === "expired_token" ? t("The code ran out before it was approved. Try again.") : ytWhy(r, t("Google refused the sign-in")));
      } catch (e) { if (ytSign !== sign) return; c.s.youtube = before; ytStop(); ytError = String(e.message || e); c.render(); }
    };
    again();
    c.render();
    ytPopup(c); // the code is here: the popup shows it. The page is opened by its button, not by itself.
  } catch (e) { ytStop(); ytError = String(e.message || e); c.render(); }
}
/* The sign-in's popup: the steps, the code, and a button that opens Google's page. It is up from the
   press on "Connect YouTube" until the sign-in is done, failed or cancelled (ytStop takes it down). */
function ytPopup(c) {
  let el = document.getElementById("st-pop");
  if (!el) {
    el = document.createElement("div");
    el.id = "st-pop"; el.className = "st-pop";
    document.body.append(el);
    el.addEventListener("click", (e) => {
      const act = e.target.closest("[data-yp]") ? e.target.closest("[data-yp]").dataset.yp : e.target === el ? "cancel" : "";
      if (act === "open" && ytSign) c.open(ytSign.uri).catch(c.fail);
      if (act === "copy" && ytSign) (typeof c.copy === "function" ? c.copy(ytSign.code) : navigator.clipboard.writeText(ytSign.code)).then(() => c.flash(t("Code copied"))).catch(c.fail);
      if (act === "cancel") { ytStop(); c.render(); }
    });
    el.addEventListener("keydown", (e) => { if (e.key === "Escape") { ytStop(); c.render(); } });
  }
  const code = ytSign ? ytSign.code : "";
  el.innerHTML = `<div class="confirm-box st-signin" role="dialog" aria-modal="true" aria-label="${esc(t("Connect YouTube"))}" tabindex="-1">
    <h3>${t("Connect YouTube")}</h3>
    <ol>
      <li>${t("Press <b>Open page in new tab</b> below. Google's sign-in page opens in your browser.")}</li>
      <li>${t("Type in this code:")} ${code ? `<code class="st-code">${esc(code)}</code><button class="btn sm" data-yp="copy">${t("Copy")}</button>` : `<span class="st-wait">${t("Getting a code…")}</span>`}</li>
      <li>${t("Pick your YouTube channel and press <b>Allow</b>.")}</li>
      <li>${t("Come back to Glint. It connects by itself.")}</li>
    </ol>
    <div class="confirm-row"><span class="st-wait">${code ? t("Waiting for you to finish on Google…") : ""}</span><button class="btn" data-yp="cancel">${t("Cancel")}</button><button class="btn primary" data-yp="open"${code ? "" : " disabled"}>${t("Open page in new tab")}</button></div>
  </div>`;
  const box = el.firstElementChild;
  if (box && !el.contains(document.activeElement)) box.focus();
}
/** Sign out: Google forgets Glint's permission (as well as it can be told), and so does Glint. */
function ytOff(c) {
  ytStop();
  const acc = c.s.youtube;
  if (acc && (acc.refresh || acc.access)) twCall(c, { method: "POST", url: "https://oauth2.googleapis.com/revoke", headers: YT_FORM, body: new URLSearchParams({ token: acc.refresh || acc.access }).toString() }).catch(() => {});
  if (acc && acc.key && c.s.key === acc.key) c.s.key = "";
  c.s.youtube = null;
  ytLive = { ...ytLive, id: "", viewers: null, error: "" };
  c.save(); c.render();
}
/** When Glint opens: a fresh token, and once a day the key again in case it was reset on YouTube. */
async function ytRenew(c) {
  if (!ytAccount(c) || !ytOn(c)) return;
  try {
    if (!(await ytFresh(c)) || Date.now() - (+ytAccount(c).read || 0) < 86400000) return;
    await ytLoad(c, false);
    c.save(); c.render();
  } catch (e) { /* offline: the saved key still works */ }
}
function ytRows(c) {
  if (!ytOn(c)) return "";
  const acc = ytAccount(c);
  if (ytSign) return c.row(t("Approve Glint on Google"), t("Enter this code on {page}, then pick your YouTube channel", { page: esc(ytSign.uri.replace(/^https:\/\/(www\.)?/, "")) }),
    `<code class="st-code">${esc(ytSign.code)}</code><button class="btn sm" data-pact="yt-open">${t("Open page in new tab")}</button><button class="btn sm" data-pact="yt-cancel">${t("Cancel")}</button>`);
  if (acc) return c.row(t("YouTube account"), acc.key ? t("Signed in as: {name}", { name: esc(acc.name) }) : t("Signed in as {name}. Live streaming is not switched on for this channel: switch it on in YouTube Studio (it can take a day), then connect again. Or paste a stream key below", { name: esc(acc.name) }),
    `<button class="btn sm" data-pact="yt-off">${t("Disconnect")}</button>`);
  const hint = ytError ? `${t("Could not sign in:")} ${esc(ytError)}` : c.s.youtube && c.s.youtube.expired ? t("YouTube asks you to sign in again") : t("Sign in and Glint gets your stream key by itself. Or paste the key below");
  return c.row(t("YouTube account"), hint, `<button class="btn sm primary" data-pact="yt-on">${t("Connect YouTube")}</button>`);
}

/* The live stream on YouTube: whether it is up, how many are watching, its title. YouTube starts a
   broadcast by itself when the stream arrives on the channel's key; Glint finds it (asking every 20
   seconds for the first two minutes, then once a minute) and from then on reads it once a minute. */
async function ytWatch(c) {
  if (!status.running || serviceOf(c.s) !== "youtube" || !ytAccount(c) || !ytOn(c)) return;
  if (ytLive.run !== status.started) ytLive = { run: status.started, id: "", title: "", viewers: null, at: Date.now(), tries: 0, busy: false, error: "" };
  const mine = ytLive;
  if (mine.busy || Date.now() - mine.at < (mine.id || mine.tries >= 6 ? 60000 : 20000)) return;
  mine.busy = true; mine.at = Date.now();
  try {
    if (!mine.id) {
      mine.tries++;
      const r = await ytApi(c, "GET", "liveBroadcasts?part=snippet,status&broadcastStatus=active&broadcastType=all&maxResults=5");
      const b = r.status === 200 && r.data.items && r.data.items[0];
      mine.error = r.status === 200 || r.status === 401 ? "" : ytWhy(r, "");
      if (b) {
        mine.id = b.id; mine.title = (b.snippet && b.snippet.title) || "";
        // The title box follows YouTube while it is empty; a title typed in it goes to YouTube.
        if (!String(c.s.yt_title || "").trim()) { c.s.yt_title = mine.title; if (!String(c.s.yt_desc || "").trim()) c.s.yt_desc = (b.snippet && b.snippet.description) || ""; c.save(); if (ytLive === mine) c.render(); }
        if (ytLive === mine) ytSendInfo(c, true);
      }
    } else {
      const r = await ytApi(c, "GET", `videos?part=snippet,liveStreamingDetails&id=${encodeURIComponent(mine.id)}`);
      if (r.status === 200) {
        const v = r.data.items && r.data.items[0], d = (v && v.liveStreamingDetails) || {};
        mine.error = "";
        if (!v || d.actualEndTime) { mine.gone = mine.id; mine.id = ""; mine.viewers = null; mine.tries = 0; }
        else { mine.viewers = d.concurrentViewers == null ? null : Math.max(0, +d.concurrentViewers || 0); mine.title = (v.snippet && v.snippet.title) || mine.title; }
      } else if (r.status !== 401) mine.error = ytWhy(r, "");
    }
  } catch (e) { /* offline: asked again next time */ }
  mine.busy = false;
  if (ytLive !== mine) return;
  const el = document.querySelector(".st-yt");
  if (el) el.outerHTML = ytLiveHtml(c);
  c.chrome();
}
const ytWatching = () => (status.running && ytLive.id && ytLive.viewers != null ? t("{n} watching", { n: ytLive.viewers }) : "");
/** The Stream page's line while live on YouTube. */
function ytLiveHtml(c) {
  if (!status.running || serviceOf(c.s) !== "youtube" || !ytAccount(c) || !ytOn(c)) return "";
  if (ytLive.run === status.started && ytLive.id) return `<div class="note st-yt"><b>${t("Live on YouTube.")}</b> ${[ytWatching(), esc(ytLive.title)].filter(Boolean).join(" · ")}</div>`;
  if (ytLive.run === status.started && ytLive.error) return `<div class="note st-yt">${t("YouTube did not say how the stream is doing:")} ${esc(ytLive.error)}</div>`;
  return `<div class="note st-yt">${t("Waiting for YouTube to show the stream…")}</div>`;
}
/* The stream's title, description and who can see it. With an account, Glint makes the broadcast on
   YouTube itself when you go live (so it is private or unlisted from its first second, not after),
   ties it to the channel's key and lets YouTube start and end it with the stream. A stream that Glint
   did not make (started by the keybind while the window was closed) is found a little later and
   brought in line. */
const YT_SEE = ["public", "unlisted", "private"];
const ytWant = (c) => ({ title: String(c.s.yt_title || "").replace(/[<>]/g, "").trim().slice(0, 100), desc: String(c.s.yt_desc || "").replace(/[<>]/g, "").trim().slice(0, 5000), see: YT_SEE.includes(c.s.yt_privacy) ? c.s.yt_privacy : "public" });
const ytSentKey = (w) => JSON.stringify([w.title, w.desc, w.see]);
/** Make the broadcast for the stream that is about to start: its id. Throws with YouTube's reason. */
async function ytCreate(c) {
  const acc = ytAccount(c), w = ytWant(c);
  const title = w.title || t("{name} live", { name: acc.name });
  const made = await ytApi(c, "POST", "liveBroadcasts?part=snippet,status,contentDetails", { snippet: { title, description: w.desc, scheduledStartTime: new Date().toISOString() }, status: { privacyStatus: w.see }, contentDetails: { enableAutoStart: true, enableAutoStop: true } });
  if (made.status !== 200 || !made.data.id) throw new Error(ytWhy(made, t("YouTube did not answer")));
  const tied = await ytApi(c, "POST", `liveBroadcasts/bind?part=id&id=${encodeURIComponent(made.data.id)}&streamId=${encodeURIComponent(acc.stream)}`);
  if (tied.status !== 200) { ytDrop(c, made.data.id); throw new Error(ytWhy(tied, t("YouTube did not answer"))); }
  c.s.yt_sent = ytSentKey({ ...w, title });
  return { id: made.data.id, title };
}
/** A broadcast that was made and then not used. */
const ytDrop = (c, id) => { ytApi(c, "DELETE", `liveBroadcasts?id=${encodeURIComponent(id)}`).catch(() => {}); };
/** Send the title, description and visibility to the live stream. `quiet`: when a stream is found, and only when something differs. */
async function ytSendInfo(c, quiet) {
  const w = ytWant(c), id = ytLive.id;
  if (ytInfo.busy || !ytAccount(c) || !status.running || !id) return;
  if (quiet && (!w.title || ytSentKey(w) === c.s.yt_sent)) return;
  ytInfo = { ...ytInfo, busy: true, error: "" };
  if (!quiet) c.render();
  try {
    if (!w.title) throw new Error(t("Give the stream a title first"));
    // YouTube replaces these parts whole, so what it has now is sent back with the changes.
    const cur = await ytApi(c, "GET", `liveBroadcasts?part=snippet,status&id=${encodeURIComponent(id)}`);
    const it = cur.data.items && cur.data.items[0], sn = it && it.snippet;
    if (cur.status !== 200 || !sn) throw new Error(ytWhy(cur, t("YouTube did not take the change")));
    const snippet = { title: w.title, description: w.desc, scheduledStartTime: sn.scheduledStartTime || sn.actualStartTime || new Date().toISOString() };
    if (sn.scheduledEndTime) snippet.scheduledEndTime = sn.scheduledEndTime;
    const st = { privacyStatus: w.see };
    if (it.status && typeof it.status.selfDeclaredMadeForKids === "boolean") st.selfDeclaredMadeForKids = it.status.selfDeclaredMadeForKids;
    const r = await ytApi(c, "PUT", "liveBroadcasts?part=snippet,status", { id, snippet, status: st });
    if (r.status !== 200) throw new Error(ytWhy(r, t("YouTube did not take the change")));
    c.s.yt_title = w.title; c.s.yt_desc = w.desc; c.s.yt_privacy = w.see; c.s.yt_sent = ytSentKey(w); ytLive.title = w.title;
    ytInfo = { busy: false, error: "", done: Date.now() };
    c.save();
  } catch (e) { ytInfo = { ...ytInfo, busy: false, error: String(e.message || e) }; }
  c.render();
}
function ytInfoRows(c) {
  const found = status.running && ytLive.run === status.started && !!ytLive.id;
  if (!YT_SEE.includes(c.s.yt_privacy)) c.s.yt_privacy = "public";
  const changed = found && ytSentKey(ytWant(c)) !== c.s.yt_sent;
  const hint = ytInfo.error ? `${t("YouTube did not take the change:")} ${esc(ytInfo.error)}` : Date.now() - ytInfo.done < 6000 ? t("Updated on YouTube") : changed ? t("Not on YouTube yet") : found ? t("Changes the stream that is live now") : t("Used when you go live. While live, Update sends changes");
  return c.row(t("Stream Title"), "", c.text("yt_title", { placeholder: t("Stream title"), width: 340 }))
    + c.row(t("Description"), "", `<textarea class="field st-desc" data-set="${c.key("yt_desc")}" maxlength="5000" rows="3" spellcheck="false" placeholder="${esc(t("Optional"))}">${esc(c.s.yt_desc || "")}</textarea>`)
    + c.row(t("Visibility"), c.s.yt_privacy === "private" ? t("Only you, and people you invite on YouTube") : c.s.yt_privacy === "unlisted" ? t("Anyone with the link. It is not listed on your channel") : t("Everyone. It is listed on your channel"), c.select("yt_privacy", [["public", t("Public")], ["unlisted", t("Unlisted")], ["private", t("Private")]]))
    + c.row(t("Update on YouTube"), hint, `<button class="btn sm${ytInfo.busy || !found ? "" : " primary"}" data-pact="yt-info"${ytInfo.busy || !found ? " disabled" : ""}>${ytInfo.busy ? t("Updating…") : t("Update")}</button>`);
}

/* The stream key box holds one key. An account's key is kept with the account too, so choosing another
   service brings that service's own key (or empties the box of a key that belongs to the other one). */
let lastService = "";
function syncKey(c) {
  const now = serviceOf(c.s), accOf = (sv) => (sv === "twitch" ? twAccount(c) : sv === "youtube" ? ytAccount(c) : null);
  if (lastService && lastService !== now) {
    const was = accOf(lastService), is = accOf(now);
    if (is && is.key) c.s.key = is.key;
    else if (was && was.key && c.s.key === was.key) c.s.key = "";
  }
  lastService = now;
}

/** The settings, as sections: on the Stream page and, the same ones, on the Settings page. */
function whereRows(c) {
  const s = c.s, service = serviceOf(s);
  const account = service === "twitch" ? twAccount(c) : service === "youtube" && ytOn(c) ? ytAccount(c) : null;
  const keyed = !!account && (service === "twitch" || !!account.key); // the account gave the key: no box for it
  return c.row(t("Service"), t("Other services work with Custom server: use the server address they give you"), c.select("service", [["twitch", "Twitch"], ["youtube", "YouTube"], ["custom", t("Custom server")]]))
    + (service === "custom" ? c.row(t("Server address"), t("Starts with rtmp:// or rtmps://"), c.text("url", { placeholder: "rtmp://", width: 340 })) : "")
    + (service === "twitch" ? twRows(c) : service === "youtube" ? ytRows(c) : "")
    + (keyed ? "" : c.row(t("Stream key"), t("From your channel's dashboard. Anyone who has it can stream as you. It is saved on this PC, in Glint's settings file"),
      `${c.text("key", { password: !showKey, width: 260 })}<button class="btn sm" data-pact="showkey">${showKey ? t("Hide") : t("Show")}</button>`))
    + (account ? "" : c.row(t("Your stream's link"), t("Optional. Where viewers watch you: Glint offers it to copy while you are live and after"), c.text("link", { placeholder: "https://", width: 340 })))
    + infoRows(c);
}
/* What viewers see is asked once, when the stream has somewhere to go (a key was saved or an account
   connected): a whole screen shows everything on it, which is rarely what someone means to put out.
   The answer is the "What to show" setting below; this only makes sure it was a choice. */
let pickWin = false;  // the card is on its second step: which window
let lastSource = "";  // the "What to show" setting as last seen: a change made by hand is a choice too
const chosen = (c) => !!c.s.source_picked || kindOf(c) !== "monitor";
function pickHtml(c) {
  if (status.running || !target(c.s) || (chosen(c) && !pickWin)) return "";
  const choice = (act, title, hint, main) => `<button class="st-choice${main ? " main" : ""}" data-pact="${act}"><b>${title}</b><span>${hint}</span></button>`;
  if (pickWin) {
    askWindows(c);
    const list = (wins || []).filter((x) => !x.excluded);
    return `<div class="section st-pick"><h2>${t("Which window?")}<span><button class="btn sm" data-pact="wins">${t("Refresh")}</button><button class="btn sm" data-pact="pick-back">${t("Back")}</button></span></h2>
      <div class="st-wins">${!wins ? `<div class="st-none">${t("Looking for windows…")}</div>` : list.length ? list.map((x) => `<button class="st-win" data-pact="pick-win" data-hwnd="${x.hwnd}"><b>${esc(x.title.slice(0, 80))}</b><span>${esc(x.exe)}${x.minimized ? ` · ${t("minimized")}` : ""}</span></button>`).join("") : `<div class="st-none">${t("No window is open. Open the game or program, then press Refresh.")}</div>`}</div></div>`;
  }
  const account = serviceOf(c.s) === "twitch" ? twAccount(c) : serviceOf(c.s) === "youtube" && ytOn(c) ? ytAccount(c) : null;
  return `<div class="section st-pick"><h2>${t("Sources")}</h2>
    <div class="st-pick-sub">${account ? t("Your account is connected. Choose this before you go live. You can change it later under Video.") : t("Your stream key is saved. Choose this before you go live. You can change it later under Video.")}</div>
    <div class="st-choices">${choice("pick-game", t("Only the game"), t("Your desktop and your other windows stay off the stream. While no game is on screen, viewers see a black picture"), true)}${choice("pick-window", t("One window"), t("Only the window you pick, whatever is in front of it"))}${choice("pick-monitor", t("Whole screen"), t("Everything on the screen, as you see it"))}</div></div>`;
}
/** A choice made on the card. */
function pickAct(name, el, c) {
  if (name === "pick-back") { pickWin = false; return c.render(); }
  if (name === "pick-window") { pickWin = true; askWindows(c, true); return c.render(); }
  if (name === "pick-win") {
    const w = (wins || []).find((x) => String(x.hwnd) === el.dataset.hwnd);
    if (!w) return askWindows(c, true);
    Object.assign(c.s, { source: "window", window_hwnd: w.hwnd, window_exe: w.exe, window_title: w.title });
    c.flash(t("Viewers will see only {name}", { name: w.title.slice(0, 60) }));
  } else {
    c.s.source = name === "pick-game" ? "game" : "monitor";
    c.flash(name === "pick-game" ? t("Viewers will see only the game") : t("Viewers will see your whole screen"));
  }
  pickWin = false; c.s.source_picked = true; lastSource = c.s.source;
  c.save(); c.render();
}
/** The card comes up as soon as the key is in its box (a text box saves without the page being drawn again), and Go live with it. */
function pickSync(c) {
  if (lastSource && lastSource !== c.s.source) c.s.source_picked = true;
  lastSource = c.s.source;
  const slot = document.querySelector(".st-pick-slot"), goBtn = document.querySelector('.st-head [data-pact="go"]');
  if (slot && !!slot.firstElementChild !== !!pickHtml(c)) slot.innerHTML = pickHtml(c);
  if (goBtn) { const ready = !!buildLive(c); goBtn.disabled = !ready; if (ready) goBtn.title = ""; }
}
function videoRows(c) {
  const g = c.glint, mon = monitorOf(c);
  const cap = (g.monitors || []).find((m) => m.index === g.settings.monitor) || mon;
  const heights = [720, 900, 1080, 1440].filter((h) => h < mon.height);
  if (!heights.includes(c.s.height)) c.s.height = 0; // a height this monitor does not have below its own: its own
  const { w, h } = sizeOf(c);
  const encName = { nvenc: "NVIDIA NVENC", amf: "AMD AMF", qsv: "Intel Quick Sync", x264: "x264 (CPU)" };
  const kind = kindOf(c);
  if (kind === "window") askWindows(c);
  const picked = (wins || []).find((x) => x.hwnd === c.s.window_hwnd);
  const winOpts = [[0, wins ? t("Pick a window") : t("Looking for windows…")], ...(!picked && c.s.window_title ? [[c.s.window_hwnd, `${c.s.window_title} (${t("not open")})`]] : []), ...(wins || []).map((x) => [x.hwnd, `${x.title.slice(0, 60)} · ${x.exe}`])];
  const game = (g.status && g.status.game) || "";
  return c.row(t("What to show"), kind === "game" ? (game ? t("Only {game}. While it is minimized or closed, viewers see a black picture", { game }) : t("Only the game Glint sees. While none is on screen, viewers see a black picture"))
      : kind === "window" ? t("Only one window, whatever is in front of it. Needs Windows 11") : t("A whole screen, with everything on it"),
      c.select("source", [["monitor", t("Display")], ["game", t("Game")], ["window", t("Window")]]))
    + (kind === "window" ? c.row(t("Window"), t("A window that stands still sends no new picture, so a stream of it can stall. Minimized, it shows black"), `${c.select("window_hwnd", winOpts, true)}<button class="btn sm" data-pact="wins">${t("Refresh")}</button>`) : "")
    + (kind === "monitor" ? c.row(t("Monitor"), t("The screen that is streamed"), c.select("monitor", [[-1, t("Capture monitor ({name})", { name: cap.name })], ...(g.monitors || []).map((m) => [m.index, `${m.name} · ${m.width}×${m.height}`])], true)) : "")
    + c.row(t("Resolution"), t("Streams as {size}. Lower needs less upload speed", { size: `${w}×${h}` }), c.select("height", [...heights.map((x) => [x, `${x}p`]), [0, t("Native")]], true))
    + c.row(t("Frame rate"), "", c.select("fps", [[30, "30 fps"], [60, "60 fps"]], true))
    + c.row(t("Bitrate"), t("Twitch takes up to about 6000 kbps. Stay under your upload speed"), c.select("bitrate", [2500, 3500, 4500, 6000, 8000, 12000, 20000].map((b) => [b, `${b} kbps`]), true))
    + c.row(t("Encoder"), t("Auto uses your graphics card: {name}", { name: encName[encoderOf({ s: { encoder: "auto" }, glint: g })] }), c.select("encoder", [["auto", t("Auto")], ["nvenc", encName.nvenc], ["amf", encName.amf], ["qsv", encName.qsv], ["x264", t("x264 (CPU, heavy)")]]));
}
function cameraRows(c) {
  if (typeof c.confirm !== "function") return "";
  if (!["ask", "allow", "deny"].includes(c.s.camera_access)) c.s.camera_access = "ask";
  return c.row(t("Camera in the preview"), t("Whether Glint may open your camera to show it on the Stream page. A camera in your scene goes out on the stream either way"),
    c.select("camera_access", [["ask", t("Ask me")], ["allow", t("Allowed")], ["deny", t("Not allowed")]]));
}
function audioRows(c) {
  askAudioDevs(c);
  const m = mixOf(c), g = c.glint, split = !m.desktop.on;
  if (split && m.app.on) askWindows(c);
  const vol = (key, off) => `<input class="range st-vol" type="range" min="0" max="150" step="5" data-set="${c.key(`mix.${key}.vol`)}" data-num value="${m[key].vol}"${off ? " disabled" : ""}><span class="range-val">${m[key].vol}%</span>`;
  const sw = (key, off) => `<label class="switch"><input type="checkbox" data-set="${c.key(`mix.${key}.on`)}" ${m[key].on ? "checked" : ""}${off ? " disabled" : ""}><i></i></label>`;
  const dev = (key, list) => { const cur = m[key].device, names = list.map((d) => d.name); return c.select(`mix.${key}.device`, [["", t("Windows default")], ...(cur && !names.includes(cur) ? [[cur, `${cur} (${t("not connected")})`]] : []), ...names.map((n) => [n, n])]); };
  const part = (title, hint, key, extra = "") => `<div class="st-part${split ? "" : " off"}">${c.row(title, hint, `${extra}${vol(key, !split || !m[key].on)}${sw(key, !split)}`)}</div>`;
  const game = (g.status && g.status.game) || "";
  const exes = [...new Set([...(wins || []).map((x) => x.exe), ...(m.app.exe ? [m.app.exe] : [])])].filter((x) => !DISCORD.includes(x)).sort((a, b) => a.localeCompare(b));
  return c.row(t("Desktop audio"), t("Everything this PC plays: the game, Discord, music. Switch it off to choose programs one by one"), `${dev("desktop", audioDevs.outputs)}${vol("desktop", !m.desktop.on)}${sw("desktop")}`)
    + c.row(t("Microphone"), "", `${dev("mic", audioDevs.inputs)}${vol("mic", !m.mic.on)}${sw("mic")}`)
    + part(t("Game audio"), !split ? t("Part of desktop audio") : game ? t("Only the sound of {game}", { game }) : t("Only the sound of the game Glint sees"), "game")
    + part(t("Discord"), !split ? t("Part of desktop audio") : t("Voice chat and Discord's own sounds"), "discord")
    + part(t("Another program"), !split ? t("Part of desktop audio") : t("The sound of one more program, such as a browser or a music player"), "app",
      `<select class="field st-app" data-set="${c.key("mix.app.exe")}"${split ? "" : " disabled"}><option value="">${esc(t("Pick a program"))}</option>${exes.map((x) => `<option${x === m.app.exe ? " selected" : ""}>${esc(x)}</option>`).join("")}</select>`);
}

/* ---------- the scene's preview ---------- */

let sel = "";          // the picked source's id
let devices = null;    // the cameras ffmpeg can open, by name (null: not asked yet)
let asking = false;
let shot = "";         // the screen's newest small picture
let shotBusy = false;
let feedTimer = 0;
let setting = false;
const cams = {};       // a camera source's id -> { name, stream, state: "opening" | "on" | "failed" }
const ICON = {
  screen: '<svg viewBox="0 0 24 24"><rect x="3" y="4.500" width="18" height="12" rx="1.500"/><path d="M8.500 20h7M12 16.500V20"/></svg>',
  camera: '<svg viewBox="0 0 24 24"><rect x="3" y="6.500" width="12.500" height="11" rx="2"/><path d="M15.500 10.500 21 7.500v9l-5.500-3"/></svg>',
  eye: '<svg viewBox="0 0 24 24"><path d="M2.500 12S6 5.500 12 5.500 21.500 12 21.500 12 18 18.500 12 18.500 2.500 12 2.500 12z"/><circle cx="12" cy="12" r="2.800"/></svg>',
  eyeOff: '<svg viewBox="0 0 24 24"><path d="M4 4l16 16M9.900 5.800A9 9 0 0 1 12 5.500c6 0 9.500 6.500 9.500 6.500a17 17 0 0 1-2.800 3.600M6.300 7.400A17 17 0 0 0 2.500 12S6 18.500 12 18.500a9 9 0 0 0 3.900-.9"/></svg>',
  up: '<svg viewBox="0 0 24 24"><path d="M6 14.500 12 8.500l6 6"/></svg>',
  down: '<svg viewBox="0 0 24 24"><path d="M6 9.500 12 15.500l6-6"/></svg>',
  image: '<svg viewBox="0 0 24 24"><rect x="3.500" y="4.500" width="17" height="15" rx="2"/><circle cx="9" cy="10" r="1.600"/><path d="M4 17l5-4.500 3.500 3 3-2.500 4.500 4"/></svg>',
  blur: '<svg viewBox="0 0 24 24"><rect x="3.500" y="4.500" width="17" height="15" rx="2" stroke-dasharray="2.500 2.500"/><path d="M8 10h8M8 14h8"/></svg>',
  bin: '<svg viewBox="0 0 24 24"><path d="M4.500 7h15M9.500 7V4.500h5V7M6.500 7l.8 12.500h9.400L17.500 7"/></svg>',
};
const pct = (v) => `${+(v * 100).toFixed(3)}%`;
const nameOf = (x) => (x.type === "screen" ? { monitor: t("Screen"), game: t("Game"), window: t("Window") }[kindOf(ctx)] : x.type === "blur" ? t("Blur") : x.type === "image" ? x.name || t("Picture") : x.device || t("Camera"));
const boxStyle = (x) => `left:${pct(x.x)};top:${pct(x.y)};width:${pct(x.w)};height:${pct(x.h)};transform:rotate(${x.rot}deg);opacity:${x.opacity / 100}`;
function picStyle(x) {
  const [l, tp, r, b] = x.crop, kw = 1 - l - r, kh = 1 - tp - b;
  return `width:${pct(1 / kw)};height:${pct(1 / kh)};left:${pct(-l / kw)};top:${pct(-tp / kh)}`;
}
/* Camera access. Glint's window opens a camera only after the user said yes in Glint's own popup
   (`camera_access`: "ask" until then, "allow" or "deny" after). Glint's backend answers the
   window's camera request from the same setting, so the browser's own prompt never shows. */
let camAsking = null;   // the popup, while it is open
let camSkipped = false; // closed without an answer: not asked again until the page is opened again
let camSaved;           // the camera_access the backend has
// A Glint from before this (0.1.54) has no popup of its own and does not answer the window's request: there the browser's prompt is the question.
const camAllowed = (c) => c.s.camera_access === "allow" || typeof c.confirm !== "function";
function askCamera(c) {
  if (camAsking) return camAsking;
  camAsking = (async () => {
    const answer = await c.confirm({
      title: t("Let Glint use your camera?"),
      text: t("Glint opens your camera to show it in the Stream preview. Nothing is recorded or sent until you go live. You can change this in the Stream settings."),
      ok: t("Allow"), cancel: t("Don't allow"),
    });
    if (!answer) camSkipped = true;
    else { c.s.camera_access = answer === "yes" ? "allow" : "deny"; await c.saveNow().catch(() => {}); camSaved = c.s.camera_access; }
  })().finally(() => { camAsking = null; c.render(); });
  return camAsking;
}

/** What a camera's box says while it has no picture. */
function camMsg(x, c) {
  if (!x.device) return t("Pick a camera");
  if (status.running) return t("On the stream now");
  if (c && !camAllowed(c)) return t("Camera preview is off");
  const cm = cams[x.id];
  return cm && cm.state === "blocked" ? t("The camera is blocked") : cm && cm.state === "failed" ? t("No preview of this camera") : cm && cm.state === "on" ? "" : x.device;
}

/** Ask ffmpeg which cameras it can open (DirectShow): the answer comes back as the process "devices" ends. */
function askDevices(c) {
  if (asking) return;
  asking = true;
  const done = () => c.proc.status("devices").then((st) => { if (!st.running) gotDevices(st, c); }).catch(() => {});
  c.proc.start("devices", ["-hide_banner", "-f", "dshow", "-list_devices", "true", "-i", "dummy"], { quiet: true }).then(() => setTimeout(done, 1500)).catch(() => { asking = false; devices = devices || []; });
}
function gotDevices(st, c) {
  if (!asking) return;
  asking = false;
  const found = [];
  let video = true; // older builds list the names under a "DirectShow video devices" line
  for (const line of st.log || []) {
    if (/DirectShow video devices/i.test(line)) video = true;
    else if (/DirectShow audio devices/i.test(line)) video = false;
    if (/Alternative name/i.test(line)) continue;
    const m = /"([^"]+)"(?:\s+\((video|audio|none)\))?\s*$/.exec(line);
    if (m && (m[2] ? m[2] !== "audio" : video) && !found.includes(m[1])) found.push(m[1]);
  }
  devices = found;
  // A camera added before the list came gets the first camera that is not in the scene yet.
  const list = sources(c);
  let filled = false;
  for (const x of list) {
    if (x.type !== "camera" || x.device) continue;
    const free = found.find((d) => !list.some((v) => v.device === d));
    if (free) { x.device = free; filled = true; }
  }
  if (filled) c.save();
  c.render();
}

/** Open a camera in Glint's window for the preview. ffmpeg opens it by itself for the stream. */
async function openCam(c, x) {
  const cm = (cams[x.id] = { name: x.device, state: "opening", stream: null });
  try {
    // The backend answers the window's camera request from the saved setting: it has to have it first.
    if (c.saveNow && camSaved !== c.s.camera_access) { await c.saveNow(); camSaved = c.s.camera_access; }
    const md = navigator.mediaDevices;
    const videos = async () => (await md.enumerateDevices()).filter((d) => d.kind === "videoinput");
    let list = await videos();
    if (!list.some((d) => d.label)) { (await md.getUserMedia({ video: true })).getTracks().forEach((k) => k.stop()); list = await videos(); } // names come after the first yes
    const want = x.device.toLowerCase();
    const d = list.find((v) => v.label.toLowerCase().startsWith(want)) || list.find((v) => v.label && want.startsWith(v.label.toLowerCase()));
    if (!d) throw new Error("not found");
    const stream = await md.getUserMedia({ video: { deviceId: { exact: d.deviceId }, width: { ideal: 1280 }, height: { ideal: 720 } } });
    if (cams[x.id] !== cm) return stream.getTracks().forEach((k) => k.stop());
    cm.stream = stream; cm.state = "on";
    const got = stream.getVideoTracks()[0].getSettings();
    if (got.width && got.height) { x.aw = got.width; x.ah = got.height; }
  } catch (e) {
    if (cams[x.id] === cm) cm.state = e && e.name === "NotAllowedError" ? "blocked" : "failed";
    if (cm.state === "blocked") c.render(); // the page says how to unblock it
    return;
  }
  paintCams(c);
}
function closeCam(id) {
  const cm = cams[id];
  if (cm && cm.stream) cm.stream.getTracks().forEach((k) => k.stop());
  delete cams[id];
}
/** The cameras the preview shows right now are open, the others closed, and each picture is in its box. */
function paintCams(c) {
  const stage = document.querySelector(".st-stage");
  let want = stage && c.s.preview !== false && !status.running ? sources(c).filter((x) => x.type === "camera" && x.visible && x.device) : [];
  if (want.length && !camAllowed(c)) {
    if (c.s.camera_access !== "deny" && !camSkipped) askCamera(c); // the first camera: Glint asks, once
    want = [];
  }
  for (const id of Object.keys(cams)) { const x = want.find((v) => v.id === id); if (!x || x.device !== cams[id].name) closeCam(id); }
  for (const x of want) if (!cams[x.id]) openCam(c, x);
  if (!stage) return;
  for (const v of stage.querySelectorAll("video[data-cam]")) {
    const cm = cams[v.dataset.cam], x = sources(c).find((k) => k.id === v.dataset.cam);
    if (cm && cm.stream && v.srcObject !== cm.stream) v.srcObject = cm.stream;
    if ((!cm || !cm.stream) && v.srcObject) v.srcObject = null;
    const msg = v.parentNode.querySelector(".st-msg");
    if (msg && x) msg.textContent = camMsg(x, c);
  }
}
/** While the preview is on screen: the screen's picture twice a second, the cameras live. Everything stops when the page is left. */
function feed(c) {
  const tick = () => {
    const stage = document.querySelector(".st-stage");
    if (!stage || c.s.preview === false) return stopFeed(c);
    if (document.hidden || shotBusy || !sources(c)[0] || !stage.querySelector("img[data-shot]")) return;
    shotBusy = true;
    c.invoke("monitor_preview", { index: monitorOf(c).index, width: 640 }).then((url) => {
      if (!url) return;
      shot = url;
      const img = document.querySelector(".st-stage img[data-shot]");
      if (img) img.src = url;
    }).catch(() => {}).finally(() => { shotBusy = false; });
  };
  paintCams(c);
  if (c.s.preview === false) return stopFeed(c);
  if (!feedTimer) feedTimer = setInterval(tick, 500);
  tick();
}
function stopFeed(c) {
  clearInterval(feedTimer); feedTimer = 0;
  if (c.s.preview === false || !document.querySelector(".st-stage")) for (const id of Object.keys(cams)) closeCam(id);
}

function stageHtml(c) {
  const { w, h } = sizeOf(c), on = c.s.preview !== false;
  const boxes = sources(c).map((x) => {
    if (!x.visible) return "";
    const pic = x.type === "screen"
      ? (on ? `<img data-shot alt="" draggable="false" style="${picStyle(x)}"${shot ? ` src="${shot}"` : ""}>` : "")
      : x.type === "blur" ? `<i class="st-blur" style="--blur:${f4((sigmaOf(x) / w) * 100)}cqw"></i>`
      : x.type === "image" ? `<img alt="" draggable="false" style="${picStyle(x)}" src="${esc(c.fileSrc(x.file))}">`
      : `<video data-cam="${esc(x.id)}" muted autoplay playsinline style="${picStyle(x)}"></video><span class="st-msg">${esc(camMsg(x, c))}</span>`;
    const handles = [[-1, -1], [0, -1], [1, -1], [1, 0], [1, 1], [0, 1], [-1, 1], [-1, 0]].map(([hx, hy]) => `<i class="st-h" data-h="${hx},${hy}" style="left:${50 + hx * 50}%;top:${50 + hy * 50}%"></i>`).join("");
    return `<div class="st-src st-${x.type}${x.id === sel ? " sel" : ""}" data-src="${esc(x.id)}" style="${boxStyle(x)}"><div class="st-pic${x.flip ? " flip" : ""}">${pic}${!on && x.type !== "image" && x.type !== "blur" ? `<span class="st-msg">${esc(nameOf(x))}</span>` : ""}</div>${handles}</div>`;
  }).join("");
  return `<div class="st-stage${on ? "" : " off"}" style="aspect-ratio:${w}/${h}">${boxes}</div>`;
}
function listHtml(c) {
  const list = sources(c);
  // The top layer is the top row, as layers are listed everywhere.
  return [...list].reverse().map((x) => {
    const i = list.indexOf(x);
    return `<div class="st-item${x.id === sel ? " sel" : ""}${x.visible ? "" : " hid"}">
      <button class="st-ib" data-pact="src-eye" data-id="${esc(x.id)}" title="${esc(x.visible ? t("Hide") : t("Show"))}">${x.visible ? ICON.eye : ICON.eyeOff}</button>
      <button class="st-name" data-pact="src-sel" data-id="${esc(x.id)}">${ICON[x.type]}<span>${esc(nameOf(x))}</span></button>
      <button class="st-ib" data-pact="src-up" data-id="${esc(x.id)}" title="${esc(t("Bring forward"))}"${i === list.length - 1 ? " disabled" : ""}>${ICON.up}</button>
      <button class="st-ib" data-pact="src-down" data-id="${esc(x.id)}" title="${esc(t("Send back"))}"${i === 0 ? " disabled" : ""}>${ICON.down}</button>
      ${x.type !== "screen" ? `<button class="st-ib" data-pact="src-del" data-id="${esc(x.id)}" title="${esc(t("Remove"))}">${ICON.bin}</button>` : `<span class="st-ib"></span>`}
    </div>`;
  }).join("");
}
function propsHtml(c) {
  const x = sources(c).find((v) => v.id === sel);
  if (!x) return `<div class="st-none">${t("Click a source to move, size, crop or turn it.")}</div>`;
  const { w, h } = sizeOf(c);
  const n = (key, value, min, max, unit) => `<label class="st-n"><span>${unit}</span><input type="number" data-native data-st="${key}" value="${value}" min="${min}" max="${max}"></label>`;
  const names = [...new Set([...(devices || []), ...(x.device ? [x.device] : [])])];
  const device = x.type !== "camera" ? "" : `<div class="st-p"><span>${t("Camera")}</span><div><select class="field" data-st="device"><option value="">${esc(devices ? (names.length ? t("Pick a camera") : t("No camera found")) : t("Looking for cameras…"))}</option>${names.map((d) => `<option${d === x.device ? " selected" : ""}>${esc(d)}</option>`).join("")}</select><button class="btn sm" data-pact="devices">${t("Refresh")}</button></div></div>`;
  const place = `<div class="st-p"><span>${t("Position")}</span><div>${n("x", Math.round(x.x * w), -w * 4, w * 4, "X")}${n("y", Math.round(x.y * h), -h * 4, h * 4, "Y")}</div></div>
    <div class="st-p"><span>${t("Size")}</span><div>${n("w", Math.round(x.w * w), 8, w * 4, "W")}${n("h", Math.round(x.h * h), 8, h * 4, "H")}</div></div>`;
  if (x.type === "blur") return `${place}<div class="st-p"><span>${t("Strength")}</span><div>${n("strength", x.strength, 1, 100, "%")}</div></div>`;
  return `${device}
    <div class="st-p"><span>${t("Position")}</span><div>${n("x", Math.round(x.x * w), -w * 4, w * 4, "X")}${n("y", Math.round(x.y * h), -h * 4, h * 4, "Y")}</div></div>
    <div class="st-p"><span>${t("Size")}</span><div>${n("w", Math.round(x.w * w), 8, w * 4, "W")}${n("h", Math.round(x.h * h), 8, h * 4, "H")}</div></div>
    <div class="st-p"><span>${t("Turn")}</span><div>${n("rot", x.rot, -180, 180, "°")}<button class="btn sm${x.flip ? " on" : ""}" data-pact="flip">${t("Mirror")}</button></div></div>
    <div class="st-p"><span>${t("Crop")}</span><div class="st-crop">${n("cl", Math.round(x.crop[0] * 100), 0, 95, t("L"))}${n("ct", Math.round(x.crop[1] * 100), 0, 95, t("T"))}${n("cr", Math.round(x.crop[2] * 100), 0, 95, t("R"))}${n("cb", Math.round(x.crop[3] * 100), 0, 95, t("B"))}</div></div>
    <div class="st-p"><span>${t("Opacity")}</span><div>${n("opacity", x.opacity, 5, 100, "%")}</div></div>
    <div class="st-p"><span></span><div><button class="btn sm" data-pact="fit">${t("Fit")}</button><button class="btn sm" data-pact="fill">${t("Fill")}</button><button class="btn sm" data-pact="reset">${t("Reset")}</button></div></div>`;
}
/** Under the preview, while a camera of the scene cannot be shown: why, and the button that changes it. */
function camNoteHtml(c) {
  const list = sources(c).filter((x) => x.type === "camera" && x.visible && x.device);
  if (!list.length || c.s.preview === false || status.running) return "";
  if (!camAllowed(c)) return `<div class="st-cam-note"><span>${t("Glint is not allowed to show your camera in the preview. The camera still goes out when you go live.")}</span><button class="btn sm" data-pact="cam-allow">${t("Allow camera")}</button></div>`;
  if (list.some((x) => cams[x.id] && cams[x.id].state === "blocked")) return `<div class="st-cam-note"><span>${t("The camera is blocked for Glint. In Windows, open Settings → Privacy & security → Camera and let desktop apps use it, then try again.")}</span><button class="btn sm" data-pact="cam-retry">${t("Try again")}</button></div>`;
  return "";
}
function sceneHtml(c) {
  if (devices === null && sources(c).some((x) => x.type === "camera")) askDevices(c);
  const on = c.s.preview !== false;
  return `<div class="section st-scene"><h2><span>${t("Scene")}</span><button class="btn sm${on ? " on" : ""}" data-pact="preview" title="${esc(t("The live picture of the screen and the cameras. Off uses nothing: the boxes can still be moved"))}">${on ? t("Preview on") : t("Preview off")}</button></h2>
    <div class="st-body">
      <div class="st-left">${stageHtml(c)}${camNoteHtml(c)}<div class="st-hint">${t("Drag to move. Drag a corner to size, with Shift to stretch, with Alt to crop. Drop a PNG, JPEG or GIF here to add it.")}</div></div>
      <div class="st-side"><div class="st-list">${listHtml(c)}</div><div class="st-add"><button class="btn sm" data-pact="cam-add">${ICON.camera}${t("Camera")}</button><button class="btn sm" data-pact="img-add">${ICON.image}${t("Picture")}</button><button class="btn sm" data-pact="blur-add">${ICON.blur}${t("Blur")}</button></div><div class="st-props">${propsHtml(c)}</div></div>
    </div></div>`;
}

/** Fit a source in the picture (whole, or filling it), keeping its shape. */
function fitSource(c, x, fill) {
  const { w, h } = sizeOf(c);
  const shape = (shapeOf(c, x) * (1 - x.crop[0] - x.crop[2])) / (1 - x.crop[1] - x.crop[3]), frame = w / h;
  const wide = fill ? shape < frame : shape > frame;
  x.w = wide ? 1 : shape / frame; x.h = wide ? frame / shape : 1;
  x.x = (1 - x.w) / 2; x.y = (1 - x.h) / 2;
}
/** A value typed into the picked source's fields. */
function setProp(c, el) {
  const x = sources(c).find((v) => v.id === sel);
  if (!x || setting) return;
  const { w, h } = sizeOf(c), key = el.dataset.st, v = num(el.value, 0);
  if (key === "device") { x.device = el.value; closeCam(x.id); }
  else if (key === "x") x.x = v / w; else if (key === "y") x.y = v / h;
  else if (key === "w") x.w = v / w; else if (key === "h") x.h = v / h;
  else if (key === "rot") x.rot = v; else if (key === "opacity") x.opacity = v; else if (key === "strength") x.strength = v;
  else { const i = ["cl", "ct", "cr", "cb"].indexOf(key); if (i >= 0) x.crop[i] = clamp(v, 0, 95) / 100; }
  sources(c); c.save();
  setting = true; // redrawing takes the field away, which can report its change once more
  try { c.render(); } finally { setting = false; }
}
/** Moving and sizing on the preview. A drag works in the stage's own pixels, around the source's middle, so a turned source sizes along its own sides. */
function pointerDown(e, c, stage) {
  if (e.button) return;
  const box = e.target.closest(".st-src");
  if (!box) { if (sel) { sel = ""; c.render(); } return; }
  const x = sources(c).find((v) => v.id === box.dataset.src);
  if (!x) return;
  const picked = sel !== x.id;
  sel = x.id;
  for (const b of stage.querySelectorAll(".st-src")) b.classList.toggle("sel", b === box);
  e.preventDefault();
  const r = stage.getBoundingClientRect(), SW = r.width, SH = r.height;
  const [hx, hy] = (e.target.dataset.h || "").split(",").map(Number), sizing = e.target.dataset.h != null;
  const a = (x.rot * Math.PI) / 180, cos = Math.cos(a), sin = Math.sin(a);
  const w0 = x.w * SW, h0 = x.h * SH, cx0 = x.x * SW + w0 / 2, cy0 = x.y * SH + h0 / 2, x0 = x.x, y0 = x.y, crop0 = [...x.crop];
  // The uncropped picture's size on the stage: cropping keeps it and moves an edge over it.
  const fullW = w0 / (1 - crop0[0] - crop0[2]), fullH = h0 / (1 - crop0[1] - crop0[3]);
  const L = x.flip ? 2 : 0, R = x.flip ? 0 : 2; // a mirrored source's left edge is its picture's right
  let moved = false;
  const move = (ev) => {
    const dx = ev.clientX - e.clientX, dy = ev.clientY - e.clientY;
    if (!moved && Math.abs(dx) + Math.abs(dy) < 3) return;
    moved = true;
    if (!sizing) {
      let nx = x0 + dx / SW, ny = y0 + dy / SH;
      if (!x.rot && !ev.shiftKey) { // sticks to the picture's edges and middle
        const sx = 6 / SW, sy = 6 / SH;
        for (const g of [0, (1 - x.w) / 2, 1 - x.w]) if (Math.abs(nx - g) < sx) nx = g;
        for (const g of [0, (1 - x.h) / 2, 1 - x.h]) if (Math.abs(ny - g) < sy) ny = g;
      }
      x.x = nx; x.y = ny;
    } else {
      const lx = dx * cos + dy * sin, ly = -dx * sin + dy * cos;
      let w = Math.max(8, w0 + hx * lx), h = Math.max(8, h0 + hy * ly);
      if (ev.altKey && x.type !== "blur") {
        const k = [...crop0];
        if (hx) { const side = hx > 0 ? R : L, other = hx > 0 ? L : R; w = Math.min(w, fullW * (1 - crop0[other])); k[side] = clamp(1 - crop0[other] - w / fullW, 0, 0.95); }
        if (hy) { const side = hy > 0 ? 3 : 1, other = hy > 0 ? 1 : 3; h = Math.min(h, fullH * (1 - crop0[other])); k[side] = clamp(1 - crop0[other] - h / fullH, 0, 0.95); }
        x.crop = k;
      } else if (hx && hy && !ev.shiftKey && x.type !== "blur") {
        const s = Math.abs(w / w0 - 1) > Math.abs(h / h0 - 1) ? w / w0 : h / h0;
        w = w0 * s; h = h0 * s;
      }
      const ox = (hx * (w - w0)) / 2, oy = (hy * (h - h0)) / 2;
      const cx = cx0 + ox * cos - oy * sin, cy = cy0 + ox * sin + oy * cos;
      x.w = w / SW; x.h = h / SH; x.x = (cx - w / 2) / SW; x.y = (cy - h / 2) / SH;
    }
    box.style.cssText = boxStyle(x);
    const pic = box.querySelector("img, video");
    if (pic) pic.style.cssText = picStyle(x);
  };
  const up = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    window.removeEventListener("pointercancel", up);
    if (moved) { sources(c); c.save(); }
    if (moved || picked) c.render();
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
  window.addEventListener("pointercancel", up);
}
function after(c, root) {
  // A key or an address typed in: the Go live button follows without the page being drawn again (which would take the cursor).
  root.addEventListener("change", (e) => { const head = root.querySelector(".st-head"); if (head && e.target.matches("input.field")) setTimeout(() => { head.innerHTML = headHtml(c); }, 0); });
  const stage = root.querySelector(".st-stage");
  if (!stage) return;
  stage.addEventListener("pointerdown", (e) => pointerDown(e, c, stage));
  root.querySelector(".st-scene").addEventListener("change", (e) => { const el = e.target.closest("[data-st]"); if (el) { e.stopPropagation(); setProp(c, el); } });
  feed(c);
}
/** Add pictures to the scene: each is copied into the plugin's own files, checked that it opens, and put at `at` (a point of the picture, 0..1) or in the middle. */
async function addImages(c, paths, at) {
  let added = 0, step = 0;
  for (const path of paths.slice(0, 8)) {
    if (!/\.(png|jpe?g|gif)$/i.test(path)) { c.flash(t("Only PNG, JPEG and GIF pictures can be added"), true); continue; }
    let kept;
    try { kept = await c.files.keep(path); } catch (e) { c.fail(e); continue; }
    const size = await new Promise((done) => { const im = new Image(); im.onload = () => done({ w: im.naturalWidth, h: im.naturalHeight }); im.onerror = () => done(null); im.src = c.fileSrc(kept.path); });
    if (!size || !size.w || !size.h) { c.files.drop(kept.path).catch(() => {}); c.flash(t("{name} could not be opened as a picture", { name: kept.name }), true); continue; }
    const { w, h } = sizeOf(c);
    // Its own size on the stream, or smaller when that is more than half the picture.
    const k = Math.min(1, (w * 0.5) / size.w, (h * 0.5) / size.h), sw = (size.w * k) / w, sh = (size.h * k) / h;
    const mid = at || { x: 0.5, y: 0.5 };
    const src = { id: "img-" + Date.now().toString(36) + added, type: "image", file: kept.path, name: kept.name, aw: size.w, ah: size.h, x: clamp(mid.x - sw / 2 + step, 0, 1 - sw), y: clamp(mid.y - sh / 2 + step, 0, 1 - sh), w: sw, h: sh, crop: [0, 0, 0, 0], rot: 0, flip: false, opacity: 100, visible: true };
    sources(c).push(src); sel = src.id; added++; step += 0.03;
  }
  if (added) { sources(c); c.save(); c.render(); }
}
/** Files dropped on Glint's window while the Stream page is open. On the preview they land where they were dropped. */
function onDrop(paths, point, c) {
  const stage = document.querySelector(".st-stage");
  if (!stage) return;
  const r = stage.getBoundingClientRect();
  const inside = point.x >= r.left && point.x <= r.right && point.y >= r.top && point.y <= r.bottom;
  addImages(c, paths, inside ? { x: (point.x - r.left) / r.width, y: (point.y - r.top) / r.height } : null);
}

/** The buttons of the scene's list and fields. True when the click was one of them. */
function sceneAct(name, el, c) {
  const list = sources(c), id = el.dataset.id, x = list.find((v) => v.id === (id || sel));
  const done = () => { sources(c); c.save(); c.render(); return true; };
  if (name === "preview") { c.s.preview = c.s.preview === false; return done(); }
  if (name === "cam-allow") { c.s.camera_access = "allow"; camSkipped = false; c.saveNow().catch(() => {}).then(() => { camSaved = c.s.camera_access; c.render(); }); return true; }
  if (name === "cam-retry") { for (const id of Object.keys(cams)) if (cams[id].state !== "on") closeCam(id); c.render(); return true; }
  if (name === "devices") { devices = null; askDevices(c); c.render(); return true; }
  if (name === "cam-add") {
    const { w, h } = sizeOf(c), free = (devices || []).find((d) => !list.some((v) => v.device === d)) || "";
    const cw = 0.25, ch = (cw * (w / h)) / (16 / 9);
    const cam = { id: "cam-" + Date.now().toString(36), type: "camera", device: free, x: 1 - cw - 0.02, y: 1 - ch - 0.02 * (w / h), w: cw, h: ch, crop: [0, 0, 0, 0], rot: 0, flip: false, opacity: 100, visible: true };
    list.push(cam); sel = cam.id;
    if (devices === null) askDevices(c);
    return done();
  }
  if (name === "img-add") { c.invoke("pick_media_file").then((path) => { if (path) addImages(c, [path], null); }).catch(c.fail); return true; }
  if (name === "blur-add") {
    const box = { id: "blur-" + Date.now().toString(36), type: "blur", strength: 40, x: 0.35, y: 0.4, w: 0.3, h: 0.2, crop: [0, 0, 0, 0], rot: 0, flip: false, opacity: 100, visible: true };
    list.push(box); sel = box.id;
    return done();
  }
  if (!x) return false;
  const i = list.indexOf(x);
  if (name === "src-sel") { sel = sel === x.id ? "" : x.id; c.render(); return true; }
  if (name === "src-eye") { x.visible = !x.visible; return done(); }
  if (name === "src-up" && i < list.length - 1) { list.splice(i, 1); list.splice(i + 1, 0, x); return done(); }
  if (name === "src-down" && i > 0) { list.splice(i, 1); list.splice(i - 1, 0, x); return done(); }
  if (name === "src-del" && x.type !== "screen") {
    list.splice(i, 1); closeCam(x.id);
    if (x.type === "image" && !list.some((v) => v.file === x.file)) c.files.drop(x.file).catch(() => {});
    if (sel === x.id) sel = "";
    return done();
  }
  if (name === "flip") { x.flip = !x.flip; return done(); }
  if (name === "fit" || name === "fill") { x.rot = 0; fitSource(c, x, name === "fill"); return done(); }
  if (name === "reset") { Object.assign(x, { crop: [0, 0, 0, 0], rot: 0, flip: false, opacity: 100 }); fitSource(c, x, false); return done(); }
  return false;
}

function render(c) {
  const hasBind = (c.glint.settings.keybinds || []).some((k) => k.action === "plugin:stream:toggle");
  return `<div class="page-head"><div><h1>${t("Stream")}</h1><div class="sub">${t("Go live from Glint")}</div></div><div class="head-actions st-head">${headHtml(c)}</div></div>
    <div class="note">${t("<b>Preview.</b> Streams your screen, a game or a window, with cameras and pictures over it. Sound changes apply at once while live; a change to the picture pauses it for a second.")}</div>
    ${statusHtml(c)}
    ${ytLiveHtml(c)}
    <div class="st-pick-slot">${pickHtml(c)}</div>
    ${sceneHtml(c)}
    <div class="section"><h2>${t("Where to")}</h2>${whereRows(c)}</div>
    <div class="section"><h2>${t("Video")}</h2>${videoRows(c)}${cameraRows(c)}</div>
    <div class="section"><h2>${t("Sound")}</h2>${audioRows(c)}</div>
    <div class="section"><h2>${t("Keybind")}</h2>
      ${c.row(t("Start and stop with a key"), hasBind ? t("Set on the Keybinds page. It works while Glint's window is closed too") : t("Adds “Start / stop streaming” to your keybinds, where you pick its key"),
        `<button class="btn sm" data-pact="keybind">${hasBind ? t("Open Keybinds") : t("Add keybind")}</button>`)}
    </div>`;
}

async function go(c) {
  let spec = buildLive(c);
  if (!spec) return c.flash(t("Choose where to stream and paste your stream key first"), true);
  if (typeof c.proc.live !== "function") return c.flash(t("Update Glint to go live with this version of Stream"), true);
  // Nobody goes live with a whole screen without having said so.
  if (!chosen(c) && typeof c.confirm === "function") {
    const a = await c.confirm({ title: t("Sources"), text: t("The stream is set to show your whole screen, with everything on it: your desktop, your other windows, your messages. Glint can show only the game instead."), ok: t("Only the game"), cancel: t("Whole screen") });
    if (!a) return;
    c.s.source = a === "yes" ? "game" : "monitor"; c.s.source_picked = true; lastSource = c.s.source;
    await (typeof c.saveNow === "function" ? c.saveNow() : c.save());
    spec = buildLive(c);
    if (!spec) return;
  }
  if (kindOf(c) === "window" && !+c.s.window_hwnd && !c.s.window_exe) return c.flash(t("Pick the window to show first, under Video"), true);
  try {
    // A camera is one program's at a time: the preview lets go of them before ffmpeg opens them.
    const held = Object.keys(cams);
    held.forEach(closeCam);
    if (held.length) await new Promise((r) => setTimeout(r, 400));
    ended = null;
    // On YouTube with an account, Glint makes the broadcast first: title, description and who can watch are right from the start.
    let made = null;
    if (serviceOf(c.s) === "youtube" && ytOn(c) && ytAccount(c) && ytAccount(c).stream && c.s.key === ytAccount(c).key) {
      try { made = await ytCreate(c); } catch (e) { return c.flash(`${t("YouTube did not set up the stream:")} ${String(e.message || e)}`, true); }
    }
    try { status = await c.proc.live(PROC, spec); } catch (e) { if (made) ytDrop(c, made.id); throw e; }
    if (made) { ytLive = { run: status.started, id: made.id, title: made.title, viewers: null, at: Date.now(), tries: 0, busy: false, error: "" }; c.save(); }
    live = shape(c); liveKey = shapeKey(c);
    watch(c);
    c.render();
    twSendInfo(c, true); // a title typed and not sent yet goes with the stream
  } catch (e) { c.fail(e); }
}
/** Put the stream's link on the clipboard. */
function copyLink(c, link) {
  if (!link) return;
  (typeof c.copy === "function" ? c.copy(link) : navigator.clipboard.writeText(link)).then(() => c.flash(t("Link copied: {link}", { link }))).catch(c.fail);
}

Glint.register({
  defaults: { service: "twitch", url: "", key: "", link: "", source: "monitor", source_picked: false, window_hwnd: 0, window_exe: "", window_title: "", monitor: -1, height: 1080, fps: 60, bitrate: 6000, encoder: "auto", audio: true, mix: null, preview: true, camera_access: "ask", twitch: null, tw_title: "", tw_category: "", tw_tags: "", tw_sent: "", youtube: null, yt_title: "", yt_desc: "", yt_privacy: "public", yt_sent: "", scene: { sources: [newScreen()] }, procs: {} },
  pages: [{
    id: "stream",
    title: "Stream",
    icon: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="2.200"/><path d="M7.800 16.200a6 6 0 0 1 0-8.400M16.200 7.800a6 6 0 0 1 0 8.400M5 19A10 10 0 0 1 5 5M19 5a10 10 0 0 1 0 14"/></svg>',
    render,
    after,
  }],
  settings: [
    { title: "Stream", render: (c) => whereRows(c) + videoRows(c) + cameraRows(c) + audioRows(c) },
  ],
  // The backend has already started or stopped the process when a keybind fires: only the page follows.
  topbar,
  onDrop,
  actions: { toggle: (c) => refresh(c).then(() => { if (status.running) watch(c); }) },
  onProc: (st, c) => {
    if (st.name === "devices") { if (!st.running) gotDevices(st, c); return; }
    if (st.name !== PROC) return;
    if (status.running && !st.running && !st.error) noteEnded(c, status.started);
    status = st; live = st.running ? live || shape(c) : null;
    if (st.running) { liveKey = liveKey || shapeKey(c); watch(c); } else liveKey = "";
    c.render();
  },
  onSave: (c) => { syncKey(c); mixOf(c); syncWindow(c); pickSync(c); c.s.procs = savedProcs(c); pushLive(c); },
  onAct: (name, el, c) => {
    if (sceneAct(name, el, c)) return;
    if (name.startsWith("pick-")) return pickAct(name, el, c);
    if (name === "tw-on") return twConnect(c);
    if (name === "tw-open") return twSign && c.open(twSign.uri).catch(c.fail);
    if (name === "tw-cancel") { twStop(); return c.render(); }
    if (name === "tw-off") { twStop(); c.s.twitch = null; c.s.key = ""; c.save(); return c.render(); }
    if (name === "yt-on") return ytConnect(c);
    if (name === "yt-open") return ytSign && c.open(ytSign.uri).catch(c.fail);
    if (name === "yt-cancel") { ytStop(); return c.render(); }
    if (name === "yt-off") return ytOff(c);
    if (name === "yt-info") return ytSendInfo(c, false);
    if (name === "go") return go(c);
    if (name === "open") return c.go("p:stream:stream");
    if (name === "stop") { const started = status.started; return c.proc.stop(PROC).then((st) => { if (status.running) noteEnded(c, started); status = { ...st, running: false, error: "" }; live = null; liveKey = ""; c.render(); }).catch(c.fail); }
    if (name === "copy-link") return copyLink(c, linkOf(c));
    if (name === "copy-ended") return copyLink(c, ended && ended.link);
    if (name === "tw-info") return twSendInfo(c, false);
    if (name === "wins") { askWindows(c, true); return; }
    if (name === "showkey") { showKey = !showKey; return c.render(); }
    if (name === "keybind") {
      const binds = c.glint.settings.keybinds;
      if (!binds.some((k) => k.action === "plugin:stream:toggle")) {
        binds.push({ id: "kb-" + Date.now().toString(36), ctrl: false, shift: false, alt: false, win: false, key: "", action: "plugin:stream:toggle", seconds: 60, monitor: -1 });
        c.save();
      }
      return c.go("keybinds");
    }
  },
  start: (c) => {
    // The keybind's arguments follow this PC as it is now (its monitors, its graphics card).
    mixOf(c);
    if (JSON.stringify(c.s.procs) !== JSON.stringify(savedProcs(c))) c.save();
    refresh(c).then(() => { if (status.running) watch(c); });
    lastService = serviceOf(c.s); lastSource = c.s.source;
    twRenew(c);
    ytRenew(c);
  },
  lang: {
    ko: {
      "The connection was lost. Connecting again…": "연결이 끊겼어요. 다시 연결하는 중…", "Reconnecting…": "다시 연결 중…", "Copy stream link": "방송 링크 복사", "The connection was lost.": "연결이 끊겼어요.", "Glint is connecting again by itself.": "Glint가 알아서 다시 연결하고 있어요.", "The screen capture stopped and is starting again. The stream is still up.": "화면 캡처가 멈춰서 다시 시작하는 중이에요. 방송은 계속되고 있어요.", "No game is on screen, so viewers see a black picture until it is back.": "화면에 게임이 없어서, 게임이 돌아올 때까지 시청자에게 검은 화면이 보여요.", "The window is closed or minimized, so viewers see a black picture until it is back.": "창이 닫혔거나 최소화돼서, 창이 돌아올 때까지 시청자에게 검은 화면이 보여요.", "The stream ended.": "방송이 끝났어요.", "You were live for {time}.": "{time} 동안 방송했어요.", "Give the stream a title first": "먼저 방송 제목을 정해 주세요", "Twitch has no category called {name}": "Twitch에 {name}(이)라는 카테고리가 없어요", "Twitch did not take the change": "Twitch가 변경을 받아들이지 않았어요", "Title and category": "제목과 카테고리", "To set your stream's title, category and tags from Glint, connect Twitch once more: Glint asks for that permission now": "Glint에서 방송 제목과 카테고리, 태그를 정하려면 Twitch를 한 번 더 연결해 주세요. 이제 Glint가 그 권한을 요청해요", "Connect again": "다시 연결", "Twitch did not take the change:": "Twitch가 변경을 받아들이지 않았어요:", "Updated on Twitch": "Twitch에 반영됐어요", "Not on Twitch yet. It is also sent when you go live": "아직 Twitch에 반영되지 않았어요. 방송을 시작할 때도 전송돼요", "Works while you are live too": "방송 중에도 바꿀 수 있어요", "Title": "제목", "What viewers see above your stream": "시청자가 방송 위에서 보는 문구", "Stream title": "방송 제목", "Category": "카테고리", "The game or kind of stream, by its name on Twitch": "게임이나 방송 종류를 Twitch에 있는 이름으로 적어 주세요", "Tags": "태그", "Up to 10, with commas between them": "최대 10개, 쉼표로 구분", "Update on Twitch": "Twitch에 반영", "Updating…": "반영하는 중…", "Update": "반영", "Your stream's link": "내 방송 링크", "Optional. Where viewers watch you: Glint offers it to copy while you are live and after": "선택 사항이에요. 시청자가 방송을 보는 주소로, 방송 중과 방송 후에 Glint에서 복사할 수 있어요", "Pick a window": "창 선택", "Looking for windows…": "창 찾는 중…", "not open": "열려 있지 않음", "What to show": "보여 줄 화면", "Only {game}. While it is minimized or closed, viewers see a black picture": "{game}만 보여 줘요. 최소화되거나 닫혀 있는 동안에는 시청자에게 검은 화면이 보여요", "Only the game Glint sees. While none is on screen, viewers see a black picture": "Glint가 인식한 게임만 보여 줘요. 화면에 게임이 없는 동안에는 시청자에게 검은 화면이 보여요", "Only one window, whatever is in front of it. Needs Windows 11": "다른 창이 앞에 있어도 창 하나만 보여 줘요. Windows 11이 필요해요", "A whole screen, with everything on it": "화면 전체와 그 위의 모든 것", "Display": "디스플레이", "Game": "게임", "Window": "창", "A window that stands still sends no new picture, so a stream of it can stall. Minimized, it shows black": "움직임이 없는 창은 새 화면을 보내지 않아서 방송이 멈칫할 수 있어요. 최소화하면 검은 화면이 나와요", "Windows default": "Windows 기본값", "not connected": "연결되지 않음", "Desktop audio": "데스크톱 오디오", "Everything this PC plays: the game, Discord, music. Switch it off to choose programs one by one": "이 PC에서 나는 모든 소리예요. 게임, Discord, 음악이 다 들어가요. 끄면 프로그램을 하나씩 고를 수 있어요", "Microphone": "마이크", "Game audio": "게임 오디오", "Part of desktop audio": "데스크톱 오디오에 포함돼요", "Only the sound of {game}": "{game}의 소리만", "Only the sound of the game Glint sees": "Glint가 인식한 게임의 소리만", "Discord": "Discord", "Voice chat and Discord's own sounds": "음성 채팅과 Discord 자체 소리", "Another program": "다른 프로그램", "The sound of one more program, such as a browser or a music player": "브라우저나 음악 플레이어 같은 프로그램 하나의 소리", "Pick a program": "프로그램 선택", "<b>Preview.</b> Streams your screen, a game or a window, with cameras and pictures over it. Sound changes apply at once while live; a change to the picture pauses it for a second.": "<b>미리 보기.</b> 화면이나 게임, 창을 카메라와 사진을 얹어 방송해요. 방송 중에 소리를 바꾸면 바로 적용되고, 화면을 바꾸면 화면이 1초쯤 멈춰요.", "Update Glint to go live with this version of Stream": "이 버전의 방송 플러그인으로 방송하려면 Glint를 업데이트해 주세요", "Link copied: {link}": "링크를 복사했어요: {link}", "Go live from Glint to Twitch, YouTube or any RTMP server, with your screen, a game or a window and your cameras arranged in a scene. Adds a Stream page under Tools, its settings, and a keybind to start and stop.": "Glint에서 Twitch, YouTube 또는 RTMP 서버로 방송해요. 화면이나 게임, 창과 카메라를 장면에 배치할 수 있어요. 도구에 방송 페이지와 설정, 시작/종료 단축키가 추가돼요.",
      "YouTube did not say whose channel this is": "YouTube가 어떤 채널인지 알려 주지 않았어요", "This Google account has no YouTube channel": "이 Google 계정에는 YouTube 채널이 없어요", "YouTube sign-in is not set up on Glint's server yet": "Glint 서버에 YouTube 로그인이 아직 설정되지 않았어요", "Google did not answer": "Google이 응답하지 않았어요", "The sign-in was declined on Google": "Google에서 로그인이 거부됐어요", "Google refused the sign-in": "Google이 로그인을 거부했어요", "Approve Glint on Google": "Google에서 Glint 승인하기", "YouTube account": "YouTube 계정", "YouTube asks you to sign in again": "YouTube에 다시 로그인해야 해요", "Connect YouTube": "YouTube 연결", "{n} watching": "{n}명 시청 중", "Live on YouTube.": "YouTube에서 방송 중이에요.", "YouTube did not say how the stream is doing:": "YouTube가 방송 상태를 알려 주지 않았어요:", "Waiting for YouTube to show the stream…": "YouTube에 방송이 뜨기를 기다리는 중…", "YouTube did not take the change": "YouTube가 변경을 받아들이지 않았어요", "YouTube did not take the change:": "YouTube가 변경을 받아들이지 않았어요:", "Updated on YouTube": "YouTube에 반영됐어요", "Changes the stream that is live now": "지금 방송 중인 스트림에 반영돼요", "Update on YouTube": "YouTube에 반영",
      "Code copied": "코드를 복사했어요", "Press <b>Open page in new tab</b> below. Google's sign-in page opens in your browser.": "아래의 <b>새 탭에서 페이지 열기</b>를 눌러 주세요. 브라우저에 Google 로그인 페이지가 열려요.", "Type in this code:": "이 코드를 입력해 주세요:", "Copy": "복사", "Getting a code…": "코드를 받는 중…", "Pick your YouTube channel and press <b>Allow</b>.": "YouTube 채널을 선택하고 <b>허용</b>을 눌러 주세요.", "Come back to Glint. It connects by itself.": "Glint로 돌아오면 알아서 연결돼요.", "Waiting for you to finish on Google…": "Google에서 마치기를 기다리는 중…", "Open page in new tab": "새 탭에서 페이지 열기", "Enter this code on {page}, then pick your YouTube channel": "{page}에서 이 코드를 입력하고 YouTube 채널을 선택해 주세요", "Signed in as {name}. Live streaming is not switched on for this channel: switch it on in YouTube Studio (it can take a day), then connect again. Or paste a stream key below": "{name}(으)로 로그인됨. 이 채널은 실시간 스트리밍이 켜져 있지 않아요. YouTube 스튜디오에서 켠 다음(하루쯤 걸릴 수 있어요) 다시 연결해 주세요. 아래에 스트림 키를 붙여 넣어도 돼요", "{name} live": "{name} 라이브", "YouTube did not answer": "YouTube가 응답하지 않았어요", "Not on YouTube yet": "아직 YouTube에 반영되지 않았어요", "Used when you go live. While live, Update sends changes": "방송을 시작할 때 적용돼요. 방송 중에는 반영을 누르면 바뀌어요", "Description": "설명", "Optional": "선택 사항", "Stream Title": "방송 제목", "Signed in as: {name}": "로그인한 계정: {name}", "Visibility": "공개 범위", "Only you, and people you invite on YouTube": "나와 YouTube에서 초대한 사람만", "Anyone with the link. It is not listed on your channel": "링크가 있는 누구나. 채널에는 표시되지 않아요", "Everyone. It is listed on your channel": "모두. 채널에 표시돼요", "Public": "공개", "Unlisted": "일부 공개", "Private": "비공개", "YouTube did not set up the stream:": "YouTube가 방송을 만들지 못했어요:",
      "Twitch did not say whose account this is": "Twitch가 어떤 계정인지 알려 주지 않았어요", "Twitch did not give the stream key": "Twitch가 스트림 키를 주지 않았어요", "Twitch did not answer": "Twitch가 응답하지 않았어요", "The code ran out before it was approved. Try again.": "승인하기 전에 코드가 만료됐어요. 다시 시도해 주세요.", "Twitch refused the sign-in": "Twitch가 로그인을 거부했어요", "Approve Glint on Twitch": "Twitch에서 Glint 승인하기", "Your browser opened twitch.tv. Check that it shows this code, then press Authorize there": "브라우저에 twitch.tv가 열렸어요. 이 코드가 보이는지 확인하고 거기서 승인을 눌러 주세요", "Open Twitch": "Twitch 열기", "Cancel": "취소", "Twitch account": "Twitch 계정", "Signed in as {name}. Glint reads your stream key from Twitch": "{name}(으)로 로그인됨. Glint가 Twitch에서 스트림 키를 읽어 와요", "Disconnect": "연결 해제", "Could not sign in:": "로그인하지 못했어요:", "Twitch asks you to sign in again": "Twitch에 다시 로그인해야 해요", "Sign in and Glint gets your stream key by itself. Or paste the key below": "로그인하면 Glint가 스트림 키를 알아서 가져와요. 아래에 키를 붙여 넣어도 돼요", "Connect Twitch": "Twitch 연결",
      "Let Glint use your camera?": "Glint가 카메라를 사용해도 될까요?", "Glint opens your camera to show it in the Stream preview. Nothing is recorded or sent until you go live. You can change this in the Stream settings.": "Glint는 방송 미리 보기에 카메라를 보여 주려고 카메라를 열어요. 방송을 시작하기 전에는 아무것도 녹화되거나 전송되지 않아요. 방송 설정에서 바꿀 수 있어요.", "Allow": "허용", "Don't allow": "허용 안 함", "Camera preview is off": "카메라 미리 보기가 꺼져 있어요", "The camera is blocked": "카메라가 차단됐어요", "Glint is not allowed to show your camera in the preview. The camera still goes out when you go live.": "Glint가 미리 보기에 카메라를 보여 줄 수 없어요. 방송을 시작하면 카메라는 그대로 나가요.", "Allow camera": "카메라 허용", "The camera is blocked for Glint. In Windows, open Settings → Privacy & security → Camera and let desktop apps use it, then try again.": "Glint에서 카메라가 차단됐어요. Windows 설정 → 개인 정보 및 보안 → 카메라에서 데스크톱 앱의 카메라 사용을 허용한 뒤 다시 시도해 주세요.", "Try again": "다시 시도", "Camera in the preview": "미리 보기의 카메라", "Whether Glint may open your camera to show it on the Stream page. A camera in your scene goes out on the stream either way": "Glint가 방송 페이지에 카메라를 보여 주려고 카메라를 열어도 되는지 정해요. 장면에 있는 카메라는 어느 쪽이든 방송에 나가요", "Ask me": "물어보기", "Allowed": "허용됨", "Not allowed": "허용 안 됨",
      "Screen": "화면", "Camera": "카메라", "Pick a camera": "카메라 선택", "On the stream now": "지금 방송에 나가는 중", "No preview of this camera": "이 카메라는 미리 볼 수 없어요", "Bring forward": "앞으로 가져오기", "Send back": "뒤로 보내기", "Remove": "제거", "Click a source to move, size, crop or turn it.": "소스를 클릭하면 옮기고, 크기를 바꾸고, 자르고, 돌릴 수 있어요.", "No camera found": "카메라를 찾지 못했어요", "Looking for cameras…": "카메라 찾는 중…", "Refresh": "새로 고침", "Position": "위치", "Size": "크기", "Turn": "회전", "Mirror": "좌우 반전", "Crop": "자르기", "L": "좌", "T": "상", "R": "우", "B": "하", "Opacity": "불투명도", "Fit": "맞추기", "Fill": "채우기", "Reset": "초기화", "Scene": "장면", "The live picture of the screen and the cameras. Off uses nothing: the boxes can still be moved": "화면과 카메라의 실시간 모습이에요. 끄면 자원을 쓰지 않고, 상자는 계속 옮길 수 있어요", "Preview on": "미리 보기 켜짐", "Preview off": "미리 보기 꺼짐", "Drag to move. Drag a corner to size, with Shift to stretch, with Alt to crop. Drop a PNG, JPEG or GIF here to add it.": "끌어서 옮겨요. 모서리를 끌면 크기가 바뀌고, Shift를 누르면 늘어나고, Alt를 누르면 잘려요. PNG, JPEG, GIF를 여기에 끌어다 놓으면 추가돼요.", "Blur": "블러", "Picture": "사진", "Strength": "강도", "Only PNG, JPEG and GIF pictures can be added": "PNG, JPEG, GIF 사진만 추가할 수 있어요", "{name} could not be opened as a picture": "{name}을(를) 사진으로 열 수 없어요",
      "Stream": "방송", "Go live from Glint": "Glint에서 바로 방송하기", "Go live": "방송 시작", "Stop streaming": "방송 종료", "Streaming": "방송 중", "Connecting…": "연결 중…", "Streaming for {time}": "{time}째 방송 중", "Connecting to the server…": "서버에 연결하는 중…",
      "Start / stop streaming": "방송 시작 / 종료", "Stream started": "방송을 시작했어요", "Stream stopped": "방송을 종료했어요",
      "Choose where to stream and paste your stream key first": "먼저 방송할 곳을 고르고 스트림 키를 붙여 넣어 주세요",
      "The stream stopped.": "방송이 중단됐어요.",
      "Where to": "방송할 곳", "Video": "영상", "Sound": "소리", "Keybind": "단축키",
      "Service": "서비스", "Other services work with Custom server: use the server address they give you": "다른 서비스는 사용자 지정 서버로 쓸 수 있어요. 서비스에서 알려 주는 서버 주소를 넣어 주세요",
      "Custom server": "사용자 지정 서버", "Server address": "서버 주소", "Starts with rtmp:// or rtmps://": "rtmp:// 또는 rtmps:// 로 시작해요",
      "Stream key": "스트림 키", "From your channel's dashboard. Anyone who has it can stream as you. It is saved on this PC, in Glint's settings file": "채널 대시보드에서 볼 수 있어요. 이 키가 있으면 누구나 내 채널로 방송할 수 있어요. 이 PC의 Glint 설정 파일에 저장돼요",
      "Hide": "숨기기", "Show": "보기",
      "Monitor": "모니터", "The screen that is streamed": "방송되는 화면", "Capture monitor ({name})": "캡처 모니터 ({name})",
      "Resolution": "해상도", "Streams as {size}. Lower needs less upload speed": "{size}로 방송해요. 낮을수록 업로드 속도가 덜 필요해요", "Native": "원본",
      "Frame rate": "프레임 속도", "Bitrate": "비트레이트", "Twitch takes up to about 6000 kbps. Stay under your upload speed": "Twitch는 약 6000 kbps까지 받아요. 내 업로드 속도보다 낮게 잡아 주세요",
      "Encoder": "인코더", "Auto uses your graphics card: {name}": "자동은 그래픽 카드를 써요: {name}", "Auto": "자동", "x264 (CPU, heavy)": "x264 (CPU, 무거움)",
      "Start and stop with a key": "키로 시작하고 종료하기", "Set on the Keybinds page. It works while Glint's window is closed too": "단축키 페이지에서 정해요. Glint 창이 닫혀 있어도 작동해요",
      "Adds “Start / stop streaming” to your keybinds, where you pick its key": "단축키에 “방송 시작 / 종료”를 추가해요. 거기서 키를 정하면 돼요",
      "Which window?": "어떤 창을 보여 줄까요?", "Back": "뒤로", "minimized": "최소화됨", "No window is open. Open the game or program, then press Refresh.": "열려 있는 창이 없어요. 게임이나 프로그램을 연 다음 새로 고침을 눌러 주세요.", "Sources": "소스", "Your account is connected. Choose this before you go live. You can change it later under Video.": "계정이 연결됐어요. 방송을 시작하기 전에 골라 주세요. 나중에 영상에서 바꿀 수 있어요.", "Your stream key is saved. Choose this before you go live. You can change it later under Video.": "스트림 키가 저장됐어요. 방송을 시작하기 전에 골라 주세요. 나중에 영상에서 바꿀 수 있어요.", "Only the game": "게임만", "Your desktop and your other windows stay off the stream. While no game is on screen, viewers see a black picture": "바탕 화면과 다른 창은 방송에 나가지 않아요. 화면에 게임이 없는 동안에는 시청자에게 검은 화면이 보여요", "One window": "창 하나", "Only the window you pick, whatever is in front of it": "앞에 무엇이 있든 고른 창만 보여요", "Whole screen": "화면 전체", "Everything on the screen, as you see it": "화면에 있는 모든 것이 보이는 그대로 나가요", "Viewers will see only {name}": "시청자에게 {name}만 보여요", "Viewers will see only the game": "시청자에게 게임만 보여요", "Viewers will see your whole screen": "시청자에게 화면 전체가 보여요", "The stream is set to show your whole screen, with everything on it: your desktop, your other windows, your messages. Glint can show only the game instead.": "지금은 화면 전체가 방송되도록 되어 있어요. 바탕 화면, 다른 창, 메시지까지 모두 나가요. 대신 게임만 보여 줄 수도 있어요.", "Pick the window to show first, under Video": "먼저 영상에서 보여 줄 창을 골라 주세요",
      "A game in a window that cannot be captured by itself is not shown either: set it to fullscreen or borderless.": "단독으로 캡처할 수 없는 창 모드 게임도 나가지 않아요. 전체 화면이나 테두리 없는 창으로 바꿔 주세요.",
      "Open Keybinds": "단축키 열기", "Add keybind": "단축키 추가", "Preview": "미리 보기",
    },
    el: {
      "The connection was lost. Connecting again…": "Η σύνδεση χάθηκε. Γίνεται ξανά σύνδεση…", "Reconnecting…": "Επανασύνδεση…", "Copy stream link": "Αντιγραφή συνδέσμου μετάδοσης", "The connection was lost.": "Η σύνδεση χάθηκε.", "Glint is connecting again by itself.": "Το Glint συνδέεται ξανά μόνο του.", "The screen capture stopped and is starting again. The stream is still up.": "Η καταγραφή οθόνης σταμάτησε και ξεκινά ξανά. Η μετάδοση συνεχίζεται.", "No game is on screen, so viewers see a black picture until it is back.": "Δεν υπάρχει παιχνίδι στην οθόνη, οπότε οι θεατές βλέπουν μαύρη εικόνα μέχρι να επιστρέψει.", "The window is closed or minimized, so viewers see a black picture until it is back.": "Το παράθυρο είναι κλειστό ή ελαχιστοποιημένο, οπότε οι θεατές βλέπουν μαύρη εικόνα μέχρι να επιστρέψει.", "The stream ended.": "Η μετάδοση τελείωσε.", "You were live for {time}.": "Ήσουν σε μετάδοση για {time}.", "Give the stream a title first": "Δώσε πρώτα έναν τίτλο στη μετάδοση", "Twitch has no category called {name}": "Το Twitch δεν έχει κατηγορία με όνομα {name}", "Twitch did not take the change": "Το Twitch δεν δέχτηκε την αλλαγή", "Title and category": "Τίτλος και κατηγορία", "To set your stream's title, category and tags from Glint, connect Twitch once more: Glint asks for that permission now": "Για να ορίζεις τίτλο, κατηγορία και ετικέτες της μετάδοσης από το Glint, σύνδεσε ξανά το Twitch: το Glint ζητά πλέον αυτή την άδεια", "Connect again": "Σύνδεση ξανά", "Twitch did not take the change:": "Το Twitch δεν δέχτηκε την αλλαγή:", "Updated on Twitch": "Ενημερώθηκε στο Twitch", "Not on Twitch yet. It is also sent when you go live": "Δεν έχει σταλεί ακόμη στο Twitch. Στέλνεται και όταν ξεκινάς μετάδοση", "Works while you are live too": "Λειτουργεί και κατά τη μετάδοση", "Title": "Τίτλος", "What viewers see above your stream": "Αυτό που βλέπουν οι θεατές πάνω από τη μετάδοσή σου", "Stream title": "Τίτλος μετάδοσης", "Category": "Κατηγορία", "The game or kind of stream, by its name on Twitch": "Το παιχνίδι ή το είδος της μετάδοσης, με το όνομά του στο Twitch", "Tags": "Ετικέτες", "Up to 10, with commas between them": "Έως 10, χωρισμένες με κόμματα", "Update on Twitch": "Ενημέρωση στο Twitch", "Updating…": "Ενημέρωση…", "Update": "Ενημέρωση", "Your stream's link": "Ο σύνδεσμος της μετάδοσής σου", "Optional. Where viewers watch you: Glint offers it to copy while you are live and after": "Προαιρετικό. Εκεί όπου σε βλέπουν οι θεατές: το Glint τον προσφέρει για αντιγραφή κατά τη μετάδοση και μετά", "Pick a window": "Διάλεξε παράθυρο", "Looking for windows…": "Αναζήτηση παραθύρων…", "not open": "δεν είναι ανοιχτό", "What to show": "Τι να φαίνεται", "Only {game}. While it is minimized or closed, viewers see a black picture": "Μόνο το {game}. Όσο είναι ελαχιστοποιημένο ή κλειστό, οι θεατές βλέπουν μαύρη εικόνα", "Only the game Glint sees. While none is on screen, viewers see a black picture": "Μόνο το παιχνίδι που βλέπει το Glint. Όσο δεν υπάρχει κανένα στην οθόνη, οι θεατές βλέπουν μαύρη εικόνα", "Only one window, whatever is in front of it. Needs Windows 11": "Μόνο ένα παράθυρο, ό,τι κι αν βρίσκεται μπροστά του. Χρειάζεται Windows 11", "A whole screen, with everything on it": "Ολόκληρη οθόνη, με ό,τι υπάρχει πάνω της", "Display": "Οθόνη", "Game": "Παιχνίδι", "Window": "Παράθυρο", "A window that stands still sends no new picture, so a stream of it can stall. Minimized, it shows black": "Ένα παράθυρο που μένει ακίνητο δεν στέλνει νέα εικόνα, οπότε η μετάδοσή του μπορεί να κολλήσει. Ελαχιστοποιημένο, δείχνει μαύρο", "Windows default": "Προεπιλογή των Windows", "not connected": "μη συνδεδεμένη", "Desktop audio": "Ήχος επιφάνειας εργασίας", "Everything this PC plays: the game, Discord, music. Switch it off to choose programs one by one": "Ό,τι παίζει αυτός ο υπολογιστής: το παιχνίδι, το Discord, η μουσική. Απενεργοποίησέ το για να διαλέξεις προγράμματα ένα ένα", "Microphone": "Μικρόφωνο", "Game audio": "Ήχος παιχνιδιού", "Part of desktop audio": "Μέρος του ήχου επιφάνειας εργασίας", "Only the sound of {game}": "Μόνο ο ήχος του {game}", "Only the sound of the game Glint sees": "Μόνο ο ήχος του παιχνιδιού που βλέπει το Glint", "Discord": "Discord", "Voice chat and Discord's own sounds": "Φωνητική συνομιλία και οι ήχοι του ίδιου του Discord", "Another program": "Άλλο πρόγραμμα", "The sound of one more program, such as a browser or a music player": "Ο ήχος ενός ακόμη προγράμματος, όπως ένας περιηγητής ή ένα πρόγραμμα μουσικής", "Pick a program": "Διάλεξε πρόγραμμα", "<b>Preview.</b> Streams your screen, a game or a window, with cameras and pictures over it. Sound changes apply at once while live; a change to the picture pauses it for a second.": "<b>Προεπισκόπηση.</b> Μεταδίδει την οθόνη σου, ένα παιχνίδι ή ένα παράθυρο, με κάμερες και εικόνες από πάνω. Οι αλλαγές στον ήχο ισχύουν αμέσως κατά τη μετάδοση· μια αλλαγή στην εικόνα την παγώνει για ένα δευτερόλεπτο.", "Update Glint to go live with this version of Stream": "Ενημέρωσε το Glint για να μεταδώσεις με αυτή την έκδοση της Μετάδοσης", "Link copied: {link}": "Ο σύνδεσμος αντιγράφηκε: {link}", "Go live from Glint to Twitch, YouTube or any RTMP server, with your screen, a game or a window and your cameras arranged in a scene. Adds a Stream page under Tools, its settings, and a keybind to start and stop.": "Ζωντανή μετάδοση από το Glint σε Twitch, YouTube ή οποιονδήποτε διακομιστή RTMP, με την οθόνη σου, ένα παιχνίδι ή ένα παράθυρο και τις κάμερές σου τοποθετημένες σε σκηνή. Προσθέτει τη σελίδα Μετάδοση στα Εργαλεία, τις ρυθμίσεις της και συντόμευση για έναρξη και διακοπή.",
      "YouTube did not say whose channel this is": "Το YouTube δεν είπε ποιανού είναι το κανάλι", "This Google account has no YouTube channel": "Αυτός ο λογαριασμός Google δεν έχει κανάλι στο YouTube", "YouTube sign-in is not set up on Glint's server yet": "Η σύνδεση με το YouTube δεν έχει ρυθμιστεί ακόμη στον διακομιστή του Glint", "Google did not answer": "Η Google δεν απάντησε", "The sign-in was declined on Google": "Η σύνδεση απορρίφθηκε στη Google", "Google refused the sign-in": "Η Google αρνήθηκε τη σύνδεση", "Approve Glint on Google": "Έγκρινε το Glint στη Google", "YouTube account": "Λογαριασμός YouTube", "YouTube asks you to sign in again": "Το YouTube ζητά να συνδεθείς ξανά", "Connect YouTube": "Σύνδεση YouTube", "{n} watching": "{n} παρακολουθούν", "Live on YouTube.": "Ζωντανά στο YouTube.", "YouTube did not say how the stream is doing:": "Το YouTube δεν είπε πώς πάει η μετάδοση:", "Waiting for YouTube to show the stream…": "Αναμονή μέχρι το YouTube να δείξει τη μετάδοση…", "YouTube did not take the change": "Το YouTube δεν δέχτηκε την αλλαγή", "YouTube did not take the change:": "Το YouTube δεν δέχτηκε την αλλαγή:", "Updated on YouTube": "Ενημερώθηκε στο YouTube", "Changes the stream that is live now": "Αλλάζει τη μετάδοση που είναι τώρα ζωντανή", "Update on YouTube": "Ενημέρωση στο YouTube",
      "Code copied": "Ο κωδικός αντιγράφηκε", "Press <b>Open page in new tab</b> below. Google's sign-in page opens in your browser.": "Πάτησε <b>Άνοιγμα σελίδας σε νέα καρτέλα</b> παρακάτω. Η σελίδα σύνδεσης της Google ανοίγει στον περιηγητή σου.", "Type in this code:": "Γράψε αυτόν τον κωδικό:", "Copy": "Αντιγραφή", "Getting a code…": "Λήψη κωδικού…", "Pick your YouTube channel and press <b>Allow</b>.": "Διάλεξε το κανάλι σου στο YouTube και πάτησε <b>Να επιτρέπεται</b>.", "Come back to Glint. It connects by itself.": "Γύρνα στο Glint. Συνδέεται μόνο του.", "Waiting for you to finish on Google…": "Αναμονή μέχρι να τελειώσεις στη Google…", "Open page in new tab": "Άνοιγμα σελίδας σε νέα καρτέλα", "Enter this code on {page}, then pick your YouTube channel": "Γράψε αυτόν τον κωδικό στο {page} και διάλεξε το κανάλι σου στο YouTube", "Signed in as {name}. Live streaming is not switched on for this channel: switch it on in YouTube Studio (it can take a day), then connect again. Or paste a stream key below": "Συνδεδεμένος ως {name}. Η ζωντανή μετάδοση δεν είναι ενεργή σε αυτό το κανάλι: ενεργοποίησέ τη στο YouTube Studio (μπορεί να πάρει μια μέρα) και σύνδεσε ξανά. Ή επικόλλησε ένα κλειδί μετάδοσης παρακάτω", "{name} live": "{name} ζωντανά", "YouTube did not answer": "Το YouTube δεν απάντησε", "Not on YouTube yet": "Δεν έχει σταλεί ακόμη στο YouTube", "Used when you go live. While live, Update sends changes": "Ισχύει όταν ξεκινάς μετάδοση. Κατά τη μετάδοση, η Ενημέρωση στέλνει τις αλλαγές", "Description": "Περιγραφή", "Optional": "Προαιρετικό", "Stream Title": "Τίτλος μετάδοσης", "Signed in as: {name}": "Συνδεδεμένος ως: {name}", "Visibility": "Ορατότητα", "Only you, and people you invite on YouTube": "Μόνο εσύ και όσοι προσκαλέσεις στο YouTube", "Anyone with the link. It is not listed on your channel": "Όποιος έχει τον σύνδεσμο. Δεν εμφανίζεται στο κανάλι σου", "Everyone. It is listed on your channel": "Όλοι. Εμφανίζεται στο κανάλι σου", "Public": "Δημόσια", "Unlisted": "Μη καταχωρισμένη", "Private": "Ιδιωτική", "YouTube did not set up the stream:": "Το YouTube δεν έστησε τη μετάδοση:",
      "Twitch did not say whose account this is": "Το Twitch δεν είπε ποιανού είναι ο λογαριασμός", "Twitch did not give the stream key": "Το Twitch δεν έδωσε το κλειδί μετάδοσης", "Twitch did not answer": "Το Twitch δεν απάντησε", "The code ran out before it was approved. Try again.": "Ο κωδικός έληξε πριν εγκριθεί. Δοκίμασε ξανά.", "Twitch refused the sign-in": "Το Twitch αρνήθηκε τη σύνδεση", "Approve Glint on Twitch": "Έγκρινε το Glint στο Twitch", "Your browser opened twitch.tv. Check that it shows this code, then press Authorize there": "Ο περιηγητής σου άνοιξε το twitch.tv. Έλεγξε ότι δείχνει αυτόν τον κωδικό και πάτησε εκεί Εξουσιοδότηση", "Open Twitch": "Άνοιγμα Twitch", "Cancel": "Άκυρο", "Twitch account": "Λογαριασμός Twitch", "Signed in as {name}. Glint reads your stream key from Twitch": "Συνδεδεμένος ως {name}. Το Glint διαβάζει το κλειδί μετάδοσης από το Twitch", "Disconnect": "Αποσύνδεση", "Could not sign in:": "Η σύνδεση απέτυχε:", "Twitch asks you to sign in again": "Το Twitch ζητά να συνδεθείς ξανά", "Sign in and Glint gets your stream key by itself. Or paste the key below": "Συνδέσου και το Glint παίρνει μόνο του το κλειδί μετάδοσης. Ή επικόλλησε το κλειδί παρακάτω", "Connect Twitch": "Σύνδεση Twitch",
      "Let Glint use your camera?": "Να χρησιμοποιήσει το Glint την κάμερά σου;", "Glint opens your camera to show it in the Stream preview. Nothing is recorded or sent until you go live. You can change this in the Stream settings.": "Το Glint ανοίγει την κάμερά σου για να τη δείξει στην προεπισκόπηση της Μετάδοσης. Τίποτα δεν καταγράφεται ούτε στέλνεται μέχρι να ξεκινήσεις μετάδοση. Μπορείς να το αλλάξεις στις ρυθμίσεις της Μετάδοσης.", "Allow": "Να επιτρέπεται", "Don't allow": "Να μην επιτρέπεται", "Camera preview is off": "Η προεπισκόπηση κάμερας είναι ανενεργή", "The camera is blocked": "Η κάμερα είναι αποκλεισμένη", "Glint is not allowed to show your camera in the preview. The camera still goes out when you go live.": "Το Glint δεν επιτρέπεται να δείξει την κάμερά σου στην προεπισκόπηση. Η κάμερα μεταδίδεται κανονικά όταν ξεκινήσεις μετάδοση.", "Allow camera": "Να επιτρέπεται η κάμερα", "The camera is blocked for Glint. In Windows, open Settings → Privacy & security → Camera and let desktop apps use it, then try again.": "Η κάμερα είναι αποκλεισμένη για το Glint. Στα Windows άνοιξε Ρυθμίσεις → Απόρρητο και ασφάλεια → Κάμερα, επίτρεψε στις εφαρμογές υπολογιστή να τη χρησιμοποιούν και δοκίμασε ξανά.", "Try again": "Δοκίμασε ξανά", "Camera in the preview": "Κάμερα στην προεπισκόπηση", "Whether Glint may open your camera to show it on the Stream page. A camera in your scene goes out on the stream either way": "Αν το Glint μπορεί να ανοίγει την κάμερά σου για να τη δείχνει στη σελίδα Μετάδοση. Μια κάμερα της σκηνής μεταδίδεται σε κάθε περίπτωση", "Ask me": "Να ερωτώμαι", "Allowed": "Επιτρέπεται", "Not allowed": "Δεν επιτρέπεται",
      "Screen": "Οθόνη", "Camera": "Κάμερα", "Pick a camera": "Διάλεξε κάμερα", "On the stream now": "Στη μετάδοση τώρα", "No preview of this camera": "Χωρίς προεπισκόπηση για αυτή την κάμερα", "Bring forward": "Μεταφορά μπροστά", "Send back": "Μεταφορά πίσω", "Remove": "Αφαίρεση", "Click a source to move, size, crop or turn it.": "Κάνε κλικ σε μια πηγή για να τη μετακινήσεις, να αλλάξεις το μέγεθός της, να την περικόψεις ή να τη γυρίσεις.", "No camera found": "Δεν βρέθηκε κάμερα", "Looking for cameras…": "Αναζήτηση καμερών…", "Refresh": "Ανανέωση", "Position": "Θέση", "Size": "Μέγεθος", "Turn": "Περιστροφή", "Mirror": "Καθρέφτης", "Crop": "Περικοπή", "L": "Α", "T": "Π", "R": "Δ", "B": "Κ", "Opacity": "Αδιαφάνεια", "Fit": "Προσαρμογή", "Fill": "Γέμισμα", "Reset": "Επαναφορά", "Scene": "Σκηνή", "The live picture of the screen and the cameras. Off uses nothing: the boxes can still be moved": "Η ζωντανή εικόνα της οθόνης και των καμερών. Όταν είναι ανενεργή δεν χρησιμοποιεί τίποτα: τα πλαίσια μετακινούνται ακόμα", "Preview on": "Προεπισκόπηση ενεργή", "Preview off": "Προεπισκόπηση ανενεργή", "Drag to move. Drag a corner to size, with Shift to stretch, with Alt to crop. Drop a PNG, JPEG or GIF here to add it.": "Σύρε για μετακίνηση. Σύρε μια γωνία για αλλαγή μεγέθους, με Shift για τέντωμα, με Alt για περικοπή. Άφησε εδώ ένα PNG, JPEG ή GIF για να το προσθέσεις.", "Blur": "Θόλωμα", "Picture": "Εικόνα", "Strength": "Ένταση", "Only PNG, JPEG and GIF pictures can be added": "Μόνο εικόνες PNG, JPEG και GIF μπορούν να προστεθούν", "{name} could not be opened as a picture": "Το {name} δεν άνοιξε ως εικόνα",
      "Stream": "Μετάδοση", "Go live from Glint": "Ζωντανή μετάδοση από το Glint", "Go live": "Έναρξη μετάδοσης", "Stop streaming": "Διακοπή μετάδοσης", "Streaming": "Μετάδοση", "Connecting…": "Σύνδεση…", "Streaming for {time}": "Μετάδοση εδώ και {time}", "Connecting to the server…": "Σύνδεση με τον διακομιστή…",
      "Start / stop streaming": "Έναρξη / διακοπή μετάδοσης", "Stream started": "Η μετάδοση ξεκίνησε", "Stream stopped": "Η μετάδοση σταμάτησε",
      "Choose where to stream and paste your stream key first": "Διάλεξε πρώτα πού θα μεταδώσεις και επικόλλησε το κλειδί μετάδοσης",
      "The stream stopped.": "Η μετάδοση σταμάτησε.",
      "Where to": "Προορισμός", "Video": "Βίντεο", "Sound": "Ήχος", "Keybind": "Συντόμευση",
      "Service": "Υπηρεσία", "Other services work with Custom server: use the server address they give you": "Άλλες υπηρεσίες δουλεύουν με Προσαρμοσμένο διακομιστή: βάλε τη διεύθυνση διακομιστή που σου δίνουν",
      "Custom server": "Προσαρμοσμένος διακομιστής", "Server address": "Διεύθυνση διακομιστή", "Starts with rtmp:// or rtmps://": "Ξεκινά με rtmp:// ή rtmps://",
      "Stream key": "Κλειδί μετάδοσης", "From your channel's dashboard. Anyone who has it can stream as you. It is saved on this PC, in Glint's settings file": "Από τον πίνακα ελέγχου του καναλιού σου. Όποιος το έχει μπορεί να μεταδώσει ως εσύ. Αποθηκεύεται σε αυτόν τον υπολογιστή, στο αρχείο ρυθμίσεων του Glint",
      "Hide": "Απόκρυψη", "Show": "Εμφάνιση",
      "Monitor": "Οθόνη", "The screen that is streamed": "Η οθόνη που μεταδίδεται", "Capture monitor ({name})": "Οθόνη καταγραφής ({name})",
      "Resolution": "Ανάλυση", "Streams as {size}. Lower needs less upload speed": "Μεταδίδει σε {size}. Χαμηλότερη ανάλυση θέλει μικρότερη ταχύτητα αποστολής", "Native": "Εγγενής",
      "Frame rate": "Ρυθμός καρέ", "Bitrate": "Ρυθμός bit", "Twitch takes up to about 6000 kbps. Stay under your upload speed": "Το Twitch δέχεται έως περίπου 6000 kbps. Μείνε κάτω από την ταχύτητα αποστολής σου",
      "Encoder": "Κωδικοποιητής", "Auto uses your graphics card: {name}": "Το Αυτόματο χρησιμοποιεί την κάρτα γραφικών σου: {name}", "Auto": "Αυτόματο", "x264 (CPU, heavy)": "x264 (CPU, βαρύ)",
      "Start and stop with a key": "Έναρξη και διακοπή με πλήκτρο", "Set on the Keybinds page. It works while Glint's window is closed too": "Ορίζεται στη σελίδα Συντομεύσεις. Δουλεύει και όταν το παράθυρο του Glint είναι κλειστό",
      "Adds “Start / stop streaming” to your keybinds, where you pick its key": "Προσθέτει το «Έναρξη / διακοπή μετάδοσης» στις συντομεύσεις σου, όπου διαλέγεις το πλήκτρο",
      "Which window?": "Ποιο παράθυρο;", "Back": "Πίσω", "minimized": "ελαχιστοποιημένο", "No window is open. Open the game or program, then press Refresh.": "Δεν υπάρχει ανοιχτό παράθυρο. Άνοιξε το παιχνίδι ή το πρόγραμμα και πάτησε Ανανέωση.", "Sources": "Πηγές", "Your account is connected. Choose this before you go live. You can change it later under Video.": "Ο λογαριασμός σου συνδέθηκε. Διάλεξε πριν ξεκινήσεις τη μετάδοση. Αλλάζει αργότερα στο Βίντεο.", "Your stream key is saved. Choose this before you go live. You can change it later under Video.": "Το κλειδί μετάδοσης αποθηκεύτηκε. Διάλεξε πριν ξεκινήσεις τη μετάδοση. Αλλάζει αργότερα στο Βίντεο.", "Only the game": "Μόνο το παιχνίδι", "Your desktop and your other windows stay off the stream. While no game is on screen, viewers see a black picture": "Η επιφάνεια εργασίας και τα άλλα παράθυρά σου μένουν εκτός μετάδοσης. Όσο δεν υπάρχει παιχνίδι στην οθόνη, οι θεατές βλέπουν μαύρη εικόνα", "One window": "Ένα παράθυρο", "Only the window you pick, whatever is in front of it": "Μόνο το παράθυρο που διαλέγεις, ό,τι κι αν είναι μπροστά του", "Whole screen": "Ολόκληρη η οθόνη", "Everything on the screen, as you see it": "Ό,τι υπάρχει στην οθόνη, όπως το βλέπεις", "Viewers will see only {name}": "Οι θεατές θα βλέπουν μόνο το {name}", "Viewers will see only the game": "Οι θεατές θα βλέπουν μόνο το παιχνίδι", "Viewers will see your whole screen": "Οι θεατές θα βλέπουν ολόκληρη την οθόνη σου", "The stream is set to show your whole screen, with everything on it: your desktop, your other windows, your messages. Glint can show only the game instead.": "Η μετάδοση είναι ρυθμισμένη να δείχνει ολόκληρη την οθόνη σου, με ό,τι έχει πάνω: την επιφάνεια εργασίας, τα άλλα παράθυρα, τα μηνύματά σου. Το Glint μπορεί να δείχνει μόνο το παιχνίδι.", "Pick the window to show first, under Video": "Διάλεξε πρώτα το παράθυρο που θα φαίνεται, στο Βίντεο",
      "A game in a window that cannot be captured by itself is not shown either: set it to fullscreen or borderless.": "Ούτε ένα παιχνίδι σε παράθυρο που δεν καταγράφεται μόνο του φαίνεται: βάλε το σε πλήρη οθόνη ή σε παράθυρο χωρίς περίγραμμα.",
      "Open Keybinds": "Άνοιγμα Συντομεύσεων", "Add keybind": "Προσθήκη συντόμευσης", "Preview": "Προεπισκόπηση",
    },
  },
});
