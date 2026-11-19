"use strict";
// Animated backdrops (0.1.46): a quiet, slow animation behind the window's pages, in the active
// theme's colors. Settings → Appearance → Backdrop (settings.backdrop: "" = none, the default).
//
// One <canvas> (2D), fixed behind #app (backdrop.css). It is created only while a backdrop is on;
// with none there is no canvas, no listener and no class (html.bd-on) at all.
// Light by design:
// - the canvas is at most CSS-pixel size (devicePixelRatio 1) and never over 1920 x 1080; the very
//   soft backdrops draw at an eighth of that and CSS scales them up;
// - at most 30 frames a second (frames are skipped in requestAnimationFrame), less for the slow ones;
// - every backdrop is a pure function of its clock: nothing builds up, nothing is allocated per frame;
// - it stops completely (no frame, no timer) while the window is hidden or not the active one (it keeps
//   its last frame), in the clip viewer / editor and fullscreen (hidden there), and shows one still frame
//   when the system asks for reduced motion.
// Text stays readable: every color is held to an opacity at which the theme's text over it keeps a
// contrast of at least 4.8 : 1 (bdCap).

const BD_KEY = "glint.backdrop"; // the choice, mirrored for the first paint (like the theme in themes.js)
const BD_MAX_W = 1920, BD_MAX_H = 1080;

/* ---------- small helpers ---------- */

/** A seeded random generator (mulberry32): the same layout every time for the same seed. */
function bdRand(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let x = Math.imul(a ^ (a >>> 15), 1 | a);
    x = (x + Math.imul(x ^ (x >>> 7), 61 | x)) ^ x;
    return ((x ^ (x >>> 14)) >>> 0) / 4294967296;
  };
}
const bdLerp = (a, b, k) => a + (b - a) * k;
const bdMod = (v, m) => ((v % m) + m) % m;
const BD_TAU = Math.PI * 2;

