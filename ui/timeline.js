"use strict";
// The editor's multi-track timeline. Loaded before app.js and editor.js: nothing here may touch their globals at load time.
//
// Defines one global, mountTimeline(host, opts) -> handle, plus private helpers named tl...
// It draws the edit model (editmodel.js) and changes it only through the em... functions.
//
// Layout inside the host (class "tl"):
//   .tl-bar     toolbar
//   .tl-body    grid: .tl-heads (fixed header column) | .tl-lanes (ruler + one canvas per row + overlays)
//   .tl-sbar    the horizontal scrollbar, under the lanes
// The canvases are only as wide as the visible lane: everything is drawn for the
// current scroll position, so a 10 minute clip at 400 px/s costs the same as a short one.
// Times are source seconds. Lane x of a time: tlPad + view(sec) * pps - scrollX, where
// view() is the identity, or with "Hide cut parts" on, the position in the finished clip
// (removed segments take no room). The ruler is drawn in view time.
//
// Selection: a list of pieces { row, seg } (row = "video" or an audio row id) with one
// primary. Toolbar actions, the right-click menu (a .menu element of our own, appended
// to <body>) and handle.run() act on all of them.
//
// NOTE: never name a variable `t` here: it would hide the translation function t().

const tlRulerH = 26;    // px, ruler height (keep in sync with timeline.css)
const tlVideoH = 56;    // px, video row height
const tlAudioH = 50;    // px, audio row height
const tlPad = 14;       // px of empty lane before 0 and after the end, so the end handles can be grabbed
const tlMaxPps = 400;   // most pixels per second
const tlSnapPx = 6;     // snapping distance
const tlEdgePx = 6;     // grab zone of a file clip's edges
const tlBarStep = 3;    // waveform: one bar every 3 px
const tlBarW = 2;       // waveform bar width
const tlDragPx = 3;     // movement before a press becomes a drag

// Ruler steps: [label step, candidate tick steps (finest first)], seconds.
const tlSteps = [
  [0.1, [0.02, 0.05]], [0.2, [0.05, 0.1]], [0.5, [0.1, 0.25]], [1, [0.1, 0.25, 0.5]], [2, [0.25, 0.5, 1]],
  [5, [0.5, 1, 2.5]], [10, [1, 2, 5]], [15, [1, 3, 5]], [30, [5, 10]], [60, [5, 10, 15, 30]],
  [120, [10, 30, 60]], [300, [30, 60]], [600, [60, 120, 300]], [1800, [300, 600]], [3600, [600, 1200]],
];

// Icon paths (24 x 24, stroked). Used both as inline SVG and as canvas Path2D.
const tlPaths = {
  split: "M12 3v18M8 8l-4 4 4 4M16 8l4 4-4 4",
  restore: "M4 12a8 8 0 1 0 2.4-5.7M4 4v5h5",
  undo: "M9 14 4 9l5-5M4 9h10a6 6 0 0 1 0 12h-3",
  redo: "m15 14 5-5-5-5M20 9H10a6 6 0 0 0 0 12h3",
  zoomIn: "M11 4a7 7 0 1 1 0 14a7 7 0 1 1 0-14zM16.2 16.2 21 21M8 11h6M11 8v6",
  zoomOut: "M11 4a7 7 0 1 1 0 14a7 7 0 1 1 0-14zM16.2 16.2 21 21M8 11h6",
  fit: "M4 5v14M20 5v14M7 12h10M7 12l3-3M7 12l3 3M17 12l-3-3M17 12l-3 3",
  film: "M5 5h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2zM7 5v14M17 5v14M3 9h4M3 15h4M17 9h4M17 15h4",
  note: "M9 18V6l10-2v12M9 18a2.5 2.5 0 1 1-5 0a2.5 2.5 0 1 1 5 0zM19 16a2.5 2.5 0 1 1-5 0a2.5 2.5 0 1 1 5 0z",
  level: "M5 19v-4M10 19v-8M15 19V8M20 19V4",
  muted: "M4 9.5v5h3.5l4.5 4v-13l-4.5 4zM16 9.5l5 5M21 9.5l-5 5",
  vol: "M4 9.5v5h3.5l4.5 4v-13l-4.5 4zM16 9a4 4 0 0 1 0 6M18.5 6.5a7.5 7.5 0 0 1 0 11",
  trash: "M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13",
  eye: "M2.5 12C3.5 10.5 7 6 12 6s8.5 4.5 9.5 6c-1 1.5-4.5 6-9.5 6s-8.5-4.5-9.5-6zM12 9a3 3 0 1 1 0 6a3 3 0 1 1 0-6z",
  eyeOff: "M3 3l18 18M10.6 6.1A9.5 9.5 0 0 1 12 6c5 0 8.5 4.5 9.5 6a17 17 0 0 1-2.8 3.4M6.6 7.6C4.5 9 3.1 11 2.5 12c1 1.5 4.5 6 9.5 6a9 9 0 0 0 4.4-1.1M9.9 9.9a3 3 0 0 0 4.2 4.2",
  solo: "M4 16v-4a8 8 0 0 1 16 0v4M4 15h3v5H4zM17 15h3v5h-3z",
  join: "M12 4v5M12 15v5M4 12h16M7 9l-3 3 3 3M17 9l3 3-3 3",
  hideCuts: "M3 12h5M16 12h5M5 9l3 3-3 3M19 9l-3 3 3 3M12 5v14",
  selAll: "M4 4h16v16H4zM8 12l3 3 5-6",
  keys: "M3 6h18v12H3zM7 10h1M11 10h1M15 10h1M7 14h10",
  full: "M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5",
};
const tlSvg = (d) => `<svg viewBox="0 0 24 24" aria-hidden="true"><path d="${d}"/></svg>`;
let tlPath2d = null; // Path2D objects, made on first draw

/* ---------- pure helpers (tested under Node) ---------- */

/** Label step and tick step for a zoom (pixels per second): labels at least `minLabelPx` apart, ticks at least `minTickPx`. */
function tlPickStep(pps, minLabelPx = 72, minTickPx = 8) {
  let major = tlSteps[tlSteps.length - 1];
  for (const s of tlSteps) if (s[0] * pps >= minLabelPx) { major = s; break; }
  let minor = major[0];
  for (const c of major[1]) if (c * pps >= minTickPx) { minor = c; break; }
  return { major: major[0], minor };
}

/** Ruler label: "0:02" for whole-second steps, "0:02.5" below one second. */
function tlFmtTick(sec, step) {
  const tenths = step < 1;
  const total = Math.round(Math.max(0, sec) * (tenths ? 10 : 1));
  const s = tenths ? Math.floor(total / 10) : total;
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}${tenths ? "." + (total % 10) : ""}`;
}

/** Drag tooltip: "1:02.35". */
function tlFmtPrecise(sec) {
  const cs = Math.round(Math.max(0, sec) * 100);
  const s = Math.floor(cs / 100);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}.${String(cs % 100).padStart(2, "0")}`;
}

/** Nearest candidate within `tol` seconds, else `sec` itself. */
function tlSnap(sec, cands, tol) {
  let best = sec, dist = tol, hit = false;
  for (const c of cands) {
    const d = Math.abs(c - sec);
    if (d <= dist) { dist = d; best = c; hit = true; }
  }
  return { at: best, hit, dist: hit ? dist : Infinity };
}

/** Which part of a file clip is under time `sec`: "left" | "right" | "move" | null. */
const tlFileHit = (row, sec, pps) => tlSpanHit(row.offset, row.offset + row.length, sec, pps);
/** Same for any span [a, b] and position `sec` on one time axis (source or view). An empty span is never hit. */
function tlSpanHit(a, b, sec, pps) {
  if (!(b > a)) return null;
  const edge = Math.min(tlEdgePx, ((b - a) * pps) / 3) / pps;
  if (Math.abs(sec - a) <= edge) return "left";
  if (Math.abs(sec - b) <= edge) return "right";
  if (sec > a && sec < b) return "move";
  return null;
}

/** Like tlSnap, but distances are measured after mapping with `toV` (source -> view); returns the source candidate. */
function tlSnapV(sec, cands, tol, toV) {
  const v = toV(sec);
  let best = sec, dist = tol, hit = false;
  for (const c of cands) {
    const d = Math.abs(toV(c) - v);
    if (d <= dist) { dist = d; best = c; hit = true; }
  }
  return { at: best, hit, dist: hit ? dist : Infinity };
}

/* View mapping. Off: view time = source time. On ("Hide cut parts"): removed parts
   take no room, view time = position in the finished clip (speed not applied).
   `kept` is emKept(m): merged kept source ranges [{ start, end }]. */
function tlViewMap(kept, collapse, duration) {
  if (!collapse) return { collapse: false, total: Math.max(0, duration), ranges: [] };
  let v = 0;
  const ranges = (kept || []).map((r) => { const o = { s0: r.start, s1: r.end, v0: v }; v += r.end - r.start; return o; });
  return { collapse: true, total: v, ranges, lastSrc: Math.max(0, duration) };
}
/** Last range starting at or before `x` on key `k` ("s0" or "v0"), by binary search; -1 if none. */
function tlRangeIdx(ranges, k, x) {
  let lo = 0, hi = ranges.length - 1, ans = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (ranges[mid][k] <= x) { ans = mid; lo = mid + 1; } else hi = mid - 1;
  }
  return ans;
}
/** Source time -> view time. Inside a removed part: the joint where it was cut. Before 0 / after the end: linear (the lane's padding). */
function tlSrcToView(map, sec) {
  if (!map.collapse) return sec;
  const R = map.ranges;
  if (sec < 0 || !R.length) return Math.min(0, sec);
  const i = tlRangeIdx(R, "s0", sec);
  if (i < 0) return 0;
  const r = R[i];
  if (sec <= r.s1) return r.v0 + (sec - r.s0);
  if (i === R.length - 1 && sec > map.lastSrc) return map.total + (sec - map.lastSrc);
  return r.v0 + (r.s1 - r.s0);
}
/** View time -> source time. At a joint: the start of the kept part after it. Clamped to the kept parts. */
function tlViewToSrc(map, v) {
  if (!map.collapse) return v;
  const R = map.ranges;
  if (!R.length) return 0;
  if (v <= 0) return R[0].s0;
  if (v >= map.total) return R[R.length - 1].s1;
  const r = R[Math.max(0, tlRangeIdx(R, "v0", v))];
  return Math.min(r.s1, r.s0 + (v - r.v0));
}

/** The removed runs (neighbouring removed segments together): [{ first, last, start, end }], segment indexes inclusive. */
function tlJoints(removed, edges) {
  const out = [];
  for (let i = 0; i < removed.length; i++) {
    if (!removed[i]) continue;
    const last = out[out.length - 1];
    if (last && last.last === i - 1) { last.last = i; last.end = edges[i + 1]; }
    else out.push({ first: i, last: i, start: edges[i], end: edges[i + 1] });
  }
  return out;
}

