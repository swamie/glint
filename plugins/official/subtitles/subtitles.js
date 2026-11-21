// Subtitles: what is said in a clip, written on the picture. An official Glint plugin
// (plugins/official/subtitles in Glint's repository).
//
// What it does: adds a "Subtitles" section to the editor. "Make subtitles" writes the sound of the
// clips on the timeline to a WAV with Glint's ffmpeg, has whisper.cpp turn the speech into timed
// lines, and puts each line on the picture as an ordinary text item (centred, marked with the tag
// "subtitles"). So the lines are on the timeline, can be dragged, retimed and retyped like any
// text, are saved with a project, are undone with Ctrl+Z, and are part of the exported clip
// without anything new in the export. "Save .srt" writes them as a subtitle file, timed for the
// finished clip.
//
// Everything runs on this PC. whisper.cpp and the speech model are not part of the plugin: they
// are its downloads (plugin.json "downloads"), fetched the first time subtitles are made and
// checked against the SHA-256 written there. Nothing is uploaded.
const { ctx, t, esc } = Glint;
const TAG = "subtitles";
const MODELS = {
  base: { dl: "model-base", name: "Fast" },
  small: { dl: "model-small", name: "Better" },
  turbo: { dl: "model-turbo", name: "Best, slower" },
};
// whisper's language codes, with each language's own name.
const LANGS = [
  ["auto", "Detect"], ["en", "English"], ["ko", "한국어"], ["el", "Ελληνικά"], ["es", "Español"], ["fr", "Français"], ["de", "Deutsch"],
  ["pt", "Português"], ["it", "Italiano"], ["nl", "Nederlands"], ["pl", "Polski"], ["tr", "Türkçe"], ["ru", "Русский"], ["uk", "Українська"],
  ["sv", "Svenska"], ["da", "Dansk"], ["no", "Norsk"], ["fi", "Suomi"], ["cs", "Čeština"], ["ro", "Română"], ["hu", "Magyar"],
  ["ja", "日本語"], ["zh", "中文"], ["hi", "हिन्दी"], ["ar", "العربية"], ["id", "Bahasa Indonesia"], ["vi", "Tiếng Việt"], ["th", "ไทย"],
];
const COLORS = ["#ffffff", "#ffe14d", "#7df9ff", "#7dff9b", "#ff8fb1", "#111111"];
const LINE_H = 1.2;      // a text item's line height, times its size (as Glint draws it)
const MAX_W = 0.9;       // the widest a line may be, as a fraction of the picture
const EDGE = 0.07;       // how far from the top or bottom edge the lines sit
const STYLE_KEYS = ["size", "place", "color", "outline"];
const CANCELLED = {};

const st = {
  run: null,   // while subtitles are being made: { label, pct (null = unknown), cancel, dl, proc }
  dls: null,   // the plugin's downloads: [{ id, name, size, installed }]
  error: "",
};

/* ---------- pure helpers (tested under Node: tools/subtitles-test.js) ---------- */

const clamp = (x, lo, hi) => Math.min(hi, Math.max(lo, x));
const r3 = (x) => Math.round(x * 1000) / 1000;
const mb = (bytes) => `${Math.max(1, Math.round(bytes / 1048576))} MB`;

/** "1:02.5" for 62.5 seconds. */
function fmtTime(sec) {
  const s = Math.max(0, Math.round((+sec || 0) * 10) / 10), mins = Math.floor(s / 60), rest = s - mins * 60;
  return `${mins}:${rest < 10 ? "0" : ""}${rest.toFixed(1)}`;
}

/** "00:01:02,500" for 62.5 seconds (an .srt file's times). */
function srtTime(sec) {
  const ms = Math.max(0, Math.round((+sec || 0) * 1000));
  const p = (n, w = 2) => String(n).padStart(w, "0");
  return `${p(Math.floor(ms / 3600000))}:${p(Math.floor(ms / 60000) % 60)}:${p(Math.floor(ms / 1000) % 60)},${p(ms % 1000, 3)}`;
}

/** An .srt file of `cues` ([{ from, to, text }], seconds of the finished clip). */
function srtText(cues) {
  return cues.map((c, i) => `${i + 1}\n${srtTime(c.from)} --> ${srtTime(c.to)}\n${String(c.text).split("\n").map((l) => l.trim()).filter(Boolean).join("\n")}\n`).join("\n");
}

