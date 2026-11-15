"use strict";
// Pictures, GIFs and clips on the editor's picture (0.1.35), the freeform crop of such an item,
// and the mask of round / soft blur boxes.
//
// A media item is an overlay item (m.overlays, ui/overlays.js) of kind "image" (a still), "gif"
// (animated, loops) or "video" (a clip's picture, without its sound):
//   { id, kind, from, to, path, title, mw, mh,  x, y, w, h,  crop (left, top, right, bottom: l / t / r / b),  off, mlen, loop, opacity, keys?, smooth? }
// x, y, w, h (fractions of the picture) are the box of what is SHOWN: the media after its crop,
// in its own proportions. crop (fractions of the media) is what is cut off. off is the second of
// the media shown at `from`, mlen its length (GIF and clip). The timeline (ui/timeline.js) moves and
// trims items itself and keeps those meanings.
//
// Preview: overlays.js draws each item into its art canvas (so it is stacked with the other items
// as in the export): a still as it is, a GIF's frame at the playhead's time (decoded once with
// ImageDecoder), a clip from a muted <video> of its own kept at off + (now - from).
// Export: each item is an input of ffmpeg (src-tauri/src/edmedia.rs), one entry per kept range it
// covers (medExportEntries), so a cut under a clip item skips the same seconds of the clip.

const MED_KINDS = ["image", "gif", "video"];
const MED_STILL_EXT = ["png", "jpg", "jpeg", "bmp", "webp", "jfif"];
const MED_VIDEO_EXT = ["mp4", "mkv", "mov", "webm", "avi", "m4v"];
const MED_STILL_LEN = 5;          // seconds a new still (or a short GIF) is on screen
const MED_GIF_FRAMES = 600;       // GIF frames kept decoded per file
const MED_MIN_CROP = 0.02;        // what a crop leaves at least, of the media's side
const medClamp = (x, lo, hi) => Math.min(hi, Math.max(lo, x));
const medNum = (x, d) => (Number.isFinite(+x) ? +x : d);
const medR = (x) => Math.round(x * 1e6) / 1e6;

