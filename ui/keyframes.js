"use strict";
/*
 * Keyframes (0.1.27) for the editor's overlay items (ui/overlays.js) and canvas pieces (ui/canvas.js).
 *
 * A keyframe fixes where a thing is and how large it is at one time; between two keyframes it
 * moves and scales evenly, or eases in and out when the thing is "Smooth". Keyframe times are
 * TIMELINE seconds, like an overlay's from / to (ui/editmodel.js). The keys live on the item, so
 * undo / redo, project saves and emChanged carry them:
 *   overlay item  keys: [{ t, dx, dy, sx, sy }], smooth
 *                 moved by dx, dy (fractions of the picture) and scaled by sx, sy about the middle of
 *                 the item's own box; sx == sy for the kinds that must not stretch (text, drawing,
 *                 line, arrow). The item's own fields stay its shape before the move.
 *   canvas piece  keys: [{ t, x, y, w, h, cx, cy, zoom }], smooth
 *                 its place (fractions of the canvas), the middle of what it shows (fractions of the
 *                 clip's picture) and the zoom (canvas pixels per picture pixel). The source box
 *                 follows from those at every instant, so a piece is never stretched.
 * No keys = the thing stands still, exactly as before.
 *
 * The export gets each moving thing's track as points in OUTPUT seconds (after cuts, part order
 * and speed): kfOutTrack. A cut between two keyframes makes a jump, sent as two points at the
 * same output time. src-tauri/src/editor.rs turns the points into ffmpeg expressions.
 */

const KF_NEAR = 0.03;        // a keyframe this close to the playhead (seconds) is "at the playhead"
const KF_MAX = 200;          // keys kept per item
const KF_STEP = 1 / 15;      // a smooth move cut short by a cut is sent as points this far apart (output seconds)
const KF_STEPS_MAX = 12;     // ... at most this many per piece of a move
const KF_UNIFORM = ["text", "pen", "line", "arrow"]; // kinds that scale the same both ways
const KF_OV_FIELDS = ["dx", "dy", "sx", "sy"];
const KF_CV_FIELDS = ["x", "y", "w", "h", "cx", "cy", "zoom"];
const kfClamp = (x, lo, hi) => Math.min(hi, Math.max(lo, x));
const kfFin = (x, d) => (typeof x === "number" && Number.isFinite(x) ? x : d);
const kfR = (x, k = 100000) => Math.round(x * k) / k;

/* ---------- keys in general ---------- */

/** Ease in and out (smoothstep) of u in 0..1. */
const kfEase = (u) => u * u * (3 - 2 * u);
/** True if the thing has keyframes. */
const kfHas = (o) => !!(o && Array.isArray(o.keys) && o.keys.length);
/** Index of the key at `tm` (within KF_NEAR; the nearest), or -1. */
function kfIndexAt(keys, tm) {
  let best = -1, d = KF_NEAR;
  (keys || []).forEach((k, i) => { const e = Math.abs(k.t - tm); if (e <= d) { d = e; best = i; } });
  return best;
}
/** Time of the last key before `tm` (one at the playhead does not count), or null. */
function kfPrev(keys, tm) {
  let out = null;
  for (const k of keys || []) if (k.t < tm - KF_NEAR) out = k.t;
  return out;
}
/** Time of the first key after `tm`, or null. */
function kfNext(keys, tm) {
  for (const k of keys || []) if (k.t > tm + KF_NEAR) return k.t;
  return null;
}
/** The values of `fields` at timeline second `tm`: held before the first key and after the last. */
function kfValues(keys, tm, smooth, fields) {
  const n = keys.length, pick = (k) => Object.fromEntries(fields.map((f) => [f, k[f]]));
  if (tm <= keys[0].t) return pick(keys[0]);
  if (tm >= keys[n - 1].t) return pick(keys[n - 1]);
  let i = 0;
  while (i < n - 2 && tm >= keys[i + 1].t) i++;
  const a = keys[i], b = keys[i + 1], span = b.t - a.t;
  let u = span > 1e-9 ? kfClamp((tm - a.t) / span, 0, 1) : 1;
  if (smooth) u = kfEase(u);
  return Object.fromEntries(fields.map((f) => [f, a[f] + (b[f] - a[f]) * u]));
}
/** `keys` with `vals` at `tm`: the key at the playhead changed, or a new one put in its place in time. */
function kfPut(keys, tm, vals) {
  const list = (keys || []).map((k) => ({ ...k }));
  const i = kfIndexAt(list, tm);
  if (i >= 0) Object.assign(list[i], vals);
  else list.push({ t: Math.max(0, kfR(tm, 10000)), ...vals });
  list.sort((a, b) => a.t - b.t);
  return list.slice(0, KF_MAX);
}
/** `keys` without the one at `tm`. */
const kfDrop = (keys, tm) => { const i = kfIndexAt(keys, tm); return i < 0 ? keys.slice() : keys.filter((_, k) => k !== i); };
/** Every key `d` seconds later (a part moved). */
const kfShift = (keys, d) => (keys || []).map((k) => ({ ...k, t: k.t + d }));

