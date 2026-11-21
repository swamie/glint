// Stream: go live from Glint. An official Glint plugin (plugins/official/stream in Glint's repository).
//
// What it does: runs Glint's ffmpeg as the process "live", which captures a monitor (Desktop
// Duplication), encodes it with the graphics card's encoder and sends it to an RTMP server.
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
  let v = `ddagrab=output_idx=${mon.index}:framerate=${fps}:draw_mouse=${c.glint.settings.cursor === false ? 0 : 1}`;
  if (scaled) v += `,hwdownload,format=bgra,scale=${w}:${h}:flags=bilinear${enc === "x264" ? "" : ",format=nv12"}`;
  else if (enc === "x264") v += ",hwdownload,format=bgra";
  else if (enc === "qsv") v += ",hwmap=derive_device=qsv,format=qsv";
  const rate = `${kbps}k`, buf = `${kbps * 2}k`;
  const a = ["-hide_banner", "-loglevel", "error", "-stats", "-stats_period", "2", "-nostdin"];
  // Sound: Glint's live audio on stdin ("{audio_rate}" is filled in by Glint), or silence.
  if (s.audio) a.push("-f", "s16le", "-ar", "{audio_rate}", "-ac", "2", "-thread_queue_size", "1024", "-i", "pipe:0");
  else a.push("-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo");
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
const errors = () => (status.log || []).filter((l) => !l.startsWith("frame=") && !l.startsWith("size="));

async function refresh(c, redraw = true) {
  try { status = await c.proc.status(PROC); } catch (e) { return; }
  if (redraw && c.glint && isOpen()) paintLive(c);
}
const isOpen = () => !!document.querySelector('.plug-page[data-plugin="stream"]');
/** The live line and the head's button, on the page as drawn (no redraw while the user is in a field). */
function paintLive(c) {
  const el = document.querySelector('.plug-page[data-plugin="stream"] .st-head');
  if (el) el.innerHTML = headHtml(c);
  const log = document.querySelector('.plug-page[data-plugin="stream"] .st-status');
  if (log) log.outerHTML = statusHtml(c);
}
function watch(c) {
  clearInterval(poll);
  poll = setInterval(() => { if (!status.running) return clearInterval(poll); if (isOpen()) refresh(c); }, 1000);
}

