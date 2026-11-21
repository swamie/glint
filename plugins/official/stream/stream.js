// Stream: go live from Glint. An official Glint plugin (plugins/official/stream in Glint's repository).
//
// What it does: runs Glint's ffmpeg as the process "live", which captures a monitor (Desktop
// Duplication), encodes it with the graphics card's encoder and sends it to an RTMP server.
// The picture is a scene: the screen, cameras (DirectShow), pictures (PNG, JPEG, GIF: dropped on
// the preview) and blur boxes, each moved, sized, cropped, turned and stacked on the Stream page's
// preview. A scene that is only the whole screen keeps the
// short capture chain; anything else is put together by ffmpeg's overlay filter.
// Sound is Glint's own live desktop and microphone audio, handed to ffmpeg by Glint.
// The arguments are worked out here and saved in the plugin's settings (`procs.live`), so the
// keybind "Start / stop streaming" also works while Glint's window is closed.
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
function sceneGraph(c, mon, fps, w, h) {
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
    if (x.type === "screen") from = `ddagrab=output_idx=${mon.index}:framerate=${fps}:draw_mouse=${c.glint.settings.cursor === false ? 0 : 1},hwdownload,format=bgra,`;
    else {
      // Input 0 is the sound; the cameras and pictures follow in the order of their layers.
      if (x.type === "camera") inputs.push("-f", "dshow", "-rtbufsize", "256M", "-thread_queue_size", "512", "-i", `video=${x.device}`);
      else if (isGif(x)) inputs.push("-stream_loop", "-1", "-i", x.file); // a GIF plays at its own speed, over and over
      else inputs.push("-loop", "1", "-framerate", "2", "-i", x.file); // a still picture: two frames a second are plenty
      from = `[${++cam}:v]`;
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

/** ffmpeg's arguments for the stream, or null while the server or the key is missing. */
function buildArgs(c) {
  const s = c.s;
  const url = target(s);
  if (!url) return null;
  const mon = monitorOf(c);
  const enc = encoderOf(c);
  const fps = [30, 60].includes(+s.fps) ? +s.fps : 60;
  const kbps = Math.min(51000, Math.max(500, +s.bitrate || 6000));
  const { w, h, scaled } = sizeOf(c);
  // The same capture chain as Glint's own Desktop Duplication capture (recorder.rs).
  const scene = plain(c) ? null : sceneGraph(c, mon, fps, w, h);
  let v = `ddagrab=output_idx=${mon.index}:framerate=${fps}:draw_mouse=${c.glint.settings.cursor === false ? 0 : 1}`;
  if (scene) v = `${scene.parts.join(";")};[${scene.last}]format=${enc === "x264" ? "yuv420p" : "nv12"}`;
  else if (scaled) v += `,hwdownload,format=bgra,scale=${w}:${h}:flags=bilinear${enc === "x264" ? "" : ",format=nv12"}`;
  else if (enc === "x264") v += ",hwdownload,format=bgra";
  else if (enc === "qsv") v += ",hwmap=derive_device=qsv,format=qsv";
  const rate = `${kbps}k`, buf = `${kbps * 2}k`;
  const a = ["-hide_banner", "-loglevel", "error", "-stats", "-stats_period", "2", "-nostdin"];
  // Sound: Glint's live audio on stdin ("{audio_rate}" is filled in by Glint), or silence.
  if (s.audio) a.push("-f", "s16le", "-ar", "{audio_rate}", "-ac", "2", "-thread_queue_size", "1024", "-i", "pipe:0");
  else a.push("-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo");
  if (scene) a.push(...scene.inputs);
  a.push("-filter_complex", `${v}[v]`, "-map", "[v]", "-map", "0:a");
  if (enc === "nvenc") a.push("-c:v", "h264_nvenc", "-preset", "p4", "-tune", "hq", "-bf", "0", "-rc", "cbr", "-b:v", rate, "-maxrate", rate, "-bufsize", buf, "-forced-idr", "1");
  else if (enc === "amf") a.push("-c:v", "h264_amf", "-usage", "transcoding", "-quality", "speed", "-bf", "0", "-rc", "cbr", "-b:v", rate, "-maxrate", rate, "-bufsize", buf);
  else if (enc === "qsv") a.push("-c:v", "h264_qsv", "-preset", "veryfast", "-bf", "0", "-b:v", rate, "-maxrate", rate, "-bufsize", buf);
  else a.push("-c:v", "libx264", "-preset", "veryfast", "-tune", "zerolatency", "-pix_fmt", "yuv420p", "-b:v", rate, "-maxrate", rate, "-bufsize", buf);
  // A keyframe every two seconds, which is what streaming services ask for.
  a.push("-g", String(fps * 2), "-force_key_frames", "expr:gte(t,n_forced*2)");
  a.push("-c:a", "aac", "-b:a", "160k", "-ar", "48000", "-f", "flv", url);
  return a;
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
  if (status.running && !live) live = shape(c);
  if (!status.running) live = null;
  if (was !== status.running) c.render(); else c.chrome();
}
/** While live, look every two seconds: the stream can end by itself, or by its keybind. */
function watch(c) {
  clearInterval(poll);
  poll = setInterval(() => { if (!status.running) return clearInterval(poll); refresh(c); }, 2000);
}
/** What is being sent, as the top bar says it: "1080p @ 60fps". Taken when the stream starts. */
const shape = (c) => `${sizeOf(c).h}p @ ${[30, 60].includes(+c.s.fps) ? +c.s.fps : 60}fps`;

/** The top bar while live: an ordinary top bar button, "Streaming 1080p @ 60fps" (it opens the Stream page), joined to a stop button. */
function topbar(c) {
  if (!status.running) return "";
  const sending = !!stats();
  const tip = sending ? `${t("Streaming for {time}", { time: uptime() })} · ${stats()}` : t("Connecting to the server…");
  return `<span class="top-group${sending ? " live" : ""}"><button class="btn" data-pact="open" title="${esc(tip)}"><i class="top-dot${sending ? "" : " idle"}"></i><span class="top-label">${sending ? t("Streaming") : t("Connecting…")}</span><span class="top-muted top-detail top-label">${esc(live || shape(c))}</span></button><button class="btn top-icon" data-pact="stop" title="${esc(t("Stop streaming"))}" aria-label="${esc(t("Stop streaming"))}"><svg viewBox="0 0 24 24"><rect x="6" y="6" width="12" height="12" rx="1.500"/></svg></button></span>`;
}

/** The page's one button: Go live, or Stop streaming. What is live shows in the top bar. */
function headHtml(c) {
  if (status.running) return `<button class="btn solid-danger" data-pact="stop">${t("Stop streaming")}</button>`;
  const ready = !!buildArgs(c);
  return `<button class="btn primary" data-pact="go"${ready ? "" : " disabled"} title="${esc(ready ? "" : t("Choose where to stream and paste your stream key first"))}">${t("Go live")}</button>`;
}
function statusHtml(c) {
  if (!status.error) return "";
  return `<div class="note bad"><b>${t("The stream stopped.")}</b> ${esc(status.error)}</div>`;
}

/** The settings, as sections: on the Stream page and, the same ones, on the Settings page. */
function whereRows(c) {
  const s = c.s, service = serviceOf(s);
  return c.row(t("Service"), t("Other services work with Custom server: use the server address they give you"), c.select("service", [["twitch", "Twitch"], ["youtube", "YouTube"], ["custom", t("Custom server")]]))
    + (service === "custom" ? c.row(t("Server address"), t("Starts with rtmp:// or rtmps://"), c.text("url", { placeholder: "rtmp://", width: 340 })) : "")
    + c.row(t("Stream key"), t("From your channel's dashboard. Anyone who has it can stream as you. It is saved on this PC, in Glint's settings file"),
      `${c.text("key", { password: !showKey, width: 260 })}<button class="btn sm" data-pact="showkey">${showKey ? t("Hide") : t("Show")}</button>`);
}
function videoRows(c) {
  const g = c.glint, mon = monitorOf(c);
  const cap = (g.monitors || []).find((m) => m.index === g.settings.monitor) || mon;
  const heights = [720, 900, 1080, 1440].filter((h) => h < mon.height);
  if (!heights.includes(c.s.height)) c.s.height = 0; // a height this monitor does not have below its own: its own
  const { w, h } = sizeOf(c);
  const encName = { nvenc: "NVIDIA NVENC", amf: "AMD AMF", qsv: "Intel Quick Sync", x264: "x264 (CPU)" };
  return c.row(t("Monitor"), t("The screen that is streamed"), c.select("monitor", [[-1, t("Capture monitor ({name})", { name: cap.name })], ...(g.monitors || []).map((m) => [m.index, `${m.name} · ${m.width}×${m.height}`])], true))
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
  const g = c.glint;
  const none = !g.settings.desktop_audio && !g.settings.mic;
  const hint = !c.s.audio ? t("Off: the stream is silent")
    : none ? t("Desktop audio and the microphone are both off in Settings, so the stream would be silent")
    : !(g.status && g.status.recording) ? t("Clipping is paused, and the sound comes from the same capture: the stream is silent until you resume")
    : t("Desktop sound and your microphone, as set in Settings → Audio and sounds");
  return c.row(t("Send Glint's sound"), hint, c.toggle("audio"));
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
const nameOf = (x) => (x.type === "screen" ? t("Screen") : x.type === "blur" ? t("Blur") : x.type === "image" ? x.name || t("Picture") : x.device || t("Camera"));
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
    <div class="note">${t("<b>Preview.</b> Streams your screen, cameras and pictures with Glint's sound. Changes you make while live apply the next time you go live.")}</div>
    ${statusHtml(c)}
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
  const args = buildArgs(c);
  if (!args) return c.flash(t("Choose where to stream and paste your stream key first"), true);
  try {
    // A camera is one program's at a time: the preview lets go of them before ffmpeg opens them.
    const held = Object.keys(cams);
    held.forEach(closeCam);
    if (held.length) await new Promise((r) => setTimeout(r, 400));
    status = await c.proc.start(PROC, args, { audio: !!c.s.audio });
    live = shape(c);
    watch(c);
    c.render();
  } catch (e) { c.fail(e); }
}

Glint.register({
  defaults: { service: "twitch", url: "", key: "", monitor: -1, height: 1080, fps: 60, bitrate: 6000, encoder: "auto", audio: true, preview: true, camera_access: "ask", scene: { sources: [newScreen()] }, procs: {} },
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
  onProc: (st, c) => { if (st.name === "devices") { if (!st.running) gotDevices(st, c); return; } if (st.name !== PROC) return; status = st; live = st.running ? live || shape(c) : null; if (st.running) watch(c); c.render(); },
  onSave: (c) => { c.s.procs = { [PROC]: { args: buildArgs(c) || [], audio: !!c.s.audio } }; },
  onAct: (name, el, c) => {
    if (sceneAct(name, el, c)) return;
    if (name === "go") return go(c);
    if (name === "open") return c.go("p:stream:stream");
    if (name === "stop") return c.proc.stop(PROC).then((st) => { status = { ...st, running: false }; live = null; c.render(); }).catch(c.fail);
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
    const now = { [PROC]: { args: buildArgs(c) || [], audio: !!c.s.audio } };
    if (JSON.stringify(c.s.procs) !== JSON.stringify(now)) c.save();
    refresh(c).then(() => { if (status.running) watch(c); });
  },
  lang: {
    ko: {
      "Let Glint use your camera?": "Glint가 카메라를 사용해도 될까요?", "Glint opens your camera to show it in the Stream preview. Nothing is recorded or sent until you go live. You can change this in the Stream settings.": "Glint는 방송 미리 보기에 카메라를 보여 주려고 카메라를 열어요. 방송을 시작하기 전에는 아무것도 녹화되거나 전송되지 않아요. 방송 설정에서 바꿀 수 있어요.", "Allow": "허용", "Don't allow": "허용 안 함", "Camera preview is off": "카메라 미리 보기가 꺼져 있어요", "The camera is blocked": "카메라가 차단됐어요", "Glint is not allowed to show your camera in the preview. The camera still goes out when you go live.": "Glint가 미리 보기에 카메라를 보여 줄 수 없어요. 방송을 시작하면 카메라는 그대로 나가요.", "Allow camera": "카메라 허용", "The camera is blocked for Glint. In Windows, open Settings → Privacy & security → Camera and let desktop apps use it, then try again.": "Glint에서 카메라가 차단됐어요. Windows 설정 → 개인 정보 및 보안 → 카메라에서 데스크톱 앱의 카메라 사용을 허용한 뒤 다시 시도해 주세요.", "Try again": "다시 시도", "Camera in the preview": "미리 보기의 카메라", "Whether Glint may open your camera to show it on the Stream page. A camera in your scene goes out on the stream either way": "Glint가 방송 페이지에 카메라를 보여 주려고 카메라를 열어도 되는지 정해요. 장면에 있는 카메라는 어느 쪽이든 방송에 나가요", "Ask me": "물어보기", "Allowed": "허용됨", "Not allowed": "허용 안 됨",
      "Go live from Glint to Twitch, YouTube or any RTMP server, with your screen and cameras arranged in a scene. Adds a Stream page under Tools, its settings, and a keybind to start and stop.": "Glint에서 Twitch, YouTube 또는 RTMP 서버로 방송해요. 화면과 카메라를 장면에 배치할 수 있어요. 도구에 방송 페이지와 설정, 시작/종료 단축키가 추가돼요.", "<b>Preview.</b> Streams your screen, cameras and pictures with Glint's sound. Changes you make while live apply the next time you go live.": "<b>미리 보기.</b> 화면, 카메라, 사진을 Glint의 소리와 함께 방송해요. 방송 중에 바꾼 설정은 다음 방송부터 적용돼요.", "Screen": "화면", "Camera": "카메라", "Pick a camera": "카메라 선택", "On the stream now": "지금 방송에 나가는 중", "No preview of this camera": "이 카메라는 미리 볼 수 없어요", "Bring forward": "앞으로 가져오기", "Send back": "뒤로 보내기", "Remove": "제거", "Click a source to move, size, crop or turn it.": "소스를 클릭하면 옮기고, 크기를 바꾸고, 자르고, 돌릴 수 있어요.", "No camera found": "카메라를 찾지 못했어요", "Looking for cameras…": "카메라 찾는 중…", "Refresh": "새로 고침", "Position": "위치", "Size": "크기", "Turn": "회전", "Mirror": "좌우 반전", "Crop": "자르기", "L": "좌", "T": "상", "R": "우", "B": "하", "Opacity": "불투명도", "Fit": "맞추기", "Fill": "채우기", "Reset": "초기화", "Scene": "장면", "The live picture of the screen and the cameras. Off uses nothing: the boxes can still be moved": "화면과 카메라의 실시간 모습이에요. 끄면 자원을 쓰지 않고, 상자는 계속 옮길 수 있어요", "Preview on": "미리 보기 켜짐", "Preview off": "미리 보기 꺼짐", "Drag to move. Drag a corner to size, with Shift to stretch, with Alt to crop. Drop a PNG, JPEG or GIF here to add it.": "끌어서 옮겨요. 모서리를 끌면 크기가 바뀌고, Shift를 누르면 늘어나고, Alt를 누르면 잘려요. PNG, JPEG, GIF를 여기에 끌어다 놓으면 추가돼요.", "Blur": "블러", "Picture": "사진", "Strength": "강도", "Only PNG, JPEG and GIF pictures can be added": "PNG, JPEG, GIF 사진만 추가할 수 있어요", "{name} could not be opened as a picture": "{name}을(를) 사진으로 열 수 없어요",
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
      "Send Glint's sound": "Glint의 소리 보내기", "Off: the stream is silent": "꺼짐: 방송에 소리가 없어요",
      "Desktop audio and the microphone are both off in Settings, so the stream would be silent": "설정에서 데스크톱 오디오와 마이크가 모두 꺼져 있어서 방송에 소리가 없어요",
      "Clipping is paused, and the sound comes from the same capture: the stream is silent until you resume": "클리핑이 일시 정지됐어요. 소리는 같은 캡처에서 오기 때문에 다시 시작할 때까지 방송에 소리가 없어요",
      "Desktop sound and your microphone, as set in Settings → Audio and sounds": "설정 → 오디오와 소리에서 정한 데스크톱 소리와 마이크",
      "Start and stop with a key": "키로 시작하고 종료하기", "Set on the Keybinds page. It works while Glint's window is closed too": "단축키 페이지에서 정해요. Glint 창이 닫혀 있어도 작동해요",
      "Adds “Start / stop streaming” to your keybinds, where you pick its key": "단축키에 “방송 시작 / 종료”를 추가해요. 거기서 키를 정하면 돼요",
      "Open Keybinds": "단축키 열기", "Add keybind": "단축키 추가", "Preview": "미리 보기",
    },
    el: {
      "Let Glint use your camera?": "Να χρησιμοποιήσει το Glint την κάμερά σου;", "Glint opens your camera to show it in the Stream preview. Nothing is recorded or sent until you go live. You can change this in the Stream settings.": "Το Glint ανοίγει την κάμερά σου για να τη δείξει στην προεπισκόπηση της Μετάδοσης. Τίποτα δεν καταγράφεται ούτε στέλνεται μέχρι να ξεκινήσεις μετάδοση. Μπορείς να το αλλάξεις στις ρυθμίσεις της Μετάδοσης.", "Allow": "Να επιτρέπεται", "Don't allow": "Να μην επιτρέπεται", "Camera preview is off": "Η προεπισκόπηση κάμερας είναι ανενεργή", "The camera is blocked": "Η κάμερα είναι αποκλεισμένη", "Glint is not allowed to show your camera in the preview. The camera still goes out when you go live.": "Το Glint δεν επιτρέπεται να δείξει την κάμερά σου στην προεπισκόπηση. Η κάμερα μεταδίδεται κανονικά όταν ξεκινήσεις μετάδοση.", "Allow camera": "Να επιτρέπεται η κάμερα", "The camera is blocked for Glint. In Windows, open Settings → Privacy & security → Camera and let desktop apps use it, then try again.": "Η κάμερα είναι αποκλεισμένη για το Glint. Στα Windows άνοιξε Ρυθμίσεις → Απόρρητο και ασφάλεια → Κάμερα, επίτρεψε στις εφαρμογές υπολογιστή να τη χρησιμοποιούν και δοκίμασε ξανά.", "Try again": "Δοκίμασε ξανά", "Camera in the preview": "Κάμερα στην προεπισκόπηση", "Whether Glint may open your camera to show it on the Stream page. A camera in your scene goes out on the stream either way": "Αν το Glint μπορεί να ανοίγει την κάμερά σου για να τη δείχνει στη σελίδα Μετάδοση. Μια κάμερα της σκηνής μεταδίδεται σε κάθε περίπτωση", "Ask me": "Να ερωτώμαι", "Allowed": "Επιτρέπεται", "Not allowed": "Δεν επιτρέπεται",
      "Go live from Glint to Twitch, YouTube or any RTMP server, with your screen and cameras arranged in a scene. Adds a Stream page under Tools, its settings, and a keybind to start and stop.": "Ζωντανή μετάδοση από το Glint σε Twitch, YouTube ή οποιονδήποτε διακομιστή RTMP, με την οθόνη και τις κάμερές σου τοποθετημένες σε σκηνή. Προσθέτει τη σελίδα Μετάδοση στα Εργαλεία, τις ρυθμίσεις της και συντόμευση για έναρξη και διακοπή.", "<b>Preview.</b> Streams your screen, cameras and pictures with Glint's sound. Changes you make while live apply the next time you go live.": "<b>Προεπισκόπηση.</b> Μεταδίδει την οθόνη, τις κάμερες και τις εικόνες σου με τον ήχο του Glint. Οι αλλαγές κατά τη μετάδοση ισχύουν από την επόμενη μετάδοση.", "Screen": "Οθόνη", "Camera": "Κάμερα", "Pick a camera": "Διάλεξε κάμερα", "On the stream now": "Στη μετάδοση τώρα", "No preview of this camera": "Χωρίς προεπισκόπηση για αυτή την κάμερα", "Bring forward": "Μεταφορά μπροστά", "Send back": "Μεταφορά πίσω", "Remove": "Αφαίρεση", "Click a source to move, size, crop or turn it.": "Κάνε κλικ σε μια πηγή για να τη μετακινήσεις, να αλλάξεις το μέγεθός της, να την περικόψεις ή να τη γυρίσεις.", "No camera found": "Δεν βρέθηκε κάμερα", "Looking for cameras…": "Αναζήτηση καμερών…", "Refresh": "Ανανέωση", "Position": "Θέση", "Size": "Μέγεθος", "Turn": "Περιστροφή", "Mirror": "Καθρέφτης", "Crop": "Περικοπή", "L": "Α", "T": "Π", "R": "Δ", "B": "Κ", "Opacity": "Αδιαφάνεια", "Fit": "Προσαρμογή", "Fill": "Γέμισμα", "Reset": "Επαναφορά", "Scene": "Σκηνή", "The live picture of the screen and the cameras. Off uses nothing: the boxes can still be moved": "Η ζωντανή εικόνα της οθόνης και των καμερών. Όταν είναι ανενεργή δεν χρησιμοποιεί τίποτα: τα πλαίσια μετακινούνται ακόμα", "Preview on": "Προεπισκόπηση ενεργή", "Preview off": "Προεπισκόπηση ανενεργή", "Drag to move. Drag a corner to size, with Shift to stretch, with Alt to crop. Drop a PNG, JPEG or GIF here to add it.": "Σύρε για μετακίνηση. Σύρε μια γωνία για αλλαγή μεγέθους, με Shift για τέντωμα, με Alt για περικοπή. Άφησε εδώ ένα PNG, JPEG ή GIF για να το προσθέσεις.", "Blur": "Θόλωμα", "Picture": "Εικόνα", "Strength": "Ένταση", "Only PNG, JPEG and GIF pictures can be added": "Μόνο εικόνες PNG, JPEG και GIF μπορούν να προστεθούν", "{name} could not be opened as a picture": "Το {name} δεν άνοιξε ως εικόνα",
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
      "Send Glint's sound": "Αποστολή του ήχου του Glint", "Off: the stream is silent": "Ανενεργό: η μετάδοση δεν έχει ήχο",
      "Desktop audio and the microphone are both off in Settings, so the stream would be silent": "Ο ήχος επιφάνειας εργασίας και το μικρόφωνο είναι ανενεργά στις Ρυθμίσεις, άρα η μετάδοση δεν θα έχει ήχο",
      "Clipping is paused, and the sound comes from the same capture: the stream is silent until you resume": "Η καταγραφή κλιπ είναι σε παύση και ο ήχος έρχεται από την ίδια καταγραφή: η μετάδοση δεν έχει ήχο μέχρι να συνεχίσεις",
      "Desktop sound and your microphone, as set in Settings → Audio and sounds": "Ήχος επιφάνειας εργασίας και μικρόφωνο, όπως ορίζονται στις Ρυθμίσεις → Ήχος και ήχοι",
      "Start and stop with a key": "Έναρξη και διακοπή με πλήκτρο", "Set on the Keybinds page. It works while Glint's window is closed too": "Ορίζεται στη σελίδα Συντομεύσεις. Δουλεύει και όταν το παράθυρο του Glint είναι κλειστό",
      "Adds “Start / stop streaming” to your keybinds, where you pick its key": "Προσθέτει το «Έναρξη / διακοπή μετάδοσης» στις συντομεύσεις σου, όπου διαλέγεις το πλήκτρο",
      "Open Keybinds": "Άνοιγμα Συντομεύσεων", "Add keybind": "Προσθήκη συντόμευσης", "Preview": "Προεπισκόπηση",
    },
  },
});
