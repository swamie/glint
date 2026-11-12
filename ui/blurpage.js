"use strict";
// The Blur settings page, with a live preview of what Blur does to motion.
// Loaded before app.js: nothing here may touch app.js globals (S, $, $$, row,
// toggle, select, number, esc, ...) at load time, only inside functions.
//
// Layout of this file:
//   1. Blur's maths, ported from blur v2.23 lib/blur.py and lib/blur/weighting.py
//      (pure functions, testable in Node: see the module.exports line at the end).
//   2. Page data: weighting names, presets.
//   3. renderBlur(): builds the page.
//   4. The preview: live settings, status line, canvas animation loop.

/* ---------- 1. Blur's maths ---------- */

// Node tests load this file without i18n.js: fall back to a plain t() that only fills the placeholders.
if (typeof t === "undefined") globalThis.t = (text, vars) => (vars ? text.replace(/\{(\w+)\}/g, (whole, key) => (key in vars ? String(vars[key]) : whole)) : text);

const BW_NAMES = ["equal", "gaussian_sym", "vegas", "pyramid", "gaussian", "ascending", "descending", "gaussian_reverse"];

// weighting.py normalize(): shift negatives so the smallest becomes 1, then make them sum to 1.
function bwNormalize(ws) {
  const min = Math.min(...ws);
  if (min < 0) ws = ws.map((w) => w - min + 1);
  const total = ws.reduce((a, w) => a + w, 0);
  if (!(total !== 0 && Number.isFinite(total))) return null; // Python: ZeroDivisionError
  return ws.map((w) => w / total);
}
// weighting.py scale_range(): n evenly spaced numbers from start to end.
function bwScaleRange(n, start, end) {
  if (n <= 1) return Array(Math.max(n, 0)).fill(start);
  return Array.from({ length: n }, (_, i) => start + (i * (end - start)) / (n - 1));
}
function bwGaussian(n, mean, sd, bound) {
  if (sd === 0) return "sd0"; // Python: division by zero
  return bwNormalize(bwScaleRange(n, bound[0], bound[1]).map((x) => Math.exp(-((x - mean) ** 2) / (2 * sd ** 2))));
}

/**
 * The weights Blur uses for `n` blended frames, oldest frame first.
 * `g` = { sd, mean, bound } for the gaussian kinds (bound already parsed).
 * Returns { w: number[] } or { error: "..." }.
 */
function blurWeights(name, n, g) {
  let w;
  switch (name) {
    case "equal": w = Array(n).fill(1 / n); break;
    case "ascending": w = bwNormalize(Array.from({ length: n }, (_, i) => i + 1)); break;
    case "descending": w = bwNormalize(Array.from({ length: n }, (_, i) => n - i)); break;
    case "pyramid": { const h = (n - 1) / 2; w = bwNormalize(Array.from({ length: n }, (_, i) => h - Math.abs(i - h) + 1)); break; }
    case "gaussian": w = bwGaussian(n, g.mean, g.sd, g.bound); break;
    case "gaussian_reverse": w = bwGaussian(n, g.mean, g.sd, g.bound); if (Array.isArray(w)) w.reverse(); break;
    case "gaussian_sym": { const m = Math.max(...g.bound.slice(0, 2).map(Math.abs)); w = bwGaussian(n, 0, g.sd, [-m, m]); break; }
    case "vegas": w = bwNormalize(n % 2 === 0 ? [1, ...Array(n - 2).fill(2), 1] : Array(n).fill(1)); break;
    default: {
      // Custom: Python int() on each comma-separated part, then divide() stretches them over n frames.
      const parts = String(name).split(",");
      if (!parts.every((p) => /^\s*[+-]?\d+\s*$/.test(p))) return { error: t('custom weights "{name}" must be whole numbers separated by commas, such as 1, 2, 1', { name }) };
      const cw = parts.map((p) => parseInt(p, 10));
      w = bwNormalize(bwScaleRange(n, 0, cw.length - 0.1).map((x) => cw[Math.trunc(x)]));
    }
  }
  if (w === "sd0") return { error: t("a Gaussian std dev of 0 makes Blur divide by zero") };
  return w ? { w } : { error: t("these weights add up to nothing (Blur divides by zero)") };
}

/** blur.py parse_fps_setting(): "5x" multiplies the current rate, "600" is a rate. Returns { fps } or { error }. */
function blurParseFps(value, fps, label) {
  const v = String(value ?? "").trim();
  if (v.endsWith("x")) {
    const m = v.slice(0, -1).trim();
    const k = m === "" ? NaN : Number(m);
    if (!Number.isFinite(k)) return { error: t('{label} "{v}" should be a multiplier like 5x', { label, v }) };
    return { fps: fps * k };
  }
  if (!/^[+-]?\d+$/.test(v)) return { error: t('{label} "{v}" should be a whole number like 600, or a multiplier like 5x', { label, v }) };
  return { fps: parseInt(v, 10) };
}

const bpClamp = (v, lo, hi, d) => (Number.isFinite(+v) ? Math.min(hi, Math.max(lo, +v)) : d);

/**
 * What Blur will do with these settings, following blur.py step by step.
 * c = S.settings.blur (or the live copy), src = capture frame rate.
 * kind: "error" | "off" | "zero" (amount 0) | "slow" (frame_gap < 1) | "tiny" (gap * amount < 1) | "one" (1 frame) | "blend".
 */