/* Selection lists: arrays of { row, seg }, no duplicates, in the order added. */
const tlSelKey = (s) => `${s.row}\u0000${s.seg}`;
const tlSelHas = (list, s) => !!s && list.some((x) => x.row === s.row && x.seg === s.seg);
/** `list` plus every piece of `add` not in it yet. */
function tlSelMerge(list, add) {
  const seen = new Set(list.map(tlSelKey)), out = list.map((s) => ({ row: s.row, seg: s.seg }));
  for (const s of add) { const k = tlSelKey(s); if (!seen.has(k)) { seen.add(k); out.push({ row: s.row, seg: s.seg }); } }
  return out;
}
/** `list` with `s` added, or taken out if it was in. */
const tlSelToggle = (list, s) => (tlSelHas(list, s) ? list.filter((x) => !(x.row === s.row && x.seg === s.seg)) : tlSelMerge(list, [s]));
/** Every piece in the rectangle of rows (by their place in `order`) x segments spanned by `a` and `b`, that `has(row, seg)` accepts. */
function tlSelRect(order, a, b, has) {
  const ra = order.indexOf(a.row), rb = order.indexOf(b.row);
  if (ra < 0 || rb < 0) return [];
  const out = [];
  for (let r = Math.min(ra, rb); r <= Math.max(ra, rb); r++) {
    for (let sg = Math.min(a.seg, b.seg); sg <= Math.max(a.seg, b.seg); sg++) if (has(order[r], sg)) out.push({ row: order[r], seg: sg });
  }
  return out;
}
/**
 * Rubber band hit test. `rows`: [{ id, y0, y1, pieces: [{ seg, x0, x1 }] }], `box`: { x0, x1, y0, y1 }
 * (any corner order), all in one coordinate space. A piece is hit when it overlaps the box by any
 * amount; pieces narrower than half a pixel (hidden ones) never are.
 */
function tlBandHits(rows, box) {
  const x0 = Math.min(box.x0, box.x1), x1 = Math.max(box.x0, box.x1), y0 = Math.min(box.y0, box.y1), y1 = Math.max(box.y0, box.y1);
  const out = [];
  for (const r of rows) {
    if (!(r.y0 < y1 && r.y1 > y0)) continue;
    for (const p of r.pieces) if (p.x1 - p.x0 >= 0.5 && p.x0 < x1 && p.x1 > x0) out.push({ row: r.id, seg: p.seg });
  }
  return out;
}
/** Selection after its segment indexes moved: a cut was added before segment `at` + 1 (delta 1) or cut `at` was joined (delta -1). */
function tlSelShift(list, at, delta) {
  return tlSelMerge([], list.map((s) => (s.seg > at ? { row: s.row, seg: s.seg + delta } : s)));
}

/** "3.2 s" style length for a tooltip. */
const tlFmtLen = (sec) => (Math.round(Math.max(0, sec) * 10) / 10).toFixed(1);

/** Left-edge trim of a file clip: the new start `at`, keeping the audio where it is. Returns { offset, trimStart, length }. */
function tlTrimLeft(row0, at, minSeg) {
  let d = at - row0.offset;
  d = Math.max(d, -row0.trimStart, -row0.offset);
  d = Math.min(d, row0.length - minSeg);
  return { offset: row0.offset + d, trimStart: row0.trimStart + d, length: row0.length - d };
}

/** Largest peak in index range [i0, i1) (at least one index), 0 outside the array. */
function tlPeakMax(peaks, i0, i1) {
  const n = peaks.length;
  let a = Math.floor(i0), b = Math.max(a + 1, Math.ceil(i1));
  if (b <= 0 || a >= n) return 0;
  a = Math.max(0, a);
  b = Math.min(n, b);
  let mx = 0;
  for (let i = a; i < b; i++) { const p = peaks[i]; if (p > mx) mx = p; }
  return mx > 1 ? 1 : mx;
}