/** A line as whisper gave it, tidied: "" for silence and sounds ("[BLANK_AUDIO]", "(music)", "♪"). */
function cleanText(text) {
  const s = String(text ?? "").replace(/\s+/g, " ").trim();
  if (!s || /^[\[(*♪].*[\])*♪]$/.test(s) || !/[\p{L}\p{N}]/u.test(s)) return "";
  return s;
}

/** whisper-cli's JSON (-oj) as [{ a, b, text }] in seconds of the file, when the WAV started at second `lo` of it. */
function parseWhisper(json, lo) {
  const v = typeof json === "string" ? JSON.parse(json) : json;
  const out = [];
  for (const s of (v && v.transcription) || []) {
    const a = +s?.offsets?.from, b = +s?.offsets?.to, text = cleanText(s?.text);
    if (!Number.isFinite(a) || !Number.isFinite(b) || !(b > a) || !text) continue;
    out.push({ a: lo + a / 1000, b: lo + b / 1000, text });
  }
  return out;
}

/** The percent in the last "progress = 40%" line of whisper-cli's messages, or null. */
function progressOf(log) {
  for (let i = (log || []).length - 1; i >= 0; i--) {
    const m = /progress\s*=\s*(\d+)\s*%/.exec(log[i]);
    if (m) return clamp(+m[1], 0, 100);
  }
  return null;
}

/** The files the timeline's footage comes from, each with the stretch of it that is used: [{ path, lo, hi, tracks }]. */
function filesOf(foot) {
  const out = [];
  for (const f of foot) {
    const end = f.from + f.len * (f.speed || 1);
    const have = out.find((x) => x.path === f.path);
    if (have) { have.lo = Math.min(have.lo, f.from); have.hi = Math.max(have.hi, end); if (!have.tracks.length) have.tracks = f.tracks || []; }
    else out.push({ path: f.path, lo: f.from, hi: end, tracks: f.tracks || [] });
  }
  return out;
}

/** The audio stream of a file to listen to. Glint's clips name their tracks: "Mix" (everything), "Desktop", "Microphone", "Game". */
function pickStream(tracks, listen) {
  const by = (re) => (tracks || []).find((x) => re.test(String(x.title || "")));
  const all = by(/^mix$/i);
  const hit = listen === "mic" ? by(/^mic/i) : listen === "game" ? by(/^game/i) || by(/^desktop/i) : null;
  return (hit || all || { index: 0 }).index; // stream 0 is the mix of a clip of Glint's, and the only sound of any other file
}

/**
 * Lines heard in the files (`segs`: path -> [{ a, b, text }], seconds of the file) as cues on the timeline, in
 * order: [{ from, to, text }]. `foot` is the editor's footage (which stretch of which file each part of the
 * timeline shows, and how fast). A line that a cut takes most of is left out, and so is one in a part that is
 * cut out of the result (`removedAt(sec)`).
 */
function cuesOnTimeline(foot, segs, removedAt) {
  const out = [];
  for (const f of foot) {
    const sp = f.speed || 1, end = f.from + f.len * sp;
    for (const s of segs[f.path] || []) {
      const a = Math.max(s.a, f.from), b = Math.min(s.b, end);
      if (b - a <= 0 || b - a < Math.min(0.4, (s.b - s.a) / 2)) continue;
      const from = f.at + (a - f.from) / sp, to = f.at + (b - f.from) / sp;
      if (removedAt && removedAt((from + to) / 2)) continue;
      out.push({ from, to, text: s.text });
    }
  }
  out.sort((x, y) => x.from - y.from);
  // One line at a time: a line ends where the next begins.
  for (let i = 0; i < out.length; i++) {
    const next = out[i + 1];
    if (next && out[i].to > next.from) out[i].to = next.from;
    out[i].from = r3(out[i].from);
    out[i].to = r3(Math.max(out[i].to, out[i].from + 0.1));
  }
  return out;
}

/** `text` on as few lines as fit (`fits(line)`), at most three, the first two as even as they can be. */
function wrap(text, fits) {
  const words = String(text).replace(/\s+/g, " ").trim().split(" ").filter(Boolean);
  const whole = words.join(" ");
  if (words.length < 2 || fits(whole)) return [whole];
  // Two lines: the break that leaves them most alike in length.
  let best = null;
  for (let i = 1; i < words.length; i++) {
    const a = words.slice(0, i).join(" "), b = words.slice(i).join(" ");
    if (!fits(a) || !fits(b)) continue;
    const d = Math.abs(a.length - b.length);
    if (!best || d < best.d) best = { d, lines: [a, b] };
  }
  if (best) return best.lines;
  // Longer: fill line after line; what is left stays on the third.
  const lines = [];
  let cur = "";
  for (const w of words) {
    const next = cur ? `${cur} ${w}` : w;
    if (cur && !fits(next) && lines.length < 2) { lines.push(cur); cur = w; } else cur = next;
  }
  lines.push(cur);
  return lines;
}

