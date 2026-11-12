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
// Times are source seconds. Lane x of a time: tlPad + sec * pps - scrollX.
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
function tlFileHit(row, sec, pps) {
  const a = row.offset, b = row.offset + row.length;
  const edge = Math.min(tlEdgePx, ((b - a) * pps) / 3) / pps;
  if (Math.abs(sec - a) <= edge) return "left";
  if (Math.abs(sec - b) <= edge) return "right";
  if (sec > a && sec < b) return "move";
  return null;
}

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
    sel: null,             // { row, seg }
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
      <span class="tl-sep"></span>
      <button type="button" class="btn sm tl-add" data-tl-act="add" title="${esc(t("Add an audio file as a new track"))}">${ICON.plus}<span>${esc(t("Add audio"))}</span></button>
    </div>
    <div class="tl-body">
      <div class="tl-heads"><div class="tl-corner"></div><div class="tl-hrows"></div></div>
      <div class="tl-lanes">
        <div class="tl-ruler" data-tl-ruler title="${esc(t("Click or drag to move the playhead"))}"><canvas></canvas><div class="tl-ph-k" data-tl-ph></div></div>
        <div class="tl-rows"></div>
        <div class="tl-marks"><div class="tl-cuts"></div><div class="tl-ph"></div></div>
        <div class="tl-tip" hidden></div>
      </div>
    </div>
    <div class="tl-sbar"><div></div><div class="tl-scroll"><div class="tl-spacer"></div></div></div>`;
  const q = (sel) => host.querySelector(sel);
  const el = {
    bar: q(".tl-bar"), hrows: q(".tl-hrows"), lanes: q(".tl-lanes"), rows: q(".tl-rows"),
    ruler: q(".tl-ruler canvas"), phK: q(".tl-ph-k"), ph: q(".tl-ph"), cuts: q(".tl-cuts"), tip: q(".tl-tip"),
    scroll: q(".tl-scroll"), spacer: q(".tl-spacer"),
    level: q("[data-tl-level]"), levelPct: q(".tl-level .tl-pct"), levelBox: q(".tl-level"),
    bSplit: q('[data-tl-act="split"]'), bRemove: q('[data-tl-act="remove"]'), bMute: q('[data-tl-act="mute"]'),
    bUndo: q('[data-tl-act="undo"]'), bRedo: q('[data-tl-act="redo"]'), bOut: q('[data-tl-act="zoom-out"]'), bIn: q('[data-tl-act="zoom-in"]'),
  };
  const rulerCtx = el.ruler.getContext("2d");

  /* ----- mapping ----- */
  const X = (sec) => tlPad + sec * st.pps - st.scrollX;
  const timeAtX = (x) => (x + st.scrollX - tlPad) / st.pps;
  const contentW = () => dur() * st.pps + 2 * tlPad;
  const minPps = () => (st.laneW > 2 * tlPad + 10 && dur() > 0 ? (st.laneW - 2 * tlPad) / dur() : 1);
  const maxPps = () => Math.max(tlMaxPps, minPps());
  const markAll = () => { st.dirtyAll = true; st.dirtyRuler = true; };

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
    const at = timeAtX(x);
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
    setScroll(tlPad + st.shownT * st.pps - st.laneW / 2);
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
  function normSel(sel) {
    if (!sel) return null;
    if (sel.row !== "video" && !emRow(m, sel.row)) return null;
    const n = m.removed.length;
    const seg = Math.min(n - 1, Math.max(0, Math.floor(+sel.seg || 0)));
    return { row: sel.row, seg };
  }
  function setSel(sel) {
    const next = normSel(sel);
    if (sameSel(next, st.sel)) return;
    st.sel = next;
    markAll();
    updateBar();
    opts.onSelect?.(next ? { ...next } : null);
  }
  const validateSel = () => setSel(st.sel);

  /* ----- toolbar ----- */
  const canSplitAt = (sec) => {
    if (!(sec > 0 && sec < dur())) return false;
    const seg = emSegments(m)[emSegmentAt(m, sec)];
    return !!seg && sec - seg.start >= EM_MIN_SEG && seg.end - sec >= EM_MIN_SEG;
  };
  const selRow = () => (st.sel && st.sel.row !== "video" ? emRow(m, st.sel.row) : null);
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
    const s = st.sel;
    const removed = s ? !!m.removed[s.seg] : false;
    const kept = m.removed.filter((r) => !r).length;
    setBtn(el.bRemove, "remove", !s || (!removed && kept <= 1), tlSvg(removed ? tlPaths.restore : "M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"),
      removed ? t("Restore segment (Del)") : t("Delete segment (Del)"));
    const row = selRow();
    const gain = row ? row.gains[s.seg] ?? 1 : 1;
    setBtn(el.bMute, "mute", !row, gain === 0 ? PICON.mute : PICON.vol, row && gain === 0 ? t("Unmute segment (M)") : t("Mute segment (M)"));
    el.bMute.classList.toggle("on", !!row && gain === 0);
    el.level.disabled = !row;
    el.levelBox.classList.toggle("off", !row);
    const pct = Math.round(gain * 100);
    if (st.gest?.el !== el.level && +el.level.value !== pct) el.level.value = pct;
    el.levelPct.textContent = row ? t("{pct}%", { pct }) : "";
    el.bOut.disabled = st.pps <= minPps() * 1.0001;
    el.bIn.disabled = st.pps >= maxPps() * 0.9999;
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
    updateBar();
    opts.onChange?.(what);
  }
  function doSplit() {
    const at = st.shownT;
    if (!canSplitAt(at)) return;
    emSnapshot(m);
    const c = emSplit(m, at);
    if (c < 0) throw new Error(`timeline: emSplit refused ${at} although canSplitAt allowed it`);
    if (st.sel && st.sel.seg > c) st.sel = { ...st.sel, seg: st.sel.seg + 1 };
    changed("split");
  }
  function doJoin(c) {
    if (c < 0 || c >= m.cuts.length) return;
    emSnapshot(m);
    emJoin(m, c);
    if (st.sel && st.sel.seg > c) st.sel = { ...st.sel, seg: st.sel.seg - 1 };
    changed("join");
  }
  function doRemove() {
    const s = st.sel;
    if (!s) return;
    const was = !!m.removed[s.seg];
    if (!was && m.removed.filter((r) => !r).length <= 1) return;
    emSnapshot(m);
    if (!emSetRemoved(m, s.seg, !was)) throw new Error(`timeline: emSetRemoved(${s.seg}, ${!was}) refused`);
    changed(was ? "restore" : "remove");
  }
  function doMuteSeg() {
    const row = selRow();
    if (!row) return;
    const g = row.gains[st.sel.seg];
    emSnapshot(m);
    emSetGain(m, row.id, st.sel.seg, g > 0 ? 0 : 1);
    changed("mute");
  }
  function doUndo(redo) {
    if (!(redo ? emRedo(m) : emUndo(m))) return;
    changed(redo ? "redo" : "undo");
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

  function drawRuler(C) {
    const ctx = rulerCtx, w = st.laneW, h = tlRulerH;
    prep(ctx, h);
    ctx.fillStyle = C.panel;
    ctx.fillRect(0, 0, w, h);
    const end = X(dur());
    if (end < w) { ctx.fillStyle = C.bg; ctx.globalAlpha = 0.6; ctx.fillRect(Math.max(0, end), 0, w - Math.max(0, end), h); ctx.globalAlpha = 1; }
    const x0 = X(0);
    if (x0 > 0) { ctx.fillStyle = C.bg; ctx.globalAlpha = 0.6; ctx.fillRect(0, 0, x0, h); ctx.globalAlpha = 1; }
    const { major, minor } = tlPickStep(st.pps);
    const t0 = Math.max(0, timeAtX(-80)), t1 = Math.min(dur(), timeAtX(w + 4));
    ctx.fillStyle = C.dim;
    if (minor < major) {
      for (let k = Math.ceil(t0 / minor - 1e-9); k * minor <= t1 + 1e-9; k++) {
        const sec = k * minor;
        if (Math.abs(Math.round(sec / major) * major - sec) < 1e-6) continue;
        ctx.fillRect(Math.round(X(sec)) - 0.5, h - 6, 1, 6);
      }
    }
    ctx.font = `500 11px ${C.sans}`;
    ctx.textBaseline = "middle";
    for (let k = Math.ceil(t0 / major - 1e-9); k * major <= t1 + 1e-9; k++) {
      const sec = Math.round(k * major * 1000) / 1000;
      const x = Math.round(X(sec)) - 0.5;
      ctx.fillStyle = C.muted;
      ctx.fillRect(x, h - 11, 1, 11);
      ctx.fillText(tlFmtTick(sec, major), x + 4, 10);
    }
    ctx.fillStyle = C.line;
    ctx.fillRect(0, h - 1, w, 1);
  }

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
    // Segment borders inside the bar.
    ctx.fillStyle = C.bg;
    for (const c of m.cuts) {
      if (c <= a || c >= b) continue;
      const x = X(c);
      if (x > -2 && x < st.laneW + 2) ctx.fillRect(Math.round(x) - 1, y, 2, bh);
    }
    return segs;
  }
  function selOutline(ctx, C, a, b, y, bh, rowId) {
    if (!st.sel || st.sel.row !== rowId) return;
    const s = emSegments(m)[st.sel.seg];
    if (!s) return;
    const sp = span(Math.max(a, s.start), Math.min(b, s.end), 4);
    if (!sp || sp[1] <= sp[0]) return;
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
        const mid = ((k + 0.5) * tw) / st.pps;
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
        const s0 = timeAtX(x), s1 = timeAtX(x + tlBarStep), sc = (s0 + s1) / 2;
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
      const vis = x > -8 && x < st.laneW + 8;
      c.style.transform = `translateX(${vis ? x : -100}px)`;
      c.classList.toggle("drag", st.drag?.type === "cut" && st.drag.moved && st.drag.c === i);
    }
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
    if (wasIn && !jump && x > st.laneW - 4) setScroll(tlPad + now * st.pps - 24); // ran off the right edge: next page
    else if (wasIn || jump) setScroll(tlPad + now * st.pps - st.laneW / 3);
  }

  function frame() {
    st.raf = requestAnimationFrame(frame);
    if (window.devicePixelRatio !== st.dpr) { st.dpr = window.devicePixelRatio || 1; st.needSize = true; }
    if (st.needSize && st.laneW) applySize();
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
  function showTip(sec, text) {
    el.tip.hidden = false;
    el.tip.textContent = text || tlFmtPrecise(sec);
    el.tip.style.transform = `translateX(${Math.min(st.laneW - 30, Math.max(30, X(sec)))}px) translateX(-50%)`;
  }

  function onDown(e) {
    if (e.button !== 0 || st.drag) return;
    const rect = el.lanes.getBoundingClientRect();
    const x = e.clientX - rect.left, at = timeAtX(x);
    const d = { x0: e.clientX, rect, moved: false, snapped: false, lastE: e };
    const cut = e.target.closest("[data-tl-cut]");
    const lane = e.target.closest("[data-tl-lane]");
    if (cut) Object.assign(d, { type: "cut", c: +cut.dataset.tlCut });
    else if (e.target.closest("[data-tl-ph], [data-tl-ruler]")) Object.assign(d, { type: "seek", moved: true });
    else if (lane) {
      const id = lane.dataset.tlLane, row = id === "video" ? null : emRow(m, id);
      const hit = row?.kind === "file" ? tlFileHit(row, at, st.pps) : null;
      const segAt = at >= 0 && at <= dur() ? emSegmentAt(m, Math.min(at, dur() - 1e-6)) : -1;
      const sel = row?.kind === "file" ? (hit ? { row: id, seg: segAt } : null) : segAt >= 0 ? { row: id, seg: segAt } : null;
      if (hit) Object.assign(d, { type: hit, rowId: id, row0: { offset: row.offset, trimStart: row.trimStart, length: row.length }, grab: at, sel });
      else Object.assign(d, { type: "click", sel });
    } else return;
    e.preventDefault();
    st.drag = d;
    host.classList.add("tl-dragging");
    host.dataset.tlDrag = d.type;
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    if (d.type === "seek") apply(d, e);
  }
  function onMove(e) {
    const d = st.drag;
    if (!d) return;
    d.lastE = e;
    if (!d.moved) {
      if (Math.abs(e.clientX - d.x0) < tlDragPx) return;
      d.moved = true;
      if (d.type === "click") return;
    }
    if (d.type !== "click") apply(d, e);
  }
  /** One step of a drag at mouse event `e`. */
  function apply(d, e) {
    const x = e.clientX - d.rect.left, raw = timeAtX(x), free = e.altKey; // Alt: no snapping
    const snap = (sec, skip) => (free ? sec : tlSnap(sec, snapCands(skip), snapTol()).at);
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
        const c = snapCands({ row: d.rowId }), tol = snapTol();
        const s1 = tlSnap(o, c, tol), s2 = tlSnap(o + len, c, tol);
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
    if (!d.moved && d.sel !== undefined) setSel(d.sel);
    // A drag that ends outside the timeline must not count as a click on the viewer's backdrop.
    if (d.moved || !host.contains(e.target)) swallowClick();
  }
  function endDrag() {
    window.removeEventListener("mousemove", onMove);
    window.removeEventListener("mouseup", onUp);
    st.drag = null;
    host.classList.remove("tl-dragging");
    delete host.dataset.tlDrag;
    el.tip.hidden = true;
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
      const hit = row ? tlFileHit(row, timeAtX(e.clientX - rect.left), st.pps) : null;
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
        if (st.sel?.row === id) setSel(null);
        changed("file");
        break;
    }
  }
  function onInput(e) {
    const r = e.target;
    const isLevel = "tlLevel" in r.dataset, volId = r.dataset.tlVol;
    if (!isLevel && volId === undefined) return;
    if (st.gest?.el !== r) { emSnapshot(m); st.gest = { el: r }; } // one snapshot per slider gesture
    const v = +r.value / 100;
    if (isLevel) {
      const row = selRow();
      if (!row) return;
      emSetGain(m, row.id, st.sel.seg, v);
      st.dirty.add(row.id);
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
      syncRows();
      validateSel();
      setPps(st.pps);
      setScroll(st.scrollX);
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
    select(sel) { if (!st.dead) setSel(sel); },
    zoom(factor) { if (!st.dead) zoom(factor); },
    fit() { if (!st.dead) fit(); },
    destroy() {
      if (st.dead) return;
      st.dead = true;
      cancelAnimationFrame(st.raf);
      endDrag();
      unswallow();
      ro.disconnect();
      mo.disconnect();
      el.lanes.removeEventListener("mousedown", onDown);
      el.lanes.removeEventListener("mousemove", onHover);
      el.lanes.removeEventListener("wheel", onWheel);
      el.lanes.removeEventListener("dblclick", onDbl);
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