/** Keys from a project or an undo step made safe: finite numbers in range, sorted; [] if none. */
function kfNormKeys(keys, fields, lim) {
  if (!Array.isArray(keys)) return [];
  return keys.filter((k) => k && typeof k === "object" && Number.isFinite(k.t)).slice(0, KF_MAX)
    .map((k) => { const o = { t: Math.max(0, k.t) }; for (const f of fields) o[f] = kfClamp(kfFin(k[f], lim[f][2]), lim[f][0], lim[f][1]); return o; })
    .sort((a, b) => a.t - b.t);
}
const KF_OV_LIM = { dx: [-4, 4, 0], dy: [-4, 4, 0], sx: [0.02, 50, 1], sy: [0.02, 50, 1] };
const KF_CV_LIM = { x: [-4, 4, 0], y: [-4, 4, 0], w: [0.005, 8, 1], h: [0.005, 8, 1], cx: [0, 1, 0.5], cy: [0, 1, 0.5], zoom: [0.01, 100, 1] };
/** An overlay item's keys made safe (uniform kinds keep sx == sy). */
function kfNormOv(o) {
  const keys = kfNormKeys(o.keys, KF_OV_FIELDS, KF_OV_LIM);
  if (KF_UNIFORM.includes(o.kind)) for (const k of keys) k.sy = k.sx;
  return keys;
}
const kfNormPiece = (p) => kfNormKeys(p.keys, KF_CV_FIELDS, KF_CV_LIM);

/* ---------- overlay items ---------- */

const kfIdentity = () => ({ dx: 0, dy: 0, sx: 1, sy: 1 });
/** The move of overlay `o` at timeline second `tm` ({ dx, dy, sx, sy }). */
const kfOvT = (o, tm) => (kfHas(o) ? kfValues(o.keys, tm, !!o.smooth, KF_OV_FIELDS) : kfIdentity());
/** The middle of the item's own box, in fractions of a W x H picture. */
function kfOvMid(o, W, H, measure) {
  const b = ovBBox(o, W, H, measure);
  return [(b.x + b.w / 2) / (W || 1), (b.y + b.h / 2) / (H || 1)];
}
/** Item `o` with move `T` built in (a plain item of the same kind, for hit tests, handles and blur boxes). */
function kfOvBake(o, T, W, H, measure) {
  const [mx, my] = kfOvMid(o, W, H, measure);
  const X = (x) => mx + (x - mx) * T.sx + T.dx, Y = (y) => my + (y - my) * T.sy + T.dy;
  const b = JSON.parse(JSON.stringify(o));
  delete b.keys;
  delete b.smooth;
  if (o.kind === "line" || o.kind === "arrow") Object.assign(b, { x1: X(o.x1), y1: Y(o.y1), x2: X(o.x2), y2: Y(o.y2) });
  else if (o.kind === "pen") b.pts = o.pts.map((p) => [X(p[0]), Y(p[1])]);
  else if (o.kind === "text") Object.assign(b, { x: X(o.x), y: Y(o.y), size: o.size * T.sx });
  else Object.assign(b, { x: X(o.x), y: Y(o.y), w: o.w * T.sx, h: o.h * T.sy });
  if (KF_UNIFORM.includes(o.kind) && b.width !== undefined) b.width = o.width * T.sx;
  return b;
}
/** The item as it is shown at `tm` (the item itself when it has no keys). */
const kfOvAt = (o, tm, W, H, measure) => (kfHas(o) ? { ...kfOvBake(o, kfOvT(o, tm), W, H, measure), keys: o.keys, smooth: o.smooth } : o);