/** The theme's colors now, as "#rrggbb" (a CSS color of any form is read through a 2D context). */
function bdPalette(ctx) {
  const cs = getComputedStyle(document.documentElement);
  const hex = (v, fallback) => {
    const raw = cs.getPropertyValue("--" + v).trim();
    const n = typeof glintNormHex === "function" ? glintNormHex(raw) : "";
    if (n) return n;
    if (raw && ctx) {
      ctx.fillStyle = "#000000";
      ctx.fillStyle = raw;
      const f = String(ctx.fillStyle);
      if (/^#[0-9a-f]{6}$/i.test(f)) return f.toLowerCase();
      const m = f.match(/rgba?\(\s*(\d+)[,\s]+(\d+)[,\s]+(\d+)/);
      if (m) return glintRgbToHex({ r: +m[1], g: +m[2], b: +m[3] });
    }
    return fallback;
  };
  const bg = hex("bg", "#070a10"), text = hex("text", "#e8edf6"), accent = hex("accent", "#62b6f7"), accent2 = hex("accent-2", "#8ccbff");
  const dark = glintLuminance(bg) < 0.18;
  // The neighbour hue of the accent (Aurora's third band); a grey accent stays grey.
  const hsv = glintRgbToHsv(glintHexToRgb(accent));
  const hue2 = hsv.s < 0.12 ? glintMix(accent, bg, 0.3) : glintRgbToHex(glintHsvToRgb({ h: hsv.h + 42, s: Math.min(1, hsv.s * 0.9), v: Math.max(0.55, hsv.v) }));
  const light = dark ? glintMix(text, "#ffffff", 0.35) : glintMix(text, accent, 0.35); // snow, stars, light bands
  return { bg, text, accent, accent2, hue2, dark, light, snow: glintMix(light, accent, dark ? 0.12 : 0.25) };
}

/**
 * The highest opacity, at most `want`, at which `color` over the background still leaves the theme's
 * text a contrast of at least 4.8 : 1 (WCAG asks 4.5; a little room for where two shapes overlap).
 */
function bdCap(pal, color, want, under = pal.bg) {
  const ok = (a) => glintContrast(pal.text, glintMix(under, color, a)) >= 4.8;
  if (ok(want)) return want;
  let lo = 0, hi = want;
  for (let i = 0; i < 14; i++) { const mid = (lo + hi) / 2; if (ok(mid)) lo = mid; else hi = mid; }
  return lo;
}

/** A soft round dot of `color` (opaque in the middle, fading out), `size` px square; drawn scaled with globalAlpha. */
function bdDot(color, size = 64) {
  const c = document.createElement("canvas");
  c.width = c.height = size;
  const g = c.getContext("2d");
  const r = size / 2;
  const grad = g.createRadialGradient(r, r, 0, r, r, r);
  const { r: R, g: G, b: B } = glintHexToRgb(color);
  grad.addColorStop(0, `rgba(${R},${G},${B},1)`);
  grad.addColorStop(0.35, `rgba(${R},${G},${B},0.8)`);
  grad.addColorStop(0.7, `rgba(${R},${G},${B},0.25)`);
  grad.addColorStop(1, `rgba(${R},${G},${B},0)`);
  g.fillStyle = grad;
  g.fillRect(0, 0, size, size);
  return c;
}
/** A 1 x 1 context, made once, for gradients that are reused with any context. */
let bdScratchCtx = null;
function bdScratch() {
  if (!bdScratchCtx) { const c = document.createElement("canvas"); c.width = c.height = 1; bdScratchCtx = c.getContext("2d"); }
  return bdScratchCtx;
}
const bdRgba = (hex, a) => { const { r, g, b } = glintHexToRgb(hex); return `rgba(${r},${g},${b},${a})`; };

/* ---------- the five backdrops ----------
   make({ w, h, u, pal, preview }) -> { draw(ctx, sec), fast?(sec) }
   w, h: the canvas's backing size; u: backing px per CSS px; sec: the backdrop's own clock in seconds.
   `scale` draws the backdrop at that fraction of the screen (CSS scales the canvas up); `fps` its frame rate. */

function bdMakeSnow({ w, h, u, pal, preview }) {
  const rnd = bdRand(7);
  const area = (w / u) * (h / u);
  const n = preview ? 26 : Math.round(Math.min(90, Math.max(24, area / 34000)));
  const dot = bdDot(pal.snow);
  const layers = [ // far, middle, near: smaller, slower and fainter far away
    { r: [0.9, 1.5], v: [9, 14], a: 0.16, wind: 1.5, sway: [4, 9] },
    { r: [1.6, 2.4], v: [15, 22], a: 0.24, wind: 3, sway: [7, 14] },
    { r: [2.6, 3.8], v: [24, 33], a: 0.32, wind: 5, sway: [10, 20] },
  ].map((l) => ({ ...l, a: bdCap(pal, pal.snow, l.a) }));
  const flakes = [];
  for (let i = 0; i < n; i++) {
    const l = layers[i % 7 < 3 ? 0 : i % 7 < 6 ? 1 : 2]; // more far flakes than near ones
    const k = preview ? 0.8 : 1;
    flakes.push({
      x0: rnd() * w, y0: rnd() * h,
      r: bdLerp(l.r[0], l.r[1], rnd()) * u * 2.2 * k, // the sprite's soft edge: the visible core is about half of it
      v: bdLerp(l.v[0], l.v[1], rnd()) * u * k,
      wind: l.wind * u * k * (0.6 + rnd() * 0.8),
      sway: bdLerp(l.sway[0], l.sway[1], rnd()) * u * k,
      om: BD_TAU / bdLerp(6, 13, rnd()), ph: rnd() * BD_TAU,
      a: l.a * (0.75 + rnd() * 0.25),
    });
  }
  return {
    draw(ctx, sec) {
      ctx.clearRect(0, 0, w, h);
      for (const f of flakes) {
        const span = h + 2 * f.r;
        const y = bdMod(f.y0 + f.v * sec, span) - f.r;
        const x = bdMod(f.x0 + f.wind * sec + Math.sin(sec * f.om + f.ph) * f.sway, w + 2 * f.r) - f.r;
        ctx.globalAlpha = f.a;
        ctx.drawImage(dot, x - f.r, y - f.r, 2 * f.r, 2 * f.r);
      }
      ctx.globalAlpha = 1;
    },
  };
}

function bdMakeAurora({ w, h, pal }) {
  const colors = [pal.accent, pal.hue2, pal.accent2];
  // Overlapping bands add up: each gets a share of what the brightest color may reach.
  const cap = Math.min(...colors.map((c) => bdCap(pal, c, 0.3)));
  const mk = (c) => {
    const { r, g, b } = glintHexToRgb(c);
    // Local coordinates: a unit circle, stretched by the transform into a wide band.
    const grad = bdScratch().createRadialGradient(0, 0, 0, 0, 0, 1);
    // A brighter core and a quicker falloff than a plain fade, so the bands read as shapes, not haze.
    grad.addColorStop(0, `rgba(${r},${g},${b},1)`);
    grad.addColorStop(0.25, `rgba(${r},${g},${b},0.85)`);
    grad.addColorStop(0.55, `rgba(${r},${g},${b},0.32)`);
    grad.addColorStop(0.8, `rgba(${r},${g},${b},0.08)`);
    grad.addColorStop(1, `rgba(${r},${g},${b},0)`);
    return grad;
  };
  const bands = [
    { c: mk(colors[0]), x: 0.25, y: 0.18, rx: 0.62, ry: 0.2, rot: -0.16, a: 0.9, p: [70, 95, 41], ph: 0.0 },
    { c: mk(colors[1]), x: 0.72, y: 0.3, rx: 0.55, ry: 0.17, rot: -0.1, a: 0.75, p: [83, 110, 53], ph: 2.1 },
    { c: mk(colors[2]), x: 0.5, y: 0.08, rx: 0.48, ry: 0.13, rot: -0.2, a: 0.55, p: [97, 77, 47], ph: 4.2 },
  ];
  return {
    draw(ctx, sec) {
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, w, h);
      for (const b of bands) {
        const cx = (b.x + 0.09 * Math.sin((sec * BD_TAU) / b.p[0] + b.ph)) * w;
        const cy = (b.y + 0.05 * Math.sin((sec * BD_TAU) / b.p[1] + b.ph * 1.7)) * h;
        const rot = b.rot + 0.05 * Math.sin((sec * BD_TAU) / b.p[2] + b.ph);
        const breathe = 0.62 + 0.38 * Math.sin((sec * BD_TAU) / b.p[2] + b.ph * 0.7);
        const cos = Math.cos(rot), sin = Math.sin(rot), rx = b.rx * w, ry = b.ry * h * (0.9 + 0.1 * breathe);
        ctx.setTransform(cos * rx, sin * rx, -sin * ry, cos * ry, cx, cy);
        ctx.globalAlpha = cap * b.a * breathe;
        ctx.fillStyle = b.c;
        ctx.fillRect(-1, -1, 2, 2);
      }
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.globalAlpha = 1;
    },
  };
}

function bdMakeStars({ w, h, u, pal, preview }) {
  const rnd = bdRand(11);
  const area = (w / u) * (h / u);
  const n = preview ? 80 : Math.round(Math.min(320, Math.max(80, area / 9000)));
  const top = bdCap(pal, pal.light, 0.42);
  const stars = [];
  for (let i = 0; i < n; i++) {
    const tw = rnd() < 0.12;
    stars.push({ x: Math.round(rnd() * w), y: Math.round(rnd() * h), s: Math.max(1, Math.round((rnd() < 0.7 ? 1 : 2) * u * (preview ? 1.4 : 1))), a: top * bdLerp(0.4, 0.92, rnd() ** 1.3) * (tw ? 0.78 : 1) /* a twinkling star also gets a glow over it */, tw, om: BD_TAU / bdLerp(5, 11, rnd()), ph: rnd() * BD_TAU });
  }
  const glow = bdDot(pal.light, 32);
  // Now and then, rarely, a faint shooting star: one chance in every 45 s window, at a fixed place in it.
  const WIN = 45, LIFE = 1.3;
  const plans = new Map(); // window number -> its shooting star (or null); only the last two are kept
  const plan = (k) => {
    if (plans.has(k)) return plans.get(k);
    const r = bdRand(1000 + k);
    let m = null;
    if (r() <= 0.75) {
      const at = k * WIN + 6 + r() * (WIN - 6 - LIFE);
      const dir = r() < 0.5 ? -1 : 1;
      const ang = 0.5 + r() * 0.25;
      m = { at, x0: (0.15 + r() * 0.7) * w, y0: (0.05 + r() * 0.35) * h, dx: Math.cos(ang) * dir, dy: Math.sin(ang), dist: 420 * u, len: 150 * u, p: 0 };
    }
    plans.set(k, m);
    if (plans.size > 2) plans.delete(plans.keys().next().value);
    return m;
  };
  const meteor = (sec) => {
    const m = plan(Math.floor(sec / WIN));
    if (!m) return null;
    const p = (sec - m.at) / LIFE;
    if (p < 0 || p > 1) return null;
    m.p = p;
    return m;
  };
  return {
    fast: (sec) => !!meteor(sec) || !!meteor(sec + 0.2),
    draw(ctx, sec) {
      ctx.clearRect(0, 0, w, h);
      ctx.fillStyle = pal.light;
      for (const st of stars) {
        let a = st.a;
        if (st.tw) a *= 0.55 + 0.45 * Math.sin(sec * st.om + st.ph);
        ctx.globalAlpha = a;
        ctx.fillRect(st.x, st.y, st.s, st.s);
        if (st.tw && a > top * 0.45) { ctx.globalAlpha = a * 0.35; ctx.drawImage(glow, st.x - 4 * u, st.y - 4 * u, 8 * u + st.s, 8 * u + st.s); }
      }
      const m = preview ? null : meteor(sec);
      if (m) {
        const hx = m.x0 + m.dx * m.dist * m.p, hy = m.y0 + m.dy * m.dist * m.p;
        const tx = hx - m.dx * m.len, ty = hy - m.dy * m.len;
        const g = ctx.createLinearGradient(tx, ty, hx, hy); // only while one is crossing, about once a minute
        g.addColorStop(0, bdRgba(pal.light, 0));
        g.addColorStop(1, bdRgba(pal.light, 1));
        ctx.globalAlpha = top * 0.8 * Math.sin(Math.PI * m.p);
        ctx.strokeStyle = g;
        ctx.lineWidth = 1.2 * u;
        ctx.lineCap = "round";
        ctx.beginPath();
        ctx.moveTo(tx, ty);
        ctx.lineTo(hx, hy);
        ctx.stroke();
      }
      ctx.globalAlpha = 1;
    },
  };
}

function bdMakeFireflies({ w, h, u, pal, preview }) {
  const rnd = bdRand(23);
  const area = (w / u) * (h / u);
  const n = preview ? 7 : Math.round(Math.min(13, Math.max(6, area / 180000)));
  const core = glintMix(pal.accent, pal.dark ? "#ffffff" : pal.accent, 0.45);
  const glow = bdDot(pal.accent), dot = bdDot(core, 16);
  // Two glows may cross, and the core is drawn over its glow: the glow is held so that two of them stay
  // under the cap, and the core to what is left over them.
  const aGlow = bdCap(pal, pal.accent, 0.4) / 2;
  const aCore = bdCap(pal, core, 0.6,glintMix(pal.bg, pal.accent, Math.min(1, aGlow * 2.2)));
  const flies = [];
  for (let i = 0; i < n; i++) {
    flies.push({
      fx: 0.06 + rnd() * 0.88, fy: 0.08 + rnd() * 0.84,
      ax: 0.05 + rnd() * 0.08, ay: 0.04 + rnd() * 0.07, bx: 0.02 + rnd() * 0.03, by: 0.02 + rnd() * 0.03,
      w1: BD_TAU / bdLerp(45, 80, rnd()), w2: BD_TAU / bdLerp(23, 37, rnd()), w3: BD_TAU / bdLerp(36, 62, rnd()), w4: BD_TAU / bdLerp(19, 31, rnd()),
      p: [rnd() * BD_TAU, rnd() * BD_TAU, rnd() * BD_TAU, rnd() * BD_TAU],
      lw: BD_TAU / bdLerp(7, 13, rnd()), lp: rnd() * BD_TAU,
      r: bdLerp(16, 24, rnd()) * u * (preview ? 0.7 : 1),
    });
  }
  return {
    draw(ctx, sec) {
      ctx.clearRect(0, 0, w, h);
      for (const f of flies) {
        const x = (f.fx + f.ax * Math.sin(sec * f.w1 + f.p[0]) + f.bx * Math.sin(sec * f.w2 + f.p[1])) * w;
        const y = (f.fy + f.ay * Math.sin(sec * f.w3 + f.p[2]) + f.by * Math.sin(sec * f.w4 + f.p[3])) * h;
        const s = 0.5 - 0.5 * Math.cos(sec * f.lw + f.lp); // fades in and out, slowly
        const lit = s * s * (3 - 2 * s);
        if (lit < 0.02) continue;
        ctx.globalAlpha = aGlow * lit;
        ctx.drawImage(glow, x - f.r, y - f.r, 2 * f.r, 2 * f.r);
        const cr = 3.6 * u * (preview ? 0.8 : 1);
        ctx.globalAlpha = aCore * lit;
        ctx.drawImage(dot, x - cr, y - cr, 2 * cr, 2 * cr);
      }
      ctx.globalAlpha = 1;
    },
  };
}

function bdMakeDrift({ w, h, pal }) {
  const rnd = bdRand(31);
  const color = glintMix(pal.light, pal.accent, 0.3);
  const top = bdCap(pal, color, 0.11);
  const { r, g, b } = glintHexToRgb(color);
  // Local coordinates: a band 1 wide (across) and 1 long; the transform places, tilts and sizes it.
  const grad = bdScratch().createLinearGradient(0, 0, 1, 0);
  grad.addColorStop(0, `rgba(${r},${g},${b},0)`);
  grad.addColorStop(0.5, `rgba(${r},${g},${b},1)`);
  grad.addColorStop(1, `rgba(${r},${g},${b},0)`);
  const diag = Math.hypot(w, h);
  const ang = -0.52; // about 30 degrees off vertical, like light through blinds
  const bands = [];
  for (let i = 0; i < 6; i++) bands.push({ o: i / 6 + rnd() * 0.06, bw: bdLerp(0.05, 0.12, rnd()), a: bdLerp(0.5, 1, rnd()), om: BD_TAU / bdLerp(40, 70, rnd()), ph: rnd() * BD_TAU });
  const SPEED = 1 / 300; // the whole set moves one band spacing in 50 s: almost imperceptible
  return {
    draw(ctx, sec) {
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, w, h);
      ctx.fillStyle = grad;
      const cos = Math.cos(ang), sin = Math.sin(ang);
      for (const bd of bands) {
        const off = (bdMod(bd.o + sec * SPEED, 1) - 0.5) * diag * 1.6; // across the bands, centred on the screen
        const bw = bd.bw * diag;
        // Across = (cos, sin) * bw, along = (-sin, cos) * 2 * diag, starting at the band's left edge.
        const ox = w / 2 + cos * (off - bw / 2) + sin * diag, oy = h / 2 + sin * (off - bw / 2) - cos * diag;
        ctx.setTransform(cos * bw, sin * bw, -sin * 2 * diag, cos * 2 * diag, ox, oy);
        ctx.globalAlpha = top * bd.a * (0.7 + 0.3 * Math.sin(sec * bd.om + bd.ph));
        ctx.fillRect(0, 0, 1, 1);
      }
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.globalAlpha = 1;
    },
  };
}