function blurModel(c, src) {
  const m = { kind: "blend", src, fps: src, out: Math.round(bpClamp(c.output_fps, 1, 1000, 60)), amount: bpClamp(c.amount, 0, 10, 1), gap: 0, n: 0, w: null, interp: "", error: "" };
  let fps = src;
  // input timescale
  if (c.timescale) { const ti = bpClamp(c.input_timescale, 0.01, 100, 1); if (ti !== 1) fps *= 1 / ti; }
  // interpolation: both rates are parsed before anything runs; SVP and RIFE get int(rate)
  if (c.interpolate) {
    const it = blurParseFps(c.interpolated_fps || "5x", fps, t("interpolated rate"));
    if (it.error) return { ...m, kind: "error", error: it.error };
    if (c.interpolation_method !== "rife" && c.pre_interpolate) {
      const pre = blurParseFps(c.pre_interpolated_fps || "360", fps, t("pre-interpolated rate"));
      if (pre.error) return { ...m, kind: "error", error: pre.error };
      if (fps < pre.fps) { fps = Math.trunc(pre.fps); m.interp = "pre"; }
    }
    if (fps < it.fps) { fps = Math.trunc(it.fps); m.interp = "ran"; } else if (m.interp !== "pre") m.interp = "skipped";
  }
  // output timescale
  if (c.timescale) { const to = bpClamp(c.output_timescale, 0.01, 100, 1); if (to !== 1) fps *= to; }
  m.fps = fps;
  // blurring
  if (!c.blur) return { ...m, kind: "off" };
  if (!(m.amount > 0)) return { ...m, kind: "zero" };
  m.gap = Math.floor(fps / m.out + 1e-9); // int(video.fps / blur_output_fps)
  let n = Math.trunc(m.gap * m.amount); // int(frame_gap * blur_amount)
  if (n <= 0) return { ...m, kind: m.gap < 1 ? "slow" : "tiny" };
  if (n % 2 === 0) n += 1; // the number of weights must be odd
  m.n = n;
  // Glint writes the gaussian settings only with Advanced on; otherwise Blur uses its defaults.
  const adv = !!c.advanced;
  let bound = [0, 2];
  if (adv) {
    // blur.py parses the bound with json.loads for every weighting, so a bad bound always fails.
    try { bound = JSON.parse(String(c.gaussian_bound || "[0,2]")); } catch { bound = null; }
    if (!Array.isArray(bound) || bound.length < 2 || !bound.slice(0, 2).every((x) => typeof x === "number")) return { ...m, kind: "error", error: t('Gaussian bound "{bound}" should look like [0,2]', { bound: c.gaussian_bound }) };
  }
  const g = { sd: adv ? +c.gaussian_std_dev : 1, mean: adv ? +c.gaussian_mean : 2, bound };
  const r = blurWeights(String(c.weighting || "equal").trim(), n, g);
  if (r.error) return { ...m, kind: "error", error: r.error };
  m.w = r.w;
  if (n === 1) m.kind = "one";
  return m;
}

/* ---------- 2. Page data ---------- */

// [value, label, what the viewer sees]
const BLUR_WEIGHTS = [
  // The labels and descriptions are translated when the page is built, not here.
  ["equal", "Equal", "An even streak: every blended frame counts the same."],
  ["gaussian_sym", "Gaussian, symmetric", "The streak fades out softly at both ends."],
  ["pyramid", "Pyramid", "The streak is strongest in the middle and fades evenly to both ends."],
  ["gaussian", "Gaussian", "The streak is brightest at the front, where the object is heading, and fades softly behind it."],
  ["ascending", "Ascending", "The streak brightens steadily toward the front, where the object is heading."],
  ["descending", "Descending", "The streak is brightest at the back, where the object came from, and fades toward the front."],
  ["gaussian_reverse", "Gaussian, reversed", "The streak is brightest at the back and fades softly toward the front."],
  ["vegas", "Vegas", "Looks the same as Equal, because Blur always blends an odd number of frames."],
];

const BP_PRESETS = [
  { id: "light", label: "Light", tip: "A short smear that keeps things crisp", set: { blur: true, amount: 0.5, output_fps: 60, weighting: "equal", gamma: 1, interpolate: true, interpolated_fps: "5x" } },
  { id: "medium", label: "Medium", tip: "Blur's default: smooth, natural motion", set: { blur: true, amount: 1, output_fps: 60, weighting: "equal", gamma: 1, interpolate: true, interpolated_fps: "5x" } },
  { id: "heavy", label: "Heavy", tip: "Long, soft trails", set: { blur: true, amount: 2, output_fps: 60, weighting: "gaussian_sym", gamma: 1, interpolate: true, interpolated_fps: "5x" } },
  { id: "cine", label: "Cinematic 30 fps", tip: "Film-like: 30 fps with a half-frame smear", set: { blur: true, amount: 0.5, output_fps: 30, weighting: "equal", gamma: 1, interpolate: true, interpolated_fps: "5x" } },
];

/* ---------- 3. The page ---------- */

