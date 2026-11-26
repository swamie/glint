// Vertical: a clip as a 9:16 video for TikTok, YouTube Shorts and Instagram Reels, in one click.
// An official Glint plugin (plugins/official/vertical in Glint's repository).
//
// What it does: it is a preset on top of Glint's own canvas (ui/canvas.js) and export. The preset
// says which app the video is for, how the wide picture becomes a tall one (a crop that fills the
// screen, the whole picture over a blurred copy of itself, or a crop that follows the action), the
// size, the frame rate and how large the file may get. It is used from three places:
//
//   - the library: right-click a clip (or a selection), "Export vertical". Nothing else to do.
//   - the editor: a "Vertical" section. "Make it vertical" puts the preset on the clip as an
//     ordinary canvas, so it is previewed, can be arranged by hand, is undone with Ctrl+Z and is
//     saved with a project. "Export vertical" is the editor's export with the preset's frame rate
//     and bitrate. Subtitles made by the Subtitles plugin are wrapped for the narrow picture and
//     moved above the app's own buttons.
//   - Tools → Vertical: the preset itself.
//
// "Follow the action" looks at the clip on this PC: a small copy of two frames a moment apart,
// a few times a second of footage, and where they differ most is where the crop goes. The moves
// are ordinary keyframes of the canvas piece.
const { ctx, t, esc } = Glint;
const ID = "vertical";
const SUB_TAG = "subtitles";   // the tag the Subtitles plugin gives its lines
const LINE_H = 1.2;            // a text item's line height, times its size (as Glint draws it)
const SUB_W = 0.74;            // the widest a subtitle line may be: the apps' buttons sit on the right
const MOVE = 0.7;              // seconds a follow move takes
const LABEL = "vertical";      // "<title> (vertical).mp4"

// `limit`: the longest video the app takes, in seconds (0 = none). `bottom`: the part of the
// picture's height the app covers with its own caption and buttons, measured from the bottom.
const PLATFORMS = {
  tiktok: { name: "TikTok", limit: 600, bottom: 0.2 },
  shorts: { name: "YouTube Shorts", limit: 180, bottom: 0.2 },
  reels: { name: "Instagram Reels", limit: 180, bottom: 0.22 },
  any: { name: "", limit: 0, bottom: 0.12 },
};
const SIZES = { 1080: [1080, 1920], 720: [720, 1280] };
const KBPS = { high: 16000, balanced: 10000, small: 6000 };
const DEFAULTS = { platform: "tiktok", layout: "fill", pos: 50, zoom: 0, bg: "blur", size: 1080, fps: 60, quality: "high", over: "ask", subs: true, sub_size: 3 };

const st = {
  queue: [],     // the library's exports still to do: [{ id, title, cut }]
  now: null,     // the one running: { id, title, step: "look" | "export", pct }
  done: 0,       // finished since the queue last ran dry
  follow: null,  // the editor's "follow the action" while it looks: { pct, cancel }
  note: "",      // what the last look found, for the editor's section
  pending: null, // a clip id opened from the library that becomes vertical once its editor has loaded
};

/* ---------- pure helpers (tested under Node: tools/vertical-test.js) ---------- */

const clamp = (x, lo, hi) => Math.min(hi, Math.max(lo, x));
const r5 = (x) => Math.round(x * 100000) / 100000;
const r3 = (x) => Math.round(x * 1000) / 1000;