/** Where a subtitle of `n` lines sits: the `y` (fraction of the picture) of its top. */
function placeY(n, size, place) {
  const h = n * size * LINE_H;
  return r3(place === "top" ? EDGE : place === "middle" ? 0.5 - h / 2 : 1 - EDGE - h);
}

/** The style in the plugin's settings, made safe. */
function styleOf(s) {
  return {
    size: clamp(Math.round(+s.size || 5), 2, 12) / 100,
    place: ["bottom", "middle", "top"].includes(s.place) ? s.place : "bottom",
    color: /^#[0-9a-f]{6}$/i.test(s.color || "") ? s.color : "#ffffff",
    outline: s.outline !== false,
  };
}

/** A text item of the picture for one cue. `width(line, size)`: a line's width as a fraction of the picture's. */
function itemOf(cue, style, width, id) {
  const lines = wrap(cue.text, (l) => width(l, style.size) <= MAX_W);
  return { id, kind: "text", from: cue.from, to: cue.to, x: 0.5, y: placeY(lines.length, style.size, style.place), text: lines.join("\n"), color: style.color, size: style.size, outline: style.outline, align: "center", tag: TAG };
}

const newId = () => "o" + Date.now().toString(36) + Math.random().toString(36).slice(2, 7);
const plain = (text) => String(text || "").replace(/\s*\n\s*/g, " ").trim();

/* ---------- making the subtitles ---------- */

const modelOf = (c) => (MODELS[c.s.model] ? c.s.model : "base");
const langOf = (c) => (LANGS.some(([k]) => k === c.s.language) ? c.s.language : "auto");
/** Half the processor, so Glint keeps clipping and a game keeps running meanwhile. */
const threads = () => clamp(Math.floor((navigator.hardwareConcurrency || 4) / 2), 2, 8);
/** What "Make subtitles" still has to download: [{ id, name, size }]. */
const missing = (c) => (st.dls || []).filter((d) => (d.id === "whisper" || d.id === MODELS[modelOf(c)].dl) && !d.installed);

async function refreshDls(c) {
  try { st.dls = await c.dl.state(); } catch (e) { console.warn("Glint subtitles: the downloads could not be listed", e); }
}