function renderBlur() {
  const b = S.settings.blur;
  const src = S.settings.fps || 60;
  const text = (path, width = 110, ph = "", extra = "") => `<input class="field" type="text" data-set="${path}" value="${esc(getPath(S.settings, path))}" placeholder="${esc(ph)}" spellcheck="false" style="width:${width}px;min-width:0"${extra}>`;
  // A slider and a number box bound to the same setting; the input listener keeps them in step.
  const slider = (path, label, min, max, step, nmin, nmax) => {
    const v = getPath(S.settings, path);
    return `<div class="bp-slider"><input class="range" type="range" data-set="${path}" data-num min="${min}" max="${max}" step="${step}" value="${v}" aria-label="${label}"><input class="field bp-num" type="number" data-set="${path}" data-num min="${nmin}" max="${nmax}" step="${step}" value="${v}" aria-label="${label}"></div>`;
  };
  const group = (id, title, sum, badge, rows) => `<details class="section bp-group" data-b-group="${id}"${BP.open.has(id) ? " open" : ""}>
      <summary><h2>${title}</h2><span class="bp-sum">${sum}</span>${badge ? `<span class="bp-badge">${badge}</span>` : ""}</summary>${rows}</details>`;
  const onOff = (v) => (v ? t("On") : t("Off"));

  const known = BLUR_WEIGHTS.find(([v]) => v === b.weighting);
  const fpsOpts = [...new Set([24, 30, 48, 50, 60, 72, 90, 120, 144, 240, b.output_fps])].sort((x, y) => x - y).map((f) => [f, `${f} fps`]);
  const presetOn = (p) => Object.entries(p.set).every(([k, v]) => String(b[k]) === String(v));
  const gaussHint = b.weighting && b.weighting.startsWith("gaussian") && !b.advanced ? " " + t("Its curve can be tuned under Advanced.") : "";

  $("#view").innerHTML = `<div class="bp-page">` + pageHead(t("Blur"), t("Frame-blended motion blur for finished clips")) + `
    <div class="note${S.blur ? "" : " bad"}">${S.blur ? t("Using <b>{path}</b>. Open a clip and press <b>Render with Blur</b>; the result is added to the library.", { path: esc(S.blur) }) : t("<b>blur-cli.exe was not found.</b> Install Blur (f0e/blur) or set its path below.")}</div>

    <div class="bp-preview">
      <div class="bp-panes">
        <figure class="bp-pane"><figcaption>${t("Original")} <span data-b-cap="src"></span></figcaption><canvas data-b-cv="src" aria-label="${esc(t("Moving shapes as recorded"))}"></canvas></figure>
        <figure class="bp-pane"><figcaption>${t("With these settings")} <span data-b-cap="out"></span></figcaption><canvas data-b-cv="out" aria-label="${esc(t("The same shapes after Blur"))}"></canvas></figure>
      </div>
      <div class="bp-foot">
        <div class="bp-status" data-b-status aria-live="polite"></div>
        <div class="bp-weights" data-b-weights title="${esc(t("How much each blended frame counts, oldest on the left, newest on the right"))}"></div>
        <button class="btn sm bp-slow" data-b-slow aria-pressed="${BP.slow}">${t("Slow motion")}</button>
      </div>
    </div>

    <div class="bp-presets" role="group" aria-label="${esc(t("Quick presets"))}"><span class="bp-presets-label">${t("Quick presets")}</span>
      ${BP_PRESETS.map((p) => `<button class="bp-preset${presetOn(p) ? " active" : ""}" data-b-preset="${p.id}" title="${esc(t(p.tip))}">${t(p.label)}</button>`).join("")}
    </div>

    <div class="section"><h2>${t("Basics")}</h2>
      ${row(t("Motion blur"), t("Smears moving things into streaks, like a camera with a slow shutter; off keeps every frame sharp."), toggle("blur.blur"))}
      ${row(t("Blur amount"), t("Length of the streaks: 0.5 is a short smear, 1 joins frames into smooth motion, 2 or more leaves long ghostly trails."), slider("blur.amount", t("Blur amount"), 0, 3, 0.05, 0, 10))}
      ${row(t("Output frame rate"), t("Frame rate of the finished video; at a lower rate each frame covers more motion, so streaks get longer."), select("blur.output_fps", fpsOpts, true))}
      ${row(t("Weighting"), (known ? t(known[2]) : t("Your own weights, from the back of the streak to the front.")) + gaussHint, select("blur.weighting", [...BLUR_WEIGHTS.map(([v, l]) => [v, t(l)]), [known ? "1, 2, 1" : b.weighting, t("Custom weights")]]))}
      ${known ? "" : row(t("Custom weights"), t("Whole numbers separated by commas, stretched along the streak from back to front: 1, 2, 3, 2, 1 fades both ends."), text("blur.weighting", 180, "1, 2, 1"))}
      ${row(t("Interpolate"), t("Invents in-between frames before blending, so streaks are smooth instead of a few separate copies."), toggle("blur.interpolate"))}
      ${row(t("Interpolated frame rate"), t("Frames per second to invent, as a multiplier (5x turns your {src} fps clips into {out} fps) or a number; low values leave visible copies, higher is smoother but renders slower.", { src, out: src * 5 }), text("blur.interpolated_fps", 96, "5x", ` list="bp-ifps"`) + `<datalist id="bp-ifps">${["3x", "5x", "10x", "600", "1200"].map((v) => `<option value="${v}">`).join("")}</datalist>`)}
    </div>

    ${group("look", t("Trail brightness and colour"), t("Gamma and colour filters"), b.filters ? t("Filters on") : "", `
      ${row(t("Gamma"), t("Brightness of the streaks: 1 is a plain mix, higher makes faint trails and highlights glow, lower makes them darker and thinner."), slider("blur.gamma", t("Gamma"), 0.2, 4, 0.1, 0.1, 10))}
      ${row(t("Filters"), t("Applies the brightness, saturation and contrast below to the finished video."), toggle("blur.filters"))}
      ${row(t("Brightness"), t("1 leaves the picture as it is; above 1 lifts everything, dark areas included, below 1 darkens it."), slider("blur.brightness", t("Brightness"), 0, 2, 0.05, 0, 10))}
      ${row(t("Saturation"), t("1 leaves colours as they are; 0 is black and white, above 1 is more vivid."), slider("blur.saturation", t("Saturation"), 0, 2, 0.05, 0, 10))}
      ${row(t("Contrast"), t("1 leaves the picture as it is; above 1 makes bright parts brighter while black stays black."), slider("blur.contrast", t("Contrast"), 0, 2, 0.05, 0, 10))}`)}

    ${group("interp", t("Interpolation quality"), t("How the in-between frames are made"), b.interpolate ? String(b.interpolation_method || "svp").toUpperCase() : t("Interpolation off"), `
      ${row(t("Method"), t("SVP is fast and suits games; RIFE is an AI model that handles tricky motion better but renders much slower."), select("blur.interpolation_method", [["svp", "SVP"], ["rife", "RIFE"]]))}
      ${row(t("Pre-interpolate"), t("Lets RIFE make the first in-between frames for cleaner shapes, then SVP the rest; slower, and only with the SVP method."), toggle("blur.pre_interpolate"))}
      ${row(t("Pre-interpolated frame rate"), t("How far RIFE goes before SVP takes over, as a multiplier like 2x or a number like 360."), text("blur.pre_interpolated_fps", 96))}`)}

    ${group("dedup", t("Stutter fix"), t("Repeated frames"), onOff(b.deduplicate), `
      ${row(t("Deduplicate"), t("Fills in repeated frames from stutters or drops, so the blur keeps flowing instead of stopping and jumping."), toggle("blur.deduplicate"))}
      ${row(t("Method"), t("SVP is fast, RIFE is cleaner but slower, Old is Blur's earlier method."), select("blur.deduplicate_method", [["svp", "SVP"], ["rife", "RIFE"], ["old", t("Old")]]))}`)}

    ${group("speed", t("Speed"), t("Slow motion and speed-ups"), onOff(b.timescale), `
      ${row(t("Timescale"), t("Speeds the video up or slows it down using the two values below."), toggle("blur.timescale"))}
      ${row(t("Input timescale"), t("Speed the footage was recorded at: 1 is normal, 0.5 means it was already slowed to half speed."), number("blur.input_timescale", 0.01, 100, 0.05))}
      ${row(t("Output timescale"), t("Speed of the result: 0.5 plays in half-speed slow motion, 2 plays twice as fast."), number("blur.output_timescale", 0.01, 100, 0.05))}
      ${row(t("Adjust audio pitch"), t("Sound gets lower when slowed and higher when sped up, like a tape; off keeps voices at their normal pitch."), toggle("blur.adjust_audio_pitch"))}`)}

    ${group("output", t("Output file"), t("Encoder, quality and file dates"), "", `
      ${row(t("Encode preset"), t("H.264 plays everywhere; H.265 and AV1 make smaller files; VP9 needs GPU encoding off."), select("blur.encode_preset", [...new Set(["h264", "h265", "av1", "vp9", b.encode_preset])].map((p) => [p, { h264: "H.264", h265: "H.265", av1: "AV1", vp9: "VP9" }[p] || p])))}
      ${row(t("Quality"), t("Lower numbers look sharper and make bigger files: 16 (Blur's default) looks near perfect, 23 is smaller and slightly softer."), number("blur.quality", 0, 51))}
      ${row(t("Copy dates"), t("The rendered file gets the same created and modified dates as the original clip."), toggle("blur.copy_dates"))}`)}

    ${group("gpu", t("Graphics card"), t("GPU acceleration"), "", `
      ${row(t("GPU decoding"), t("Reads the clip on the graphics card, which is usually faster."), toggle("blur.gpu_decoding"))}
      ${row(t("GPU interpolation"), t("Invents the in-between frames on the graphics card, which is much faster."), toggle("blur.gpu_interpolation"))}
      ${row(t("GPU encoding"), t("Writes the video on the graphics card: faster, but files are larger at the same quality number."), toggle("blur.gpu_encoding"))}
      ${row(t("GPU type"), t("The brand of graphics card to use."), select("blur.gpu_type", [["nvidia", "NVIDIA"], ["amd", "AMD"], ["intel", "Intel"]]))}
      ${row(t("RIFE GPU number"), t("Which graphics card RIFE runs on; -1 lets Blur test them and pick the fastest."), number("blur.rife_gpu", -1, 16))}`)}

    ${group("adv", t("Advanced"), t("Gaussian curve, interpolation tuning, file type"), onOff(b.advanced), `
      ${row(t("Advanced"), t("Uses the settings below; off means Blur's own defaults."), toggle("blur.advanced"))}
      ${!b.advanced ? "" : `
      ${row(t("Gaussian std dev"), t("Width of the Gaussian curve: low gives a short bright core to the streak, high makes it close to Equal."), number("blur.gaussian_std_dev", 0, 100, 0.1))}
      ${row(t("Gaussian mean"), t("Where along the bound the curve peaks: at the bound's end (2 by default) the front of the streak is brightest."), number("blur.gaussian_mean", -100, 100, 0.1))}
      ${row(t("Gaussian bound"), t("The range the curve is drawn over, written like [0,2]."), text("blur.gaussian_bound", 96))}
      ${row(t("Deduplicate range"), t("How many repeated frames in a row get filled in; -1 means no limit."), number("blur.deduplicate_range", -1, 1000))}
      ${row(t("Deduplicate threshold"), t("How alike two frames must be to count as a repeat; higher also catches nearly still frames."), number("blur.deduplicate_threshold", 0, 1, 0.001))}
      ${row(t("Video container"), t("File type of the render; Glint's player handles mp4 best."), text("blur.container", 96))}
      ${row(t("Custom ffmpeg filters"), t("Your own ffmpeg output options, used instead of the encode preset."), text("blur.ffmpeg_filters", 300))}
      ${row(t("Debug"), t("Blur's debug mode, which marks the frames deduplication changed."), toggle("blur.debug"))}
      ${row(t("SVP interpolation preset"), t("SVP's starting point for how it invents frames."), select("blur.svp_preset", [...new Set(["weak", "film", "smooth", "animation", "default", "test", b.svp_preset])].map((p) => [p, p])))}
      ${row(t("SVP interpolation algorithm"), t("1, 2, 11, 13, 21 or 23; different trade-offs between smoothness and warping."), number("blur.svp_algorithm", 1, 23))}
      ${row(t("Interpolation block size"), t("4, 8, 16 or 32; smaller follows fine motion better but renders slower."), number("blur.block_size", 4, 32))}
      ${row(t("Interpolation mask area"), t("0 to 100; higher keeps still parts of the picture from warping."), number("blur.mask_area", 0, 100))}
      ${row(t("RIFE model"), t("Folder name under Blur's models folder."), text("blur.rife_model", 240))}`}`)}

    <div class="section"><h2>${t("Program")}</h2>
      ${row(t("blur-cli.exe path"), t("Leave empty to find it automatically"), `<input class="field" type="text" data-set="blur_path" value="${esc(S.settings.blur_path)}" placeholder="${esc(t("Auto"))}" style="width:340px">`)}
    </div>
  </div>`;
  bpBind();
  bpStart();
}

