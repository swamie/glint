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
//   .tl-grip    drag it to make the rows taller or shorter (st.rowK, see "track height")
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
// Bookmarks (m.marks): a flag on the ruler (.tl-bmks) and a thin line through the rows (.tl-bmls)
// each. They are not part of the edit: no undo snapshot, no onChange; opts.onMarks(list) instead.
//
// Trim brackets (.tl-trims): [ where the kept span starts and ] where it ends (emTrimBounds). Dragging one
// re-applies emSetTrimStart / emSetTrimEnd to the segments as they were when the drag began (emSegCopy),
// so a drag never piles up cut points and a part cut out in the middle comes back cut out if the bracket
// is dragged past it and back. The cut point under a bracket has no handle of its own (the bracket is it).
// With cut parts hidden, a bracket drag shows them for as long as it lasts (d.rehide), so a bracket can be
// pulled back out over footage that was cut away; they are hidden again on release.
//
// Parts (m.parts, "Edit together"): a strip of clip blocks (lane "parts") above the video row, one per part in
// display order. Drag a block sideways to move the clip (emMovePart, one undo step, onChange("order")); click
// it to go to its start; right-click: move left / right, cut the whole clip out or put it back. Part borders
// are locked cuts (emCutLocked): drawn stronger, no handle, never joined. All times here are then TIMELINE
// seconds (the model's); only the film strip and the clip's own track waveforms are source-time data and
// are looked up through emToSrc, so each block shows its own pictures and sound.
//
// After the video (0.1.35): the result ends where its last thing ends (emEnd: the video, an overlay item, a file
// row). The lanes run on past that by free space (tlViewSpan) where the playhead can stand and things can be put.
// Overlay rows (lanes "ov0", "ov1", ... kind "ov", ov0 just above the video, drawn top-down from the highest):
// one block per item of m.overlays, laid out by tlOvRows. Drag a block to move it, an end to trim it (emMoveOverlay,
// emTrimOverlay), up or down to bring it to the front or send it to the back; a click selects it (opts.onOverlayPick,
// and the overlay code lights it with handle.overlaySel). Row heads are named like an editor's: V2, V3 ... for the
// overlay rows, V1 the video, A1, A2 ... the audio rows. The wheel over a selected file block slips its sound.
//
// NOTE: never name a variable `t` here: it would hide the translation function t().

const tlRulerH = 26;    // px, ruler height (keep in sync with timeline.css)
const tlVideoH = 56;    // px, video row height
const tlAudioH = 50;    // px, audio row height
const tlPartsH = 28;    // px, the strip of clip blocks above the video row (only for a model with parts)
const tlOvH = 30;       // px, one row of overlay blocks (texts, shapes, blur boxes, pictures) above the video (0.1.35)
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
  names: "M4 6h9l7 6-7 6H4zM8.5 12h.01",
  through: "M3 12h18M7 8l-4 4 4 4M17 8l4 4-4 4",
  selAll: "M4 4h16v16H4zM8 12l3 3 5-6",
  keys: "M3 6h18v12H3zM7 10h1M11 10h1M15 10h1M7 14h10",
  full: "M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5",
  mark: "M6 21V4M6 4h12l-3 4.5 3 4.5H6",
  markOff: "M6 21V4M6 4h12l-3 4.5 3 4.5H6M3 3l18 18",
  markNext: "M4 12h11M11 7l5 5-5 5M20 5v14",
  markPrev: "M20 12H9M13 7l-5 5 5 5M4 5v14",
  trimStart: "M9 4H5v16h4M10 12h10M16 8l4 4-4 4",   // [ ->
  trimEnd: "M15 4h4v16h-4M14 12H4M8 8l-4 4 4 4",    // <- ]
  partLeft: "M20 12H6M11 7l-5 5 5 5",
  partRight: "M4 12h14M13 7l5 5-5 5",
  crop: "M6 2v16h16M2 6h16v16",
  pointer: "M6 3l13 8-6 1.5L10 19z",
  blade: "M4 20 13 11M13 11l6-6a2.1 2.1 0 0 0-3-3l-6 6zM8 8l8 8",
  gapClose: "M4 6v12M20 6v12M8 12h8M8 12l3-3M8 12l3 3M16 12l-3-3M16 12l-3 3",
  lift: "M4 7h16v10H4zM9 7v10M15 7v10",
  ungroup: "M3 6h8v5H3zM13 13h8v5h-8zM11 9l2 2",
  group: "M3 6h8v5H3zM13 13h8v5h-8zM7 11v4h6",
  dup: "M8 8h12v12H8zM4 16V4h12",
  front: "M4 14h8v6H4zM10 4h10v10H10z",
  back: "M10 4h10v10H10zM4 10h8v10H4z",
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
/** View time -> source time. At a joint: the start of the kept part after it. Clamped to the kept parts, but after
 *  the end it is the time after the video (0.1.35), as tlSrcToView maps it. */
function tlViewToSrc(map, v) {
  if (!map.collapse) return v;
  const R = map.ranges;
  if (!R.length) return 0;
  if (v <= 0) return R[0].s0;
  if (v > map.total + 1e-9) return map.lastSrc + (v - map.total);
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
    for (const p of r.pieces) if (p.x1 - p.x0 >= 0.5 && p.x0 < x1 && p.x1 > x0) out.push({ row: p.row || r.id, seg: p.seg }); // a piece may name its own row (an audio clip on a row: 0.1.39)
  }
  return out;
}
/** Selection after its segment indexes moved: a cut was added before segment `at` + 1 (delta 1) or cut `at` was joined (delta -1). */
function tlSelShift(list, at, delta) {
  return tlSelMerge([], list.map((s) => (s.seg > at ? { row: s.row, seg: s.seg + delta } : s)));
}

/** Dragging clip block `from` (of `mids.length`, block middles in lane x): the gap the pointer at `x` is
 *  nearest, 0..n (before block k), and the display index the block would get there (`to`). */
function tlPartSlot(mids, from, x) {
  let slot = 0;
  while (slot < mids.length && x > mids[slot]) slot++;
  return { slot, to: slot > from ? slot - 1 : slot };
}

/** The overlay rows (0.1.35): which row each item of `items` (m.overlays, in draw order: later = on top) goes on,
 *  0 = the row just above the video. An item goes one row above every earlier item it overlaps in time, so of two
 *  overlapping items the one drawn on top is always the higher block. Returns { rows: [row per item], n: rows used }. */
function tlOvRows(items) {
  const rows = [], list = Array.isArray(items) ? items : [];
  let n = 0;
  list.forEach((o, i) => {
    let k = 0;
    for (let j = 0; j < i; j++) {
      const p = list[j];
      if (p && o && p.from < o.to - 1e-9 && o.from < p.to - 1e-9) k = Math.max(k, rows[j] + 1);
    }
    rows.push(k);
    n = Math.max(n, k + 1);
  });
  return { rows, n };
}
/** How long (view seconds) the timeline is when the result ends at view second `end`: free space after it, at least
 *  10 s and a quarter of the whole (so a third of `end`), for the playhead and for placing things. */
const tlViewSpan = (end) => Math.max(0, end) + Math.max(10, Math.max(0, end) / 3);
/** The wheel over a selected file block slips its sound: seconds per notch (Shift: fine, Ctrl: 1 s), signed by `dy`. */
const tlSlipStep = (dy, shift, ctrl) => (dy > 0 ? 1 : dy < 0 ? -1 : 0) * (ctrl ? 1 : shift ? 0.05 : 0.25);

/** Where a clip dragged on the video row goes (0.1.36). `parts`: emParts(m), `from` the dragged one's index, `sec` the
 *  timeline second under the pointer. Over another clip: { mode: "beside", k, after } (the nearer half); over a gap,
 *  the free space after the end or the clip itself: { mode: "free" }. */
function tlDropAt(parts, from, sec) {
  const p = (parts || []).find((q) => !q.gap && q.i !== from && sec >= q.at && sec < q.end);
  return p ? { mode: "beside", k: p.i, after: sec >= (p.at + p.end) / 2 } : { mode: "free" };
}

/** The stretches to outline for the selected pieces of row `rowId` on a block [a, b] (0.1.41): an audio clip (`clip`)
 *  whole when it is selected; another row per selected segment `segs[sel.seg]` inside the block. */
function tlSelSpans(sels, rowId, clip, segs, a, b) {
  const mine = (sels || []).filter((s) => s.row === rowId);
  if (!mine.length) return [];
  if (clip) return b > a ? [[a, b]] : [];
  return mine.map((s) => segs[s.seg]).filter(Boolean).map((s) => [Math.max(a, s.start), Math.min(b, s.end)]).filter(([lo, hi]) => hi > lo);
}

/** "3.2 s" style length for a tooltip. */
const tlFmtLen = (sec) => (Math.round(Math.max(0, sec) * 10) / 10).toFixed(1);
/** The speed choices for a piece (0.1.41): the menu's chips and the inspector's list. */
const TL_SPEEDS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 4];
/** A speed as the block shows it (0.1.41): "2x", "0.5x", "1.25x". */
const tlSpeedText = (sp) => `${+(+sp).toFixed(2)}x`;

/** Left-edge trim of a file clip: the new start `at`, keeping the audio where it is. Returns { offset, trimStart, length }. */
function tlTrimLeft(row0, at, minSeg) {
  let d = at - row0.offset;
  d = Math.max(d, -row0.trimStart, -row0.offset);
  d = Math.min(d, row0.length - minSeg);
  return { offset: row0.offset + d, trimStart: row0.trimStart + d, length: row0.length - d };
}

/** Largest peak in index range [i0, i1) (at least one index), 0 outside the array. */
/** How much taller than its true level a waveform is drawn (clipped at the row's height): quiet audio stays readable. */
const tlWaveGain = 2;
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

/** Hue (0..360) of a "#rgb", "#rrggbb" or "rgb(...)" color, or null. */
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

/* ---------- track height (the grip under the tracks) ---------- */
// The video and audio rows are tlVideoH / tlAudioH times a factor the user sets by dragging the grip under the
// tracks (remembered in localStorage["glint.tl.rowK"]; a double-click puts it back to 1). 1 is the smallest:
// an audio row's header (name, mute, volume) needs its 50 px.
const tlRowKMin = 1;
const tlRowKMax = 3;
const tlRowKKey = "glint.tl.rowK";
/** The factor after dragging the grip `dy` px down (negative: up) from factor `k0`, for rows `base` px tall at factor 1:
 *  the rows grow by what the pointer moved, so the grip stays under it. */
function tlRowScale(k0, dy, base) {
  const k = (Number.isFinite(k0) ? k0 : 1) + (base > 0 && Number.isFinite(dy) ? dy / base : 0);
  return Math.round(Math.min(tlRowKMax, Math.max(tlRowKMin, k)) * 1000) / 1000;
}
/** A remembered factor (localStorage text, or null): a number in range, else 1. */
function tlRowKParse(s) {
  const k = parseFloat(s);
  return Number.isFinite(k) ? Math.min(tlRowKMax, Math.max(tlRowKMin, k)) : 1;
}

/* ---------- the timeline ---------- */

/** What an audio row is called on the timeline. The clip's own tracks say what they hold ("Mic audio",
 *  "Overwatch audio", "Desktop audio"); an added file keeps its name. `game` is the clip's game ("" or "Desktop" = none). */
function tlRowName(r, game) {
  if (!r || r.kind !== "track") return r ? t(r.title) : "";
  // Rows by role (0.1.44): named by what they are, whatever the recorder called the track.
  if (r.role === "desktop") return t("Desktop audio");
  if (r.role === "mic") return t("Mic audio");
  if (r.role === "game") return game && game !== "Desktop" ? t("{game} audio", { game }) : t("Game audio");
  if (r.role === "clip") return t("Clip audio");
  if (r.title === "Microphone") return t("Mic audio");
  if (r.title === "Desktop") return t("Desktop audio");
  if (r.title === "Game") return game && game !== "Desktop" ? t("{game} audio", { game }) : t("Game audio");
  if (r.title === "Mix") return t("All audio");
  return t(r.title);
}

