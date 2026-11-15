"use strict";
// Editor overlays (0.1.25): text, shapes (rectangle, ellipse, line, arrow), freehand
// drawing and blur boxes on the picture, and the fade in / fade out of the result.
//
// The items live in the edit model (m.overlays, m.fadeIn, m.fadeOut; see editmodel.js),
// so undo / redo, project saves and emChanged cover them. Every position is a fraction
// (0..1) of the picture, every line width and text size a fraction of its HEIGHT, so an
// item looks the same at any size the player shows it and in the export. Times (from,
// to) are timeline seconds; the export gets them as output seconds (emOutRange).
//
// Preview: a layer over the shown picture (inside the letterbox bars): blur boxes are
// divs with backdrop-filter, everything else is drawn on a canvas by ovDraw, the fade
// is a black cover. Export: ovDraw paints the same items onto a canvas of the clip's
// own size, one transparent PNG per run of items that share a time range; the backend
// lays them over the picture with ffmpeg (editor.rs, ExportOpts::overlays), so no font
// is needed there and the export looks like the preview.

// 0.1.35: "image", "gif" and "video" are pictures, GIFs and clips on the picture (ui/media.js).
const OV_KINDS = ["text", "rect", "ellipse", "line", "arrow", "pen", "blur", "image", "gif", "video"];
const OV_MEDIA_PATHS = {
  image: "M4 5h16v14H4zM4 15l5-5 4 4 3-3 4 4M15 9.5a1.5 1.5 0 1 0 0-.01",
  gif: "M4 5h16v14H4zM8 10H6.5v4H8v-1.5M11 10v4M14 14v-4h2.5M14 12h2",
  video: "M4 6h11v12H4zM15 10l5-3v10l-5-3",
};
const OV_ADD_PATH = "M4 5h11M4 5v14h16v-6M4 15l5-5 4 4M18 3v6M15 6h6";
const OV_TOOLS = [
  ["select", "Select", "M5 3l13 8-6 1.5L9 19z"],
  ["text", "Text", "M5 6V4h14v2M12 4v16M9 20h6"],
  ["rect", "Rectangle", "M4 6h16v12H4z"],
  ["ellipse", "Ellipse", "M12 5c4.4 0 8 3.1 8 7s-3.6 7-8 7-8-3.1-8-7 3.6-7 8-7z"],
  ["line", "Line", "M5 19 19 5"],
  ["arrow", "Arrow", "M5 19 19 5M10 5h9v9"],
  ["pen", "Pen", "M4 20c3-1 4-4 6-7s4-6 7-8l2 2c-2 3-5 5-8 7s-6 3-7 6z"],
  ["blur", "Blur box", "M4 4h16v16H4zM8 8h2v2H8zM14 8h2v2h-2zM11 11h2v2h-2zM8 14h2v2H8zM14 14h2v2h-2z"],
];
/** Each tool's and kind's name, translated (t() with plain strings, so the translation checker sees them). */
const ovName = (k) => ({ select: t("Select"), text: t("Text"), rect: t("Rectangle"), ellipse: t("Ellipse"), line: t("Line"), arrow: t("Arrow"), pen: t("Pen"), blur: t("Blur box"), drawing: t("Drawing"), image: t("Picture"), gif: t("GIF"), video: t("Clip") })[k] || k;
/** The name an item goes by in lists and on the timeline (ui/timeline.js): a text's first line, a picture's file name, else its kind. */
function ovItemLabel(o) {
  if (!o) return "";
  if (o.kind === "text" && String(o.text || "").trim()) return String(o.text).trim().split("\n")[0].slice(0, 40);
  if (OV_MEDIA_PATHS[o.kind] && (o.title || o.path)) return String(o.title || String(o.path).split(/[\\/]/).pop()).slice(0, 60);
  return ovName(o.kind === "pen" ? "drawing" : o.kind);
}
/** An SVG path (24 x 24) for an item kind's icon. */
const ovKindPath = (kind) => OV_MEDIA_PATHS[kind] || OV_TOOLS.find((q) => q[0] === kind)?.[2] || "";
const OV_COLORS = ["#ffffff", "#111111", "#ff3b30", "#ff9500", "#ffcc00", "#34c759", "#0a84ff", "#af52de"];
const OV_FONT_FAMILY = '"Segoe UI", system-ui, sans-serif';
const OV_LINE_H = 1.2;              // text line height, times the font size
const OV_WIDTH_UNIT = 1 / 720;      // a thickness of 1 is one pixel of a 720p picture
const OV_MIN_SPAN = 0.1;            // shortest time range, seconds
const OV_STYLE_KEY = "glint.ov.style";
const OV_DEFAULT_STYLE = { color: "#ffffff", width: 4, size: 7, fill: "none", outline: true, strength: 6, round: 0, soft: 0 };
const ovClamp = (x, lo, hi) => Math.min(hi, Math.max(lo, x));
const ovNum = (x, d) => (Number.isFinite(+x) ? +x : d);
const ovColorOk = (c) => typeof c === "string" && /^#[0-9a-f]{6}$/i.test(c);
const ovNewId = () => "o" + Date.now().toString(36) + Math.random().toString(36).slice(2, 7);

/* ---------- pure model helpers (tested under Node) ---------- */

/** The tool style remembered between sessions (width 1..24, size 2..20 % of the height, strength 1..10). */
function ovStyleLoad() {
  let s = {};
  try { s = JSON.parse(localStorage.getItem(OV_STYLE_KEY) || "{}") || {}; } catch { s = {}; }
  return {
    color: ovColorOk(s.color) ? s.color : OV_DEFAULT_STYLE.color,
    width: ovClamp(Math.round(ovNum(s.width, OV_DEFAULT_STYLE.width)), 1, 24),
    size: ovClamp(Math.round(ovNum(s.size, OV_DEFAULT_STYLE.size)), 2, 20),
    fill: ["none", "solid", "half"].includes(s.fill) ? s.fill : "none",
    outline: s.outline !== false,
    strength: ovClamp(Math.round(ovNum(s.strength, OV_DEFAULT_STYLE.strength)), 1, 10),
    // A blur box's round corners and soft edge, 0..100 % (0.1.35).
    round: ovClamp(Math.round(ovNum(s.round, 0)), 0, 100),
    soft: ovClamp(Math.round(ovNum(s.soft, 0)), 0, 100),
  };
}
function ovStyleSave(s) { try { localStorage.setItem(OV_STYLE_KEY, JSON.stringify(s)); } catch { /* not remembered */ } }

/** Blur strength 1..10 -> Gaussian radius as a fraction of the picture's height (ExportOpts::blurs). */
const ovStrength = (level) => ovClamp(level, 1, 10) * 0.005;
const ovLevel = (strength) => ovClamp(Math.round(strength / 0.005), 1, 10);

/**
 * A new item of `kind` from a drag from point `a` to point `b` (fractions), with the
 * tool style `st`, on screen for the whole of a `dur` second timeline.
 */
function ovMake(kind, st, a, b, dur) {
  const base = { id: ovNewId(), kind, from: 0, to: Math.max(OV_MIN_SPAN, +dur || 0) };
  const box = () => ({ x: Math.min(a[0], b[0]), y: Math.min(a[1], b[1]), w: Math.abs(b[0] - a[0]), h: Math.abs(b[1] - a[1]) });
  switch (kind) {
    case "text": return { ...base, x: a[0], y: a[1], text: "", color: st.color, size: st.size / 100, outline: st.outline };
    case "rect": case "ellipse": return { ...base, ...box(), color: st.color, width: st.width * OV_WIDTH_UNIT, fill: st.fill };
    case "line": case "arrow": return { ...base, x1: a[0], y1: a[1], x2: b[0], y2: b[1], color: st.color, width: st.width * OV_WIDTH_UNIT };
    case "pen": return { ...base, pts: [[a[0], a[1]]], color: st.color, width: st.width * OV_WIDTH_UNIT };
    case "blur": return { ...base, ...box(), strength: ovStrength(st.strength), ...(st.round > 0 ? { round: st.round / 100 } : {}), ...(st.soft > 0 ? { soft: st.soft / 100 } : {}) };
  }
  return null;
}

/** A saved or restored item made safe to draw: known kind, numbers in range, or null. */
function ovNorm(o) {
  if (!o || typeof o !== "object" || !OV_KINDS.includes(o.kind) || typeof o.id !== "string") return null;
  const f = (x, d = 0) => ovClamp(ovNum(x, d), -0.5, 1.5);
  const out = { id: o.id, kind: o.kind, from: Math.max(0, ovNum(o.from, 0)), to: Math.max(0, ovNum(o.to, 0)) };
  if (OV_MEDIA_PATHS[o.kind]) {
    // A picture, GIF or clip (media.js): its box, then its own fields.
    Object.assign(out, { x: f(o.x), y: f(o.y), w: ovClamp(ovNum(o.w, 0.25), 0.001, 4), h: ovClamp(ovNum(o.h, 0.25), 0.001, 4) });
    if (typeof medNorm === "function") medNorm(o, out);
    const keys = Array.isArray(o.keys) && typeof kfNormOv === "function" ? kfNormOv(o) : [];
    if (keys.length) Object.assign(out, { keys, smooth: !!o.smooth });
    return out;
  }
  if (o.kind !== "blur") { out.color = ovColorOk(o.color) ? o.color : "#ffffff"; }
  if (o.kind === "text") Object.assign(out, { x: f(o.x), y: f(o.y), text: String(o.text ?? ""), size: ovClamp(ovNum(o.size, 0.07), 0.01, 0.5), outline: o.outline !== false });
  else if (o.kind === "line" || o.kind === "arrow") Object.assign(out, { x1: f(o.x1), y1: f(o.y1), x2: f(o.x2), y2: f(o.y2), width: ovClamp(ovNum(o.width, 4 / 720), 0.0005, 0.1) });
  else if (o.kind === "pen") Object.assign(out, { pts: Array.isArray(o.pts) ? o.pts.filter((p) => Array.isArray(p) && p.length >= 2).map((p) => [f(p[0]), f(p[1])]) : [], width: ovClamp(ovNum(o.width, 4 / 720), 0.0005, 0.1) });
  else {
    Object.assign(out, { x: f(o.x), y: f(o.y), w: ovClamp(ovNum(o.w, 0.1), 0, 2), h: ovClamp(ovNum(o.h, 0.1), 0, 2) });
    if (o.kind === "blur") {
      out.strength = ovClamp(ovNum(o.strength, 0.03), 0.001, 0.2);
      // Round corners and a soft edge (0.1.35), 0..1; an old box has neither.
      const rd = ovClamp(ovNum(o.round, 0), 0, 1), sf = ovClamp(ovNum(o.soft, 0), 0, 1);
      if (rd > 0) out.round = rd;
      if (sf > 0) out.soft = sf;
    }
    else Object.assign(out, { width: ovClamp(ovNum(o.width, 4 / 720), 0, 0.1), fill: ["none", "solid", "half"].includes(o.fill) ? o.fill : "none" });
  }
  // Keyframes (keyframes.js), when it has any.
  const keys = Array.isArray(o.keys) && typeof kfNormOv === "function" ? kfNormOv(o) : [];
  if (keys.length) Object.assign(out, { keys, smooth: !!o.smooth });
  return out;
}