/* ---------- 4. The preview ---------- */

// Module state. Survives re-renders; the canvases themselves are rebuilt each time.
const BP = {
  open: new Set(), // collapsible groups the user opened
  slow: false, // slow motion (quarter speed)
  live: null, // copy of S.settings.blur, updated on every `input` event
  src: 60, // capture frame rate
  model: null, // blurModel(live, src)
  ver: 0, // bumps whenever the model changes, forces a redraw
  raf: 0,
  last: 0, // previous animation timestamp
  vt: 0, // video time in seconds (keeps running across re-renders)
  view: null, // the #view element the listeners are on
  panes: null, // { src, out } canvas state
  col: null, // theme colours
  colAt: 0,
  lastKey: "",
  lut: null, // gamma tables
};

const bpFmt = (n) => (Number.isInteger(n) ? String(n) : String(+n.toFixed(2)));

function bpBind() {
  const view = $("#view");
  if (BP.view === view) return;
  BP.view = view;
  view.addEventListener("input", bpOnInput);
  view.addEventListener("click", bpOnClick);
  view.addEventListener("toggle", bpOnToggle, true); // toggle does not bubble
  if (!BP.visBound) { BP.visBound = true; document.addEventListener("visibilitychange", () => { if (!document.hidden) bpKick(); }); }
}

