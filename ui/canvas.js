"use strict";
/*
 * The editor's canvas (0.1.26): a picture of a chosen shape and size (9:16 for TikTok, Shorts and
 * Reels, 1:1, 4:5, ...) on which PIECES of the clip's picture are placed. A piece is a box cut
 * out of the clip's picture ("source box": sx, sy, sw, sh, fractions of the clip's picture) and
 * a box on the canvas where it is shown ("place": x, y, w, h, fractions of the canvas). Both
 * boxes always have the same shape in pixels, so a piece is never stretched: a corner handle
 * scales, an edge handle crops (or shows more) at the same zoom. Behind the pieces is the
 * background: the clip blurred, black or a color.
 *
 * The model keeps it as `m.canvas` (ui/editmodel.js: null = the clip's own picture), so undo,
 * projects and the export (`canvas` of export_clip, src-tauri/src/editor.rs) carry it. The
 * preview is a <canvas> over the hidden <video>, drawn from the video's frames the way the
 * export builds it. Text, shapes and blur boxes (ui/overlays.js) are then placed on the canvas.
 */

const CV_MIN = 0.03; // smallest place box side, fraction of the canvas
const CV_SRC_MIN = 0.02; // smallest source box side, fraction of the clip's picture
const CV_COLORS = ["#3b82f6", "#f59e0b", "#10b981", "#ef4444", "#a855f7", "#06b6d4", "#ec4899", "#84cc16"];
/** Shapes with their sizes; the second size of each is the usual one. */
const CV_SHAPES = [
  ["9:16", [[720, 1280], [1080, 1920], [1440, 2560], [2160, 3840]]],
  ["1:1", [[720, 720], [1080, 1080], [1440, 1440], [2160, 2160]]],
  ["4:5", [[720, 900], [1080, 1350], [1440, 1800], [2160, 2700]]],
  ["16:9", [[1280, 720], [1920, 1080], [2560, 1440], [3840, 2160]]],
];
const cvShapeName = (id) => (id === "9:16" ? t("9:16 vertical (TikTok, Shorts, Reels)") : id === "1:1" ? t("1:1 square") : id === "4:5" ? t("4:5 portrait") : id === "16:9" ? t("16:9 wide") : id);
const cvLayoutName = (k) => (k === "fill" ? t("Fill") : k === "fit" ? t("Whole picture") : k === "three" ? t("Gameplay and two strips") : k === "two" ? t("Gameplay and one strip") : k);
const cvLayoutHint = (k) => (k === "fill" ? t("The middle of the picture fills the canvas")
  : k === "fit" ? t("The whole picture, with the background above and below")
    : k === "three" ? t("The gameplay in the middle, a zoomed strip above (kill feed) and below (health bar)")
      : k === "two" ? t("The gameplay on top and one zoomed strip below") : "");

const cvClamp = (x, lo, hi) => Math.min(hi, Math.max(lo, x));
const cvEven = (x) => cvClamp(Math.round((+x || 0) / 2) * 2, 16, 4096);
let cvSeq = 0;
const cvNewId = () => `p${Date.now().toString(36)}${(cvSeq++).toString(36)}`;

/** The shape id of a canvas size, or "custom". */
function cvShapeOf(C) {
  const hit = CV_SHAPES.find(([, sizes]) => sizes.some(([w, h]) => w === C.width && h === C.height));
  return hit ? hit[0] : "custom";
}

/**
 * A source box of pixel shape `a` (width / height) in a `src.w` x `src.h` picture: at most `wide`
 * of its width and never taller than it, pushed to the side `ax`, `ay` say (0 = left / top,
 * 0.5 = middle, 1 = right / bottom).
 */
function cvCrop(a, src, ax, ay, wide) {
  let sw = cvClamp(wide, CV_SRC_MIN, 1), sh = (sw * src.w) / a / src.h;
  if (sh > 1) { sh = 1; sw = Math.min(1, (a * src.h) / src.w); }
  return { sx: (1 - sw) * ax, sy: (1 - sh) * ay, sw, sh };
}

/** The layouts a canvas of this shape offers. */
const cvLayoutsFor = (C) => (C.height / C.width >= 1.2 ? ["fill", "fit", "three", "two"] : ["fill", "fit"]);

/** The pieces (without ids) of layout `kind` on canvas `C` for a clip picture `src`. */
function cvLayout(kind, C, src) {
  const band = (y, h, ax, ay, wide) => ({ ...cvCrop(C.width / (h * C.height), src, ax, ay, wide), x: 0, y, w: 1, h });
  if (kind === "three") return [band(0, 0.24, 1, 0, 0.36), band(0.24, 0.5, 0.5, 0.5, 1), band(0.74, 0.26, 0, 1, 0.3)];
  if (kind === "two") return [band(0, 0.7, 0.5, 0.5, 1), band(0.7, 0.3, 0.5, 1, 0.4)];
  if (kind === "fit") {
    const a = src.w / src.h;
    let w = 1, h = C.width / a / C.height;
    if (h > 1) { h = 1; w = (C.height * a) / C.width; }
    return [{ sx: 0, sy: 0, sw: 1, sh: 1, x: (1 - w) / 2, y: (1 - h) / 2, w, h }];
  }
  return [band(0, 1, 0.5, 0.5, 1)];
}

/**
 * `p` with its source box given the pixel shape of its place again (after the canvas changed
 * shape, or on another clip): about as much of the picture, at the same side of it, inside it.
 */
function cvRefit(p, C, src) {
  const a = (p.w * C.width) / (p.h * C.height);
  // Where the box sits in the room it has (0 = at the left / top edge, 1 = at the right / bottom): kept, so a
  // box in a corner of the picture (a kill feed, a health bar) stays in that corner.
  const ax = p.sw < 0.999 ? cvClamp(p.sx / (1 - p.sw), 0, 1) : 0.5, ay = p.sh < 0.999 ? cvClamp(p.sy / (1 - p.sh), 0, 1) : 0.5;
  const area = Math.max(1, p.sw * src.w * p.sh * src.h);
  let wpx = Math.sqrt(area * a), hpx = wpx / a;
  const k = Math.min(1, src.w / wpx, src.h / hpx);
  wpx *= k;
  hpx *= k;
  const sw = wpx / src.w, sh = hpx / src.h;
  return { ...p, sw, sh, sx: (1 - sw) * ax, sy: (1 - sh) * ay };
}