/** The move after the shown item was dragged by (ddx, ddy). */
const kfOvMoveT = (T, ddx, ddy) => ({ ...T, dx: T.dx + ddx, dy: T.dy + ddy });
/**
 * The move that shows `o` as `after`, the shown item `before` (kfOvAt) resized by a handle
 * (ovResized). Boxes take the new box; the other kinds scale evenly about the point that stayed.
 */
function kfOvResizeT(o, T, before, after, handle, W, H, measure) {
  const [mx, my] = kfOvMid(o, W, H, measure);
  if (o.kind === "rect" || o.kind === "ellipse" || o.kind === "blur") {
    const sx = o.w > 1e-6 ? kfClamp(after.w / o.w, 0.02, 50) : T.sx, sy = o.h > 1e-6 ? kfClamp(after.h / o.h, 0.02, 50) : T.sy;
    return { dx: after.x + (o.w * sx) / 2 - mx, dy: after.y + (o.h * sy) / 2 - my, sx, sy };
  }
  // Even scaling by k about the point A that stays: p -> A + k (p - A).
  let k = 1, A = [mx, my];
  if (o.kind === "text") { k = before.size > 0 ? after.size / before.size : 1; A = [before.x, before.y]; }
  else if (o.kind === "line" || o.kind === "arrow") {
    const fixed = handle === "p1" ? [before.x2, before.y2] : [before.x1, before.y1];
    const was = handle === "p1" ? [before.x1, before.y1] : [before.x2, before.y2], now = handle === "p1" ? [after.x1, after.y1] : [after.x2, after.y2];
    const len = (p) => Math.hypot((p[0] - fixed[0]) * W, (p[1] - fixed[1]) * H);
    k = len(was) > 0.5 ? len(now) / len(was) : 1;
    A = fixed;
  }
  const s = kfClamp(T.sx * k, 0.02, 50);
  k = s / T.sx;
  return { dx: (1 - k) * (A[0] - mx) + k * T.dx, dy: (1 - k) * (A[1] - my) + k * T.dy, sx: s, sy: s };
}

/**
 * Draws the items shown at `tm` on a W x H canvas the way the export lays them: an item with keys
 * is its own still picture moved and scaled about its middle (so a stretched rectangle's line
 * stretches too, as in the export). `skip`: an id not to draw.
 */
function kfOvDraw(ctx, list, tm, W, H, skip, measure) {
  for (const o of list) {
    if (o.kind === "blur" || o.id === skip) continue;
    if (!kfHas(o)) { ovDraw(ctx, [o], W, H, skip); continue; }
    const T = kfOvT(o, tm), [mx, my] = kfOvMid(o, W, H, measure);
    ctx.save();
    ctx.translate((mx + T.dx) * W, (my + T.dy) * H);
    ctx.scale(T.sx, T.sy);
    ctx.translate(-mx * W, -my * H);
    ovDraw(ctx, [o], W, H, skip);
    ctx.restore();
  }
}

/** Room around a moving item's box in its export picture (line ends, arrow heads, text outline), pixels. */
function kfOvPad(o, H) {
  const lw = (o.width || 0) * H;
  return Math.ceil(Math.max(4, lw * 3.5, H * 0.025) + (o.kind === "text" ? o.size * H * 0.35 : 0));
}
/** Where a moving item's own picture lies, in pixels of a W x H picture: centred on the item's box middle. */
function kfOvFrame(o, W, H, measure) {
  const b = ovBBox(o, W, H, measure), pad = kfOvPad(o, H);
  const mx = b.x + b.w / 2, my = b.y + b.h / 2;
  const pw = Math.max(2, 2 * Math.ceil(b.w / 2 + pad)), ph = Math.max(2, 2 * Math.ceil(b.h / 2 + pad));
  return { x: mx - pw / 2, y: my - ph / 2, w: pw, h: ph };
}