async function make(ed, c) {
  if (st.run || !ed.model) return;
  const run = (st.run = { label: t("Getting ready…"), pct: null, cancel: false, dl: "", proc: "" });
  st.error = "";
  const show = (change) => { Object.assign(run, change); ed.redraw(); };
  const check = () => { if (run.cancel) throw CANCELLED; };
  show({});
  try {
    // 1. whisper.cpp and the speech model, the first time.
    await refreshDls(c);
    for (const d of missing(c)) {
      show({ dl: d.id, pct: 0, label: d.id === "whisper" ? t("Downloading whisper.cpp…") : t("Downloading the speech model…") });
      st.dls = await c.dl.get(d.id);
      show({ dl: "" });
      check();
    }
    // 2. The sound of each file on the timeline, then its speech.
    const foot = ed.footage().filter((f) => !f.missing && f.path);
    const files = filesOf(foot);
    if (!files.length) throw t("There is no clip on the timeline to listen to.");
    await c.work.clear();
    const segs = {};
    for (let i = 0; i < files.length; i++) {
      const f = files[i], name = `a${i}`;
      show({ proc: "sound", pct: null, label: t("Reading the sound…") });
      let r = await c.proc.run("sound", ["-v", "error", "-y", "-ss", f.lo.toFixed(3), "-t", Math.max(0.1, f.hi - f.lo).toFixed(3), "-i", f.path,
        "-map", `0:a:${pickStream(f.tracks, c.s.listen)}`, "-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", `{work}\\${name}.wav`]);
      check();
      if (r.code !== 0) throw r.error || t("The clip's sound could not be read.");
      const part = (pct) => ((i + pct / 100) / files.length) * 100;
      show({ proc: "speech", pct: part(0), label: files.length > 1 ? t("Listening… (clip {n} of {total})", { n: i + 1, total: files.length }) : t("Listening…") });
      const args = ["-m", `{dl:${MODELS[modelOf(c)].dl}}`, "-f", `{work}\\${name}.wav`, "-oj", "-of", `{work}\\${name}`, "-l", langOf(c), "-t", String(threads()), "-pp", "-ml", "60", "-sow"];
      if (c.s.translate) args.push("-tr");
      r = await c.proc.run("speech", args, { tool: "whisper", onTick: (now) => { const p = progressOf(now.log); if (p !== null && !run.cancel) show({ pct: part(p) }); } });
      check();
      if (r.code !== 0) throw r.error || t("The speech could not be read.");
      segs[f.path] = parseWhisper(await c.work.read(`${name}.json`), f.lo);
    }
    // 3. Onto the picture, in place of the subtitles that were there: one undo step.
    const cues = cuesOnTimeline(foot, segs, (sec) => ed.removedAt(sec));
    if (!cues.length) throw t("No speech was found in this clip.");
    const style = styleOf(c.s);
    ed.change((m) => {
      m.overlays = (m.overlays || []).filter((o) => !(o.kind === "text" && o.tag === TAG));
      for (const cue of cues) m.overlays.push(itemOf(cue, style, ed.textWidth, newId()));
    });
    c.flash(t("{n} subtitle lines added", { n: cues.length }));
  } catch (e) {
    if (e !== CANCELLED && !run.cancel) st.error = String(e?.message ?? e);
  }
  st.run = null;
  c.work.clear().catch(() => {});
  await refreshDls(c);
  ed.redraw();
}

function cancel(c) {
  const run = st.run;
  if (!run) return;
  run.cancel = true;
  run.label = t("Stopping…");
  if (run.dl) c.dl.cancel(run.dl).catch(() => {});
  if (run.proc) c.proc.stop(run.proc).catch(() => {});
  c.editor()?.redraw();
}

/* ---------- changing them ---------- */

/** The style controls changed: every line takes the new look (and its place for it). */
function restyle(ed, c) {
  const style = styleOf(c.s);
  ed.change((m) => {
    for (const o of m.overlays || []) {
      if (o.kind !== "text" || o.tag !== TAG) continue;
      const it = itemOf({ from: o.from, to: o.to, text: plain(o.text) }, style, ed.textWidth, o.id);
      Object.assign(o, { x: it.x, y: it.y, text: it.text, color: it.color, size: it.size, outline: it.outline, align: "center" });
    }
  });
}

/** A line was retyped in the list. Emptied: the line goes. */
function retype(ed, id, value) {
  const text = String(value || "").replace(/\s+/g, " ").trim();
  ed.change((m) => {
    const o = (m.overlays || []).find((x) => x.id === id);
    if (!o) return;
    if (!text) { m.overlays = m.overlays.filter((x) => x.id !== id); return; }
    const before = String(o.text).split("\n").length;
    const lines = wrap(text, (l) => ed.textWidth(l, o.size) <= MAX_W);
    o.text = lines.join("\n");
    // Its bottom edge stays where it is (its middle, for a centred one) when the number of lines changes.
    const grew = (lines.length - before) * o.size * LINE_H, place = styleOf(ctx.s).place;
    if (grew && place !== "top") o.y = r3(o.y - (place === "middle" ? grew / 2 : grew));
  });
}

async function saveSrt(ed, c) {
  const cues = ed.texts(TAG).map((o) => { const r = ed.outRange(o.from, o.to); return r && plain(o.text) ? { from: r.from, to: r.to, text: plain(o.text) } : null; }).filter(Boolean);
  if (!cues.length) return c.flash(t("There are no subtitles to save."), true);
  try {
    const name = `${String(ed.clip.title || "subtitles").replace(/[\\/:*?"<>|]+/g, " ").trim() || "subtitles"}.srt`;
    const path = await c.saveText(name, srtText(cues));
    if (path) c.flash(t("Saved {name}", { name: String(path).split(/[\\/]/).pop() }));
  } catch (e) { c.fail(e); }
}

/* ---------- the section ---------- */

const X_ICON = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18"/></svg>';
const sel = (key, value, opts) => `<select class="field" data-sub-set="${key}">${opts.map(([v, label]) => `<option value="${esc(v)}"${String(v) === String(value) ? " selected" : ""}>${esc(label)}</option>`).join("")}</select>`;