function headHtml(c) {
  if (status.running) {
    const st = stats();
    return `<span class="plug-live">${st ? t("LIVE") : t("Connecting…")} ${uptime()}</span>${st ? `<span class="small muted">${esc(st)}</span>` : ""}<button class="btn solid-danger" data-pact="stop">${t("Stop streaming")}</button>`;
  }
  const ready = !!buildArgs(c);
  return `<button class="btn primary" data-pact="go"${ready ? "" : " disabled"} title="${esc(ready ? "" : t("Choose where to stream and paste your stream key first"))}">${t("Go live")}</button>`;
}
function statusHtml(c) {
  const lines = errors();
  if (!status.error && !(status.running && lines.length)) return `<div class="st-status" hidden></div>`;
  return `<div class="st-status note${status.error ? " bad" : ""}">${status.error ? `<b>${t("The stream stopped.")}</b> ${esc(status.error)}` : esc(lines.slice(-3).join("\n"))}</div>`;
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
function audioRows(c) {
  const g = c.glint;
  const none = !g.settings.desktop_audio && !g.settings.mic;
  const hint = !c.s.audio ? t("Off: the stream is silent")
    : none ? t("Desktop audio and the microphone are both off in Settings, so the stream would be silent")
    : !(g.status && g.status.recording) ? t("Clipping is paused, and the sound comes from the same capture: the stream is silent until you resume")
    : t("Desktop sound and your microphone, as set in Settings → Audio and sounds");
  return c.row(t("Send Glint's sound"), hint, c.toggle("audio"));
}

function render(c) {
  const hasBind = (c.glint.settings.keybinds || []).some((k) => k.action === "plugin:stream:toggle");
  return `<div class="page-head"><div><h1>${t("Stream")}</h1><div class="sub">${t("Go live from Glint")}</div></div><div class="head-actions st-head">${headHtml(c)}</div></div>
    <div class="note">${t("<b>Preview.</b> This first version streams one whole monitor with Glint's sound. Changes you make while live apply the next time you go live.")}</div>
    ${statusHtml(c)}
    <div class="section"><h2>${t("Where to")}</h2>${whereRows(c)}</div>
    <div class="section"><h2>${t("Video")}</h2>${videoRows(c)}</div>
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
    status = await c.proc.start(PROC, args, { audio: !!c.s.audio });
    watch(c);
    c.render();
  } catch (e) { c.fail(e); }
}

Glint.register({
  defaults: { service: "twitch", url: "", key: "", monitor: -1, height: 1080, fps: 60, bitrate: 6000, encoder: "auto", audio: true, procs: {} },
  pages: [{
    id: "stream",
    title: "Stream",
    icon: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="2.200"/><path d="M7.800 16.200a6 6 0 0 1 0-8.400M16.200 7.800a6 6 0 0 1 0 8.400M5 19A10 10 0 0 1 5 5M19 5a10 10 0 0 1 0 14"/></svg>',
    render,
    after: (c) => { refresh(c).then(() => { if (status.running) watch(c); }); },
    badge: () => (status.running ? t("LIVE") : ""),
  }],
  settings: [
    { title: "Stream", render: (c) => whereRows(c) + videoRows(c) + audioRows(c) },
  ],
  // The backend has already started or stopped the process when a keybind fires: only the page follows.
  actions: { toggle: (c) => refresh(c, false).then(() => { if (status.running) watch(c); c.render(); }) },
  onProc: (st, c) => { if (st.name !== PROC) return; status = st; if (st.running) watch(c); c.render(); },
  onSave: (c) => { c.s.procs = { [PROC]: { args: buildArgs(c) || [], audio: !!c.s.audio } }; },
  onAct: (name, el, c) => {
    if (name === "go") return go(c);
    if (name === "stop") return c.proc.stop(PROC).then((st) => { status = { ...st, running: false }; c.render(); }).catch(c.fail);
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
    refresh(c, false).then(() => { if (status.running) watch(c); c.render(); });
  },
  lang: {
    ko: {
      "Stream": "방송", "Go live from Glint": "Glint에서 바로 방송하기", "Go live": "방송 시작", "Stop streaming": "방송 종료", "LIVE": "방송 중", "Connecting…": "연결 중…",
      "Start / stop streaming": "방송 시작 / 종료", "Stream started": "방송을 시작했어요", "Stream stopped": "방송을 종료했어요",
      "Go live from Glint to Twitch, YouTube or any RTMP server. Adds a Stream page under Tools, its settings, and a keybind to start and stop.": "Glint에서 Twitch, YouTube 또는 RTMP 서버로 방송해요. 도구에 방송 페이지와 설정, 시작/종료 단축키가 추가돼요.",
      "Choose where to stream and paste your stream key first": "먼저 방송할 곳을 고르고 스트림 키를 붙여 넣어 주세요",
      "The stream stopped.": "방송이 중단됐어요.",
      "<b>Preview.</b> This first version streams one whole monitor with Glint's sound. Changes you make while live apply the next time you go live.": "<b>미리 보기.</b> 첫 버전은 모니터 한 대 전체를 Glint의 소리와 함께 방송해요. 방송 중에 바꾼 설정은 다음 방송부터 적용돼요.",
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
      "Stream": "Μετάδοση", "Go live from Glint": "Ζωντανή μετάδοση από το Glint", "Go live": "Έναρξη μετάδοσης", "Stop streaming": "Διακοπή μετάδοσης", "LIVE": "ΖΩΝΤΑΝΑ", "Connecting…": "Σύνδεση…",
      "Start / stop streaming": "Έναρξη / διακοπή μετάδοσης", "Stream started": "Η μετάδοση ξεκίνησε", "Stream stopped": "Η μετάδοση σταμάτησε",
      "Go live from Glint to Twitch, YouTube or any RTMP server. Adds a Stream page under Tools, its settings, and a keybind to start and stop.": "Ζωντανή μετάδοση από το Glint σε Twitch, YouTube ή οποιονδήποτε διακομιστή RTMP. Προσθέτει τη σελίδα Μετάδοση στα Εργαλεία, τις ρυθμίσεις της και συντόμευση για έναρξη και διακοπή.",
      "Choose where to stream and paste your stream key first": "Διάλεξε πρώτα πού θα μεταδώσεις και επικόλλησε το κλειδί μετάδοσης",
      "The stream stopped.": "Η μετάδοση σταμάτησε.",
      "<b>Preview.</b> This first version streams one whole monitor with Glint's sound. Changes you make while live apply the next time you go live.": "<b>Προεπισκόπηση.</b> Αυτή η πρώτη έκδοση μεταδίδει μία ολόκληρη οθόνη με τον ήχο του Glint. Οι αλλαγές κατά τη μετάδοση ισχύουν από την επόμενη μετάδοση.",
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