/** True for a picture, GIF or clip item. */
const medIs = (o) => !!o && MED_KINDS.includes(o.kind);
/** "image", "gif", "video" or null, from a file's name. */
function medKindOfPath(p) {
  const m = /\.([a-z0-9]+)$/i.exec(String(p || "").split(/[?#]/)[0]);
  const ext = m ? m[1].toLowerCase() : "";
  if (ext === "gif") return "gif";
  if (MED_STILL_EXT.includes(ext)) return "image";
  if (MED_VIDEO_EXT.includes(ext)) return "video";
  return null;
}
/** A file's name without its folder and extension. */
const medBaseName = (p) => String(p || "").split(/[\\/]/).pop().replace(/\.[^.]+$/, "");

/* ---------- pure model helpers (tested under Node) ---------- */

/**
 * A new item from an editor_media_info result `info` ({ path, kind, title, width, height, duration }),
 * starting at timeline second `at`, on a picture of `picW` x `picH` pixels: in the middle, at most half
 * the picture's width and height, in its own proportions.
 */
function medMake(info, at, picW, picH) {
  const kind = MED_KINDS.includes(info?.kind) ? info.kind : "image";
  const mw = Math.max(1, Math.round(+info.width || 16)), mh = Math.max(1, Math.round(+info.height || 9));
  const pw = Math.max(1, +picW || 16), ph = Math.max(1, +picH || 9);
  // w * pw / (h * ph) = mw / mh
  let w = 0.5, h = (w * pw * mh) / (mw * ph);
  if (h > 0.5) { h = 0.5; w = (h * ph * mw) / (mh * pw); }
  const len = Math.max(0, +info.duration || 0);
  // A still: 5 s. A GIF: its own length, or 5 s of it looping when it is shorter than 2 s. A clip: its own length.
  const span = kind === "image" ? MED_STILL_LEN : kind === "gif" ? (len >= 2 ? len : MED_STILL_LEN) : Math.max(0.1, len);
  const from = Math.max(0, +at || 0);
  const o = {
    id: typeof ovNewId === "function" ? ovNewId() : "o" + Math.random().toString(36).slice(2, 9),
    kind, from: medR(from), to: medR(from + span), path: String(info.path || ""), title: String(info.title || medBaseName(info.path)),
    mw, mh, x: medR((1 - w) / 2), y: medR((1 - h) / 2), w: medR(w), h: medR(h), crop: { l: 0, t: 0, r: 0, b: 0 }, opacity: 1,
  };
  if (kind !== "image") Object.assign(o, { off: 0, mlen: medR(len || span), loop: kind === "gif" });
  return o;
}

/** The media fields of a saved item made safe (ovNorm calls it for the media kinds). */
function medNorm(o, out) {
  const c = o.crop && typeof o.crop === "object" ? o.crop : {};
  let l = medClamp(medNum(c.l, 0), 0, 0.98), r = medClamp(medNum(c.r, 0), 0, 0.98), tp = medClamp(medNum(c.t, 0), 0, 0.98), b = medClamp(medNum(c.b, 0), 0, 0.98);
  if (l + r > 0.98) { const k = 0.98 / (l + r); l *= k; r *= k; }
  if (tp + b > 0.98) { const k = 0.98 / (tp + b); tp *= k; b *= k; }
  Object.assign(out, {
    path: String(o.path || ""), title: String(o.title || medBaseName(o.path)),
    mw: Math.max(1, Math.round(medNum(o.mw, 16))), mh: Math.max(1, Math.round(medNum(o.mh, 9))),
    crop: { l, t: tp, r, b }, opacity: medClamp(medNum(o.opacity, 1), 0, 1),
  });
  if (o.kind !== "image") Object.assign(out, { off: Math.max(0, medNum(o.off, 0)), mlen: Math.max(0, medNum(o.mlen, 0)), loop: o.kind === "gif" ? o.loop !== false : !!o.loop });
  return out;
}

/** The second of the media shown at timeline second `tm` (held at its ends; a looping one starts over). */
function medTimeAt(o, tm) {
  const x = (+o.off || 0) + (tm - o.from), len = +o.mlen || 0;
  if (!(len > 0)) return Math.max(0, x);
  if (o.loop) return ((x % len) + len) % len;
  return medClamp(x, 0, Math.max(0, len - 0.001));
}

/** The whole media's box (before its crop), from its shown box: { x, y, w, h } fractions. */
function medFullBox(o) {
  const c = o.crop || { l: 0, t: 0, r: 0, b: 0 };
  const fw = o.w / Math.max(0.02, 1 - c.l - c.r), fh = o.h / Math.max(0.02, 1 - c.t - c.b);
  return { x: o.x - c.l * fw, y: o.y - c.t * fh, w: fw, h: fh };
}

/** The crop handles of item `o` (shown box, W x H pixels): its corners and the middles of its edges. */
function medCropHandles(o, W, H) {
  const x0 = o.x * W, y0 = o.y * H, x1 = (o.x + o.w) * W, y1 = (o.y + o.h) * H, xm = (x0 + x1) / 2, ym = (y0 + y1) / 2;
  return [{ k: "nw", x: x0, y: y0 }, { k: "n", x: xm, y: y0 }, { k: "ne", x: x1, y: y0 }, { k: "e", x: x1, y: ym },
    { k: "se", x: x1, y: y1 }, { k: "s", x: xm, y: y1 }, { k: "sw", x: x0, y: y1 }, { k: "w", x: x0, y: ym }];
}

/**
 * Item `o0` with crop handle `k` ("n", "e", "s", "w" or two of them) dragged to point (x, y)
 * (fractions of the picture): that edge moves, freely, within the whole media; what lies outside the
 * new box is cut off. The rest of the media stays exactly where it was. Returns a new item.
 */
function medCropDrag(o0, k, x, y) {
  const o = JSON.parse(JSON.stringify(o0)), F = medFullBox(o0);
  const minW = F.w * MED_MIN_CROP, minH = F.h * MED_MIN_CROP;
  let L = o0.x, T = o0.y, R = o0.x + o0.w, B = o0.y + o0.h;
  if (k.includes("w")) L = medClamp(x, F.x, R - minW);
  if (k.includes("e")) R = medClamp(x, L + minW, F.x + F.w);
  if (k.includes("n")) T = medClamp(y, F.y, B - minH);
  if (k.includes("s")) B = medClamp(y, T + minH, F.y + F.h);
  const z = (v) => (Math.abs(v) < 1e-7 ? 0 : medR(v));
  o.crop = { l: z((L - F.x) / F.w), t: z((T - F.y) / F.h), r: z((F.x + F.w - R) / F.w), b: z((F.y + F.h - B) / F.h) };
  Object.assign(o, { x: medR(L), y: medR(T), w: medR(R - L), h: medR(B - T) });
  return o;
}

/** `o` with its crop taken off: the whole media shown, in the place it has in the picture. */
function medCropReset(o0) {
  const o = JSON.parse(JSON.stringify(o0)), F = medFullBox(o0);
  Object.assign(o, { x: medR(F.x), y: medR(F.y), w: medR(F.w), h: medR(F.h), crop: { l: 0, t: 0, r: 0, b: 0 } });
  return o;
}

/**
 * Keys of an item whose own box changed from `before` to `after` (a crop): each key's move is changed
 * so that what is still shown stays where it was at that key (the keys scale about the box's middle).
 */
function medCropKeys(keys, before, after) {
  const dmx = before.x + before.w / 2 - (after.x + after.w / 2), dmy = before.y + before.h / 2 - (after.y + after.h / 2);
  return (keys || []).map((k) => ({ ...k, dx: medR(k.dx + dmx * (1 - k.sx)), dy: medR(k.dy + dmy * (1 - k.sy)) }));
}

/** Point `p` (fractions) of the shown picture in the item's own (unmoved) frame, under keyframe move `T` about middle (mx, my). */
const medUnmove = (p, T, mx, my) => [mx + (p[0] - mx - T.dx) / (T.sx || 1), my + (p[1] - my - T.dy) / (T.sy || 1)];

/** `o0` resized from corner `k` to (x, y) (fractions of a W x H picture), its proportions kept. */
function medResized(o0, k, x, y, W, H) {
  const o = JSON.parse(JSON.stringify(o0));
  const A = (o0.w * W) / Math.max(1e-9, o0.h * H);
  const ax = k.includes("w") ? o0.x + o0.w : o0.x, ay = k.includes("n") ? o0.y + o0.h : o0.y; // the corner that stays
  let w = Math.abs(x - ax), h = Math.abs(y - ay);
  if ((w * W) / Math.max(1e-9, h * H) > A) h = (w * W) / (A * H); else w = (h * H * A) / W;
  w = Math.max(w, 4 / Math.max(1, W));
  h = Math.max(h, (w * W) / (A * H));
  o.x = x < ax ? ax - w : ax;
  o.y = y < ay ? ay - h : ay;
  o.w = w;
  o.h = h;
  return o;
}

/**
 * The timeline ranges of model `m` and where each starts in the result: the kept parts of the video,
 * then the time after its end (emEnd, ui/editmodel.js). [{ start, end, at }] (at: kept seconds before it).
 */
function medRanges(m) {
  const out = [];
  let at = 0;
  for (const r of emKept(m)) { out.push({ start: r.start, end: r.end, at }); at += r.end - r.start; }
  const D = m.duration || 0, end = typeof emEnd === "function" ? emEnd(m) : D;
  if (end > D + 1e-6) out.push({ start: D, end, at });
  return out;
}

/** The OUTPUT ranges (seconds of the result) item `o` is seen in, each with the media second it starts on: [{ from, to, off }]. */
function medOutPieces(m, o) {
  const sp = m.speed || 1, out = [];
  for (const r of medRanges(m)) {
    const a = Math.max(r.start, o.from), b = Math.min(r.end, o.to);
    if (b - a < 0.001) continue;
    const from = (r.at + a - r.start) / sp, to = (r.at + b - r.start) / sp;
    const last = out[out.length - 1];
    // Two ranges that meet in the result and in the media are one (always so for a still).
    if (last && Math.abs(last.to - from) < 1e-4 && (o.kind === "image" || Math.abs(last.off + (last.to - last.from) * sp - ((+o.off || 0) + (a - o.from))) < 1e-4)) { last.to = to; continue; }
    out.push({ from, to, off: (+o.off || 0) + (a - o.from) });
  }
  return out.map((p) => ({ from: medR(p.from), to: medR(p.to), off: medR(p.off) }));
}

/** Item `o`'s entries for export_clip's `overlays`: [{ png: "", from, to, x, y, w, h, track?, media }]. */
function medExportEntries(m, o, track) {
  const c = o.crop || {};
  return medOutPieces(m, o).map((p) => ({
    png: "", from: p.from, to: p.to, x: medR(o.x), y: medR(o.y), w: medR(o.w), h: medR(o.h),
    ...(track && track.length ? { track } : {}),
    media: {
      path: o.path, kind: o.kind, off: o.kind === "image" ? 0 : p.off, mlen: +o.mlen || 0, loop: !!o.loop, speed: m.speed || 1,
      crop: { l: +c.l || 0, t: +c.t || 0, r: +c.r || 0, b: +c.b || 0 }, opacity: medClamp(medNum(o.opacity, 1), 0, 1),
    },
  }));
}

/**
 * How much of a blur box's blur shows at pixel (px, py) of a w x h box: `round` (0..1) rounds the
 * corners (1 = an ellipse), `soft` (0..1) fades the blur out toward the edge. The same formula as
 * mask_alpha in src-tauri/src/edmedia.rs, so the preview's mask and the export's agree.
 */
function medMaskAlpha(px, py, w, h, round, soft) {
  const ex = Math.min(px, w - px), ey = Math.min(py, h - py);
  let d = Math.min(ex, ey);
  const rx = (round * w) / 2, ry = (round * h) / 2;
  if (rx > 0.5 && ry > 0.5 && ex < rx && ey < ry) {
    const qx = (rx - ex) / rx, qy = (ry - ey) / ry;
    d = Math.min(d, (1 - Math.hypot(qx, qy)) * Math.min(rx, ry));
  }
  const f = (soft * Math.min(w, h)) / 2;
  if (f > 0.5) { const u = medClamp(d / f, 0, 1); return u * u * (3 - 2 * u); }
  return medClamp(d + 0.5, 0, 1);
}
const medWantsMask = (o) => (+o.round || 0) > 0.005 || (+o.soft || 0) > 0.005;

/* ---------- the preview's sources (browser only) ---------- */

const medStills = new Map(); // path -> { img, ready, failed }
const medGifs = new Map();   // path -> { frames: [{ bmp, at, dur }], total, ready, failed, img }
const medMasks = new Map();  // "aspect|round|soft" -> data URL

/** A blur box mask as a data URL (CSS mask-image, stretched over the box), cached by its shape. */
function medMaskUrl(wPx, hPx, round, soft) {
  const asp = Math.max(0.02, Math.min(50, wPx / Math.max(1, hPx)));
  const key = `${asp.toFixed(2)}|${round.toFixed(2)}|${soft.toFixed(2)}`;
  if (medMasks.has(key)) return medMasks.get(key);
  const S = 160, w = asp >= 1 ? S : Math.max(4, Math.round(S * asp)), h = asp >= 1 ? Math.max(4, Math.round(S / asp)) : S;
  const cv = document.createElement("canvas");
  cv.width = w;
  cv.height = h;
  const ctx = cv.getContext("2d"), img = ctx.createImageData(w, h);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    const i = (y * w + x) * 4;
    img.data[i] = img.data[i + 1] = img.data[i + 2] = 255;
    img.data[i + 3] = Math.round(medMaskAlpha(x + 0.5, y + 0.5, w, h, round, soft) * 255);
  }
  ctx.putImageData(img, 0, 0);
  const url = cv.toDataURL("image/png");
  if (medMasks.size > 200) medMasks.clear();
  medMasks.set(key, url);
  return url;
}

const medSrcUrl = (p) => (typeof fileSrc === "function" ? fileSrc(p) : p);

/** Starts loading what item `o` shows; `onReady` is called once it can be drawn. */
function medLoad(o, onReady) {
  if (o.kind === "image") {
    let s = medStills.get(o.path);
    if (!s) {
      s = { img: new Image(), ready: false, failed: false, waiting: [] };
      medStills.set(o.path, s);
      s.img.decoding = "async";
      s.img.onload = () => { s.ready = true; for (const f of s.waiting.splice(0)) f(); };
      s.img.onerror = () => { s.failed = true; s.waiting.length = 0; };
      s.img.src = medSrcUrl(o.path);
    }
    if (!s.ready && !s.failed && onReady && !s.waiting.includes(onReady)) s.waiting.push(onReady);
    return s;
  }
  if (o.kind === "gif") {
    let g = medGifs.get(o.path);
    if (!g) {
      g = { frames: [], total: 0, ready: false, failed: false, waiting: [], img: null };
      medGifs.set(o.path, g);
      medDecodeGif(o.path, g).then(() => { g.ready = true; for (const f of g.waiting.splice(0)) f(); })
        .catch((e) => { console.warn("Glint: the GIF could not be decoded", o.path, e); g.failed = true; for (const f of g.waiting.splice(0)) f(); });
    }
    if (!g.ready && !g.failed && onReady && !g.waiting.includes(onReady)) g.waiting.push(onReady);
    return g;
  }
  return null;
}

/** Every frame of a GIF as ImageBitmaps with their times (ImageDecoder); without it, the first frame as an <img>. */
async function medDecodeGif(path, g) {
  const url = medSrcUrl(path);
  if (typeof ImageDecoder !== "function") {
    const img = new Image();
    await new Promise((ok, no) => { img.onload = ok; img.onerror = no; img.src = url; });
    g.img = img;
    return;
  }
  const data = await (await fetch(url)).arrayBuffer();
  const dec = new ImageDecoder({ data, type: "image/gif" });
  await dec.tracks.ready;
  const n = Math.min(MED_GIF_FRAMES, dec.tracks.selectedTrack?.frameCount || 1);
  let at = 0;
  for (let i = 0; i < n; i++) {
    const { image } = await dec.decode({ frameIndex: i });
    // A GIF frame without a delay is shown for 0.1 s, as browsers and ffmpeg do.
    const dur = image.duration ? image.duration / 1e6 : 0.1;
    const bmp = await createImageBitmap(image);
    image.close();
    g.frames.push({ bmp, at, dur: dur > 0.011 ? dur : 0.1 });
    at += g.frames[g.frames.length - 1].dur;
  }
  g.total = at;
  dec.close();
}

/** The index of the frame shown at second `x` of frames [{ at, dur }]. */
function medFrameIndex(frames, x) {
  let lo = 0, hi = frames.length - 1;
  while (lo < hi) { const mid = (lo + hi + 1) >> 1; if (frames[mid].at <= x + 1e-6) lo = mid; else hi = mid - 1; }
  return lo;
}

/** A muted <video> per clip item, kept off the page and at the item's time (one player's set). */
function medVideoSet(onFrame) {
  const els = new Map(); // item id -> { el, path, want }
  const get = (o) => {
    let e = els.get(o.id);
    if (e && e.path !== o.path) { kill(e); els.delete(o.id); e = null; }
    if (!e) {
      const el = document.createElement("video");
      el.muted = true; el.defaultMuted = true; el.volume = 0; // never heard: its sound is a separate row if wanted
      el.playsInline = true; el.preload = "auto"; el.disablePictureInPicture = true;
      el.src = medSrcUrl(o.path);
      e = { el, path: o.path, want: 0 };
      for (const ev of ["seeked", "loadeddata"]) el.addEventListener(ev, () => onFrame());
      els.set(o.id, e);
    }
    return e;
  };
  const kill = (e) => { try { e.el.pause(); e.el.removeAttribute("src"); e.el.load(); } catch { /* gone */ } };
  return {
    /** Keeps each shown clip item at its time: played along while `playing` at `rate` (timeline seconds per
     *  second: the edit's speed times the player's own), else seeked exactly. */
    sync(items, tm, playing, rate = 1, soon = []) {
      const live = new Set(), r = medClamp(+rate || 1, 0.0625, 16);
      for (const o of items) {
        const e = get(o), el = e.el, want = medTimeAt(o, tm);
        live.add(o.id);
        e.want = want;
        el.muted = true;
        if (playing) {
          const drift = el.currentTime - want;
          if (el.paused) { try { el.currentTime = want + 0.04 * r; } catch { /* not ready */ } el.playbackRate = r; el.play().catch(() => {}); }
          else if (!el.seeking && Math.abs(drift) > 0.15 * Math.max(1, r)) { try { el.currentTime = want + 0.04 * r; } catch { /* not ready */ } }
          else {
            // A small drift is taken up by playing up to 25 % faster or slower for a moment (a seek would stutter).
            const nudge = Math.abs(drift) > 0.015 ? medClamp((-drift * 1.5) / r, -0.25, 0.25) : 0, want2 = r * (1 + nudge);
            if (Math.abs(el.playbackRate - want2) > 1e-3) el.playbackRate = want2;
          }
        } else {
          if (Math.abs(el.playbackRate - r) > 1e-3) el.playbackRate = r;
          if (!el.paused) el.pause();
          if (!el.seeking && Math.abs(el.currentTime - want) > 0.02 && el.readyState >= 1) { try { el.currentTime = want; } catch { /* not ready */ } }
        }
      }
      // Items about to start: their element made and waiting at its first frame, so it starts on time.
      for (const o of soon) {
        if (live.has(o.id)) continue;
        const e = get(o), el = e.el, first = medTimeAt(o, o.from);
        live.add(o.id);
        if (!el.paused) el.pause();
        if (!el.seeking && el.readyState >= 1 && Math.abs(el.currentTime - first) > 0.02) { try { el.currentTime = first; } catch { /* not ready */ } }
      }
      for (const [id, e] of els) if (!live.has(id)) { if (!e.el.paused) e.el.pause(); }
    },
    el: (o) => get(o).el,
    /** For checks: each clip item's element time, the time it should show, and whether it is muted and paused. */
    state: () => [...els].map(([id, e]) => ({ id, at: e.el.currentTime, want: e.want, muted: e.el.muted, paused: e.el.paused })),
    /** Frees elements of items that are gone. */
    prune(ids) { for (const [id, e] of els) if (!ids.has(id)) { kill(e); els.delete(id); } },
    destroy() { for (const e of els.values()) kill(e); els.clear(); },
  };
}

/**
 * Draws media item `o` as it is at timeline second `tm` into its box on a W x H canvas (crop and
 * opacity applied). `videos`: the player's medVideoSet. Something not loaded yet is a faint box.
 */
function medDraw(ctx, o, W, H, tm, videos) {
  let src = null, sw = 0, sh = 0;
  if (o.kind === "image") { const s = medStills.get(o.path); if (s?.ready) { src = s.img; sw = s.img.naturalWidth; sh = s.img.naturalHeight; } }
  else if (o.kind === "gif") {
    const g = medGifs.get(o.path);
    if (g?.ready && g.frames.length) {
      const f = g.frames[medFrameIndex(g.frames, o.loop && g.total > 0 ? ((medTimeAt(o, tm) % g.total) + g.total) % g.total : medTimeAt(o, tm))];
      src = f.bmp; sw = f.bmp.width; sh = f.bmp.height;
    } else if (g?.img) { src = g.img; sw = g.img.naturalWidth; sh = g.img.naturalHeight; }
  } else if (o.kind === "video" && videos) {
    const el = videos.el(o);
    if (el.readyState >= 2) { src = el; sw = el.videoWidth; sh = el.videoHeight; }
  }
  const x = o.x * W, y = o.y * H, w = o.w * W, h = o.h * H;
  ctx.save();
  if (!src || !sw || !sh) {
    ctx.fillStyle = "rgba(255,255,255,0.10)";
    ctx.fillRect(x, y, w, h);
    ctx.restore();
    return false;
  }
  const c = o.crop || { l: 0, t: 0, r: 0, b: 0 };
  ctx.globalAlpha = medClamp(medNum(o.opacity, 1), 0, 1);
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(src, c.l * sw, c.t * sh, Math.max(1, (1 - c.l - c.r) * sw), Math.max(1, (1 - c.t - c.b) * sh), x, y, w, h);
  ctx.restore();
  return true;
}
/** The cut-off part of item `o` while it is cropped: the whole media, faint, around its box. */
function medDrawCropped(ctx, o, W, H, tm, videos) {
  const F = medFullBox(o);
  ctx.save();
  ctx.globalAlpha = 0.35;
  medDraw(ctx, { ...o, ...F, crop: { l: 0, t: 0, r: 0, b: 0 }, opacity: 1 }, W, H, tm, videos);
  ctx.restore();
}

/* ---------- a piece of the main clip dragged up onto an overlay row (0.1.37) ---------- */

/**
 * The pure part of medFromPiece: puts into model `m` (no snapshot, no DOM) a clip item covering the
 * whole picture that shows `len` seconds of file `path` from second `off`, at timeline second `at`,
 * and the same stretch of the file as a file audio row that plays through cuts (emAddFile), at the
 * same place. `clip` = { path, title, duration, width, height }: the file it comes from (a single
 * clip's own file, or a project's working file) and its own length (the item's `mlen`, which with
 * `off` limits how far its ends can be pulled). Returns { item, row }.
 */
function medPieceInto(m, piece, clip) {
  const path = String(piece.path || clip.path || ""), off = Math.max(0, +piece.off || 0), len = Math.max(0.1, +piece.len || 0), at = Math.max(0, +piece.at || 0);
  const mlen = Math.max(off + len, +clip.duration || 0);
  const title = String(clip.title || medBaseName(path));
  const item = {
    id: typeof ovNewId === "function" ? ovNewId() : "o" + Date.now().toString(36) + Math.random().toString(36).slice(2, 7),
    kind: "video", from: medR(at), to: medR(at + len), path, title,
    mw: Math.max(1, Math.round(+clip.width || 16)), mh: Math.max(1, Math.round(+clip.height || 9)),
    x: 0, y: 0, w: 1, h: 1, crop: { l: 0, t: 0, r: 0, b: 0 }, opacity: 1,
    off: medR(off), mlen: medR(mlen), loop: false,
  };
  m.overlays = m.overlays || [];
  m.overlays.push(item);
  const row = emAddFile(m, { path, title, duration: mlen }, at);
  row.through = true;
  row.trimStart = medR(off);
  row.length = medR(len);
  return { item, row };
}

/**
 * A stretch of the main clip put on the picture (CONTRACT_0137 point 5): a clip item over the whole
 * picture plus its sound as an audio row, from `{ path?, off, len, at }` (emPieceToItem: source
 * seconds `off`, length `len`, timeline second `at`; `path` defaults to the open clip's file, which
 * for a project is its working file).
 *
 * CALLING CONVENTION with ui/timeline.js (B): ONE undo step for the whole drop. B calls this INSIDE
 * its own change, passing the model it is changing as `m`:
 *     P.edit("overlay", (m) => { const it = emPieceToItem(m, i); medFromPiece(P, it, m); emRemovePiece(m, i, true); });
 * With `m` given this function only changes `m` (no emSnapshot, no sync: P.edit does both). Without
 * `m` it makes its own undo step through P.edit. Returns the new item's id (null if nothing was made).
 */
function medFromPiece(P, piece, m) {
  const c = P?.clipMeta || {}, v = P?.v;
  const clip = { path: c.path, title: c.title, duration: +c.duration || (typeof P?.duration === "function" ? P.duration() : 0), width: c.width || v?.videoWidth, height: c.height || v?.videoHeight };
  // A project that references its clips (0.1.38, ui/editor.js pieceFile): the piece's own clip file, length, size and name.
  if (piece && piece.path && +piece.mlen > 0) Object.assign(clip, { path: piece.path, duration: +piece.mlen, title: piece.title || clip.title, width: +piece.width || clip.width, height: +piece.height || clip.height });
  if (!piece || !(+piece.len > 0) || !(piece.path || clip.path)) return null;
  let id = null;
  if (m) id = medPieceInto(m, piece, clip).item.id;
  else if (typeof P.edit === "function") P.edit("overlay", (mm) => { id = medPieceInto(mm, piece, clip).item.id; });
  if (id) setTimeout(() => { try { P.ov?.select?.(id); } catch { /* the player is gone */ } }, 0);
  return id;
}

/* ---------- adding media: the file dialog, drops, the Library list ---------- */

/** Adds each file as a picture, GIF or clip item of the open clip (edit mode first). */
async function medAddPaths(paths) {
  const P = S.player;
  if (!P || !S.open || !paths.length) return;
  try {
    const m = await (P.editing ? P.whenModel() : P.setEdit(true));
    if (!m || S.player !== P || !P.alive?.() || !P.ov?.addMedia) return;
    for (const path of paths) {
      let info;
      try { info = await invoke("editor_media_info", { path }); }
      catch (e) { flash(t("Could not add {name}: {error}", { name: medBaseName(path), error: String(e?.message ?? e) }), true); continue; }
      if (S.player !== P || P.model !== m) return;
      P.ov.addMedia(info);
    }
  } catch (e) {
    console.error("Glint: adding a picture failed", e);
    flash(String(e?.message ?? e), true);
  }
}

/** "Add a picture, GIF or clip": a file dialog, then the same way as a drop. */
async function medPick() {
  try {
    const path = await invoke("pick_media_file");
    if (path) await medAddPaths([path]);
  } catch (e) {
    console.error("Glint: pick_media_file failed", e);
    flash(String(e?.message ?? e), true);
  }
}

/**
 * Files dropped on the window while a clip is open (editor.js): pictures and GIFs become items on
 * the picture; a video does when it is dropped on the player. Returns the paths left for the audio rows.
 */
function medTakeDrop(paths, position) {
  if (!S.open || !S.player) return paths;
  const box = document.querySelector("#modal .player");
  let onPlayer = false;
  if (box && position && Number.isFinite(position.x)) {
    const k = window.devicePixelRatio || 1, r = box.getBoundingClientRect(), x = position.x / k, y = position.y / k;
    onPlayer = x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
  }
  const mine = paths.filter((p) => { const k = medKindOfPath(p); return k === "image" || k === "gif" || (k === "video" && onPlayer); });
  if (mine.length) medAddPaths(mine);
  return paths.filter((p) => !mine.includes(p));
}

/** The Library list's right-click entry in a project (projects.js): "Put on the picture" for one clip. */
function medLibMenuItem(clipId) {
  if (!clipId || !S.player?.ov?.addMedia) return "";
  return `<hr><button data-med="lib" data-arg="${esc(clipId)}"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="${MED_ICON}"/></svg><span>${esc(t("Put on the picture"))}</span></button>`;
}
const MED_ICON = "M4 5h16v14H4zM4 15l5-5 4 4 3-3 4 4M15 9.5a1.5 1.5 0 1 0 0-.01";

/** A menu of #menu (app.js's menu, placed with placeMenu) whose items carry data-med; `act(name, arg)` runs a pick. */
let medMenuAct = null;
function medOpenMenu(html, x, y, act) {
  const el = typeof $ === "function" ? $("#menu") : null;
  if (!el || !html) return;
  el.innerHTML = html;
  medMenuAct = act;
  placeMenu(x, y);
}
if (typeof document !== "undefined") {
  // Capturing: before app.js's own click handling of #menu.
  document.addEventListener("click", (e) => {
    const b = e.target.closest?.("#menu [data-med]");
    if (!b) return;
    e.preventDefault();
    e.stopPropagation();
    const a = b.dataset.med, arg = b.dataset.arg || "";
    if (typeof closeMenu === "function") closeMenu();
    if (a === "lib") {
      const c = (S.clips || []).find((k) => k.id === arg);
      if (c?.path) medAddPaths([c.path]);
      return;
    }
    const act = medMenuAct;
    medMenuAct = null;
    act?.(a, arg);
  }, true);
}

// Lets the pure functions be tested under Node; ignored in the app.
if (typeof module !== "undefined" && module.exports) {
  module.exports = { medPieceInto, medFromPiece, MED_KINDS, medIs, medKindOfPath, medBaseName, medMake, medNorm, medTimeAt, medFullBox, medCropHandles, medCropDrag, medCropReset, medCropKeys, medUnmove, medResized, medRanges, medOutPieces, medExportEntries, medMaskAlpha, medWantsMask, medFrameIndex };
}