/** Read a control's current value into the live settings, without saving (app.js saves on `change`). */
function bpOnInput(e) {
  const el = e.target;
  const path = el.dataset && el.dataset.set;
  if (!path || !path.startsWith("blur.") || !BP.live || S.page !== "blur") return;
  let v;
  if (el.type === "checkbox") v = el.checked;
  else if ("num" in el.dataset) { if (el.value === "") return; v = +el.value; if (!Number.isFinite(v)) return; }
  else v = el.value;
  BP.live[path.slice(5)] = v;
  // Keep a slider and its number box showing the same value.
  for (const o of $$(`[data-set="${path}"]`, BP.view)) if (o !== el && o.tagName === "INPUT" && o.type !== "checkbox") o.value = el.value;
  bpRecalc();
}

function bpOnClick(e) {
  if (S.page !== "blur") return;
  const p = e.target.closest("[data-b-preset]");
  if (p) {
    const pr = BP_PRESETS.find((x) => x.id === p.dataset.bPreset);
    if (pr) { Object.assign(S.settings.blur, pr.set); saveSettings(true); }
    return;
  }
  const s = e.target.closest("[data-b-slow]");
  if (s) { BP.slow = !BP.slow; s.setAttribute("aria-pressed", String(BP.slow)); BP.lastKey = ""; }
}

function bpOnToggle(e) {
  const id = e.target.dataset && e.target.dataset.bGroup;
  if (!id) return;
  if (e.target.open) BP.open.add(id); else BP.open.delete(id);
}