function render(ed, c) {
  if (!ed.model) return "";
  const lines = ed.texts(TAG), s = c.s, style = styleOf(s), now = ed.time();
  let html = "";
  if (st.run) {
    const pct = st.run.pct;
    html += `<div class="sub-prog" role="status">
      <div class="sub-prog-top"><b>${esc(st.run.label)}</b>${pct === null ? "" : `<span>${Math.round(pct)}%</span>`}</div>
      <div class="progress${pct === null ? " wait" : ""}"><i style="width:${pct === null ? 30 : Math.round(pct)}%"></i></div>
      <div class="sub-btns"><button type="button" class="btn sm" data-pact="cancel"${st.run.cancel ? " disabled" : ""}>${esc(t("Cancel"))}</button></div>
    </div>`;
  } else {
    const need = missing(c), size = need.reduce((n, d) => n + d.size, 0);
    html += `<label class="sub-row"><span>${esc(t("Spoken in"))}</span>${sel("language", langOf(c), LANGS.map(([k, name]) => [k, k === "auto" ? t("Detect the language") : name]))}</label>
      <label class="sub-row"><span>${esc(t("Listen to"))}</span>${sel("listen", s.listen, [["all", t("All the sound")], ["mic", t("My microphone")], ["game", t("The game or desktop")]])}</label>
      <label class="sub-row"><span>${esc(t("Quality"))}</span>${sel("model", modelOf(c), Object.entries(MODELS).map(([k, m]) => [k, t(m.name)]))}</label>
      <label class="check sub-row"><input type="checkbox" data-sub-set="translate"${s.translate ? " checked" : ""}><span>${esc(t("Translate into English"))}</span></label>
      <div class="sub-btns"><button type="button" class="btn sm primary" data-pact="make">${esc(lines.length ? t("Make them again") : t("Make subtitles"))}</button></div>
      <div class="small">${esc(need.length
        ? t("The first time, this downloads {size} (the speech model and whisper.cpp). After that it works without internet. The sound never leaves this PC.", { size: mb(size) })
        : lines.length ? t("Making them again replaces every line below.") : t("Made on this PC. The sound never leaves it."))}</div>`;
  }
  if (st.error) html += `<div class="sub-err" role="alert">${esc(st.error)}</div>`;
  if (lines.length) {
    html += `<div class="sub-list" data-keep-scroll role="list">${lines.map((o) => `<div class="sub-line${now >= o.from && now <= o.to ? " now" : ""}" role="listitem">
        <button type="button" class="sub-at" data-pact="seek" data-id="${esc(o.id)}" title="${esc(t("Go to this line"))}">${fmtTime(o.from)}</button>
        <input type="text" class="sub-text" data-sub-line="${esc(o.id)}" value="${esc(plain(o.text))}" spellcheck="false" aria-label="${esc(t("Subtitle text"))}">
        <button type="button" class="sub-x" data-pact="del" data-id="${esc(o.id)}" title="${esc(t("Delete this line"))}" aria-label="${esc(t("Delete this line"))}">${X_ICON}</button>
      </div>`).join("")}</div>
      <div class="sub-style">
        <div class="e-sliders"><label class="e-slider"><span>${esc(t("Size"))}</span><input type="range" min="2" max="12" step="1" value="${Math.round(style.size * 100)}" data-sub-set="size"><output>${Math.round(style.size * 100)}%</output></label></div>
        <label class="sub-row"><span>${esc(t("Place"))}</span>${sel("place", style.place, [["bottom", t("Bottom")], ["middle", t("Middle")], ["top", t("Top")]])}</label>
        <div class="sub-row"><span>${esc(t("Color"))}</span><div class="sub-sw">${COLORS.map((col) => `<button type="button" class="${col === style.color.toLowerCase() ? "on" : ""}" data-pact="color" data-color="${col}" style="--sw:${col}" aria-label="${col}" title="${col}"></button>`).join("")}</div></div>
        <label class="check sub-row"><input type="checkbox" data-sub-set="outline"${style.outline ? " checked" : ""}><span>${esc(t("Dark outline"))}</span></label>
      </div>
      <div class="sub-btns">
        <button type="button" class="btn sm" data-pact="srt">${esc(t("Save .srt file"))}</button>
        <button type="button" class="btn sm danger" data-pact="clear">${esc(t("Remove all"))}</button>
      </div>
      <div class="small">${esc(t("The lines are part of the exported clip. Each is also a text on the picture: move or retime it there or on the timeline."))}</div>`;
  }
  return html;
}