function mountTimeline(host, opts) {  const m = opts.model;
  const dur = () => Math.max(0, +m.duration || 0);
  const st = {
    pps: 100, scrollX: 0, laneW: 0, sbW: 0, dpr: window.devicePixelRatio || 1,
    fitted: true,          // follow the width when the lane is resized
    sel: null,             // the primary selected piece { row, seg }; always one of sels
    sels: [],              // every selected piece [{ row, seg }]
    selNotify: false,      // the selection's indexes were shifted: tell the editor on the next setSels
    hideCuts: false,       // "Hide cut parts": removed segments take no room
    map: null,             // view mapping (tlViewMap), rebuilt on demand after markAll
    mapHold: null,         // the mapping a trim drag with cut parts hidden measures and draws with (see the file header)
    joints: [], jointSig: "",
    markSig: "",           // what the bookmark flags were last built for
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
    rowK: 1,               // row height factor (the grip under the tracks)
    rdrag: null,           // the grip drag in progress { y, k0, base, moved }
    ovSel: null,           // id of the selected overlay block (0.1.35), or null
    ovSigShown: "",        // the overlay items the lanes were last drawn for (they may change outside the timeline)
    slip: null,            // a burst of wheel turns slipping a file row { id, timer }: one undo step
    tcText: "",            // the timecode shown in the toolbar
    tool: "select",        // "select" or "blade" (0.1.36): with the blade a click splits there
  };
  st.hideCuts = false; // 0.1.37: a cut really removes, so "Hide cut parts" has nothing to hide (its button is gone)
  st.names = true; // the name written on each audio row's block (a song's title); remembered like "Hide cut parts"
  try { st.names = localStorage.getItem("glint.tl.names") !== "0"; } catch {}
  try { st.rowK = tlRowKParse(localStorage.getItem(tlRowKKey)); } catch {}
  const vidH = () => Math.round(tlVideoH * st.rowK);
  const audH = () => Math.round(tlAudioH * st.rowK);

  /* ----- DOM ----- */
  host.classList.add("tl");
  const btn = (act, path, title, extra = "") => `<button type="button" class="tl-btn" data-tl-act="${act}" title="${esc(title)}" aria-label="${esc(title)}"${extra}>${tlSvg(path)}</button>`;
  host.innerHTML = `
    <div class="tl-bar" role="toolbar" aria-label="${esc(t("Timeline"))}">
      <span class="tl-tc" role="timer" aria-live="off" title="${esc(t("Playhead"))}">0:00.00</span>
      ${btn("tool-select", tlPaths.pointer, t("Select and move"), ' aria-pressed="true"')}
      ${btn("tool-blade", tlPaths.blade, t("Blade: click the video or an overlay to split it there (C)"), ' aria-pressed="false"')}
      <span class="tl-sep"></span>
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
      ${btn("hide-cuts", tlPaths.hideCuts, t("Hide cut parts (H)"), ' aria-pressed="false" hidden')}
      ${btn("names", tlPaths.names, t("Names on the audio"), ' aria-pressed="true"')}
      <span class="tl-sep"></span>
      <button type="button" class="btn sm tl-add" data-tl-act="add" title="${esc(t("Add an audio file as a new track"))}">${ICON.plus}<span>${esc(t("Add audio"))}</span></button>
    </div>
    <div class="tl-rowsbar" hidden></div>
    <div class="tl-body">
      <div class="tl-heads"><div class="tl-corner"></div><div class="tl-hrows"></div></div>
      <div class="tl-lanes">
        <div class="tl-ruler" data-tl-ruler title="${esc(t("Click or drag to move the playhead"))}"><canvas></canvas><div class="tl-ph-k" data-tl-ph></div><div class="tl-bmks"></div></div>
        <div class="tl-rows"></div>
        <div class="tl-marks"><div class="tl-bmls"></div><div class="tl-joints"></div><div class="tl-trims"><div class="tl-trim s" data-tl-trim="start" style="top:${tlRulerH}px"><b></b></div><div class="tl-trim e" data-tl-trim="end" style="top:${tlRulerH}px"><b></b></div></div><div class="tl-cuts"></div><div class="tl-pdrop" hidden></div><div class="tl-ph"></div></div>
        <div class="tl-band" hidden></div>
        <div class="tl-tip" hidden></div>
      </div>
    </div>
    <div class="tl-sbar"><div></div><div class="tl-scroll"><div class="tl-spacer"></div></div></div>
    <div class="tl-grip" role="separator" aria-orientation="horizontal" aria-label="${esc(t("Track height"))}" title="${esc(t("Drag to make the tracks taller or shorter. Double-click to reset."))}"><span></span></div>`;
  const q = (sel) => host.querySelector(sel);
  const el = {
    bar: q(".tl-bar"), rowsbar: q(".tl-rowsbar"), body: q(".tl-body"), sbar: q(".tl-sbar"), grip: q(".tl-grip"), hrows: q(".tl-hrows"), lanes: q(".tl-lanes"), rows: q(".tl-rows"),
    ruler: q(".tl-ruler canvas"), phK: q(".tl-ph-k"), ph: q(".tl-ph"), cuts: q(".tl-cuts"), joints: q(".tl-joints"), tip: q(".tl-tip"), band: q(".tl-band"),
    bHide: q('[data-tl-act="hide-cuts"]'), bNames: q('[data-tl-act="names"]'), bFit: q('[data-tl-act="fit"]'),
    bmFlags: q(".tl-bmks"), bmLines: q(".tl-bmls"), tc: q(".tl-tc"), bSelect: q('[data-tl-act="tool-select"]'), bBlade: q('[data-tl-act="tool-blade"]'),
    trimS: q('[data-tl-trim="start"]'), trimE: q('[data-tl-trim="end"]'), pdrop: q(".tl-pdrop"),
    scroll: q(".tl-scroll"), spacer: q(".tl-spacer"),
    level: q("[data-tl-level]"), levelPct: q(".tl-level .tl-pct"), levelBox: q(".tl-level"),
    bSplit: q('[data-tl-act="split"]'), bRemove: q('[data-tl-act="remove"]'), bMute: q('[data-tl-act="mute"]'),
    bUndo: q('[data-tl-act="undo"]'), bRedo: q('[data-tl-act="redo"]'), bOut: q('[data-tl-act="zoom-out"]'), bIn: q('[data-tl-act="zoom-in"]'),
  };
  const rulerCtx = el.ruler.getContext("2d");

  /* ----- tooltips with the current shortcuts ----- */
  /** "Label (key)" with the current shortcut of editor command `cmd` (opts.keyText), without brackets when it has none.
   *  Without opts.keyText: `fixed`, the label with the default key. */
  function keyTip(label, cmd, fixed) {
    const k = keyName(cmd);
    if (k === null) return fixed;
    return k ? `${label} (${k})` : label;
  }
  /** The current shortcut of editor command `cmd` as text ("" if none), or null without opts.keyText. */
  function keyName(cmd) {
    if (typeof opts.keyText !== "function") return null;
    try { return String(opts.keyText(cmd) || ""); } catch (e) { console.warn("Glint timeline: keyText failed", cmd, e); return ""; }
  }
  /** The trim brackets' tooltips: what a drag does, the key that trims to the playhead, and with cut parts hidden, why they do not move outward. */
  function trimTips() {
    for (const [b, side] of [[el.trimS, "start"], [el.trimE, "end"]]) {
      const k = keyName(side === "start" ? "trimStart" : "trimEnd") ?? (side === "start" ? "I" : "O");
      const lines = [side === "start" ? t("Drag to trim the start") : t("Drag to trim the end")];
      if (k) lines.push(t("To the playhead: {key}", { key: k }));
      lines.push(t("Drag it back out to bring cut footage back."));
      const title = lines.join("\n");
      if (b.title !== title) b.title = title;
    }
  }
  /** The fixed toolbar buttons' tooltips (remove and mute change with the selection: see updateBar). */
  function barTips() {
    const set = (b, title) => { if (b.title !== title) { b.title = title; b.setAttribute("aria-label", title); } };
    set(el.bSplit, keyTip(t("Split at the playhead"), "split", t("Split at the playhead (S)")));
    set(el.bUndo, keyTip(t("Undo"), "undo", t("Undo (Ctrl+Z)")));
    set(el.bRedo, keyTip(t("Redo"), "redo", t("Redo (Ctrl+Y)")));
    set(el.bOut, keyTip(t("Zoom out"), "zoomOut", t("Zoom out (-)")));
    set(el.bIn, keyTip(t("Zoom in"), "zoomIn", t("Zoom in (+)")));
    set(el.bFit, keyTip(t("Fit the whole clip"), "fit", t("Fit the whole clip (Shift+F)")));
    set(el.bHide, keyTip(t("Hide cut parts"), "hideCuts", t("Hide cut parts (H)")));
    set(el.bBlade, keyTip(t("Blade: click the video or an overlay to split it there"), "blade", t("Blade: click the video or an overlay to split it there (C)")));
    trimTips();
  }
  barTips();

  /* ----- mapping ----- */
  // Source time <-> view time (see the file header). The map is rebuilt after every markAll, so it follows the model.
  const mapSig = () => (st.hideCuts ? `${m.duration}|${m.cuts.join(",")}|${m.removed.join(",")}` : "");
  const vmap = () => st.mapHold || (st.map ||= Object.assign(tlViewMap(st.hideCuts ? emKept(m) : null, st.hideCuts, dur()), { sig: mapSig() }));
  const toV = (sec) => tlSrcToView(vmap(), sec);
  const toS = (v) => tlViewToSrc(vmap(), v);
  const vdur = () => vmap().total; // the video's length in view time
  /** Where the result ends (timeline seconds; emEnd), in view time, and the whole length of the lanes with the free space after it. */
  const endT = () => (typeof emEnd === "function" ? emEnd(m) : dur());
  const vEnd = () => toV(endT());
  const vspan = () => tlViewSpan(vEnd());
  /** The latest timeline second the playhead can be put at: the end of the free space. */
  const maxT = () => toS(vspan());
  const XV = (v) => tlPad + v * st.pps - st.scrollX;
  const vAtX = (x) => (x + st.scrollX - tlPad) / st.pps;
  const X = (sec) => XV(toV(sec));
  const timeAtX = (x) => toS(vAtX(x));
  const contentW = () => vspan() * st.pps + 2 * tlPad;
  const minPps = () => (st.laneW > 2 * tlPad + 10 && vspan() > 0 ? (st.laneW - 2 * tlPad) / vspan() : 1);
  const maxPps = () => Math.max(tlMaxPps, minPps());
  const markAll = () => { st.dirtyAll = true; st.dirtyRuler = true; st.map = null; };
  /** A segment is drawn (and can be picked) unless it is cut out while cut parts are hidden. */
  const segShown = (i) => !(st.hideCuts && m.removed[i]);
  /** True for a clip made of parts ("Edit together"): the clip blocks strip is shown. */
  const hasParts = () => Array.isArray(m.parts) && m.parts.length > 0; // timeline != source (parts, or gaps: 0.1.36)
  /** The strip of clip blocks is shown for two clips or more (a single clip with gaps has none). */
  // 0.1.37: the pieces themselves lie on the video row and move there: the strip of clip blocks above it is not shown.
  const showStrip = () => false;
  /** Display index of the gap at timeline second `sec`, or -1; and whether segment `i` is a gap (0.1.36). */
  const gapAt = (sec) => (m.parts ? emGapAt(m, sec) : -1);
  const segGap = (i) => !!m.parts && emSegIsGap(m, i);
  /* Overlay items (0.1.35, see the file header): m.overlays, their rows (tlOvRows, cached by a signature). */
  const ovItems = () => (Array.isArray(m.overlays) ? m.overlays : []);
  const ovSig = () => ovItems().map((o) => `${o.id}:${o.from}:${o.to}`).join("|");
  let ovCache = { sig: null, rows: [], n: 0 };
  const ovLayout = () => { const s = ovSig(); if (ovCache.sig !== s) ovCache = { sig: s, ...tlOvRows(ovItems()) }; return ovCache; };
  const ovN = () => ovLayout().n;
  const ovById = (id) => ovItems().find((o) => o.id === id) || null;
  /** Top of the rows that the cut handles, brackets and bookmark lines reach down from: the video row. */
  const rowsTop = () => tlRulerH + ovN() * tlOvH + (showStrip() ? tlPartsH : 0);
  /** True if cut `c` is a part border (no handle, cannot be joined or dragged). */
  const lockedCut = (c) => hasParts() && emCutLocked(m, c);
  /** Timeline second -> source second, for the source-time data (film strip, track waveforms). */
  const srcAt = (sec) => (hasParts() ? emToSrc(m, sec) : sec);
  /** The own speed of the piece under timeline second `sec` (1 without one, 0.1.41). */
  const pieceSpeedAt = (sec) => (hasParts() ? emParts(m)[emPartAt(m, sec)]?.speed || 1 : 1);
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
  // 0.1.39: the audio rows are emLanes (the clip's own track rows, then the added rows); clips lie on them.
  const rowSig = () => `${ovN()}|${showStrip() ? 1 : 0}|` + emLanes(m).map((r) => `${r.id}/${r.kind}`).join("|");
  function headHtml(r, i) {
    const file = r.kind === "lane";
    return `<div class="tl-head${r.muted ? " off" : ""}" data-tl-row="${esc(r.id)}" style="height:${audH()}px">
      <div class="tl-hline">
        <span class="tl-htag">A${i + 1}</span>
        <button type="button" class="tl-btn tl-hmute" data-tl-act="row-mute" data-tl-row="${esc(r.id)}"></button>
        <span class="tl-hname"></span>
        <!-- 0.1.38: no "plays through cuts" switch: an added file is always heard wherever its block lies -->
        <button type="button" class="tl-btn tl-hx" data-tl-act="row-remove" data-tl-row="${esc(r.id)}" title="${esc(file ? t("Remove this audio file") : t("Delete this track from the result"))}" aria-label="${esc(file ? t("Remove this audio file") : t("Delete this track from the result"))}">${ICON.trash}</button>
      </div>
      <div class="tl-hline"><input type="range" class="tl-range" min="0" max="200" step="1" data-tl-vol="${esc(r.id)}" aria-label="${esc(t("Track volume"))}"><output class="tl-pct"></output></div>
    </div>`;
  }
  function buildRows() {
    st.rowsSig = rowSig();
    const partsTip = esc(t("Drag a clip to change the order. Right-click for more."));
    const ovTip = esc(t("Texts, shapes, blur boxes and pictures on the video. Drag a block to move it, an end to trim it, up or down to bring it to the front or send it to the back."));
    const ovRows = Array.from({ length: ovN() }, (_, i) => ovN() - 1 - i); // the highest row on top
    el.hrows.innerHTML = ovRows.map((k) => `<div class="tl-head tl-head-ov" style="height:${tlOvH}px" title="${ovTip}"><span class="tl-htag">V${k + 2}</span><span class="tl-hname">${esc(t("Overlays"))}</span></div>`).join("") +
      (showStrip() ? `<div class="tl-head tl-head-parts" style="height:${tlPartsH}px" title="${partsTip}">${esc(t("Clips"))}</div>` : "") +
      `<div class="tl-head tl-head-video" style="height:${vidH()}px" title="${esc(opts.title || t("Video"))}" aria-label="${esc(t("Video"))}"><span class="tl-htag">V1</span>${tlSvg(tlPaths.film)}<span class="tl-hname">${esc(t("Video"))}</span></div>` + emLanes(m).map(headHtml).join("") +
      `<div class="tl-head tl-head-add"><button type="button" class="tl-btn tl-hadd" data-tl-act="add" title="${esc(t("Add an audio file as a new track"))}" aria-label="${esc(t("Add an audio file as a new track"))}">${ICON.plus}</button><span class="tl-hname">${esc(t("Add audio"))}</span>` +
      // 0.1.44: rows by role, or one per track title ("Show every track"); shown once the tracks are known.
      `<button type="button" class="tl-rmode" data-tl-act="rows-mode" role="switch" aria-checked="false" hidden title="${esc(t("Show every track of the clips as its own row, as the recorders named them"))}">${esc(t("Every track"))}</button></div>`;
    const list = [...ovRows.map((k) => ({ id: `ov${k}`, kind: "ov", k, h: tlOvH })), ...(showStrip() ? [{ id: "parts", kind: "parts", h: tlPartsH }] : []), { id: "video", kind: "video", h: vidH() }, ...emLanes(m).map((r) => ({ id: r.id, kind: r.kind, h: audH() }))];
    el.rows.innerHTML = list.map((L) => `<div class="tl-lane" data-tl-lane="${esc(L.id)}" data-tl-kind="${L.kind}" style="height:${L.h}px"${L.kind === "parts" ? ` title="${partsTip}"` : ""}><canvas></canvas></div>`).join("");
    el.trimS.style.top = el.trimE.style.top = `${rowsTop()}px`;
    // The cut handles, joints and bookmark lines start at the video row: made again with the new top on the next draw.
    el.cuts.innerHTML = "";
    st.jointSig = st.markSig = "";
    const els = el.rows.querySelectorAll(".tl-lane");
    st.lanes = list.map((L, i) => {
      const canvas = els[i].firstChild;
      return { ...L, el: els[i], canvas, ctx: canvas.getContext("2d") };
    });
    st.needSize = true;
    markAll();
    growHost();
  }
  /** The rows at the current height factor, without rebuilding them (the grip drag). */
  function applyRowK() {
    for (const L of st.lanes) {
      if (L.kind === "parts" || L.kind === "ov") continue;
      L.h = L.kind === "video" ? vidH() : audH();
      L.el.style.height = `${L.h}px`;
    }
    for (const h of el.hrows.querySelectorAll(".tl-head")) {
      if (h.classList.contains("tl-head-parts") || h.classList.contains("tl-head-ov") || h.classList.contains("tl-head-add")) continue;
      h.style.height = `${h.classList.contains("tl-head-video") ? vidH() : audH()}px`;
    }
    st.needSize = true;
    markAll();
    growHost();
  }
  /** Rows made taller than the default may make the whole area taller, taking the room from the video above:
   *  --tl-want is the height that shows every row; the editor's CSS keeps the area between its usual height
   *  and what leaves the video a usable size. At factor 1 the area is the usual height. */
  function growHost() {
    const want = st.rowK > tlRowKMin
      ? `${Math.ceil(el.bar.offsetHeight + el.body.scrollHeight + el.sbar.offsetHeight + el.grip.offsetHeight + (host.offsetHeight - host.clientHeight))}px`
      : "";
    if (host.style.getPropertyValue("--tl-want") === want) return;
    if (want) host.style.setProperty("--tl-want", want); else host.style.removeProperty("--tl-want");
  }
  /** Rows of `base` px at factor 1 that the grip scales: the video row and the audio rows. */
  const rowsBase = () => tlVideoH + emLanes(m).length * tlAudioH;
  function saveRowK() {
    try { if (st.rowK > tlRowKMin) localStorage.setItem(tlRowKKey, String(st.rowK)); else localStorage.removeItem(tlRowKKey); } catch {}
  }
  function onGripDown(e) {
    if (e.button !== 0 || st.drag) return;
    e.preventDefault();
    st.rdrag = { y: e.clientY, k0: st.rowK, base: rowsBase(), moved: false };
    host.classList.add("tl-dragging");
    host.dataset.tlDrag = "rows";
    window.addEventListener("mousemove", onGripMove);
    window.addEventListener("mouseup", onGripUp);
  }
  function onGripMove(e) {
    const d = st.rdrag;
    if (!d) return;
    if (!d.moved && Math.abs(e.clientY - d.y) < tlDragPx) return;
    d.moved = true;
    const k = tlRowScale(d.k0, e.clientY - d.y, d.base);
    if (k !== st.rowK) { st.rowK = k; applyRowK(); }
  }
  function endGrip() {
    window.removeEventListener("mousemove", onGripMove);
    window.removeEventListener("mouseup", onGripUp);
    if (!st.rdrag) return;
    st.rdrag = null;
    host.classList.remove("tl-dragging");
    delete host.dataset.tlDrag;
  }
  function onGripUp(e) {
    const moved = !!st.rdrag?.moved;
    endGrip();
    saveRowK();
    // A drag that ends outside the timeline must not count as a click on the viewer's backdrop.
    if (moved || !host.contains(e.target)) swallowClick();
  }
  function onGripDbl(e) {
    e.preventDefault();
    if (st.rowK === 1) return;
    st.rowK = 1;
    applyRowK();
    saveRowK();
  }
  function updateHeads() {
    for (const r of [...m.audio.filter((x) => x.kind === "track"), ...(m.lanes || [])]) { // the rows (0.1.39)
      const h = el.hrows.querySelector(`.tl-head[data-tl-row="${CSS.escape(r.id)}"]`);
      if (!h) continue;
      h.classList.toggle("off", !!r.muted);
      const name = h.querySelector(".tl-hname");
      const shown = tlRowName(r, opts.game);
      if (name.textContent !== shown) name.textContent = shown;
      const hint = typeof emRowHint === "function" ? emRowHint(m, r) : "";
      const tip = hint ? `${shown}\n${t(hint)}` : shown;
      if (name.title !== tip) name.title = tip;
      const tb = h.querySelector(".tl-hthru");
      if (tb) {
        const key2 = r.through ? "1" : "0";
        if (tb.dataset.tlState !== key2) {
          tb.dataset.tlState = key2;
          tb.innerHTML = tlSvg(tlPaths.through);
          tb.classList.toggle("on", !!r.through);
          tb.setAttribute("aria-pressed", String(!!r.through));
          const tip2 = r.through ? t("Plays through cuts. Click to cut it with the video") : t("Cut with the video. Click to let it play through cuts");
          tb.title = tip2;
          tb.setAttribute("aria-label", tip2);
        }
      }
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
    if (next.length) st.ovSel = null; // a piece picked: the overlay block is no longer the selection
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
  const rowOrder = () => ["video", ...m.audio.map((r) => r.id)]; // pieces: the video, the track rows' grouped sound, every clip
  /** True if segment `seg` of row `rowId` is a piece that can be picked (a file row only where its clip is). */
  function pieceExists(rowId, seg) {
    if (!(seg >= 0 && seg < m.removed.length) || !segShown(seg)) return false;
    if (rowId === "video") return true;
    const row = emRow(m, rowId);
    if (!row) return false;
    if (row.kind !== "file") return !segGap(seg); // the clip's own tracks have nothing in a gap (0.1.36)
    const s = emSegments(m)[seg];
    return Math.min(s.end, emRowEnd(m, row)) - Math.max(s.start, row.offset) > 1e-6;
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
    if (!(sec > 0 && sec < dur()) || gapAt(sec) >= 0) return false; // a gap has nothing to split
    const seg = emSegments(m)[emSegmentAt(m, sec)];
    return !!seg && sec - seg.start >= EM_MIN_SEG && seg.end - sec >= EM_MIN_SEG;
  };
  /** What "remove" would do to the selection: { restore, segs, ok }. Restores when the primary's segment is cut out, else cuts out; never cuts out every kept segment.
   *  A gap is not cut out (0.1.36: Delete closes it, see doRemoveOrClose). */
  /** True if row id `id` is an audio clip (0.1.39). */
  const isClip = (id) => !!id && id !== "video" && emRow(m, id)?.kind === "file";
  function removePlan() {
    // Pieces only: a selected audio clip is removed on its own (doRemove), not the piece under it.
    const segs = selSegs(st.sels.filter((s) => !isClip(s.row))).filter((i) => !segGap(i));
    if (!segs.length) return { restore: false, segs, ok: false };
    const restore = !!m.removed[segs[0]];
    const kept = m.removed.filter((r, i) => !r && !segGap(i)).length;
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
  /** Rows by role (0.1.44): what the editor says about them ({ mode, ready, offer }), or null without that API. */
  const rowsInfo = () => { try { return opts.rowsInfo?.() || null; } catch (e) { console.error("Glint timeline: rowsInfo failed", e); return null; } };
  /** The quiet line above the rows ("Simplify the audio rows"), and the "Every track" switch. Only touched when it changes. */
  function updateRowsBar() {
    const info = rowsInfo(), sw = el.hrows.querySelector(".tl-rmode");
    if (sw) {
      const show = !!info && info.ready;
      if (sw.hidden === show) sw.hidden = !show;
      const on = String(!!info && info.mode === "titles");
      if (sw.getAttribute("aria-checked") !== on) { sw.setAttribute("aria-checked", on); sw.classList.toggle("on", on === "true"); }
    }
    const o = info?.offer;
    const html = o ? `<span class="tl-rows-msg">${esc(t("{n} audio rows. By role it would be {k}.", { n: o.before, k: o.after }))}${o.mixes ? " " + esc(t("Mixes that doubled the sound are left out.")) : ""}</span>` +
      `<button type="button" class="btn sm" data-tl-act="rows-simplify">${esc(t("Simplify the audio rows"))}</button>` +
      `<button type="button" class="tl-btn tl-rows-x" data-tl-act="rows-dismiss" title="${esc(t("Not now"))}" aria-label="${esc(t("Not now"))}">${tlSvg("M6 6l12 12M18 6 6 18")}</button>` : "";
    if (html !== st.rowsBarHtml) { st.rowsBarHtml = html; el.rowsbar.innerHTML = html; el.rowsbar.hidden = !html; }
  }
  function updateBar() {
    updateRowsBar();
    const plan = removePlan(), removed = plan.restore;
    setBtn(el.bRemove, "remove", !plan.ok, tlSvg(removed ? tlPaths.restore : tlPaths.trash),
      removed ? keyTip(t("Restore segment"), "remove", t("Restore segment (Del)")) : keyTip(t("Delete segment"), "remove", t("Delete segment (Del)")));
    const ref = refAudio(), row = ref?.r || null;
    const gain = row ? levelOf(ref) ?? 1 : 1; // an audio clip: its own level (R1 #5)
    setBtn(el.bMute, "mute", !row, gain === 0 ? PICON.mute : PICON.vol,
      row && gain === 0 ? keyTip(t("Unmute segment"), "mute", t("Unmute segment (M)")) : keyTip(t("Mute segment"), "mute", t("Mute segment (M)")));
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
    if (el.bNames) { el.bNames.classList.toggle("on", st.names); el.bNames.setAttribute("aria-pressed", String(st.names)); }
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
    else syncSpacer();           // the result may have become longer or shorter (emEnd)
    updateBar();
    opts.onChange?.(what);
  }
  /* ----- overlay blocks (0.1.35) ----- */
  /** The overlay code's handle (C's P.ov) and the canvas's (P.cv), or null: menu entries whose function is missing are left out. */
  const ovApi = () => { try { return opts.ovApi?.() || null; } catch { return null; } };
  const cvApi = () => { try { return opts.cvApi?.() || null; } catch { return null; } };
  const isMedia = (o) => !!o && ["image", "gif", "video"].includes(o.kind);
  /** Select overlay block `id` (null: none): its pieces selection goes, and the overlay code is told. */
  function pickOv(id) {
    if (id && st.sels.length) setSels([], null);
    if (st.ovSel !== (id || null)) { st.ovSel = id || null; markAll(); }
    try { opts.onOverlayPick?.(id || null); } catch (e) { console.error("Glint timeline: onOverlayPick failed", e); }
  }
  /** Remove overlay item `id`: through the overlay code when it can (it takes the undo step), else here. */
  function doOvRemove(id) {
    const o = ovById(id);
    if (!o) return false;
    const api = ovApi();
    if (typeof api?.remove === "function") {
      try { api.remove(id); } catch (e) { console.error("Glint timeline: ov.remove failed", e); return false; }
      if (ovById(id)) return false;
    } else {
      emSnapshot(m);
      m.overlays.splice(m.overlays.indexOf(o), 1);
      changed("overlay");
    }
    if (st.ovSel === id) st.ovSel = null;
    ovChanged();
    return true;
  }
  /** Bring item `id` to the front (the end of m.overlays: drawn last) or send it to the back. One undo step. */
  function doOvOrder(id, front) {
    const L = ovItems(), i = L.findIndex((o) => o.id === id);
    if (i < 0 || (front ? i === L.length - 1 : i === 0)) return false;
    emSnapshot(m);
    const [o] = L.splice(i, 1);
    if (front) L.push(o); else L.unshift(o);
    ovChanged();
    changed("overlay");
    return true;
  }
  /** The overlay items changed (here or in the overlay code): rows again if their number changed, redraw. */
  function ovChanged() {
    st.ovSigShown = ovSig();
    if (rowSig() !== st.rowsSig) buildRows();
    if (st.ovSel && !ovById(st.ovSel)) st.ovSel = null;
    markAll();
    syncSpacer();
  }
  // Each do... below takes one undo snapshot, changes the model, then calls changed(). They return true if they did something.
  function doSplit(at = st.shownT, only) {
    // 0.1.41 (as Vegas): Split cuts ONLY what is selected, at `at`: a selected V1 piece (or a piece of its own sound on a
    // track row) with its grouped sound, a selected audio clip, a selected group. Nothing selected: the V1 piece under
    // `at` and its grouped sound. Never an added song, an ungrouped sound or an overlay item that is not selected.
    // `only` (the blade): { video: true } the V1 piece under the pointer, { clip: id } that audio clip.
    const under = (r) => !!r && at > r.offset + EM_MIN_SEG - 1e-9 && at < r.offset + r.length - EM_MIN_SEG + 1e-9;
    let piece = false;
    const clips = [];
    if (only) { if (only.clip) clips.push(only.clip); else piece = true; }
    else if (!st.sels.length && st.ovSel) return doOvSplit(st.ovSel, at); // a selected overlay item: that one
    else if (!st.sels.length) piece = true;
    else {
      const segAll = emSegments(m);
      for (const s of st.sels) {
        if (isClip(s.row)) { clips.push(s.row); continue; }
        const sg = segAll[s.seg];
        if (sg && at > sg.start + 1e-6 && at < sg.end - 1e-6) piece = true; // the V1 piece, or its own sound on a track row
      }
    }
    piece = piece && canSplitAt(at);
    const cl = [...new Set(clips)].filter((id) => under(emRow(m, id)));
    if (!piece && !cl.length) return false;
    const P = piece ? emParts(m)[emPartAt(m, at)] : null, g = P && m.parts ? m.parts[P.i]?.group : null;
    if (cl.length || g || !piece) return doSplitClips(cl, at, piece);
    emSnapshot(m);
    const c = emSplit(m, at);
    if (c < 0) throw new Error(`timeline: emSplit refused ${at} although canSplitAt allowed it`);
    shiftSels(c, 1);
    changed("split");
    return true;
  }
  /* ----- audio clips and groups (0.1.39) ----- */
  /** Split audio clips `ids` (and the piece under `sec` when `piece`), with every member of their groups that lies under
   *  `sec`: the left halves stay one group, the right halves become another. One undo step. */
  function doSplitClips(ids, sec, piece) {
    const groups = new Set(), clipIds = new Set(ids);
    for (const id of ids) { const r = emRow(m, id); if (r?.group) groups.add(r.group); }
    if (piece && m.parts && emPartAt(m, sec) >= 0) { const g = m.parts[emPartAt(m, sec)]?.group; if (g) groups.add(g); }
    return layout(() => {
      let any = false;
      if (piece) any = emSplitAt(m, sec) || any;
      for (const g of groups) {
        for (const r of m.audio) if (r.kind === "file" && r.group === g) clipIds.add(r.id);
        emParts(m).forEach((p) => { if (m.parts[p.i]?.group === g && sec > p.at + EM_MIN_SEG && sec < p.end - EM_MIN_SEG) any = emSplitAt(m, sec) || any; });
      }
      for (const id of clipIds) any = emSplitClip(m, id, sec) || any;
      // The right halves of each group: a group of their own.
      for (const g of groups) {
        const g2 = `g${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
        for (const r of m.audio) if (r.kind === "file" && r.group === g && r.offset >= sec - 1e-6) r.group = g2;
        emParts(m).forEach((p) => { if (m.parts[p.i]?.group === g && p.at >= sec - 1e-6) m.parts[p.i].group = g2; });
      }
      emTidyGroups(m);
      return any;
    }) && (relaid("split"), true);
  }
  /** Move every member of group `g` by `dt` seconds (limited as a whole: nothing before 0), but `skip` (a part object
   *  already moved). Pieces go by free placement, the rightmost first when moving later. */
  function groupMove(g, dt, skip) {
    const clips = m.audio.filter((r) => r.kind === "file" && r.group === g);
    const parts = emParts(m).filter((p) => m.parts?.[p.i]?.group === g && m.parts[p.i] !== skip);
    dt = Math.max(dt, -Math.min(Infinity, ...clips.map((r) => r.offset), ...parts.map((p) => p.at)));
    // A piece moved left stops at the clip before it (R1 #3), and the whole group with it, so it stays in sync.
    if (dt < 0) for (const p of parts) dt = Math.max(dt, freeFrom(p.i, p.at + dt) - p.at);
    if (!(Math.abs(dt) > 1e-9)) return;
    for (const r of clips) r.offset += dt;
    const objs = parts.map((p) => ({ o: m.parts[p.i], at: p.at })).sort((a, b) => (dt > 0 ? b.at - a.at : a.at - b.at));
    for (const { o, at } of objs) { const i = m.parts.indexOf(o); if (i >= 0) emPlacePart(m, i, at + dt); }
  }
  /** Free placement of piece `i` (display index) at timeline second `from`, moved LEFT: a start that would land inside
   *  a clip lying before where it was goes to that clip's end instead (R1 #3: a 1 s nudge onto the neighbour put it
   *  "beside" and left a piece-long gap). Moved right, or onto empty space, `from` as it is. */
  function freeFrom(i, from) {
    const P = emParts(m), me = P[i];
    if (!me || from >= me.at) return from;
    let out = from;
    for (const q of P) if (q.i !== i && !q.gap && out > q.at + 1e-6 && out < q.end - 1e-6 && q.end <= me.at + 1e-6) out = q.end;
    return out;
  }
  /** The latest a dragged piece `i` of group `grp` may start when moved left to `from`: the group stops as a whole. */
  function groupFrom(i, grp, from) {
    const P = emParts(m), me = P[i];
    if (!me) return from;
    let dt = freeFrom(i, from) - me.at;
    if (grp && dt < 0) for (const p of P) if (p.i !== i && m.parts?.[p.i]?.group === grp) dt = Math.max(dt, freeFrom(p.i, p.at + dt) - p.at);
    return me.at + dt;
  }
  /* ----- moving a selection together (0.1.41) -----
   * Everything selected together moves together when one of it is dragged (or with Alt+Left / Alt+Right), as one
   * temporary group: the selected V1 pieces (a piece of a track row stands for its V1 piece) and audio clips, with the
   * members of their own groups. The group machinery does the moving (live, stopping together at a limit, one undo
   * step). Each member keeps its own group in `_g0` meanwhile; tmpGroupOff puts it back, and the undo step is
   * rewritten so it never holds the temporary group. */
  function tmpGroupOn(sels) {
    const segAll = emSegments(m), things = [];
    for (const s of sels || []) {
      if (isClip(s.row)) { const r = emRow(m, s.row); if (r) things.push(r); continue; }
      const sg = segAll[s.seg];
      if (!sg || segGap(s.seg)) continue;
      emEnsureParts(m, opts.title);
      const p = m.parts[emPartAt(m, (sg.start + sg.end) / 2)];
      if (p && !p.gap) things.push(p);
    }
    const real = new Set(things.map((x) => x.group).filter(Boolean));
    for (const x of [...(m.parts || []), ...m.audio.filter((r) => r.kind === "file")]) if (x.group && real.has(x.group)) things.push(x);
    const list = [...new Set(things)];
    if (list.length < 2) return null;
    const id = `tmp${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
    for (const x of list) { x._g0 = x.group || ""; x.group = id; }
    return id;
  }
  function tmpGroupOff() {
    for (const x of [...(m.parts || []), ...m.audio]) {
      if (!("_g0" in x)) continue;
      if (x._g0) x.group = x._g0; else delete x.group;
      delete x._g0;
    }
  }
  /** What a temporary group is made of, as a selection now (pieces by where they are, clips by their row). */
  function tmpSels() {
    // Its members carry `_g0` (the group itself may have been tidied away when it had one member left).
    const out = [];
    for (const p of emParts(m)) if (m.parts[p.i] && "_g0" in m.parts[p.i]) out.push({ row: "video", seg: emSegmentAt(m, (p.at + p.end) / 2) });
    for (const r of m.audio) if (r.kind === "file" && "_g0" in r) out.push(clipSel(r));
    return out;
  }
  /** After a drag (or a nudge) with a temporary group: the groups as they were, one clean undo step, the selection kept. */
  function tmpDone(d) {
    const tp = d?.tmp;
    if (!tp) return;
    d.tmp = null;
    const keep = d.moved ? tmpSels() : null;
    tmpGroupOff();
    if (m.undo.length > tp.n0 && m.undo[tp.n0] === tp.preTemp) m.undo[tp.n0] = tp.pre;
    if (emState(m) === tp.pre) { m.undo.length = Math.min(m.undo.length, tp.n0); }
    if (!d.moved && emState(m) !== tp.pre) emRestore(m, tp.pre); // a click: the parts a single clip was given go again
    if (keep && keep.length) setSels(keep, keep[0]);
    markAll();
  }
  /** Alt+Left / Alt+Right (editor.js): move the selection by `dt` seconds, together, one undo step. */
  function nudgeSel(dt) {
    if (!st.sels.length || !Number.isFinite(dt) || !dt) return false;
    const pre = emState(m), n0 = m.undo.length;
    const one = st.sels.length === 1 ? st.sels[0] : null;
    let id = tmpGroupOn(st.sels);
    if (!id) {
      // One thing: a clip moves by itself; a piece through a group of one (the same free placement).
      if (one && isClip(one.row)) {
        const r = emRow(m, one.row);
        emSnapshot(m);
        emPlaceFile(m, r.id, { offset: Math.max(0, r.offset + dt) });
        if (emState(m) === pre) { m.undo.length = n0; return false; }
        setSels([clipSel(r)], clipSel(r));
        changed("file");
        return true;
      }
      if (!one || one.row !== "video" || segGap(one.seg)) return false;
      emEnsureParts(m, opts.title);
      const sg = emSegments(m)[one.seg], p = sg && m.parts[emPartAt(m, (sg.start + sg.end) / 2)];
      if (!p || p.gap) { if (emState(m) !== pre) emRestore(m, pre); return false; }
      id = `tmp${Date.now().toString(36)}`;
      p._g0 = p.group || "";
      p.group = id;
    }
    const d = { moved: true, tmp: { id, pre, preTemp: emState(m), n0 } };
    emSnapshot(m);
    groupMove(id, dt);
    tmpDone(d);
    if (emState(m) === pre) return false;
    changed("order");
    return true;
  }
  /** The selection pieces of every member of `s`'s group (just `s` when it has none). */
  function groupSels(s) {
    if (!s) return [];
    const clip = isClip(s.row);
    if (!clip && (s.row !== "video" || !m.parts)) return [s];
    const own = clip ? emRow(m, s.row).group : m.parts[emPartAt(m, (emSegments(m)[s.seg].start + emSegments(m)[s.seg].end) / 2)]?.group;
    if (!own) return [s];
    return emGroupOf(m, clip ? { clip: s.row } : { row: "video", seg: s.seg }).map((it) => (it.clip ? clipSel(emRow(m, it.clip)) : it));
  }
  /** Delete: the selected audio clips leave alone; pieces as before; a group goes as a whole. */
  function doRemoveAll() {
    const sels = tlSelMerge([], st.sels.flatMap(groupSels));
    const clips = sels.filter((s) => isClip(s.row)).map((s) => s.row);
    if (!clips.length) return doRemove();
    const segAll = emSegments(m), times = [...new Set(sels.filter((s) => !isClip(s.row) && !segGap(s.seg)).map((s) => (segAll[s.seg].start + segAll[s.seg].end) / 2))].sort((a, b) => b - a);
    const ok = layout(() => {
      let any = false;
      for (const id of clips) any = emRemoveClip(m, id) || any;
      for (const at of times) any = emRemovePiece(m, emPieceAt(m, at), false) || any;
      return any;
    });
    if (ok) relaid("remove");
    return ok;
  }
  /** U: the selected pieces' sound becomes audio clips; selected members of a group leave it. */
  function doUngroup() {
    if (!st.sels.length) return false;
    const sels = st.sels.map((s) => ({ ...s }));
    const ok = layout(() => { emUngroup(m, sels); return true; });
    if (ok) relaid("ungroup");
    return ok;
  }
  /** G: the selected pieces and audio clips become one group. */
  function doGroup() {
    const items = [];
    for (const s of st.sels) { if (s.row === "video") items.push({ row: "video", seg: s.seg }); else if (isClip(s.row)) items.push({ clip: s.row }); }
    if (items.length < 2) return false;
    const ok = layout(() => !!emGroup(m, items));
    if (ok) relaid("group");
    return ok;
  }
  function doJoin(c) {
    if (c < 0 || c >= m.cuts.length || lockedCut(c)) return false;
    emSnapshot(m);
    emJoin(m, c);
    shiftSels(c, -1);
    changed("join");
    return true;
  }
  /** Cut out the selected segments, or restore them (see removePlan). */
  function doRemove() {
    // 0.1.37: the selected pieces really leave the row and what follows moves left (emRemovePiece).
    const plan = removePlan();
    if (!plan.ok || plan.restore) return false;
    const segAll = emSegments(m), times = plan.segs.map((i) => (segAll[i].start + segAll[i].end) / 2).sort((a, b) => b - a);
    const ok = layout(() => { let any = false; for (const at of times) any = emRemovePiece(m, emPieceAt(m, at), false) || any; return any; });
    if (ok) relaid("remove");
    return ok;
  }
  /* ----- gaps and free placement on the main video row (0.1.36) ----- */
  /** The row was laid out anew (gaps, clips moved): the old segment indexes mean nothing now, the selection goes
   *  (or becomes the segments of `keep`, timeline seconds of pieces to select). */
  function relaid(what, keep) {
    st.sels = (keep || []).map((at) => ({ row: "video", seg: emSegmentAt(m, at) }));
    st.sel = st.sels[0] || null;
    st.selNotify = true;
    st.hold = null;
    changed(what);
    if (marks().length) opts.onMarks?.(marks().slice()); // bookmarks go with their clip
  }
  /** Run model change `fn` as one undo step; false (and no step) if it changed nothing. */
  function layout(fn) {
    const before = emState(m);
    emSnapshot(m);
    if (!fn() || emState(m) === before) { if (m.undo[m.undo.length - 1] === before) m.undo.pop(); return false; }
    return true;
  }
  /** Display indexes of the selected gaps. */
  const selGaps = () => [...new Set(st.sels.filter((s) => s.row === "video" && segGap(s.seg)).map((s) => { const sg = emSegments(m)[s.seg]; return gapAt((sg.start + sg.end) / 2); }))].filter((i) => i >= 0);
  /** Close the selected gaps (or `list`): what follows moves earlier. */
  function doCloseGaps(list = selGaps()) {
    if (!list.length) return false;
    const ok = layout(() => { let any = false; for (const i of list.slice().sort((a, b) => b - a)) any = emCloseGap(m, i) || any; return any; });
    if (ok) relaid("gap");
    return ok;
  }
  function doCloseAllGaps() {
    const ok = layout(() => emCloseGaps(m));
    if (ok) relaid("gap");
    return ok;
  }
  /** "Cut out and leave a gap": the selected video pieces (or the primary's segment) leave an empty stretch. */
  function doLift() {
    const segs = selSegs().filter((i) => !segGap(i));
    if (!segs.length) return false;
    const segAll = emSegments(m), times = segs.map((i) => (segAll[i].start + segAll[i].end) / 2).sort((a, b) => b - a);
    const ok = layout(() => { let any = false; for (const at of times) any = emLiftSeg(m, emSegmentAt(m, at), opts.title) || any; return any; });
    if (ok) relaid("gap", times.filter((at) => gapAt(at) >= 0).slice(0, 1));
    return ok;
  }
  /** Drop a dragged clip (`c` = { i, at0, len }) where its ghost is: beside another clip, or freely at ghost.from. */
  function doPlaceClip(c, g) {
    let at = null;
    const obj = m.parts ? m.parts[c.i] : null, grp = obj?.group; // a piece in a group takes its group along (0.1.39)
    const ok = layout(() => {
      const from = g.mode === "beside" ? g.from : groupFrom(c.i, grp, g.from); // R1 #3: not onto the clip before it
      const done = g.mode === "beside" ? emPutBeside(m, c.i, g.k, g.after) : Math.abs(from - c.at0) > 1e-6 && emPlacePart(m, c.i, from, opts.title);
      if (done && grp) { const p = emParts(m).find((q) => m.parts[q.i] === obj); if (p) groupMove(grp, p.at - c.at0, obj); }
      return done;
    });
    if (!ok) return false;
    // The moved clip stays selected: found again by its part object (the model keeps it through a new layout; R1 #6:
    // matching by length picked the first piece of the same length, and Delete then removed that one).
    const P = emParts(m), q = obj ? P.find((p) => m.parts?.[p.i] === obj) : null;
    if (q) at = q.at + Math.min(q.len / 2, 0.05);
    else if (g.mode === "free") at = g.from + Math.min(c.len / 2, 0.05);
    else at = null;
    relaid("order", at === null ? [] : [at]);
    return true;
  }
  /** The piece end under lane x `x` (0.1.37): { i, side: "start" | "end" }, or null. Inside a piece its own ends;
   *  in a gap (not at its right edge, which resizes the gap) or after the end, a piece end right beside it. */
  function pieceEndHit(x) {
    const sec = timeAtX(x), L = emPieceList(m);
    const inside = L.find((p) => !p.gap && sec >= p.at && sec < p.at + p.len);
    if (inside) {
      const edge = Math.min(tlEdgePx, (inside.len * st.pps) / 3);
      if (x - X(inside.at) <= edge) return { i: inside.i, side: "start" };
      if (X(inside.at + inside.len) - x <= edge) return { i: inside.i, side: "end" };
      return null;
    }
    if (gapHitRight(x) >= 0) return null;
    for (const p of L) {
      if (p.gap) continue;
      if (Math.abs(X(p.at + p.len) - x) <= tlEdgePx) return { i: p.i, side: "end" };
      if (Math.abs(X(p.at) - x) <= tlEdgePx) return { i: p.i, side: "start" };
    }
    return null;
  }
  /** Put piece `i` on the picture at timeline second `at` (0.1.37): the media code makes a clip item covering the
   *  picture and its sound row (opts.pieceUp, inside this undo step), and the piece leaves the row with a gap. */
  function doPieceUp(i, at) {
    // A piece with its own speed (0.1.41) stays on V1: a picture item plays its footage at 1x.
    if (emPieceSpeed(m, i) !== 1) { showTip(at, t("A clip with its own speed stays on the video row. Set it to 1x first.")); return false; }
    const info = emPieceToItem(m, i);
    if (!info || typeof opts.pieceUp !== "function") return false;
    const n0 = m.undo.length;
    emSnapshot(m);
    let id = null;
    try { id = opts.pieceUp({ ...info, at: Math.max(0, at) }, m); } catch (e) { console.error("Glint timeline: pieceUp failed", e); }
    if (!id) { if (m.undo.length > n0) emRestore(m, m.undo.pop()); return false; }
    emRemovePiece(m, i, true); // the last piece with footage stays (then the item is a copy of it)
    ovChanged();
    relaid("overlay");
    return true;
  }
  /** The gap whose right edge is under lane x `x` (within the grab zone), or -1. */
  function gapHitRight(x) {
    if (!m.parts) return -1;
    for (const p of emParts(m)) if (p.gap && Math.abs(X(p.end) - x) <= tlEdgePx) return p.i;
    return -1;
  }
  /** The blade on an overlay block: the item splits in two at `sec` (one undo step). */
  function doOvSplit(id, sec) {
    const o = ovById(id);
    if (!o) return false;
    const n0 = m.undo.length;
    emSnapshot(m);
    const nid = `o${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
    const b = emSplitOverlay(o, sec, nid);
    if (!b) { if (m.undo.length > n0) m.undo.pop(); showTip(sec, t("Cannot split here")); return false; }
    const L = ovItems();
    L.splice(L.indexOf(o) + 1, 0, b); // right above its first half
    ovChanged();
    changed("overlay");
    return true;
  }
  /** Delete: a selected gap closes, anything else is cut out (or restored) as before. */
  const doRemoveOrClose = () => (selGaps().length && st.sel && segGap(st.sel.seg) ? doCloseGaps() : doRemoveAll());
  /** Put back the removed run `j` (a tlJoints entry): "Restore the part cut out here". */
  function doRestoreRun(j) {
    if (!j) return false;
    emSnapshot(m);
    for (let i = j.first; i <= j.last; i++) emSetRemoved(m, i, false);
    changed("restore");
    return true;
  }
  /** Move clip block `from` to display index `to`: one undo step. The selection stays on the same pictures;
   *  the editor keeps the playhead on its picture (onChange "order"). */
  function doMovePart(from, to) {
    const n = hasParts() ? m.parts.length : 0;
    if (!(from >= 0 && from < n && to >= 0 && to < n) || from === to) return false;
    const T = selTimes(), src = (s) => s && { row: s.row, at: emToSrc(m, s.at) };
    const S = { list: T.list.map(src), prim: src(T.prim) };
    emSnapshot(m);
    if (!emMovePart(m, from, to)) { m.undo.pop(); return false; }
    const back = (s) => s && { row: s.row, at: emFromSrc(m, s.at) };
    remapSels({ list: S.list.map(back), prim: back(S.prim) });
    st.hold = null;
    changed("order");
    if (marks().length) opts.onMarks?.(marks().slice()); // they moved with their clip
    return true;
  }
  /** Cut clip `i` out (all of it), or put it back. Never cuts out the last kept clip. */
  function doPartRemoved(i, removed) {
    const segs = emPartSegs(m, i);
    if (!segs.length || segs.every((k) => !!m.removed[k] === !!removed)) return false;
    const T = selTimes();
    emSnapshot(m);
    if (!emSetPartRemoved(m, i, removed)) { m.undo.pop(); return false; }
    remapSels(T);
    changed(removed ? "remove" : "restore");
    return true;
  }
  /** Display index of the clip block under lane x `x`, or -1. */
  function partAtX(x) {
    const v = vAtX(x);
    if (!showStrip() || !(v >= 0 && v <= vdur())) return -1;
    const i = emPartAt(m, toS(Math.min(v, vdur() - 1e-6)));
    return emParts(m)[i]?.gap ? -1 : i; // a gap is no clip block
  }
  /* Trimming splits and merges segments anywhere before (or after) the edge, so the selection is kept as
     times (the middle of each piece's segment) and found again afterwards. */
  function selTimes() {
    const segs = emSegments(m), mid = (s) => (segs[s.seg] ? (segs[s.seg].start + segs[s.seg].end) / 2 : -1);
    return { list: st.sels.map((s) => ({ row: s.row, at: mid(s) })).filter((s) => s.at >= 0), prim: st.sel && mid(st.sel) >= 0 ? { row: st.sel.row, at: mid(st.sel) } : null };
  }
  function remapSels(T) {
    const back = (s) => ({ row: s.row, seg: emSegmentAt(m, s.at) });
    st.sels = tlSelMerge([], T.list.map(back));
    st.sel = T.prim ? back(T.prim) : null;
    st.selNotify = true; // changed() tells the editor
  }
  /** Apply a trim of `side` ("start" | "end") to `sec`, refused (null) when it would leave less than EM_MIN_SEG. */
  const trimTo = (side, sec) => (side === "start" ? emSetTrimStart(m, sec, true) : emSetTrimEnd(m, sec, true));
  /** True if trimming `side` to `sec` would change anything (for the menu: tried, then put back). */
  function trimWould(side, sec) {
    const before = emState(m), marks0 = (m.marks || []).slice();
    // A trim the model refuses by throwing (R1 #7: near the very start or end) is no trim: the menu still opens.
    let ok = false;
    try { ok = trimTo(side, sec) !== null && emState(m) !== before; } catch (e) { console.warn("Glint timeline: trim refused", e); ok = false; }
    emRestore(m, before);
    if (m.marks) m.marks = marks0;
    return ok;
  }
  /** Trim the start (or end) to source time `sec`: I and O, and the menu. One undo step. */
  function doTrimAt(side, sec) {
    if (!Number.isFinite(sec) || !trimWould(side, sec)) return false;
    const T = selTimes();
    emSnapshot(m);
    trimTo(side, sec);
    remapSels(T);
    changed("trim");
    return true;
  }
  /** Set the level of every selected audio piece: `gain(oldGain)` -> new gain. */
  function setGains(what, gain) {
    const list = selAudio().map((s) => ({ ...s, g: gain(levelOf(s)) }));
    if (!list.some((s) => Math.abs(s.g - levelOf(s)) > 1e-9)) return false; // nothing would change: no empty undo step
    emSnapshot(m);
    for (const s of list) {
      if (!isClip(s.row)) { emSetGain(m, s.r.id, s.seg, s.g); continue; }
      // An audio clip's mute and level are its own (R1 #5): they cover the whole clip and go where it goes.
      if (s.g <= 0.005) s.r.muted = true;
      else if (what === "mute" && s.r.muted && s.r.volume > 0.005) s.r.muted = false; // unmuted: the level it had
      else { s.r.muted = false; s.r.volume = Math.min(EM_MAX_GAIN, s.g); }
    }
    changed(what);
    return true;
  }
  /** The level of a selected audio piece: a track row's piece its segment's, an audio clip its own (R1 #5). */
  function levelOf(s) { return isClip(s.row) ? (s.r.muted ? 0 : s.r.volume) : s.r.gains[s.seg]; }
  /** Mute the selected audio pieces, or unmute them all if the shown one (refAudio) is muted. */
  function doMuteSeg() {
    const ref = refAudio();
    if (!ref) return false;
    const on = levelOf(ref) > 0;
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
    // Among the clip's own track rows (0.1.41): an audio clip is not tied to V1's pieces (emSolo).
    for (const s of selAudio()) { if (isClip(s.row)) continue; if (!groups.has(s.seg)) groups.set(s.seg, []); groups.get(s.seg).push(s.r.id); }
    // A segment where every row is selected cannot be soloed.
    const nT = m.audio.filter((r) => r.kind === "track").length;
    return [...groups].filter(([, ids]) => ids.length < nT);
  }
  const tracksOnly = () => m.audio.filter((r) => r.kind === "track");
  const groupSolo = (seg, ids) => (ids.length === 1 ? emIsSolo(m, ids[0], seg) : tracksOnly().every((r) => (ids.includes(r.id) ? r.gains[seg] > 0 : r.gains[seg] === 0)));
  const allSolo = () => { const g = soloGroups(); return g.length > 0 && g.every(([seg, ids]) => groupSolo(seg, ids)); };
  function doSolo() {
    const groups = soloGroups();
    if (!groups.length || tracksOnly().length < 2) return false;
    const undo = groups.every(([seg, ids]) => groupSolo(seg, ids));
    emSnapshot(m);
    for (const [seg, ids] of groups) {
      if (groupSolo(seg, ids) !== undo) continue;
      if (ids.length === 1) { emSolo(m, ids[0], seg); continue; }
      for (const r of tracksOnly()) {
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
  /** The selected pieces on V1 with footage (display indexes; pieces form: one segment per piece), 0.1.41. */
  function speedPieces() {
    const L = emPieceList(m);
    return [...new Set(st.sels.filter((s) => s.row === "video").map((s) => (m.parts ? emPartAt(m, (emSegments(m)[s.seg]?.start ?? -1) + 1e-6) : 0)))]
      .filter((i) => L[i] && !L[i].gap);
  }
  /** Speed per piece (0.1.41): every selected piece plays at `v` (one undo step). Later indexes first is not needed:
   *  a speed change never splits or merges pieces, so the indexes stay. */
  function doSpeed(v) {
    const list = speedPieces();
    if (!list.length || !Number.isFinite(v)) return false;
    const base = emState(m);
    emSnapshot(m);
    let any = false;
    for (const i of list) any = emSetPieceSpeed(m, i, v) || any;
    if (!any) { if (m.undo[m.undo.length - 1] === base && emState(m) === base) m.undo.pop(); return false; }
    changed("speed");
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
  /** Show or hide the name written on each audio row's block. */
  function setNames(on) {
    st.names = !!on;
    try { localStorage.setItem("glint.tl.names", st.names ? "1" : "0"); } catch {}
    if (el.bNames) { el.bNames.classList.toggle("on", st.names); el.bNames.setAttribute("aria-pressed", String(st.names)); }
    markAll();
    return true;
  }
  /** The pointer's tool (0.1.36): "select" (pick, move, trim) or "blade" (a click splits the video or an overlay there). */
  function setTool(name) {
    name = name === "blade" ? "blade" : "select";
    if (st.drag) return false;
    const changedTool = st.tool !== name;
    st.tool = name;
    el.lanes.dataset.tlTool = name;
    el.bSelect.setAttribute("aria-pressed", String(name === "select"));
    el.bBlade.setAttribute("aria-pressed", String(name === "blade"));
    return changedTool;
  }
  function setHideCuts(on) {
    on = !!on;
    if (on) return false; // 0.1.37: nothing is left on the row to hide
    if (on === st.hideCuts || st.drag) return false;
    const x = st.laneW ? X(st.shownT) : 0; // keep the playhead where it is on screen
    st.hideCuts = on;
    try { localStorage.setItem("glint.tl.hideCuts", on ? "1" : "0"); } catch {}
    markAll();
    relayout();
    if (st.laneW && !st.fitted) setScroll(tlPad + toV(st.shownT) * st.pps - x);
    closeMenu();
    trimTips();
    updateBar();
    return true;
  }

  /* ----- bookmarks ----- */
  // m.marks (editmodel.js): marked moments, only there to find a spot again. Changing them takes
  // no undo snapshot and does not call onChange (the edit is the same); onMarks gets the new list.
  const marks = () => m.marks || [];
  /** A bookmark is drawn (and can be picked) unless it lies inside a cut-out part while cut parts are hidden. One on the edge of a kept part stays. */
  const markShown = (sec) => !st.hideCuts || !(m.removed[emSegmentAt(m, sec - 1e-4)] && m.removed[emSegmentAt(m, sec + 1e-4)]);
  const shownMarks = () => marks().filter(markShown);
  /** Index in m.marks of the shown bookmark nearest lane x `x`, within tlSnapPx; -1 if none. */
  function markAtX(x) {
    let best = -1, dist = tlSnapPx;
    marks().forEach((sec, i) => {
      if (!markShown(sec)) return;
      const d = Math.abs(X(sec) - x);
      if (d <= dist) { dist = d; best = i; }
    });
    return best;
  }
  function marksChanged() {
    markAll(); // the flags are placed with the next draw
    opts.onMarks?.(marks().slice());
    return true;
  }
  const doAddMark = (sec) => emAddMark(m, sec) >= 0 && marksChanged();
  const doRemoveMark = (i) => emRemoveMark(m, i) && marksChanged();
  function doClearMarks() {
    if (!marks().length) return false;
    m.marks = [];
    return marksChanged();
  }
  /** The playhead exactly onto `sec` (even inside a cut-out part): the editor's seek otherwise moves it out of one. */
  function seekExact(sec) {
    st.hold = { at: sec, until: performance.now() + 500 };
    st.shownT = sec;
    opts.onSeek?.(sec, true);
  }
  /** To the next (or previous) shown bookmark from the playhead. False if there is none. */
  function doGoMark(prev) {
    const now = +opts.getTime() || 0, vis = { marks: shownMarks() };
    const at = prev ? emPrevMark(vis, now) : emNextMark(vis, now);
    if (at === null) return false;
    seekExact(at);
    return true;
  }

  /* ----- colors ----- */
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
    // The free space after the result's end (emEnd) is dimmed; the ticks run on through it (the playhead may stand there).
    const end = XV(vEnd());
    if (end < w) { ctx.fillStyle = C.bg; ctx.globalAlpha = 0.6; ctx.fillRect(Math.max(0, end), 0, w - Math.max(0, end), h); ctx.globalAlpha = 1; }
    const x0 = XV(0);
    if (x0 > 0) { ctx.fillStyle = C.bg; ctx.globalAlpha = 0.6; ctx.fillRect(0, 0, x0, h); ctx.globalAlpha = 1; }
    const { major, minor } = tlPickStep(st.pps);
    const t0 = Math.max(0, vAtX(-80)), t1 = Math.min(vspan(), vAtX(w + 4));
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
  /** The removed runs (tlJoints) of the model as it is now, but for the leading and trailing ones: those are the trim, shown by the brackets. */
  const joints = () => tlJoints(m.removed, [0, ...m.cuts, dur()]).filter((j) => j.first > 0 && j.last < m.removed.length - 1);

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
      if (lockedCut(i)) return; // a part border: drawn below, stronger
      if (st.hideCuts && (m.removed[i] || m.removed[i + 1])) return;
      const x = X(c);
      if (x > -2 && x < st.laneW + 2) ctx.fillRect(Math.round(x) - 1, y, 2, bh);
    });
    // Part borders ("Edit together"): a wider gap with a light line in it, on every row.
    if (hasParts()) {
      for (const p of emParts(m).slice(1)) {
        if (p.at <= a || p.at >= b) continue;
        const x = Math.round(X(p.at));
        if (x < -4 || x > st.laneW + 4) continue;
        ctx.fillStyle = C.bg;
        ctx.fillRect(x - 2, y, 4, bh);
        ctx.globalAlpha = 0.7;
        ctx.fillStyle = C.text;
        ctx.fillRect(x - 0.5, y, 1, bh);
        ctx.globalAlpha = 1;
      }
    }
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
    // 0.1.41: an audio clip is outlined whole (before, only the segment of the video under where it was clicked: for a
    // song that runs past the video that was a sliver at the end of the last piece, so the click seemed to miss).
    for (const [lo, hi] of tlSelSpans(st.sels, rowId, isClip(rowId), emSegments(m), a, b)) {
      const sp = span(lo, hi, 4);
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
    // The frames are spread over the SOURCE: with parts, each block draws its own tiles (clipped to it) from its own frames.
    const strip = (from, to, part) => {
      const n = st.thumbs.length, tw = Math.max(24, (bh * 16) / 9);
      for (let k = Math.floor((from + st.scrollX - tlPad) / tw); ; k++) {
        const sx = tlPad + k * tw - st.scrollX;
        if (sx >= to) break;
        let mid = toS(((k + 0.5) * tw) / st.pps);
        if (part) mid = emToSrc(m, Math.min(part.end - 1e-6, Math.max(part.at, mid)));
        const img = nearestThumb(Math.min(n - 1, Math.max(0, Math.floor((mid / srcLen()) * n))));
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
    };
    if (ok && dur() > 0) {
      const from = Math.max(0, bx0), to = Math.min(st.laneW, bx1);
      if (!hasParts()) strip(from, to, null);
      else {
        for (const p of emParts(m)) {
          if (p.gap) continue;
          const a = Math.max(from, X(p.at)), b = Math.min(to, X(p.end));
          if (b - a < 1) continue;
          ctx.save();
          ctx.beginPath();
          ctx.rect(a, y, b - a, bh);
          ctx.clip();
          strip(a, b, p);
          ctx.restore();
        }
      }
    }
    pictureMarks(ctx, C, y, bh);
    overlays(ctx, C, 0, dur(), y, bh, "video");
    ctx.restore();
    ctx.lineWidth = 1;
    ctx.strokeStyle = C["line-2"];
    rr(ctx, bx0 + 0.5, y + 0.5, bx1 - bx0 - 1, bh - 1, 8);
    ctx.stroke();
    drawGaps(ctx, C, y, bh, true);
    selOutline(ctx, C, 0, dur(), y, bh, "video");
    // Each piece (0.1.37): its name at its visible left end and a grip at each end (they trim and restore).
    for (const p of emPieceList(m)) {
      if (p.gap) continue;
      const a = Math.max(bx0, X(p.at)), b = Math.min(bx1, X(p.at + p.len));
      if (b - a < 2) continue;
      // A project's clip that is missing (0.1.38): its pieces are black and say so.
      const gone = typeof opts.missingAt === "function" ? opts.missingAt(p.src + p.flen / 2) : null;
      if (gone) {
        ctx.globalAlpha = 0.9;
        ctx.fillStyle = "#000";
        rr(ctx, a, y, b - a, bh, 6);
        ctx.fill();
        ctx.globalAlpha = 1;
        label(ctx, C, "eyeOff", t("Clip missing: {title}", { title: gone }), a, b, y + 5);
        continue;
      }
      label(ctx, C, "film", p.title || opts.title || t("Video"), a, b, y + 5);
      if (m.parts?.[p.i]?.group && b - a > 16) groupMark(ctx, C, a + 5, y + bh - 13); // in a group (0.1.39)
      if (p.speed !== 1 && b - a > 30) speedMark(ctx, C, tlSpeedText(p.speed), b - 6, y + bh - 7); // its own speed (0.1.41)
      if (b - a < 18) continue;
      ctx.fillStyle = C.text;
      ctx.globalAlpha = 0.75;
      for (const ex of [X(p.at) + 3, X(p.at + p.len) - 5]) if (ex > -4 && ex < st.laneW + 4) { rr(ctx, ex, y + bh / 2 - 9, 2, 18, 1); ctx.fill(); }
      ctx.globalAlpha = 1;
    }
    drawGhost(ctx, C, y, bh);
    drawZoomKeys(ctx, C);
    drawTail(ctx, C, y, bh);
  }
  /** The source's length (the film strip's frames are spread over it). */
  const srcLen = () => (Number.isFinite(m.source) ? m.source : hasParts() ? Math.max(1e-6, ...emParts(m).filter((p) => !p.gap).map((p) => p.src + p.len * (p.speed || 1))) : dur()) || 1;
  /** A piece's speed as a small tag at its bottom right ("2x", 0.1.41), right-aligned at `xr`. */
  function speedMark(ctx, C, text, xr, yb) {
    ctx.font = `700 10px ${C.sans}`;
    const w = ctx.measureText(text).width + 8;
    ctx.globalAlpha = 0.85;
    ctx.fillStyle = C.bg;
    rr(ctx, xr - w, yb - 12, w, 13, 4);
    ctx.fill();
    ctx.globalAlpha = 1;
    ctx.fillStyle = C.text;
    ctx.textBaseline = "middle";
    ctx.fillText(text, xr - w + 4, yb - 5.5);
  }
  /** Gaps on the main video row (0.1.36): an empty stretch with a dashed outline (and on the video row "Gap 1.5 s"). */
  function drawGaps(ctx, C, y, bh, labelled) {
    if (!m.parts) return;
    for (const p of emParts(m)) {
      if (!p.gap) continue;
      const sp = span(p.at, p.end, 4);
      if (!sp) continue;
      const x0 = sp[0] + 1, x1 = sp[1] - 1;
      ctx.clearRect(sp[0] - 0.5, y - 1, sp[1] - sp[0] + 1, bh + 2);
      if (x1 - x0 < 2) continue;
      ctx.setLineDash([4, 3]);
      ctx.lineWidth = 1;
      ctx.strokeStyle = C["line-2"];
      rr(ctx, x0 + 0.5, y + 0.5, x1 - x0 - 1, bh - 1, 6);
      ctx.stroke();
      ctx.setLineDash([]);
      if (!labelled) continue;
      const lx = Math.max(x0, 0) + 8, room = Math.min(x1, st.laneW) - lx - 6;
      if (room > 24) {
        ctx.font = `600 11px ${C.sans}`;
        ctx.textBaseline = "middle";
        ctx.fillStyle = C.muted;
        ctx.fillText(tlEllipsize(ctx, t("Gap {sec} s", { sec: tlFmtLen(p.len) }), room), lx, y + bh / 2);
      }
    }
  }
  /** The zoom's keyframes (0.1.41, zoom.js: the canvas pieces' keys): small diamonds on V1's top edge; a click goes to
   *  one, a drag retimes it (kfRetime). */
  const zoomKeys = () => (m.canvas?.pieces || []).flatMap((p, pi) => (Array.isArray(p.keys) ? p.keys.map((k) => ({ pi, t: k.t })) : []));
  function drawZoomKeys(ctx, C) {
    const L = zoomKeys();
    if (!L.length) return;
    ctx.save();
    for (const k of L) {
      const x = X(k.t);
      if (x < -6 || x > st.laneW + 6) continue;
      const on = st.drag?.type === "zkey" && st.drag.pi === k.pi && Math.abs(st.drag.cur - k.t) < 1e-6;
      ctx.fillStyle = on ? C["accent-2"] || C.accent : C.warn || C.accent;
      ctx.strokeStyle = C.bg;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(x, 0.5); ctx.lineTo(x + 4.5, 5); ctx.lineTo(x, 9.5); ctx.lineTo(x - 4.5, 5); ctx.closePath();
      ctx.fill();
      ctx.stroke();
    }
    ctx.restore();
  }
  /** The zoom key under lane x `x` near the video row's top edge (client y `cy`), or null. */
  function zoomKeyAt(x, cy) {
    const r = st.lanes.find((L) => L.id === "video")?.el.getBoundingClientRect();
    if (!r || cy - r.top > 11) return null;
    let best = null, dist = 6;
    for (const k of zoomKeys()) { const d = Math.abs(X(k.t) - x); if (d <= dist) { dist = d; best = k; } }
    return best;
  }
  /** Where a clip being dragged would land (0.1.36): its outline, at the pointer (free) or beside a clip (a marker). */
  function drawGhost(ctx, C, y, bh) {
    const g = st.drag?.type === "clip" && st.drag.moved ? st.drag.ghost : null;
    if (!g) return;
    const sp = span(g.from, g.to, 4);
    if (!sp) return;
    ctx.globalAlpha = 0.22;
    ctx.fillStyle = C["accent-2"] || C.accent;
    rr(ctx, sp[0], y, sp[1] - sp[0], bh, 8);
    ctx.fill();
    ctx.globalAlpha = 1;
    ctx.lineWidth = 2;
    ctx.strokeStyle = C["accent-2"] || C.accent;
    if (g.mode === "up") ctx.setLineDash([5, 4]); // going onto the picture (0.1.37)
    rr(ctx, sp[0] + 1, y + 1, sp[1] - sp[0] - 2, bh - 2, 8);
    ctx.stroke();
    ctx.setLineDash([]);
  }
  /** After the video, up to where the result ends (0.1.35): a black block, as the result is there. */
  function drawTail(ctx, C, y, bh) {
    const E = endT();
    if (!(E > dur() + 1e-6)) return;
    const sp = span(dur(), E);
    if (!sp) return;
    const [x0, x1] = [sp[0] + 2, sp[1]];
    if (x1 - x0 < 2) return;
    ctx.globalAlpha = 0.85;
    ctx.fillStyle = "#000";
    rr(ctx, x0, y, x1 - x0, bh, 8);
    ctx.fill();
    ctx.globalAlpha = 1;
    ctx.setLineDash([4, 3]);
    ctx.lineWidth = 1;
    ctx.strokeStyle = C["line-2"];
    rr(ctx, x0 + 0.5, y + 0.5, x1 - x0 - 1, bh - 1, 8);
    ctx.stroke();
    ctx.setLineDash([]);
    const lx = Math.max(x0, 0) + 8, room = Math.min(x1, st.laneW) - lx - 8;
    if (room > 30) {
      ctx.font = `600 11px ${C.sans}`;
      ctx.textBaseline = "middle";
      ctx.fillStyle = C.muted;
      ctx.fillText(tlEllipsize(ctx, t("Black after the video"), room), lx, y + bh / 2);
    }
  }

  /** The overlay rows: one block per item on row L.k (tlOvRows), with the item's icon and label, keyframe dots,
   *  and the selected one outlined. A media item that loops shows a faint mark where its media starts again. */
  const ovPath = new Map(); // kind -> Path2D of ovKindPath(kind)
  function ovIcon(kind) {
    if (ovPath.has(kind)) return ovPath.get(kind);
    let p = null;
    try { const d = typeof ovKindPath === "function" ? ovKindPath(kind) : ""; if (d) p = new Path2D(d); } catch { p = null; }
    ovPath.set(kind, p);
    return p;
  }
  function ovLabel(o) {
    try { if (typeof ovItemLabel === "function") { const s = ovItemLabel(o); if (s) return String(s); } } catch {}
    const names = { text: () => t("Text"), rect: () => t("Rectangle"), ellipse: () => t("Ellipse"), line: () => t("Line"), arrow: () => t("Arrow"), pen: () => t("Drawing"),
      blur: () => t("Blur box"), image: () => t("Picture"), gif: () => t("GIF"), video: () => t("Clip") };
    return names[o.kind] ? names[o.kind]() : String(o.kind || "");
  }
  function drawOv(L, C) {
    const ctx = L.ctx, h = L.h;
    prep(ctx, h);
    const { rows } = ovLayout(), items = ovItems();
    const y = 3, bh = h - 6;
    const dragId = st.drag?.ovId && st.drag.moved ? st.drag.ovId : null;
    items.forEach((o, i) => {
      if (rows[i] !== L.k) return;
      const sp = span(o.from, o.to);
      if (!sp) return;
      const x0 = sp[0] + 0.5, x1 = sp[1] - 0.5, w = x1 - x0;
      if (w < 1) return;
      const sel = st.ovSel === o.id, tint = o.kind === "blur" ? C.muted : isMedia(o) ? C.file : C["accent-2"] || C.accent;
      ctx.save();
      rr(ctx, x0, y, w, bh, 6);
      ctx.fillStyle = C.raised;
      ctx.fill();
      ctx.clip();
      ctx.globalAlpha = dragId === o.id ? 0.5 : 0.34;
      ctx.fillStyle = tint;
      ctx.fillRect(x0, y, w, bh);
      ctx.globalAlpha = 1;
      // Keyframes: small diamonds along the bottom.
      if (Array.isArray(o.keys)) {
        ctx.fillStyle = C.text;
        for (const k of o.keys) {
          const kx = X(k?.t);
          if (!(kx >= x0 - 4 && kx <= x1 + 4)) continue;
          ctx.beginPath();
          ctx.moveTo(kx, y + bh - 8); ctx.lineTo(kx + 3, y + bh - 5); ctx.lineTo(kx, y + bh - 2); ctx.lineTo(kx - 3, y + bh - 5);
          ctx.fill();
        }
      }
      // A looping media item: a faint line where its media starts over.
      const ml = Number.isFinite(o.mlen) && o.mlen > 0.05 && o.loop ? o.mlen : 0;
      if (ml && (o.to - o.from) / ml < 200) {
        ctx.globalAlpha = 0.4;
        ctx.fillStyle = C.text;
        for (let s = o.from + ml - (Number.isFinite(o.off) ? o.off % ml : 0); s < o.to - 1e-6; s += ml) { const lx = X(s); if (lx > x0 && lx < x1) ctx.fillRect(Math.round(lx) - 0.5, y, 1, bh); }
        ctx.globalAlpha = 1;
      }
      // Icon and label at the visible left end.
      const lx = Math.max(x0, 0) + 6, room = Math.min(x1, st.laneW) - lx - 6;
      const p = ovIcon(o.kind);
      let tx = lx;
      if (p && room >= 16) {
        ctx.save();
        ctx.translate(lx, y + (bh - 14) / 2);
        ctx.scale(14 / 24, 14 / 24);
        ctx.strokeStyle = C.text;
        ctx.lineWidth = 2;
        ctx.lineCap = ctx.lineJoin = "round";
        ctx.stroke(p);
        ctx.restore();
        tx += 19;
      }
      if (room - (tx - lx) > 14) {
        ctx.font = `600 11px ${C.sans}`;
        ctx.textBaseline = "middle";
        ctx.fillStyle = C.text;
        ctx.fillText(tlEllipsize(ctx, ovLabel(o), room - (tx - lx)), tx, y + bh / 2 + 0.5);
      }
      ctx.restore();
      ctx.lineWidth = sel ? 2 : 1;
      ctx.strokeStyle = sel ? C["accent-2"] || C.accent : C["line-2"];
      rr(ctx, x0 + (sel ? 1 : 0.5), y + (sel ? 1 : 0.5), w - (sel ? 2 : 1), bh - (sel ? 2 : 1), 6);
      ctx.stroke();
      // Grips on both ends (they trim), when the block is wide enough.
      if (w > 18) {
        ctx.fillStyle = C.text;
        ctx.globalAlpha = 0.6;
        for (const ex of [x0 + 3, x1 - 5]) if (ex > -4 && ex < st.laneW + 4) { rr(ctx, ex, y + bh / 2 - 6, 2, 12, 1); ctx.fill(); }
        ctx.globalAlpha = 1;
      }
    });
  }
  /** The overlay item under lane x `x` on overlay row `k`, the top one first: { o, hit: "left" | "right" | "move" } or null. */
  function ovAtX(k, x) {
    const { rows } = ovLayout(), items = ovItems(), v = vAtX(x);
    for (let i = items.length - 1; i >= 0; i--) {
      if (rows[i] !== k) continue;
      const hit = tlSpanHit(toV(items[i].from), toV(items[i].to), v, st.pps);
      if (hit) return { o: items[i], hit };
    }
    return null;
  }
  function nearestThumb(i) {
    const list = st.thumbs;
    for (let d = 0; d < list.length; d++) {
      if (list[i - d]?.ok) return list[i - d].img;
      if (list[i + d]?.ok) return list[i + d].img;
    }
    return null;
  }

  /* An audio row (0.1.39): a track row draws the clip's own sound (silent over a piece whose sound was ungrouped),
   * then every audio clip that lies on the row; an added row draws only its clips. Clips may lie over each other. */
  function drawAudio(L, C) {
    const ctx = L.ctx, h = L.h;
    prep(ctx, h);
    const lane = emLaneOf(m, L.id);
    if (!lane) return;
    const lm = !!lane.muted, lv = lane.kind === "lane" || lane.kind === "track" ? lane.volume ?? 1 : 1;
    if (lane.kind === "track") {
      drawBlock(L, C, lane, false, 1);
      // Pieces whose sound was ungrouped: nothing of the grouped sound there.
      for (const p of emParts(m)) if (m.parts[p.i]?.noSound) { const sp = span(p.at, p.end, 4); if (sp) ctx.clearRect(sp[0], 2, sp[1] - sp[0], h - 4); }
    }
    for (const r of m.audio) if (r.kind === "file" && r.lane === L.id) drawBlock(L, C, r, lm, lv);
  }
  function drawBlock(L, C, row, laneMuted, laneVol) {
    const ctx = L.ctx, h = L.h;
    const file = row.kind === "file";
    const muted = row.muted || laneMuted;
    const a = file ? row.offset : 0, b = file ? emRowEnd(m, row) : dur();
    const sp = span(a, b);
    if (!sp) return;
    const [bx0, bx1] = sp, y = 4, bh = h - 8, mid = y + bh / 2, half = bh / 2 - 4;
    const tint = muted ? C.dim : file ? C.file : C.accent;
    ctx.save();
    rr(ctx, bx0, y, bx1 - bx0, bh, 7);
    ctx.clip();
    ctx.fillStyle = C.raised;
    ctx.fillRect(bx0, y, bx1 - bx0, bh);
    ctx.globalAlpha = muted ? 0.18 : 0.3;
    ctx.fillStyle = tint;
    ctx.fillRect(bx0, y, bx1 - bx0, bh);
    ctx.globalAlpha = 1;
    // An audio clip (0.1.41) is one piece of its own: V1's cuts do not show through it, and its level is its own (one
    // badge for the whole clip). A track row is the pieces' own sound: one segment per piece.
    const segs = file ? [{ i: 0, start: a, end: b, removed: false }] : emSegments(m);
    const levelAt = (i) => (file ? (row.muted ? 0 : row.volume) : row.gains[i]);
    // Silent segments: greyed.
    for (const s of segs) {
      if (file || row.gains[s.i] !== 0) continue; // a muted clip is dimmed as a whole (tint)
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
        while (!file && si < segs.length - 1 && sc >= segs[si].end) si++;
        // A Game row is drawn quiet where Desktop is heard: the result uses one or the other, never both.
        const g = muted || (!file && typeof emGameHidden === "function" && emGameHidden(m, row, si)) ? 0 : (file ? 1 : row.gains[si] ?? 1) * row.volume * laneVol;
        // A file's peaks follow the timeline; a track's follow the source (with parts: the part under the bar).
        const base = file ? row.trimStart - row.offset : 0, mapped = !file && hasParts();
        // A file that plays through cuts: the place in the file under the bar (it pauses under a cut-out part).
        const thru = file && row.through;
        const w0 = thru ? emRowFileAt(m, row, s0) - base : mapped ? srcAt(s0) : s0, w1 = thru ? emRowFileAt(m, row, s1) - base : mapped ? w0 + (s1 - s0) * pieceSpeedAt(s0) : s1; // a sped piece's sound is squeezed (0.1.41)
        const p = tlPeakMax(wave.peaks, (w0 + base) * wave.ps, (w1 + base) * wave.ps);
        const hh = Math.max(1, Math.min(1, p * g * tlWaveGain) * half);
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
      // Its sound is still on its way (0.1.41, a project's clips load in the background): said quietly.
      if (!file && opts.soundLoading?.()) {
        ctx.font = `600 10.5px ${C.sans}`;
        ctx.textBaseline = "middle";
        ctx.fillStyle = C.muted;
        ctx.globalAlpha = 0.8;
        ctx.fillText(t("Loading sound…"), Math.max(from, 0) + 10, mid - 9);
        ctx.globalAlpha = 1;
      }
    }
    if (!file) overlays(ctx, C, a, b, y, bh, row.id); // V1's cut and piece borders: not through an audio clip (0.1.41)
    ctx.restore();
    ctx.lineWidth = 1;
    ctx.strokeStyle = C["line-2"];
    rr(ctx, bx0 + 0.5, y + 0.5, bx1 - bx0 - 1, bh - 1, 7);
    ctx.stroke();
    if (!file) drawGaps(ctx, C, y, bh, false); // the clip's own sound is not there in a gap (0.1.36)
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
      const g = levelAt(s.i);
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
    if (st.names) label(ctx, C, "note", tlRowName(row, opts.game), bx0, bx1, y + bh - 24); // the name can be switched off (toolbar, menu)
    // A member of a group (0.1.39): a small link mark at its top left.
    if (file && row.group && bx1 - bx0 > 16) groupMark(ctx, C, Math.max(bx0, 0) + 5, y + 4);
  }
  function groupMark(ctx, C, x, y) {
    ctx.save();
    ctx.globalAlpha = 0.9;
    ctx.fillStyle = C["accent-2"] || C.accent;
    rr(ctx, x, y, 8, 8, 2);
    ctx.fill();
    ctx.fillStyle = C.bg;
    ctx.fillRect(x + 2, y + 3.5, 4, 1);
    ctx.restore();
  }

  /** The clip blocks ("Edit together"): one per part in display order with its title and length. A clip cut out
   *  entirely is hatched; the one being dragged is faded (the drop marker .tl-pdrop shows where it goes). */
  function drawParts(L, C) {
    const ctx = L.ctx, h = L.h;
    prep(ctx, h);
    if (!showStrip()) return;
    const y = 3, bh = h - 6, mid = y + bh / 2 + 0.5;
    const dragged = st.drag?.type === "part" && st.drag.moved ? st.drag.from : -1;
    const sans = `600 11.5px ${C.sans}`, mono = `600 10.5px ${C.mono}`;
    ctx.textBaseline = "middle";
    for (const p of emParts(m)) {
      if (p.gap) continue; // an empty stretch has no block
      const sp = span(p.at, p.end);
      if (!sp) continue;
      const x0 = sp[0] + 1.5, x1 = sp[1] - 1.5, w = x1 - x0; // a small gap between neighbours
      if (w < 2) continue;
      const out = emPartRemoved(m, p.i), alpha = p.i === dragged ? 0.4 : 1;
      ctx.save();
      ctx.globalAlpha = alpha;
      rr(ctx, x0, y, w, bh, 6);
      ctx.fillStyle = C.raised;
      ctx.fill();
      ctx.clip();
      ctx.globalAlpha = alpha * (out ? 0.12 : 0.34);
      ctx.fillStyle = C.accent;
      ctx.fillRect(x0, y, w, bh);
      ctx.globalAlpha = alpha;
      if (out) { ctx.fillStyle = hatchFor(ctx, C); ctx.fillRect(x0, y, w, bh); }
      ctx.restore();
      ctx.save();
      ctx.globalAlpha = alpha;
      ctx.lineWidth = 1;
      ctx.strokeStyle = C["line-2"];
      rr(ctx, x0 + 0.5, y + 0.5, w - 1, bh - 1, 6);
      ctx.stroke();
      // Title at the visible left end, the length at the right when there is room.
      const lx = Math.max(x0, 0) + 8, rx = Math.min(x1, st.laneW) - 8, room = rx - lx;
      if (room > 12) {
        const len = tlFmtTick(p.len, p.len < 10 ? 0.5 : 1);
        ctx.font = mono;
        const lw = ctx.measureText(len).width, showLen = room > lw + 48;
        ctx.font = sans;
        const title = tlEllipsize(ctx, p.title || "", room - (showLen ? lw + 10 : 0));
        if (title) { ctx.fillStyle = out ? C.muted : C.text; ctx.fillText(title, lx, mid); }
        if (showLen) { ctx.font = mono; ctx.fillStyle = C.muted; ctx.fillText(len, rx - lw, mid); }
      }
      ctx.restore();
    }
  }
  /** The drop marker while a clip block is dragged: in the gap the block would go to. */
  function placeDrop(d) {
    const P = emParts(m);
    const at = d.slot < P.length ? P[d.slot].at : dur();
    el.pdrop.hidden = false;
    el.pdrop.style.top = `${tlRulerH + ovN() * tlOvH}px`;
    el.pdrop.style.transform = `translateX(${Math.round(X(at))}px)`;
    el.pdrop.classList.toggle("same", d.to === d.from);
  }

  function placeCuts() {
    const n = m.cuts.length;
    if (el.cuts.children.length !== n) {
      const title = esc(t("Drag to move this cut, double-click to join the two segments"));
      el.cuts.innerHTML = m.cuts.map((_, i) => `<div class="tl-cut" data-tl-cut="${i}" title="${title}" style="top:${rowsTop()}px"><b></b></div>`).join("");
    }
    // The cuts where the kept span starts and ends are the trim brackets: no handle of their own.
    const f = m.removed.indexOf(false), l = m.removed.lastIndexOf(false);
    for (let i = 0; i < n; i++) {
      const c = el.cuts.children[i], x = X(m.cuts[i]);
      // With cut parts hidden, a cut next to a removed part sits on a joint: the joint is shown instead (and cannot be dragged).
      // A part border has no handle: it cannot be dragged or joined.
      const vis = x > -8 && x < st.laneW + 8 && !(st.hideCuts && (m.removed[i] || m.removed[i + 1])) && i !== f - 1 && i !== l && !lockedCut(i);
      c.style.transform = `translateX(${vis ? x : -100}px)`;
      c.classList.toggle("drag", st.drag?.type === "cut" && st.drag.moved && st.drag.c === i);
    }
    placeJoints();
    placeTrims();
    placeMarks();
  }
  /** The two trim brackets, at the ends of the kept span (with cut parts hidden: the ends of what is shown). */
  function placeTrims() {
    // 0.1.37: every piece has its own end handles on the video row; the brackets of the whole kept span are gone.
    if (!el.trimS.hidden) el.trimS.hidden = el.trimE.hidden = true;
  }
  /** Per shown bookmark: a flag on the ruler (.tl-bmk: click goes there, hover shows the time, in ruler time) and a thin line through the rows (.tl-bml). */
  function placeMarks() {
    const list = marks().map((sec, i) => ({ sec, i, at: tlFmtPrecise(toV(sec)) })).filter((b) => markShown(b.sec));
    const sig = list.map((b) => `${b.i}:${b.at}`).join("|");
    if (sig !== st.markSig) {
      st.markSig = sig;
      el.bmFlags.innerHTML = list.map((b) => `<div class="tl-bmk" data-tl-bm="${b.i}" title="${esc(t("Bookmark at {time}", { time: b.at }))}"><b></b></div>`).join("");
      el.bmLines.innerHTML = list.map(() => `<div class="tl-bml" style="top:${rowsTop()}px"></div>`).join("");
    }
    list.forEach((b, k) => {
      const x = Math.round(X(b.sec) * 2) / 2, vis = x > -16 && x < st.laneW + 16;
      el.bmFlags.children[k].style.transform = el.bmLines.children[k].style.transform = `translateX(${vis ? x : -100}px)`;
    });
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
        return `<div class="tl-joint" data-tl-joint="${i}" title="${title}" style="top:${rowsTop()}px"><b></b></div>`;
      }).join("");
    }
    list.forEach((j, i) => {
      const x = X(j.start), vis = x > -8 && x < st.laneW + 8;
      el.joints.children[i].style.transform = `translateX(${vis ? x : -100}px)`;
    });
  }

  function draw() {
    if (!st.laneW) return;
    // Nothing below changes the model: its lists are built once for the whole redraw (editmodel.js).
    return typeof emHold === "function" ? emHold(drawNow) : drawNow();
  }
  function drawNow() {
    const C = colors();
    if (st.dirtyAll || st.dirtyRuler) drawRuler(C);
    for (const L of st.lanes) {
      if (!st.dirtyAll && !st.dirty.has(L.id)) continue;
      if (L.kind === "video") drawVideo(L, C); else if (L.kind === "parts") drawParts(L, C); else if (L.kind === "ov") drawOv(L, C); else drawAudio(L, C);
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
    if (st.hideCuts && st.map && !st.mapHold && st.map.sig !== mapSig()) { markAll(); relayout(); }
    // The overlay code added, removed or changed an item without calling refresh(): rows and blocks follow.
    if (ovSig() !== st.ovSigShown) ovChanged();
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
    flushMove(st.drag);
    if (st.drag?.moved && st.drag.type !== "click") edgeScroll();
    if (st.dirtyAll || st.dirtyRuler || st.dirty.size) draw();
    placePlayhead(st.shownT);
    updateBarCheap(st.shownT, false);
    // The timecode at the left of the toolbar: the playhead in ruler time, as the ruler and the drag tips count it.
    const tc = tlFmtPrecise(toV(st.shownT));
    if (tc !== st.tcText) { st.tcText = tc; el.tc.textContent = tc; }
  }

  /* ----- mouse ----- */
  function snapCands(skip) {
    const c = [0, dur(), ...m.cuts.filter((_, i) => !(skip.cut === i)), ...shownMarks()];
    if (!skip.playhead) c.push(st.shownT);
    for (const r of m.audio) if (r.kind === "file" && r.id !== skip.row) c.push(r.offset, emRowEnd(m, r));
    for (const o of ovItems()) if (o.id !== skip.ov) c.push(o.from, o.to); // the other blocks' ends
    if (!skip.ov && !skip.row) c.push(endT());
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
  const fileHit = (row, x) => tlSpanHit(toV(row.offset), toV(emRowEnd(m, row)), vAtX(x), st.pps);
  /** The lane under client y, or null. */
  const laneAt = (clientY) => st.lanes.find((L) => { const r = L.el.getBoundingClientRect(); return clientY >= r.top && clientY < r.bottom; }) || null;
  /** The piece under lane x `x` on lane `id`: { row, seg }, or null (outside the clip, on a file row off its clip). */
  /** The audio clip on row `laneId` under lane x `x`, the top one first (0.1.39): { row, hit } or null. */
  function clipAtX(laneId, x) {
    const L = m.audio.filter((r) => r.kind === "file" && r.lane === laneId);
    for (let i = L.length - 1; i >= 0; i--) { const hit = fileHit(L[i], x); if (hit) return { row: L[i], hit }; }
    return null;
  }
  /** The selection piece of a clip: its row is the clip, its segment the one under its middle. */
  const clipSel = (r) => ({ row: r.id, seg: emSegmentAt(m, Math.min(dur() - 1e-6, Math.max(0, r.offset + (emRowEnd(m, r) - r.offset) / 2))) });
  /** The piece under lane x `x` on lane `id`: { row, seg }, or null. On an audio row (0.1.39): a clip there, else the
   *  track row's own sound of the piece above (none where its sound was ungrouped). */
  function pieceAtX(id, x) {
    if (id === "parts") return null; // the clip blocks are not pieces
    const v = vAtX(x);
    if (id !== "video") {
      const c = clipAtX(id, x);
      if (c) return { row: c.row.id, seg: emSegmentAt(m, Math.min(dur() - 1e-6, Math.max(0, toS(v)))) };
      const lane = emLaneOf(m, id);
      if (!lane || lane.kind !== "track") return null;
    }
    if (!(v >= 0 && v <= vdur())) return null;
    const seg = emSegmentAt(m, toS(Math.min(v, vdur() - 1e-6)));
    if (!segShown(seg)) return null;
    if (id !== "video" && (segGap(seg) || emSegNoSound(m, seg))) return null;
    return { row: id, seg };
  }
  /** Pieces of lane `id` in content x (lane x + scrollX), for the rubber band: [{ seg, x0, x1, row? }]. */
  function lanePieces(id) {
    const cx = (sec) => tlPad + toV(sec) * st.pps;
    const segs = emSegments(m).filter((s) => segShown(s.i));
    if (id === "video") return segs.map((s) => ({ seg: s.i, x0: cx(s.start), x1: cx(s.end) })).filter((p) => p.x1 > p.x0);
    const lane = emLaneOf(m, id);
    if (!lane) return [];
    const out = lane.kind === "track" ? segs.filter((s) => !segGap(s.i) && !emSegNoSound(m, s.i)).map((s) => ({ seg: s.i, row: id, x0: cx(s.start), x1: cx(s.end) })) : [];
    for (const r of m.audio) if (r.kind === "file" && r.lane === id) { const c = clipSel(r); out.push({ seg: c.seg, row: r.id, x0: cx(r.offset), x1: cx(emRowEnd(m, r)) }); }
    return out.filter((p) => p.x1 > p.x0);
  }

  function onDown(e) {
    if (e.button !== 0 || st.drag) return;
    closeMenu();
    const rect = el.lanes.getBoundingClientRect();
    const x = e.clientX - rect.left, at = timeAtX(x);
    const d = { x0: e.clientX, y0: e.clientY, sx0: st.scrollX, rect, moved: false, snapped: false, lastE: e, ctrl: e.ctrlKey || e.metaKey, shift: e.shiftKey };
    const cut = e.target.closest("[data-tl-cut]"), bm = e.target.closest("[data-tl-bm]");
    const bmAt = bm ? marks()[+bm.dataset.tlBm] : undefined;
    // A joint marker is only a hover / right-click target: a press on it acts on the lane under it.
    const lane = e.target.closest("[data-tl-lane]") || (e.target.closest("[data-tl-joint]") ? laneAt(e.clientY)?.el : null);
    const trim = cut ? null : e.target.closest("[data-tl-trim]");
    // The blade (0.1.36): a click on the video row or an overlay block splits it there; nothing else is picked.
    if (st.tool === "blade" && lane && !cut && !trim && bmAt === undefined) {
      e.preventDefault();
      const sec = e.altKey ? at : snapAt(at, [st.shownT, ...shownMarks()]).at;
      // 0.1.41: the blade cuts what it clicks (and that thing's group), nothing else.
      if (lane.dataset.tlLane === "video") { if (!doSplit(sec, { video: true })) showTip(sec, t("Cannot split here")); }
      else if (lane.dataset.tlKind === "ov") { const hit = ovAtX(+lane.dataset.tlLane.slice(2), x); if (hit) doOvSplit(hit.o.id, sec); }
      else { const c = clipAtX(lane.dataset.tlLane, x); if (c) { if (!doSplit(sec, { clip: c.row.id })) showTip(sec, t("Cannot split here")); } else if (emRow(m, lane.dataset.tlLane)?.kind === "track" && !doSplit(sec, { video: true })) showTip(sec, t("Cannot split here")); } // an audio clip (0.1.39); a track row: its piece
      return;
    }
    if (cut) Object.assign(d, { type: "cut", c: +cut.dataset.tlCut });
    // A trim bracket: the drag works from the segments as they are now (see the file header); it snaps to
    // what is there now, so not to the cut it makes itself.
    else if (trim) {
      Object.assign(d, { type: "trim", side: trim.dataset.tlTrim === "end" ? "end" : "start", base: emSegCopy(m), b0: emTrimBounds(m), cands: snapCands({}), selT: selTimes() });
      if (st.hideCuts) {
        // Cut parts hidden: they are shown for the drag, so the bracket can be pulled back out over footage that
        // was cut away (0.1.33). The bracket stays under the pointer, the zoom stays, and the parts are hidden
        // again on release (endDrag).
        const at = d.side === "start" ? d.b0.start : d.b0.end, x = X(at);
        d.rehide = { fitted: st.fitted };
        st.hideCuts = false;
        st.fitted = false;
        markAll();
        relayout();
        setScroll(tlPad + toV(at) * st.pps - x);
      }
    }
    // A bookmark flag: the playhead goes exactly onto it; dragging on from there is an ordinary ruler drag.
    else if (bmAt !== undefined) Object.assign(d, { type: "seek", at: bmAt });
    else if (e.target.closest("[data-tl-ph], [data-tl-ruler]")) Object.assign(d, { type: "seek", moved: true });
    // A clip block: a click goes to it, a drag moves it (see doMovePart).
    else if (lane && lane.dataset.tlLane === "parts") {
      const i = partAtX(x);
      if (i < 0) return;
      Object.assign(d, { type: "part", from: i, to: i, slot: i });
    } else if (lane && lane.dataset.tlKind === "ov") {
      // An overlay block: a click selects it, a drag moves or trims it (one undo step), up or down changes its order.
      const k = +lane.dataset.tlLane.slice(2), hit = ovAtX(k, x);
      if (!hit) Object.assign(d, { type: "click", sel: null, ovClear: true });
      else {
        const o = hit.o;
        Object.assign(d, { type: "ov-" + hit.hit, ovId: o.id, ovRow: k, grab: at,
          o0: { from: o.from, to: o.to, off: o.off, keys: Array.isArray(o.keys) ? o.keys.map((q) => q?.t) : null } });
      }
    } else if (lane) {
      const id = lane.dataset.tlLane, ch = id === "video" ? null : clipAtX(id, x), row = ch?.row || null, hit = ch?.hit || null;
      const sel = pieceAtX(id, x);
      // 0.1.41: a press on something that is selected with other things: they all move together (a temporary group;
      // tmpDone ends it on release).
      // (Only a press on a body: an end grip trims, a keyframe diamond retimes, a gap's edge resizes, as before.)
      const body = id === "video" && sel && !zoomKeyAt(x, e.clientY) && !pieceEndHit(x) && gapHitRight(x) < 0;
      const pressedSel = hit === "move" ? st.sels.some((s) => s.row === row.id) : body ? tlSelHas(st.sels, sel) && !segGap(sel.seg) : false;
      if (pressedSel && st.sels.length > 1 && !d.ctrl && !d.shift) {
        const pre = emState(m), n0 = m.undo.length, tid = tmpGroupOn(st.sels);
        if (tid) d.tmp = { id: tid, pre, preTemp: emState(m), n0 };
        else if (emState(m) !== pre) emRestore(m, pre);
      }
      // On an audio row, a press on a clip moves it (also onto another row: 0.1.39) or trims it; anywhere else it is a
      // click, or a rubber band once it moves.
      if (hit) Object.assign(d, { type: hit, rowId: row.id, lane0: row.lane, row0: { offset: row.offset, trimStart: row.trimStart, length: row.length }, grab: at, sel: clipSel(row), base: emState(m), marks0: (m.marks || []).slice() });
      else Object.assign(d, { type: "click", sel });
      // The video row (0.1.36): a press on a clip drags it once it moves (free placement, or beside another clip);
      // one on a gap's right edge makes the gap longer or shorter.
      // 0.1.37: a press on either end of a piece trims it, or pulls cut-away footage back out (emTrimPiece).
      const zk = id === "video" ? zoomKeyAt(x, e.clientY) : null; // 0.1.41: a zoom keyframe on the top edge
      const pe = id === "video" && !zk ? pieceEndHit(x) : null;
      if (zk) Object.assign(d, { type: "zkey", pi: zk.pi, t0: zk.t, cur: zk.t, sel: undefined });
      else if (pe) {
        const p = emPieceList(m)[pe.i];
        Object.assign(d, { type: "piece-end", sel, i: pe.i, side: pe.side, base: emState(m), marks0: (m.marks || []).slice(), at0: p.at, src0: p.src, len0: p.len, lo: p.lo, hi: p.hi, speed: p.speed });
      } else if (id === "video" && sel) {
        const gi = gapHitRight(x);
        if (gi >= 0) Object.assign(d, { type: "gap-edge", gap: gi, base: emState(m), at0: emParts(m)[gi].at, len0: emParts(m)[gi].len });
        else if (!segGap(sel.seg) && !d.ctrl && !d.shift) {
          const P = emParts(m), i = hasParts() ? emPartAt(m, at) : 0, p = P[i] || { at: 0, end: dur(), len: dur() };
          d.clip = { i, at0: p.at, len: p.end - p.at, grab: at, grp: m.parts?.[i]?.group || null };
        }
      }
    } else return;
    e.preventDefault();
    st.drag = d;
    host.classList.add("tl-dragging");
    host.dataset.tlDrag = d.type;
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    window.addEventListener("blur", onLost);
    if (d.type === "seek" && d.moved) apply(d, e);
    else if (d.type === "seek") { seekExact(d.at); showTip(d.at); }
  }
  // The mouse-up never arrives when the window loses the mouse mid-drag (Alt+Tab, a
  // screenshot tool): end the drag instead of leaving it, and its rubber band, hanging.
  /** The window lost the mouse mid-drag (R1 #10: it left a half-applied live preview and an undo step behind). A drag
   *  that only began is dropped; one with a live preview from a saved state is called off (back to it, no undo step);
   *  any other is finished as if released where it last was. */
  function onLost() {
    const d = st.drag;
    if (!d) return;
    try { onLost0(d); } finally { tmpDone(d); }
  }
  function onLost0(d) {
    if (!d.moved) return endDrag();
    if (d.snapped && typeof d.base === "string") {
      emRestore(m, d.base);
      if (m.marks && d.marks0) m.marks = d.marks0.slice();
      if (m.undo[m.undo.length - 1] === d.base) m.undo.pop();
      for (const x of st.lanes) x.el.classList.remove("tl-drop-to");
      el.rows.classList.remove("tl-drop-new");
      el.pdrop.hidden = true;
      endDrag();
      relaid("cancel");
      return;
    }
    onUp(d.lastE || { clientX: d.x0, clientY: d.y0, target: host });
  }
  function onMove(e) {
    const d = st.drag;
    if (!d) return;
    if (e.buttons === 0) return onUp(e); // released while we could not see it
    d.lastE = e;
    if (!d.moved) {
      // A click may turn into a rubber band in any direction; the other drags are sideways.
      const dist = d.type === "click" || d.type === "ov-move" || d.type === "move" ? Math.max(Math.abs(e.clientX - d.x0), Math.abs(e.clientY - d.y0)) : Math.abs(e.clientX - d.x0);
      if (dist < tlDragPx) return;
      d.moved = true;
      if (d.type === "click" && d.clip) { d.type = "clip"; host.dataset.tlDrag = "clip"; }
      else if (d.type === "click") startBand(d);
    }
    // Applied once per animation frame (frame()), with the newest position: a mouse can report
    // several hundred moves a second, and each step redraws the timeline and syncs the editor.
    d.pendE = e;
  }
  /** Apply the drag's newest mouse position, if it has not been yet. */
  function flushMove(d) {
    if (!d || !d.pendE) return;
    const e = d.pendE;
    d.pendE = null;
    apply(d, e);
  }
  /** One step of a drag at mouse event `e`. */
  function apply(d, e) {
    if (d.type === "band") return moveBand(d, e);
    const x = e.clientX - d.rect.left, raw = timeAtX(x), free = e.altKey; // Alt: no snapping
    const snap = (sec, skip) => (free ? sec : snapAt(sec, snapCands(skip)).at);
    if (d.type === "seek") {
      // Anywhere on the lanes, also after the video and after the result's end (0.1.35).
      const at = Math.min(maxT(), Math.max(0, snap(raw, { playhead: true })));
      d.at = at;
      st.shownT = at;
      showTip(at);
      opts.onSeek?.(at);
      return;
    }
    if (d.type === "part") {
      const P = emParts(m);
      Object.assign(d, tlPartSlot(P.map((p) => X((p.at + p.end) / 2)), d.from, x));
      placeDrop(d);
      showTip(d.slot < P.length ? P[d.slot].at : dur(), P[d.from].title);
      st.dirty.add("parts");
      return;
    }
    if (d.type === "clip") {
      // A clip on the video row (0.1.36): over another clip it goes beside it (the nearer half), anywhere else it
      // sits where it is dropped (snapped by its start or its end). Shown as an outline; applied on release.
      // A piece in a group (0.1.39): its other members are shown moving with it, live, from the state the drag began
      // in (the drop itself is made again on release as one undo step).
      if (d.clip.grp) {
        if (!d.snapped) { emSnapshot(m); d.snapped = true; d.base = m.undo[m.undo.length - 1]; d.marks0 = (m.marks || []).slice(); }
        emRestore(m, d.base);
        if (m.marks) m.marks = d.marks0.slice();
        markAll();
      }
      const c = d.clip, P = emParts(m), drop = hasParts() ? tlDropAt(P, c.i, raw) : { mode: "free" };
      // Above the video row (0.1.37): onto the overlay rows, as a clip item over the whole picture.
      const vTop = st.lanes.find((L) => L.id === "video")?.el.getBoundingClientRect().top ?? -Infinity;
      if (e.clientY < vTop - 2 && opts.canPieceUp?.()) {
        el.pdrop.hidden = true;
        let from = Math.max(0, c.at0 + raw - c.grab);
        if (!free) { const cands = [0, st.shownT, ...shownMarks(), ...ovItems().flatMap((o) => [o.from, o.to])]; const s1 = snapAt(from, cands), s2 = snapAt(from + c.len, cands); if (s1.hit || s2.hit) from = Math.max(0, s1.dist <= s2.dist ? s1.at : s2.at - c.len); }
        d.ghost = { mode: "up", from, to: from + c.len };
        showTip(from, t("On the picture at {time}", { time: tlFmtPrecise(toV(from)) }));
        st.dirty.add("video");
        return;
      }
      if (drop.mode === "beside") {
        const q = P[drop.k], from = drop.after ? q.end : q.at - c.len;
        d.ghost = { mode: "beside", k: drop.k, after: drop.after, from: drop.after ? q.end : Math.max(0, from), to: drop.after ? q.end + c.len : q.at };
        el.pdrop.hidden = false;
        el.pdrop.style.top = `${tlRulerH + ovN() * tlOvH}px`;
        el.pdrop.style.transform = `translateX(${Math.round(X(drop.after ? q.end : q.at))}px)`;
        el.pdrop.classList.remove("same");
        showTip(drop.after ? q.end : q.at, drop.after ? t("After {name}", { name: q.title || "" }) : t("Before {name}", { name: q.title || "" }));
      } else {
        el.pdrop.hidden = true;
        let from = Math.max(0, c.at0 + raw - c.grab);
        if (!free) {
          // Snap to the playhead, the other clips' ends, bookmarks and 0 (by the clip's start or its end).
          const cands = [0, st.shownT, ...shownMarks(), ...P.filter((p) => !p.gap && p.i !== c.i).flatMap((p) => [p.at, p.end])];
          const s1 = snapAt(from, cands), s2 = snapAt(from + c.len, cands);
          if (s1.hit || s2.hit) from = Math.max(0, s1.dist <= s2.dist ? s1.at : s2.at - c.len);
        }
        from = groupFrom(c.i, c.grp, from); // moved left it stops at the clip before it (R1 #3)
        d.ghost = { mode: "free", from, to: from + c.len };
        showTip(from);
        if (c.grp && Math.abs(from - c.at0) > 1e-6) {
          const obj = m.parts[c.i];
          emPlacePart(m, c.i, from);
          const q = emParts(m).find((x) => m.parts[x.i] === obj);
          if (q) groupMove(c.grp, q.at - c.at0, obj);
          changed("order", false);
        }
      }
      st.dirty.add("video");
      return;
    }
    if (d.type === "zkey") {
      // A zoom keyframe dragged along (0.1.41): kfRetime keeps it between its neighbours. One undo step.
      const p = m.canvas?.pieces?.[d.pi];
      if (!p || !Array.isArray(p.keys) || typeof kfRetime !== "function") return;
      if (!d.snapped) { emSnapshot(m); d.snapped = true; }
      const to = free ? raw : snapAt(raw, [st.shownT, ...shownMarks(), ...m.cuts]).at;
      d.cur = kfRetime(p.keys, d.cur, Math.max(0, to), Math.max(dur(), endT()));
      showTip(d.cur);
      changed("canvas", false);
      return;
    }
    if (d.type === "piece-end") {
      // A piece's end (0.1.37): inward it trims, outward it brings back footage, up to the source clip's limit.
      // Live, from the state the drag began in.
      if (!d.snapped) { emSnapshot(m); d.snapped = true; }
      emRestore(m, d.base);
      if (m.marks) m.marks = d.marks0.slice();
      const L = emPieceList(m), cands = [st.shownT, ...shownMarks(), ...L.filter((p) => p.i !== d.i && !p.gap).flatMap((p) => [p.at, p.at + p.len])];
      const want = free ? raw : snapAt(raw, cands).at;
      const part0 = m.parts ? m.parts[d.i] : null, edge0 = d.side === "end" ? d.at0 + d.len0 : d.at0;
      const r = emTrimPiece(m, d.i, d.side, want);
      if (!r) return;
      // Its group (0.1.39): the same end of every audio clip in it that lay at the same time goes along.
      // The start (R1 #4): the clip gains (or loses) as much footage as the piece did and starts where it starts now;
      // pulled out without room before it the piece grows in place, and its sound must too (not stay where it was).
      if (part0?.group) for (const x of m.audio) if (x.kind === "file" && x.group === part0.group && Math.abs((d.side === "end" ? x.offset + x.length : x.offset) - edge0) < 1e-3) {
        if (d.side === "end") { emTrimClip(m, x.id, "end", r.at + r.len); continue; }
        const gained = d.src0 - r.src, lo = Number.isFinite(x.fileLo) ? x.fileLo : 0;
        const ts = Math.max(lo, x.trimStart - gained);
        x.length = Math.max(EM_MIN_SEG, x.length + (x.trimStart - ts));
        x.trimStart = ts;
        x.offset = r.at;
      }
      const sp = d.speed || 1; // a piece with its own speed (0.1.41): footage = timeline x speed
      const limit = d.side === "end" ? r.src + r.len * sp >= d.hi - 1e-6 && want > r.at + r.len + 0.02 : r.src <= d.lo + 1e-6 && want < d.at0 - (d.src0 - d.lo) / sp - 0.02;
      host.classList.toggle("tl-limit", limit);
      const edgeT = d.side === "end" ? r.at + r.len : r.at;
      showTip(edgeT, limit ? t("No more footage") : `${tlFmtPrecise(toV(edgeT))} · ${tlFmtLen(r.len)} s`);
      changed("trim");
      // 0.1.46: the preview shows the frame under the handle, live (the new first frame, or the new last one).
      // Was: nothing seeked during the drag, so the picture kept the frame the playhead stood on before it.
      opts.onTrimPeek?.({ at: r.at, src: r.src, len: r.len }, d.side, sp);
      return;
    }
    if (d.type === "gap-edge") {
      // A gap's right edge (0.1.36): longer or shorter, what follows moves along. Live, from the state it began in.
      if (!d.snapped) { emSnapshot(m); d.snapped = true; }
      emRestore(m, d.base);
      let end = Math.max(d.at0, raw);
      if (!free) end = snapAt(end, [st.shownT, ...shownMarks()]).at;
      emSetGapLen(m, d.gap, end - d.at0);
      showTip(d.at0 + Math.max(0, end - d.at0), t("Gap {sec} s", { sec: tlFmtLen(Math.max(0, end - d.at0)) }));
      changed("gap");
      return;
    }
    if (d.type === "cut") {
      if (lockedCut(d.c)) return;
      if (!d.snapped) { emSnapshot(m); d.snapped = true; }
      const used = emMoveCut(m, d.c, snap(raw, { cut: d.c }));
      showTip(used);
      changed("cut", false);
      return;
    }
    if (d.type === "trim") {
      if (!d.snapped) { emSnapshot(m); d.snapped = true; }
      let sec = free ? raw : snapAt(raw, d.cands).at;
      emSegPaste(m, d.base);
      const used = d.side === "start" ? emSetTrimStart(m, sec) : emSetTrimEnd(m, sec);
      remapSels(d.selT);
      showTip(used, t("{time} · clip {len}", { time: tlFmtPrecise(toV(used)), len: tlFmtTick(emOutDuration(m), 0.5) }));
      changed("trim", false);
      return;
    }
    if (d.type.startsWith("ov-")) {
      const o = ovById(d.ovId);
      if (!o) return;
      if (!d.snapped) { emSnapshot(m); d.snapped = true; d.maxT = maxT(); }
      // From the item as it was when the drag began, so snapping never piles up.
      o.from = d.o0.from; o.to = d.o0.to;
      if (d.o0.off !== undefined) o.off = d.o0.off;
      if (d.o0.keys && Array.isArray(o.keys)) o.keys.forEach((q, i) => { if (q && Number.isFinite(d.o0.keys[i])) q.t = d.o0.keys[i]; });
      const skip = { ov: o.id };
      if (d.type === "ov-move") {
        const len = o.to - o.from;
        let from = Math.min(d.maxT - EM_OV_MIN, Math.max(0, d.o0.from + raw - d.grab));
        if (!free) {
          const c = snapCands(skip), s1 = snapAt(from, c), s2 = snapAt(from + len, c);
          if (s1.hit || s2.hit) from = Math.max(0, s1.dist <= s2.dist ? s1.at : s2.at - len);
        }
        emMoveOverlay(o, from - o.from);
        // Up or down onto another row: dropped there, it comes to the front or goes to the back (see onUp).
        const lane = laneAt(e.clientY);
        d.ovTo = lane?.kind === "ov" ? lane.k : lane ? -1 : d.ovRow;
        showTip(o.from);
      } else {
        const side = d.type === "ov-left" ? "left" : "right";
        emTrimOverlay(o, side, Math.min(d.maxT, snap(raw, skip)));
        showTip(side === "left" ? o.from : o.to, `${tlFmtPrecise(toV(side === "left" ? o.from : o.to))} · ${tlFmtLen(o.to - o.from)} s`);
      }
      ovChanged();
      opts.onChange?.("overlay");
      return;
    }
    let row = emRow(m, d.rowId);
    if (!row) return;
    if (!d.snapped) { emSnapshot(m); d.snapped = true; d.maxT = maxT(); }
    const r0 = d.row0;
    if (d.type === "move") {
      // An audio clip: sideways anywhere (also after the video), up or down onto any audio row (0.1.39: applied on
      // release; a video row says no and puts it back).
      const len = emRowEnd(m, row) - row.offset, hi = Math.max(0, d.maxT - EM_MIN_SEG);
      let o = Math.min(hi, Math.max(0, r0.offset + raw - d.grab));
      if (!free) {
        const c = snapCands({ row: d.rowId });
        const s1 = snapAt(o, c), s2 = snapAt(o + len, c);
        if (s1.hit || s2.hit) o = s1.dist <= s2.dist ? s1.at : s2.at - len;
        o = Math.min(hi, Math.max(0, o));
      }
      if (row.group) {
        // A member of a group (0.1.39): every member moves with it, live, by the same amount (the whole group stops
        // where one of them cannot go further), from the state the drag began in.
        emRestore(m, d.base);
        if (m.marks) m.marks = d.marks0.slice();
        row = emRow(m, d.rowId);
        groupMove(row.group, o - r0.offset);
        changed("file", true);
      } else emPlaceFile(m, d.rowId, { offset: o });
      // Near the bottom of the rows the view scrolls down, and the last 12 px of the last row (or the space under it)
      // make a new row.
      const bodyR = el.body.getBoundingClientRect();
      if (e.clientY > bodyR.bottom - 16 && el.body.scrollTop + el.body.clientHeight < el.body.scrollHeight) el.body.scrollTop += 8;
      const rowsR = el.rows.getBoundingClientRect(), last = st.lanes[st.lanes.length - 1];
      let L = laneAt(e.clientY);
      const below = (!L && e.clientY > rowsR.bottom - 1) || (L && L === last && e.clientY > L.el.getBoundingClientRect().bottom - 12) || e.clientY > Math.min(rowsR.bottom, bodyR.bottom) - 6;
      if (below) L = null;
      d.toLane = L ? (L.kind === "track" || L.kind === "lane" ? L.id : "no") : below ? "new" : d.toLane;
      for (const x of st.lanes) x.el.classList.toggle("tl-drop-to", d.toLane === x.id && x.id !== d.lane0);
      el.rows.classList.toggle("tl-drop-new", d.toLane === "new");
      host.classList.toggle("tl-limit", d.toLane === "no");
      showTip(row.offset, d.toLane === "no" ? t("Audio clips go on audio rows only") : undefined);
    } else {
      // An end (0.1.39: emTrimClip): inward it trims, outward it brings back sound as far as its file has any. The
      // same end of every member of its group that lies at the same time goes along.
      const side = d.type === "left" ? "start" : "end", at = snap(raw, { row: d.rowId });
      emRestore(m, d.base);
      if (m.marks) m.marks = d.marks0.slice();
      row = emRow(m, d.rowId);
      const edge0 = side === "start" ? r0.offset : r0.offset + r0.length;
      const r = emTrimClip(m, d.rowId, side, at);
      if (row.group) for (const x of m.audio) if (x !== row && x.kind === "file" && x.group === row.group && Math.abs((side === "start" ? x.offset : x.offset + x.length) - edge0) < 1e-3) emTrimClip(m, x.id, side, side === "start" ? r.at : r.at + r.len);
      if (row.group) emParts(m).forEach((p) => { if (m.parts[p.i]?.group === row.group && Math.abs((side === "start" ? p.at : p.end) - edge0) < 1e-3) emTrimPiece(m, p.i, side, side === "start" ? r.at : r.at + r.len); });
      const want = side === "start" ? at : at, limit = side === "start" ? want < r.at - 0.02 : want > r.at + r.len + 0.02;
      host.classList.toggle("tl-limit", limit);
      showTip(side === "start" ? r.at : r.at + r.len, limit ? t("No more footage") : undefined);
      changed("file");
      return;
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
    // Drawn only where the rows are in view (0.1.40): not over the ruler, which stays on top while the rows scroll.
    const vTop = el.body.scrollTop + tlRulerH, vBot = el.body.scrollTop + el.body.clientHeight;
    const top = Math.max(rowsTop, vTop, Math.min(d.by0, by1)), bot = Math.max(top, Math.min(rowsBot, vBot, Math.max(d.by0, by1)));
    clearTimeout(st.bandTimer);
    el.band.classList.remove("out");
    el.band.hidden = false;
    // Placed with left / top (0.1.40): with a translate, the fade's scale shrank it toward the lanes' top left, so on
    // release it slid away instead of shrinking in place like the Library's marquee.
    el.band.style.left = `${left}px`;
    el.band.style.top = `${top}px`;
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
    // 0.1.40: rows taller than the timeline: a rubber band (or a clip) dragged near the top or the bottom of the rows
    // scrolls them up or down, so rows out of view can be reached (before, the band stopped at the visible rows).
    let vy = 0;
    if (d.type === "band" || d.type === "move") {
      const br = el.body.getBoundingClientRect(), top = br.top + tlRulerH, y = e.clientY;
      if (y < top + 18 && el.body.scrollTop > 0) vy = -Math.ceil((top + 18 - y) / 2);
      else if (y > br.bottom - 18 && el.body.scrollTop + el.body.clientHeight < el.body.scrollHeight) vy = Math.ceil((y - (br.bottom - 18)) / 2);
    }
    if (!v && !vy) return;
    const before = st.scrollX, beforeY = el.body.scrollTop;
    if (v) setScroll(st.scrollX + Math.max(-40, Math.min(40, v)));
    if (vy) el.body.scrollTop += Math.max(-30, Math.min(30, vy));
    if (st.scrollX !== before || el.body.scrollTop !== beforeY) apply(d, e);
  }
  function onUp(e) {
    const d0 = st.drag;
    flushMove(d0); // the drop is where the mouse was last
    try { onUp0(e); } finally { tmpDone(d0); } // a selection moved together: its groups back (0.1.41)
  }
  function onUp0(e) {
    const d = st.drag;
    endDrag();
    if (!d) return;
    if (d.type === "seek" && d.at !== undefined) st.hold = { at: d.at, until: performance.now() + 500 };
    if (d.type === "part") { if (d.moved) doMovePart(d.from, d.to); else partClick(d.from); }
    if (d.type === "zkey") { if (!d.moved) seekExact(d.t0); else if (d.snapped && m.undo[m.undo.length - 1] === emState(m)) m.undo.pop(); }
    if (d.type === "clip" && d.snapped && d.base) { emRestore(m, d.base); if (m.marks) m.marks = d.marks0.slice(); m.undo.pop(); changed("order"); } // the live preview goes; the drop below is the one undo step
    if (d.type === "clip" && d.ghost) { if (d.ghost.mode === "up") doPieceUp(d.clip.i, d.ghost.from); else doPlaceClip(d.clip, d.ghost); }
    if ((d.type === "move" || d.type === "left" || d.type === "right") && d.snapped && d.rowId) {
      // An audio clip (0.1.39): onto the row it was dropped on (a video row: back where it was), its group along.
      for (const x of st.lanes) x.el.classList.remove("tl-drop-to");
      el.rows.classList.remove("tl-drop-new");
      const row = emRow(m, d.rowId);
      if (d.type === "move" && row) {
        if (d.toLane === "no") { emRestore(m, d.base); if (m.marks) m.marks = d.marks0.slice(); } // back where it (and its group) was
        else if (d.toLane && d.toLane !== d.lane0) emMoveClip(m, d.rowId, { lane: d.toLane });
      }
      if (m.undo[m.undo.length - 1] === emState(m)) m.undo.pop(); // moved out and back: no empty undo step
      else changed("file");
    }
    if (d.type === "piece-end" && d.snapped) {
      if (m.undo[m.undo.length - 1] === emState(m)) m.undo.pop(); // dragged out and back: no empty undo step
      else relaid("trim");
    }
    if (d.type === "gap-edge" && d.snapped) {
      if (m.undo[m.undo.length - 1] === emState(m)) m.undo.pop(); // dragged out and back: no empty undo step
      else relaid("gap");
    }
    if (d.type.startsWith("ov-")) {
      if (!d.moved) pickOv(d.ovId);
      else {
        // Dropped on a higher overlay row: to the front; lower (or on the video and audio rows): to the back. Part of the drag's undo step.
        const L = ovItems(), i = L.findIndex((o) => o.id === d.ovId);
        if (d.type === "ov-move" && i >= 0 && d.ovTo !== undefined && d.ovTo !== d.ovRow) {
          const [o] = L.splice(i, 1);
          if (d.ovTo > d.ovRow) L.push(o); else L.unshift(o);
          ovChanged();
        }
        if (d.snapped && m.undo[m.undo.length - 1] === emState(m)) m.undo.pop(); // moved out and back: no empty undo step
        else changed("overlay");
        if (st.ovSel !== d.ovId) pickOv(d.ovId);
      }
    }
    if (d.ovClear && !d.moved && st.ovSel) pickOv(null);
    if (!d.moved && d.sel !== undefined) clickSel(d);
    // A drag that ends outside the timeline must not count as a click on the viewer's backdrop.
    if (d.moved || !host.contains(e.target)) swallowClick();
  }
  /** A click on clip block `i`: its video pieces are selected and the playhead goes to its start. */
  function partClick(i) {
    const p = emParts(m)[i];
    if (!p) return;
    const list = emPartSegs(m, i).filter(segShown).map((seg) => ({ row: "video", seg }));
    setSels(list, list[0] || null);
    opts.onSeek?.(p.at);
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
    // A member of a group (0.1.39): the whole group.
    const all = s ? groupSels(s) : [];
    if (all.length > 1) return setSels(all, s);
    setSel(s);
  }
  function endDrag() {
    window.removeEventListener("mousemove", onMove);
    window.removeEventListener("mouseup", onUp);
    window.removeEventListener("blur", onLost);
    const d = st.drag;
    st.drag = null;
    if (d?.type === "trim") {
      // Dragged out and back to where it began: no empty undo step.
      if (d.snapped && m.undo[m.undo.length - 1] === emState(m)) m.undo.pop();
      // Cut parts hidden: the view catches up with what is cut out now.
      if (st.mapHold) { st.mapHold = null; markAll(); if (st.hideCuts) relayout(); }
      // The cut parts were shown only for this drag: hidden again, the bracket staying where it is on screen.
      if (d.rehide) {
        const now = emTrimBounds(m), at = d.side === "start" ? now.start : now.end, x = X(at);
        st.hideCuts = true;
        if (d.rehide.fitted) st.fitted = true;
        markAll();
        relayout();
        if (!st.fitted) setScroll(tlPad + toV(at) * st.pps - x);
        trimTips();
      }
      updateBar();
    }
    host.classList.remove("tl-dragging", "tl-limit");
    delete host.dataset.tlDrag;
    el.tip.hidden = true;
    el.pdrop.hidden = true;
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
    let cur = "", tip = null;
    if (st.tool === "blade") cur = "";
    else if (lane && lane.dataset.tlKind === "video") {
      // A clip can be dragged (0.1.36), a gap's right edge makes it longer or shorter.
      const x = e.clientX - el.lanes.getBoundingClientRect().left, p = pieceAtX("video", x), pe = pieceEndHit(x);
      cur = pe || gapHitRight(x) >= 0 ? "resize" : p && !segGap(p.seg) ? "grab" : "";
      // A piece's end (0.1.37): trim it, or pull back the footage that was cut away, while the source has any.
      const q = pe ? emPieceList(m)[pe.i] : null, none = q && (pe.side === "start" ? q.src <= q.lo + 1e-6 : q.src + q.flen >= q.hi - 1e-6);
      tip = pe ? `${pe.side === "start" ? t("Drag to trim the start, or out to bring back what was cut") : t("Drag to trim the end, or out to bring back what was cut")}${none ? "\n" + t("No more footage on this side") : ""}`
        : gapHitRight(x) >= 0 ? t("Drag to make the gap longer or shorter") : p && segGap(p.seg) ? t("A gap: black and silent. Click it, then Delete closes it.") : p ? t("Drag to move the clip. Over another clip it goes before or after it; anywhere else it sits where you drop it.") : "";
    } else if (lane && lane.dataset.tlKind === "parts") cur = "grab";
    else if (lane && (lane.dataset.tlKind === "track" || lane.dataset.tlKind === "lane")) {
      const rect = el.lanes.getBoundingClientRect();
      const ch = clipAtX(lane.dataset.tlLane, e.clientX - rect.left), row = ch?.row || null, hit = ch?.hit || null; // the audio clip under it (0.1.39)
      cur = hit === "move" ? "grab" : hit ? "resize" : "";
      // The block's tooltip says how to slip its sound (0.1.35).
      tip = hit ? `${tlRowName(row, opts.game)}\n${t("Click it, then turn the mouse wheel over it to move the sound inside it (Shift: finer, Ctrl: 1 s)")}\n${t("Drag it up or down onto any audio row.")}` : "";
    } else if (lane && lane.dataset.tlKind === "ov") {
      const rect = el.lanes.getBoundingClientRect();
      const hit = ovAtX(+lane.dataset.tlLane.slice(2), e.clientX - rect.left);
      cur = hit?.hit === "move" ? "grab" : hit ? "resize" : "";
      tip = hit ? `${ovLabel(hit.o)}\n${t("Drag to move, drag an end to trim. Right-click for more.")}` : "";
    }
    if (el.lanes.dataset.tlCur !== cur) el.lanes.dataset.tlCur = cur;
    if (tip !== null && lane.title !== tip) lane.title = tip;
  }
  function onWheel(e) {
    const rect = el.lanes.getBoundingClientRect();
    const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? st.laneW : 1;
    // Over a selected file block (0.1.35): the wheel slips its sound (emSlipFile); the block stays where it is.
    const lane = e.target.closest?.("[data-tl-lane]");
    const row = lane && (lane.dataset.tlKind === "track" || lane.dataset.tlKind === "lane") && !st.drag ? clipAtX(lane.dataset.tlLane, e.clientX - rect.left)?.row || null : null;
    if (row && st.sels.some((s) => s.row === row.id) && fileHit(row, e.clientX - rect.left)) {
      e.preventDefault();
      const dy = e.deltaY || e.deltaX; // Shift turns a wheel's deltaY into deltaX
      const step = tlSlipStep(dy, e.shiftKey, e.ctrlKey || e.metaKey);
      if (!step) return;
      // One undo step per burst of turns.
      if (st.slip?.id !== row.id) { if (st.slip) clearTimeout(st.slip.timer); emSnapshot(m); st.slip = { id: row.id, timer: 0, snap: m.undo.length }; }
      clearTimeout(st.slip.timer);
      st.slip.timer = setTimeout(() => {
        const s = st.slip;
        st.slip = null;
        if (s && m.undo.length === s.snap && m.undo[m.undo.length - 1] === emState(m)) m.undo.pop(); // it moved nothing
      }, 700);
      if (emSlipFile(m, row.id, step)) {
        changed("file");
        const at = Math.round(row.trimStart * 100) / 100;
        showTip(row.offset, t("Sound from {time} of the file", { time: tlFmtPrecise(at) }));
        clearTimeout(st.slipTip);
        st.slipTip = setTimeout(() => { if (!st.drag) el.tip.hidden = true; }, 900);
      }
      return;
    }
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
    // An overlay block has its own menu (0.1.35).
    const ovLane = e.target.closest("[data-tl-lane]");
    if (ovLane?.dataset.tlKind === "ov") {
      const hit = ovAtX(+ovLane.dataset.tlLane.slice(2), x);
      if (hit) {
        pickOv(hit.o.id);
        openMenu(ovMenuHtml(hit.o), e.clientX, e.clientY, (act) => ovAct(act, hit.o.id));
      }
      return;
    }
    // A clip block has its own menu.
    if (e.target.closest("[data-tl-lane]")?.dataset.tlLane === "parts") {
      const i = partAtX(x);
      if (i >= 0) openMenu(partMenuHtml(i), e.clientX, e.clientY, (act) => partAct(act, i));
      return;
    }
    const cutEl = e.target.closest("[data-tl-cut]"), jointEl = e.target.closest("[data-tl-joint]"), bmEl = e.target.closest("[data-tl-bm]");
    const trimEl = cutEl ? null : e.target.closest("[data-tl-trim]");
    const at = timeAtX(x);
    // The bookmark under the pointer: its flag, one within tlSnapPx, or one too close to add another (when zoomed far in).
    let mark = bmEl ? +bmEl.dataset.tlBm : markAtX(x);
    if (mark < 0) { const near = emMarkNear(m, at, EM_MARK_GAP); if (near >= 0 && markShown(marks()[near])) mark = near; }
    const c = {
      at, mark, cut: cutEl ? +cutEl.dataset.tlCut : -1, joint: jointEl ? st.joints[+jointEl.dataset.tlJoint] || null : null,
      trim: trimEl ? (trimEl.dataset.tlTrim === "end" ? "end" : "start") : null,
      splitAt: e.altKey ? at : snapAt(at, shownMarks()).at, // "Split here" snaps to a bookmark (Alt: not)
    };
    const lane = !cutEl && !jointEl && !trimEl ? e.target.closest("[data-tl-lane]") : null;
    if (lane) {
      const p = pieceAtX(lane.dataset.tlLane, x);
      if (p && !tlSelHas(st.sels, p)) setSel(p); // the piece under the pointer becomes the selection
      c.video = lane.dataset.tlLane === "video" && !!p && !segGap(p.seg);
    }
    openMenu(menuHtml(c), e.clientX, e.clientY, (act, arg) => menuAct(act, arg, c));
  }
  function menuHtml(c) {
    const item = (act, path, label, { arg = "", on = null } = {}) =>
      `<button type="button" role="${on === null ? "menuitem" : "menuitemcheckbox"}"${on === null ? "" : ` aria-checked="${on}"`} data-tl-m="${act}" data-tl-arg="${esc(String(arg))}" class="${on ? "on" : ""}">${tlSvg(path)}<span>${esc(label)}</span></button>`;
    const out = [], top = [], mk = [];
    // Bookmarks first: they are about the moment under the pointer, not the selection.
    if (c.mark >= 0 && c.mark < marks().length) mk.push(item("mark-remove", tlPaths.markOff, t("Remove this bookmark")));
    else if (c.at >= 0 && c.at <= dur()) mk.push(item("mark-add", tlPaths.mark, t("Add a bookmark here")));
    const now = +opts.getTime() || 0, vis = { marks: shownMarks() };
    if (emNextMark(vis, now) !== null) mk.push(item("mark-next", tlPaths.markNext, t("Go to the next bookmark")));
    if (emPrevMark(vis, now) !== null) mk.push(item("mark-prev", tlPaths.markPrev, t("Go to the previous bookmark")));
    if (marks().length >= 2) mk.push(item("mark-clear", tlPaths.trash, t("Remove all bookmarks")));
    out.push(...mk);
    if (mk.length) out.push("<hr>");
    const n = st.sels.length;
    if (n > 1) out.push(`<div class="menu-head">${esc(t("{n} selected", { n }))}</div>`);
    // On a bracket: give back all the footage on its side (only when there is some).
    if (c.trim === "start" && trimWould("start", 0)) top.push(item("trim-reset", tlPaths.restore, t("Reset the start"), { arg: "start" }));
    if (c.trim === "end" && trimWould("end", dur())) top.push(item("trim-reset", tlPaths.restore, t("Reset the end"), { arg: "end" }));
    if (c.cut >= 0 && c.cut < m.cuts.length && !lockedCut(c.cut)) top.push(item("join", tlPaths.join, t("Remove this cut point"), { arg: c.cut }));
    if (c.joint) top.push(item("restore-run", tlPaths.restore, t("Restore the part cut out here")));
    if (canSplitAt(c.splitAt)) top.push(item("split-at", tlPaths.split, t("Split here"), { arg: c.splitAt }));
    const plan = removePlan();
    if (plan.ok) top.push(item("remove", plan.restore ? tlPaths.restore : tlPaths.trash, plan.restore ? t("Restore") : t("Cut out")));
    // Ungroup / Group (0.1.39), with their keys.
    if (st.sels.length) top.push(item("ungroup", tlPaths.ungroup, keyTip(t("Ungroup"), "ungroup", t("Ungroup (U)"))));
    if (st.sels.filter((s) => s.row === "video" || isClip(s.row)).length >= 2) top.push(item("group", tlPaths.group, keyTip(t("Group"), "group", t("Group (G)"))));
    // Gaps on the main video row (0.1.36).
    const vids = st.sels.filter((s) => s.row === "video");
    if (vids.some((s) => !segGap(s.seg)) && m.removed.some((r, i) => !r && !segGap(i) && !vids.some((s) => s.seg === i)))
      top.push(item("lift", tlPaths.lift, t("Cut out and leave a gap")));
    if (selGaps().length) top.push(item("close-gap", tlPaths.gapClose, t("Close gap")));
    if (emHasGaps(m)) top.push(item("close-gaps", tlPaths.gapClose, t("Close all gaps")));
    // The mouse's I and O: at the pointer (snapped to a bookmark like "Split here"). Not on a bracket, where it would hardly move it.
    if (!c.trim && trimWould("start", c.splitAt)) top.push(item("trim-start", tlPaths.trimStart, t("Trim the start to here"), { arg: c.splitAt }));
    if (!c.trim && trimWould("end", c.splitAt)) top.push(item("trim-end", tlPaths.trimEnd, t("Trim the end to here"), { arg: c.splitAt }));
    const aud = selAudio();
    if (aud.length) {
      const ref = refAudio(), g = levelOf(ref), many = aud.length > 1;
      const label = g === 0 ? (many ? t("Unmute these pieces") : t("Unmute this piece")) : many ? t("Mute these pieces") : t("Mute this piece");
      top.push(item("mute", g === 0 ? tlPaths.vol : tlPaths.muted, label));
      const chips = [0.25, 0.5, 1, 1.5, 2].map((v) => {
        const on = Math.abs(g - v) < 0.005;
        return `<button type="button" role="menuitemradio" aria-checked="${on}" data-tl-m="level" data-tl-arg="${v}" class="${on ? "on" : ""}">${esc(t("{pct}%", { pct: Math.round(v * 100) }))}</button>`;
      }).join("");
      top.push(`<div class="tl-mlevel" role="group" aria-label="${esc(t("Level"))}">${tlSvg(tlPaths.level)}<span>${esc(t("Level"))}</span><div class="tl-mchips">${chips}</div></div>`);
      if (m.audio.length > 1 && soloGroups().length) top.push(item("solo", tlPaths.solo, allSolo() ? t("Unsolo") : many ? t("Solo these pieces") : t("Solo this piece")));
    }
    if (st.sels.some((s) => s.row === "video" && !segGap(s.seg))) { // a gap has no picture to hide (0.1.36)
      const segs = blankSegs(), hidden = segs.length > 0 && !!m.blank[segs[0]];
      top.push(item("blank", hidden ? tlPaths.eye : tlPaths.eyeOff, hidden ? t("Show the video here") : t("Hide the video here")));
    }
    if (c.video && typeof cvApi()?.crop === "function") top.push(item("cv-crop", tlPaths.crop, t("Crop to a part of the picture")));
    // Speed of the selected pieces (0.1.41): the same choices as the inspector.
    const spl = speedPieces();
    if (spl.length) {
      const cur = emPieceSpeed(m, spl[0]);
      const chips = TL_SPEEDS.map((v) => {
        const on = Math.abs(cur - v) < 0.005;
        return `<button type="button" role="menuitemradio" aria-checked="${on}" data-tl-m="speed" data-tl-arg="${v}" class="${on ? "on" : ""}">${esc(tlSpeedText(v))}</button>`;
      }).join("");
      top.push(`<div class="tl-mlevel" role="group" aria-label="${esc(t("Speed"))}">${tlSvg(tlPaths.level)}<span>${esc(t("Speed"))}</span><div class="tl-mchips">${chips}</div></div>`);
    }
    out.push(...top);
    if (top.length) out.push("<hr>");
    out.push(item("names", tlPaths.names, t("Names on the audio"), { on: st.names }));
    // Rows by role (0.1.44): the switch, and the offer while the rows are by title.
    const ri = rowsInfo();
    if (ri?.ready) {
      if (ri.mode === "titles" && ri.offer) out.push(item("rows-simplify", tlPaths.level, t("Simplify the audio rows")));
      out.push(item("rows-mode", tlPaths.level, t("Show every track"), { on: ri.mode === "titles" }));
    }
    out.push(item("select-all", tlPaths.selAll, t("Select all pieces")), item("fit", tlPaths.fit, t("Fit the whole clip")));
    if (typeof opts.onCommand === "function") out.push("<hr>", item("shortcuts", tlPaths.keys, t("Shortcuts…")), item("fullscreen", tlPaths.full, t("Fullscreen")));
    return out.join("");
  }
  /** The menu of an overlay block: Crop (media), Duplicate, Bring to front, Send to back, Delete. An entry whose
   *  function the overlay code does not have is left out (Delete and the order work without it). */
  function ovMenuHtml(o) {
    const item = (act, path, label) => `<button type="button" role="menuitem" data-tl-m="${act}" data-tl-arg="">${tlSvg(path)}<span>${esc(label)}</span></button>`;
    const api = ovApi(), L = ovItems(), i = L.indexOf(o), out = [`<div class="menu-head">${esc(ovLabel(o))}</div>`];
    if (isMedia(o) && typeof api?.crop === "function") out.push(item("ov-crop", tlPaths.crop, t("Crop")));
    if (typeof api?.duplicate === "function") out.push(item("ov-dup", tlPaths.dup, t("Duplicate")));
    if (i < L.length - 1) out.push(item("ov-front", tlPaths.front, t("Bring to front")));
    if (i > 0) out.push(item("ov-back", tlPaths.back, t("Send to back")));
    out.push("<hr>", item("ov-del", tlPaths.trash, t("Delete")));
    return out.join("");
  }
  function ovAct(act, id) {
    const api = ovApi();
    switch (act) {
      case "ov-crop": try { api?.crop?.(id); } catch (e) { console.error("Glint timeline: ov.crop failed", e); } return true;
      case "ov-dup": {
        let nid = null;
        try { nid = api?.duplicate?.(id); } catch (e) { console.error("Glint timeline: ov.duplicate failed", e); }
        ovChanged();
        if (nid && ovById(nid)) pickOv(nid);
        return !!nid;
      }
      case "ov-front": return doOvOrder(id, true);
      case "ov-back": return doOvOrder(id, false);
      case "ov-del": return doOvRemove(id);
    }
    return false;
  }
  /** The menu of clip block `i`: move it, cut it out or put it back (both when it is partly cut out). */
  function partMenuHtml(i) {
    const P = emParts(m), segs = emPartSegs(m, i);
    const item = (act, path, label) => `<button type="button" role="menuitem" data-tl-m="${act}" data-tl-arg="">${tlSvg(path)}<span>${esc(label)}</span></button>`;
    const out = [`<div class="menu-head">${esc(P[i]?.title || "")}</div>`];
    if (i > 0) out.push(item("part-left", tlPaths.partLeft, t("Move left")));
    if (i < P.length - 1) out.push(item("part-right", tlPaths.partRight, t("Move right")));
    const others = m.removed.some((r, k) => !r && !segs.includes(k)); // something else stays
    if (segs.some((k) => !m.removed[k]) && others) out.push(item("part-cut", tlPaths.trash, t("Cut this clip out")));
    if (segs.some((k) => m.removed[k])) out.push(item("part-restore", tlPaths.restore, t("Put this clip back")));
    return out.join("");
  }
  function partAct(act, i) {
    switch (act) {
      case "part-left": return doMovePart(i, i - 1);
      case "part-right": return doMovePart(i, i + 1);
      case "part-cut": return doPartRemoved(i, true);
      case "part-restore": return doPartRemoved(i, false);
    }
    return false;
  }
  function menuAct(act, arg, c) {
    switch (act) {
      case "mark-add": return doAddMark(c.at);
      case "mark-remove": return doRemoveMark(c.mark);
      case "mark-next": return doGoMark(false);
      case "mark-prev": return doGoMark(true);
      case "mark-clear": return doClearMarks();
      case "join": return doJoin(+arg);
      case "restore-run": return doRestoreRun(c.joint);
      case "split-at": return doSplit(+arg);
      case "trim-reset": return arg === "end" ? doTrimAt("end", dur()) : doTrimAt("start", 0);
      case "trim-start": return doTrimAt("start", +arg);
      case "trim-end": return doTrimAt("end", +arg);
      case "remove": return doRemove();
      case "lift": return doLift();
      case "ungroup": return doUngroup();
      case "group": return doGroup();
      case "close-gap": return doCloseGaps();
      case "close-gaps": return doCloseAllGaps();
      case "mute": return doMuteSeg();
      case "level": return doLevel(+arg);
      case "solo": return doSolo();
      case "blank": return doBlank();
      case "speed": return doSpeed(+arg);
      case "cv-crop": try { cvApi()?.crop?.(); } catch (e) { console.error("Glint timeline: cv.crop failed", e); } return true;
      case "hide-cuts": return setHideCuts(!st.hideCuts);
      case "names": return setNames(!st.names);
      case "rows-simplify": try { opts.rowsApi?.simplify?.(); } catch (e) { console.error("Glint timeline: simplify failed", e); } updateBar(); return true;
      case "rows-mode": { const i = rowsInfo(); if (i?.ready) { try { opts.rowsApi?.everyTrack?.(i.mode !== "titles"); } catch (e) { console.error("Glint timeline: rows mode failed", e); } } updateBar(); return true; }
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
      // Rows by role (0.1.44): the editor does them (it knows the clips' tracks) and reports back through onChange.
      case "rows-simplify": try { opts.rowsApi?.simplify?.(); } catch (e) { console.error("Glint timeline: simplify failed", e); } updateBar(); break;
      case "rows-dismiss": try { opts.rowsApi?.dismiss?.(); } catch (e) { console.error(e); } updateBar(); break;
      case "rows-mode": { const i = rowsInfo(); if (i?.ready) { try { opts.rowsApi?.everyTrack?.(i.mode !== "titles"); } catch (e) { console.error("Glint timeline: rows mode failed", e); } } updateBar(); break; }
      case "remove": doRemoveOrClose(); break;
      case "tool-select": setTool("select"); break;
      case "tool-blade": setTool("blade"); break;
      case "mute": doMuteSeg(); break;
      case "undo": doUndo(false); break;
      case "redo": doUndo(true); break;
      case "zoom-out": zoom(0.8); break;
      case "zoom-in": zoom(1.25); break;
      case "fit": fit(); break;
      case "hide-cuts": setHideCuts(!st.hideCuts); break;
      case "names": setNames(!st.names); break;
      case "add": opts.onAddAudio?.(); break;
      case "row-mute": {
        const row = emLaneOf(m, id); // a row: its own sound and every clip on it (0.1.39)
        if (!row) return;
        emSnapshot(m);
        row.muted = !row.muted; // the model has no setter for this; see the report
        changed("mute");
        break;
      }
      case "row-through": {
        // An added file: play on through every cut, or be cut with the video (emSetThrough).
        const row = emRow(m, id);
        if (!row) return;
        emSnapshot(m);
        if (!emSetThrough(m, id, !row.through)) { m.undo.pop(); return; }
        changed("file");
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
      const row = emLaneOf(m, volId);
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
    growHost(); // the toolbar may wrap onto another line, or rows came or went
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
  el.grip.addEventListener("mousedown", onGripDown);
  el.grip.addEventListener("dblclick", onGripDbl);
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
      barTips(); // the shortcuts may have been changed
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
    /** The overlay code selected item `id` (null: none): its block lights up (0.1.35). Does not call onOverlayPick back. */
    overlaySel(id) {
      if (st.dead) return;
      const next = id && ovById(id) ? id : null;
      if (next && st.sels.length) setSels([], null);
      if (next !== st.ovSel) { st.ovSel = next; markAll(); }
    },
    overlaySelected: () => st.ovSel,
    /** Speed per piece (0.1.41): the selected pieces on V1 ({ n, speed } of the first; n 0 = none), and set theirs. */
    pieceSpeed() { const L = st.dead ? [] : speedPieces(); return { n: L.length, speed: L.length ? emPieceSpeed(m, L[0]) : 1 }; },
    setPieceSpeed(v) { return !st.dead && doSpeed(+v); },
    /** Move the selection by `dt` seconds, together (Alt+Left / Alt+Right, 0.1.41). */
    nudgeSel(dt) { return !st.dead && !st.drag && nudgeSel(+dt); },
    /** The pointer's tool: "select" or "blade" (0.1.36). */
    tool: () => st.tool,
    setTool(name) { if (!st.dead) setTool(name); },
    /** The timeline second under client x `clientX` if it is over the lanes (for a dropped file), else null. */
    timeAtClient(clientX, clientY) {
      if (st.dead || !st.laneW) return null;
      const r = el.lanes.getBoundingClientRect();
      if (!(clientX >= r.left && clientX <= r.right && clientY >= r.top && clientY <= r.bottom)) return null;
      return Math.min(maxT(), Math.max(0, timeAtX(clientX - r.left)));
    },
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
        case "remove": return st.ovSel && ovById(st.ovSel) ? doOvRemove(st.ovSel) : doRemoveOrClose(); // a selected overlay block first; a gap closes
        case "lift": return doLift();
        case "ungroup": return doUngroup();
        case "group": return doGroup();
        case "blade": return setTool(st.tool === "blade" ? "select" : "blade");
        case "mute": return doMuteSeg();
        case "louder": return doNudge(0.1);
        case "quieter": return doNudge(-0.1);
        case "level": return arg !== null && arg !== "" && Number.isFinite(+arg) ? doLevel(+arg) : false;
        case "solo": return doSolo();
        case "blank": return doBlank();
        case "selectAll": return selectAll();
        case "clearSel": { const had = st.sels.length > 0; setSels([], null); return had; }
        case "hideCuts": return setHideCuts(!st.hideCuts);
        case "mark": { // on the playhead's bookmark: remove it, else add one
          const now = +opts.getTime() || 0, i = emMarkNear(m, now, EM_MARK_GAP);
          return i >= 0 ? doRemoveMark(i) : doAddMark(now);
        }
        case "markNext": return doGoMark(false);
        case "markPrev": return doGoMark(true);
        case "trimStart": return doTrimAt("start", +opts.getTime() || 0); // I: cut out everything before the playhead (or give it back up to there)
        case "trimEnd": return doTrimAt("end", +opts.getTime() || 0);     // O: the same from the playhead on
      }
      console.warn("Glint timeline: run() does not know the command", cmd);
      return false;
    },
    destroy() {
      if (st.dead) return;
      st.dead = true;
      cancelAnimationFrame(st.raf);
      if (st.slip) clearTimeout(st.slip.timer);
      clearTimeout(st.slipTip);
      closeMenu();
      endDrag();
      endGrip();
      unswallow();
      ro.disconnect();
      mo.disconnect();
      el.lanes.removeEventListener("mousedown", onDown);
      el.lanes.removeEventListener("mousemove", onHover);
      el.lanes.removeEventListener("wheel", onWheel);
      el.lanes.removeEventListener("dblclick", onDbl);
      el.lanes.removeEventListener("contextmenu", onCtx);
      el.scroll.removeEventListener("scroll", onScroll);
      el.grip.removeEventListener("mousedown", onGripDown);
      el.grip.removeEventListener("dblclick", onGripDbl);
      host.removeEventListener("click", onClick);
      host.removeEventListener("input", onInput);
      host.removeEventListener("change", onChangeEv);
      for (const x of st.thumbs) { x.img.onload = x.img.onerror = null; x.img.src = ""; }
      st.thumbs = [];
      st.waves.clear();
      host.innerHTML = "";
      host.classList.remove("tl", "tl-dragging");
      host.style.removeProperty("--tl-want");
    },
  };
}

// Lets the pure helpers be tested under Node; ignored in the app.
if (typeof module !== "undefined" && module.exports) {
  module.exports = { tlRowScale, tlRowKParse, tlPickStep, tlFmtTick, tlFmtPrecise, tlSnap, tlSnapV, tlFileHit, tlSpanHit, tlTrimLeft, tlPeakMax, tlHue, tlViewMap, tlSrcToView, tlViewToSrc, tlJoints, tlSelKey, tlSelHas, tlSelMerge, tlSelToggle, tlSelRect, tlBandHits, tlSelShift, tlFmtLen, tlPartSlot, tlOvRows, tlViewSpan, tlSlipStep, tlDropAt, tlSelSpans };
}
