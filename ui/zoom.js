"use strict";
/*
 * Zoom (0.1.41): the editor's direct zoom, with keyframes, on top of the canvas (ui/canvas.js) and its piece keyframes
 * (ui/keyframes.js: a piece's keys carry x, y, w, h, cx, cy, zoom; the export already sends them as canvas_tracks).
 *
 * The first zoom on a clip without a canvas makes one of the clip's own size with ONE piece filling it (the same
 * picture at 100 %), and "Reset zoom" removes that canvas again when nothing else of it was changed, so the export is
 * exactly what it was. With a canvas already there (a vertical layout) the selected piece zooms, else the largest one.
 * 100 % = the piece shows as much of the picture as fits its place; 500 % at most.
 *
 * The section "Zoom" (after Speed): the amount, keyframe buttons, Smooth, Reset; a drag on the picture while the
 * section is open moves the zoom's centre; the picture's right-click menu gets "Zoom in here" / "Zoom out" (a key at
 * the playhead and one ZM_PRESET s later). The timeline draws the keys as diamonds on V1's top edge (ui/timeline.js).
 * Never name a variable `t` here.
 */

const ZM_MAX = 5;          // 500 %
const ZM_PRESET = 0.5;     // seconds a right-click zoom takes
const zmEven = (x) => Math.max(16, Math.min(4096, Math.round((+x || 0) / 2) * 2));
const zmNear = (a, b) => Math.abs(a - b) < 1e-4;

/* ---------- pure parts (tested under Node) ---------- */

/** A canvas of the clip's own size with one piece filling it: the clip's own picture, unchanged. */
function zmNewCanvas(src, id) {
  return { width: zmEven(src.w), height: zmEven(src.h), background: "black", pieces: [{ id: id || `z${Date.now().toString(36)}`, sx: 0, sy: 0, sw: 1, sh: 1, x: 0, y: 0, w: 1, h: 1 }] };
}
/** True if canvas `C` is just that (and no key): it can go again. */
function zmIsPlain(C, src) {
  if (!C || C.crop || !Array.isArray(C.pieces) || C.pieces.length !== 1) return false;
  const p = C.pieces[0];
  return C.width === zmEven(src.w) && C.height === zmEven(src.h) && !(Array.isArray(p.keys) && p.keys.length)
    && zmNear(p.x, 0) && zmNear(p.y, 0) && zmNear(p.w, 1) && zmNear(p.h, 1) && zmNear(p.sx, 0) && zmNear(p.sy, 0) && zmNear(p.sw, 1) && zmNear(p.sh, 1);
}
/** The piece the zoom acts on: `selId`'s, else the largest. */
function zmPieceIndex(C, selId) {
  if (!C || !C.pieces.length) return -1;
  const i = C.pieces.findIndex((p) => p.id === selId);
  if (i >= 0) return i;
  let best = 0;
  C.pieces.forEach((p, k) => { if (p.w * p.h > C.pieces[best].w * C.pieces[best].h) best = k; });
  return best;
}
/** The zoom value (keyframes.js `zoom`) at which piece `p` shows as much of the picture as fits its place: 100 %. */
const zmFit = (p, C, src) => Math.max((p.w * C.width) / src.w, (p.h * C.height) / src.h);
/** The piece's key values at timeline second `tm` (its own boxes without keys). */
const zmValues = (p, tm, C, src) => (kfHas(p) ? kfValues(p.keys, tm, !!p.smooth, KF_CV_FIELDS) : kfPieceKey(p, C, src));
/** The zoom at `tm` in percent of 100 % (1 = 100 %). */
const zmPct = (p, tm, C, src) => zmValues(p, tm, C, src).zoom / zmFit(p, C, src);
/**
 * Piece `p` with its zoom at `tm` changed: `pct` (1..ZM_MAX) and / or the centre `cx`, `cy` (fractions of the picture,
 * kept inside it). With keys, the key at `tm` changes or is added; without, the piece itself changes.
 */