/* ---------- canvas pieces ---------- */

/** A piece's key values from its boxes, on canvas `C` of a `src` picture ({ w, h } pixels). */
function kfPieceKey(p, C, src) {
  return { x: p.x, y: p.y, w: p.w, h: p.h, cx: p.sx + p.sw / 2, cy: p.sy + p.sh / 2, zoom: (p.w * C.width) / Math.max(1e-9, p.sw * src.w) };
}
/** The boxes of a piece from key values: the source box is the place's size over the zoom, around (cx, cy), inside the picture. */
function kfPieceBoxes(v, C, src) {
  const sw = Math.min(1, (v.w * C.width) / (v.zoom * src.w)), sh = Math.min(1, (v.h * C.height) / (v.zoom * src.h));
  return { x: v.x, y: v.y, w: v.w, h: v.h, sw, sh, sx: kfClamp(v.cx - sw / 2, 0, 1 - sw), sy: kfClamp(v.cy - sh / 2, 0, 1 - sh) };
}
/** The piece as it is at timeline second `tm` (the piece itself when it has no keys). */
function kfPieceAt(p, tm, C, src) {
  if (!kfHas(p)) return p;
  return { ...p, ...kfPieceBoxes(kfValues(p.keys, tm, !!p.smooth, KF_CV_FIELDS), C, src) };
}
/** Piece `p` (with keys) after its shown state at `tm` became `boxes`: the key there changes or is added; its own boxes follow. */
function kfPieceSet(p, tm, boxes, C, src) {
  const b = { sx: boxes.sx, sy: boxes.sy, sw: boxes.sw, sh: boxes.sh, x: boxes.x, y: boxes.y, w: boxes.w, h: boxes.h };
  return { ...p, ...b, keys: kfPut(p.keys, tm, kfPieceKey(b, C, src)) };
}
/** Every key of `p` refitted after the canvas changed size (from `C0` to `C`), as cvRefit does to a still piece. */
function kfPieceRefit(p, C0, C, src) {
  if (!kfHas(p)) return p;
  const keys = p.keys.map((k) => ({ t: k.t, ...kfPieceKey(cvRefit({ ...kfPieceBoxes(k, C0, src) }, C, src), C, src) }));
  return { ...p, keys };
}

/* ---------- the export: tracks in output seconds ---------- */

/**
 * The track of a thing with `keys` (timeline seconds) for the export: points [{ t, ...values, e }]
 * in OUTPUT seconds of model `m` (cut-out parts gone, speed applied), over timeline range
 * [from, to]. `valuesAt(tl)` gives the values at timeline second tl. Each kept range gives a point
 * at its start, at every key inside it and at its end, so a cut between two keys is a jump (two
 * points at one output time). `e` (smooth only) eases the way to the next point; a smooth move that
 * a cut ends part way is sent as points KF_STEP apart instead.
 */