/** "3:05" for 185 seconds. */
function fmtLen(sec) {
  const s = Math.max(0, Math.round(sec));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** The preset in the plugin's settings, made safe. */
function presetOf(s) {
  s = s || {};
  const platform = PLATFORMS[s.platform] ? s.platform : DEFAULTS.platform;
  const size = SIZES[+s.size] ? +s.size : DEFAULTS.size;
  return {
    platform,
    layout: ["fill", "fit", "follow"].includes(s.layout) ? s.layout : DEFAULTS.layout,
    pos: clamp(Number.isFinite(+s.pos) ? +s.pos : 50, 0, 100) / 100,
    zoom: clamp(Number.isFinite(+s.zoom) ? +s.zoom : 0, 0, 100) / 100,
    bg: s.bg === "black" ? "black" : "blur",
    size,
    width: SIZES[size][0],
    height: SIZES[size][1],
    fps: [0, 30, 60].includes(+s.fps) ? +s.fps : DEFAULTS.fps,
    quality: KBPS[s.quality] ? s.quality : DEFAULTS.quality,
    over: ["ask", "cut", "keep"].includes(s.over) ? s.over : DEFAULTS.over,
    subs: s.subs !== false,
    subSize: clamp(Math.round(+s.sub_size || DEFAULTS.sub_size), 2, 6) / 100,
    limit: PLATFORMS[platform].limit,
    bottom: PLATFORMS[platform].bottom,
  };
}

/** The most kbit/s the picture may take: less for the smaller size and for 30 frames a second. */
function kbpsOf(p) {
  return Math.round(KBPS[p.quality] * (p.size === 720 ? 0.5 : 1) * (p.fps === 30 ? 0.75 : 1));
}

/** What the export gets on top of the canvas. */
const extrasOf = (p) => ({ fps_cap: p.fps, max_kbps: kbpsOf(p), label: LABEL });

/**
 * The one piece of the picture on a canvas `C` for a clip picture `src` ({ w, h } pixels): `zoom`
 * 0 shows the whole picture, 1 fills the canvas, and what does not fit is cut away around `pos`
 * (0 = the left of the picture, 1 = the right). The two boxes have the same shape in pixels, so
 * nothing is stretched. `k` is the scale: canvas pixels per source pixel.
 */
function pieceFor(src, C, zoom, pos) {
  const k0 = Math.min(C.width / src.w, C.height / src.h), k1 = Math.max(C.width / src.w, C.height / src.h);
  const k = k0 + (k1 - k0) * clamp(zoom, 0, 1);
  const sw = Math.min(1, C.width / (k * src.w)), sh = Math.min(1, C.height / (k * src.h));
  const w = Math.min(1, (sw * src.w * k) / C.width), h = Math.min(1, (sh * src.h * k) / C.height);
  return { sx: r5((1 - sw) * clamp(pos, 0, 1)), sy: r5((1 - sh) / 2), sw: r5(sw), sh: r5(sh), x: r5((1 - w) / 2), y: r5((1 - h) / 2), w: r5(w), h: r5(h), k };
}

/** The canvas of preset `p` for a clip picture `src`: { width, height, background, pieces: [one] }. */
function canvasFor(p, src) {
  const C = { width: p.width, height: p.height };
  const fit = p.layout === "fit";
  const { k, ...piece } = pieceFor(src, C, fit ? p.zoom : 1, p.layout === "follow" ? 0.5 : p.pos);
  return { width: C.width, height: C.height, background: fit ? p.bg : "black", pieces: [piece], k };
}

/** A clip's picture size, or 16:9 when it is not known. */
const srcOf = (clip) => (clip && clip.width > 0 && clip.height > 0 ? { w: clip.width, h: clip.height } : { w: 1920, h: 1080 });

/** A canvas this plugin could have made: taller than wide, one piece. */
const isVertical = (cv) => !!cv && cv.height >= cv.width * 1.5 && Array.isArray(cv.pieces) && cv.pieces.length === 1;

/**
 * Where two small gray frames (`a`, `b`: W x H bytes) differ, as a fraction of the width (0 = the
 * left edge), or null when they hardly differ (a still picture) or differ nearly everywhere (the
 * camera turned, or a cut): then there is no one place to follow.
 */
function motionX(a, b, W, H) {
  let total = 0, moment = 0, changed = 0;
  for (let x = 0; x < W; x++) {
    let col = 0;
    for (let y = 0; y < H; y++) {
      const d = Math.abs(a[y * W + x] - b[y * W + x]);
      if (d > 14) { col += d; changed++; }
    }
    total += col;
    moment += col * x;
  }
  const part = changed / (W * H);
  if (part < 0.004 || part > 0.6 || total <= 0) return null;
  return moment / total / Math.max(1, W - 1);
}

/**
 * The crop's keys from what was seen. `samples`: [{ t, x }] in time order (x: motionX's answer),
 * `sw`: the crop's width as a fraction of the picture. The crop holds still until the action is
 * clearly somewhere else, then moves there in MOVE seconds: [{ t, cx }], the first at t = 0.
 */
function followKeys(samples, sw) {
  const lo = sw / 2, hi = 1 - sw / 2;
  if (hi - lo < 0.01) return [{ t: 0, cx: 0.5 }];
  // Nothing seen: where it was. Then the middle one of three, so a single odd look moves nothing.
  const xs = [];
  let last = null;
  for (const s of samples) { if (s.x != null && Number.isFinite(s.x)) last = s.x; xs.push(last); }
  const first = xs.find((x) => x != null);
  if (first == null) return [{ t: 0, cx: 0.5 }];
  const filled = xs.map((x) => (x == null ? first : x));
  const mid = filled.map((x, i) => { const tri = [filled[Math.max(0, i - 1)], x, filled[Math.min(filled.length - 1, i + 1)]].sort((p, q) => p - q); return tri[1]; });
  const dead = Math.max(0.05, sw * 0.16);
  let cur = clamp(mid[0], lo, hi);
  const keys = [{ t: 0, cx: r3(cur) }];
  for (let i = 1; i < mid.length && keys.length < 180; i++) {
    const want = clamp(mid[i], lo, hi), at = samples[i].t, prev = keys[keys.length - 1].t;
    if (Math.abs(want - cur) <= dead || at <= prev + 0.1) continue;
    const hold = Math.max(prev + 0.05, at - MOVE);
    if (hold < at - 0.05 && hold > prev) keys.push({ t: r3(hold), cx: r3(cur) });
    cur = want;
    keys.push({ t: r3(at), cx: r3(cur) });
  }
  return keys;
}

/** The times to look at on a timeline `total` seconds long: at most 150, at least 0.4 s apart. */
function lookTimes(total) {
  const step = Math.max(0.4, total / 150), out = [];
  for (let x = step / 2; x < total - 0.15; x += step) out.push(r3(x));
  return out;
}

/** Follow keys as the canvas piece's keyframes (ui/keyframes.js). */
const pieceKeys = (keys, k) => keys.map((p) => ({ t: p.t, x: 0, y: 0, w: 1, h: 1, cx: p.cx, cy: 0.5, zoom: r5(k) }));

/** Follow keys as export_clip's track for a clip exported whole (or cut at `end`): output seconds are the clip's own. */
function trackOf(keys, k, end) {
  const pt = (tm, cx, e) => ({ t: r3(tm), x: 0, y: 0, w: 1, h: 1, cx, cy: 0.5, z: r5(k), ...(e ? { e: true } : {}) });
  const inside = keys.filter((p) => p.t < end);
  const out = inside.map((p, i) => pt(p.t, p.cx, i + 1 < inside.length));
  if (out.length) out.push(pt(end, inside[inside.length - 1].cx, false));
  return out;
}

/** `text` on as few lines as fit (`fits(line)`), at most `max`; what is left stays on the last. */
function wrapLines(text, fits, max = 4) {
  const words = String(text).replace(/\s+/g, " ").trim().split(" ").filter(Boolean);
  const lines = [];
  let cur = "";
  for (const w of words) {
    const next = cur ? `${cur} ${w}` : w;
    if (cur && !fits(next) && lines.length < max - 1) { lines.push(cur); cur = w; } else cur = next;
  }
  if (cur) lines.push(cur);
  return lines;
}

/** One subtitle line made to fit the vertical picture: narrower, at the preset's size, above the app's buttons. */
function subFit(text, p, width) {
  const plain = String(text || "").replace(/\s*\n\s*/g, " ").trim();
  const lines = wrapLines(plain, (l) => width(l, p.subSize) <= SUB_W);
  return { text: lines.join("\n"), size: p.subSize, x: 0.5, y: r3(1 - p.bottom - lines.length * p.subSize * LINE_H), align: "center" };
}

/** Whether a result `len` seconds long is over the preset's limit. */
const isOver = (p, len) => p.limit > 0 && len > p.limit + 0.5;

/** export_clip's options for a library clip exported as it is. `keys`: follow keys, or null. `cut`: end it at the limit. */
function optsFor(clip, p, keys, cut) {
  const src = srcOf(clip), C = canvasFor(p, src);
  const end = cut && isOver(p, clip.duration) ? p.limit : 0;
  const piece = { ...C.pieces[0] };
  if (keys && keys.length) piece.sx = r5(clamp(keys[0].cx - piece.sw / 2, 0, 1 - piece.sw));
  const moving = keys && keys.length > 1;
  return {
    start: 0, end, speed: 1,
    canvas: { width: C.width, height: C.height, background: C.background, pieces: [piece] },
    ...(moving ? { canvas_tracks: [trackOf(keys, C.k, end || clip.duration || keys[keys.length - 1].t + 1)] } : {}),
    ...extrasOf(p),
  };
}

/* ---------- looking at the clip ---------- */

const LOOK_W = 96, LOOK_H = 54;
const CANCELLED = {};

function waitFor(el, name, ms) {
  return new Promise((resolve, reject) => {
    const done = (ok, why) => { clearTimeout(timer); el.removeEventListener(name, yes); el.removeEventListener("error", no); ok ? resolve() : reject(new Error(why)); };
    const yes = () => done(true), no = () => done(false, "the clip could not be opened");
    const timer = setTimeout(() => done(false, "the clip took too long to open"), ms);
    el.addEventListener(name, yes, { once: true });
    el.addEventListener("error", no, { once: true });
  });
}

/**
 * Where the action is over a timeline. `spans`: [{ at, len, speed, path, from }] (an editor's
 * footage(), or one span for a whole clip); `total`: the timeline's length. `run`: { pct, cancel },
 * `tick()` is called as it moves on. Resolves to [{ t, x }]; rejects with CANCELLED or an Error.
 */
async function look(spans, total, c, run, tick) {
  const times = lookTimes(total);
  const cv = document.createElement("canvas");
  cv.width = LOOK_W;
  cv.height = LOOK_H;
  const g = cv.getContext("2d", { willReadFrequently: true });
  const videos = new Map();
  const open = async (path) => {
    if (videos.has(path)) return videos.get(path);
    const v = document.createElement("video");
    v.muted = true;
    v.preload = "auto";
    v.crossOrigin = "anonymous";
    v.src = c.fileSrc(path);
    videos.set(path, v);
    await waitFor(v, "loadeddata", 12000);
    return v;
  };
  const gray = async (v, at) => {
    const to = clamp(at, 0, Math.max(0, (v.duration || at) - 0.05));
    if (Math.abs(v.currentTime - to) > 0.001) { v.currentTime = to; await waitFor(v, "seeked", 6000); }
    g.drawImage(v, 0, 0, LOOK_W, LOOK_H);
    const d = g.getImageData(0, 0, LOOK_W, LOOK_H).data, out = new Uint8Array(LOOK_W * LOOK_H);
    for (let i = 0; i < out.length; i++) out[i] = (d[i * 4] * 77 + d[i * 4 + 1] * 150 + d[i * 4 + 2] * 29) >> 8;
    return out;
  };
  const out = [];
  try {
    for (let i = 0; i < times.length; i++) {
      if (run.cancel) throw CANCELLED;
      const tm = times[i];
      const sp = spans.find((x) => tm >= x.at && tm < x.at + x.len);
      let x = null;
      if (sp && sp.path && !sp.missing) {
        const v = await open(sp.path), at = sp.from + (tm - sp.at) * (sp.speed || 1);
        const a = await gray(v, at), b = await gray(v, at + 0.2);
        x = motionX(a, b, LOOK_W, LOOK_H);
      }
      out.push({ t: tm, x });
      run.pct = ((i + 1) / times.length) * 100;
      tick();
    }
  } finally {
    for (const v of videos.values()) { v.removeAttribute("src"); try { v.load(); } catch { /* gone already */ } }
  }
  return out;
}

/** Why a look failed, in words for the user. */
const lookWhy = (e) => (e && e.name === "SecurityError" ? t("Glint could not read this clip's picture.") : t("Glint could not look at this clip ({reason}).", { reason: String(e?.message ?? e) }));

/* ---------- the library ---------- */

const onMyPage = () => !!document.querySelector(`.plug-page[data-plugin="${ID}"]`);
const refresh = (c) => { if (onMyPage()) c.render(); else c.chrome(); };
const platName = (p) => PLATFORMS[p.platform].name;

/** "Export vertical" on clips of the library: asks once about the ones that are too long, then one after another. */
async function exportClips(clips, c) {
  const p = presetOf(c.s);
  const list = clips.filter((k) => !k.draft && !k.cloud_only);
  if (!list.length) return c.flash(t("Open a project in the editor to export it vertical."), true);
  const long = list.filter((k) => isOver(p, k.duration));
  let cut = p.over === "cut";
  if (long.length && p.over === "ask") {
    const answer = await c.confirm({
      title: t("Longer than {app} takes", { app: platName(p) }),
      text: long.length === 1 && list.length === 1
        ? t("This clip is {len} long. {app} takes up to {max}. Cut it at {max}, or export all of it?", { len: fmtLen(long[0].duration), app: platName(p), max: fmtLen(p.limit) })
        : t("{n} of these clips are longer than the {max} that {app} takes. Cut them at {max}, or export all of each?", { n: long.length, app: platName(p), max: fmtLen(p.limit) }),
      ok: t("Cut at {max}", { max: fmtLen(p.limit) }),
      cancel: t("Export all of it"),
    });
    if (answer === null) return;
    cut = answer === "yes";
  }
  for (const k of list) if (!st.queue.some((q) => q.id === k.id) && st.now?.id !== k.id) st.queue.push({ id: k.id, title: k.title, cut });
  c.flash(list.length === 1 ? t("Making {title} vertical…", { title: list[0].title }) : t("Making {n} clips vertical…", { n: list.length }));
  if (!st.now) runQueue(c);
  else refresh(c);
}

async function runQueue(c) {
  let made = 0, lastTitle = "";
  while (st.queue.length) {
    const job = st.queue.shift();
    const clip = c.clips.get(job.id);
    if (!clip) continue;
    const p = presetOf(c.s);
    st.now = { id: job.id, title: job.title, step: "export", pct: 0, cancel: false };
    refresh(c);
    try {
      let keys = null;
      if (p.layout === "follow") {
        st.now.step = "look";
        refresh(c);
        const sw = canvasFor(p, srcOf(clip)).pieces[0].sw;
        try {
          const seen = await look([{ at: 0, len: clip.duration, speed: 1, path: clip.path, from: 0 }], clip.duration, c, st.now, () => { if (onMyPage()) paintNow(); });
          keys = followKeys(seen, sw);
        } catch (e) {
          if (e === CANCELLED) throw e;
          console.warn("Glint vertical: the look failed, the crop stays in the middle", e);
        }
        st.now.step = "export";
        refresh(c);
      }
      const meta = await c.clips.export(job.id, optsFor(clip, p, keys, job.cut));
      made++;
      lastTitle = meta?.title || job.title;
    } catch (e) {
      if (e === CANCELLED || /cancel/i.test(String(e?.message ?? e))) { st.queue.length = 0; c.flash(t("Vertical export cancelled")); }
      else c.fail(e);
    }
    st.now = null;
    refresh(c);
  }
  if (made === 1) c.flash(t("Vertical clip saved: {title}", { title: lastTitle }));
  else if (made > 1) c.flash(t("{n} vertical clips saved", { n: made }));
}

function cancelQueue(c) {
  st.queue.length = 0;
  if (!st.now) return;
  st.now.cancel = true;
  if (st.now.step === "export") c.invoke("cancel_export", { id: st.now.id }).catch(() => {});
  refresh(c);
}

/* ---------- the editor ---------- */

const mySection = (el) => el?.closest?.(`.e-plug[data-plugin="${ID}"]`);
const newId = () => "p" + Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
/** How long the finished clip will be. */
const resultLen = (ed) => { const m = ed.model; return m ? ed.outRange(0, m.duration)?.to || 0 : 0; };

/**
 * Put the preset on the open clip: one undo step. A piece that was there keeps its id, and its
 * follow keys while the preset still follows. Subtitles are fitted with it.
 */
function apply(ed, c, { subs = true } = {}) {
  const p = presetOf(c.s), src = srcOf(ed.clip), C = canvasFor(p, src);
  return ed.change((m) => {
    const old = isVertical(m.canvas) ? m.canvas.pieces[0] : null;
    const piece = { id: old?.id || newId(), ...C.pieces[0] };
    if (p.layout === "follow" && old && Array.isArray(old.keys) && old.keys.length) {
      // The moves found before, at this size's scale.
      piece.keys = old.keys.map((k) => ({ ...k, x: 0, y: 0, w: 1, h: 1, cy: 0.5, zoom: r5(C.k) }));
      piece.smooth = true;
      piece.sx = r5(clamp(piece.keys[0].cx - piece.sw / 2, 0, 1 - piece.sw));
    }
    m.canvas = { width: C.width, height: C.height, background: C.background, pieces: [piece] };
    if (subs && p.subs) fitSubs(m, ed, p);
  });
}

/** The Subtitles plugin's lines, made to fit the vertical picture (inside a change of the model). */
function fitSubs(m, ed, p) {
  const pic = { width: p.width, height: p.height };
  let n = 0;
  for (const o of m.overlays || []) {
    if (o.kind !== "text" || o.tag !== SUB_TAG || (Array.isArray(o.keys) && o.keys.length)) continue;
    Object.assign(o, subFit(o.text, p, (line, size) => ed.textWidth(line, size, pic)));
    n++;
  }
  return n;
}

/** "Follow the action" on the open clip: look, then the piece gets its keys (one undo step). */
async function follow(ed, c) {
  if (st.follow || !ed.model) return;
  const p = presetOf(c.s), run = (st.follow = { pct: 0, cancel: false });
  st.note = "";
  ed.redraw();
  try {
    const m = ed.model, src = srcOf(ed.clip), C = canvasFor(p, src);
    const seen = await look(ed.footage(), m.duration, c, run, () => paintFollow());
    if (c.editor() !== ed || !ed.model) return;
    const keys = followKeys(seen, C.pieces[0].sw);
    ed.change((mm) => {
      const piece = isVertical(mm.canvas) ? mm.canvas.pieces[0] : null;
      if (!piece) return;
      delete piece.keys;
      delete piece.smooth;
      piece.sx = r5(clamp(keys[0].cx - piece.sw / 2, 0, 1 - piece.sw));
      if (keys.length > 1) { piece.keys = pieceKeys(keys, C.k); piece.smooth = true; }
    });
    const moves = Math.floor(keys.length / 2);
    st.note = keys.length > 1 ? (moves === 1 ? t("The crop moves once to stay on the action.") : t("The crop moves {n} times to stay on the action.", { n: moves })) : t("The action stays in one place, so the crop does too.");
  } catch (e) {
    if (e !== CANCELLED) st.note = lookWhy(e) + " " + t("The crop stays in the middle.");
  } finally {
    st.follow = null;
    if (c.editor() === ed) ed.redraw();
  }
}

/** The bar of a running look, moved without drawing the section again. */
function paintFollow() {
  const bar = document.querySelector(`.e-plug[data-plugin="${ID}"] .vx-look i`);
  if (bar && st.follow) bar.style.width = `${Math.round(st.follow.pct)}%`;
}
function paintNow() {
  const bar = document.querySelector(`.plug-page[data-plugin="${ID}"] .vx-look i`);
  if (bar && st.now) bar.style.width = `${Math.round(st.now.pct)}%`;
}

/* ---------- what it draws ---------- */

const ICON_V = '<svg viewBox="0 0 24 24"><rect x="7" y="3" width="10" height="18" rx="2"/><path d="M10.500 17.500h3"/></svg>';
const sel = (key, cur, opts) => `<select class="field" data-vx="${key}">${opts.map(([v, l]) => `<option value="${esc(v)}"${String(v) === String(cur) ? " selected" : ""}>${esc(l)}</option>`).join("")}</select>`;
const slider = (key, label, val, unit = "%") => `<label class="e-slider"><span>${esc(label)}</span><input type="range" min="0" max="100" step="1" value="${Math.round(val)}" data-vx="${key}"><output>${Math.round(val)}${unit}</output></label>`;
const platOpts = () => [["tiktok", "TikTok"], ["shorts", "YouTube Shorts"], ["reels", "Instagram Reels"], ["any", t("Any app (no length limit)")]];
const layoutOpts = () => [["fill", t("Fill the screen")], ["fit", t("Whole picture, blurred behind")], ["follow", t("Fill and follow the action")]];
const fpsOpts = () => [[60, t("60 fps")], [30, t("30 fps")], [0, t("As recorded")]];
const qualityOpts = (p) => Object.keys(KBPS).map((k) => [k, `${k === "high" ? t("High") : k === "balanced" ? t("Balanced") : t("Small file")} · ${t("up to {n} Mbps", { n: Math.round(kbpsOf({ ...p, quality: k }) / 100) / 10 })}`]);
const layoutHint = (k) => (k === "fill" ? t("The picture is cut at the sides so it fills the whole screen.")
  : k === "fit" ? t("Nothing is cut away. The space above and below is the clip itself, blurred.")
    : t("Fills the screen, and the cut moves to where things happen. Best when the action is off to one side, such as a face camera or a replay."));

/** The length line: fine, or over what the app takes. */
function lengthNote(p, len) {
  if (!p.limit) return { text: t("The result is {len} long.", { len: fmtLen(len) }), bad: false };
  return isOver(p, len)
    ? { text: t("The result is {len} long. {app} takes up to {max}: trim it on the timeline.", { len: fmtLen(len), app: platName(p), max: fmtLen(p.limit) }), bad: true }
    : { text: t("The result is {len} long. {app} takes up to {max}.", { len: fmtLen(len), app: platName(p), max: fmtLen(p.limit) }), bad: false };
}

/** The editor's section. */
function renderEditor(ed, c) {
  const m = ed.model;
  if (!m) return `<div class="small">${esc(t("Loading the editor…"))}</div>`;
  const p = presetOf(c.s), src = srcOf(ed.clip), on = isVertical(m.canvas);
  if (!on) {
    return `<div class="small">${esc(t("Makes this clip a {w} × {h} video for TikTok, Shorts and Reels.", { w: p.width, h: p.height }))}</div>
      <label class="vx-row"><span>${esc(t("Picture"))}</span>${sel("layout", p.layout, layoutOpts())}</label>
      <div class="vx-btns"><button type="button" class="btn sm primary" data-pact="apply">${ICON_V}<span>${esc(t("Make it vertical"))}</span></button></div>
      ${m.canvas ? `<div class="small vx-warn">${esc(t("This clip already has a canvas layout. Making it vertical replaces it."))}</div>` : ""}`;
  }
  const piece = m.canvas.pieces[0], cropped = piece.sw < 0.999, moving = Array.isArray(piece.keys) && piece.keys.length > 1;
  const subs = ed.texts(SUB_TAG).length, len = lengthNote(p, resultLen(ed));
  let html = `<label class="vx-row"><span>${esc(t("For"))}</span>${sel("platform", p.platform, platOpts())}</label>
    <label class="vx-row"><span>${esc(t("Picture"))}</span>${sel("layout", p.layout, layoutOpts())}</label>`;
  if (p.layout === "fit") html += `<div class="e-sliders">${slider("zoom", t("Zoom in"), p.zoom * 100)}${p.zoom > 0.001 && cropped ? slider("pos", t("Left to right"), p.pos * 100) : ""}</div>
    <label class="vx-row"><span>${esc(t("Behind it"))}</span>${sel("bg", p.bg, [["blur", t("The clip, blurred")], ["black", t("Black")]])}</label>`;
  else if (p.layout === "fill" && cropped) html += `<div class="e-sliders">${slider("pos", t("Left to right"), p.pos * 100)}</div>`;
  if (p.layout === "follow") {
    html += st.follow
      ? `<div class="vx-prog" role="status"><div class="vx-prog-top"><b>${esc(t("Looking for the action…"))}</b></div><div class="progress vx-look"><i style="width:${Math.round(st.follow.pct)}%"></i></div>
          <div class="vx-btns"><button type="button" class="btn sm" data-pact="follow-cancel">${esc(t("Cancel"))}</button></div></div>`
      : `<div class="vx-btns"><button type="button" class="btn sm" data-pact="follow">${esc(moving ? t("Look again") : t("Find the action"))}</button></div>
          ${st.note ? `<div class="small">${esc(st.note)}</div>` : moving ? "" : `<div class="small">${esc(t("Glint looks through the clip on this PC and moves the cut to where things happen."))}</div>`}`;
  }
  html += `<label class="vx-row"><span>${esc(t("Frame rate"))}</span>${sel("fps", p.fps, fpsOpts())}</label>
    <label class="vx-row"><span>${esc(t("Quality"))}</span>${sel("quality", p.quality, qualityOpts(p))}</label>
    <div class="small${len.bad ? " vx-warn" : ""}">${esc(len.text)}</div>`;
  if (subs) html += `<div class="vx-btns"><button type="button" class="btn sm" data-pact="subs">${esc(t("Fit the subtitles"))}</button></div>
    <div class="small">${esc(t("Wraps the subtitles for the narrow picture and puts them above the app's own buttons."))}</div>`;
  html += `<div class="vx-btns">
      <button type="button" class="btn sm primary" data-pact="export"${ed.exporting || st.follow ? " disabled" : ""}>${ICON_V}<span>${esc(t("Export vertical"))}</span></button>
      <button type="button" class="btn sm" data-pact="off">${esc(t("Back to the clip's own shape"))}</button>
    </div>
    <div class="small">${esc(t("To move or resize the picture by hand, use the Canvas section above."))}</div>`;
  return html;
}

/** A small drawing of what the preset does to a 16:9 clip: the part that is kept, and the phone's screen. */
function diagram(p) {
  const src = { w: 1920, h: 1080 }, C = canvasFor(p, src), q = C.pieces[0];
  const W = 160, H = 90, PW = 72, PH = 128;
  const follow = p.layout === "follow";
  return `<svg class="vx-diagram" viewBox="0 0 300 140" role="img" aria-label="${esc(t("What is kept of the picture"))}">
    <rect class="vx-d-src" x="6" y="25" width="${W}" height="${H}" rx="4"/>
    <rect class="vx-d-keep" x="${(6 + q.sx * W).toFixed(1)}" y="${(25 + q.sy * H).toFixed(1)}" width="${(q.sw * W).toFixed(1)}" height="${(q.sh * H).toFixed(1)}" rx="2"/>
    ${follow ? `<path class="vx-d-arrow" d="M${(6 + q.sx * W - 16).toFixed(1)} 70h10M${(6 + (q.sx + q.sw) * W + 6).toFixed(1)} 70h10"/>` : ""}
    <path class="vx-d-arrow" d="M178 70h28m-7-6 7 6-7 6"/>
    <rect class="${C.background === "blur" && q.h < 0.999 ? "vx-d-blur" : "vx-d-black"}" x="220" y="6" width="${PW}" height="${PH}" rx="8"/>
    <rect class="vx-d-keep" x="${(220 + q.x * PW).toFixed(1)}" y="${(6 + q.y * PH).toFixed(1)}" width="${(q.w * PW).toFixed(1)}" height="${(q.h * PH).toFixed(1)}" rx="${q.h > 0.999 ? 8 : 2}"/>
    <rect class="vx-d-phone" x="220" y="6" width="${PW}" height="${PH}" rx="8"/>
  </svg>`;
}

/** Tools → Vertical. */
function renderPage(c) {
  const p = presetOf(c.s);
  const cropped = canvasFor(p, { w: 1920, h: 1080 }).pieces[0].sw < 0.999;
  const busy = st.now ? `<div class="note vx-busy"><div class="vx-busy-main"><b>${esc(st.now.step === "look" ? t("Looking for the action in {title}…", { title: st.now.title }) : t("Exporting {title}…", { title: st.now.title }))}</b>
      <span>${esc(st.queue.length ? t("{n} more waiting", { n: st.queue.length }) : st.now.step === "look" ? "" : t("Its progress is in the renders list at the top."))}</span>
      ${st.now.step === "look" ? `<div class="progress vx-look"><i style="width:${Math.round(st.now.pct)}%"></i></div>` : ""}</div>
      <button type="button" class="btn sm" data-pact="queue-cancel">${esc(t("Cancel"))}</button></div>` : "";
  return `<div class="page-head"><div><h1>${esc(t("Vertical export"))}</h1><div class="sub">${esc(t("A clip as a 9:16 video for TikTok, Shorts and Reels, in one click"))}</div></div></div>
    ${busy}
    <div class="section"><h2>${esc(t("How to use it"))}</h2>
      <div class="vx-how">
        <div><b>${esc(t("In the library"))}</b><span>${esc(t("Right-click a clip, or a selection of clips, and pick Export vertical. The new clip appears beside the original."))}</span></div>
        <div><b>${esc(t("In the editor"))}</b><span>${esc(t("Open a clip and press Make it vertical in the Vertical section. You see the result before you export, and can trim it, move the picture and add subtitles."))}</span></div>
      </div>
    </div>
    <div class="section"><h2>${esc(t("Picture"))}</h2>
      <div class="vx-pic">
        <div class="vx-pic-rows">
          ${c.row(t("Made for"), p.limit ? t("{app} takes videos up to {max}", { app: platName(p), max: fmtLen(p.limit) }) : t("No length limit, and subtitles sit a little lower"), sel("platform", p.platform, platOpts()))}
          ${c.row(t("How it fits"), layoutHint(p.layout), sel("layout", p.layout, layoutOpts()))}
          ${p.layout === "fit" ? c.row(t("Zoom in"), t("0% shows the whole picture, 100% fills the screen"), `<div class="vx-range"><input type="range" min="0" max="100" step="1" value="${Math.round(p.zoom * 100)}" data-vx="zoom"><output>${Math.round(p.zoom * 100)}%</output></div>`) : ""}
          ${p.layout !== "follow" && cropped ? c.row(t("Left to right"), t("Which part of the picture is kept: 50% is the middle"), `<div class="vx-range"><input type="range" min="0" max="100" step="1" value="${Math.round(p.pos * 100)}" data-vx="pos"><output>${Math.round(p.pos * 100)}%</output></div>`) : ""}
          ${p.layout === "fit" ? c.row(t("Behind the picture"), "", sel("bg", p.bg, [["blur", t("The clip, blurred")], ["black", t("Black")]])) : ""}
        </div>
        ${diagram(p)}
      </div>
    </div>
    <div class="section"><h2>${esc(t("File"))}</h2>
      ${c.row(t("Size"), t("1080 × 1920 is what the apps show at their best"), sel("size", p.size, [[1080, "1080 × 1920"], [720, "720 × 1280"]]))}
      ${c.row(t("Frame rate"), t("A clip recorded faster is brought down to this. The apps do not play more than 60"), sel("fps", p.fps, fpsOpts()))}
      ${c.row(t("Quality"), t("The apps compress every upload again, so a larger file than High gains nothing"), sel("quality", p.quality, qualityOpts(p)))}
      ${p.limit ? c.row(t("A clip that is too long"), t("From the library. In the editor you trim it yourself"), sel("over", p.over, [["ask", t("Ask me")], ["cut", t("Cut it at {max}", { max: fmtLen(p.limit) })], ["keep", t("Export all of it")]])) : ""}
    </div>
    <div class="section"><h2>${esc(t("Subtitles"))}</h2>
      ${c.row(t("Fit subtitles to the vertical picture"), t("In the editor, lines made with the Subtitles plugin are wrapped for the narrow picture and moved above the app's own buttons"), `<label class="switch"><input type="checkbox" data-vx="subs"${p.subs ? " checked" : ""}><i></i></label>`)}
      ${p.subs ? c.row(t("Text size"), t("As a part of the picture's height"), sel("sub_size", Math.round(p.subSize * 100), [2, 3, 4, 5, 6].map((n) => [n, `${n}%`]))) : ""}
    </div>`;
}

/* ---------- its controls ---------- */

const NUM_KEYS = ["pos", "zoom", "size", "fps", "sub_size"];
const LOOK_KEYS = ["layout", "pos", "zoom", "bg", "size"];   // what changes the canvas

/** A control with data-vx changed: the setting is saved, and an open vertical clip follows it. */
function onChange(e) {
  const el = e.target, key = el.dataset?.vx;
  if (!key || !(key in DEFAULTS)) return;
  const inEditor = !!mySection(el);
  if (!inEditor && !el.closest(`.plug-page[data-plugin="${ID}"]`)) return;
  ctx.s[key] = el.type === "checkbox" ? el.checked : NUM_KEYS.includes(key) ? +el.value : el.value;
  ctx.save();
  el.blur?.();
  const ed = ctx.editor();
  if (inEditor && ed?.model) {
    if (isVertical(ed.model.canvas)) {
      if (LOOK_KEYS.includes(key)) {
        apply(ed, ctx, { subs: key === "size" });
        const piece = ed.model.canvas?.pieces?.[0];
        if (key === "layout" && ctx.s.layout === "follow" && !(piece?.keys?.length > 1)) follow(ed, ctx);
      } else if (key === "platform" && presetOf(ctx.s).subs && ed.texts(SUB_TAG).length) ed.change((m) => { fitSubs(m, ed, presetOf(ctx.s)); });
    }
    ed.redraw();
  } else ctx.render();
}

/** A slider's number follows it while it is dragged (the picture changes when it is let go). */
function onInput(e) {
  const el = e.target;
  if (el.type !== "range" || !el.dataset?.vx) return;
  const out = el.parentElement.querySelector("output");
  if (out) out.textContent = `${el.value}%`;
  // On the page the drawing follows at once.
  const page = el.closest(`.plug-page[data-plugin="${ID}"]`), old = page?.querySelector(".vx-diagram");
  if (old) old.outerHTML = diagram(presetOf({ ...ctx.s, [el.dataset.vx]: +el.value }));
}

Glint.register({
  defaults: { ...DEFAULTS },
  pages: [{
    id: "vertical", title: "Vertical", icon: ICON_V,
    render: renderPage,
    badge: () => (st.now ? String(st.queue.length + 1) : ""),
  }],
  editor: {
    title: "Vertical",
    tab: "clip",
    render: renderEditor,
    onOpen: (ed) => { st.note = ""; },
    // A clip opened from the library's "Open as vertical": the preset goes on once its editor has loaded.
    onSync: (ed, what, c) => {
      if (st.pending !== ed.clip.id || !ed.model) return;
      st.pending = null;
      setTimeout(() => {
        if (c.editor() !== ed || !ed.model || isVertical(ed.model.canvas)) return;
        apply(ed, c);
        if (presetOf(c.s).layout === "follow") follow(ed, c);
      }, 0);
    },
    onClose: () => { if (st.follow) st.follow.cancel = true; },
  },
  clipMenu: (clips, c) => {
    const list = clips.filter((k) => !k.draft && !k.cloud_only);
    if (!list.length) return [];
    const app = platName(presetOf(c.s));
    const items = [{ id: "export", icon: ICON_V, label: list.length > 1 ? t("Export {n} vertical", { n: list.length }) : app ? t("Export vertical for {app}", { app }) : t("Export vertical") }];
    if (clips.length === 1) items.push({ id: "edit", icon: ICON_V, label: t("Open as vertical in the editor") });
    return items;
  },
  onClipMenu: (id, clips, c) => {
    if (id === "export") return void exportClips(clips, c).catch(c.fail);
    if (id === "edit" && clips[0]) { st.pending = clips[0].id; c.clips.open(clips[0].id); }
  },
  onAct: (name, el, c) => {
    if (name === "queue-cancel") return cancelQueue(c);
    const ed = c.editor();
    if (!ed || !ed.model) return;
    switch (name) {
      case "apply": apply(ed, c); if (presetOf(c.s).layout === "follow") follow(ed, c); return ed.redraw();
      case "off": if (st.follow) st.follow.cancel = true; ed.change((m) => { m.canvas = null; }); return ed.redraw();
      case "follow": return void follow(ed, c);
      case "follow-cancel": if (st.follow) st.follow.cancel = true; return;
      case "subs": { let n = 0; ed.change((m) => { n = fitSubs(m, ed, presetOf(c.s)); }); return c.flash(n ? t("The subtitles fit the vertical picture now") : t("There are no subtitles to fit")); }
      case "export": return void ed.export(extrasOf(presetOf(c.s)));
    }
  },
  start: () => {
    document.addEventListener("change", onChange);
    document.addEventListener("input", onInput);
  },
  // For tools/vertical-test.js.
  _pure: { fmtLen, presetOf, kbpsOf, extrasOf, pieceFor, canvasFor, isVertical, motionX, followKeys, lookTimes, pieceKeys, trackOf, wrapLines, subFit, isOver, optsFor, lengthNote },
  lang: {
    ko: {
      "Turns a clip into a 9:16 video for TikTok, YouTube Shorts and Instagram Reels in one click. Fill the screen, keep the whole picture over a blurred background, or let the crop follow the action. Works from a clip's right-click menu and in the editor, where subtitles are fitted too.": "클릭 한 번으로 클립을 TikTok, YouTube Shorts, Instagram Reels용 9:16 영상으로 만들어요. 화면을 가득 채우거나, 흐린 배경 위에 전체 화면을 두거나, 움직임을 따라가게 할 수 있어요. 클립의 오른쪽 클릭 메뉴와 편집기에서 쓸 수 있고, 편집기에서는 자막도 맞춰 줘요.",
      "Vertical": "세로",
      "Glint could not read this clip's picture.": "Glint가 이 클립의 화면을 읽지 못했어요.",
      "Glint could not look at this clip ({reason}).": "Glint가 이 클립을 살펴보지 못했어요 ({reason}).",
      "Open a project in the editor to export it vertical.": "프로젝트는 편집기에서 열어 세로로 내보내세요.",
      "Longer than {app} takes": "{app}에 올리기엔 길어요",
      "This clip is {len} long. {app} takes up to {max}. Cut it at {max}, or export all of it?": "이 클립은 {len} 길이예요. {app}은(는) {max}까지 받아요. {max}에서 자를까요, 전부 내보낼까요?",
      "{n} of these clips are longer than the {max} that {app} takes. Cut them at {max}, or export all of each?": "이 중 {n}개 클립이 {app}이(가) 받는 {max}보다 길어요. {max}에서 자를까요, 각각 전부 내보낼까요?",
      "Cut at {max}": "{max}에서 자르기",
      "Export all of it": "전부 내보내기",
      "Making {title} vertical…": "{title}을(를) 세로로 만드는 중…",
      "Making {n} clips vertical…": "클립 {n}개를 세로로 만드는 중…",
      "Vertical export cancelled": "세로 내보내기를 취소했어요",
      "Vertical clip saved: {title}": "세로 클립 저장됨: {title}",
      "{n} vertical clips saved": "세로 클립 {n}개 저장됨",
      "The crop moves once to stay on the action.": "움직임을 따라 화면이 한 번 이동해요.",
      "The crop moves {n} times to stay on the action.": "움직임을 따라 화면이 {n}번 이동해요.",
      "The action stays in one place, so the crop does too.": "움직임이 한곳에 머물러서 화면도 그대로 있어요.",
      "The crop stays in the middle.": "화면은 가운데에 그대로 있어요.",
      "Any app (no length limit)": "모든 앱 (길이 제한 없음)",
      "Fill the screen": "화면 가득 채우기",
      "Whole picture, blurred behind": "전체 화면, 뒤는 흐리게",
      "Fill and follow the action": "가득 채우고 움직임 따라가기",
      "As recorded": "녹화한 그대로",
      "High": "높음",
      "Balanced": "균형",
      "Small file": "작은 파일",
      "up to {n} Mbps": "최대 {n} Mbps",
      "The picture is cut at the sides so it fills the whole screen.": "화면 전체를 채우도록 양옆을 잘라요.",
      "Nothing is cut away. The space above and below is the clip itself, blurred.": "아무것도 잘리지 않아요. 위아래 빈 곳은 클립을 흐리게 한 화면이에요.",
      "Fills the screen, and the cut moves to where things happen. Best when the action is off to one side, such as a face camera or a replay.": "화면을 가득 채우고, 움직임이 있는 곳으로 화면이 따라가요. 얼굴 카메라나 리플레이처럼 움직임이 한쪽에 있을 때 가장 좋아요.",
      "The result is {len} long.": "결과물 길이는 {len}이에요.",
      "The result is {len} long. {app} takes up to {max}: trim it on the timeline.": "결과물 길이는 {len}이에요. {app}은(는) {max}까지 받으니 타임라인에서 잘라 주세요.",
      "The result is {len} long. {app} takes up to {max}.": "결과물 길이는 {len}이에요. {app}은(는) {max}까지 받아요.",
      "Loading the editor…": "편집기를 불러오는 중…",
      "Makes this clip a {w} × {h} video for TikTok, Shorts and Reels.": "이 클립을 TikTok, Shorts, Reels용 {w} × {h} 영상으로 만들어요.",
      "Picture": "화면",
      "Make it vertical": "세로로 만들기",
      "This clip already has a canvas layout. Making it vertical replaces it.": "이 클립에는 이미 캔버스 배치가 있어요. 세로로 만들면 그 배치가 바뀌어요.",
      "For": "대상",
      "Zoom in": "확대",
      "Left to right": "왼쪽에서 오른쪽",
      "Behind it": "뒤 배경",
      "The clip, blurred": "흐리게 한 클립",
      "Black": "검정",
      "Looking for the action…": "움직임을 찾는 중…",
      "Cancel": "취소",
      "Look again": "다시 찾기",
      "Find the action": "움직임 찾기",
      "Glint looks through the clip on this PC and moves the cut to where things happen.": "Glint가 이 PC에서 클립을 살펴보고 움직임이 있는 곳으로 화면을 옮겨요.",
      "Frame rate": "프레임 속도",
      "Quality": "품질",
      "Fit the subtitles": "자막 맞추기",
      "Wraps the subtitles for the narrow picture and puts them above the app's own buttons.": "좁은 화면에 맞게 자막 줄을 바꾸고 앱의 버튼 위쪽에 놓아요.",
      "Export vertical": "세로로 내보내기",
      "Back to the clip's own shape": "클립의 원래 모양으로",
      "To move or resize the picture by hand, use the Canvas section above.": "화면을 직접 옮기거나 크기를 바꾸려면 위의 캔버스 섹션을 쓰세요.",
      "What is kept of the picture": "화면에서 남는 부분",
      "Looking for the action in {title}…": "{title}에서 움직임을 찾는 중…",
      "Exporting {title}…": "{title} 내보내는 중…",
      "{n} more waiting": "{n}개 더 대기 중",
      "Its progress is in the renders list at the top.": "진행 상황은 위쪽 렌더 목록에 있어요.",
      "Vertical export": "세로 내보내기",
      "A clip as a 9:16 video for TikTok, Shorts and Reels, in one click": "클릭 한 번으로 클립을 TikTok, Shorts, Reels용 9:16 영상으로",
      "How to use it": "사용 방법",
      "In the library": "라이브러리에서",
      "Right-click a clip, or a selection of clips, and pick Export vertical. The new clip appears beside the original.": "클립 하나나 선택한 클립들을 오른쪽 클릭하고 세로로 내보내기를 고르세요. 새 클립이 원본 옆에 생겨요.",
      "In the editor": "편집기에서",
      "Open a clip and press Make it vertical in the Vertical section. You see the result before you export, and can trim it, move the picture and add subtitles.": "클립을 열고 세로 섹션에서 세로로 만들기를 누르세요. 내보내기 전에 결과를 볼 수 있고, 자르고 화면을 옮기고 자막을 넣을 수 있어요.",
      "Made for": "대상 앱",
      "{app} takes videos up to {max}": "{app}은(는) {max}까지의 영상을 받아요",
      "No length limit, and subtitles sit a little lower": "길이 제한이 없고 자막이 조금 더 아래에 놓여요",
      "How it fits": "맞추는 방법",
      "0% shows the whole picture, 100% fills the screen": "0%는 전체 화면, 100%는 화면 가득",
      "Which part of the picture is kept: 50% is the middle": "화면의 어느 부분을 남길지: 50%가 가운데예요",
      "Behind the picture": "화면 뒤 배경",
      "File": "파일",
      "Size": "크기",
      "1080 × 1920 is what the apps show at their best": "1080 × 1920이 앱에서 가장 잘 보이는 크기예요",
      "A clip recorded faster is brought down to this. The apps do not play more than 60": "더 빠르게 녹화한 클립은 이 값으로 낮춰요. 앱은 60보다 높게 재생하지 않아요",
      "The apps compress every upload again, so a larger file than High gains nothing": "앱이 올린 영상을 다시 압축하므로 높음보다 큰 파일은 이득이 없어요",
      "A clip that is too long": "너무 긴 클립",
      "From the library. In the editor you trim it yourself": "라이브러리에서 내보낼 때예요. 편집기에서는 직접 잘라요",
      "Ask me": "물어보기",
      "Cut it at {max}": "{max}에서 자르기",
      "Subtitles": "자막",
      "Fit subtitles to the vertical picture": "자막을 세로 화면에 맞추기",
      "In the editor, lines made with the Subtitles plugin are wrapped for the narrow picture and moved above the app's own buttons": "편집기에서 자막 플러그인으로 만든 줄을 좁은 화면에 맞게 바꾸고 앱의 버튼 위쪽으로 옮겨요",
      "Text size": "글자 크기",
      "As a part of the picture's height": "화면 높이에 대한 비율",
      "Export {n} vertical": "{n}개 세로로 내보내기",
      "Export vertical for {app}": "{app}용 세로로 내보내기",
      "Open as vertical in the editor": "편집기에서 세로로 열기",
      "The subtitles fit the vertical picture now": "자막을 세로 화면에 맞췄어요",
      "There are no subtitles to fit": "맞출 자막이 없어요",
    },
    el: {
      "Turns a clip into a 9:16 video for TikTok, YouTube Shorts and Instagram Reels in one click. Fill the screen, keep the whole picture over a blurred background, or let the crop follow the action. Works from a clip's right-click menu and in the editor, where subtitles are fitted too.": "Μετατρέπει ένα κλιπ σε βίντεο 9:16 για TikTok, YouTube Shorts και Instagram Reels με ένα κλικ. Γεμίστε την οθόνη, κρατήστε ολόκληρη την εικόνα πάνω σε θολό φόντο ή αφήστε την περικοπή να ακολουθεί τη δράση. Λειτουργεί από το μενού δεξιού κλικ ενός κλιπ και στον επεξεργαστή, όπου προσαρμόζονται και οι υπότιτλοι.",
      "Vertical": "Κάθετο",
      "Glint could not read this clip's picture.": "Το Glint δεν μπόρεσε να διαβάσει την εικόνα αυτού του κλιπ.",
      "Glint could not look at this clip ({reason}).": "Το Glint δεν μπόρεσε να εξετάσει αυτό το κλιπ ({reason}).",
      "Open a project in the editor to export it vertical.": "Ανοίξτε το έργο στον επεξεργαστή για να το εξαγάγετε κάθετα.",
      "Longer than {app} takes": "Μεγαλύτερο από όσο δέχεται το {app}",
      "This clip is {len} long. {app} takes up to {max}. Cut it at {max}, or export all of it?": "Αυτό το κλιπ διαρκεί {len}. Το {app} δέχεται έως {max}. Να κοπεί στο {max} ή να εξαχθεί ολόκληρο;",
      "{n} of these clips are longer than the {max} that {app} takes. Cut them at {max}, or export all of each?": "{n} από αυτά τα κλιπ είναι μεγαλύτερα από το {max} που δέχεται το {app}. Να κοπούν στο {max} ή να εξαχθούν ολόκληρα;",
      "Cut at {max}": "Κοπή στο {max}",
      "Export all of it": "Εξαγωγή ολόκληρου",
      "Making {title} vertical…": "Το {title} γίνεται κάθετο…",
      "Making {n} clips vertical…": "{n} κλιπ γίνονται κάθετα…",
      "Vertical export cancelled": "Η κάθετη εξαγωγή ακυρώθηκε",
      "Vertical clip saved: {title}": "Το κάθετο κλιπ αποθηκεύτηκε: {title}",
      "{n} vertical clips saved": "Αποθηκεύτηκαν {n} κάθετα κλιπ",
      "The crop moves once to stay on the action.": "Η περικοπή μετακινείται μία φορά για να μένει στη δράση.",
      "The crop moves {n} times to stay on the action.": "Η περικοπή μετακινείται {n} φορές για να μένει στη δράση.",
      "The action stays in one place, so the crop does too.": "Η δράση μένει σε ένα σημείο, άρα και η περικοπή.",
      "The crop stays in the middle.": "Η περικοπή μένει στη μέση.",
      "Any app (no length limit)": "Οποιαδήποτε εφαρμογή (χωρίς όριο διάρκειας)",
      "Fill the screen": "Γέμισμα της οθόνης",
      "Whole picture, blurred behind": "Ολόκληρη η εικόνα, θολό φόντο",
      "Fill and follow the action": "Γέμισμα και παρακολούθηση της δράσης",
      "As recorded": "Όπως καταγράφηκε",
      "High": "Υψηλή",
      "Balanced": "Ισορροπημένη",
      "Small file": "Μικρό αρχείο",
      "up to {n} Mbps": "έως {n} Mbps",
      "The picture is cut at the sides so it fills the whole screen.": "Η εικόνα κόβεται στα πλάγια ώστε να γεμίζει όλη την οθόνη.",
      "Nothing is cut away. The space above and below is the clip itself, blurred.": "Δεν κόβεται τίποτα. Ο χώρος πάνω και κάτω είναι το ίδιο το κλιπ, θολωμένο.",
      "Fills the screen, and the cut moves to where things happen. Best when the action is off to one side, such as a face camera or a replay.": "Γεμίζει την οθόνη και η περικοπή πηγαίνει εκεί όπου συμβαίνουν πράγματα. Ιδανικό όταν η δράση είναι στο πλάι, όπως μια κάμερα προσώπου ή μια επανάληψη.",
      "The result is {len} long.": "Το αποτέλεσμα διαρκεί {len}.",
      "The result is {len} long. {app} takes up to {max}: trim it on the timeline.": "Το αποτέλεσμα διαρκεί {len}. Το {app} δέχεται έως {max}: κόψτε το στη γραμμή χρόνου.",
      "The result is {len} long. {app} takes up to {max}.": "Το αποτέλεσμα διαρκεί {len}. Το {app} δέχεται έως {max}.",
      "Loading the editor…": "Φόρτωση του επεξεργαστή…",
      "Makes this clip a {w} × {h} video for TikTok, Shorts and Reels.": "Κάνει αυτό το κλιπ βίντεο {w} × {h} για TikTok, Shorts και Reels.",
      "Picture": "Εικόνα",
      "Make it vertical": "Κάντε το κάθετο",
      "This clip already has a canvas layout. Making it vertical replaces it.": "Αυτό το κλιπ έχει ήδη διάταξη καμβά. Αν γίνει κάθετο, αντικαθίσταται.",
      "For": "Για",
      "Zoom in": "Μεγέθυνση",
      "Left to right": "Αριστερά προς δεξιά",
      "Behind it": "Πίσω της",
      "The clip, blurred": "Το κλιπ, θολωμένο",
      "Black": "Μαύρο",
      "Looking for the action…": "Αναζήτηση της δράσης…",
      "Cancel": "Άκυρο",
      "Look again": "Νέα αναζήτηση",
      "Find the action": "Εύρεση της δράσης",
      "Glint looks through the clip on this PC and moves the cut to where things happen.": "Το Glint εξετάζει το κλιπ σε αυτόν τον υπολογιστή και μετακινεί την περικοπή εκεί όπου συμβαίνουν πράγματα.",
      "Frame rate": "Ρυθμός καρέ",
      "Quality": "Ποιότητα",
      "Fit the subtitles": "Προσαρμογή υποτίτλων",
      "Wraps the subtitles for the narrow picture and puts them above the app's own buttons.": "Αναδιπλώνει τους υπότιτλους για τη στενή εικόνα και τους βάζει πάνω από τα κουμπιά της εφαρμογής.",
      "Export vertical": "Κάθετη εξαγωγή",
      "Back to the clip's own shape": "Πίσω στο σχήμα του κλιπ",
      "To move or resize the picture by hand, use the Canvas section above.": "Για να μετακινήσετε ή να αλλάξετε μέγεθος στην εικόνα με το χέρι, χρησιμοποιήστε την ενότητα Καμβάς πιο πάνω.",
      "What is kept of the picture": "Τι κρατιέται από την εικόνα",
      "Looking for the action in {title}…": "Αναζήτηση της δράσης στο {title}…",
      "Exporting {title}…": "Εξαγωγή του {title}…",
      "{n} more waiting": "{n} ακόμη σε αναμονή",
      "Its progress is in the renders list at the top.": "Η πρόοδος φαίνεται στη λίστα αποδόσεων επάνω.",
      "Vertical export": "Κάθετη εξαγωγή",
      "A clip as a 9:16 video for TikTok, Shorts and Reels, in one click": "Ένα κλιπ ως βίντεο 9:16 για TikTok, Shorts και Reels, με ένα κλικ",
      "How to use it": "Πώς χρησιμοποιείται",
      "In the library": "Στη βιβλιοθήκη",
      "Right-click a clip, or a selection of clips, and pick Export vertical. The new clip appears beside the original.": "Κάντε δεξί κλικ σε ένα κλιπ ή σε μια επιλογή κλιπ και διαλέξτε Κάθετη εξαγωγή. Το νέο κλιπ εμφανίζεται δίπλα στο αρχικό.",
      "In the editor": "Στον επεξεργαστή",
      "Open a clip and press Make it vertical in the Vertical section. You see the result before you export, and can trim it, move the picture and add subtitles.": "Ανοίξτε ένα κλιπ και πατήστε Κάντε το κάθετο στην ενότητα Κάθετο. Βλέπετε το αποτέλεσμα πριν την εξαγωγή και μπορείτε να το κόψετε, να μετακινήσετε την εικόνα και να προσθέσετε υπότιτλους.",
      "Made for": "Φτιαγμένο για",
      "{app} takes videos up to {max}": "Το {app} δέχεται βίντεο έως {max}",
      "No length limit, and subtitles sit a little lower": "Χωρίς όριο διάρκειας, και οι υπότιτλοι μπαίνουν λίγο πιο χαμηλά",
      "How it fits": "Πώς χωράει",
      "0% shows the whole picture, 100% fills the screen": "Το 0% δείχνει όλη την εικόνα, το 100% γεμίζει την οθόνη",
      "Which part of the picture is kept: 50% is the middle": "Ποιο μέρος της εικόνας κρατιέται: το 50% είναι η μέση",
      "Behind the picture": "Πίσω από την εικόνα",
      "File": "Αρχείο",
      "Size": "Μέγεθος",
      "1080 × 1920 is what the apps show at their best": "Το 1080 × 1920 είναι αυτό που δείχνουν καλύτερα οι εφαρμογές",
      "A clip recorded faster is brought down to this. The apps do not play more than 60": "Ένα κλιπ με περισσότερα καρέ κατεβαίνει σε αυτό. Οι εφαρμογές δεν παίζουν πάνω από 60",
      "The apps compress every upload again, so a larger file than High gains nothing": "Οι εφαρμογές συμπιέζουν ξανά κάθε ανέβασμα, άρα αρχείο μεγαλύτερο από το Υψηλή δεν προσφέρει κάτι",
      "A clip that is too long": "Κλιπ που είναι πολύ μεγάλο",
      "From the library. In the editor you trim it yourself": "Από τη βιβλιοθήκη. Στον επεξεργαστή το κόβετε εσείς",
      "Ask me": "Να με ρωτά",
      "Cut it at {max}": "Κοπή στο {max}",
      "Subtitles": "Υπότιτλοι",
      "Fit subtitles to the vertical picture": "Προσαρμογή υποτίτλων στην κάθετη εικόνα",
      "In the editor, lines made with the Subtitles plugin are wrapped for the narrow picture and moved above the app's own buttons": "Στον επεξεργαστή, οι γραμμές του πρόσθετου Υπότιτλοι αναδιπλώνονται για τη στενή εικόνα και μπαίνουν πάνω από τα κουμπιά της εφαρμογής",
      "Text size": "Μέγεθος κειμένου",
      "As a part of the picture's height": "Ως μέρος του ύψους της εικόνας",
      "Export {n} vertical": "Κάθετη εξαγωγή {n} κλιπ",
      "Export vertical for {app}": "Κάθετη εξαγωγή για {app}",
      "Open as vertical in the editor": "Άνοιγμα ως κάθετο στον επεξεργαστή",
      "The subtitles fit the vertical picture now": "Οι υπότιτλοι ταιριάζουν πλέον στην κάθετη εικόνα",
      "There are no subtitles to fit": "Δεν υπάρχουν υπότιτλοι για προσαρμογή",
    },
  },
});