window.GLINT_BACKDROPS = [
  { id: "snowfall", get name() { return t("Snowfall"); }, tier: "builtin", fps: 30, scale: 1, make: bdMakeSnow },
  { id: "aurora", get name() { return t("Aurora"); }, tier: "builtin", fps: 12, scale: 0.33, make: bdMakeAurora },
  { id: "stars", get name() { return t("Starfield"); }, tier: "builtin", fps: 12, scale: 1, make: bdMakeStars },
  { id: "fireflies", get name() { return t("Fireflies"); }, tier: "builtin", fps: 30, scale: 1, make: bdMakeFireflies },
  { id: "drift", get name() { return t("Drift"); }, tier: "builtin", fps: 12, scale: 0.125, make: bdMakeDrift },
];

/** Whether a backdrop may be used. */
function backdropAllowed(b) {
  return !!b && b.tier === "builtin";
}

/** The registry entry for an id, or null (none, unknown or not allowed). */
function bdFind(id) {
  const b = window.GLINT_BACKDROPS.find((x) => x.id === id);
  return b && backdropAllowed(b) ? b : null;
}

/* ---------- the layer behind the app ---------- */

const BD = {
  id: "",           // the backdrop on screen ("" = none)
  canvas: null, ctx: null, inst: null, entry: null,
  w: 0, h: 0, pal: null,
  raf: 0, last: 0, clock: 12, // the clock starts a little in, so the first frame is already a full scene
  focused: true, mq: null, mo: null,
  stats: { ticks: 0, frames: 0, ms: 0 }, // for measuring (bdStats)
};