/** `p` zoomed: its source box `f` times as large around its middle (f < 1 = closer), same shape, inside the picture. */
function cvZoom(p, f) {
  const hi = Math.min(1 / p.sw, 1 / p.sh), lo = Math.min(hi, Math.max(CV_SRC_MIN / p.sw, CV_SRC_MIN / p.sh));
  f = cvClamp(f, lo, hi);
  const sw = p.sw * f, sh = p.sh * f;
  return { ...p, sw, sh, sx: cvClamp(p.sx + (p.sw - sw) / 2, 0, 1 - sw), sy: cvClamp(p.sy + (p.sh - sh) / 2, 0, 1 - sh) };
}

/**
 * A drag on the canvas ("result" view) of piece `p0` by `dx`, `dy` (fractions of the canvas).
 * `handle`: "move", an edge ("n", "s", "e", "w": crops or shows more at the same zoom, as far as
 * the clip's picture goes) or a corner ("nw", "ne", "sw", "se": scales the place, same content).
 */
function cvResultDrag(p0, handle, dx, dy) {
  const p = { ...p0 };
  if (handle === "move") {
    p.x = cvClamp(p0.x + dx, CV_MIN - p0.w, 1 - CV_MIN);
    p.y = cvClamp(p0.y + dy, CV_MIN - p0.h, 1 - CV_MIN);
    return p;
  }
  const kx = p0.w / p0.sw, ky = p0.h / p0.sh; // place per source, both in fractions
  if (handle.length === 2) {
    const fx = (p0.w + (handle[1] === "e" ? dx : -dx)) / p0.w, fy = (p0.h + (handle[0] === "s" ? dy : -dy)) / p0.h;
    const f = cvClamp(Math.max(fx, fy), CV_MIN / Math.min(p0.w, p0.h), 8 / Math.max(p0.w, p0.h));
    p.w = p0.w * f;
    p.h = p0.h * f;
    if (handle[1] === "w") p.x = p0.x + p0.w - p.w;
    if (handle[0] === "n") p.y = p0.y + p0.h - p.h;
    return p;
  }
  if (handle === "e") { p.w = cvClamp(p0.w + dx, CV_MIN, (1 - p0.sx) * kx); p.sw = p.w / kx; }
  else if (handle === "w") { p.w = cvClamp(p0.w - dx, CV_MIN, (p0.sx + p0.sw) * kx); p.sw = p.w / kx; p.x = p0.x + p0.w - p.w; p.sx = p0.sx + p0.sw - p.sw; }
  else if (handle === "s") { p.h = cvClamp(p0.h + dy, CV_MIN, (1 - p0.sy) * ky); p.sh = p.h / ky; }
  else if (handle === "n") { p.h = cvClamp(p0.h - dy, CV_MIN, (p0.sy + p0.sh) * ky); p.sh = p.h / ky; p.y = p0.y + p0.h - p.h; p.sy = p0.sy + p0.sh - p.sh; }
  return p;
}

/**
 * A drag on the clip's picture ("source" view) of piece `p0`'s source box by `dx`, `dy`
 * (fractions of the clip's picture). "move" shows another part; a corner zooms (the place
 * stays); an edge crops or shows more at the same zoom (the place grows or shrinks with it).
 */
function cvSourceDrag(p0, handle, dx, dy) {
  const p = { ...p0 };
  if (handle === "move") {
    p.sx = cvClamp(p0.sx + dx, 0, 1 - p0.sw);
    p.sy = cvClamp(p0.sy + dy, 0, 1 - p0.sh);
    return p;
  }
  const kx = p0.w / p0.sw, ky = p0.h / p0.sh;
  if (handle.length === 2) {
    const east = handle[1] === "e", south = handle[0] === "s";
    const fx = (p0.sw + (east ? dx : -dx)) / p0.sw, fy = (p0.sh + (south ? dy : -dy)) / p0.sh;
    const hi = Math.min((east ? 1 - p0.sx : p0.sx + p0.sw) / p0.sw, (south ? 1 - p0.sy : p0.sy + p0.sh) / p0.sh);
    const f = cvClamp(Math.max(fx, fy), Math.min(hi, CV_SRC_MIN / Math.min(p0.sw, p0.sh)), hi);
    p.sw = p0.sw * f;
    p.sh = p0.sh * f;
    if (!east) p.sx = p0.sx + p0.sw - p.sw;
    if (!south) p.sy = p0.sy + p0.sh - p.sh;
    return p;
  }
  if (handle === "e") { p.sw = cvClamp(p0.sw + dx, CV_SRC_MIN, 1 - p0.sx); p.w = p.sw * kx; }
  else if (handle === "w") { p.sw = cvClamp(p0.sw - dx, CV_SRC_MIN, p0.sx + p0.sw); p.sx = p0.sx + p0.sw - p.sw; p.w = p.sw * kx; p.x = p0.x + p0.w - p.w; }
  else if (handle === "s") { p.sh = cvClamp(p0.sh + dy, CV_SRC_MIN, 1 - p0.sy); p.h = p.sh * ky; }
  else if (handle === "n") { p.sh = cvClamp(p0.sh - dy, CV_SRC_MIN, p0.sy + p0.sh); p.sy = p0.sy + p0.sh - p.sh; p.h = p.sh * ky; p.y = p0.y + p0.h - p.h; }
  return p;
}

/**
 * Where a new piece cut out as source box `b` goes on canvas `C`: as wide as the canvas (or as
 * tall, if that is what fits), in the middle of the largest free band between the other pieces.
 */
function cvPlaceNew(b, C, src, pieces) {
  const a = (b.sw * src.w) / (b.sh * src.h);
  let w = 1, h = C.width / a / C.height;
  if (h > 1) { h = 1; w = (C.height * a) / C.width; }
  const taken = (pieces || []).map((p) => [cvClamp(p.y, 0, 1), cvClamp(p.y + p.h, 0, 1)]).filter(([y0, y1]) => y1 - y0 > 0.001).sort((p, q) => p[0] - q[0]);
  let best = [0, 0], at = 0;
  for (const [y0, y1] of [...taken, [1, 1]]) {
    if (y0 - at > best[1] - best[0]) best = [at, y0];
    at = Math.max(at, y1);
  }
  const y = best[1] - best[0] > 0.001 ? cvClamp(best[0] + (best[1] - best[0] - h) / 2, 0, Math.max(0, 1 - h)) : (1 - h) / 2;
  return { sx: b.sx, sy: b.sy, sw: b.sw, sh: b.sh, x: (1 - w) / 2, y, w, h };
}