/** True while the playhead (timeline seconds) is inside the item's time range. */
const ovVisibleAt = (o, now) => now >= o.from - 1e-6 && now <= o.to + 1e-6;

const ovFont = (px) => `700 ${px}px ${OV_FONT_FAMILY}`;
const ovLines = (o) => String(o.text || "").split("\n");

/** Width of a text item's widest line in pixels at picture height H; `measure(text, px)` gives a line's width. */
function ovTextBox(o, W, H, measure) {
  const px = o.size * H, lines = ovLines(o);
  const w = Math.max(px * 0.5, ...lines.map((l) => measure(l, px)));
  return { x: o.x * W, y: o.y * H, w, h: lines.length * px * OV_LINE_H };
}

/** The item's box in pixels of a W x H picture: { x, y, w, h }. */
function ovBBox(o, W, H, measure) {
  switch (o.kind) {
    case "text": return ovTextBox(o, W, H, measure);
    case "line": case "arrow": {
      const x = Math.min(o.x1, o.x2) * W, y = Math.min(o.y1, o.y2) * H;
      return { x, y, w: Math.abs(o.x2 - o.x1) * W, h: Math.abs(o.y2 - o.y1) * H };
    }
    case "pen": {
      if (!o.pts.length) return { x: 0, y: 0, w: 0, h: 0 };
      const xs = o.pts.map((p) => p[0] * W), ys = o.pts.map((p) => p[1] * H);
      const x = Math.min(...xs), y = Math.min(...ys);
      return { x, y, w: Math.max(...xs) - x, h: Math.max(...ys) - y };
    }
    default: return { x: o.x * W, y: o.y * H, w: o.w * W, h: o.h * H };
  }
}

/** Distance from point (px, py) to the segment (ax, ay)-(bx, by). */
function ovSegDist(px, py, ax, ay, bx, by) {
  const dx = bx - ax, dy = by - ay, len = dx * dx + dy * dy;
  const k = len ? ovClamp(((px - ax) * dx + (py - ay) * dy) / len, 0, 1) : 0;
  return Math.hypot(px - (ax + k * dx), py - (ay + k * dy));
}

/** True if pixel (px, py) of a W x H picture is on the item (within `tol` pixels). */
function ovHit(o, W, H, px, py, tol, measure) {
  const lw = (o.width || 0) * H / 2;
  if (o.kind === "line" || o.kind === "arrow") return ovSegDist(px, py, o.x1 * W, o.y1 * H, o.x2 * W, o.y2 * H) <= tol + lw;
  if (o.kind === "pen") {
    const p = o.pts;
    if (p.length === 1) return Math.hypot(px - p[0][0] * W, py - p[0][1] * H) <= tol + lw;
    for (let i = 1; i < p.length; i++) if (ovSegDist(px, py, p[i - 1][0] * W, p[i - 1][1] * H, p[i][0] * W, p[i][1] * H) <= tol + lw) return true;
    return false;
  }
  const b = ovBBox(o, W, H, measure);
  return px >= b.x - tol - lw && px <= b.x + b.w + tol + lw && py >= b.y - tol - lw && py <= b.y + b.h + tol + lw;
}

/** The top-most item shown at `now` under pixel (px, py), or null. */
function ovHitTop(list, now, W, H, px, py, tol, measure) {
  for (let i = list.length - 1; i >= 0; i--) if (ovVisibleAt(list[i], now) && ovHit(list[i], W, H, px, py, tol, measure)) return list[i];
  return null;
}

/** The grab handles of an item, in pixels: [{ k, x, y }]. A drawing has none (it is only moved). */
function ovHandles(o, W, H, measure) {
  if (o.kind === "line" || o.kind === "arrow") return [{ k: "p1", x: o.x1 * W, y: o.y1 * H }, { k: "p2", x: o.x2 * W, y: o.y2 * H }];
  if (o.kind === "pen") return [];
  const b = ovBBox(o, W, H, measure);
  if (o.kind === "text") return [{ k: "se", x: b.x + b.w, y: b.y + b.h }];
  return [{ k: "nw", x: b.x, y: b.y }, { k: "ne", x: b.x + b.w, y: b.y }, { k: "sw", x: b.x, y: b.y + b.h }, { k: "se", x: b.x + b.w, y: b.y + b.h }];
}

/** `o0` moved by (dx, dy) (fractions). Returns a new item. */
function ovMoved(o0, dx, dy) {
  const o = JSON.parse(JSON.stringify(o0));
  if (o.kind === "line" || o.kind === "arrow") { o.x1 += dx; o.y1 += dy; o.x2 += dx; o.y2 += dy; }
  else if (o.kind === "pen") o.pts = o.pts.map((p) => [p[0] + dx, p[1] + dy]);
  else { o.x += dx; o.y += dy; }
  return o;
}

/**
 * `o0` with handle `k` dragged to point (x, y) (fractions of a W x H picture). Boxes keep the
 * opposite corner; a text's corner sets its size; a line's end moves. Returns a new item.
 */
function ovResized(o0, k, x, y, W, H, measure) {
  const o = JSON.parse(JSON.stringify(o0));
  if (o.kind === "line" || o.kind === "arrow") {
    if (k === "p1") { o.x1 = x; o.y1 = y; } else { o.x2 = x; o.y2 = y; }
    return o;
  }
  if (o.kind === "text") {
    const lines = ovLines(o).length, b = ovTextBox(o, W, H, measure);
    // The box grows with the corner: whichever of width and height asks for more.
    const byH = (y - o.y) / (lines * OV_LINE_H);
    const byW = b.w > 0 ? o.size * (((x - o.x) * W) / b.w) : byH;
    o.size = ovClamp(Math.max(byH, byW), 0.015, 0.5);
    return o;
  }
  // A picture, GIF or clip keeps its proportions (media.js).
  if (OV_MEDIA_PATHS[o.kind] && typeof medResized === "function") return medResized(o0, k, x, y, W, H);
  const L = o.x, T = o.y, R = o.x + o.w, B = o.y + o.h;
  const ax = k.includes("w") ? R : L, ay = k.includes("n") ? B : T; // the corner that stays
  o.x = Math.min(ax, x); o.y = Math.min(ay, y);
  o.w = Math.abs(x - ax); o.h = Math.abs(y - ay);
  return o;
}

/** The items as the export's PNGs need them: runs of neighbours (bottom to top, blur boxes left out) that share an OUTPUT time range: [{ items, from, to }].
 *  An item with keyframes is a group of its own with `moving: true` (its own picture, moved by ffmpeg). */
function ovGroups(m) {
  const out = [];
  for (const o of m.overlays || []) {
    if (o.kind === "blur" || (o.kind === "text" && !String(o.text || "").trim()) || (o.kind === "pen" && !o.pts.length)) continue;
    // A picture, GIF or clip is an ffmpeg input of its own (media.js: medExportEntries), in its place in the order.
    if (OV_MEDIA_PATHS[o.kind]) { out.push({ items: [o], media: true }); continue; }
    const r = emOutRange(m, o.from, o.to);
    if (!r) continue;
    if (Array.isArray(o.keys) && o.keys.length) { out.push({ items: [o], from: r.from, to: r.to, moving: true }); continue; }
    const last = out[out.length - 1];
    if (last && !last.moving && !last.media && last.from === r.from && last.to === r.to) last.items.push(o);
    else out.push({ items: [o], from: r.from, to: r.to });
  }
  return out;
}

/** Opacity of the black fade cover at timeline second `now` (0 = none). */
function ovFadeAlpha(m, now) {
  const fi = +m.fadeIn || 0, fo = +m.fadeOut || 0;
  if (!(fi > 0 || fo > 0)) return 0;
  // The whole result, with the time after the video (0.1.35): the fade out ends where the export ends.
  const total = typeof emResultDuration === "function" ? emResultDuration(m) : emOutDuration(m), half = total / 2;
  const at = emSrcToOut(m, now) / (m.speed || 1);
  let a = 0;
  if (fi > 0) a = Math.max(a, 1 - at / Math.min(fi, half));
  if (fo > 0) a = Math.max(a, 1 - (total - at) / Math.min(fo, half));
  return ovClamp(a, 0, 1);
}

/** "1:02.5" for 62.5 seconds. */
function ovFmtTime(sec) {
  const s = Math.max(0, Math.round((+sec || 0) * 10) / 10), mins = Math.floor(s / 60), rest = s - mins * 60;
  return `${mins}:${rest < 10 ? "0" : ""}${rest.toFixed(1)}`;
}
/** Seconds from "1:02.5", "62.5" or "62,5"; null if it is not a time. */
function ovParseTime(text) {
  const s = String(text || "").trim().replace(",", ".");
  const m = /^(?:(\d+):)?(\d+(?:\.\d*)?|\.\d+)$/.exec(s);
  if (!m) return null;
  return (m[1] ? +m[1] * 60 : 0) + +m[2];
}

/** A time range kept inside the timeline and at least OV_MIN_SPAN long; `keep` ("from" / "to") is the end that wins. */
function ovSpan(from, to, dur, keep) {
  const D = Math.max(OV_MIN_SPAN, dur);
  let a = ovClamp(+from || 0, 0, D), b = ovClamp(+to || 0, 0, D);
  if (b - a < OV_MIN_SPAN) {
    if (keep === "to") a = Math.max(0, b - OV_MIN_SPAN), b = Math.max(b, a + OV_MIN_SPAN);
    else b = Math.min(D, a + OV_MIN_SPAN), a = Math.min(a, b - OV_MIN_SPAN);
  }
  return { from: a, to: b };
}

/* ---------- drawing (the preview and the export use the same code) ---------- */

/** Draws the items (blur boxes are left out: the preview and ffmpeg do those) on a W x H canvas. `skip`: an item id not to draw (the one being typed).
 *  Pictures, GIFs and clips (media.js) are drawn as they are at timeline second `tm`, clips from the player's `videos`. */