/** The id the settings ask for: the loaded settings', else the one remembered from the last run. */
function bdWanted() {
  try { if (typeof S !== "undefined" && S && S.settings) return String(S.settings.backdrop || ""); } catch {}
  try { return localStorage.getItem(BD_KEY) || ""; } catch { return ""; }
}

/** The clip viewer, the editor workspace or a fullscreen view is open: no backdrop there. */
function bdCovered() {
  if (document.fullscreenElement) return true;
  const m = document.getElementById("modal");
  return !!(m && !m.hidden && m.querySelector(":scope > .viewer"));
}
const bdReduced = () => !!(BD.mq && BD.mq.matches);
const bdShouldRun = () => !!BD.inst && !document.hidden && BD.focused && !bdCovered() && !bdReduced();

/** Size the canvas to the window (capped) and make the backdrop for it. */
function bdBuild() {
  if (!BD.entry || !BD.canvas) return;
  const cw = Math.max(1, window.innerWidth), ch = Math.max(1, window.innerHeight);
  const k = Math.min(1, BD_MAX_W / cw, BD_MAX_H / ch) * BD.entry.scale;
  const w = Math.max(1, Math.round(cw * k)), h = Math.max(1, Math.round(ch * k));
  if (BD.canvas.width !== w || BD.canvas.height !== h) { BD.canvas.width = w; BD.canvas.height = h; }
  BD.w = w; BD.h = h;
  BD.pal = bdPalette(BD.ctx);
  BD.inst = BD.entry.make({ w, h, u: k, pal: BD.pal, preview: false });
  BD.inst.draw(BD.ctx, BD.clock);
}