/** Called at the end of renderBlur(): pick up the new canvases and (re)start the loop. */
function bpStart() {
  if (BP.raf) cancelAnimationFrame(BP.raf);
  BP.raf = 0;
  BP.live = { ...S.settings.blur };
  BP.src = S.settings.fps || 60;
  const mk = (el) => ({ el, ctx: el.getContext("2d", { alpha: false }), pw: 0, ph: 0, w: 0, h: 0 });
  BP.panes = { src: mk($('[data-b-cv="src"]')), out: mk($('[data-b-cv="out"]')) };
  // Stick to the very top of #view: sticky offsets count from inside #view's padding.
  const pv = $(".bp-preview");
  pv.style.top = `${-(parseFloat(getComputedStyle(BP.view).paddingTop) || 0)}px`;
  bpColors();
  bpRecalc();
  bpKick();
}

function bpKick() {
  if (BP.raf || document.hidden || S.page !== "blur" || !BP.panes || !BP.panes.out.el.isConnected) return;
  BP.last = 0;
  BP.raf = requestAnimationFrame(bpTick);
}

function bpRecalc() {
  BP.model = blurModel(BP.live, BP.src);
  BP.ver++;
  BP.lastKey = "";
  bpStatus();
}

/** The line of plain numbers under the preview, the weights strip and the captions. */
function bpStatus() {
  const m = BP.model, c = BP.live;
  const el = $("[data-b-status]");
  if (!el) return;
  const fps = bpFmt(m.fps), out = m.out;
  let s, warn = true;
  switch (m.kind) {
    case "error": s = t("Blur would stop with an error: {error}.", { error: esc(m.error) }); break;
    case "off": warn = false; s = m.fps !== m.src ? t("Blur is off, so no frames are blended; the video comes out at {fps} fps.", { fps }) : t("Blur is off, so no frames are blended."); break;
    case "zero": s = t("Blur amount is 0, so nothing is blended; the video is only converted to {out} fps.", { out }); break;
    case "slow": s = t("Nothing is blended: the video is only {fps} fps, not above the {out} fps output.", { fps, out }) + " " + (c.interpolate ? t("Raise the interpolated rate above {out} fps.", { out }) : t("Turn on Interpolate or lower the output frame rate.")); break;
    case "tiny": s = m.gap === 1 ? t("Nothing is blended: an amount of {amount} covers less than one of the {gap} frame between output frames. Raise the blur amount.", { amount: bpFmt(m.amount), gap: m.gap }) : t("Nothing is blended: an amount of {amount} covers less than one of the {gap} frames between output frames. Raise the blur amount.", { amount: bpFmt(m.amount), gap: m.gap }); break;
    case "one": s = t("Each output frame uses just 1 frame of the {fps} fps video, so there is no blur.", { fps }) + " " + (c.interpolate ? t("Raise the blur amount or the interpolated rate.") : t("Raise the blur amount, or turn on Interpolate.")); break;
    default: {
      warn = false;
      const n = m.n, gap = m.gap;
      s = n <= gap ? t("Each output frame blends <b>{n}</b> frames (out of {gap} available) from {fps} fps video, shown at {out} fps.", { n, gap, fps, out }) : t("Each output frame blends <b>{n}</b> frames ({gap} available, so neighbouring streaks overlap) from {fps} fps video, shown at {out} fps.", { n, gap, fps, out });
      if (m.interp === "skipped") s += " " + t("Interpolation is skipped because the video is already {fps} fps, so the streak is {n} separate copies.", { fps, n });
      else if (m.interp === "") s += " " + t("Without interpolation the streak is {n} separate copies.", { n });
    }
  }
  el.innerHTML = s;
  el.classList.toggle("warn", warn);

  // Weights strip, oldest to newest, at most 31 bars.
  const wEl = $("[data-b-weights]");
  if (wEl) {
    let bars = [];
    if (m.w && m.n > 1) {
      const nb = Math.min(31, m.w.length);
      for (let i = 0; i < nb; i++) {
        const a = Math.floor((i * m.w.length) / nb), z = Math.floor(((i + 1) * m.w.length) / nb);
        let sum = 0; for (let j = a; j < z; j++) sum += m.w[j];
        bars.push(sum / (z - a));
      }
      const max = Math.max(...bars) || 1;
      bars = bars.map((v) => `<i style="height:${Math.max(4, (v / max) * 100).toFixed(1)}%"></i>`);
    }
    wEl.innerHTML = bars.join("");
    wEl.hidden = !bars.length;
  }
  const capS = $('[data-b-cap="src"]'), capO = $('[data-b-cap="out"]');
  if (capS) capS.textContent = `${BP.src} fps`;
  if (capO) capO.textContent = m.kind === "error" ? t("(not rendered)") : m.kind === "off" ? `${fps} fps` : `${out} fps`;

  // Highlight the preset that matches what is on screen right now.
  for (const b of $$("[data-b-preset]")) {
    const p = BP_PRESETS.find((x) => x.id === b.dataset.bPreset);
    b.classList.toggle("active", !!p && Object.entries(p.set).every(([k, v]) => String(c[k]) === String(v)));
  }
}

function bpColors() {
  const cs = getComputedStyle(document.documentElement);
  const v = (name, d) => cs.getPropertyValue(name).trim() || d;
  const col = { bg: v("--bg", "#070a10"), line: v("--line-2", "#263043"), accent: v("--accent", "#62b6f7"), text: v("--text", "#e8edf6") };
  BP.colAt = performance.now();
  if (BP.col && Object.keys(col).every((k) => col[k] === BP.col[k])) return;
  BP.col = col;
  if (BP.panes) { BP.panes.src.pw = 0; BP.panes.out.pw = 0; } // repaint the static layer
  BP.lastKey = "";
}