function zmWith(p, tm, C, src, change) {
  const v = zmValues(p, tm, C, src), fit = zmFit(p, C, src);
  const pct = Number.isFinite(change.pct) ? Math.min(ZM_MAX, Math.max(1, change.pct)) : v.zoom / fit;
  const boxes = kfPieceBoxes({ ...v, zoom: pct * fit, cx: Number.isFinite(change.cx) ? change.cx : v.cx, cy: Number.isFinite(change.cy) ? change.cy : v.cy }, C, src);
  return kfHas(p) ? kfPieceSet(p, tm, boxes, C, src) : { ...p, ...boxes };
}
/** A key at `tm` with the zoom as it is there (eased by default). */
function zmAddKey(p, tm, C, src) {
  return { ...p, keys: kfPut(p.keys, tm, zmValues(p, tm, C, src)), smooth: kfHas(p) ? !!p.smooth : true };
}
/** Without the key at `tm`; without keys at all it keeps the zoom it had there. */
function zmDropKey(p, tm, C, src) {
  if (!kfHas(p)) return p;
  const keys = kfDrop(p.keys, tm);
  if (keys.length) return { ...p, keys };
  const { keys: _k, smooth: _s, ...rest } = { ...p, ...kfPieceBoxes(zmValues(p, tm, C, src), C, src) };
  return rest;
}
/**
 * Right-click "Zoom in here" ("in": 200 % centred on `pt` = { x, y } fractions of the picture) or "Zoom out" ("out":
 * 100 %): a key at `tm` with the zoom as it is and one ZM_PRESET s later with the new zoom.
 */
function zmPreset(p, tm, C, src, kind, pt) {
  const v = zmValues(p, tm, C, src), fit = zmFit(p, C, src);
  const to = kind === "in" ? { ...v, zoom: 2 * fit, cx: pt ? pt.x : v.cx, cy: pt ? pt.y : v.cy } : { ...v, zoom: fit };
  const b = kfPieceBoxes(to, C, src);
  let keys = kfPut(p.keys, tm, v);
  keys = kfPut(keys, tm + ZM_PRESET, kfPieceKey(b, C, src));
  return { ...p, keys, smooth: kfHas(p) ? !!p.smooth : true };
}
/** Reset zoom: the piece back at 100 % (centred when it filled the picture), no keys; a canvas that is plain again goes. */
function zmReset(m, src, selId) {
  const C = m.canvas;
  if (!C) return false;
  const i = zmPieceIndex(C, selId);
  if (i < 0) return false;
  const p = C.pieces[i], v = zmValues(p, 0, C, src), fit = zmFit(p, C, src);
  const { keys: _k, smooth: _s, ...rest } = p;
  C.pieces[i] = { ...rest, ...kfPieceBoxes({ ...v, x: p.x, y: p.y, w: p.w, h: p.h, zoom: fit, cx: 0.5, cy: 0.5 }, C, src) };
  if (zmIsPlain(C, src)) m.canvas = null;
  return true;
}
/** The source point (fractions of the clip's picture) under canvas point (fx, fy) through piece `q` (its boxes now). */
const zmSrcPoint = (q, fx, fy) => ({ x: Math.min(1, Math.max(0, q.sx + ((fx - q.x) / q.w) * q.sw)), y: Math.min(1, Math.max(0, q.sy + ((fy - q.y) / q.h) * q.sh)) });

/* ---------- the section, the pan, the picture menu ---------- */