function bdTick(now) {
  BD.raf = 0;
  if (!bdShouldRun()) return;
  BD.stats.ticks++;
  const fps = BD.inst.fast && BD.inst.fast(BD.clock) ? 30 : BD.entry.fps;
  const gap = now - BD.last;
  if (gap >= 1000 / fps - 2) {
    BD.clock += Math.min(gap, 100) / 1000; // after a pause the scene goes on where it stopped
    BD.last = now;
    const a = performance.now();
    BD.inst.draw(BD.ctx, BD.clock);
    BD.stats.ms += performance.now() - a;
    BD.stats.frames++;
  }
  BD.raf = requestAnimationFrame(bdTick);
}

/** Start or stop the frames (and show or hide the layer) after anything changed. */
function bdUpdate() {
  if (!BD.canvas) return;
  const covered = bdCovered();
  BD.canvas.hidden = covered;
  if (bdShouldRun()) {
    if (!BD.raf) { BD.last = performance.now(); BD.raf = requestAnimationFrame(bdTick); }
  } else if (BD.raf) {
    cancelAnimationFrame(BD.raf);
    BD.raf = 0;
  }
}

const bdOnFocus = () => { BD.focused = true; bdUpdate(); };
const bdOnBlur = () => { BD.focused = false; bdUpdate(); };
const bdOnResize = () => { bdBuild(); bdUpdate(); };
const bdOnMotion = () => { if (BD.inst) BD.inst.draw(BD.ctx, BD.clock); bdUpdate(); };
const BD_EVENTS = [[document, "visibilitychange", bdUpdate], [document, "fullscreenchange", bdUpdate], [window, "focus", bdOnFocus], [window, "blur", bdOnBlur], [window, "resize", bdOnResize]];