/** Match the canvas backing store to its CSS size and the screen's pixel ratio. */
function bpSize(P) {
  const dpr = window.devicePixelRatio || 1;
  const w = P.el.clientWidth, h = P.el.clientHeight;
  const pw = Math.max(1, Math.round(w * dpr)), ph = Math.max(1, Math.round(h * dpr));
  if (pw === P.pw && ph === P.ph) return false;
  Object.assign(P, { w, h, pw, ph, dpr });
  P.el.width = pw; P.el.height = ph;
  // Static layer (things that do not move, so they stay sharp).
  P.bg = document.createElement("canvas"); P.bg.width = pw; P.bg.height = ph;
  const g = P.bg.getContext("2d", { willReadFrequently: true });
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.fillStyle = BP.col.bg; g.fillRect(0, 0, w, h);
  const L = bpLayout(w, h);
  g.strokeStyle = BP.col.line; g.lineWidth = 1;
  g.beginPath(); g.moveTo(L.pad, Math.round(L.by) + 0.5); g.lineTo(w - L.pad, Math.round(L.by) + 0.5); g.stroke();
  g.beginPath(); g.arc(L.sx, L.sy, L.arm + 6, 0, Math.PI * 2); g.stroke();
  for (let x = L.pad; x <= w - L.pad + 0.5; x += (w - 2 * L.pad) / 8) { g.beginPath(); g.moveTo(Math.round(x) + 0.5, h - 10); g.lineTo(Math.round(x) + 0.5, h - 4); g.stroke(); }
  P.bgData = g.getImageData(0, 0, pw, ph).data;
  // Accumulation layer for the blended frames.
  P.acc = document.createElement("canvas"); P.acc.width = pw; P.acc.height = ph;
  P.accCtx = P.acc.getContext("2d", { willReadFrequently: true });
  P.img = P.ctx.createImageData(pw, ph);
  return true;
}

function bpLayout(w, h) {
  const r = Math.max(6, h * 0.075);
  return { r, by: h * 0.3, pad: r + 8, sx: w * 0.5, sy: h * 0.7, arm: h * 0.24 };
}

// The scene: a ball sweeping back and forth (0.45 s per crossing) and a bar spinning 2.5 turns a second.
const BP_CROSS = 0.45, BP_SPIN = 2.5;
function bpDrawScene(ctx, w, h, tm) {
  const L = bpLayout(w, h);
  const p = (((tm / (2 * BP_CROSS)) % 1) + 1) % 1;
  const x = L.pad + (p < 0.5 ? p * 2 : 2 - p * 2) * (w - 2 * L.pad);
  ctx.fillStyle = BP.col.accent;
  ctx.beginPath(); ctx.arc(x, L.by, L.r, 0, Math.PI * 2); ctx.fill();
  const a = tm * BP_SPIN * Math.PI * 2;
  // A bar with a dot at its tip. They must not overlap: overlapping shapes would add up twice when blended.
  const cos = Math.cos(a), sin = Math.sin(a), lw = Math.max(3, h * 0.04), dr = Math.max(3, h * 0.035);
  const end = L.arm - dr - lw / 2 - 1;
  ctx.strokeStyle = BP.col.text; ctx.lineWidth = lw; ctx.lineCap = "round";
  ctx.beginPath(); ctx.moveTo(L.sx - cos * L.arm * 0.35, L.sy - sin * L.arm * 0.35); ctx.lineTo(L.sx + cos * end, L.sy + sin * end); ctx.stroke();
  ctx.fillStyle = BP.col.accent;
  ctx.beginPath(); ctx.arc(L.sx + cos * L.arm, L.sy + sin * L.arm, dr, 0, Math.PI * 2); ctx.fill();
}

function bpTick(now) {
  BP.raf = 0;
  const P = BP.panes;
  // Stop on our own when the page was left, rebuilt elsewhere, or the window is hidden.
  if (document.hidden || S.page !== "blur" || !P || !P.out.el.isConnected) return;
  const dt = BP.last ? Math.min(now - BP.last, 100) : 0;
  BP.last = now;
  BP.vt += (dt / 1000) * (BP.slow ? 0.25 : 1);
  if (now - BP.colAt > 1500) bpColors(); // follow theme changes
  const resized = bpSize(P.src) | bpSize(P.out);
  if (resized) BP.lastKey = "";
  bpFrame();
  BP.raf = requestAnimationFrame(bpTick);
}