const mySection = (el) => el?.closest?.('.e-plug[data-plugin="subtitles"]');

/** A select, check box or slider of the section changed: the setting is saved; a style control restyles the lines. */
function onChange(e) {
  const el = e.target, ed = ctx.editor();
  if (!mySection(el) || !ed) return;
  if (el.dataset.subLine) return retype(ed, el.dataset.subLine, el.value);
  const key = el.dataset.subSet;
  if (!key) return;
  ctx.s[key] = el.type === "checkbox" ? el.checked : el.type === "range" ? +el.value : el.value;
  ctx.save();
  el.blur?.();
  if (STYLE_KEYS.includes(key) && ed.texts(TAG).length) restyle(ed, ctx);
  if (key === "model") refreshDls(ctx).then(() => ed.redraw());
  ed.redraw();
}

Glint.register({
  defaults: { model: "base", language: "auto", listen: "all", translate: false, size: 5, place: "bottom", color: "#ffffff", outline: true },
  editor: {
    title: "Subtitles",
    tab: "overlays",
    render,
    onOpen: (ed, c) => { st.error = ""; refreshDls(c).then(() => ed.redraw()); },
    onClose: (ed, c) => cancel(c),
  },
  settings: [{
    title: "Subtitles",
    render: (c) => {
      const list = (st.dls || []).filter((d) => d.installed);
      return c.row(t("Downloaded files"), t("whisper.cpp and the speech models the Subtitles plugin fetched. Removing one frees its space; it is downloaded again when it is needed."), "")
        + (list.length ? list.map((d) => `<div class="sub-dl"><div>${esc(t(d.name))} <span>· ${mb(d.size)}</span></div><button type="button" class="btn sm danger" data-pact="dl-remove" data-id="${esc(d.id)}">${esc(t("Remove"))}</button></div>`).join("")
          : `<div class="small">${esc(t("Nothing is downloaded yet."))}</div>`);
    },
  }],
  onAct: (name, el, c) => {
    if (name === "dl-remove") return c.dl.remove(el.dataset.id).then((list) => { st.dls = list; c.render(); }).catch(c.fail);
    const ed = c.editor();
    if (!ed) return;
    // A line being typed is taken first (a click on a button does not take the focus away from it).
    if (mySection(document.activeElement) && document.activeElement !== el) document.activeElement.blur();
    const id = el.dataset.id;
    switch (name) {
      case "make": return void make(ed, c);
      case "cancel": return cancel(c);
      case "srt": return void saveSrt(ed, c);
      case "clear": return void ed.change((m) => { m.overlays = (m.overlays || []).filter((o) => !(o.kind === "text" && o.tag === TAG)); });
      case "del": return void ed.change((m) => { m.overlays = (m.overlays || []).filter((o) => o.id !== id); });
      case "seek": { const o = ed.texts(TAG).find((x) => x.id === id); if (o) { ed.seek(o.from + 0.01); ed.redraw(); } return; }
      case "color": c.s.color = el.dataset.color; c.save(); restyle(ed, c); return ed.redraw();
    }
  },
  onDownload: (d) => {
    const run = st.run;
    if (!run || run.dl !== d.id || run.cancel) return;
    if (d.phase === "download") run.pct = d.total > 0 ? clamp((d.done / d.total) * 100, 0, 100) : null;
    else if (d.phase === "check" || d.phase === "unpack") { run.pct = null; run.label = t("Checking the download…"); }
    ctx.editor()?.redraw();
  },
  start: (c) => {
    document.addEventListener("change", onChange);
    // The slider's number follows it while it is dragged; the lines change when it is let go.
    document.addEventListener("input", (e) => { const el = e.target; if (el.type === "range" && el.dataset.subSet && mySection(el)) { const out = el.parentElement.querySelector("output"); if (out) out.textContent = `${el.value}%`; } });
    // Enter in a line's box takes the text, as leaving the box does.
    document.addEventListener("keydown", (e) => { if (e.key === "Enter" && e.target.dataset?.subLine) e.target.blur(); });
    refreshDls(c);
  },
  // For tools/subtitles-test.js.
  _pure: { fmtTime, srtTime, srtText, cleanText, parseWhisper, progressOf, filesOf, pickStream, cuesOnTimeline, wrap, placeY, styleOf, itemOf },
  lang: {
    ko: {
      "Turns what is said in a clip into subtitles, on this PC. Adds a Subtitles section to the editor: make them with one click, fix the words, style them, and they are part of the exported clip. Can also save an .srt file.": "클립에서 말한 내용을 이 PC에서 자막으로 만들어요. 편집기에 자막 섹션이 생겨요. 클릭 한 번으로 만들고, 글자를 고치고, 모양을 바꾸면 내보낸 클립에 그대로 들어가요. .srt 파일로 저장할 수도 있어요.",
      "Subtitles": "자막", "Speech model (fast)": "음성 모델 (빠름)", "Speech model (better)": "음성 모델 (더 정확)", "Speech model (best)": "음성 모델 (가장 정확)",
      "Fast": "빠름", "Better": "더 정확", "Best, slower": "가장 정확, 느림",
      "Getting ready…": "준비 중…", "Downloading whisper.cpp…": "whisper.cpp 다운로드 중…", "Downloading the speech model…": "음성 모델 다운로드 중…", "Checking the download…": "다운로드 확인 중…",
      "Reading the sound…": "소리를 읽는 중…", "Listening…": "듣는 중…", "Listening… (clip {n} of {total})": "듣는 중… (클립 {n}/{total})", "Stopping…": "멈추는 중…",
      "There is no clip on the timeline to listen to.": "타임라인에 들을 클립이 없어요.", "The clip's sound could not be read.": "클립의 소리를 읽지 못했어요.", "The speech could not be read.": "말소리를 읽지 못했어요.",
      "No speech was found in this clip.": "이 클립에서 말소리를 찾지 못했어요.", "{n} subtitle lines added": "자막 {n}줄을 추가했어요",
      "There are no subtitles to save.": "저장할 자막이 없어요.", "Saved {name}": "{name} 저장됨",
      "Spoken in": "말한 언어", "Detect the language": "언어 자동 감지", "Listen to": "들을 소리", "All the sound": "모든 소리", "My microphone": "내 마이크", "The game or desktop": "게임 또는 데스크톱",
      "Quality": "품질", "Translate into English": "영어로 번역", "Make subtitles": "자막 만들기", "Make them again": "다시 만들기",
      "The first time, this downloads {size} (the speech model and whisper.cpp). After that it works without internet. The sound never leaves this PC.": "처음에는 {size}를 다운로드해요 (음성 모델과 whisper.cpp). 그다음부터는 인터넷 없이 작동해요. 소리는 이 PC 밖으로 나가지 않아요.",
      "Making them again replaces every line below.": "다시 만들면 아래의 모든 줄이 바뀌어요.", "Made on this PC. The sound never leaves it.": "이 PC에서 만들어요. 소리는 밖으로 나가지 않아요.",
      "Go to this line": "이 줄로 이동", "Subtitle text": "자막 글자", "Delete this line": "이 줄 삭제",
      "Place": "위치", "Bottom": "아래", "Middle": "가운데", "Top": "위",
      "Save .srt file": ".srt 파일 저장", "Remove all": "모두 지우기",
      "The lines are part of the exported clip. Each is also a text on the picture: move or retime it there or on the timeline.": "자막은 내보낸 클립에 들어가요. 각 줄은 화면 위의 텍스트이기도 해서, 화면이나 타임라인에서 옮기거나 시간을 바꿀 수 있어요.",
      "Downloaded files": "다운로드한 파일", "whisper.cpp and the speech models the Subtitles plugin fetched. Removing one frees its space; it is downloaded again when it is needed.": "자막 플러그인이 받은 whisper.cpp와 음성 모델이에요. 지우면 공간이 비워지고, 필요할 때 다시 다운로드돼요.",
      "Nothing is downloaded yet.": "아직 다운로드한 것이 없어요.",
    },
    el: {
      "Turns what is said in a clip into subtitles, on this PC. Adds a Subtitles section to the editor: make them with one click, fix the words, style them, and they are part of the exported clip. Can also save an .srt file.": "Μετατρέπει όσα λέγονται σε ένα κλιπ σε υπότιτλους, σε αυτό το PC. Προσθέτει την ενότητα Υπότιτλοι στον επεξεργαστή: φτιάξτε τους με ένα κλικ, διορθώστε τις λέξεις, αλλάξτε την εμφάνισή τους, και μπαίνουν στο κλιπ που εξάγετε. Μπορεί επίσης να αποθηκεύσει αρχείο .srt.",
      "Subtitles": "Υπότιτλοι", "Speech model (fast)": "Μοντέλο ομιλίας (γρήγορο)", "Speech model (better)": "Μοντέλο ομιλίας (καλύτερο)", "Speech model (best)": "Μοντέλο ομιλίας (το καλύτερο)",
      "Fast": "Γρήγορο", "Better": "Καλύτερο", "Best, slower": "Το καλύτερο, πιο αργό",
      "Getting ready…": "Προετοιμασία…", "Downloading whisper.cpp…": "Λήψη του whisper.cpp…", "Downloading the speech model…": "Λήψη του μοντέλου ομιλίας…", "Checking the download…": "Έλεγχος της λήψης…",
      "Reading the sound…": "Ανάγνωση του ήχου…", "Listening…": "Ακρόαση…", "Listening… (clip {n} of {total})": "Ακρόαση… (κλιπ {n} από {total})", "Stopping…": "Διακοπή…",
      "There is no clip on the timeline to listen to.": "Δεν υπάρχει κλιπ στη γραμμή χρόνου για ακρόαση.", "The clip's sound could not be read.": "Ο ήχος του κλιπ δεν μπόρεσε να διαβαστεί.", "The speech could not be read.": "Η ομιλία δεν μπόρεσε να διαβαστεί.",
      "No speech was found in this clip.": "Δεν βρέθηκε ομιλία σε αυτό το κλιπ.", "{n} subtitle lines added": "Προστέθηκαν {n} γραμμές υποτίτλων",
      "There are no subtitles to save.": "Δεν υπάρχουν υπότιτλοι για αποθήκευση.", "Saved {name}": "Αποθηκεύτηκε το {name}",
      "Spoken in": "Γλώσσα ομιλίας", "Detect the language": "Αυτόματη αναγνώριση", "Listen to": "Ακρόαση από", "All the sound": "Όλος ο ήχος", "My microphone": "Το μικρόφωνό μου", "The game or desktop": "Το παιχνίδι ή η επιφάνεια εργασίας",
      "Quality": "Ποιότητα", "Translate into English": "Μετάφραση στα αγγλικά", "Make subtitles": "Δημιουργία υποτίτλων", "Make them again": "Δημιουργία ξανά",
      "The first time, this downloads {size} (the speech model and whisper.cpp). After that it works without internet. The sound never leaves this PC.": "Την πρώτη φορά γίνεται λήψη {size} (το μοντέλο ομιλίας και το whisper.cpp). Μετά λειτουργεί χωρίς ίντερνετ. Ο ήχος δεν φεύγει ποτέ από αυτό το PC.",
      "Making them again replaces every line below.": "Η νέα δημιουργία αντικαθιστά όλες τις παρακάτω γραμμές.", "Made on this PC. The sound never leaves it.": "Δημιουργούνται σε αυτό το PC. Ο ήχος δεν φεύγει ποτέ από αυτό.",
      "Go to this line": "Μετάβαση σε αυτή τη γραμμή", "Subtitle text": "Κείμενο υποτίτλου", "Delete this line": "Διαγραφή αυτής της γραμμής",
      "Place": "Θέση", "Bottom": "Κάτω", "Middle": "Μέση", "Top": "Πάνω",
      "Save .srt file": "Αποθήκευση αρχείου .srt", "Remove all": "Αφαίρεση όλων",
      "The lines are part of the exported clip. Each is also a text on the picture: move or retime it there or on the timeline.": "Οι γραμμές μπαίνουν στο κλιπ που εξάγετε. Καθεμία είναι και κείμενο πάνω στην εικόνα: μετακινήστε την ή αλλάξτε τον χρόνο της εκεί ή στη γραμμή χρόνου.",
      "Downloaded files": "Ληφθέντα αρχεία", "whisper.cpp and the speech models the Subtitles plugin fetched. Removing one frees its space; it is downloaded again when it is needed.": "Το whisper.cpp και τα μοντέλα ομιλίας που κατέβασε το πρόσθετο Υπότιτλοι. Η αφαίρεση ελευθερώνει χώρο· γίνεται ξανά λήψη όταν χρειαστεί.",
      "Nothing is downloaded yet.": "Δεν έχει γίνει ακόμη καμία λήψη.",
    },
  },
});