/** `v` moved onto the nearest of `targets` when within `tol`; else `v`. */
function cvSnap(v, targets, tol) {
  let best = v, d = tol;
  for (const x of targets) { const e = Math.abs(x - v); if (e < d) { d = e; best = x; } }
  return best;
}

/** The start of a box `size` long at `start`, moved so that its start, middle or end lands on the nearest of `targets` within `tol`. */
function cvSnapBox(start, size, targets, tol) {
  let best = start, d = tol;
  for (const o of [0, size / 2, size]) {
    const raw = start + o, hit = cvSnap(raw, targets, tol), e = Math.abs(hit - raw);
    if (hit !== raw && e < d) { d = e; best = hit - o; }
  }
  return best;
}

/** A canvas made safe to use (from a project or a saved layout): null when it cannot be. */
function cvNorm(c) {
  if (!c || typeof c !== "object" || !Array.isArray(c.pieces)) return null;
  const n = (x, d) => (typeof x === "number" && Number.isFinite(x) ? x : d);
  const pieces = c.pieces.map((p) => {
    if (!p || typeof p !== "object") return null;
    const sw = cvClamp(n(p.sw, 1), CV_SRC_MIN, 1), sh = cvClamp(n(p.sh, 1), CV_SRC_MIN, 1);
    const w = cvClamp(n(p.w, 1), 0.005, 8), h = cvClamp(n(p.h, 1), 0.005, 8);
    // Keyframes (keyframes.js), when it has any.
    const keys = Array.isArray(p.keys) && typeof kfNormPiece === "function" ? kfNormPiece(p) : [];
    return { id: typeof p.id === "string" && p.id ? p.id : cvNewId(), sx: cvClamp(n(p.sx, 0), 0, 1 - sw), sy: cvClamp(n(p.sy, 0), 0, 1 - sh), sw, sh, x: cvClamp(n(p.x, 0), -4, 4), y: cvClamp(n(p.y, 0), -4, 4), w, h, ...(keys.length ? { keys, smooth: !!p.smooth } : {}) };
  }).filter(Boolean);
  const bg = typeof c.background === "string" && (c.background === "blur" || c.background === "black" || /^#[0-9a-f]{6}$/i.test(c.background)) ? c.background : "black";
  return { width: cvEven(c.width), height: cvEven(c.height), background: bg, pieces };
}

/** A small picture of a layout: the canvas with its pieces as boxes. */
function cvThumb(C, pieces) {
  const W = 30, H = Math.round((30 * C.height) / C.width), k = Math.min(1, 44 / H);
  const boxes = pieces.map((p, i) => `<rect x="${(p.x * W).toFixed(1)}" y="${(p.y * H).toFixed(1)}" width="${(p.w * W).toFixed(1)}" height="${(p.h * H).toFixed(1)}" fill="${CV_COLORS[i % CV_COLORS.length]}" fill-opacity=".75" stroke="rgba(0,0,0,.45)" stroke-width=".8"/>`).join("");
  return `<svg class="cv-thumb" viewBox="0 0 ${W} ${H}" width="${Math.round(W * k)}" height="${Math.round(H * k)}" aria-hidden="true"><rect width="${W}" height="${H}" rx="2" fill="rgba(255,255,255,.1)"/>${boxes}</svg>`;
}

/* ---------- the canvas preview, the piece boxes and the side panel of one player ---------- */

/**
 * Mounts the canvas tools on the viewer `root` for clip `c` and player `P` (editor.js). Returns
 * the handle editor.js calls: paint(now), sync(what), setEdit(on), command(cmd), size(), destroy().
 */
function cvMount(root, c, P) {
  const box = root.querySelector(".player"), v = box?.querySelector("video"), pane = root.querySelector(".v-edit");
  if (!box || !v || !pane) return null;
  // The blurred background is an element of its own under the pieces: a small canvas of the clip, blurred and
  // darkened by CSS. (Blurring inside the pieces' canvas with ctx.filter, on a canvas too small for the browser
  // to keep on the graphics card, copied every video frame back to memory: the preview stuttered badly.)
  v.insertAdjacentHTML("afterend", `<div class="cv-bgw" hidden><canvas class="cv-bg"></canvas></div><canvas class="cv-view" hidden></canvas>`);
  box.querySelector(".p-audio").insertAdjacentHTML("beforebegin", `<div class="cv-layer" hidden></div>`);
  const view = box.querySelector(".cv-view"), layer = box.querySelector(".cv-layer");
  const bgw = box.querySelector(".cv-bgw"), bg = bgw.querySelector(".cv-bg");
  const hasFrames = typeof v.requestVideoFrameCallback === "function";
  const panel = document.createElement("section");
  panel.className = "e-sec e-cv";
  pane.insertBefore(panel, pane.querySelector(".e-ov") || pane.querySelector(".e-export"));
  // A button on the overlay tool bar turns "arrange the pieces" on and off.
  const bar = box.querySelector(".ov-bar");
  bar?.insertAdjacentHTML("afterbegin", `<button type="button" class="ov-tool cv-tool" data-cv-arrange hidden title="${esc(t("Arrange the canvas pieces"))}" aria-label="${esc(t("Arrange the canvas pieces"))}" aria-pressed="false"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M7 3h10v18H7zM7 9h10M7 15h10"/></svg></button><span class="ov-sep cv-sep" hidden></span>`);
  const barBtn = bar?.querySelector("[data-cv-arrange]"), barSep = bar?.querySelector(".cv-sep");

  const st = {
    editing: false, arrange: false, view: "result", draw: false, sel: null, custom: false,
    W: 0, H: 0, left: 0, top: 0, drag: null, band: null, lastPanel: "", lastLayer: "", lastT: -1, dirty: true, dead: false, wheel: null,
    drawAt: 0, bgAt: 0, bgDirty: true, bgSig: "", // when the pieces and the background were last drawn; the background's look as set
  };
  const m = () => P.model;
  const cv = () => m()?.canvas || null;
  const src = () => ({ w: v.videoWidth || c.width || 1920, h: v.videoHeight || c.height || 1080 });
  const piece = (id) => cv()?.pieces.find((p) => p.id === id) || null;
  /** A piece as it is at the playhead (keyframes.js): its own boxes when it has no keyframes. */
  const shown = (p, tm = P.time()) => (p && kfHas(p) ? kfPieceAt(p, tm, cv(), src()) : p);
  const on = () => st.editing && !!cv();
  const srcView = () => on() && st.arrange && st.view === "source";

  /* ----- one undo step for a gesture or a change ----- */
  function begin() {
    const mm = m();
    const tx = { before: emState(mm), undo: mm.undo.slice(), redo: mm.redo.slice() };
    emSnapshot(mm);
    return tx;
  }
  function finish(tx) {
    const mm = m();
    if (!mm || !tx) return false;
    if (emState(mm) === tx.before) { mm.undo = tx.undo; mm.redo = tx.redo; refresh(); return false; }
    P.edSync("canvas");
    return true;
  }
  const change = (fn) => { if (!m()) return false; const tx = begin(); fn(m()); return finish(tx); };
  const withIds = (pieces) => pieces.map((p) => ({ id: cvNewId(), ...p }));

  /* ----- where the picture is: the canvas (or, in the source view, the clip's picture) fitted in the player ----- */
  function place() {
    const br = box.getBoundingClientRect(), vr = v.getBoundingClientRect(), C = cv(), s = src();
    if (!vr.width || !vr.height || !C) return false;
    const aw = srcView() ? s.w : C.width, ah = srcView() ? s.h : C.height;
    const k = Math.min(vr.width / aw, vr.height / ah), W = aw * k, H = ah * k;
    const left = vr.left - br.left + (vr.width - W) / 2, top = vr.top - br.top + (vr.height - H) / 2;
    Object.assign(st, { W, H, left, top });
    const css = { left: `${left}px`, top: `${top}px`, width: `${W}px`, height: `${H}px` };
    Object.assign(layer.style, css);
    if (!srcView()) {
      Object.assign(view.style, css);
      const dpr = window.devicePixelRatio || 1;
      const pw = Math.max(2, Math.min(C.width, Math.round(W * dpr))), ph = Math.max(2, Math.round((pw * C.height) / C.width));
      if (view.width !== pw || view.height !== ph) { view.width = pw; view.height = ph; }
      // The background's canvas: small (it is blurred anyway), yet large enough to stay on the graphics card.
      Object.assign(bgw.style, css);
      const bw = Math.max(160, Math.ceil(Math.sqrt((24000 * C.width) / C.height))), bh = Math.max(2, Math.round((bw * C.height) / C.width));
      if (bg.width !== bw || bg.height !== bh) { bg.width = bw; bg.height = bh; st.bgDirty = true; }
      styleBg();
    }
    return true;
  }
  /** The background's look: the clip blurred and darkened (with the clip's color preview), or a plain color. */
  function styleBg() {
    const C = cv();
    if (!C) return;
    const blur = C.background === "blur";
    const color = !blur && /^#[0-9a-f]{6}$/i.test(C.background) ? C.background : "#000";
    const look = v.style.filter && v.style.filter !== "none" ? `${v.style.filter} ` : ""; // the clip's color preview ("none" = neutral)
    const css = blur ? `${look}blur(${(0.02 * st.H).toFixed(2)}px) brightness(0.7)` : "";
    const sig = `${blur}|${color}|${css}`;
    if (sig === st.bgSig) return;
    st.bgSig = sig;
    bg.hidden = !blur;
    bgw.style.background = color;
    bg.style.filter = css;
    st.bgDirty = true;
  }

  /* ----- the preview: the background under it, then each piece, as the export lays them ----- */
  function draw() {
    if (st.dead || !on() || srcView()) return;
    const C = cv(), s = src(), W = view.width, H = view.height;
    const ctx = view.getContext("2d");
    const tnow = performance.now();
    st.dirty = false;
    st.lastT = v.currentTime;
    st.drawAt = tnow;
    const ready = v.readyState >= 2;
    // The background changes slowly and is blurred: 30 pictures a second are plenty.
    if (C.background === "blur" && ready && (st.bgDirty || tnow - st.bgAt > 32)) {
      const bw = bg.width, bh = bg.height, k = Math.max(bw / s.w, bh / s.h), dw = s.w * k, dh = s.h * k;
      bg.getContext("2d").drawImage(v, (bw - dw) / 2, (bh - dh) / 2, dw, dh);
      st.bgAt = tnow;
      st.bgDirty = false;
    }
    ctx.clearRect(0, 0, W, H);
    if (!ready) return;
    // A still picture gets the best scaling. While playing, plain bilinear: the better kinds build a set of
    // smaller copies of the whole video frame for every piece, every frame.
    ctx.imageSmoothingQuality = v.paused ? "high" : "low";
    const tm = P.time();
    for (const p0 of C.pieces) {
      const p = shown(p0, tm);
      ctx.drawImage(v, p.sx * s.w, p.sy * s.h, p.sw * s.w, p.sh * s.h, p.x * W, p.y * H, p.w * W, p.h * H);
    }
  }
  // Every frame the video shows (also the one after a seek while paused).
  const onFrame = () => { if (st.dead) return; draw(); v.requestVideoFrameCallback(onFrame); };
  if (hasFrames) v.requestVideoFrameCallback(onFrame);
  const onSeeked = () => { st.dirty = true; st.bgDirty = true; draw(); };
  v.addEventListener("seeked", onSeeked);
  v.addEventListener("loadeddata", onSeeked);
  v.addEventListener("pause", onSeeked); // the still picture, drawn once more at full quality

  /* ----- the piece boxes over the picture (only while arranging) ----- */
  const HANDLES = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];
  const rectOf = (p) => (st.view === "source" ? { x: p.sx, y: p.sy, w: p.sw, h: p.sh } : p);
  const boxCss = (r) => `left:${(r.x * 100).toFixed(3)}%;top:${(r.y * 100).toFixed(3)}%;width:${(r.w * 100).toFixed(3)}%;height:${(r.h * 100).toFixed(3)}%`;
  function renderLayer() {
    const show = on() && st.arrange;
    layer.hidden = !show;
    box.classList.toggle("cv-arranging", show);
    box.classList.toggle("cv-src", srcView());
    layer.classList.toggle("drawing", show && st.draw);
    if (!show) { st.lastLayer = ""; layer.innerHTML = ""; return; }
    const tm = P.time();
    const html = cv().pieces.map((p, i) => `<div class="cv-box${p.id === st.sel ? " on" : ""}${kfHas(p) ? " kf-moving" : ""}" data-cv-id="${esc(p.id)}" style="--cvc:${CV_COLORS[i % CV_COLORS.length]};${boxCss(rectOf(shown(p, tm)))}"><b>${i + 1}</b>${p.id === st.sel ? HANDLES.map((h) => `<i class="cv-h cv-h-${h}" data-cv-h="${h}"></i>`).join("") : ""}</div>`).join("")
      + (st.band ? `<div class="cv-band" style="${boxCss(st.band)}"></div>` : "");
    if (html !== st.lastLayer) { st.lastLayer = html; layer.innerHTML = html; }
  }

  function refresh() {
    if (st.dead) return;
    const active = on();
    view.hidden = bgw.hidden = !active || srcView();
    box.classList.toggle("cv-on", active && !srcView());
    st.bgDirty = true;
    if (barBtn) { barBtn.hidden = barSep.hidden = !active; barBtn.classList.toggle("on", st.arrange); barBtn.setAttribute("aria-pressed", String(st.arrange)); }
    if (active) { place(); st.dirty = true; draw(); }
    renderLayer();
    renderPanel();
  }

  function setArrange(yes, viewName) {
    yes = !!yes && on();
    if (yes && !st.arrange) { P.ov?.setTool?.("select"); P.ov?.select?.(null); }
    st.arrange = yes;
    if (viewName) st.view = viewName;
    if (!yes) { st.draw = false; st.band = null; st.drag = null; st.view = "result"; }
    refresh();
    P.ov?.sync("canvas"); // the overlay layer follows the shown picture
  }
  function select(id) {
    st.sel = id && piece(id) ? id : null;
    renderLayer();
    renderPanel();
  }
  function removePiece(id) {
    if (!piece(id)) return false;
    if (st.sel === id) st.sel = null;
    return change((mm) => { mm.canvas.pieces = mm.canvas.pieces.filter((p) => p.id !== id); });
  }

  /* ----- pointer: select, move, resize, draw a new piece ----- */
  const at = (e) => { const r = layer.getBoundingClientRect(); return { fx: (e.clientX - r.left) / (r.width || 1), fy: (e.clientY - r.top) / (r.height || 1) }; };
  layer.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || !on()) return;
    e.preventDefault();
    e.stopPropagation();
    const q = at(e);
    if (st.draw) {
      st.drag = { kind: "new", x0: cvClamp(q.fx, 0, 1), y0: cvClamp(q.fy, 0, 1) };
      st.band = { x: st.drag.x0, y: st.drag.y0, w: 0, h: 0 };
    } else {
      const el = e.target.closest?.(".cv-box");
      if (!el) return select(null);
      const id = el.dataset.cvId, handle = e.target.closest?.("[data-cv-h]")?.dataset.cvH || "move";
      if (st.sel !== id) select(id);
      // `p0` is the piece as shown at the playhead; one with keyframes changes its keyframe there (`tm`).
      const tm = P.time();
      st.drag = { kind: "piece", id, handle, x0: q.fx, y0: q.fy, p0: { ...shown(piece(id), tm) }, own: { ...piece(id) }, tm, tx: begin() };
    }
    try { layer.setPointerCapture(e.pointerId); } catch { /* the drag still works while the pointer stays over the layer */ }
    renderLayer();
  });
  /** Edges the moved box may snap to (fractions): the canvas's edges and middle, and the other pieces' edges. */
  function snapTargets(id, axis) {
    const out = [0, 0.5, 1];
    for (const p of cv().pieces) if (p.id !== id) out.push(axis === "x" ? p.x : p.y, axis === "x" ? p.x + p.w : p.y + p.h);
    return out;
  }
  layer.addEventListener("pointermove", (e) => {
    const d = st.drag;
    if (!d) return;
    const q = at(e);
    if (d.kind === "new") {
      const x1 = cvClamp(q.fx, 0, 1), y1 = cvClamp(q.fy, 0, 1);
      st.band = { x: Math.min(d.x0, x1), y: Math.min(d.y0, y1), w: Math.abs(x1 - d.x0), h: Math.abs(y1 - d.y0) };
      return renderLayer();
    }
    const p = piece(d.id);
    if (!p) return;
    let dx = q.fx - d.x0, dy = q.fy - d.y0;
    let next;
    if (st.view === "source") next = cvSourceDrag(d.p0, d.handle, dx, dy);
    else {
      // Snap (not with Alt held): a moved box by its edges and middle, a dragged edge by itself.
      if (!e.altKey) {
        const tx = 7 / (st.W || 1), ty = 7 / (st.H || 1), p0 = d.p0;
        if (d.handle === "move") {
          dx = cvSnapBox(p0.x + dx, p0.w, snapTargets(d.id, "x"), tx) - p0.x;
          dy = cvSnapBox(p0.y + dy, p0.h, snapTargets(d.id, "y"), ty) - p0.y;
        } else if (d.handle === "e") dx = cvSnap(p0.x + p0.w + dx, snapTargets(d.id, "x"), tx) - p0.x - p0.w;
        else if (d.handle === "w") dx = cvSnap(p0.x + dx, snapTargets(d.id, "x"), tx) - p0.x;
        else if (d.handle === "s") dy = cvSnap(p0.y + p0.h + dy, snapTargets(d.id, "y"), ty) - p0.y - p0.h;
        else if (d.handle === "n") dy = cvSnap(p0.y + dy, snapTargets(d.id, "y"), ty) - p0.y;
      }
      next = cvResultDrag(d.p0, d.handle, dx, dy);
    }
    // Auto-key: a piece with keyframes gets (or changes) the one at the playhead.
    Object.assign(p, kfHas(d.own) ? kfPieceSet(d.own, d.tm, next, cv(), src()) : next);
    st.dirty = true;
    draw();
    renderLayer();
  });
  const dragEnd = (e) => {
    const d = st.drag;
    if (!d) return;
    st.drag = null;
    try { layer.releasePointerCapture(e.pointerId); } catch { /* not captured */ }
    if (d.kind === "new") {
      const b = st.band;
      st.band = null;
      st.draw = false;
      if (b && b.w >= CV_SRC_MIN && b.h >= CV_SRC_MIN) {
        const id = cvNewId();
        change((mm) => { mm.canvas.pieces.push({ id, ...cvPlaceNew({ sx: b.x, sy: b.y, sw: b.w, sh: b.h }, mm.canvas, src(), mm.canvas.pieces) }); });
        st.sel = id;
        st.view = "result"; // see it on the canvas, ready to be moved
        refresh();
        P.ov?.sync("canvas");
      } else refresh();
      return;
    }
    finish(d.tx);
  };
  layer.addEventListener("pointerup", dragEnd);
  layer.addEventListener("pointercancel", dragEnd);
  layer.addEventListener("dblclick", (e) => {
    // A double click on a piece shows the other view of it: where it is cut from, or where it goes.
    const el = e.target.closest?.(".cv-box");
    if (!el || st.draw) return;
    st.sel = el.dataset.cvId;
    setArrange(true, st.view === "source" ? "result" : "source");
  });
  // The wheel over a piece on the canvas zooms what it shows (one undo step per burst).
  layer.addEventListener("wheel", (e) => {
    const el = e.target.closest?.(".cv-box"), p = el && piece(el.dataset.cvId);
    if (!p || st.view !== "result" || st.drag) return;
    e.preventDefault();
    if (!st.wheel) st.wheel = { tx: begin(), timer: 0 };
    clearTimeout(st.wheel.timer);
    const tm = P.time(), z = cvZoom(shown(p, tm), e.deltaY > 0 ? 1.06 : 1 / 1.06);
    Object.assign(p, kfHas(p) ? kfPieceSet(p, tm, z, cv(), src()) : z);
    st.dirty = true;
    draw();
    st.wheel.timer = setTimeout(() => { const w = st.wheel; st.wheel = null; finish(w.tx); }, 350);
  }, { passive: false });

  /* ----- the side panel ----- */
  const presets = () => (Array.isArray(S.settings?.canvas_presets) ? S.settings.canvas_presets : []);
  function renderPanel() {
    if (st.dead) return;
    const mm = m(), C = cv();
    let html = `<div class="e-title">${esc(t("Canvas"))}</div>`;
    if (!mm) html += `<div class="small">${esc(t("Loading…"))}</div>`;
    else {
      const shape = !C ? "off" : st.custom ? "custom" : cvShapeOf(C);
      const opt = (val, label, cur) => `<option value="${esc(val)}"${val === cur ? " selected" : ""}>${esc(label)}</option>`;
      html += `<label class="cv-row"><span>${esc(t("Shape"))}</span><select data-cv="shape">${opt("off", t("Same as the clip"), shape)}${CV_SHAPES.map(([id]) => opt(id, cvShapeName(id), shape)).join("")}${opt("custom", t("Custom size"), shape)}</select></label>`;
      if (!C) html += `<div class="small">${esc(t("Pick a shape to turn the clip into a vertical video: choose which parts of the picture show and where."))}</div>`;
      else {
        if (shape === "custom") {
          html += `<div class="cv-row"><span>${esc(t("Size"))}</span><input type="number" class="ov-num" min="16" max="4096" step="2" data-cv="w" value="${C.width}" aria-label="${esc(t("Width"))}"><span class="ov-dash">×</span><input type="number" class="ov-num" min="16" max="4096" step="2" data-cv="h" value="${C.height}" aria-label="${esc(t("Height"))}"></div>`;
        } else {
          const sizes = CV_SHAPES.find(([id]) => id === shape)[1];
          html += `<label class="cv-row"><span>${esc(t("Size"))}</span><select data-cv="size">${sizes.map(([w, h], i) => opt(`${w}x${h}`, `${w} × ${h}${i === 1 ? ` (${t("usual")})` : ""}`, `${C.width}x${C.height}`)).join("")}</select></label>`;
        }
        const s = src();
        html += `<div class="cv-row"><span>${esc(t("Layout"))}</span><div class="cv-layouts">${cvLayoutsFor(C).map((k) => `<button type="button" class="cv-lay" data-cv-layout="${k}" title="${esc(`${cvLayoutName(k)}: ${cvLayoutHint(k)}`)}" aria-label="${esc(cvLayoutName(k))}">${cvThumb(C, cvLayout(k, C, s))}</button>`).join("")}</div></div>`;
        const bgKind = C.background === "blur" || C.background === "black" ? C.background : "color";
        html += `<label class="cv-row"><span>${esc(t("Background"))}</span><select data-cv="bg">${opt("blur", t("The clip, blurred"), bgKind)}${opt("black", t("Black"), bgKind)}${opt("color", t("A color"), bgKind)}</select>${bgKind === "color" ? `<input type="color" class="ov-pick" data-cv="bgcolor" value="${esc(C.background)}" aria-label="${esc(t("Background color"))}">` : ""}</label>`;
        html += `<div class="ov-btns"><button type="button" class="btn sm${st.arrange && !st.draw ? " on" : ""}" data-cv-act="arrange" aria-pressed="${st.arrange && !st.draw}">${esc(st.arrange ? t("Done arranging") : t("Arrange the pieces"))}</button><button type="button" class="btn sm${st.draw ? " on" : ""}" data-cv-act="add" aria-pressed="${st.draw}">${esc(t("Add a piece"))}</button></div>`;
        if (st.draw) html += `<div class="small">${esc(t("Drag a box on the clip's picture around what the new piece should show."))}</div>`;
        else if (st.arrange) {
          html += `<div class="seg cv-views" role="group" aria-label="${esc(t("What the player shows"))}"><button type="button" class="${st.view === "result" ? "on" : ""}" data-cv-view="result">${esc(t("The canvas"))}</button><button type="button" class="${st.view === "source" ? "on" : ""}" data-cv-view="source">${esc(t("The clip's picture"))}</button></div>`;
          html += `<div class="small">${esc(st.view === "result"
            ? t("Drag a piece to move it. A corner makes it bigger or smaller, an edge crops it. The mouse wheel zooms what it shows. Double-click it to see where it is cut from.")
            : t("Each box is what one piece shows. Drag it over another part of the picture; a corner zooms, an edge crops."))}</div>`;
        }
        if (st.arrange && C.pieces.length) {
          html += `<div class="ov-list cv-list" role="list">${C.pieces.map((p, i) => `<div role="listitem" class="ov-item${p.id === st.sel ? " on" : ""}" data-cv-pick="${esc(p.id)}"><span class="cv-dot" style="background:${CV_COLORS[i % CV_COLORS.length]}"></span><span class="ov-in">${esc(t("Piece {n}", { n: i + 1 }))}</span>${i < C.pieces.length - 1 ? `<button type="button" class="cv-mini" data-cv-front="${esc(p.id)}" title="${esc(t("Bring to the front"))}" aria-label="${esc(t("Bring to the front"))}">↑</button>` : ""}<button type="button" class="cv-mini" data-cv-del="${esc(p.id)}" title="${esc(t("Remove this piece"))}" aria-label="${esc(t("Remove this piece"))}">×</button></div>`).join("")}</div>`;
        }
        // The selected piece's keyframes (keyframes.js): a time bar with its diamonds, and the buttons.
        const sp = st.arrange && !st.draw ? piece(st.sel) : null;
        if (sp) {
          const D = mm.duration || 1, tm = P.time();
          html += `<div class="ov-when kf-piece"><div class="cv-row"><span>${esc(t("Piece {n}", { n: C.pieces.indexOf(sp) + 1 }))}</span></div>`
            + `<div class="ov-rbar kf-bar" title="${esc(t("Click to move the playhead"))}"><div class="ov-rnow" style="left:${cvClamp((tm / D) * 100, 0, 100)}%"></div>${kfDiamonds(sp.keys, D, tm)}</div>`
            + `${kfControls(sp.keys, tm, sp.smooth)}</div>`;
        }
      }
      const mine = presets();
      if (mine.length || C) {
        html += `<div class="cv-row cv-mine"><span>${esc(t("My layouts"))}</span><div class="cv-chips">${mine.map((p) => `<span class="cv-chip"><button type="button" data-cv-preset="${esc(p.id)}" title="${esc(`${p.canvas?.width} × ${p.canvas?.height}`)}">${esc(p.name)}</button><button type="button" class="cv-mini" data-cv-unsave="${esc(p.id)}" title="${esc(t("Delete this layout"))}" aria-label="${esc(t("Delete this layout"))}">×</button></span>`).join("")}${C ? `<button type="button" class="btn sm" data-cv-act="save">${esc(t("Save this layout…"))}</button>` : ""}</div></div>`;
      }
    }
    const busy = document.activeElement?.matches?.(".e-cv input:not([type=color]), .e-cv select");
    if (html !== st.lastPanel && !busy) { st.lastPanel = html; panel.innerHTML = html; }
  }

  /** A new canvas of this size, or the current one resized: the pieces keep their places and are cut to fit. */
  function setSize(w, h) {
    w = cvEven(w);
    h = cvEven(h);
    change((mm) => {
      const s = src();
      if (!mm.canvas) {
        const C = { width: w, height: h, background: "blur", pieces: [] };
        C.pieces = withIds(cvLayout("fill", C, s));
        mm.canvas = C;
      } else {
        const C0 = { width: mm.canvas.width, height: mm.canvas.height };
        mm.canvas.width = w;
        mm.canvas.height = h;
        // Keyframes are refitted the same way, each one (keyframes.js).
        mm.canvas.pieces = mm.canvas.pieces.map((p) => kfPieceRefit(cvRefit(p, mm.canvas, s), C0, mm.canvas, s));
      }
    });
  }
  function applyPreset(id) {
    const saved = presets().find((p) => p.id === id), C = cvNorm(saved?.canvas);
    if (!C) return;
    const s = src();
    C.pieces = C.pieces.map((p) => ({ ...cvRefit(p, C, s), id: cvNewId() }));
    st.custom = cvShapeOf(C) === "custom";
    st.sel = null;
    change((mm) => { mm.canvas = C; });
  }
  async function savePreset() {
    const C = cv();
    if (!C) return;
    const name = await askText(t("Name this layout"), "", t("Save"));
    if (!name || st.dead || !cv()) return;
    const now = cv();
    const entry = { id: cvNewId(), name, canvas: { width: now.width, height: now.height, background: now.background, pieces: now.pieces.map(({ id, keys, smooth, ...rest }) => rest) } }; // a layout is still: keyframes belong to this clip's times
    S.settings.canvas_presets = [...presets().filter((p) => p.name !== name), entry];
    saveSettings();
    st.lastPanel = "";
    renderPanel();
    flash(t("Layout saved. It is under “My layouts” in every clip."));
  }
  async function unsavePreset(id) {
    const saved = presets().find((p) => p.id === id);
    if (!saved || !(await ask(t("Delete the layout “{name}”?", { name: saved.name }), t("Delete")))) return;
    S.settings.canvas_presets = presets().filter((p) => p.id !== id);
    saveSettings();
    st.lastPanel = "";
    renderPanel();
  }

  /** The selected piece's keyframe buttons (keyframes.js): previous / next move the playhead; add, remove and Smooth are undo steps. */
  function keyAction(act, value) {
    const p = piece(st.sel), tm = P.time(), C = cv();
    if (!p || !C) return;
    if (act === "prev" || act === "next") {
      const to = act === "prev" ? kfPrev(p.keys, tm) : kfNext(p.keys, tm);
      if (to !== null) P.seek(to, false, true);
      refresh();
      return;
    }
    change(() => {
      const now = shown(p, tm);
      if (act === "add") p.keys = kfPut(p.keys, tm, kfPieceKey(now, C, src()));
      else if (act === "smooth") p.smooth = !!value;
      else if (act === "remove") {
        const keys = kfDrop(p.keys || [], tm);
        if (keys.length) p.keys = keys;
        else {
          // The last keyframe: the piece stays as it is now.
          Object.assign(p, { sx: now.sx, sy: now.sy, sw: now.sw, sh: now.sh, x: now.x, y: now.y, w: now.w, h: now.h });
          delete p.keys;
          delete p.smooth;
        }
      }
    });
  }
  const keySig = (tm) => { const p = st.arrange ? piece(st.sel) : null; return p ? `${kfIndexAt(p.keys, tm)}|${kfPrev(p.keys, tm)}|${kfNext(p.keys, tm)}` : ""; };
  // The piece's time bar: a click (on a diamond: exactly there) moves the playhead.
  panel.addEventListener("pointerdown", (e) => {
    const bar = e.target.closest?.(".kf-bar");
    if (!bar || e.button !== 0 || !m()) return;
    e.preventDefault();
    const dia = e.target.closest("[data-kf-t]"), r = bar.getBoundingClientRect();
    P.seek(dia ? +dia.dataset.kfT : cvClamp((e.clientX - r.left) / (r.width || 1), 0, 1) * (m().duration || 0), false, !!dia);
    refresh();
  });

  panel.addEventListener("mousedown", (e) => { if (e.target.closest("button")) e.preventDefault(); });
  panel.addEventListener("click", (e) => {
    const kb = e.target.closest("button[data-kf]");
    if (kb) { keyAction(kb.dataset.kf); return; }
    const b = e.target.closest("button, [data-cv-pick]");
    if (!b || !m()) return;
    const d = b.dataset;
    if (d.cvLayout) { st.sel = null; change((mm) => { mm.canvas.pieces = withIds(cvLayout(d.cvLayout, mm.canvas, src())); }); }
    else if (d.cvAct === "arrange") setArrange(!st.arrange || st.draw, "result");
    else if (d.cvAct === "add") { if (st.draw) { st.draw = false; refresh(); P.ov?.sync("canvas"); } else { st.draw = true; st.sel = null; setArrange(true, "source"); } }
    else if (d.cvAct === "save") savePreset();
    else if (d.cvView) { st.draw = false; setArrange(true, d.cvView); }
    else if (d.cvFront) change((mm) => { const p = mm.canvas.pieces.find((x) => x.id === d.cvFront); if (p) mm.canvas.pieces = [...mm.canvas.pieces.filter((x) => x !== p), p]; });
    else if (d.cvDel) removePiece(d.cvDel);
    else if (d.cvPreset) applyPreset(d.cvPreset);
    else if (d.cvUnsave) unsavePreset(d.cvUnsave);
    else if (d.cvPick) select(d.cvPick);
  });
  panel.addEventListener("change", (e) => {
    const el = e.target, k = el.dataset?.cv;
    if (el.dataset?.kf === "smooth") { el.blur(); keyAction("smooth", el.checked); return; }
    if (!k || !m()) return;
    if (k === "shape") {
      if (el.value === "off") { st.custom = false; st.sel = null; change((mm) => { mm.canvas = null; }); setArrange(false); }
      else if (el.value === "custom") { st.custom = true; if (!cv()) setSize(1080, 1920); else { st.lastPanel = ""; renderPanel(); } }
      else { st.custom = false; const [w, h] = CV_SHAPES.find(([id]) => id === el.value)[1][1]; setSize(w, h); }
    } else if (k === "size") { const [w, h] = el.value.split("x").map(Number); setSize(w, h); }
    else if (k === "w" || k === "h") { const C = cv(); if (C) setSize(k === "w" ? el.value : C.width, k === "h" ? el.value : C.height); }
    else if (k === "bg") change((mm) => { mm.canvas.background = el.value === "color" ? "#101418" : el.value; });
    else if (k === "bgcolor" && /^#[0-9a-f]{6}$/i.test(el.value)) change((mm) => { mm.canvas.background = el.value; });
    el.blur?.();
    st.lastPanel = "";
    renderPanel();
  });
  panel.addEventListener("keydown", (e) => { if ((e.target.dataset?.cv === "w" || e.target.dataset?.cv === "h") && (e.code === "Enter" || e.code === "NumpadEnter")) e.target.blur(); });
  // The tool bar: its own button toggles arranging; picking a drawing tool ends it.
  const onBar = (e) => {
    if (e.target.closest("[data-cv-arrange]")) setArrange(!st.arrange, "result");
    else if (st.arrange && e.target.closest("[data-ov-tool]")) setArrange(false);
  };
  bar?.addEventListener("click", onBar);

  const ro = typeof ResizeObserver === "function" ? new ResizeObserver(() => { if (on()) { place(); st.dirty = true; draw(); } }) : null;
  ro?.observe(box);
  v.addEventListener("loadedmetadata", () => { if (on()) refresh(); });

  return {
    /** Every frame while playing, and after every seek or edit (from applyColor). */
    paint() {
      if (st.dead || !on()) return;
      // The canvas takes the clip's color preview (a CSS filter on the video).
      if (view.style.filter !== v.style.filter) view.style.filter = v.style.filter;
      styleBg();
      // Each new video frame is drawn by the video's own frame callback (onFrame), once. This call comes at the
      // screen's refresh rate (up to 280 times a second), so it draws only after a change of the edit, or when
      // the frame callback has been silent for a moment (or does not exist).
      if (st.dirty || (v.currentTime !== st.lastT && performance.now() - st.drawAt > (hasFrames ? 50 : 0))) draw();
      // Keyframes (keyframes.js): the piece boxes follow moving pieces, the time bar and its buttons the playhead.
      if (st.arrange && !st.drag && cv().pieces.some(kfHas)) renderLayer();
      const tm = P.time(), nowEl = panel.querySelector(".kf-bar .ov-rnow");
      if (nowEl) nowEl.style.left = `${cvClamp((tm / (m()?.duration || 1)) * 100, 0, 100)}%`;
      const sig = keySig(tm);
      if (sig !== st.keySig) { st.keySig = sig; renderPanel(); }
    },
    /** After any change of the model (P.edSync), including undo / redo and a project's load. */
    sync(what) {
      if (st.dead) return;
      const mm = m();
      if (mm && what === "load" && mm.canvas) { mm.canvas = cvNorm(mm.canvas); if (mm.canvas) st.custom = cvShapeOf(mm.canvas) === "custom"; }
      if (st.sel && !piece(st.sel)) st.sel = null;
      if (!cv() && st.arrange) { st.arrange = false; st.draw = false; st.view = "result"; }
      refresh();
    },
    setEdit(yes) {
      st.editing = !!yes;
      if (!yes) { st.arrange = false; st.draw = false; st.view = "result"; st.sel = null; }
      refresh();
    },
    /** Delete and Esc first act on the pieces while arranging: true when they did. */
    command(cmd) {
      if (!st.arrange) return false;
      if (cmd === "remove") return st.sel ? (removePiece(st.sel), true) : false;
      if (cmd === "escape") {
        if (st.draw) { st.draw = false; st.band = null; st.drag = null; setArrange(true, "result"); }
        else if (st.sel) select(null);
        else setArrange(false);
        return true;
      }
      return false;
    },
    /** The size overlays are placed on and drawn at: the canvas's, or null for the clip's own (also while the clip's picture is shown). */
    size() { const C = cv(); return C && st.editing ? { width: C.width, height: C.height } : null; },
    /** True while the player shows the clip's own picture to pick what a piece shows (overlays hide then). */
    sourceShown: () => srcView(),
    destroy() {
      st.dead = true;
      clearTimeout(st.wheel?.timer);
      ro?.disconnect();
      v.removeEventListener("seeked", onSeeked);
      v.removeEventListener("loadeddata", onSeeked);
      v.removeEventListener("pause", onSeeked);
      bar?.removeEventListener("click", onBar);
      box.classList.remove("cv-on", "cv-arranging", "cv-src");
      view.remove();
      bgw.remove();
      layer.remove();
      panel.remove();
      barBtn?.remove();
      barSep?.remove();
    },
    // For tests and the preview.
    _st: st, setArrange, select, setSize, draw,
  };
}

// Lets the pure functions be tested under Node; ignored in the app.
if (typeof module !== "undefined" && module.exports) {
  module.exports = { CV_SHAPES, CV_MIN, CV_SRC_MIN, cvShapeOf, cvCrop, cvLayoutsFor, cvLayout, cvRefit, cvZoom, cvResultDrag, cvSourceDrag, cvPlaceNew, cvSnap, cvSnapBox, cvNorm, cvEven };
}