function bdStart(entry) {
  BD.entry = entry;
  BD.id = entry.id;
  if (!BD.canvas) {
    const c = document.createElement("canvas");
    c.id = "bd-layer";
    c.setAttribute("aria-hidden", "true");
    document.body.insertBefore(c, document.body.firstChild);
    BD.canvas = c;
    BD.ctx = c.getContext("2d");
    BD.focused = document.hasFocus();
    for (const [el, ev, fn] of BD_EVENTS) el.addEventListener(ev, fn);
    BD.mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    BD.mq.addEventListener("change", bdOnMotion);
    // The viewer and the editor open in #modal: watch only its own hidden flag and children.
    const m = document.getElementById("modal");
    if (m) { BD.mo = new MutationObserver(bdUpdate); BD.mo.observe(m, { attributes: true, attributeFilter: ["hidden"], childList: true }); }
  }
  document.documentElement.classList.add("bd-on");
  document.documentElement.dataset.backdrop = entry.id;
  bdBuild();
  bdUpdate();
}

/** Back to none: the canvas, the listeners and the class go; the page is exactly as without backdrops. */
function bdStop() {
  if (BD.raf) cancelAnimationFrame(BD.raf);
  BD.raf = 0;
  for (const [el, ev, fn] of BD_EVENTS) el.removeEventListener(ev, fn);
  if (BD.mq) BD.mq.removeEventListener("change", bdOnMotion);
  if (BD.mo) BD.mo.disconnect();
  if (BD.canvas) { BD.canvas.width = BD.canvas.height = 0; BD.canvas.remove(); }
  Object.assign(BD, { id: "", canvas: null, ctx: null, inst: null, entry: null, pal: null, mq: null, mo: null });
  document.documentElement.classList.remove("bd-on");
  delete document.documentElement.dataset.backdrop;
}