function ovDraw(ctx, list, W, H, skip, tm = 0, videos = null) {
  for (const o of list) {
    if (o.kind === "blur" || o.id === skip) continue;
    if (OV_MEDIA_PATHS[o.kind]) { if (typeof medDraw === "function") medDraw(ctx, o, W, H, tm, videos); continue; }
    ctx.save();
    const lw = Math.max(1, (o.width || 0) * H);
    ctx.strokeStyle = o.color;
    ctx.fillStyle = o.color;
    ctx.lineWidth = lw;
    ctx.lineJoin = "round";
    ctx.lineCap = "round";
    if (o.kind === "text") {
      const px = o.size * H;
      ctx.font = ovFont(px);
      ctx.textBaseline = "top";
      ovLines(o).forEach((line, i) => {
        const x = o.x * W, y = o.y * H + i * px * OV_LINE_H + px * 0.08;
        if (o.outline) {
          ctx.strokeStyle = "rgba(0, 0, 0, 0.85)";
          ctx.lineWidth = Math.max(2, px * 0.16);
          ctx.strokeText(line, x, y);
        }
        ctx.fillText(line, x, y);
      });
    } else if (o.kind === "rect" || o.kind === "ellipse") {
      ctx.beginPath();
      if (o.kind === "rect") ctx.rect(o.x * W, o.y * H, o.w * W, o.h * H);
      else ctx.ellipse((o.x + o.w / 2) * W, (o.y + o.h / 2) * H, Math.max(0.5, (o.w * W) / 2), Math.max(0.5, (o.h * H) / 2), 0, 0, Math.PI * 2);
      if (o.fill !== "none") { ctx.globalAlpha = o.fill === "half" ? 0.45 : 1; ctx.fill(); ctx.globalAlpha = 1; }
      if (o.width > 0) ctx.stroke();
    } else if (o.kind === "line" || o.kind === "arrow") {
      const x1 = o.x1 * W, y1 = o.y1 * H, x2 = o.x2 * W, y2 = o.y2 * H;
      const ang = Math.atan2(y2 - y1, x2 - x1), head = Math.max(lw * 3.2, H * 0.02);
      const len = Math.hypot(x2 - x1, y2 - y1);
      // An arrow's line stops inside its head, so the point stays sharp.
      const back = o.kind === "arrow" ? Math.min(len, head * 0.7) : 0;
      ctx.beginPath();
      ctx.moveTo(x1, y1);
      ctx.lineTo(x2 - Math.cos(ang) * back, y2 - Math.sin(ang) * back);
      ctx.stroke();
      if (o.kind === "arrow" && len > 0.5) {
        ctx.beginPath();
        ctx.moveTo(x2, y2);
        ctx.lineTo(x2 - head * Math.cos(ang - 0.45), y2 - head * Math.sin(ang - 0.45));
        ctx.lineTo(x2 - head * Math.cos(ang + 0.45), y2 - head * Math.sin(ang + 0.45));
        ctx.closePath();
        ctx.lineWidth = Math.max(1, lw * 0.5);
        ctx.fill();
        ctx.stroke();
      }
    } else if (o.kind === "pen" && o.pts.length) {
      const p = o.pts.map((q) => [q[0] * W, q[1] * H]);
      ctx.beginPath();
      ctx.moveTo(p[0][0], p[0][1]);
      if (p.length === 1) ctx.lineTo(p[0][0] + 0.01, p[0][1]);
      // Smoothed through the midpoints between the recorded points.
      for (let i = 1; i < p.length - 1; i++) ctx.quadraticCurveTo(p[i][0], p[i][1], (p[i][0] + p[i + 1][0]) / 2, (p[i][1] + p[i + 1][1]) / 2);
      if (p.length > 1) ctx.lineTo(p[p.length - 1][0], p[p.length - 1][1]);
      ctx.stroke();
    }
    ctx.restore();
  }
}

/** A line's width in pixels with the text font, from a shared canvas (cached). */
const ovMeasure = (() => {
  let ctx = null;
  const cache = new Map();
  return (text, px) => {
    const key = `${px.toFixed(2)}|${text}`;
    if (cache.has(key)) return cache.get(key);
    if (!ctx) ctx = (typeof document !== "undefined" ? document.createElement("canvas").getContext("2d") : null);
    const w = ctx ? (ctx.font = ovFont(px), ctx.measureText(text).width) : text.length * px * 0.55;
    if (cache.size > 2000) cache.clear();
    cache.set(key, w);
    return w;
  };
})();

/* ---------- keys while a text is typed ---------- */
// Registered at load time, before app.js adds its window key listener (which closes the viewer on Esc
// even in a text box): Enter and Esc in the overlay text box are the text box's own.
if (typeof window !== "undefined") {
  window.addEventListener("keydown", (e) => {
    if (!e.target?.classList?.contains("ov-edit")) return;
    if (e.code === "Escape" || ((e.code === "Enter" || e.code === "NumpadEnter") && !e.shiftKey)) {
      e.preventDefault();
      e.stopImmediatePropagation();
      e.target.dispatchEvent(new CustomEvent("ov-key", { detail: e.code === "Escape" ? "cancel" : "commit" }));
    }
  }, true);
}

/* ---------- the overlay layer, tool bar and side panel of one player ---------- */

/**
 * Mounts the overlay tools on the viewer `root` for clip `c` and player `P` (editor.js).
 * Returns the handle editor.js calls: paint(now), sync(what), setEdit(on), command(cmd),
 * tlSelected(), fadeGain(now), exportPngs(), destroy().
 */