/** The editor's Zoom section and tools (editor.js calls it beside cvMount): paint(now), sync(what), menuHtml(item), menuAct(act, e), destroy(). */
function zmMount(root, c, P) {
  const box = root.querySelector(".player"), v = box?.querySelector("video"), pane = root.querySelector(".v-edit");
  if (!box || !v || !pane) return null;
  const sec = document.createElement("section");
  sec.className = "e-sec e-zoom";
  const after = pane.querySelector(".e-speed-sec");
  if (after) after.after(sec); else pane.insertBefore(sec, pane.querySelector(".e-export"));
  const st = { sig: "", gest: null, pan: null, dead: false };
  const m = () => P.model;
  const src = () => ({ w: (c.virtual ? c.width : v.videoWidth) || c.width || 1920, h: (c.virtual ? c.height : v.videoHeight) || c.height || 1080 });
  const selId = () => P.cv?._st?.sel || null;
  const tm = () => P.time();
  /** The piece the zoom acts on (null: no canvas yet). */
  const piece = () => { const C = m()?.canvas; const i = zmPieceIndex(C, selId()); return i >= 0 ? C.pieces[i] : null; };
  const pct = () => { const C = m()?.canvas, p = piece(); return p ? zmPct(p, tm(), C, src()) : 1; };

  /** Change the zoom piece with `fn(p, C, s)` -> the new piece; the canvas is made first when there is none. `live`: a gesture's step (one undo step for the gesture). */
  function edit(fn, live) {
    const mm = m();
    if (!mm) return false;
    if (!live || !st.gest) { st.gest = live ? { before: emState(mm) } : null; emSnapshot(mm); }
    if (!mm.canvas) mm.canvas = zmNewCanvas(src(), typeof cvNewId === "function" ? cvNewId() : undefined);
    const C = mm.canvas, i = zmPieceIndex(C, selId());
    C.pieces[i] = fn(C.pieces[i], C, src());
    if (zmIsPlain(C, src())) mm.canvas = null; // back at 100 % without keys: the clip's own picture again
    P.edSync("canvas");
    if (!live) render();
    return true;
  }
  function endGesture() {
    const mm = m(), g = st.gest;
    st.gest = null;
    if (mm && g && emState(mm) === g.before) mm.undo.pop(); // nothing changed in the end: no empty undo step
    render();
  }

  function render() {
    if (st.dead) return;
    const mm = m();
    const title = `<div class="e-title">${esc(t("Zoom"))}</div>`;
    if (!mm) { st.html = null; sec.innerHTML = `${title}<div class="small">${esc(t("Loading…"))}</div>`; return; }
    const p = piece(), now = tm(), keys = p?.keys || [], val = Math.round(pct() * 100);
    st.sig = keySig();
    const html = `${title}
      <div class="e-line zm-line"><input type="range" class="zm-range" min="100" max="${ZM_MAX * 100}" step="5" value="${val}" aria-label="${esc(t("Zoom amount"))}">
        <input type="number" class="field zm-num" min="100" max="${ZM_MAX * 100}" step="10" value="${val}" aria-label="${esc(t("Zoom amount"))}"><span class="small">%</span></div>
      ${typeof kfControls === "function" ? kfControls(keys, now, !!p?.smooth) : ""}
      <div class="e-line"><button type="button" class="btn sm" data-zm="reset"${p && (keys.length || val !== 100) ? "" : " disabled"}>${esc(t("Reset zoom"))}</button>
        <span class="small zm-hint">${esc(val > 100 ? t("Drag on the picture to move the zoom.") : t("Right-click the picture to zoom in there."))}</span></div>`;
    // Rebuilt only when it would read differently: this runs on every sync of the editor. When it
    // is left alone, the two fields still take the value a rebuild would have given them.
    if (html === st.html && sec.querySelector(".zm-range")) {
      for (const f of sec.querySelectorAll(".zm-range, .zm-num")) if (f.value !== String(val)) f.value = String(val);
      return;
    }
    st.html = html;
    sec.innerHTML = html;
  }
  /** What the section shows: redrawn when it changes (the playhead passing a key, the zoom there). */
  const keySig = () => { const p = piece(), now = tm(); return `${Math.round(pct() * 100)}|${p ? (p.keys || []).map((k) => k.t).join(",") : ""}|${p?.smooth ? 1 : 0}|${p && p.keys ? kfIndexAt(p.keys, now) : -1}|${p && p.keys ? `${kfPrev(p.keys, now)}|${kfNext(p.keys, now)}` : ""}`; };

  sec.addEventListener("input", (e) => {
    const el = e.target;
    if (!el.classList.contains("zm-range") && !el.classList.contains("zm-num")) return;
    const x = +el.value;
    if (!Number.isFinite(x)) return;
    edit((p, C, s) => zmWith(p, tm(), C, s, { pct: x / 100 }), true);
    const other = sec.querySelector(el.classList.contains("zm-range") ? ".zm-num" : ".zm-range");
    if (other) other.value = String(Math.round(x));
  });
  sec.addEventListener("change", (e) => { if (e.target.classList.contains("zm-range") || e.target.classList.contains("zm-num")) { endGesture(); e.target.blur(); } });
  sec.addEventListener("click", (e) => {
    const b = e.target.closest("[data-kf], [data-zm]");
    if (!b || b.disabled || !m()) return;
    const k = b.dataset.kf || b.dataset.zm, p = piece(), now = tm();
    if (k === "prev" || k === "next") { const x = p?.keys ? (k === "prev" ? kfPrev(p.keys, now) : kfNext(p.keys, now)) : null; if (x !== null) P.seek(x, false, true); return; }
    if (k === "add") return edit((q, C, s) => zmAddKey(q, now, C, s));
    if (k === "remove") return edit((q, C, s) => zmDropKey(q, now, C, s));
    if (k === "smooth") return edit((q) => (kfHas(q) ? { ...q, smooth: !!b.checked } : q));
    if (k === "reset") { const mm = m(); emSnapshot(mm); zmReset(mm, src(), selId()); P.edSync("canvas"); render(); }
  });

  /* The pan: a drag on the picture while the section is open and zoomed in moves the zoom's centre. */
  const picRect = () => (box.querySelector(".ov-layer") || v).getBoundingClientRect();
  const open = () => P.editing && !sec.classList.contains("ws-fold") && !!m();
  box.addEventListener("mousedown", (e) => {
    if (e.button !== 0 || !open() || pct() <= 1.001) return;
    if (e.target.closest(".p-ui, .ov-bar, button, input, .cv-layer:not([hidden]) *") || P.cv?._st?.arrange) return;
    if (P.ov?._st && (P.ov._st.tool !== "select" || P.ov._st.sel || P.ov._st.crop)) return; // drawing, or an item picked: the overlays' own
    const r = picRect();
    if (!(e.clientX >= r.left && e.clientX <= r.right && e.clientY >= r.top && e.clientY <= r.bottom)) return;
    const C = m().canvas, p = piece(), q = kfHas(p) ? kfPieceAt(p, tm(), C, src()) : p, v0 = zmValues(p, tm(), C, src());
    st.pan = { x: e.clientX, y: e.clientY, W: r.width, H: r.height, cx: v0.cx, cy: v0.cy, q };
    e.preventDefault();
    e.stopPropagation();
    const move = (ev) => {
      const d = st.pan;
      if (!d) return;
      const fx = (ev.clientX - d.x) / d.W, fy = (ev.clientY - d.y) / d.H;
      edit((pp, CC, s) => zmWith(pp, tm(), CC, s, { cx: d.cx - (fx / d.q.w) * d.q.sw, cy: d.cy - (fy / d.q.h) * d.q.sh }), true);
    };
    const up = () => { window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up); st.pan = null; endGesture(); };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  }, true);

  return {
    paint() { if (!st.dead && !st.gest && keySig() !== st.sig) render(); },
    sync() { if (!st.gest) render(); },
    /** The picture's right-click menu: "Zoom in here", "Zoom out" (overlays.js builds the menu; `item(act, path, label)`). */
    menuHtml(item) {
      if (!m() || !P.editing) return "";
      return item("zm-in", "M11 4a7 7 0 1 1 0 14a7 7 0 1 1 0-14zM16.2 16.2 21 21M8 11h6M11 8v6", t("Zoom in here"))
        + (pct() > 1.001 ? item("zm-out", "M11 4a7 7 0 1 1 0 14a7 7 0 1 1 0-14zM16.2 16.2 21 21M8 11h6", t("Zoom out")) : "") + "<hr>";
    },
    /** A pick of that menu; `x`, `y`: where it was opened (client pixels). True when it was one of ours. */
    menuAct(act, x, y) {
      if (act !== "zm-in" && act !== "zm-out") return false;
      const r = picRect(), fx = Math.min(1, Math.max(0, (x - r.left) / (r.width || 1))), fy = Math.min(1, Math.max(0, (y - r.top) / (r.height || 1))), now = tm();
      edit((p, C, s) => {
        const q = kfHas(p) ? kfPieceAt(p, now, C, s) : p;
        return zmPreset(p, now, C, s, act === "zm-in" ? "in" : "out", zmSrcPoint(q, fx, fy));
      });
      return true;
    },
    destroy() { st.dead = true; sec.remove(); },
  };
}

// Lets the pure parts be tested under Node; ignored in the app.
if (typeof module !== "undefined" && module.exports) {
  module.exports = { ZM_MAX, ZM_PRESET, zmNewCanvas, zmIsPlain, zmPieceIndex, zmFit, zmValues, zmPct, zmWith, zmAddKey, zmDropKey, zmPreset, zmReset, zmSrcPoint };
}