/** Show what the settings ask for (call it after the settings or the theme changed). */
function bdSync() {
  if (!document.body) return;
  const entry = bdFind(bdWanted());
  try { if (typeof S !== "undefined" && S && S.settings) localStorage.setItem(BD_KEY, entry ? entry.id : ""); } catch {}
  if (!entry) { if (BD.id || BD.canvas) bdStop(); return; }
  if (entry.id !== BD.id || !BD.canvas) bdStart(entry);
}

/** The theme's colors changed: draw in the new ones (a still frame too). */
function bdRepalette() {
  if (BD.inst) { bdBuild(); bdUpdate(); }
  bdPaintTiles();
}

/** Measuring: { ticks, frames, ms } since the start, and what runs now. */
function bdStats() {
  return { ...BD.stats, id: BD.id, running: !!BD.raf, w: BD.w, h: BD.h, backingBytes: BD.canvas ? BD.canvas.width * BD.canvas.height * 4 : 0 };
}

/* ---------- the picker (Settings → Appearance) ---------- */

const BD_TILE_W = 128, BD_TILE_H = 72;
let bdTileRun = null; // { el, ctx, inst, entry, raf, last, clock }: the one tile animating (hovered or focused)
let bdPickerOn = false;

/** The picker: None first, then a small live preview of each backdrop (app.js renderSettings puts it under the themes). */
function bdPickerHtml() {
  bdPickerInit();
  const cur = bdFind(bdWanted());
  const tile = (id, name, inner) => `<button type="button" class="bd-tile${(cur ? cur.id : "") === id ? " active" : ""}" data-bd="${id}" aria-pressed="${(cur ? cur.id : "") === id}">${inner}<span>${esc(name)}</span></button>`;
  const tiles = [tile("", t("None"), `<i class="bd-none"></i>`)]
    .concat(window.GLINT_BACKDROPS.filter(backdropAllowed).map((b) => tile(b.id, b.name, `<canvas width="${BD_TILE_W}" height="${BD_TILE_H}"></canvas>`)));
  queueMicrotask(bdPaintTiles); // once the page's HTML is in place: a still frame in each tile
  return `<div class="bd-pick"><div class="txt"><div class="name">${t("Backdrop")}</div><div class="hint">${t("A quiet animation behind the pages, in the theme's colors. It rests while Glint is in the background, and in the editor and fullscreen")}</div></div><div class="bd-tiles">${tiles.join("")}</div></div>`;
}