function kfOutTrack(m, keys, smooth, valuesAt, from = 0, to = m.duration) {
  const sp = m.speed || 1, out = [], first = keys[0].t, last = keys[keys.length - 1].t;
  const isKey = (x) => keys.some((k) => Math.abs(k.t - x) < 1e-6);
  let at = 0;
  for (const r of emKept(m)) {
    const a = Math.max(r.start, from), b = Math.min(r.end, to);
    const outT = (tl) => kfR((at + tl - r.start) / sp, 10000);
    const push = (tl, e) => out.push({ t: outT(tl), ...valuesAt(tl), ...(e ? { e: true } : {}) });
    if (b - a > 1e-6) {
      const times = [a, ...keys.map((k) => k.t).filter((x) => x > a + 1e-6 && x < b - 1e-6), b];
      for (let i = 0; i < times.length - 1; i++) {
        const t0 = times[i], t1 = times[i + 1];
        const moving = smooth && t0 >= first - 1e-9 && t1 <= last + 1e-9;
        if (moving && !(isKey(t0) && isKey(t1))) {
          const n = Math.min(KF_STEPS_MAX, Math.max(1, Math.ceil((t1 - t0) / sp / KF_STEP)));
          for (let j = 0; j < n; j++) push(t0 + ((t1 - t0) * j) / n, false);
        } else push(t0, moving);
      }
      push(b, false);
    }
    at += r.end - r.start;
  }
  return out;
}
const kfRound = (v) => Object.fromEntries(Object.entries(v).map(([k, x]) => [k, kfR(x)]));
/** An overlay item's or blur box's track for export_clip: [{ t, dx, dy, sx, sy, e }]. */
const kfOvTrack = (m, o) => kfOutTrack(m, o.keys, !!o.smooth, (tl) => kfRound(kfOvT(o, tl)), o.from, o.to);
/** A canvas piece's track for export_clip: [{ t, x, y, w, h, cx, cy, z, e }] (z = the zoom). */
const kfPieceTrack = (m, p) => kfOutTrack(m, p.keys, !!p.smooth, (tl) => { const v = kfValues(p.keys, tl, !!p.smooth, KF_CV_FIELDS); return kfRound({ x: v.x, y: v.y, w: v.w, h: v.h, cx: v.cx, cy: v.cy, z: v.zoom }); });

/* ---------- the panel controls (overlay panel and canvas panel) ---------- */

/** The diamonds of `keys` on a time bar of a `D` second timeline (the one at `tm` marked). */
function kfDiamonds(keys, D, tm) {
  const at = kfIndexAt(keys, tm);
  return (keys || []).map((k, i) => `<i class="kf-dia${i === at ? " on" : ""}" data-kf-t="${k.t}" style="left:${kfClamp((k.t / (D || 1)) * 100, 0, 100)}%" title="${esc(t("Keyframe at {time}", { time: ovFmtTime(k.t) }))}"></i>`).join("");
}
/** The keyframe buttons for a thing with `keys` at playhead `tm`: previous, add / remove, next, and "Smooth". */
function kfControls(keys, tm, smooth) {
  const has = (keys || []).length > 0, on = kfIndexAt(keys, tm) >= 0;
  const prev = kfPrev(keys, tm), next = kfNext(keys, tm);
  const b = (act, label, icon, dis) => `<button type="button" class="btn sm kf-btn" data-kf="${act}" title="${esc(label)}" aria-label="${esc(label)}"${dis ? " disabled" : ""}>${icon}</button>`;
  return `<div class="kf-row" role="group" aria-label="${esc(t("Keyframes"))}"><span class="kf-label">${esc(t("Keyframes"))}</span>`
    + b("prev", t("Previous keyframe"), "◀", prev === null)
    + `<button type="button" class="btn sm kf-add${on ? " on" : ""}" data-kf="${on ? "remove" : "add"}">${esc(on ? t("Remove this keyframe") : t("Add a keyframe"))}</button>`
    + b("next", t("Next keyframe"), "▶", next === null)
    + `<label class="check kf-smooth" title="${esc(t("Starts and stops gently instead of moving at one speed"))}"><input type="checkbox" data-kf="smooth"${smooth ? " checked" : ""}${has ? "" : " disabled"}><span>${esc(t("Smooth"))}</span></label></div>`
    + (has ? `<div class="small kf-hint">${esc(t("Moving or resizing it on the picture sets a keyframe at the playhead."))}</div>` : "");
}

// Lets the pure functions be tested under Node; ignored in the app.
if (typeof module !== "undefined" && module.exports) {
  module.exports = { KF_NEAR, KF_STEP, KF_UNIFORM, kfEase, kfHas, kfIndexAt, kfPrev, kfNext, kfValues, kfPut, kfDrop, kfShift, kfNormOv, kfNormPiece, kfOvT, kfOvMid, kfOvBake, kfOvAt, kfOvMoveT, kfOvResizeT, kfOvFrame, kfPieceKey, kfPieceBoxes, kfPieceAt, kfPieceSet, kfPieceRefit, kfOutTrack, kfOvTrack, kfPieceTrack };
}