/** Hue (0..360) of a "#rgb", "#rrggbb" or "rgb(...)" colour, or null. */
function tlHue(color) {
  const s = String(color || "").trim();
  let r, g, b, mm;
  if ((mm = /^#([0-9a-f]{3})$/i.exec(s))) [r, g, b] = [...mm[1]].map((h) => parseInt(h + h, 16));
  else if ((mm = /^#([0-9a-f]{6})/i.exec(s))) [r, g, b] = [0, 2, 4].map((i) => parseInt(mm[1].slice(i, i + 2), 16));
  else if ((mm = /^rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)/i.exec(s))) [r, g, b] = [+mm[1], +mm[2], +mm[3]];
  else return null;
  const mx = Math.max(r, g, b), mn = Math.min(r, g, b), d = mx - mn;
  if (d === 0) return null;
  const h = mx === r ? ((g - b) / d) % 6 : mx === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return (h * 60 + 360) % 360;
}
const tlHueDist = (a, b) => (a === null || b === null ? 180 : Math.min(Math.abs(a - b), 360 - Math.abs(a - b)));

/** Cut text to fit `max` px with an ellipsis. */
function tlEllipsize(ctx, text, max) {
  if (ctx.measureText(text).width <= max) return text;
  let lo = 0, hi = text.length;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (ctx.measureText(text.slice(0, mid) + "…").width <= max) lo = mid; else hi = mid - 1;
  }
  return lo ? text.slice(0, lo) + "…" : "";
}

/* ---------- the timeline ---------- */

function mountTimeline(host, opts) {
  const m = opts.model;
  const dur = () => Math.max(0, +m.duration || 0);
  const st = {
    pps: 100, scrollX: 0, laneW: 0, sbW: 0, dpr: window.devicePixelRatio || 1,
    fitted: true,          // follow the width when the lane is resized
    sel: null,             // the primary selected piece { row, seg }; always one of sels
    sels: [],              // every selected piece [{ row, seg }]
    selNotify: false,      // the selection's indexes were shifted: tell the editor on the next setSels
    hideCuts: false,       // "Hide cut parts": removed segments take no room
    map: null,             // view mapping (tlViewMap), rebuilt on demand after markAll
    joints: [], jointSig: "",
    menu: null,            // the open right-click menu { el, close }
    thumbs: [],            // [{ img, ok }]
    waves: new Map(),      // row id -> { peaks, ps }
    rowsSig: null,
    lanes: [],             // [{ id, kind, el, canvas, ctx, h }]
    dirtyAll: true, dirty: new Set(), dirtyRuler: true,
    needSize: true,
    drag: null,            // the press / drag in progress
    gest: null,            // the range input being dragged (one snapshot per gesture)
    hold: null,            // { at, until }: show this time until getTime() catches up after a seek
    shownT: 0, lastT: NaN, phX: NaN, phVis: null,
    bar: {},               // last toolbar state, to touch the DOM only on change
    hatch: null, hatchKey: "",
    raf: 0, dead: false, swallowTimer: 0,
  };
  try { st.hideCuts = localStorage.getItem("glint.tl.hideCuts") === "1"; } catch {}

  /* ----- DOM ----- */
  host.classList.add("tl");
  const btn = (act, path, title, extra = "") => `<button type="button" class="tl-btn" data-tl-act="${act}" title="${esc(title)}" aria-label="${esc(title)}"${extra}>${tlSvg(path)}</button>`;
  host.innerHTML = `
    <div class="tl-bar" role="toolbar" aria-label="${esc(t("Timeline"))}">
      ${btn("split", tlPaths.split, t("Split at the playhead (S)"))}
      <button type="button" class="tl-btn" data-tl-act="remove"></button>
      <button type="button" class="tl-btn" data-tl-act="mute"></button>
      <span class="tl-level" title="${esc(t("Level of the selected audio segment"))}">${tlSvg(tlPaths.level)}<input type="range" class="tl-range" min="0" max="200" step="5" value="100" data-tl-level aria-label="${esc(t("Segment level"))}"><output class="tl-pct"></output></span>
      <span class="tl-sep"></span>
      ${btn("undo", tlPaths.undo, t("Undo (Ctrl+Z)"))}
      ${btn("redo", tlPaths.redo, t("Redo (Ctrl+Y)"))}
      <span class="tl-sp"></span>
      ${btn("zoom-out", tlPaths.zoomOut, t("Zoom out (-)"))}
      ${btn("zoom-in", tlPaths.zoomIn, t("Zoom in (+)"))}
      ${btn("fit", tlPaths.fit, t("Fit the whole clip (Shift+F)"))}
      ${btn("hide-cuts", tlPaths.hideCuts, t("Hide cut parts (H)"), ' aria-pressed="false"')}
      <span class="tl-sep"></span>
      <button type="button" class="btn sm tl-add" data-tl-act="add" title="${esc(t("Add an audio file as a new track"))}">${ICON.plus}<span>${esc(t("Add audio"))}</span></button>
    </div>
    <div class="tl-body">
      <div class="tl-heads"><div class="tl-corner"></div><div class="tl-hrows"></div></div>
      <div class="tl-lanes">
        <div class="tl-ruler" data-tl-ruler title="${esc(t("Click or drag to move the playhead"))}"><canvas></canvas><div class="tl-ph-k" data-tl-ph></div></div>
        <div class="tl-rows"></div>
        <div class="tl-marks"><div class="tl-joints"></div><div class="tl-cuts"></div><div class="tl-ph"></div></div>
        <div class="tl-band" hidden></div>
        <div class="tl-tip" hidden></div>
      </div>
    </div>
    <div class="tl-sbar"><div></div><div class="tl-scroll"><div class="tl-spacer"></div></div></div>`;
  const q = (sel) => host.querySelector(sel);
  const el = {
    bar: q(".tl-bar"), hrows: q(".tl-hrows"), lanes: q(".tl-lanes"), rows: q(".tl-rows"),
    ruler: q(".tl-ruler canvas"), phK: q(".tl-ph-k"), ph: q(".tl-ph"), cuts: q(".tl-cuts"), joints: q(".tl-joints"), tip: q(".tl-tip"), band: q(".tl-band"),
    bHide: q('[data-tl-act="hide-cuts"]'),
    scroll: q(".tl-scroll"), spacer: q(".tl-spacer"),
    level: q("[data-tl-level]"), levelPct: q(".tl-level .tl-pct"), levelBox: q(".tl-level"),
    bSplit: q('[data-tl-act="split"]'), bRemove: q('[data-tl-act="remove"]'), bMute: q('[data-tl-act="mute"]'),
    bUndo: q('[data-tl-act="undo"]'), bRedo: q('[data-tl-act="redo"]'), bOut: q('[data-tl-act="zoom-out"]'), bIn: q('[data-tl-act="zoom-in"]'),
  };
  const rulerCtx = el.ruler.getContext("2d");

  /* ----- mapping ----- */
  // Source time <-> view time (see the file header). The map is rebuilt after every markAll, so it follows the model.
  const mapSig = () => (st.hideCuts ? `${m.duration}|${m.cuts.join(",")}|${m.removed.join(",")}` : "");
  const vmap = () => (st.map ||= Object.assign(tlViewMap(st.hideCuts ? emKept(m) : null, st.hideCuts, dur()), { sig: mapSig() }));
  const toV = (sec) => tlSrcToView(vmap(), sec);
  const toS = (v) => tlViewToSrc(vmap(), v);
  const vdur = () => vmap().total;
  const XV = (v) => tlPad + v * st.pps - st.scrollX;
  const vAtX = (x) => (x + st.scrollX - tlPad) / st.pps;
  const X = (sec) => XV(toV(sec));
  const timeAtX = (x) => toS(vAtX(x));
  const contentW = () => vdur() * st.pps + 2 * tlPad;
  const minPps = () => (st.laneW > 2 * tlPad + 10 && vdur() > 0 ? (st.laneW - 2 * tlPad) / vdur() : 1);
  const maxPps = () => Math.max(tlMaxPps, minPps());
  const markAll = () => { st.dirtyAll = true; st.dirtyRuler = true; st.map = null; };
  /** A segment is drawn (and can be picked) unless it is cut out while cut parts are hidden. */
  const segShown = (i) => !(st.hideCuts && m.removed[i]);
  /** Zoom and scroll after the view length may have changed (model change, hide cut parts). */
  function relayout() {
    if (!st.laneW) return;
    if (st.fitted) setPps(minPps()); else setPps(st.pps);
    syncSpacer();
    setScroll(st.scrollX);
  }

  function setScroll(x) {
    const mx = Math.max(0, contentW() - st.laneW);
    const v = Math.min(mx, Math.max(0, x));
    if (v === st.scrollX) return;
    st.scrollX = v;
    if (Math.abs(el.scroll.scrollLeft - v) >= 1) el.scroll.scrollLeft = v;
    markAll();
  }
  function syncSpacer() {
    // max scrollLeft of the scrollbar = contentW - laneW, whatever the scrollbar's own width.
    el.spacer.style.width = `${Math.max(0, contentW() - st.laneW) + st.sbW}px`;
  }
  function setPps(p) {
    if (!st.laneW) return; // not laid out yet: the first resize fits (or clamps) the zoom
    const v = Math.min(maxPps(), Math.max(minPps(), p));
    st.fitted = v <= minPps() * 1.0001;
    if (v !== st.pps) { st.pps = v; markAll(); }
    syncSpacer();
  }
  /** Zoom keeping the time under lane x `x` where it is. */
  function zoomAround(factor, x) {
    const at = vAtX(x);
    setPps(st.pps * factor);
    syncSpacer();
    setScroll(tlPad + at * st.pps - x);
    updateBar();
  }
  function zoom(factor) {
    if (!(factor > 0)) throw new Error(`timeline zoom: bad factor ${factor}`);
    const xp = X(st.shownT);
    if (xp >= 0 && xp <= st.laneW) return zoomAround(factor, xp);
    setPps(st.pps * factor);
    setScroll(tlPad + toV(st.shownT) * st.pps - st.laneW / 2);
    updateBar();
  }
  function fit() {
    st.fitted = true;
    if (!st.laneW) return; // done on the first resize
    setPps(minPps());
    setScroll(0);
    updateBar();
  }

  /* ----- rows ----- */
  const rowSig = () => m.audio.map((r) => `${r.id}/${r.kind}`).join("|");
  function headHtml(r) {
    const file = r.kind === "file";
    return `<div class="tl-head${r.muted ? " off" : ""}" data-tl-row="${esc(r.id)}" style="height:${tlAudioH}px">
      <div class="tl-hline">
        <button type="button" class="tl-btn tl-hmute" data-tl-act="row-mute" data-tl-row="${esc(r.id)}"></button>
        <span class="tl-hname"></span>
        ${file ? `<button type="button" class="tl-btn tl-hx" data-tl-act="row-remove" data-tl-row="${esc(r.id)}" title="${esc(t("Remove this audio file"))}" aria-label="${esc(t("Remove this audio file"))}">${ICON.trash}</button>` : ""}
      </div>
      <div class="tl-hline"><input type="range" class="tl-range" min="0" max="200" step="1" data-tl-vol="${esc(r.id)}" aria-label="${esc(t("Track volume"))}"><output class="tl-pct"></output></div>
    </div>`;
  }
  function buildRows() {
    st.rowsSig = rowSig();
    el.hrows.innerHTML = `<div class="tl-head tl-head-video" style="height:${tlVideoH}px" title="${esc(opts.title || t("Video"))}" aria-label="${esc(t("Video"))}">${tlSvg(tlPaths.film)}</div>` + m.audio.map(headHtml).join("");
    const list = [{ id: "video", kind: "video", h: tlVideoH }, ...m.audio.map((r) => ({ id: r.id, kind: r.kind, h: tlAudioH }))];
    el.rows.innerHTML = list.map((L) => `<div class="tl-lane" data-tl-lane="${esc(L.id)}" data-tl-kind="${L.kind}" style="height:${L.h}px"><canvas></canvas></div>`).join("");
    const els = el.rows.querySelectorAll(".tl-lane");
    st.lanes = list.map((L, i) => {
      const canvas = els[i].firstChild;
      return { ...L, el: els[i], canvas, ctx: canvas.getContext("2d") };
    });
    st.needSize = true;
    markAll();
  }
  function updateHeads() {
    for (const r of m.audio) {
      const h = el.hrows.querySelector(`.tl-head[data-tl-row="${CSS.escape(r.id)}"]`);
      if (!h) continue;
      h.classList.toggle("off", !!r.muted);
      const name = h.querySelector(".tl-hname");
      if (name.textContent !== t(r.title)) { name.textContent = t(r.title); name.title = t(r.title); }
      const mb = h.querySelector(".tl-hmute");
      const key = r.muted ? "m" : "v";
      if (mb.dataset.tlState !== key) {
        mb.dataset.tlState = key;
        mb.innerHTML = r.muted ? PICON.mute : PICON.vol;
        const label = r.muted ? t("Unmute this track") : t("Mute this track");
        mb.title = label;
        mb.setAttribute("aria-label", label);
        mb.setAttribute("aria-pressed", String(!!r.muted));
      }
      const pct = Math.round(r.volume * 100);
      const range = h.querySelector("[data-tl-vol]");
      if (st.gest?.el !== range && +range.value !== pct) range.value = pct;
      const out = h.querySelector(".tl-pct");
      const txt = t("{pct}%", { pct });
      if (out.textContent !== txt) out.textContent = txt;
    }
  }
  function syncRows() {
    if (rowSig() !== st.rowsSig) buildRows();
    updateHeads();
  }

  /* ----- selection ----- */
  const sameSel = (a, b) => (a === b) || (!!a && !!b && a.row === b.row && a.seg === b.seg);
  /** A piece given from outside (select()): the segment index is clamped into range. */
  function normSel(sel) {
    if (!sel) return null;
    if (sel.row !== "video" && !emRow(m, sel.row)) return null;
    const n = m.removed.length;
    const seg = Math.min(n - 1, Math.max(0, Math.floor(+sel.seg || 0)));
    return { row: sel.row, seg };
  }
  /** True if piece `s` still exists: its row is there and its segment index is in range. */
  const selValid = (s) => !!s && Number.isInteger(s.seg) && s.seg >= 0 && s.seg < m.removed.length && (s.row === "video" || !!emRow(m, s.row));
  /** Replace the selection. `primary` must be in `list`; if it is not, the last piece becomes the primary. */
  function setSels(list, primary) {
    const next = tlSelMerge([], (list || []).filter(selValid));
    const prim = primary && tlSelHas(next, primary) ? { row: primary.row, seg: primary.seg } : next.length ? { ...next[next.length - 1] } : null;
    const same = sameSel(prim, st.sel) && next.length === st.sels.length && next.every((s, i) => sameSel(s, st.sels[i]));
    if (same && !st.selNotify) return;
    st.selNotify = false;
    st.sels = next;
    st.sel = prim;
    markAll();
    updateBar();
    opts.onSelect?.(prim ? { ...prim } : null, next.map((s) => ({ ...s })));
  }
  function setSel(sel) {
    const one = normSel(sel);
    setSels(one ? [one] : [], one);
  }
  /** Drop pieces that no longer exist (the model was changed). */
  const validateSel = () => setSels(st.sels, st.sel);
  /** Segment indexes moved: see tlSelShift. Sets the state without notifying; the caller's changed() does. */
  function shiftSels(at, delta) {
    const map = (s) => (s && s.seg > at ? { row: s.row, seg: s.seg + delta } : s);
    st.sels = tlSelShift(st.sels, at, delta);
    st.sel = map(st.sel);
    st.selNotify = true;
  }
  /** Rows top to bottom, as selection order. */
  const rowOrder = () => ["video", ...m.audio.map((r) => r.id)];
  /** True if segment `seg` of row `rowId` is a piece that can be picked (a file row only where its clip is). */
  function pieceExists(rowId, seg) {
    if (!(seg >= 0 && seg < m.removed.length) || !segShown(seg)) return false;
    if (rowId === "video") return true;
    const row = emRow(m, rowId);
    if (!row) return false;
    if (row.kind !== "file") return true;
    const s = emSegments(m)[seg];
    return Math.min(s.end, row.offset + row.length) - Math.max(s.start, row.offset) > 1e-6;
  }
  const allPieces = () => rowOrder().flatMap((id) => m.removed.map((_, i) => ({ row: id, seg: i })).filter((s) => pieceExists(s.row, s.seg)));
  /** Selected audio pieces with their rows: [{ row, seg, r }]. */
  const selAudio = () => st.sels.filter((s) => s.row !== "video").map((s) => ({ ...s, r: emRow(m, s.row) })).filter((s) => s.r && s.seg < s.r.gains.length);
  /** The audio piece the toolbar shows: the primary if it is audio, else the first selected audio piece. */
  function refAudio() {
    const list = selAudio();
    return list.find((s) => sameSel(s, st.sel)) || list[0] || null;
  }
  /** Distinct segment indexes of the selection (or of `list`), primary's first. */
  function selSegs(list = st.sels) {
    const out = [];
    if (st.sel && list.some((s) => sameSel(s, st.sel))) out.push(st.sel.seg);
    for (const s of list) if (!out.includes(s.seg)) out.push(s.seg);
    return out;
  }

  /* ----- toolbar ----- */
  const canSplitAt = (sec) => {
    if (!(sec > 0 && sec < dur())) return false;
    const seg = emSegments(m)[emSegmentAt(m, sec)];
    return !!seg && sec - seg.start >= EM_MIN_SEG && seg.end - sec >= EM_MIN_SEG;
  };
  /** What "remove" would do to the selection: { restore, segs, ok }. Restores when the primary's segment is cut out, else cuts out; never cuts out every kept segment. */
  function removePlan() {
    const segs = selSegs();
    if (!segs.length) return { restore: false, segs, ok: false };
    const restore = !!m.removed[segs[0]];
    const kept = m.removed.filter((r) => !r).length;
    const losing = segs.filter((i) => !m.removed[i]).length;
    return { restore, segs, ok: restore || (losing > 0 && kept - losing >= 1) };
  }
  function setBtn(b, key, disabled, html, title) {
    const prev = st.bar[key];
    const sig = `${disabled}|${title}`;
    if (prev?.sig === sig && prev.html === html) return;
    if (!prev || prev.html !== html) b.innerHTML = html;
    b.disabled = disabled;
    b.title = title;
    b.setAttribute("aria-label", title);
    st.bar[key] = { sig, html };
  }
  /** Everything that depends on the model or the selection. */
  function updateBar() {
    const plan = removePlan(), removed = plan.restore;
    setBtn(el.bRemove, "remove", !plan.ok, tlSvg(removed ? tlPaths.restore : tlPaths.trash),
      removed ? t("Restore segment (Del)") : t("Delete segment (Del)"));
    const ref = refAudio(), row = ref?.r || null;
    const gain = row ? row.gains[ref.seg] ?? 1 : 1;
    setBtn(el.bMute, "mute", !row, gain === 0 ? PICON.mute : PICON.vol, row && gain === 0 ? t("Unmute segment (M)") : t("Mute segment (M)"));
    el.bMute.classList.toggle("on", !!row && gain === 0);
    el.level.disabled = !row;
    el.levelBox.classList.toggle("off", !row);
    const pct = Math.round(gain * 100);
    if (st.gest?.el !== el.level && +el.level.value !== pct) el.level.value = pct;
    el.levelPct.textContent = row ? t("{pct}%", { pct }) : "";
    el.bOut.disabled = st.pps <= minPps() * 1.0001;
    el.bIn.disabled = st.pps >= maxPps() * 0.9999;
    el.bHide.classList.toggle("on", st.hideCuts);
    el.bHide.setAttribute("aria-pressed", String(st.hideCuts));
    updateBarCheap(st.shownT, true);
  }
  /** The parts that can change every frame (playhead, undo stacks): only touches the DOM on change. */
  function updateBarCheap(sec, force) {
    const split = canSplitAt(sec), u = m.undo.length > 0, r = m.redo.length > 0;
    if (!force && st.bar.cheap && st.bar.cheap[0] === split && st.bar.cheap[1] === u && st.bar.cheap[2] === r) return;
    st.bar.cheap = [split, u, r];
    el.bSplit.disabled = !split;
    el.bUndo.disabled = !u;
    el.bRedo.disabled = !r;
  }

  /* ----- changes ----- */
  /** The model was changed by us: redraw, then tell the editor. */
  function changed(what, rows = true) {
    markAll();
    if (rows) syncRows();
    validateSel();
    if (st.hideCuts) relayout(); // the view length follows what is cut out
    updateBar();
    opts.onChange?.(what);
  }
  // Each do... below takes one undo snapshot, changes the model, then calls changed(). They return true if they did something.
  function doSplit(at = st.shownT) {
    if (!canSplitAt(at)) return false;
    emSnapshot(m);
    const c = emSplit(m, at);
    if (c < 0) throw new Error(`timeline: emSplit refused ${at} although canSplitAt allowed it`);
    shiftSels(c, 1);
    changed("split");
    return true;
  }
  function doJoin(c) {
    if (c < 0 || c >= m.cuts.length) return false;
    emSnapshot(m);
    emJoin(m, c);
    shiftSels(c, -1);
    changed("join");
    return true;
  }
  /** Cut out the selected segments, or restore them (see removePlan). */
  function doRemove() {
    const plan = removePlan();
    if (!plan.ok) return false;
    emSnapshot(m);
    for (const i of plan.segs) {
      if (!!m.removed[i] === !plan.restore) continue;
      if (!emSetRemoved(m, i, !plan.restore)) throw new Error(`timeline: emSetRemoved(${i}, ${!plan.restore}) refused`);
    }
    changed(plan.restore ? "restore" : "remove");
    return true;
  }
  /** Put back the removed run `j` (a tlJoints entry): "Restore the part cut out here". */
  function doRestoreRun(j) {
    if (!j) return false;
    emSnapshot(m);
    for (let i = j.first; i <= j.last; i++) emSetRemoved(m, i, false);
    changed("restore");
    return true;
  }
  /** Set the level of every selected audio piece: `gain(oldGain)` -> new gain. */
  function setGains(what, gain) {
    const list = selAudio().map((s) => ({ ...s, g: gain(s.r.gains[s.seg]) }));
    if (!list.some((s) => Math.abs(s.g - s.r.gains[s.seg]) > 1e-9)) return false; // nothing would change: no empty undo step
    emSnapshot(m);
    for (const s of list) emSetGain(m, s.r.id, s.seg, s.g);
    changed(what);
    return true;
  }
  /** Mute the selected audio pieces, or unmute them all if the shown one (refAudio) is muted. */
  function doMuteSeg() {
    const ref = refAudio();
    if (!ref) return false;
    const on = ref.r.gains[ref.seg] > 0;
    return setGains("mute", () => (on ? 0 : 1));
  }
  const doLevel = (g) => setGains("gain", () => Math.min(EM_MAX_GAIN, Math.max(0, +g || 0)));
  const doNudge = (step) => setGains("gain", (g) => Math.min(EM_MAX_GAIN, Math.max(0, Math.round((g + step) * 100) / 100)));
  /**
   * Solo, per segment: only the selected rows are heard in it. A segment where one row is
   * selected uses emSolo; several rows in one segment are handled the same way by hand.
   * If every selected segment is soloed already, all of them are unsoloed.
   */
  function soloGroups() {
    const groups = new Map();
    for (const s of selAudio()) { if (!groups.has(s.seg)) groups.set(s.seg, []); groups.get(s.seg).push(s.r.id); }
    // A segment where every row is selected cannot be soloed.
    return [...groups].filter(([, ids]) => ids.length < m.audio.length);
  }
  const groupSolo = (seg, ids) => (ids.length === 1 ? emIsSolo(m, ids[0], seg) : m.audio.every((r) => (ids.includes(r.id) ? r.gains[seg] > 0 : r.gains[seg] === 0)));
  const allSolo = () => { const g = soloGroups(); return g.length > 0 && g.every(([seg, ids]) => groupSolo(seg, ids)); };
  function doSolo() {
    const groups = soloGroups();
    if (!groups.length || m.audio.length < 2) return false;
    const undo = groups.every(([seg, ids]) => groupSolo(seg, ids));
    emSnapshot(m);
    for (const [seg, ids] of groups) {
      if (groupSolo(seg, ids) !== undo) continue;
      if (ids.length === 1) { emSolo(m, ids[0], seg); continue; }
      for (const r of m.audio) {
        if (!ids.includes(r.id)) emSetGain(m, r.id, seg, undo ? 1 : 0);
        else if (!undo && r.gains[seg] === 0) emSetGain(m, r.id, seg, 1);
      }
    }
    changed("gain");
    return true;
  }
  /** Segments "Hide the video" acts on: those of the selected video pieces, or of every selected piece if no video piece is selected. */
  function blankSegs() {
    const vid = st.sels.filter((s) => s.row === "video");
    return selSegs(vid.length ? vid : st.sels).filter((i) => i < m.blank.length);
  }
  /** Hide the picture of the selected segments, or show it again if the first one is hidden. */
  function doBlank(force) {
    const segs = blankSegs();
    if (!segs.length) return false;
    const on = force === undefined ? !m.blank[segs[0]] : !!force;
    emSnapshot(m);
    for (const i of segs) emSetBlank(m, i, on);
    changed("blank");
    return true;
  }
  function doUndo(redo) {
    if (!(redo ? emRedo(m) : emUndo(m))) return false;
    changed(redo ? "redo" : "undo");
    return true;
  }
  function selectAll() {
    const all = allPieces();
    if (!all.length) return false;
    setSels(all, st.sel && tlSelHas(all, st.sel) ? st.sel : all[0]);
    return true;
  }
  function setHideCuts(on) {
    on = !!on;
    if (on === st.hideCuts || st.drag) return false;
    const x = st.laneW ? X(st.shownT) : 0; // keep the playhead where it is on screen
    st.hideCuts = on;
    try { localStorage.setItem("glint.tl.hideCuts", on ? "1" : "0"); } catch {}
    markAll();
    relayout();
    if (st.laneW && !st.fitted) setScroll(tlPad + toV(st.shownT) * st.pps - x);
    closeMenu();
    updateBar();
    return true;
  }

  /* ----- colours ----- */
  function colors() {
    const cs = getComputedStyle(document.documentElement);
    const C = {};
    for (const k of ["bg", "panel", "raised", "raised-2", "line", "line-2", "text", "muted", "dim", "accent", "accent-2", "accent-ink", "danger", "warn", "ok", "sans", "mono"]) C[k] = cs.getPropertyValue(`--${k}`).trim();
    C.sans ||= "sans-serif";
    C.mono ||= "monospace";
    // File rows get whichever of ok / warn is furthest in hue from the accent, so they never look like track rows.
    const ha = tlHue(C.accent);
    C.file = tlHueDist(tlHue(C.ok), ha) >= tlHueDist(tlHue(C.warn), ha) ? C.ok : C.warn;
    return C;
  }
  function hatchFor(ctx, C) {
    const key = `${C.muted}|${st.dpr}`;
    if (st.hatch && st.hatchKey === key) return st.hatch;
    const n = Math.max(1, Math.round(9 * st.dpr));
    const c = document.createElement("canvas");
    c.width = c.height = n;
    const g = c.getContext("2d");
    g.strokeStyle = C.muted;
    g.globalAlpha = 0.45;
    g.lineWidth = 1.4 * st.dpr;
    g.beginPath();
    for (const o of [-n, 0, n]) { g.moveTo(o, n); g.lineTo(o + n, 0); }
    g.stroke();
    const p = ctx.createPattern(c, "repeat");
    p.setTransform(new DOMMatrix().scale(1 / st.dpr));
    st.hatch = p;
    st.hatchKey = key;
    return p;
  }
  function icon(ctx, name, x, y, size, color, fill) {
    tlPath2d ||= Object.fromEntries(Object.entries(tlPaths).map(([k, d]) => [k, new Path2D(d)]));
    ctx.save();
    ctx.translate(x, y);
    ctx.scale(size / 24, size / 24);
    ctx.strokeStyle = color;
    ctx.lineWidth = 2;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    if (fill) { ctx.fillStyle = color; ctx.fill(tlPath2d[name]); }
    ctx.stroke(tlPath2d[name]);
    ctx.restore();
  }

  /* ----- drawing ----- */
  function applySize() {
    st.needSize = false;
    const w = st.laneW, d = st.dpr;
    const size = (c, h) => {
      const W = Math.max(1, Math.round(w * d)), H = Math.max(1, Math.round(h * d));
      if (c.width !== W) c.width = W;
      if (c.height !== H) c.height = H;
    };
    size(el.ruler, tlRulerH);
    for (const L of st.lanes) size(L.canvas, L.h);
    markAll();
  }
  function prep(ctx, h) {
    ctx.setTransform(st.dpr, 0, 0, st.dpr, 0, 0);
    ctx.clearRect(0, 0, st.laneW, h);
  }

  /** The ruler counts view time: source time, or with cut parts hidden, time in the finished clip. */
  function drawRuler(C) {
    const ctx = rulerCtx, w = st.laneW, h = tlRulerH;
    prep(ctx, h);
    ctx.fillStyle = C.panel;
    ctx.fillRect(0, 0, w, h);
    const end = XV(vdur());
    if (end < w) { ctx.fillStyle = C.bg; ctx.globalAlpha = 0.6; ctx.fillRect(Math.max(0, end), 0, w - Math.max(0, end), h); ctx.globalAlpha = 1; }
    const x0 = XV(0);
    if (x0 > 0) { ctx.fillStyle = C.bg; ctx.globalAlpha = 0.6; ctx.fillRect(0, 0, x0, h); ctx.globalAlpha = 1; }
    const { major, minor } = tlPickStep(st.pps);
    const t0 = Math.max(0, vAtX(-80)), t1 = Math.min(vdur(), vAtX(w + 4));
    ctx.fillStyle = C.dim;
    if (minor < major) {
      for (let k = Math.ceil(t0 / minor - 1e-9); k * minor <= t1 + 1e-9; k++) {
        const sec = k * minor;
        if (Math.abs(Math.round(sec / major) * major - sec) < 1e-6) continue;
        ctx.fillRect(Math.round(XV(sec)) - 0.5, h - 6, 1, 6);
      }
    }
    ctx.font = `500 11px ${C.sans}`;
    ctx.textBaseline = "middle";
    for (let k = Math.ceil(t0 / major - 1e-9); k * major <= t1 + 1e-9; k++) {
      const sec = Math.round(k * major * 1000) / 1000;
      const x = Math.round(XV(sec)) - 0.5;
      ctx.fillStyle = C.muted;
      ctx.fillRect(x, h - 11, 1, 11);
      ctx.fillText(tlFmtTick(sec, major), x + 4, 10);
    }
    // Joints (cut parts hidden): a small notch at the bottom of the ruler.
    ctx.fillStyle = C.warn || C.accent;
    for (const j of st.hideCuts ? joints() : []) {
      const x = X(j.start);
      if (x < -6 || x > w + 6) continue;
      ctx.beginPath();
      ctx.moveTo(x - 4, h - 1);
      ctx.lineTo(x + 4, h - 1);
      ctx.lineTo(x, h - 7);
      ctx.fill();
    }
    ctx.fillStyle = C.line;
    ctx.fillRect(0, h - 1, w, 1);
  }
  /** The removed runs (tlJoints) of the model as it is now. */
  const joints = () => tlJoints(m.removed, [0, ...m.cuts, dur()]);

  /** Screen range of [a, b] clipped near the lane, or null when off screen. */
  function span(a, b, margin = 12) {
    const x0 = X(a), x1 = X(b);
    if (x1 < -margin || x0 > st.laneW + margin) return null;
    return [Math.max(x0, -margin), Math.min(x1, st.laneW + margin)];
  }
  function rr(ctx, x, y, w, h, r) {
    ctx.beginPath();
    ctx.roundRect(x, y, Math.max(0, w), h, Math.min(r, w / 2, h / 2));
  }
  function overlays(ctx, C, a, b, y, bh, rowId) {
    // Removed segments: dimmed and hatched.
    const segs = emSegments(m);
    for (const s of segs) {
      if (!s.removed) continue;
      const sp = span(Math.max(a, s.start), Math.min(b, s.end));
      if (!sp || sp[1] <= sp[0]) continue;
      ctx.globalAlpha = 0.62;
      ctx.fillStyle = C.bg;
      ctx.fillRect(sp[0], y, sp[1] - sp[0], bh);
      ctx.globalAlpha = 1;
      ctx.fillStyle = hatchFor(ctx, C);
      ctx.fillRect(sp[0], y, sp[1] - sp[0], bh);
    }
    // Segment borders inside the bar. With cut parts hidden, a cut next to a removed part is a joint instead.
    ctx.fillStyle = C.bg;
    m.cuts.forEach((c, i) => {
      if (c <= a || c >= b) return;
      if (st.hideCuts && (m.removed[i] || m.removed[i + 1])) return;
      const x = X(c);
      if (x > -2 && x < st.laneW + 2) ctx.fillRect(Math.round(x) - 1, y, 2, bh);
    });
    // Joints: where something was cut out, a thin line between the neighbours (the DOM .tl-joint on top carries the tooltip).
    if (st.hideCuts) {
      ctx.fillStyle = C.warn || C.accent;
      for (const j of joints()) { // the caller has clipped to the bar
        const x = X(j.start);
        if (x > -2 && x < st.laneW + 2) ctx.fillRect(Math.round(x) - 1, y, 2, bh);
      }
    }
    return segs;
  }
  /** Highlight every selected piece of row `rowId` (bar [a, b] in source time). */
  function selOutline(ctx, C, a, b, y, bh, rowId) {
    if (!st.sels.length) return;
    const segs = emSegments(m);
    for (const sel of st.sels) {
      if (sel.row !== rowId) continue;
      const s = segs[sel.seg];
      if (!s) continue;
      const sp = span(Math.max(a, s.start), Math.min(b, s.end), 4);
      if (!sp || sp[1] - sp[0] < 1) continue;
      ctx.globalAlpha = 0.12;
      ctx.fillStyle = C["accent-2"];
      rr(ctx, sp[0], y, sp[1] - sp[0], bh, 7);
      ctx.fill();
      ctx.globalAlpha = 1;
      ctx.lineWidth = 2;
      ctx.strokeStyle = C["accent-2"];
      rr(ctx, sp[0] + 1, y + 1, sp[1] - sp[0] - 2, bh - 2, 7);
      ctx.stroke();
    }
  }
  /** Video pieces with their picture hidden: dark, with a crossed-out eye and "Video hidden" when it fits. Graded pieces: a dot top right. */
  function pictureMarks(ctx, C, y, bh) {
    const segs = emSegments(m);
    ctx.font = `600 11px ${C.sans}`;
    ctx.textBaseline = "middle";
    for (const s of segs) {
      if (!segShown(s.i)) continue;
      const sp = span(s.start, s.end, 0);
      if (!sp || sp[1] - sp[0] < 2) continue;
      const w = sp[1] - sp[0];
      if (m.blank?.[s.i]) {
        ctx.globalAlpha = 0.9;
        ctx.fillStyle = "#000";
        ctx.fillRect(sp[0], y, w, bh);
        ctx.globalAlpha = 1;
        const ix = Math.max(sp[0], 0) + 8, iy = y + bh - 21;
        if (Math.min(sp[1], st.laneW) - ix >= 18) {
          icon(ctx, "eyeOff", ix, iy, 16, C.muted);
          const room = Math.min(sp[1], st.laneW) - ix - 30;
          const txt = room > 30 ? tlEllipsize(ctx, t("Video hidden"), room) : "";
          if (txt) { ctx.fillStyle = C.muted; ctx.fillText(txt, ix + 22, iy + 8.5); }
        }
      }
      const col = m.colors?.[s.i];
      if (col && !emSameColor(col) && w >= 16) {
        const cx = Math.min(sp[1], st.laneW) - 9, cy = y + 9;
        if (cx - 5 < sp[0]) continue;
        ctx.beginPath();
        ctx.arc(cx, cy, 5.5, 0, Math.PI * 2);
        ctx.fillStyle = C.bg;
        ctx.fill();
        ctx.beginPath();
        ctx.arc(cx, cy, 3.5, 0, Math.PI * 2);
        ctx.fillStyle = C["accent-2"] || C.accent;
        ctx.fill();
      }
    }
  }
  /** Icon + name in a pill at the visible left end of a bar. */
  function label(ctx, C, iconName, text, bx0, bx1, y) {
    const lx = Math.max(bx0, 0) + 7, room = Math.min(bx1, st.laneW) - lx - 8;
    if (room < 34) return;
    ctx.font = `600 11.5px ${C.sans}`;
    const txt = tlEllipsize(ctx, text || "", room - 30);
    const w = 26 + (txt ? ctx.measureText(txt).width + 6 : 0);
    ctx.globalAlpha = 0.72;
    ctx.fillStyle = C.bg;
    rr(ctx, lx, y, w, 20, 6);
    ctx.fill();
    ctx.globalAlpha = 1;
    icon(ctx, iconName, lx + 5, y + 3, 14, C.text);
    if (txt) {
      ctx.fillStyle = C.text;
      ctx.textBaseline = "middle";
      ctx.fillText(txt, lx + 24, y + 10.5);
    }
  }

  function drawVideo(L, C) {
    const ctx = L.ctx, h = L.h;
    prep(ctx, h);
    const sp = span(0, dur());
    if (!sp) return;
    const [bx0, bx1] = sp, y = 4, bh = h - 8;
    ctx.save();
    rr(ctx, bx0, y, bx1 - bx0, bh, 8);
    ctx.clip();
    ctx.fillStyle = C.raised;
    ctx.fillRect(bx0, y, bx1 - bx0, bh);
    ctx.globalAlpha = 0.28;
    ctx.fillStyle = C.accent;
    ctx.fillRect(bx0, y, bx1 - bx0, bh);
    ctx.globalAlpha = 1;
    // Film strip: tiles fixed to the content (they do not slide while scrolling), each showing the frame nearest its middle.
    const ok = st.thumbs.filter((x) => x.ok).length;
    if (ok && dur() > 0) {
      const n = st.thumbs.length, tw = Math.max(24, (bh * 16) / 9);
      const from = Math.max(0, bx0), to = Math.min(st.laneW, bx1);
      for (let k = Math.floor((from + st.scrollX - tlPad) / tw); ; k++) {
        const sx = tlPad + k * tw - st.scrollX;
        if (sx >= to) break;
        const mid = toS(((k + 0.5) * tw) / st.pps);
        const img = nearestThumb(Math.min(n - 1, Math.max(0, Math.floor((mid / dur()) * n))));
        if (!img) break;
        const iw = img.naturalWidth, ih = img.naturalHeight;
        let cx = 0, cy = 0, cw = iw, ch = ih;
        if (iw / ih > tw / bh) { cw = ih * (tw / bh); cx = (iw - cw) / 2; } else { ch = iw / (tw / bh); cy = (ih - ch) / 2; }
        ctx.drawImage(img, cx, cy, cw, ch, sx, y, tw, bh);
        ctx.fillStyle = C.bg;
        ctx.globalAlpha = 0.55;
        ctx.fillRect(Math.round(sx + tw) - 0.5, y, 1, bh);
        ctx.globalAlpha = 1;
      }
    }
    pictureMarks(ctx, C, y, bh);
    overlays(ctx, C, 0, dur(), y, bh, "video");
    ctx.restore();
    ctx.lineWidth = 1;
    ctx.strokeStyle = C["line-2"];
    rr(ctx, bx0 + 0.5, y + 0.5, bx1 - bx0 - 1, bh - 1, 8);
    ctx.stroke();
    selOutline(ctx, C, 0, dur(), y, bh, "video");
    label(ctx, C, "film", opts.title || t("Video"), bx0, bx1, y + 5);
  }
  function nearestThumb(i) {
    const list = st.thumbs;
    for (let d = 0; d < list.length; d++) {
      if (list[i - d]?.ok) return list[i - d].img;
      if (list[i + d]?.ok) return list[i + d].img;
    }
    return null;
  }

  function drawAudio(L, C) {
    const ctx = L.ctx, h = L.h;
    prep(ctx, h);
    const row = emRow(m, L.id);
    if (!row) return;
    const file = row.kind === "file";
    const a = file ? row.offset : 0, b = file ? row.offset + row.length : dur();
    const sp = span(a, b);
    if (!sp) return;
    const [bx0, bx1] = sp, y = 4, bh = h - 8, mid = y + bh / 2, half = bh / 2 - 4;
    const tint = row.muted ? C.dim : file ? C.file : C.accent;
    ctx.save();
    rr(ctx, bx0, y, bx1 - bx0, bh, 7);
    ctx.clip();
    ctx.fillStyle = C.raised;
    ctx.fillRect(bx0, y, bx1 - bx0, bh);
    ctx.globalAlpha = row.muted ? 0.18 : 0.3;
    ctx.fillStyle = tint;
    ctx.fillRect(bx0, y, bx1 - bx0, bh);
    ctx.globalAlpha = 1;
    const segs = emSegments(m);
    // Silent segments: greyed.
    for (const s of segs) {
      if (row.gains[s.i] !== 0) continue;
      const ss = span(Math.max(a, s.start), Math.min(b, s.end));
      if (!ss || ss[1] <= ss[0]) continue;
      ctx.globalAlpha = 0.75;
      ctx.fillStyle = C["raised-2"];
      ctx.fillRect(ss[0], y, ss[1] - ss[0], bh);
      ctx.globalAlpha = 1;
    }
    // Waveform: bars on a 3 px grid fixed to the content, height = peak x segment gain x row volume.
    const wave = st.waves.get(row.id);
    const from = Math.max(0, bx0), to = Math.min(st.laneW, bx1);
    if (wave) {
      const loud = new Path2D(), quiet = new Path2D();
      let si = 0;
      for (let k = Math.floor((from + st.scrollX) / tlBarStep); ; k++) {
        const x = k * tlBarStep - st.scrollX;
        if (x >= to) break;
        const s0 = timeAtX(x);
        // A bar across a joint (cut parts hidden) must not take the peaks of the part cut out in between.
        const s1 = Math.min(timeAtX(x + tlBarStep), s0 + tlBarStep / st.pps), sc = (s0 + s1) / 2;
        if (sc < a || sc >= b) continue;
        while (si < segs.length - 1 && sc >= segs[si].end) si++;
        const g = row.muted ? 0 : (row.gains[si] ?? 1) * row.volume;
        const base = file ? row.trimStart - row.offset : 0;
        const p = tlPeakMax(wave.peaks, (s0 + base) * wave.ps, (s1 + base) * wave.ps);
        const hh = Math.max(1, Math.min(1, p * g) * half);
        (g > 0 ? loud : quiet).roundRect(x + 0.5, mid - hh, tlBarW, hh * 2, 1);
      }
      ctx.fillStyle = tint;
      ctx.globalAlpha = 0.95;
      ctx.fill(loud);
      ctx.fillStyle = C.dim;
      ctx.fill(quiet);
      ctx.globalAlpha = 1;
    } else {
      ctx.globalAlpha = 0.5;
      ctx.fillStyle = tint;
      ctx.fillRect(from, Math.round(mid), to - from, 1);
      ctx.globalAlpha = 1;
    }
    overlays(ctx, C, a, b, y, bh, row.id);
    ctx.restore();
    ctx.lineWidth = 1;
    ctx.strokeStyle = C["line-2"];
    rr(ctx, bx0 + 0.5, y + 0.5, bx1 - bx0 - 1, bh - 1, 7);
    ctx.stroke();
    // File clips: grips on both edges (they trim).
    if (file) {
      ctx.fillStyle = C.text;
      ctx.globalAlpha = 0.7;
      for (const ex of [X(a) + 4, X(b) - 6]) if (ex > -4 && ex < st.laneW + 4) rr(ctx, ex, mid - 8, 2, 16, 1), ctx.fill();
      ctx.globalAlpha = 1;
    }
    selOutline(ctx, C, a, b, y, bh, row.id);
    // Level badges and muted marks at the top right of each segment.
    ctx.font = `600 10.5px ${C.mono}`;
    ctx.textBaseline = "middle";
    for (const s of segs) {
      const g = row.gains[s.i];
      if (Math.abs(g - 1) < 0.005 || s.removed) continue;
      const ss = span(Math.max(a, s.start), Math.min(b, s.end), 0);
      if (!ss) continue;
      const right = Math.min(ss[1], st.laneW) - 6;
      if (g === 0) {
        if (right - 18 < ss[0] + 4) continue;
        ctx.globalAlpha = 0.8;
        ctx.fillStyle = C.bg;
        rr(ctx, right - 20, y + 3, 20, 16, 5);
        ctx.fill();
        ctx.globalAlpha = 1;
        icon(ctx, "muted", right - 17, y + 4, 14, C.muted);
        continue;
      }
      const txt = t("{pct}%", { pct: Math.round(g * 100) });
      const w = ctx.measureText(txt).width + 10;
      if (right - w < ss[0] + 4) continue;
      ctx.globalAlpha = 0.85;
      ctx.fillStyle = C.bg;
      rr(ctx, right - w, y + 3, w, 16, 5);
      ctx.fill();
      ctx.globalAlpha = 1;
      ctx.fillStyle = g > 1 ? C.warn : C.text;
      ctx.fillText(txt, right - w + 5, y + 11.5);
    }
    label(ctx, C, "note", t(row.title), bx0, bx1, y + bh - 24);
  }

  function placeCuts() {
    const n = m.cuts.length;
    if (el.cuts.children.length !== n) {
      const title = esc(t("Drag to move this cut, double-click to join the two segments"));
      el.cuts.innerHTML = m.cuts.map((_, i) => `<div class="tl-cut" data-tl-cut="${i}" title="${title}" style="top:${tlRulerH}px"><b></b></div>`).join("");
    }
    for (let i = 0; i < n; i++) {
      const c = el.cuts.children[i], x = X(m.cuts[i]);
      // With cut parts hidden, a cut next to a removed part sits on a joint: the joint is shown instead (and cannot be dragged).
      const vis = x > -8 && x < st.laneW + 8 && !(st.hideCuts && (m.removed[i] || m.removed[i + 1]));
      c.style.transform = `translateX(${vis ? x : -100}px)`;
      c.classList.toggle("drag", st.drag?.type === "cut" && st.drag.moved && st.drag.c === i);
    }
    placeJoints();
  }
  /** One .tl-joint per removed run while cut parts are hidden: a hover target with "3.2 s cut out here", and right-click to restore. */
  function placeJoints() {
    const list = st.hideCuts ? joints() : [];
    st.joints = list;
    const sig = list.map((j) => `${j.first}-${j.last}-${tlFmtLen(j.end - j.start)}`).join("|");
    if (sig !== st.jointSig) {
      st.jointSig = sig;
      el.joints.innerHTML = list.map((j, i) => {
        const title = esc(t("{sec} s cut out here", { sec: tlFmtLen(j.end - j.start) }));
        return `<div class="tl-joint" data-tl-joint="${i}" title="${title}" style="top:${tlRulerH}px"><b></b></div>`;
      }).join("");
    }
    list.forEach((j, i) => {
      const x = X(j.start), vis = x > -8 && x < st.laneW + 8;
      el.joints.children[i].style.transform = `translateX(${vis ? x : -100}px)`;
    });
  }

  function draw() {
    if (!st.laneW) return;
    const C = colors();
    if (st.dirtyAll || st.dirtyRuler) drawRuler(C);
    for (const L of st.lanes) {
      if (!st.dirtyAll && !st.dirty.has(L.id)) continue;
      if (L.kind === "video") drawVideo(L, C); else drawAudio(L, C);
    }
    if (st.dirtyAll) placeCuts();
    st.dirtyAll = false;
    st.dirtyRuler = false;
    st.dirty.clear();
  }

  function placePlayhead(sec) {
    const x = Math.round(X(sec) * 2) / 2;
    const vis = x >= -6 && x <= st.laneW + 6;
    if (vis !== st.phVis) { st.phVis = vis; el.ph.style.visibility = el.phK.style.visibility = vis ? "" : "hidden"; }
    if (vis && x !== st.phX) {
      st.phX = x;
      el.ph.style.transform = el.phK.style.transform = `translateX(${x}px)`;
    }
  }

  /** Keep the playhead in view while it moves: page when it runs off the right edge or jumps out of view. */
  function follow(prev, now) {
    if (!st.laneW) return;
    const x = X(now);
    if (x >= 0 && x <= st.laneW - 4) return;
    const xp = X(prev), wasIn = xp >= 0 && xp <= st.laneW - 4;
    const jump = Math.abs(now - prev) > 0.5;
    const vx = tlPad + toV(now) * st.pps;
    if (wasIn && !jump && x > st.laneW - 4) setScroll(vx - 24); // ran off the right edge: next page
    else if (wasIn || jump) setScroll(vx - st.laneW / 3);
  }

  function frame() {
    st.raf = requestAnimationFrame(frame);
    if (window.devicePixelRatio !== st.dpr) { st.dpr = window.devicePixelRatio || 1; st.needSize = true; }
    if (st.needSize && st.laneW) applySize();
    // The editor changed what is cut out without calling refresh(): rebuild the view mapping.
    if (st.hideCuts && st.map && st.map.sig !== mapSig()) { markAll(); relayout(); }
    const real = +opts.getTime() || 0;
    let now = real;
    if (st.drag?.type === "seek" && st.drag.at !== undefined) now = st.drag.at;
    else if (st.hold) {
      if (Math.abs(real - st.hold.at) < 0.06 || performance.now() > st.hold.until) st.hold = null;
      else now = st.hold.at;
    }
    if (!st.drag && Number.isFinite(st.lastT) && now !== st.lastT) follow(st.lastT, now);
    st.lastT = now;
    st.shownT = now;
    if (st.drag?.moved && st.drag.type !== "click") edgeScroll();
    if (st.dirtyAll || st.dirtyRuler || st.dirty.size) draw();
    placePlayhead(st.shownT);
    updateBarCheap(st.shownT, false);
  }

  /* ----- mouse ----- */
  function snapCands(skip) {
    const c = [0, dur(), ...m.cuts.filter((_, i) => !(skip.cut === i))];
    if (!skip.playhead) c.push(st.shownT);
    for (const r of m.audio) if (r.kind === "file" && r.id !== skip.row) c.push(r.offset, r.offset + r.length);
    return c;
  }
  const snapTol = () => tlSnapPx / st.pps;
  /** Snap source time `sec` to a candidate within tlSnapPx on screen (distances in view time, so it also works with cut parts hidden). */
  const snapAt = (sec, cands) => tlSnapV(sec, cands, snapTol(), toV);
  function showTip(sec, text) {
    el.tip.hidden = false;
    el.tip.textContent = text || tlFmtPrecise(toV(sec)); // in ruler time (finished-clip time while cut parts are hidden)
    el.tip.style.transform = `translateX(${Math.min(st.laneW - 30, Math.max(30, X(sec)))}px) translateX(-50%)`;
  }
  /** Which part of file row `row`'s clip is under lane x `x` (see tlSpanHit), measured on screen. */
  const fileHit = (row, x) => tlSpanHit(toV(row.offset), toV(row.offset + row.length), vAtX(x), st.pps);
  /** The lane under client y, or null. */
  const laneAt = (clientY) => st.lanes.find((L) => { const r = L.el.getBoundingClientRect(); return clientY >= r.top && clientY < r.bottom; }) || null;
  /** The piece under lane x `x` on lane `id`: { row, seg }, or null (outside the clip, on a file row off its clip). */
  function pieceAtX(id, x) {
    const v = vAtX(x);
    if (!(v >= 0 && v <= vdur())) return null;
    const seg = emSegmentAt(m, toS(Math.min(v, vdur() - 1e-6)));
    if (!segShown(seg)) return null;
    if (id !== "video") {
      const row = emRow(m, id);
      if (!row || (row.kind === "file" && !fileHit(row, x))) return null;
    }
    return { row: id, seg };
  }
  /** Pieces of lane `id` in content x (lane x + scrollX), for the rubber band: [{ seg, x0, x1 }]. */
  function lanePieces(id) {
    const row = id === "video" ? null : emRow(m, id);
    if (id !== "video" && !row) return [];
    const file = row?.kind === "file";
    const a = file ? row.offset : 0, b = file ? row.offset + row.length : dur();
    const cx = (sec) => tlPad + toV(sec) * st.pps;
    return emSegments(m).filter((s) => segShown(s.i)).map((s) => ({ seg: s.i, x0: cx(Math.max(a, s.start)), x1: cx(Math.min(b, s.end)) })).filter((p) => p.x1 > p.x0);
  }

  function onDown(e) {
    if (e.button !== 0 || st.drag) return;
    closeMenu();
    const rect = el.lanes.getBoundingClientRect();
    const x = e.clientX - rect.left, at = timeAtX(x);
    const d = { x0: e.clientX, y0: e.clientY, sx0: st.scrollX, rect, moved: false, snapped: false, lastE: e, ctrl: e.ctrlKey || e.metaKey, shift: e.shiftKey };
    const cut = e.target.closest("[data-tl-cut]");
    // A joint marker is only a hover / right-click target: a press on it acts on the lane under it.
    const lane = e.target.closest("[data-tl-lane]") || (e.target.closest("[data-tl-joint]") ? laneAt(e.clientY)?.el : null);
    if (cut) Object.assign(d, { type: "cut", c: +cut.dataset.tlCut });
    else if (e.target.closest("[data-tl-ph], [data-tl-ruler]")) Object.assign(d, { type: "seek", moved: true });
    else if (lane) {
      const id = lane.dataset.tlLane, row = id === "video" ? null : emRow(m, id);
      const hit = row?.kind === "file" ? fileHit(row, x) : null;
      const sel = pieceAtX(id, x);
      // On a file row, a press on the clip moves / trims it; anywhere else it is a click, or a rubber band once it moves.
      if (hit) Object.assign(d, { type: hit, rowId: id, row0: { offset: row.offset, trimStart: row.trimStart, length: row.length }, grab: at, sel });
      else Object.assign(d, { type: "click", sel });
    } else return;
    e.preventDefault();
    st.drag = d;
    host.classList.add("tl-dragging");
    host.dataset.tlDrag = d.type;
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    window.addEventListener("blur", onLost);
    if (d.type === "seek") apply(d, e);
  }
  // The mouse-up never arrives when the window loses the mouse mid-drag (Alt+Tab, a
  // screenshot tool): end the drag instead of leaving it, and its rubber band, hanging.
  function onLost() { if (st.drag) endDrag(); }
  function onMove(e) {
    const d = st.drag;
    if (!d) return;
    if (e.buttons === 0) return onUp(e); // released while we could not see it
    d.lastE = e;
    if (!d.moved) {
      // A click may turn into a rubber band in any direction; the other drags are sideways.
      const dist = d.type === "click" ? Math.max(Math.abs(e.clientX - d.x0), Math.abs(e.clientY - d.y0)) : Math.abs(e.clientX - d.x0);
      if (dist < tlDragPx) return;
      d.moved = true;
      if (d.type === "click") startBand(d);
    }
    apply(d, e);
  }
  /** One step of a drag at mouse event `e`. */
  function apply(d, e) {
    if (d.type === "band") return moveBand(d, e);
    const x = e.clientX - d.rect.left, raw = timeAtX(x), free = e.altKey; // Alt: no snapping
    const snap = (sec, skip) => (free ? sec : snapAt(sec, snapCands(skip)).at);
    if (d.type === "seek") {
      const at = Math.min(dur(), Math.max(0, snap(raw, { playhead: true })));
      d.at = at;
      st.shownT = at;
      showTip(at);
      opts.onSeek?.(at);
      return;
    }
    if (d.type === "cut") {
      if (!d.snapped) { emSnapshot(m); d.snapped = true; }
      const used = emMoveCut(m, d.c, snap(raw, { cut: d.c }));
      showTip(used);
      changed("cut", false);
      return;
    }
    const row = emRow(m, d.rowId);
    if (!row) return;
    if (!d.snapped) { emSnapshot(m); d.snapped = true; }
    const r0 = d.row0;
    if (d.type === "move") {
      const len = row.length, hi = Math.max(0, dur() - len);
      let o = Math.min(hi, Math.max(0, r0.offset + raw - d.grab));
      if (!free) {
        const c = snapCands({ row: d.rowId });
        const s1 = snapAt(o, c), s2 = snapAt(o + len, c);
        if (s1.hit || s2.hit) o = s1.dist <= s2.dist ? s1.at : s2.at - len;
        o = Math.min(hi, Math.max(0, o));
      }
      emPlaceFile(m, d.rowId, { offset: o });
      showTip(row.offset);
    } else if (d.type === "left") {
      emPlaceFile(m, d.rowId, tlTrimLeft(r0, snap(raw, { row: d.rowId }), EM_MIN_SEG));
      showTip(row.offset);
    } else if (d.type === "right") {
      emPlaceFile(m, d.rowId, { length: snap(raw, { row: d.rowId }) - r0.offset });
      showTip(row.offset + row.length);
    }
    changed("file", false);
  }

  /* Rubber band: coordinates are content x (lane x + scrollX, so auto-scroll extends it) and y inside .tl-lanes. */
  function startBand(d) {
    d.type = "band";
    host.dataset.tlDrag = "band";
    d.bx0 = d.x0 - d.rect.left + d.sx0;
    d.by0 = d.y0 - d.rect.top;
    d.base = d.ctrl ? st.sels.map((s) => ({ ...s })) : []; // Ctrl: add to the selection
  }
  function moveBand(d, e) {
    const lr = el.lanes.getBoundingClientRect();
    const bx1 = e.clientX - lr.left + st.scrollX, by1 = e.clientY - lr.top;
    const rowsTop = el.rows.offsetTop, rowsBot = rowsTop + el.rows.offsetHeight;
    const left = Math.max(0, Math.min(d.bx0, bx1) - st.scrollX), right = Math.min(st.laneW, Math.max(d.bx0, bx1) - st.scrollX);
    const top = Math.max(rowsTop, Math.min(d.by0, by1)), bot = Math.min(rowsBot, Math.max(d.by0, by1));
    clearTimeout(st.bandTimer);
    el.band.classList.remove("out");
    el.band.hidden = false;
    el.band.style.transform = `translate(${left}px, ${top}px)`;
    el.band.style.width = `${Math.max(0, right - left)}px`;
    el.band.style.height = `${Math.max(0, bot - top)}px`;
    const rows = st.lanes.map((L) => ({ id: L.id, y0: L.el.offsetTop, y1: L.el.offsetTop + L.h, pieces: lanePieces(L.id) }));
    const hits = tlBandHits(rows, { x0: d.bx0, x1: bx1, y0: d.by0, y1: by1 });
    const prim = hits.find((s) => sameSel(s, d.sel)) || hits[0] || (d.ctrl ? st.sel : null);
    setSels(tlSelMerge(d.base, hits), prim);
  }

  function edgeScroll() {
    const d = st.drag, e = d.lastE;
    const x = e.clientX - d.rect.left, zone = 28;
    let v = 0;
    if (x < zone) v = -Math.ceil((zone - x) / 2);
    else if (x > st.laneW - zone) v = Math.ceil((x - (st.laneW - zone)) / 2);
    if (!v) return;
    const before = st.scrollX;
    setScroll(st.scrollX + Math.max(-40, Math.min(40, v)));
    if (st.scrollX !== before) apply(d, e);
  }
  function onUp(e) {
    const d = st.drag;
    endDrag();
    if (!d) return;
    if (d.type === "seek" && d.at !== undefined) st.hold = { at: d.at, until: performance.now() + 500 };
    if (!d.moved && d.sel !== undefined) clickSel(d);
    // A drag that ends outside the timeline must not count as a click on the viewer's backdrop.
    if (d.moved || !host.contains(e.target)) swallowClick();
  }
  /** A click on a piece (or on nothing): select it; Ctrl adds / removes it; Shift selects the rectangle from the primary to it. */
  function clickSel(d) {
    const s = d.sel;
    if (d.shift && s && st.sel) {
      const rect = tlSelRect(rowOrder(), st.sel, s, pieceExists);
      return setSels(d.ctrl ? tlSelMerge(st.sels, rect) : rect, st.sel);
    }
    if (d.ctrl) {
      if (!s) return;
      const had = tlSelHas(st.sels, s);
      return setSels(tlSelToggle(st.sels, s), had ? (sameSel(st.sel, s) ? null : st.sel) : s);
    }
    setSel(s);
  }
  function endDrag() {
    window.removeEventListener("mousemove", onMove);
    window.removeEventListener("mouseup", onUp);
    window.removeEventListener("blur", onLost);
    st.drag = null;
    host.classList.remove("tl-dragging");
    delete host.dataset.tlDrag;
    el.tip.hidden = true;
    // The rubber band fades and shrinks a little as it goes, then is hidden.
    if (!el.band.hidden) {
      el.band.classList.add("out");
      clearTimeout(st.bandTimer);
      st.bandTimer = setTimeout(() => { el.band.hidden = true; el.band.classList.remove("out"); }, 170);
    }
    markAll();
  }
  function swallow(e) { e.stopPropagation(); e.preventDefault(); unswallow(); }
  function unswallow() { window.removeEventListener("click", swallow, true); clearTimeout(st.swallowTimer); }
  function swallowClick() {
    unswallow();
    window.addEventListener("click", swallow, true);
    st.swallowTimer = setTimeout(unswallow, 0); // the click, if any, comes right after mouseup
  }

  function onHover(e) {
    if (st.drag) return;
    const lane = e.target.closest("[data-tl-lane]");
    let cur = "";
    if (lane && lane.dataset.tlKind === "file") {
      const row = emRow(m, lane.dataset.tlLane);
      const rect = el.lanes.getBoundingClientRect();
      const hit = row ? fileHit(row, e.clientX - rect.left) : null;
      cur = hit === "move" ? "grab" : hit ? "resize" : "";
    }
    if (el.lanes.dataset.tlCur !== cur) el.lanes.dataset.tlCur = cur;
  }
  function onWheel(e) {
    const rect = el.lanes.getBoundingClientRect();
    const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? st.laneW : 1;
    if (e.ctrlKey) {
      e.preventDefault();
      zoomAround(Math.exp(-e.deltaY * unit * 0.0015), e.clientX - rect.left);
      return;
    }
    const dv = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
    if (!dv) return;
    // Nothing to scroll sideways (whole clip fits): let a vertical wheel scroll the rows instead.
    if (contentW() - st.laneW < 1 && dv === e.deltaY && !e.shiftKey) return;
    e.preventDefault();
    setScroll(st.scrollX + dv * unit);
  }
  function onDbl(e) {
    const cut = e.target.closest("[data-tl-cut]");
    if (cut) doJoin(+cut.dataset.tlCut);
  }

  /* ----- right-click menu ----- */
  // Looks like the app's menu (style.css .menu.ctx) but is our own element, appended to <body>
  // (or to the fullscreen element if the timeline is inside it). Items: <button data-tl-m="act" data-tl-arg="…">.
  function onCtx(e) {
    e.preventDefault();
    e.stopPropagation(); // no other menu (app.js listens on the document)
    if (st.drag) return;
    const rect = el.lanes.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const cutEl = e.target.closest("[data-tl-cut]"), jointEl = e.target.closest("[data-tl-joint]");
    const c = { at: timeAtX(x), cut: cutEl ? +cutEl.dataset.tlCut : -1, joint: jointEl ? st.joints[+jointEl.dataset.tlJoint] || null : null };
    const lane = !cutEl && !jointEl ? e.target.closest("[data-tl-lane]") : null;
    if (lane) {
      const p = pieceAtX(lane.dataset.tlLane, x);
      if (p && !tlSelHas(st.sels, p)) setSel(p); // the piece under the pointer becomes the selection
    }
    openMenu(menuHtml(c), e.clientX, e.clientY, (act, arg) => menuAct(act, arg, c));
  }
  function menuHtml(c) {
    const item = (act, path, label, { arg = "", on = null } = {}) =>
      `<button type="button" role="${on === null ? "menuitem" : "menuitemcheckbox"}"${on === null ? "" : ` aria-checked="${on}"`} data-tl-m="${act}" data-tl-arg="${esc(String(arg))}" class="${on ? "on" : ""}">${tlSvg(path)}<span>${esc(label)}</span></button>`;
    const out = [], top = [];
    const n = st.sels.length;
    if (n > 1) out.push(`<div class="menu-head">${esc(t("{n} selected", { n }))}</div>`);
    if (c.cut >= 0 && c.cut < m.cuts.length) top.push(item("join", tlPaths.join, t("Remove this cut point"), { arg: c.cut }));
    if (c.joint) top.push(item("restore-run", tlPaths.restore, t("Restore the part cut out here")));
    if (canSplitAt(c.at)) top.push(item("split-at", tlPaths.split, t("Split here"), { arg: c.at }));
    const plan = removePlan();
    if (plan.ok) top.push(item("remove", plan.restore ? tlPaths.restore : tlPaths.trash, plan.restore ? t("Restore") : t("Cut out")));
    const aud = selAudio();
    if (aud.length) {
      const ref = refAudio(), g = ref.r.gains[ref.seg], many = aud.length > 1;
      const label = g === 0 ? (many ? t("Unmute these pieces") : t("Unmute this piece")) : many ? t("Mute these pieces") : t("Mute this piece");
      top.push(item("mute", g === 0 ? tlPaths.vol : tlPaths.muted, label));
      const chips = [0.25, 0.5, 1, 1.5, 2].map((v) => {
        const on = Math.abs(g - v) < 0.005;
        return `<button type="button" role="menuitemradio" aria-checked="${on}" data-tl-m="level" data-tl-arg="${v}" class="${on ? "on" : ""}">${esc(t("{pct}%", { pct: Math.round(v * 100) }))}</button>`;
      }).join("");
      top.push(`<div class="tl-mlevel" role="group" aria-label="${esc(t("Level"))}">${tlSvg(tlPaths.level)}<span>${esc(t("Level"))}</span><div class="tl-mchips">${chips}</div></div>`);
      if (m.audio.length > 1 && soloGroups().length) top.push(item("solo", tlPaths.solo, allSolo() ? t("Unsolo") : many ? t("Solo these pieces") : t("Solo this piece")));
    }
    if (st.sels.some((s) => s.row === "video")) {
      const segs = blankSegs(), hidden = segs.length > 0 && !!m.blank[segs[0]];
      top.push(item("blank", hidden ? tlPaths.eye : tlPaths.eyeOff, hidden ? t("Show the video here") : t("Hide the video here")));
    }
    out.push(...top);
    if (top.length) out.push("<hr>");
    out.push(item("hide-cuts", tlPaths.hideCuts, t("Hide cut parts"), { on: st.hideCuts }));
    out.push(item("select-all", tlPaths.selAll, t("Select all pieces")), item("fit", tlPaths.fit, t("Fit the whole clip")));
    if (typeof opts.onCommand === "function") out.push("<hr>", item("shortcuts", tlPaths.keys, t("Shortcuts…")), item("fullscreen", tlPaths.full, t("Fullscreen")));
    return out.join("");
  }
  function menuAct(act, arg, c) {
    switch (act) {
      case "join": return doJoin(+arg);
      case "restore-run": return doRestoreRun(c.joint);
      case "split-at": return doSplit(+arg);
      case "remove": return doRemove();
      case "mute": return doMuteSeg();
      case "level": return doLevel(+arg);
      case "solo": return doSolo();
      case "blank": return doBlank();
      case "hide-cuts": return setHideCuts(!st.hideCuts);
      case "select-all": return selectAll();
      case "fit": fit(); return true;
      case "shortcuts": case "fullscreen": opts.onCommand?.(act); return true;
    }
    return false;
  }
  function openMenu(html, x, y, act) {
    closeMenu();
    const box = document.createElement("div");
    box.className = "menu ctx tl-menu";
    box.setAttribute("role", "menu");
    box.innerHTML = html;
    const fs = document.fullscreenElement;
    (fs && fs.contains(host) ? fs : document.body).appendChild(box);
    // Kept inside the window.
    const r = box.getBoundingClientRect();
    box.style.left = `${Math.max(8, Math.min(x, window.innerWidth - r.width - 8))}px`;
    box.style.top = `${Math.max(8, Math.min(y, window.innerHeight - r.height - 8))}px`;
    const items = () => [...box.querySelectorAll("button:not(:disabled)")];
    const mn = {
      el: box,
      onDown: (e) => { if (!box.contains(e.target)) closeMenu(); },
      onScroll: (e) => { if (!box.contains(e.target)) closeMenu(); },
      // Capture: Esc closes the menu and goes no further (the editor would leave edit mode).
      onKey: (e) => { if (e.key === "Escape") { e.preventDefault(); e.stopImmediatePropagation(); closeMenu(); } },
    };
    box.addEventListener("keydown", (e) => {
      const list = items(), i = list.indexOf(document.activeElement);
      let next = -1;
      if (e.key === "ArrowDown" || e.key === "ArrowRight" || (e.key === "Tab" && !e.shiftKey)) next = i < 0 ? 0 : (i + 1) % list.length;
      else if (e.key === "ArrowUp" || e.key === "ArrowLeft" || (e.key === "Tab" && e.shiftKey)) next = i < 0 ? list.length - 1 : (i - 1 + list.length) % list.length;
      else if (e.key === "Home") next = 0;
      else if (e.key === "End") next = list.length - 1;
      if (next >= 0) { e.preventDefault(); list[next]?.focus(); }
      e.stopPropagation(); // the editor's shortcuts must not run while the menu has the keyboard (Enter / Space click the focused item)
    });
    box.addEventListener("click", (e) => {
      const b = e.target.closest("[data-tl-m]");
      if (!b || b.disabled) return;
      const a = b.dataset.tlM, arg = b.dataset.tlArg;
      closeMenu();
      act(a, arg);
    });
    box.addEventListener("contextmenu", (e) => { e.preventDefault(); e.stopPropagation(); });
    window.addEventListener("mousedown", mn.onDown, true);
    window.addEventListener("keydown", mn.onKey, true);
    window.addEventListener("scroll", mn.onScroll, true);
    window.addEventListener("blur", closeMenu);
    window.addEventListener("resize", closeMenu);
    st.menu = mn;
    items()[0]?.focus({ preventScroll: true });
  }
  function closeMenu() {
    const mn = st.menu;
    if (!mn) return;
    st.menu = null;
    window.removeEventListener("mousedown", mn.onDown, true);
    window.removeEventListener("keydown", mn.onKey, true);
    window.removeEventListener("scroll", mn.onScroll, true);
    window.removeEventListener("blur", closeMenu);
    window.removeEventListener("resize", closeMenu);
    mn.el.remove();
  }

  /* ----- toolbar and header events ----- */
  function onClick(e) {
    const b = e.target.closest("[data-tl-act]");
    if (!b || b.disabled) return;
    const id = b.dataset.tlRow;
    switch (b.dataset.tlAct) {
      case "split": doSplit(); break;
      case "remove": doRemove(); break;
      case "mute": doMuteSeg(); break;
      case "undo": doUndo(false); break;
      case "redo": doUndo(true); break;
      case "zoom-out": zoom(0.8); break;
      case "zoom-in": zoom(1.25); break;
      case "fit": fit(); break;
      case "hide-cuts": setHideCuts(!st.hideCuts); break;
      case "add": opts.onAddAudio?.(); break;
      case "row-mute": {
        const row = emRow(m, id);
        if (!row) return;
        emSnapshot(m);
        row.muted = !row.muted; // the model has no setter for this; see the report
        changed("mute");
        break;
      }
      case "row-remove":
        emSnapshot(m);
        if (!emRemoveRow(m, id)) throw new Error(`timeline: emRemoveRow refused ${id}`);
        changed("file"); // drops the row's pieces from the selection
        break;
    }
  }
  function onInput(e) {
    const r = e.target;
    const isLevel = "tlLevel" in r.dataset, volId = r.dataset.tlVol;
    if (!isLevel && volId === undefined) return;
    if (isLevel && !selAudio().length) return;
    if (st.gest?.el !== r) { emSnapshot(m); st.gest = { el: r }; } // one snapshot per slider gesture
    const v = +r.value / 100;
    if (isLevel) {
      // Every selected audio piece gets the level.
      for (const s of selAudio()) { emSetGain(m, s.r.id, s.seg, v); st.dirty.add(s.r.id); }
      el.levelPct.textContent = t("{pct}%", { pct: Math.round(v * 100) });
      opts.onChange?.("gain");
      updateBar();
    } else {
      const row = emRow(m, volId);
      if (!row) return;
      row.volume = Math.min(EM_MAX_GAIN, Math.max(0, v)); // no setter in the model; see the report
      st.dirty.add(row.id);
      updateHeads();
      opts.onChange?.("volume");
    }
  }
  function onChangeEv(e) {
    if (e.target.type !== "range") return;
    st.gest = null;
    e.target.blur(); // so the editor's shortcuts work again
    updateBar();
    updateHeads();
  }

  /* ----- observers ----- */
  const ro = new ResizeObserver(() => {
    const w = el.lanes.clientWidth, sb = el.scroll.clientWidth;
    if (w === st.laneW && sb === st.sbW) return;
    st.laneW = w;
    st.sbW = sb;
    st.needSize = true;
    if (st.fitted) setPps(minPps()); else setPps(st.pps);
    setScroll(st.scrollX);
    syncSpacer();
    markAll();
    updateBar();
  });
  ro.observe(el.lanes);
  ro.observe(el.scroll);
  // Themes and custom CSS are <style> elements in <head>: redraw the canvases when they change.
  const mo = new MutationObserver(() => { st.hatch = null; markAll(); });
  mo.observe(document.head, { childList: true, subtree: true, characterData: true });
  const onScroll = () => {
    const v = el.scroll.scrollLeft;
    if (Math.abs(v - st.scrollX) >= 1) { st.scrollX = Math.min(Math.max(0, contentW() - st.laneW), v); markAll(); }
  };

  el.lanes.addEventListener("mousedown", onDown);
  el.lanes.addEventListener("mousemove", onHover);
  el.lanes.addEventListener("wheel", onWheel, { passive: false });
  el.lanes.addEventListener("dblclick", onDbl);
  el.lanes.addEventListener("contextmenu", onCtx);
  el.scroll.addEventListener("scroll", onScroll);
  host.addEventListener("click", onClick);
  host.addEventListener("input", onInput);
  host.addEventListener("change", onChangeEv);

  buildRows();
  updateHeads();
  updateBar();
  st.raf = requestAnimationFrame(frame);

  /* ----- handle ----- */
  return {
    refresh() {
      if (st.dead) return;
      markAll(); // first: the view mapping must follow the model before anything measures with it
      syncRows();
      validateSel();
      if (st.hideCuts) relayout(); else { setPps(st.pps); setScroll(st.scrollX); }
      markAll();
      updateBar();
    },
    setThumbs(urls) {
      if (st.dead) return;
      for (const x of st.thumbs) { x.img.onload = x.img.onerror = null; }
      st.thumbs = (Array.isArray(urls) ? urls : []).map((url) => {
        const x = { img: new Image(), ok: false };
        x.img.decoding = "async";
        x.img.onload = () => { x.ok = x.img.naturalWidth > 0; st.dirty.add("video"); };
        x.img.onerror = () => console.warn("Glint timeline: could not load a film strip frame", url);
        x.img.src = url;
        return x;
      });
      st.dirty.add("video");
    },
    setWaveform(rowId, data) {
      if (st.dead) return;
      const peaks = data?.peaks, ps = +data?.perSecond;
      if (!peaks || typeof peaks.length !== "number" || !(ps > 0)) {
        console.error("Glint timeline: setWaveform needs { peaks: number[], perSecond > 0 }", rowId, data);
        return;
      }
      st.waves.set(rowId, { peaks, ps });
      st.dirty.add(rowId);
    },
    selection: () => (st.sel ? { ...st.sel } : null),
    selections: () => st.sels.map((s) => ({ ...s })),
    select(sel) { if (!st.dead) setSel(sel); },
    zoom(factor) { if (!st.dead) zoom(factor); },
    fit() { if (!st.dead) fit(); },
    hideCuts: () => st.hideCuts,
    setHideCuts(on) { if (!st.dead) setHideCuts(on); },
    /** Run an edit command on the selection (the editor's keys). True if it did something. */
    run(cmd, arg) {
      if (st.dead) return false;
      if (st.drag && cmd !== "clearSel") return false; // not in the middle of a drag: indexes would move under it
      switch (cmd) {
        case "split": return doSplit(+opts.getTime() || 0);
        case "remove": return doRemove();
        case "mute": return doMuteSeg();
        case "louder": return doNudge(0.1);
        case "quieter": return doNudge(-0.1);
        case "level": return arg !== null && arg !== "" && Number.isFinite(+arg) ? doLevel(+arg) : false;
        case "solo": return doSolo();
        case "blank": return doBlank();
        case "selectAll": return selectAll();
        case "clearSel": { const had = st.sels.length > 0; setSels([], null); return had; }
        case "hideCuts": return setHideCuts(!st.hideCuts);
      }
      console.warn("Glint timeline: run() does not know the command", cmd);
      return false;
    },
    destroy() {
      if (st.dead) return;
      st.dead = true;
      cancelAnimationFrame(st.raf);
      closeMenu();
      endDrag();
      unswallow();
      ro.disconnect();
      mo.disconnect();
      el.lanes.removeEventListener("mousedown", onDown);
      el.lanes.removeEventListener("mousemove", onHover);
      el.lanes.removeEventListener("wheel", onWheel);
      el.lanes.removeEventListener("dblclick", onDbl);
      el.lanes.removeEventListener("contextmenu", onCtx);
      el.scroll.removeEventListener("scroll", onScroll);
      host.removeEventListener("click", onClick);
      host.removeEventListener("input", onInput);
      host.removeEventListener("change", onChangeEv);
      for (const x of st.thumbs) { x.img.onload = x.img.onerror = null; x.img.src = ""; }
      st.thumbs = [];
      st.waves.clear();
      host.innerHTML = "";
      host.classList.remove("tl", "tl-dragging");
    },
  };
}

// Lets the pure helpers be tested under Node; ignored in the app.
if (typeof module !== "undefined" && module.exports) {
  module.exports = { tlPickStep, tlFmtTick, tlFmtPrecise, tlSnap, tlSnapV, tlFileHit, tlSpanHit, tlTrimLeft, tlPeakMax, tlHue, tlViewMap, tlSrcToView, tlViewToSrc, tlJoints, tlSelKey, tlSelHas, tlSelMerge, tlSelToggle, tlSelRect, tlBandHits, tlSelShift, tlFmtLen };
}