function ovMount(root, c, P) {
  const box = root.querySelector(".player"), v = box?.querySelector("video"), pane = root.querySelector(".v-edit");
  if (!box || !v || !pane) return null;
  const svg = (d) => `<svg viewBox="0 0 24 24" aria-hidden="true"><path d="${d}"/></svg>`;

  box.querySelector(".p-audio").insertAdjacentHTML("beforebegin", `
    <div class="ov-layer" hidden>
      <div class="ov-blurs"></div>
      <canvas class="ov-art"></canvas>
      <div class="ov-fade"></div>
      <canvas class="ov-ui"></canvas>
      <textarea class="ov-edit" hidden spellcheck="false" rows="1" aria-label="${esc(t("Text"))}"></textarea>
    </div>
    <div class="ov-bar" hidden role="toolbar" aria-label="${esc(t("Text and drawing"))}">
      ${OV_TOOLS.map(([k, , d]) => `<button type="button" class="ov-tool" data-ov-tool="${k}" title="${esc(ovName(k))}" aria-label="${esc(ovName(k))}" aria-pressed="false">${svg(d)}</button>`).join("")}
      <span class="ov-sep"></span>
      <button type="button" class="ov-tool" data-ov-add title="${esc(t("Add a picture, GIF or clip"))}" aria-label="${esc(t("Add a picture, GIF or clip"))}">${svg(OV_ADD_PATH)}</button>
      <span class="ov-sep"></span>
      <button type="button" class="ov-tool ov-del" data-ov-del title="${esc(t("Delete the selected item"))}" aria-label="${esc(t("Delete the selected item"))}" disabled>${svg("M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13")}</button>
    </div>
    <div class="med-offer" hidden role="status"></div>`);
  const layer = box.querySelector(".ov-layer"), bar = box.querySelector(".ov-bar"), offer = box.querySelector(".med-offer");
  const art = layer.querySelector(".ov-art"), ui = layer.querySelector(".ov-ui"), fadeEl = layer.querySelector(".ov-fade");
  const blursEl = layer.querySelector(".ov-blurs"), editor = layer.querySelector(".ov-edit");
  const exportSec = pane.querySelector(".e-export");
  const panel = document.createElement("section");
  panel.className = "e-sec e-ov";
  const fadeSec = document.createElement("section");
  fadeSec.className = "e-sec e-fade";
  pane.insertBefore(panel, exportSec);
  pane.insertBefore(fadeSec, exportSec);

  const st = {
    tool: "select", sel: null, style: ovStyleLoad(),
    W: 0, H: 0, left: 0, top: 0, dpr: 1, // the shown picture, in CSS pixels of the player
    shown: "", dirty: true, drag: null, typing: null, rdrag: null, slide: null,
    lastPanel: "", lastFade: "", editing: false, dead: false,
    crop: null, // the id of the picture / GIF / clip being cropped (media.js)
  };
  // Each clip item's own muted <video>, kept at its time (media.js); a frame that arrives is drawn.
  const videos = typeof medVideoSet === "function" ? medVideoSet(() => { if (!st.dead && st.editing) { st.dirty = true; redraw(); } }) : null;
  const playing = () => (typeof P.playing === "function" ? P.playing() : !v.paused);
  const isMedia = (o) => !!o && !!OV_MEDIA_PATHS[o.kind];
  const m = () => P.model;
  const list = () => m()?.overlays || [];
  const item = (id) => list().find((o) => o.id === id) || null;
  const dur = () => m()?.duration || P.duration();
  const tol = () => 6;

  /* ----- one undo step for a gesture (a drag, typing a text, a slider) ----- */
  function begin() {
    const mm = m();
    const tx = { before: edState(mm), undo: mm.undo.slice(), redo: mm.redo.slice() };
    emSnapshot(mm);
    return tx;
  }
  function finish(tx, what = "overlay") {
    const mm = m();
    if (!mm || !tx) return false;
    if (edState(mm) === tx.before) { mm.undo = tx.undo; mm.redo = tx.redo; redraw(); renderPanel(); return false; }
    P.edSync(what);
    return true;
  }
  /** A change made in one go (a button, a typed value): snapshot, change, sync. */
  const change = (fn) => { if (!m()) return false; const tx = begin(); fn(m()); return finish(tx); };

  /* ----- where the picture is ----- */
  function place() {
    const br = box.getBoundingClientRect(), vr = v.getBoundingClientRect();
    const cs = P.cv?.size?.(); // a canvas (canvas.js): the overlays lie on it, not on the clip's own picture
    const vw = cs ? cs.width : v.videoWidth || c.width || 16, vh = cs ? cs.height : v.videoHeight || c.height || 9;
    if (!vr.width || !vr.height) return false;
    const k = Math.min(vr.width / vw, vr.height / vh), W = vw * k, H = vh * k;
    const left = vr.left - br.left + (vr.width - W) / 2, top = vr.top - br.top + (vr.height - H) / 2;
    const dpr = window.devicePixelRatio || 1;
    if (Math.abs(W - st.W) < 0.5 && Math.abs(H - st.H) < 0.5 && Math.abs(left - st.left) < 0.5 && Math.abs(top - st.top) < 0.5 && dpr === st.dpr) return false;
    Object.assign(st, { W, H, left, top, dpr });
    Object.assign(layer.style, { left: `${left}px`, top: `${top}px`, width: `${W}px`, height: `${H}px` });
    for (const cv of [art, ui]) { cv.width = Math.max(1, Math.round(W * dpr)); cv.height = Math.max(1, Math.round(H * dpr)); }
    st.dirty = true;
    return true;
  }
  const ro = typeof ResizeObserver === "function" ? new ResizeObserver(() => { if (st.editing && place()) redraw(); }) : null;
  ro?.observe(box);
  ro?.observe(v);
  v.addEventListener("loadedmetadata", () => { if (st.editing) { place(); redraw(); } });

  /* ----- drawing the preview ----- */
  const now = () => P.time();
  /** An item as it is shown at `tm`: with its keyframes' move built in (keyframes.js). */
  const atTime = (o, tm) => (o && kfHas(o) ? kfOvAt(o, tm, st.W || 1, st.H || 1, ovMeasure) : o);
  /** The items shown at `tm`, as they are shown then (for hit tests). */
  const shownAt = (tm) => list().map((o) => atTime(o, tm));
  function redraw() {
    if (st.dead || !st.editing) return;
    place();
    const mm = m(), tm = now(), W = st.W, H = st.H;
    const shown = mm ? list().filter((o) => ovVisibleAt(o, tm)) : [];
    st.shown = shown.map((o) => o.id).join(",");
    st.dirty = false;
    // Pictures load once; GIFs and clips change with the playhead, like items with keyframes (media.js).
    const media = shown.filter(isMedia);
    for (const o of media) if (o.kind !== "video" && typeof medLoad === "function") medLoad(o, () => { if (!st.dead) { st.dirty = true; redraw(); } });
    videos?.sync(media.filter((o) => o.kind === "video"), tm, playing());
    st.moving = shown.some(kfHas) || media.some((o) => o.kind !== "image");
    st.lastT = tm;
    const ctx = art.getContext("2d");
    ctx.setTransform(st.dpr, 0, 0, st.dpr, 0, 0);
    ctx.clearRect(0, 0, W, H);
    kfOvDraw(ctx, shown, tm, W, H, st.typing?.id, ovMeasure, videos);
    // Blur boxes: one div each, blurring what lies under it.
    const blurs = shown.filter((o) => o.kind === "blur").map((o) => atTime(o, tm));
    while (blursEl.children.length > blurs.length) blursEl.lastChild.remove();
    while (blursEl.children.length < blurs.length) blursEl.appendChild(document.createElement("div"));
    blurs.forEach((o, i) => {
      const d = blursEl.children[i];
      Object.assign(d.style, { left: `${o.x * W}px`, top: `${o.y * H}px`, width: `${o.w * W}px`, height: `${o.h * H}px` });
      const f = `blur(${(o.strength * H).toFixed(2)}px)`;
      d.style.backdropFilter = f;
      d.style.webkitBackdropFilter = f;
      // Round corners and a soft edge: the same mask the export lays on its blurred copy (media.js, edmedia.rs).
      const mk = typeof medWantsMask === "function" && medWantsMask(o) ? `url("${medMaskUrl(o.w * W, o.h * H, +o.round || 0, +o.soft || 0)}")` : "";
      if (d._mk !== mk) {
        d._mk = mk;
        for (const p of ["maskImage", "webkitMaskImage"]) d.style[p] = mk || "none";
        for (const p of ["maskSize", "webkitMaskSize"]) d.style[p] = mk ? "100% 100%" : "";
      }
    });
    drawUi();
    paintFade(tm);
  }
  function drawUi() {
    const ctx = ui.getContext("2d"), W = st.W, H = st.H;
    ctx.setTransform(st.dpr, 0, 0, st.dpr, 0, 0);
    ctx.clearRect(0, 0, W, H);
    const draft = st.drag?.kind === "draw" ? item(st.drag.id) : null;
    const o = atTime(item(st.sel), now());
    if (!o && !draft) return;
    const accent = getComputedStyle(root).getPropertyValue("--accent").trim() || "#3b82f6";
    // A blur box being drawn shows its outline, since its inside only blurs.
    if (draft && draft.kind === "blur") { const b = ovBBox(draft, W, H, ovMeasure); ctx.setLineDash([5, 4]); ctx.strokeStyle = "#fff"; ctx.lineWidth = 1; ctx.strokeRect(b.x + 0.5, b.y + 0.5, b.w, b.h); ctx.setLineDash([]); }
    if (!o || !ovVisibleAt(o, now())) return;
    if (st.crop === o.id && isMedia(o)) {
      // Cropping: what is cut off shows faintly around the box; the box has a handle on each corner and edge.
      medDrawCropped(ctx, o, W, H, now(), videos);
      const F = medFullBox(o);
      ctx.setLineDash([4, 4]);
      ctx.lineWidth = 1;
      ctx.strokeStyle = "rgba(255,255,255,.55)";
      ctx.strokeRect(F.x * W + 0.5, F.y * H + 0.5, F.w * W, F.h * H);
      ctx.setLineDash([]);
      ctx.lineWidth = 2;
      ctx.strokeStyle = accent;
      ctx.strokeRect(o.x * W, o.y * H, o.w * W, o.h * H);
      for (const h of medCropHandles(o, W, H)) {
        ctx.fillStyle = "#fff";
        ctx.beginPath();
        if (h.k.length === 2) ctx.rect(h.x - 6, h.y - 6, 12, 12);
        else if (h.k === "n" || h.k === "s") ctx.rect(h.x - 9, h.y - 3, 18, 6);
        else ctx.rect(h.x - 3, h.y - 9, 6, 18);
        ctx.fill();
        ctx.stroke();
      }
      return;
    }
    const b = ovBBox(o, W, H, ovMeasure), pad = 4;
    ctx.setLineDash([5, 4]);
    ctx.lineWidth = 1.5;
    ctx.strokeStyle = "rgba(0,0,0,.6)";
    ctx.strokeRect(b.x - pad, b.y - pad, b.w + pad * 2, b.h + pad * 2);
    ctx.setLineDash([5, 4]);
    ctx.lineDashOffset = 4;
    ctx.strokeStyle = accent;
    ctx.strokeRect(b.x - pad, b.y - pad, b.w + pad * 2, b.h + pad * 2);
    ctx.setLineDash([]);
    for (const h of ovHandles(o, W, H, ovMeasure)) {
      ctx.fillStyle = "#fff";
      ctx.strokeStyle = accent;
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.rect(h.x - 5, h.y - 5, 10, 10);
      ctx.fill();
      ctx.stroke();
    }
  }
  function paintFade(tm) {
    const mm = m();
    const a = mm ? ovFadeAlpha(mm, tm) : 0;
    const s = a > 0.001 ? a.toFixed(3) : "0";
    if (fadeEl.style.opacity !== s) fadeEl.style.opacity = s;
  }

  /* ----- pointer: draw, select, move, resize ----- */
  const at = (e) => { const r = layer.getBoundingClientRect(); return { px: e.clientX - r.left, py: e.clientY - r.top }; };
  const frac = (p) => [p.px / (st.W || 1), p.py / (st.H || 1)];
  function setTool(k) {
    if (!OV_TOOLS.some((x) => x[0] === k)) return;
    commitText();
    st.tool = k;
    for (const b of bar.querySelectorAll("[data-ov-tool]")) { const on = b.dataset.ovTool === k; b.classList.toggle("on", on); b.setAttribute("aria-pressed", String(on)); }
    layer.classList.toggle("drawing", k !== "select");
    box.classList.toggle("ov-drawing", k !== "select");
    renderPanel();
  }
  function select(id) {
    if (st.sel === id) return;
    if (st.crop && st.crop !== id) endCrop();
    st.sel = id;
    // The timeline's block of the item lights up (ui/timeline.js).
    try { P.tl?.overlaySel?.(id); } catch (e) { console.warn("Glint: overlaySel failed", e); }
    // A picked overlay and a picked timeline segment would both answer Delete: the timeline lets go.
    if (id && P.selection?.().sels.length && typeof P.tl?.run === "function") { try { P.tl.run("clearSel"); } catch (e) { console.warn("Glint: clearSel failed", e); } }
    bar.querySelector("[data-ov-del]").disabled = !id;
    drawUi();
    renderPanel();
  }

  layer.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || !m() || st.typing) return;
    const p = at(e), pt = frac(p), mm = m(), tm = now();
    if (st.crop) {
      // Cropping (media.js): an edge or corner of the box is dragged; a click outside it ends the crop.
      const own = item(st.crop), shown = atTime(own, tm);
      const h = own && ovVisibleAt(own, tm) ? medCropHandles(shown, st.W, st.H).find((q) => Math.abs(q.x - p.px) <= 9 && Math.abs(q.y - p.py) <= 9) : null;
      if (h) {
        st.drag = { kind: "crop", handle: h.k, id: own.id, tx: begin(), x: p.px, y: p.py, moved: false, base: JSON.parse(JSON.stringify(own)), tm };
        layer.setPointerCapture(e.pointerId);
        return;
      }
      if (shown && ovHit(shown, st.W, st.H, p.px, p.py, 0, ovMeasure)) return;
      endCrop();
    }
    if (st.tool === "select") {
      const o = atTime(item(st.sel), tm);
      const h = o && ovVisibleAt(o, tm) ? ovHandles(o, st.W, st.H, ovMeasure).find((q) => Math.abs(q.x - p.px) <= 7 && Math.abs(q.y - p.py) <= 7) : null;
      const hit = h ? o : ovHitTop(shownAt(tm), tm, st.W, st.H, p.px, p.py, tol(), ovMeasure);
      if (!hit) { select(null); st.drag = { kind: "click", x: p.px, y: p.py }; layer.setPointerCapture(e.pointerId); return; }
      select(hit.id);
      // `orig` is the item as shown; one with keyframes changes its keyframe at the playhead (`tm`) instead of its shape.
      const own = item(hit.id), keyed = kfHas(own);
      st.drag = { kind: h ? "resize" : "move", handle: h?.k, id: hit.id, start: pt, orig: JSON.parse(JSON.stringify(hit)), tx: begin(), x: p.px, y: p.py, moved: false,
        keyed, tm, base: keyed ? JSON.parse(JSON.stringify(own)) : null, T0: keyed ? kfOvT(own, tm) : null };
    } else if (st.tool === "text") {
      startText(pt);
      e.preventDefault(); // keep the focus in the new text box
      return;
    } else {
      const tx = begin();
      // A new item is on screen for the whole clip; its time range can then be narrowed.
      const o = ovMake(st.tool, st.style, pt, pt, mm.duration);
      mm.overlays.push(o);
      select(o.id);
      st.drag = { kind: "draw", id: o.id, start: pt, tx, x: p.px, y: p.py, moved: false };
    }
    layer.setPointerCapture(e.pointerId);
    redraw();
  });
  layer.addEventListener("pointermove", (e) => {
    const d = st.drag;
    if (!d) { hover(e); return; }
    const p = at(e), pt = frac(p);
    if (!d.moved && Math.hypot(p.px - d.x, p.py - d.y) < 3) return;
    d.moved = true;
    if (d.kind === "click") return;
    const mm = m(), i = mm.overlays.findIndex((o) => o.id === d.id);
    if (i < 0) return;
    const cur = mm.overlays[i];
    let next = cur;
    if (d.kind === "crop") {
      // The pointer in the item's own frame (its keyframe move at the playhead taken off), then the keys follow the new box.
      const b0 = d.base, T = kfOvT(b0, d.tm), mx = b0.x + b0.w / 2, my = b0.y + b0.h / 2;
      const q = kfHas(b0) ? medUnmove(pt, T, mx, my) : pt;
      next = medCropDrag(b0, d.handle, q[0], q[1]);
      if (kfHas(b0)) next.keys = medCropKeys(b0.keys, b0, next);
      mm.overlays[i] = next;
      redraw();
      return;
    }
    if (d.keyed && (d.kind === "move" || d.kind === "resize")) {
      // Auto-key: the drag sets the keyframe at the playhead (or adds one there).
      let T = d.kind === "move" ? kfOvMoveT(d.T0, pt[0] - d.start[0], pt[1] - d.start[1])
        : kfOvResizeT(d.base, d.T0, d.orig, ovResized(d.orig, d.handle, pt[0], pt[1], st.W, st.H, ovMeasure), d.handle, st.W, st.H, ovMeasure);
      if (KF_UNIFORM.includes(cur.kind)) T = { ...T, sy: T.sx };
      next = { ...cur, keys: kfPut(d.base.keys, d.tm, T) };
    } else if (d.kind === "move") next = ovMoved(d.orig, pt[0] - d.start[0], pt[1] - d.start[1]);
    else if (d.kind === "resize") next = ovResized(d.orig, d.handle, pt[0], pt[1], st.W, st.H, ovMeasure);
    else if (cur.kind === "pen") {
      const last = cur.pts[cur.pts.length - 1];
      if (Math.hypot((pt[0] - last[0]) * st.W, (pt[1] - last[1]) * st.H) >= 2) cur.pts.push([pt[0], pt[1]]);
    } else if (cur.kind === "line" || cur.kind === "arrow") { cur.x2 = pt[0]; cur.y2 = pt[1]; }
    else Object.assign(cur, { x: Math.min(d.start[0], pt[0]), y: Math.min(d.start[1], pt[1]), w: Math.abs(pt[0] - d.start[0]), h: Math.abs(pt[1] - d.start[1]) });
    if (next !== cur) mm.overlays[i] = next;
    redraw();
  });
  const endDrag = (e) => {
    const d = st.drag;
    if (!d) return;
    st.drag = null;
    try { layer.releasePointerCapture(e.pointerId); } catch { /* already released */ }
    if (d.kind === "click") { if (!d.moved && e.type === "pointerup") P.toggle(); return; }
    const o = item(d.id);
    if (d.kind === "draw" && o) {
      // A click without a drag: a shape of a usable size around the point (a line goes right).
      const dw = 0.18, dh = 0.18 * (st.W / (st.H || 1));
      if (!d.moved) {
        if (o.kind === "line" || o.kind === "arrow") Object.assign(o, { x1: d.start[0] - dw / 2, x2: d.start[0] + dw / 2 });
        else if (o.kind === "pen") { /* a dot */ }
        else Object.assign(o, { x: d.start[0] - dw / 2, y: d.start[1] - dh / 4, w: dw, h: dh / 2 });
      } else if ((o.kind === "rect" || o.kind === "ellipse" || o.kind === "blur") && (o.w * st.W < 4 || o.h * st.H < 4)) {
        o.w = Math.max(o.w, 4 / (st.W || 1));
        o.h = Math.max(o.h, 4 / (st.H || 1));
      }
      if (o.kind !== "pen") setTool("select"); // the new item is ready to be adjusted
    }
    finish(d.tx);
    redraw();
    renderPanel();
  };
  layer.addEventListener("pointerup", endDrag);
  layer.addEventListener("pointercancel", endDrag);
  layer.addEventListener("dblclick", (e) => {
    if (!m() || st.typing) return;
    const p = at(e);
    const hit = ovHitTop(shownAt(now()), now(), st.W, st.H, p.px, p.py, tol(), ovMeasure);
    if (hit?.kind === "text") { e.preventDefault(); editText(hit.id); }
    else if (!hit && st.tool === "select") P.fullscreen(); // as a double-click on the video does
  });
  /* ----- pictures, GIFs and clips (media.js); crop; order; the right-click menu ----- */
  /** Adds an editor_media_info result as a new item at the playhead (one undo step); a clip with sound offers its sound too. */
  function addMedia(info) {
    if (!m() || !info?.path) return null;
    commitText();
    const cs = P.cv?.size?.();
    const pw = cs ? cs.width : v.videoWidth || c.width || 16, ph = cs ? cs.height : v.videoHeight || c.height || 9;
    const o = medMake(info, now(), pw, ph);
    change((mm) => { mm.overlays.push(o); });
    setTool("select");
    select(o.id);
    redraw();
    if (info.kind === "video" && info.audio) offerSound(info, o.id);
    return o.id;
  }
  /** "Add its sound too": a short note on the player with a button; the clip's file becomes an audio row at the item's place. */
  function offerSound(info, id) {
    clearTimeout(st.offerTimer);
    offer.innerHTML = `<span>${esc(t("This clip has sound. It is not heard on the picture."))}</span><button type="button" class="btn sm" data-med-sound>${esc(t("Add its sound too"))}</button><button type="button" class="med-x" data-med-x aria-label="${esc(t("Close"))}">×</button>`;
    offer.hidden = false;
    offer.onclick = async (e) => {
      if (e.target.closest("[data-med-x]")) { offer.hidden = true; return; }
      if (!e.target.closest("[data-med-sound]")) return;
      offer.hidden = true;
      const o = item(id);
      if (!o || typeof P.addFileRow !== "function") return;
      try {
        const a = await invoke("editor_audio_info", { path: info.path });
        if (st.dead || !item(id)) return;
        const was = P.time();
        // The row starts where the item does (emAddFile puts it at the playhead).
        if (Math.abs(was - o.from) > 0.01) P.seek(o.from, false, true);
        P.addFileRow(a);
        flash(t("Added {name} as a new audio track", { name: a.title || o.title }));
      } catch (err) { flash(String(err?.message ?? err), true); }
    };
    st.offerTimer = setTimeout(() => { offer.hidden = true; }, 12000);
  }
  /** Starts cropping media item `id` (its edges and corners are dragged inward, freely). */
  function startCrop(id) {
    const o = item(id);
    if (!isMedia(o)) return false;
    commitText();
    if (st.tool !== "select") setTool("select");
    if (!ovVisibleAt(o, now())) P.seek(o.from, false, true);
    select(id);
    st.crop = id;
    box.classList.add("ov-cropping");
    drawUi();
    renderPanel();
    return true;
  }
  function endCrop() {
    if (!st.crop) return;
    st.crop = null;
    box.classList.remove("ov-cropping");
    drawUi();
    renderPanel();
  }
  function resetCrop(id) {
    const o = item(id);
    if (!isMedia(o)) return;
    change((mm) => {
      const i = mm.overlays.findIndex((x) => x.id === id), n = medCropReset(o);
      if (kfHas(o)) n.keys = medCropKeys(o.keys, o, n);
      mm.overlays[i] = n;
    });
    redraw();
    renderPanel();
  }
  /** A copy of item `id` just above it, a little down and to the right; returns its id. */
  function duplicate(id) {
    const o = item(id);
    if (!o || !m()) return null;
    const copy = { ...ovMoved(o, 0.03, 0.03), id: ovNewId() };
    change((mm) => { mm.overlays.splice(mm.overlays.findIndex((x) => x.id === id) + 1, 0, copy); });
    select(copy.id);
    redraw();
    return copy.id;
  }
  function remove(id) {
    if (!item(id)) return false;
    if (st.sel === id) select(null);
    if (st.crop === id) endCrop();
    change((mm) => { mm.overlays = mm.overlays.filter((o) => o.id !== id); });
    redraw();
    return true;
  }
  /** Item `id` drawn last (on top) or first (under the others). */
  function restack(id, top) {
    const o = item(id);
    if (!o) return;
    change((mm) => { const rest = mm.overlays.filter((x) => x !== o); mm.overlays = top ? [...rest, o] : [o, ...rest]; });
    redraw();
  }
  layer.addEventListener("contextmenu", (e) => {
    if (!m() || st.typing || typeof medOpenMenu !== "function") return;
    e.preventDefault();
    e.stopPropagation(); // app.js would close the menu again
    const p = at(e), tm = now();
    const hit = ovHitTop(shownAt(tm), tm, st.W, st.H, p.px, p.py, tol(), ovMeasure);
    if (hit) select(hit.id);
    const it = (act, path, label, cls = "") => `<button data-med="${act}" class="${cls}"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="${path}"/></svg><span>${esc(label)}</span></button>`;
    const CROP = "M7 3v14h14M3 7h14v14";
    let html = "";
    if (hit) {
      html += `<div class="menu-head">${esc(ovItemLabel(hit))}</div>`;
      if (isMedia(hit)) html += it("crop", CROP, t("Crop"));
      html += it("front", "M8 8h12v12H8zM4 4h12v2M4 4v12h2", t("Bring to front")) + it("back", "M4 4h12v12H4zM20 8v12H8", t("Send to back"))
        + it("dup", "M8 8h12v12H8zM4 16V4h12", t("Duplicate")) + it("del", "M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13", t("Delete"), "danger") + "<hr>";
    } else if (typeof P.cv?.crop === "function") html += it("crop-clip", CROP, t("Crop")) + "<hr>";
    html += it("add", OV_ADD_PATH, t("Add a picture, GIF or clip"));
    medOpenMenu(html, e.clientX, e.clientY, (act) => {
      if (act === "crop" && hit) startCrop(hit.id);
      else if (act === "crop-clip") P.cv?.crop?.();
      else if (act === "front" && hit) restack(hit.id, true);
      else if (act === "back" && hit) restack(hit.id, false);
      else if (act === "dup" && hit) duplicate(hit.id);
      else if (act === "del" && hit) remove(hit.id);
      else if (act === "add") medPick();
    });
  });

  function hover(e) {
    if (st.crop && m()) {
      const p = at(e), own = atTime(item(st.crop), now());
      const h = own ? medCropHandles(own, st.W, st.H).find((q) => Math.abs(q.x - p.px) <= 9 && Math.abs(q.y - p.py) <= 9) : null;
      const cur = !h ? "" : h.k === "n" || h.k === "s" ? "ns-resize" : h.k === "e" || h.k === "w" ? "ew-resize" : h.k === "nw" || h.k === "se" ? "nwse-resize" : "nesw-resize";
      if (layer.style.cursor !== cur) layer.style.cursor = cur;
      return;
    }
    if (st.tool !== "select" || !m()) { layer.style.cursor = ""; return; }
    const p = at(e), tm = now(), o = atTime(item(st.sel), tm);
    const h = o && ovVisibleAt(o, tm) ? ovHandles(o, st.W, st.H, ovMeasure).find((q) => Math.abs(q.x - p.px) <= 7 && Math.abs(q.y - p.py) <= 7) : null;
    const cur = h ? (h.k === "nw" || h.k === "se" ? "nwse-resize" : h.k === "ne" || h.k === "sw" ? "nesw-resize" : "crosshair")
      : ovHitTop(shownAt(tm), tm, st.W, st.H, p.px, p.py, tol(), ovMeasure) ? "move" : "";
    if (layer.style.cursor !== cur) layer.style.cursor = cur;
  }

  /* ----- typing a text ----- */
  function startText(pt) {
    const mm = m(), tx = begin();
    const o = ovMake("text", st.style, pt, pt, mm.duration);
    // The click is where the text's first line sits, half a line up so it lands under the pointer.
    o.y = Math.max(0, pt[1] - (o.size * OV_LINE_H) / 2);
    mm.overlays.push(o);
    select(o.id);
    openEditor(o, tx, true);
  }
  function editText(id) {
    const o = item(id);
    if (!o) return;
    select(id);
    openEditor(o, begin(), false);
  }
  function openEditor(o, tx, fresh) {
    st.typing = { id: o.id, tx, fresh, old: o.text };
    editor.hidden = false;
    editor.value = o.text;
    styleEditor();
    redraw();
    const focus = () => { if (st.typing?.id === o.id && document.activeElement !== editor) { editor.focus(); editor.select(); } };
    focus(); // at once, so the first key typed lands in it
    setTimeout(focus, 0); // and again after the click that opened it has finished
  }
  function styleEditor() {
    const o = st.typing && atTime(item(st.typing.id), now());
    if (!o) return;
    const px = o.size * st.H;
    Object.assign(editor.style, {
      left: `${o.x * st.W}px`, top: `${o.y * st.H}px`, font: ovFont(px), lineHeight: `${px * OV_LINE_H}px`, color: o.color,
      width: `${Math.max(px * 2, ...ovLines({ text: editor.value || " " }).map((l) => ovMeasure(l, px))) + px}px`,
      height: `${ovLines({ text: editor.value || " " }).length * px * OV_LINE_H + 4}px`,
      textShadow: o.outline ? "0 0 3px #000, 0 0 3px #000" : "none",
    });
  }
  editor.addEventListener("input", () => {
    const o = st.typing && item(st.typing.id);
    if (!o) return;
    o.text = editor.value;
    styleEditor();
  });
  editor.addEventListener("ov-key", (e) => (e.detail === "cancel" ? cancelText() : commitText()));
  editor.addEventListener("blur", () => commitText());
  function closeEditor() {
    editor.hidden = true;
    if (document.activeElement === editor) editor.blur();
  }
  function commitText() {
    const ty = st.typing;
    if (!ty) return;
    st.typing = null;
    closeEditor();
    const mm = m(), o = item(ty.id);
    if (mm && o && !o.text.trim()) { mm.overlays = mm.overlays.filter((x) => x.id !== o.id); if (st.sel === o.id) select(null); }
    finish(ty.tx);
    if (st.tool === "text") setTool("select");
    redraw();
  }
  function cancelText() {
    const ty = st.typing;
    if (!ty) return;
    st.typing = null;
    closeEditor();
    const mm = m(), o = item(ty.id);
    if (mm && o) {
      if (ty.fresh) { mm.overlays = mm.overlays.filter((x) => x.id !== o.id); select(null); }
      else o.text = ty.old;
    }
    finish(ty.tx);
    if (st.tool === "text") setTool("select");
    redraw();
  }

  /* ----- the tool bar ----- */
  bar.addEventListener("mousedown", (e) => e.preventDefault()); // keep the focus (Space plays)
  bar.addEventListener("pointerdown", (e) => e.stopPropagation());
  bar.addEventListener("click", (e) => {
    const b = e.target.closest("[data-ov-tool], [data-ov-del], [data-ov-add]");
    if (!b) return;
    if (b.dataset.ovTool) setTool(b.dataset.ovTool);
    else if (b.hasAttribute("data-ov-add")) medPick();
    else deleteSel();
  });
  function deleteSel() {
    const id = st.sel;
    if (!id || !m()) return false;
    if (st.crop === id) endCrop();
    select(null);
    change((mm) => { mm.overlays = mm.overlays.filter((o) => o.id !== id); });
    redraw();
    return true;
  }

  /* ----- the side panel: style, time range, list, fades ----- */
  const itemLabel = ovItemLabel;
  /** The kind the style controls are for: the selection's, else the tool's. */
  const styleKind = () => item(st.sel)?.kind || (st.tool === "select" ? null : st.tool);
  /** The style values shown: the selected item's, else the tool style. */
  function shownStyle() {
    const o = item(st.sel), s = st.style;
    if (!o) return s;
    return {
      color: o.color || s.color,
      width: o.width !== undefined ? ovClamp(Math.round(o.width / OV_WIDTH_UNIT), 1, 24) : s.width,
      size: o.size !== undefined ? ovClamp(Math.round(o.size * 100), 2, 20) : s.size,
      fill: o.fill || s.fill,
      outline: o.outline !== undefined ? o.outline : s.outline,
      strength: o.strength !== undefined ? ovLevel(o.strength) : s.strength,
      round: o.kind === "blur" ? Math.round((+o.round || 0) * 100) : s.round,
      soft: o.kind === "blur" ? Math.round((+o.soft || 0) * 100) : s.soft,
    };
  }
  function renderPanel() {
    if (st.dead) return;
    const mm = m(), kind = styleKind(), s = shownStyle(), o = item(st.sel), D = dur();
    let html = `<div class="e-title">${esc(t("Text and drawing"))}</div>`;
    if (!mm) html += `<div class="small">${esc(t("Loading…"))}</div>`;
    else {
      if (!kind) html += `<div class="small">${esc(t("Pick a tool above the picture, then click or drag on the picture. Pick Select to move or change what you drew."))}</div>`;
      else if (OV_MEDIA_PATHS[kind] && o) {
        // A picture, GIF or clip (media.js): see-through, looping, crop.
        const op = Math.round(ovClamp(ovNum(o.opacity, 1), 0, 1) * 100);
        html += `<div class="e-sliders ov-style"><label class="e-slider"><span>${esc(t("Opacity"))}</span><input type="range" min="5" max="100" step="1" value="${op}" data-ov-input="opacity"><output>${op}%</output></label></div>`;
        if (kind !== "image") html += `<label class="check ov-row"><input type="checkbox" data-ov-input="loop"${o.loop ? " checked" : ""}><span>${esc(t("Loop"))}</span></label>`;
        const cropped = ["l", "t", "r", "b"].some((k) => (o.crop?.[k] || 0) > 1e-6);
        html += `<div class="ov-btns">${st.crop === o.id
          ? `<button type="button" class="btn sm on" data-ov-act="cropDone">${esc(t("Done"))}</button>`
          : `<button type="button" class="btn sm" data-ov-act="crop">${esc(t("Crop"))}</button>`}
          <button type="button" class="btn sm" data-ov-act="cropReset"${cropped ? "" : " disabled"}>${esc(t("Reset crop"))}</button></div>`;
        if (st.crop === o.id) html += `<div class="small">${esc(t("Drag an edge or a corner of the box inward to cut that part off. Click outside it or press Done when it is right."))}</div>`;
      }
      else {
        if (kind !== "blur") {
          html += `<div class="ov-row"><span>${esc(t("Color"))}</span><div class="ov-swatches">${OV_COLORS.map((col) => `<button type="button" class="ov-sw${col === s.color ? " on" : ""}" data-ov-color="${col}" style="--sw:${col}" aria-label="${esc(col)}" title="${esc(col)}"></button>`).join("")}<input type="color" class="ov-pick" data-ov-input="color" value="${esc(s.color)}" aria-label="${esc(t("Other color"))}" title="${esc(t("Other color"))}"></div></div>`;
        }
        const slider = (key, label, min, max, value, shown) => `<div class="e-sliders ov-style"><label class="e-slider"><span>${esc(label)}</span><input type="range" min="${min}" max="${max}" step="1" value="${value}" data-ov-input="${key}"><output>${shown}</output></label></div>`;
        if (kind === "text") html += slider("size", t("Size"), 2, 20, s.size, `${s.size}%`);
        else if (kind === "blur") {
          html += slider("strength", t("Strength"), 1, 10, s.strength, s.strength);
          html += slider("round", t("Round corners"), 0, 100, s.round, `${s.round}%`);
          html += slider("soft", t("Soft edge"), 0, 100, s.soft, `${s.soft}%`);
        }
        else html += slider("width", t("Thickness"), 1, 24, s.width, s.width);
        if (kind === "rect" || kind === "ellipse") {
          html += `<label class="ov-row"><span>${esc(t("Fill"))}</span><select data-ov-input="fill">${[["none", t("None")], ["solid", t("Solid")], ["half", t("See-through")]].map(([k, lab]) => `<option value="${k}"${s.fill === k ? " selected" : ""}>${esc(lab)}</option>`).join("")}</select></label>`;
        }
        if (kind === "text") html += `<label class="check ov-row"><input type="checkbox" data-ov-input="outline"${s.outline ? " checked" : ""}><span>${esc(t("Dark outline"))}</span></label>`;
        if (!o) html += `<div class="small">${esc(kind === "text" ? t("Click on the picture to type a text.") : kind === "pen" ? t("Draw on the picture with the mouse.") : t("Drag on the picture to draw it."))}</div>`;
      }
      if (o) {
        const pct = (x) => `${ovClamp((x / D) * 100, 0, 100)}%`;
        html += `<div class="ov-when">
          <div class="ov-row"><span>${esc(t("On screen"))}</span>
            <input class="ov-time" data-ov-time="from" value="${esc(ovFmtTime(o.from))}" aria-label="${esc(t("Starts at"))}" title="${esc(t("Starts at"))}">
            <span class="ov-dash">–</span>
            <input class="ov-time" data-ov-time="to" value="${esc(ovFmtTime(o.to))}" aria-label="${esc(t("Ends at"))}" title="${esc(t("Ends at"))}"></div>
          <div class="ov-rbar" aria-hidden="true"><div class="ov-rsel" style="left:${pct(o.from)};width:${pct(o.to - o.from)}"><i data-ov-end="from"></i><i data-ov-end="to"></i></div><div class="ov-rnow" style="left:${pct(now())}"></div>${kfDiamonds(o.keys, D, now())}</div>
          ${kfControls(o.keys, now(), o.smooth)}
          <div class="ov-btns">
            <button type="button" class="btn sm" data-ov-act="fromNow">${esc(t("From the playhead"))}</button>
            <button type="button" class="btn sm" data-ov-act="toNow">${esc(t("Until the playhead"))}</button>
            <button type="button" class="btn sm" data-ov-act="whole">${esc(t("Whole clip"))}</button>
          </div>
          <button type="button" class="btn sm danger" data-ov-act="delete">${esc(t("Delete the selected item"))}</button>
        </div>`;
      }
      if (list().length) {
        html += `<div class="ov-list" role="list">${list().slice().reverse().map((x) => `<button type="button" role="listitem" class="ov-item${x.id === st.sel ? " on" : ""}" data-ov-pick="${esc(x.id)}"><span class="ov-ik">${svg(ovKindPath(x.kind))}</span><span class="ov-in">${esc(itemLabel(x))}</span><span class="ov-it">${esc(ovFmtTime(x.from))}–${esc(ovFmtTime(x.to))}</span></button>`).join("")}</div>`;
      }
    }
    // Never redrawn under a field being typed in or a slider being dragged.
    const busy = st.slide || st.rdrag || document.activeElement?.matches?.(".e-ov input:not([type=range]):not([type=checkbox]):not([type=color]), .e-ov select");
    if (html !== st.lastPanel && !busy) { st.lastPanel = html; panel.innerHTML = html; }
    renderFade();
  }
  function renderFade() {
    const mm = m();
    const val = (x) => (Math.round((+x || 0) * 10) / 10).toString();
    const html = `<div class="e-title">${esc(t("Fade"))}</div>
      <div class="ov-fades">
        <label class="ov-row"><span>${esc(t("Fade in"))}</span><input type="number" class="ov-num" min="0" max="10" step="0.1" data-ov-fade="fadeIn" value="${val(mm?.fadeIn)}" ${mm ? "" : "disabled"}><span class="small">${esc(t("s"))}</span></label>
        <label class="ov-row"><span>${esc(t("Fade out"))}</span><input type="number" class="ov-num" min="0" max="10" step="0.1" data-ov-fade="fadeOut" value="${val(mm?.fadeOut)}" ${mm ? "" : "disabled"}><span class="small">${esc(t("s"))}</span></label>
      </div>
      <div class="small">${esc(t("From black at the start and to black at the end, with the sound. 0 is off."))}</div>`;
    if (html === st.lastFade || fadeSec.contains(document.activeElement)) return;
    st.lastFade = html;
    fadeSec.innerHTML = html;
  }

  /** Applies a style change to the selection (an undo step), or to the tool style. `live`: during a slider drag. */
  function setStyle(key, value, live) {
    const o = item(st.sel);
    if (!o) {
      st.style[key] = value;
      ovStyleSave(st.style);
      if (!live) renderPanel();
      return;
    }
    const apply = (x) => {
      if (key === "color" && x.kind !== "blur") x.color = value;
      else if (key === "width" && x.width !== undefined) x.width = value * OV_WIDTH_UNIT;
      else if (key === "size" && x.kind === "text") x.size = value / 100;
      else if (key === "strength" && x.kind === "blur") x.strength = ovStrength(value);
      else if (key === "fill" && x.fill !== undefined) x.fill = value;
      else if (key === "outline" && x.kind === "text") x.outline = !!value;
      else if ((key === "round" || key === "soft") && x.kind === "blur") { if (value > 0) x[key] = value / 100; else delete x[key]; }
      else if (key === "opacity" && isMedia(x)) x.opacity = ovClamp(value / 100, 0, 1);
      else if (key === "loop" && isMedia(x) && x.kind !== "image") x.loop = !!value;
    };
    // The tool style follows, so the next item looks like this one (a picture's own settings are its own).
    if (key in OV_DEFAULT_STYLE) { st.style[key] = value; ovStyleSave(st.style); }
    if (live) {
      if (!st.slide) st.slide = begin();
      apply(o);
      redraw();
      return;
    }
    if (st.slide) { const tx = st.slide; st.slide = null; apply(o); finish(tx); }
    else change(() => apply(o));
    redraw();
    renderPanel();
  }
  function setTimes(from, to, keep) {
    const o = item(st.sel);
    if (!o) return;
    // An item may run on after the end of the video (0.1.35); a clip or GIF that does not loop, not past its own end.
    let D = Math.max(dur(), o.to, (+from || 0) + OV_MIN_SPAN, +to || 0);
    if (isMedia(o) && o.kind !== "image" && !o.loop && +o.mlen > 0) D = Math.min(D, Math.max(o.from, +from || 0) + Math.max(OV_MIN_SPAN, o.mlen - (+o.off || 0)));
    const r = ovSpan(from, to, D, keep);
    if (Math.abs(r.from - o.from) < 1e-6 && Math.abs(r.to - o.to) < 1e-6) { renderPanel(); return; }
    change(() => { o.from = r.from; o.to = r.to; });
    redraw();
    renderPanel();
  }

  /** The keyframe buttons (keyframes.js): previous / next move the playhead; add, remove and Smooth are undo steps. */
  function keyAction(act, value) {
    const o = item(st.sel), tm = now();
    if (!o) return;
    if (act === "prev" || act === "next") {
      const to = act === "prev" ? kfPrev(o.keys, tm) : kfNext(o.keys, tm);
      if (to !== null) P.seek(to, false, true);
      redraw();
      renderPanel();
      return;
    }
    change(() => {
      if (act === "add") o.keys = kfPut(o.keys, tm, kfOvT(o, tm));
      else if (act === "smooth") o.smooth = !!value;
      else if (act === "remove") {
        const T = kfOvT(o, tm), keys = kfDrop(o.keys || [], tm);
        if (keys.length) o.keys = keys;
        else {
          // The last keyframe: the item stays where it is now.
          const flat = kfOvBake(o, T, st.W || 1, st.H || 1, ovMeasure);
          for (const k of Object.keys(o)) if (!(k in flat)) delete o[k];
          Object.assign(o, flat);
        }
      }
    });
    redraw();
    renderPanel();
  }
  /** What the keyframe buttons show for the selection at `tm` (the panel is redrawn when it changes). */
  const keySig = (tm) => { const o = item(st.sel); return o ? `${kfIndexAt(o.keys, tm)}|${kfPrev(o.keys, tm)}|${kfNext(o.keys, tm)}` : ""; };

  panel.addEventListener("mousedown", (e) => { if (e.target.closest("button")) e.preventDefault(); });
  panel.addEventListener("click", (e) => {
    const kb = e.target.closest("button[data-kf]");
    if (kb) { keyAction(kb.dataset.kf); return; }
    const b = e.target.closest("[data-ov-color], [data-ov-act], [data-ov-pick]");
    if (!b) return;
    const d = b.dataset, o = item(st.sel);
    if (d.ovColor) setStyle("color", d.ovColor);
    else if (d.ovPick) {
      const x = item(d.ovPick);
      if (!x) return;
      if (!ovVisibleAt(x, now())) P.seek(x.from, false, true); // to where it is on screen
      select(x.id);
      if (st.tool !== "select") setTool("select");
      redraw();
    } else if (d.ovAct === "delete") deleteSel();
    else if (o && d.ovAct === "crop") startCrop(o.id);
    else if (d.ovAct === "cropDone") endCrop();
    else if (o && d.ovAct === "cropReset") resetCrop(o.id);
    else if (o && d.ovAct === "fromNow") setTimes(now(), o.to, "from");
    else if (o && d.ovAct === "toNow") setTimes(o.from, now(), "to");
    else if (o && d.ovAct === "whole") setTimes(0, dur(), "from");
  });
  panel.addEventListener("input", (e) => {
    const k = e.target.dataset?.ovInput;
    if (!k) return;
    if (e.target.type === "range") {
      const x = +e.target.value;
      const out = e.target.parentElement.querySelector("output");
      if (out) out.textContent = ["size", "round", "soft", "opacity"].includes(k) ? `${x}%` : String(x);
      setStyle(k, x, true);
    } else if (k === "color") setStyle("color", e.target.value, true);
  });
  panel.addEventListener("change", (e) => {
    const k = e.target.dataset?.ovInput, tk = e.target.dataset?.ovTime;
    if (e.target.dataset?.kf === "smooth") { e.target.blur(); keyAction("smooth", e.target.checked); return; }
    if (k) {
      const x = e.target.type === "checkbox" ? e.target.checked : e.target.type === "range" ? +e.target.value : e.target.value;
      if (e.target.type === "range" || e.target.type === "checkbox" || e.target.type === "color") e.target.blur();
      setStyle(k, x, false);
    } else if (tk) {
      const o = item(st.sel), sec = ovParseTime(e.target.value);
      if (!o) return;
      if (sec === null) { flash(t("Type a time like 1:05.5 or 65.5")); e.target.value = ovFmtTime(o[tk]); return; }
      e.target.blur();
      if (tk === "from") setTimes(sec, o.to, "from"); else setTimes(o.from, sec, "to");
    }
  });
  panel.addEventListener("keydown", (e) => { if (e.target.dataset?.ovTime && (e.code === "Enter" || e.code === "NumpadEnter")) e.target.blur(); });
  // The range bar: drag either end, or the bar itself to move the whole range.
  panel.addEventListener("pointerdown", (e) => {
    // A keyframe's diamond: the playhead goes there.
    const dia = e.target.closest?.("[data-kf-t]");
    if (dia && e.button === 0) {
      e.preventDefault();
      P.seek(+dia.dataset.kfT, false, true);
      redraw();
      renderPanel();
      // Held and dragged (0.1.33): the keyframe goes along to another time (one undo step).
      const own = item(st.sel), bar = dia.closest(".ov-rbar"), r = bar?.getBoundingClientRect(), D = dur();
      if (!own || !r) return;
      let at = +dia.dataset.kfT, tx = null;
      const move = (ev) => {
        if (st.dead || !item(own.id)) return;
        if (!tx) { if (Math.abs(ev.clientX - e.clientX) < 3) return; tx = begin(); }
        const now2 = kfRetime(own.keys, at, ovClamp(((ev.clientX - r.left) / (r.width || 1)) * D, 0, D), D);
        if (now2 === at) return;
        at = now2;
        P.seek(at, false, true);
        st.lastPanel = "";
        redraw();
        renderPanel();
      };
      const up = () => {
        for (const ev of ["pointermove", "pointerup", "pointercancel"]) window.removeEventListener(ev, ev === "pointermove" ? move : up, true);
        if (tx) finish(tx);
      };
      window.addEventListener("pointermove", move, true);
      window.addEventListener("pointerup", up, true);
      window.addEventListener("pointercancel", up, true);
      return;
    }
    // The playhead's line on the bar: held and dragged, the playhead follows (scrubbing through the keyframes).
    if (e.target.closest?.(".ov-rnow") && e.button === 0) {
      e.preventDefault();
      const r = e.target.closest(".ov-rbar").getBoundingClientRect(), D = dur();
      const move = (ev) => { if (!st.dead) P.seek(ovClamp(((ev.clientX - r.left) / (r.width || 1)) * D, 0, D), false, false); };
      const up = () => { for (const ev of ["pointermove", "pointerup", "pointercancel"]) window.removeEventListener(ev, ev === "pointermove" ? move : up, true); };
      window.addEventListener("pointermove", move, true);
      window.addEventListener("pointerup", up, true);
      window.addEventListener("pointercancel", up, true);
      return;
    }
    const rb = e.target.closest(".ov-rbar"), o = item(st.sel);
    if (!rb || !o || e.button !== 0) return;
    e.preventDefault();
    const r = rb.getBoundingClientRect(), D = dur(), secAt = (x) => ovClamp(((x - r.left) / r.width) * D, 0, D);
    const end = e.target.dataset?.ovEnd || (e.target.closest(".ov-rsel") ? "move" : Math.abs(secAt(e.clientX) - o.from) < Math.abs(secAt(e.clientX) - o.to) ? "from" : "to");
    st.rdrag = { end, x0: secAt(e.clientX), from: o.from, to: o.to, tx: begin(), id: o.id, rb, secAt };
    rb.setPointerCapture(e.pointerId);
    rangeMove(e);
  });
  function rangeMove(e) {
    const d = st.rdrag, o = d && item(d.id);
    if (!o) return;
    const s = d.secAt(e.clientX), D = dur();
    let r;
    if (d.end === "move") { const len = d.to - d.from, a = ovClamp(d.from + (s - d.x0), 0, D - len); r = { from: a, to: a + len }; }
    else r = ovSpan(d.end === "from" ? s : o.from, d.end === "to" ? s : o.to, D, d.end);
    o.from = r.from;
    o.to = r.to;
    const sel = d.rb.querySelector(".ov-rsel");
    sel.style.left = `${(o.from / D) * 100}%`;
    sel.style.width = `${((o.to - o.from) / D) * 100}%`;
    for (const inp of panel.querySelectorAll("[data-ov-time]")) inp.value = ovFmtTime(o[inp.dataset.ovTime]);
    redraw();
  }
  panel.addEventListener("pointermove", (e) => { if (st.rdrag) rangeMove(e); });
  const rangeEnd = () => { const d = st.rdrag; if (!d) return; st.rdrag = null; finish(d.tx); redraw(); renderPanel(); };
  panel.addEventListener("pointerup", rangeEnd);
  panel.addEventListener("pointercancel", rangeEnd);

  fadeSec.addEventListener("change", (e) => {
    const k = e.target.dataset?.ovFade;
    if (!k || !m()) return;
    const x = ovClamp(Math.round(ovNum(e.target.value, 0) * 10) / 10, 0, 10);
    e.target.value = String(x);
    e.target.blur();
    if (Math.abs((m()[k] || 0) - x) < 1e-9) return;
    change((mm) => { mm[k] = x; });
    paintFade(now());
  });
  fadeSec.addEventListener("keydown", (e) => { if (e.target.dataset?.ovFade && (e.code === "Enter" || e.code === "NumpadEnter")) e.target.blur(); });

  /* ----- the handle editor.js calls ----- */
  return {
    /** Every frame while playing, and after every seek or edit (from applyColor). */
    paint(tm) {
      if (!st.editing || st.dead) return;
      const mm = m();
      const shown = mm ? list().filter((o) => ovVisibleAt(o, tm)).map((o) => o.id).join(",") : "";
      // Items with keyframes move: redrawn at every new playhead time.
      if (shown !== st.shown || st.dirty || (st.moving && tm !== st.lastT)) redraw();
      else paintFade(tm);
      const nowEl = panel.querySelector(".ov-rnow");
      if (nowEl) nowEl.style.left = `${ovClamp((tm / dur()) * 100, 0, 100)}%`;
      // The keyframe buttons follow the playhead (add / remove, previous / next).
      const sig = keySig(tm);
      if (sig !== st.keySig) { st.keySig = sig; renderPanel(); }
    },
    /** After any change of the model (P.edSync), including undo / redo. */
    sync(what) {
      if (st.dead) return;
      const mm = m();
      // A restored project's items are made safe to draw once (they came from disk).
      if (mm && what === "load") mm.overlays = (mm.overlays || []).map(ovNorm).filter(Boolean);
      if (st.sel && !item(st.sel)) select(null);
      if (st.crop && !item(st.crop)) endCrop();
      if (st.typing && !item(st.typing.id)) { st.typing = null; closeEditor(); }
      videos?.prune(new Set(list().map((o) => o.id)));
      st.dirty = true;
      redraw();
      renderPanel();
    },
    setEdit(on) {
      st.editing = !!on;
      layer.hidden = bar.hidden = !on;
      if (!on) { commitText(); endCrop(); select(null); setTool("select"); videos?.sync([], 0, false); offer.hidden = true; }
      else { place(); redraw(); renderPanel(); }
    },
    /** Delete and Esc first act on the overlays: true when they did. */
    command(cmd) {
      if (cmd === "remove" && st.sel) return deleteSel();
      if (cmd === "escape") {
        if (st.typing) { cancelText(); return true; }
        if (st.crop) { endCrop(); return true; }
        if (st.tool !== "select") { setTool("select"); return true; }
        if (st.sel) { select(null); return true; }
      }
      return false;
    },
    /** A timeline segment was picked: the overlay selection goes. */
    tlSelected() { if (st.sel) select(null); },
    /** The preview's sound level under the fade (1 = full). */
    fadeGain(tm) { const mm = m(); return mm && st.editing ? 1 - ovFadeAlpha(mm, tm) : 1; },
    /** The overlay PNGs for export_clip: [{ png, from, to }] (output seconds). */
    async exportPngs() {
      commitText();
      const mm = m();
      if (!mm) return [];
      const cs = P.cv?.size?.();
      const W = cs ? cs.width : v.videoWidth || c.width || 1920, H = cs ? cs.height : v.videoHeight || c.height || 1080;
      const out = [];
      for (const g of ovGroups(mm)) {
        if (g.media) {
          // A picture, GIF or clip: the file itself goes to ffmpeg, one entry per kept range it covers (media.js).
          const o = g.items[0];
          out.push(...medExportEntries(mm, o, kfHas(o) ? kfOvTrack(mm, o) : null));
          continue;
        }
        if (g.moving) {
          // An item with keyframes: a picture of its own box only, which ffmpeg moves and scales along its track (keyframes.js).
          const o = g.items[0], f = kfOvFrame(o, W, H, ovMeasure);
          const cv = document.createElement("canvas");
          cv.width = f.w;
          cv.height = f.h;
          const ctx = cv.getContext("2d");
          ctx.translate(-f.x, -f.y);
          ovDraw(ctx, [o], W, H);
          const png = await invoke("editor_overlay_png", { data: cv.toDataURL("image/png") });
          const r6 = (x) => Math.round(x * 1e6) / 1e6;
          out.push({ png, from: g.from, to: g.to, x: r6(f.x / W), y: r6(f.y / H), w: r6(f.w / W), h: r6(f.h / H), track: kfOvTrack(mm, o) });
          continue;
        }
        const cv = document.createElement("canvas");
        cv.width = W;
        cv.height = H;
        ovDraw(cv.getContext("2d"), g.items, W, H);
        const png = await invoke("editor_overlay_png", { data: cv.toDataURL("image/png") });
        out.push({ png, from: g.from, to: g.to });
      }
      return out;
    },
    /** For the timeline (ui/timeline.js) and the menus: pick, remove, copy, crop and add items. */
    select(id) { if (id === null || item(id)) { if (id && st.tool !== "select") setTool("select"); select(id); redraw(); } },
    remove,
    duplicate,
    crop: startCrop,
    addMedia,
    destroy() {
      st.dead = true;
      ro?.disconnect();
      videos?.destroy();
      clearTimeout(st.offerTimer);
      offer.remove();
      layer.remove();
      bar.remove();
      panel.remove();
      fadeSec.remove();
    },
    // For tests and the preview: the state and a few actions.
    _st: st, _videos: videos, setTool, deleteSel, endCrop, resetCrop, restack,
  };
}

// Lets the pure functions be tested under Node; ignored in the app.
if (typeof module !== "undefined" && module.exports) {
  module.exports = { ovItemLabel, ovKindPath, OV_KINDS, OV_WIDTH_UNIT, OV_MIN_SPAN, ovMake, ovNorm, ovVisibleAt, ovBBox, ovHit, ovHitTop, ovHandles, ovMoved, ovResized, ovGroups, ovFadeAlpha, ovFmtTime, ovParseTime, ovSpan, ovStrength, ovLevel, ovSegDist, ovTextBox };
}