/** A tile's backdrop, made for its small canvas. */
function bdTileInst(btn) {
  const entry = bdFind(btn.dataset.bd);
  const c = btn.querySelector("canvas");
  if (!entry || !c) return null;
  const ctx = c.getContext("2d");
  return { entry, ctx, inst: entry.make({ w: c.width, h: c.height, u: c.width / 360, pal: bdPalette(ctx), preview: true }) };
}

function bdPaintTiles() {
  for (const btn of document.querySelectorAll("#view .bd-tile[data-bd]")) {
    if (!btn.dataset.bd) continue;
    const ti = bdTileInst(btn);
    if (ti) ti.inst.draw(ti.ctx, 24);
  }
}

function bdTileStop() {
  if (!bdTileRun) return;
  cancelAnimationFrame(bdTileRun.raf);
  bdTileRun = null;
}
function bdTileStart(btn) {
  if (bdTileRun && bdTileRun.el === btn) return;
  bdTileStop();
  const ti = bdTileInst(btn);
  if (!ti) return;
  const run = (bdTileRun = { el: btn, ...ti, raf: 0, last: performance.now(), clock: 24 });
  const step = (now) => {
    if (bdTileRun !== run) return;
    if (!btn.isConnected) return bdTileStop();
    if (now - run.last >= 1000 / 30 - 2) {
      // A preview runs a little faster than the real thing, so a short hover shows what it does.
      run.clock += (Math.min(now - run.last, 100) / 1000) * (run.entry.scale < 1 ? 6 : 2);
      run.last = now;
      run.inst.draw(run.ctx, run.clock);
    }
    run.raf = requestAnimationFrame(step);
  };
  if (!window.matchMedia("(prefers-reduced-motion: reduce)").matches) run.raf = requestAnimationFrame(step);
}

/** The picker's own listeners, added the first time it is shown. */
function bdPickerInit() {
  if (bdPickerOn) return;
  bdPickerOn = true;
  const tileOf = (e) => e.target.closest?.("#view .bd-tile[data-bd]");
  document.addEventListener("click", (e) => {
    const btn = tileOf(e);
    if (!btn || typeof S === "undefined" || !S.settings) return;
    const id = btn.dataset.bd;
    if ((S.settings.backdrop || "") === id) return;
    S.settings.backdrop = id;
    bdSync();
    if (typeof saveSettings === "function") saveSettings(true);
  });
  document.addEventListener("pointerover", (e) => { const btn = tileOf(e); if (btn && btn.dataset.bd) bdTileStart(btn); });
  document.addEventListener("pointerout", (e) => { const btn = tileOf(e); if (btn && bdTileRun && bdTileRun.el === btn && !btn.contains(e.relatedTarget) && document.activeElement !== btn) { bdTileStop(); bdPaintTiles(); } });
  document.addEventListener("focusin", (e) => { const btn = tileOf(e); if (btn && btn.dataset.bd) bdTileStart(btn); });
  document.addEventListener("focusout", (e) => { const btn = tileOf(e); if (btn && bdTileRun && bdTileRun.el === btn && !btn.matches(":hover")) { bdTileStop(); bdPaintTiles(); } });
}

/* ---------- hooks: the settings arrive and the theme changes through themes.js ---------- */

(function bdHook() {
  const apply = window.applyGlintTheme;
  if (typeof apply === "function" && !apply.bdWrapped) {
    const wrapped = function (...args) { const r = apply.apply(this, args); try { bdSync(); bdRepalette(); } catch {} return r; };
    wrapped.bdWrapped = true;
    window.applyGlintTheme = wrapped;
  }
  const preview = window.glintPreviewColors;
  if (typeof preview === "function" && !preview.bdWrapped) {
    const wrapped = function (...args) { const r = preview.apply(this, args); try { bdRepalette(); } catch {} return r; };
    wrapped.bdWrapped = true;
    window.glintPreviewColors = wrapped;
  }
  if (document.body) bdSync();
  else document.addEventListener("DOMContentLoaded", bdSync, { once: true });
})();