/** Draw both panes, only when the frame shown actually changes. */
function bpFrame() {
  const m = BP.model, c = BP.live, vt = BP.vt, P = BP.panes;
  const kSrc = Math.floor(vt * BP.src + 1e-6);
  // Which video frames make up the output frame on screen now: [{ t, w }].
  let samples, kOut, blended = false;
  if (m.kind === "error") { kOut = "e" + kSrc; samples = [{ t: kSrc / BP.src, w: 1 }]; }
  else if (m.kind === "off") { kOut = Math.floor(vt * m.fps + 1e-6); samples = [{ t: kOut / m.fps, w: 1 }]; }
  else {
    kOut = Math.floor(vt * m.out + 1e-6);
    const center = Math.floor((kOut * m.fps) / m.out + 1e-6); // the frame Blur keeps when dropping to the output rate
    if (m.kind !== "blend") samples = [{ t: center / m.fps, w: 1 }];
    else {
      blended = true;
      const r = (m.n - 1) / 2;
      // At most 32 draws: canvases add up in 8 bits, so very small weights would vanish.
      const nb = Math.min(32, m.n);
      samples = [];
      for (let i = 0; i < nb; i++) {
        const a = Math.floor((i * m.n) / nb), z = Math.floor(((i + 1) * m.n) / nb);
        let sw = 0, st = 0;
        for (let j = a; j < z; j++) { sw += m.w[j]; st += m.w[j] * (center + j - r); }
        if (sw > 0) samples.push({ t: st / sw / m.fps, w: sw });
      }
    }
  }
  const key = `${kSrc}|${kOut}|${BP.ver}`;
  if (key === BP.lastKey) return;
  BP.lastKey = key;

  // Original: one sharp frame at the capture rate.
  const S0 = P.src;
  S0.ctx.setTransform(1, 0, 0, 1, 0, 0);
  S0.ctx.drawImage(S0.bg, 0, 0);
  S0.ctx.setTransform(S0.dpr, 0, 0, S0.dpr, 0, 0);
  bpDrawScene(S0.ctx, S0.w, S0.h, kSrc / BP.src);

  // With these settings: add each frame in at its weight ("lighter" sums colour and coverage).
  const O = P.out, a = O.accCtx;
  a.setTransform(1, 0, 0, 1, 0, 0);
  a.globalCompositeOperation = "source-over"; a.globalAlpha = 1;
  a.clearRect(0, 0, O.pw, O.ph);
  a.setTransform(O.dpr, 0, 0, O.dpr, 0, 0);
  a.globalCompositeOperation = "lighter";
  for (const s of samples) { a.globalAlpha = Math.min(1, s.w); bpDrawScene(a, O.w, O.h, s.t); }
  a.globalAlpha = 1; a.globalCompositeOperation = "source-over";

  const gamma = blended ? bpClamp(c.gamma, 0.1, 10, 1) : 1;
  const f = m.kind !== "error" && c.filters && (+c.brightness !== 1 || +c.contrast !== 1 || +c.saturation !== 1)
    ? { br: bpClamp(c.brightness, 0, 10, 1) - 1, ct: bpClamp(c.contrast, 0, 10, 1), sat: bpClamp(c.saturation, 0, 10, 1) } : null;
  O.ctx.setTransform(1, 0, 0, 1, 0, 0);
  if (gamma === 1 && !f) {
    // Plain weighted average: background * (1 - coverage) + colour * coverage.
    O.ctx.drawImage(O.bg, 0, 0);
    O.ctx.drawImage(O.acc, 0, 0);
  } else bpPixels(O, gamma, f);
}

/**
 * Per-pixel version for gamma and filters, as blending.py average_bright() and
 * adjust.Tweak do it: ((1-k)*bg^g + k*fg^g)^(1/g), then on BT.709 YUV
 * Y = Y*contrast + (brightness-1), U and V times saturation.
 */
function bpPixels(O, gamma, f) {
  if (gamma !== 1 && (!BP.lut || BP.lut.g !== gamma)) {
    const pw = new Float32Array(256), inv = new Uint8ClampedArray(4096);
    for (let i = 0; i < 256; i++) pw[i] = Math.pow(i / 255, gamma);
    for (let i = 0; i < 4096; i++) inv[i] = Math.pow(i / 4095, 1 / gamma) * 255 + 0.5;
    BP.lut = { g: gamma, pw, inv };
  }
  const acc = O.accCtx.getImageData(0, 0, O.pw, O.ph).data, bg = O.bgData, out = O.img.data;
  const L = gamma !== 1 ? BP.lut : null;
  for (let i = 0; i < out.length; i += 4) {
    const al = acc[i + 3];
    let r = bg[i], g = bg[i + 1], b = bg[i + 2];
    if (al) {
      const k = al / 255, q = 1 - k;
      if (L) {
        r = L.inv[Math.round((L.pw[r] * q + L.pw[acc[i]] * k) * 4095)];
        g = L.inv[Math.round((L.pw[g] * q + L.pw[acc[i + 1]] * k) * 4095)];
        b = L.inv[Math.round((L.pw[b] * q + L.pw[acc[i + 2]] * k) * 4095)];
      } else { r = r * q + acc[i] * k; g = g * q + acc[i + 1] * k; b = b * q + acc[i + 2] * k; }
    }
    if (f) {
      r /= 255; g /= 255; b /= 255;
      let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
      const u = ((b - y) / 1.8556) * f.sat, v = ((r - y) / 1.5748) * f.sat;
      y = y * f.ct + f.br;
      r = y + 1.5748 * v; b = y + 1.8556 * u; g = (y - 0.2126 * r - 0.0722 * b) / 0.7152;
      r *= 255; g *= 255; b *= 255;
    }
    out[i] = r; out[i + 1] = g; out[i + 2] = b; out[i + 3] = 255;
  }
  O.ctx.putImageData(O.img, 0, 0);
}

// For testing the maths in Node; ignored in the app.
if (typeof module !== "undefined") module.exports = { blurWeights, blurModel, blurParseFps, bwNormalize, bwScaleRange };
