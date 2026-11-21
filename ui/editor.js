"use strict";
// Clip viewer, player and editor. Loaded before app.js: nothing here may touch
// app.js globals (S, $, invoke, ICON, ...) at load time, only inside functions.
// The edit itself lives in an edit model (editmodel.js, em... functions); the
// multi-track timeline is timeline.js (mountTimeline). This file previews the
// model (video, cut skipping, audio levels, added files), runs the side column
// (speed, color, export) and the editor's keyboard shortcuts.
// Naming rule: no local may be called `t`, it would hide the translation function t().

/* ---------- editor data ---------- */
const ED_SPEEDS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4];
// How far the player's back / forward buttons and the Left / Right keys jump, in seconds.
// Right-click either button to pick one; kept on this PC (localStorage["glint.seekStep"]).
const ED_SEEK_STEPS = [1, 2, 3, 5, 10, 15, 30];
const edSeekStep = () => { try { const n = +localStorage.getItem("glint.seekStep"); return ED_SEEK_STEPS.includes(n) ? n : 5; } catch { return 5; } };
const edSetSeekStep = (n) => { try { localStorage.setItem("glint.seekStep", String(n)); } catch {} };
// 0.1.41 (P): Sharpness, 0..1 (0 = off). A color or preset without it reads as 0 (edCol).
// 0.1.45: Temperature -1 (cool) .. +1 (warm) and Glow 0..1, the same way (0 = off, absent reads as 0).
const ED_COLOR_KEYS = ["brightness", "contrast", "saturation", "gamma", "sharpness", "temperature", "glow"];
/** The color keys whose neutral value is 0 (they may be absent from a color or a preset). */
const ED_ZERO_KEYS = ["sharpness", "temperature", "glow"];
const ED_NO_COLOR = { brightness: 1, contrast: 1, saturation: 1, gamma: 1 };
/** Value `k` of color `c`, its neutral value when it is not there (sharpness, temperature, glow 0, the others 1). */
const edCol = (c, k) => (Number.isFinite(c?.[k]) ? c[k] : ED_ZERO_KEYS.includes(k) ? 0 : 1);
/** What picking a look sets besides its own numbers: everything it has not, at its neutral value. */
const ED_LOOK_ZERO = { sharpness: 0, temperature: 0, glow: 0 };
// Names are English keys, shown through t(). The built-in looks have no sharpening: picking one sets it to 0.
// The looks after "Black and white" (0.1.45) use temperature and glow; rules.rs BUILTIN_LOOKS has the same table (A).
const ED_PRESETS = [
  { name: "None", ...ED_NO_COLOR, ...ED_LOOK_ZERO },
  { name: "Vivid", brightness: 1, contrast: 1.08, saturation: 1.35, gamma: 1, ...ED_LOOK_ZERO },
  { name: "Punchy", brightness: 1, contrast: 1.25, saturation: 1.15, gamma: 0.95, ...ED_LOOK_ZERO },
  { name: "Soft", brightness: 1.02, contrast: 0.88, saturation: 0.9, gamma: 1.08, ...ED_LOOK_ZERO },
  { name: "Bright", brightness: 1.1, contrast: 1, saturation: 1.05, gamma: 1.15, ...ED_LOOK_ZERO },
  { name: "Moody", brightness: 0.94, contrast: 1.15, saturation: 0.8, gamma: 0.9, ...ED_LOOK_ZERO },
  { name: "Black and white", brightness: 1, contrast: 1.12, saturation: 0, gamma: 1, ...ED_LOOK_ZERO },
  { name: "Dark fantasy", brightness: 0.92, contrast: 1.2, saturation: 0.85, gamma: 0.88, sharpness: 0, temperature: -0.35, glow: 0.45 },
  { name: "Dreamy", brightness: 1.03, contrast: 0.94, saturation: 1.05, gamma: 1.06, sharpness: 0, temperature: 0.1, glow: 0.6 },
  { name: "Neon night", brightness: 0.97, contrast: 1.2, saturation: 1.4, gamma: 0.92, sharpness: 0, temperature: -0.15, glow: 0.35 },
  { name: "Cold", brightness: 1, contrast: 1.1, saturation: 0.9, gamma: 1, sharpness: 0, temperature: -0.4, glow: 0 },
  { name: "Golden hour", brightness: 1.03, contrast: 1.05, saturation: 1.12, gamma: 1.03, sharpness: 0, temperature: 0.4, glow: 0.15 },
  { name: "Faded film", brightness: 1.02, contrast: 0.86, saturation: 0.8, gamma: 1.1, sharpness: 0, temperature: 0.15, glow: 0 },
];
// key, label, min, max, how the value is shown
const ED_SLIDERS = [
  ["brightness", "Brightness", 0.5, 1.5, "pct"],
  ["contrast", "Contrast", 0.5, 1.5, "pct"],
  ["saturation", "Saturation", 0, 2, "pct"],
  ["gamma", "Gamma", 0.5, 2, "num"],
  ["sharpness", "Sharpness", 0, 1, "pct"],
  ["temperature", "Temperature", -1, 1, "temp"], // shown -100 .. +100, "Cool" and "Warm" at its ends (0.1.45)
  ["glow", "Glow", 0, 1, "pct"],
];
const ED_DRIFT = 0.08;          // seconds a preview audio element may drift from the video before it is re-seeked
const ED_GAIN_STEP = 0.1;       // Up / Down change a segment's level by this much
const ED_ZOOM_STEP = 1.5;       // + / - zoom factor
const ED_THUMBS = 30;           // film strip frames asked from editor_thumbs
const ED_LOAD_TIMEOUT = 15000;  // ms an audio element may take to load
const ED_CHEV = '<svg class="e-chev" viewBox="0 0 24 24"><path d="m7 10 5 5 5-5"/></svg>';
const ED_ICON_UNFULL = '<svg viewBox="0 0 24 24"><path d="M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5"/></svg>';
// Shortcuts that act on the timeline's selection: sent to the timeline's run() when it has one.
const ED_TL_CMDS = new Set(["split", "remove", "mute", "louder", "quieter", "solo", "blank", "selectAll", "clearSel", "hideCuts", "mark", "markNext", "markPrev", "trimStart", "trimEnd", "lift", "blade", "ungroup", "group"]);
/** True if trimming (cmd "trimStart" / "trimEnd") to source time `sec` would leave less than EM_MIN_SEG of the kept span. */
function edTrimRefused(m, cmd, sec) {
  const b = emTrimBounds(m);
  return cmd === "trimStart" ? sec > b.end - EM_MIN_SEG + 1e-6 : sec < b.start + EM_MIN_SEG - 1e-6;
}
const ED_MARKS_KEY = "glint.marks"; // localStorage: the bookmarks of each clip, see edMarksLoad
const ED_MARKS_MAX = 300;           // clips whose bookmarks are remembered; the longest unused are forgotten first

/* Bookmarks per clip, so they are there again when the clip is reopened. Stored as
   [[clipId, [sec, ...]], ...], least recently saved first (an array, not an object:
   an object would put number-like ids first and lose that order). */
function edMarksAll() {
  try {
    const x = JSON.parse(localStorage.getItem(ED_MARKS_KEY) || "[]");
    return Array.isArray(x) ? x.filter((e) => Array.isArray(e) && typeof e[0] === "string" && Array.isArray(e[1])) : [];
  } catch (e) {
    console.warn("Glint: could not read the saved bookmarks", e);
    return [];
  }
}
/** The saved bookmarks of clip `id`: source seconds (unsorted, unchecked: emSetMarks cleans them). */
const edMarksLoad = (id) => edMarksAll().find((e) => e[0] === id)?.[1] || [];
/** Save clip `id`'s bookmarks; an empty list drops the clip's entry. */
function edMarksSave(id, list) {
  const all = edMarksAll().filter((e) => e[0] !== id);
  const keep = (Array.isArray(list) ? list : []).filter(Number.isFinite).map((x) => Math.round(x * 1000) / 1000);
  if (keep.length) all.push([id, keep]);
  while (all.length > ED_MARKS_MAX) all.shift();
  try {
    if (all.length) localStorage.setItem(ED_MARKS_KEY, JSON.stringify(all));
    else localStorage.removeItem(ED_MARKS_KEY);
  } catch (e) { console.warn("Glint: could not save the bookmarks", e); }
}

const edFmtVal = (key, x) => {
  const how = ED_SLIDERS.find((s) => s[0] === key)[4];
  if (how === "pct") return `${Math.round(x * 100)}%`;
  if (how === "temp") { const n = Math.round(x * 100); return n > 0 ? `+${n}` : String(n); } // -100 .. +100 (0.1.45)
  return x.toFixed(2);
};
const edSameColor = (a, b) => ED_COLOR_KEYS.every((k) => Math.abs(edCol(a, k) - edCol(b, k)) < 0.005);
const edBaseName = (path) => String(path).split(/[\\/]/).pop() || String(path);
/** Used to tell whether an edit changed the model (so a no-op leaves no undo step). */
const edState = (m) => JSON.stringify([m.cuts, m.removed, m.audio, m.speed, m.colors, m.blank, ...(m.parts ? [m.parts] : []), ...(emHasLayers(m) ? [m.overlays, m.fadeIn, m.fadeOut] : []), ...(m.canvas ? [m.canvas] : [])]); // the part order only when there are parts; overlays and fades (overlays.js) only when there are any

/**
 * What the Color pane edits. `sels`: the timeline's selected pieces ({ row, seg }),
 * `sel` its primary one, `now` the playhead. Any row of a segment counts.
 * Returns { segs, shown }: segs = sorted segment indexes, or null for the whole
 * clip (nothing selected); shown = the segment whose values the sliders show
 * (the primary, else the first selected; for the whole clip the one at the playhead).
 */
function edColorTarget(m, sels, sel, now) {
  const n = m.colors.length;
  const list = Array.isArray(sels) && sels.length ? sels : sel ? [sel] : [];
  const segs = [...new Set(list.map((s) => s?.seg).filter((i) => Number.isInteger(i) && i >= 0 && i < n))].sort((a, b) => a - b);
  if (!segs.length) return { segs: null, shown: Math.max(0, Math.min(n - 1, emSegmentAt(m, now || 0))) };
  return { segs, shown: segs.includes(sel?.seg) ? sel.seg : segs[0] };
}
/** A text field: keys typed there are never shortcuts. Sliders and check boxes are not text fields. */
const edTextTarget = (node) => !!node?.matches?.('textarea, select, [contenteditable=""], [contenteditable="true"], input:not([type="range"]):not([type="checkbox"]):not([type="radio"]):not([type="button"])');

/** CSS filter for the live color preview, applied in the same order the export uses:
 *  brightness, contrast, saturation, then gamma (an SVG filter, CSS has none). */
function edFilterCss(col, picH) {
  const f = [];
  if (col.brightness !== 1) f.push(`brightness(${col.brightness})`);
  if (col.contrast !== 1) f.push(`contrast(${col.contrast})`);
  if (col.saturation !== 1) f.push(`saturate(${col.saturation})`);
  if (col.gamma !== 1) f.push("url(#e-gamma)");
  // The export's order (0.1.45): eq, temperature, sharpness, glow.
  const temp = edCol(col, "temperature");
  if (Math.abs(temp) > 0.004) f.push(`url(#${edTempFilter(temp)})`);
  const sharp = edCol(col, "sharpness");
  if (sharp > 0.004) f.push(`url(#${edSharpFilter(sharp)})`); // 0.1.41: as the export's unsharp
  const glow = edCol(col, "glow");
  if (glow > 0.004) f.push(`url(#${edGlowFilter(glow, picH)})`);
  return f.join(" ") || "none";
}

/* Temperature and glow in the preview (0.1.45): SVG filters made once each in the page's hidden <svg>, by the binding
 * formulas of CONTRACT_0145 (the export, A, uses the same numbers):
 *   temperature T (-1..1), on RGB after eq:  R' = R(1 + 0.20T), G' = G(1 + 0.02T), B' = B(1 - 0.20T);
 *   glow g (0..1): the highlights max(0, (x - ED_GLOW_KNEE) / (1 - ED_GLOW_KNEE)) per channel, blurred with a Gaussian
 *     of sigma = ED_GLOW_SIGMA x the picture's height, screened over the picture at opacity ED_GLOW_OPACITY x g
 *     (screen with opacity o = screen with the blurred layer multiplied by o, which is how the filter does it). */
const ED_TEMP = { r: 0.2, g: 0.02, b: -0.2 };
const ED_GLOW_KNEE = 0.55, ED_GLOW_SIGMA = 0.016, ED_GLOW_OPACITY = 0.9;
function edDefs() {
  let defs = document.getElementById("e-sharp-defs");
  if (!defs) {
    defs = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    defs.id = "e-sharp-defs";
    defs.setAttribute("aria-hidden", "true");
    defs.setAttribute("style", "position:absolute;width:0;height:0;overflow:hidden");
    document.body.appendChild(defs);
  }
  return defs;
}
/** The id of the page's temperature filter for `T` (in steps of 0.01), made on first use. */
function edTempFilter(T) {
  const q = Math.round(Math.min(1, Math.max(-1, +T || 0)) * 100), id = `e-temp-${q < 0 ? "m" + -q : q}`;
  if (typeof document === "undefined" || document.getElementById(id)) return id;
  const x = q / 100, r = (1 + ED_TEMP.r * x).toFixed(4), g = (1 + ED_TEMP.g * x).toFixed(4), b = (1 + ED_TEMP.b * x).toFixed(4);
  edDefs().insertAdjacentHTML("beforeend", `<filter id="${id}" color-interpolation-filters="sRGB" x="0" y="0" width="100%" height="100%"><feColorMatrix type="matrix" values="${r} 0 0 0 0  0 ${g} 0 0 0  0 0 ${b} 0 0  0 0 0 1 0"/></filter>`);
  return id;
}
/** The id of the page's glow filter for glow `g` (steps of 0.05) on a picture shown `picH` CSS px high (the blur's
 *  deviation in those pixels, steps of half a pixel), made on first use. */
function edGlowFilter(g, picH) {
  const q = Math.round(Math.min(1, Math.max(0, +g || 0)) * 20), sd = Math.max(0.5, Math.round(ED_GLOW_SIGMA * (+picH > 0 ? +picH : 720) * 2) / 2);
  const id = `e-glow-${q}-${String(sd).replace(".", "_")}`;
  if (typeof document === "undefined" || document.getElementById(id)) return id;
  const slope = (1 / (1 - ED_GLOW_KNEE)).toFixed(5), icpt = (-ED_GLOW_KNEE / (1 - ED_GLOW_KNEE)).toFixed(5), op = (ED_GLOW_OPACITY * q / 20).toFixed(4);
  const hi = ["R", "G", "B"].map((c) => `<feFunc${c} type="linear" slope="${slope}" intercept="${icpt}"/>`).join("");
  const lay = ["R", "G", "B"].map((c) => `<feFunc${c} type="linear" slope="${op}"/>`).join("");
  edDefs().insertAdjacentHTML("beforeend", `<filter id="${id}" color-interpolation-filters="sRGB" x="0" y="0" width="100%" height="100%"><feComponentTransfer in="SourceGraphic" result="hi">${hi}</feComponentTransfer><feGaussianBlur in="hi" stdDeviation="${sd}" edgeMode="duplicate" result="bl"/><feComponentTransfer in="bl" result="lay">${lay}</feComponentTransfer><feBlend in="SourceGraphic" in2="lay" mode="screen"/></filter>`);
  return id;
}

/* Sharpness in the preview (0.1.41). CSS cannot sharpen: an SVG unsharp mask does it, the picture plus `a` times
 * (picture - blurred picture), with a = 1.5 x sharpness as the export's ffmpeg `unsharp` luma amount (A: 5x5,
 * 0..1.5). One <filter> per step of 0.05, made once in a hidden <svg> of the page (not of the viewer, so it
 * outlives it); every element that copies the video's CSS filter (the canvas preview) gets it too. */
const ED_SHARP_STEPS = 20;
/** The SVG unsharp numbers for sharpness `s` (0..1): the blur's radius and the composite's k2 / k3. */
function edSharpParams(s) {
  const q = Math.round(Math.min(1, Math.max(0, +s || 0)) * ED_SHARP_STEPS) / ED_SHARP_STEPS, a = 1.5 * q;
  return { q, std: 1, k2: Math.round((1 + a) * 1000) / 1000, k3: Math.round(-a * 1000) / 1000 };
}
/** The id of the page's sharpen filter for sharpness `s`, made on first use. */
function edSharpFilter(s) {
  const { q, std, k2, k3 } = edSharpParams(s), id = `e-sharp-${Math.round(q * ED_SHARP_STEPS)}`;
  if (typeof document === "undefined" || document.getElementById(id)) return id;
  edDefs().insertAdjacentHTML("beforeend", `<filter id="${id}" color-interpolation-filters="sRGB" x="0" y="0" width="100%" height="100%"><feGaussianBlur in="SourceGraphic" stdDeviation="${std}" edgeMode="duplicate" result="b"/><feComposite in="SourceGraphic" in2="b" operator="arithmetic" k1="0" k2="${k2}" k3="${k3}" k4="0"/></filter>`);
  return id;
}

/* The preview's limiter (0.1.45): one DynamicsCompressorNode before the destination, always in the chain. Sound under
 * the ceiling passes untouched; sound that would pass it (a level over 100 %, rows summed) is held under it instead of
 * being cut flat. The export's ffmpeg alimiter (A) has the same ceiling, -1 dBFS. */
// Measured (OfflineAudioContext, 440 Hz sine, Chromium): the compressor adds its own make-up gain (x1.178 at these
// settings), undone by a gain after it, so quiet sound is exactly as loud as without it (200 % of 0.1 -> 0.2000).
// A full-scale sine comes out at 0.778 at 100 %, 0.831 at 200 %, 0.877 at 400 %: always under -1 dBFS (0.891).
// -2.5 dB rather than -1.5: at -1.5 the attack lets 200 % through at 0.923.
const ED_LIMITER = { threshold: -2.5, knee: 0, ratio: 20, attack: 0.003, release: 0.05 };
const ED_LIMITER_MAKEUP = 1.178;
const ED_CEIL_KNEE = 0.8, ED_CEIL_ROOM = 0.091; // the soft ceiling: identity to 0.8, then at most 0.8 + 0.091 = 0.891
let edCeil = null;
/** The soft ceiling's curve (input -1..1): x up to the knee, then knee + room x tanh((|x| - knee) / room). */
function edCeilingCurve() {
  if (edCeil) return edCeil;
  const n = 8193, out = new Float32Array(n);
  for (let i = 0; i < n; i++) {
    const x = (i / (n - 1)) * 2 - 1, a = Math.abs(x);
    out[i] = a <= ED_CEIL_KNEE ? x : Math.sign(x) * (ED_CEIL_KNEE + ED_CEIL_ROOM * Math.tanh((a - ED_CEIL_KNEE) / ED_CEIL_ROOM));
  }
  edCeil = out;
  return out;
}
/** The limiter of AudioContext `ctx` (made once per context, connected to its destination): what rows connect to. */
function edLimiter(ctx) {
  if (ctx.__glintLimiter) return ctx.__glintLimiter;
  const c = ctx.createDynamicsCompressor();
  for (const [k, x] of Object.entries(ED_LIMITER)) c[k].value = x;
  const back = ctx.createGain();
  back.gain.value = 1 / ED_LIMITER_MAKEUP;
  // The compressor follows the level, not every peak (two full-scale rows at 200 % still reach 0.926): a soft ceiling
  // after it catches what is left. It is the identity up to 0.8 (everything quieter passes untouched) and bends
  // smoothly towards 0.889 above that, never past it.
  const ceil = ctx.createWaveShaper();
  ceil.curve = edCeilingCurve();
  ceil.oversample = "4x";
  c.connect(back).connect(ceil).connect(ctx.destination);
  ctx.__glintLimiter = c;
  return c;
}

/** The user's saved color presets, skipping malformed entries. */
function edUserPresets() {
  const list = S.settings?.color_presets;
  // A preset saved before Sharpness (0.1.41) has none: it reads as 0.
  return Array.isArray(list) ? list.filter((p) => p && typeof p.name === "string" && ED_COLOR_KEYS.every((k) => Number.isFinite(p[k]) || (ED_ZERO_KEYS.includes(k) && p[k] == null))) : []; // temperature, glow (0.1.45) too
}

/** The tracks to edit when editor_audio_tracks fails: same rows, no files (the preview then uses the clip's own audio). */
function edFallbackTracks(c) {
  const n = c.audio_tracks || 0;
  // English titles, as the clip's own tags are: the edit model matches them ("Mix", "Desktop", "Game") and the timeline translates them.
  if (n === 3) return [{ index: 0, title: "Mix", path: "" }, { index: 1, title: "Desktop", path: "" }, { index: 2, title: "Microphone", path: "" }];
  if (n === 4) return [{ index: 0, title: "Mix", path: "" }, { index: 1, title: "Desktop", path: "" }, { index: 2, title: "Microphone", path: "" }, { index: 3, title: "Game", path: "" }];
  return Array.from({ length: n }, (_, i) => ({ index: i, title: n === 1 ? t("Audio") : t("Track {n}", { n: i + 1 }), path: "" }));
}

/** Where the playhead should land for source time `at` while editing: never inside a cut-out part.
 *  Moving forward (or a plain seek) goes to the next kept part, moving back to the end of the previous one.
 *  Past the last kept part it stops on that part's last frame. */
function edLand(m, at, back, frame) {
  const kept = emKept(m);
  if (!kept.length) return at;
  if (back) {
    for (let i = kept.length - 1; i >= 0; i--) if (at >= kept[i].start - 1e-6) return Math.min(at, Math.max(kept[i].start, kept[i].end - frame));
    return kept[0].start;
  }
  const p = emPlayable(m, at);
  const last = kept[kept.length - 1];
  return p ?? Math.max(last.start, last.end - frame);
}

/* ---------- keyboard shortcuts (edit mode) ---------- */
// Key names shown in the Shortcuts pop-up, by KeyboardEvent.code. Words go through t() when shown.
const ED_KEY_NAMES = {
  Space: "Space", Delete: "Del", Backspace: "Backspace", ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right", BracketLeft: "[", BracketRight: "]", Comma: ",", Period: ".", Equal: "+", NumpadAdd: "+", Minus: "-", NumpadSubtract: "-", Escape: "Esc", Home: "Home", End: "End",
  Enter: "Enter", NumpadEnter: "Enter", Tab: "Tab", PageUp: "PgUp", PageDown: "PgDn", Insert: "Ins", Slash: "/", Backslash: "\\", Semicolon: ";", Quote: "'", Backquote: "`", NumpadMultiply: "*", NumpadDivide: "/", NumpadDecimal: ".",
};
const edKeyName = (code) => ED_KEY_NAMES[code] || code.replace(/^Key/, "").replace(/^Digit/, "").replace(/^Numpad(?=\d)/, "Num ");
/** One key combination. `codes`: space-separated KeyboardEvent.code values that all count. shift: true, false or null (either). */
const edCombo = (codes, { ctrl = false, shift = false } = {}) => ({ codes: codes.split(" "), ctrl, shift });
// The one table of editor shortcuts: the key handlers match against it and the
// Shortcuts pop-up is built from it. `cmd` is run by the player's command();
// `once` ignores auto-repeat. Keep each group's rows together. `keys` are the
// defaults: S.settings.editor_keys may replace them per cmd (see edKeysOf).
const ED_KEYS = [
  { group: "Playback", what: "Play or pause", cmd: "play", once: true, keys: [edCombo("Space")] },
  { group: "Playback", what: "Seek back", cmd: "back5", keys: [edCombo("ArrowLeft")] },       // by the seek step (5 s unless changed)
  { group: "Playback", what: "Seek forward", cmd: "fwd5", keys: [edCombo("ArrowRight")] },
  { group: "Playback", what: "Back 1 second", cmd: "back1", keys: [edCombo("ArrowLeft", { shift: true })] },
  { group: "Playback", what: "Forward 1 second", cmd: "fwd1", keys: [edCombo("ArrowRight", { shift: true })] },
  { group: "Playback", what: "Back one frame", cmd: "framePrev", keys: [edCombo("Comma")] },
  { group: "Playback", what: "Forward one frame", cmd: "frameNext", keys: [edCombo("Period")] },
  { group: "Playback", what: "Jump to the previous cut point", cmd: "cutPrev", keys: [edCombo("BracketLeft")] },
  { group: "Playback", what: "Jump to the next cut point", cmd: "cutNext", keys: [edCombo("BracketRight")] },
  { group: "Playback", what: "Go to the start", cmd: "home", keys: [edCombo("Home")] },
  { group: "Playback", what: "Go to the end", cmd: "end", keys: [edCombo("End")] },
  { group: "Playback", what: "Loop on or off", cmd: "loop", once: true, keys: [edCombo("KeyL")] },
  { group: "Playback", what: "Fullscreen editor on or off", cmd: "fullscreen", once: true, keys: [edCombo("KeyF")] },
  { group: "Editing", what: "Split at the playhead", cmd: "split", once: true, keys: [edCombo("KeyS")] },
  { group: "Editing", what: "Cut out the selected segment, or restore it", cmd: "remove", once: true, keys: [edCombo("Delete"), edCombo("Backspace")] },
  { group: "Editing", what: "Cut out the selected segment and leave a gap", cmd: "lift", once: true, keys: [edCombo("Delete", { shift: true })] },
  { group: "Editing", what: "Blade: click the video or an overlay to split it there", cmd: "blade", once: true, keys: [edCombo("KeyC")] },
  { group: "Editing", what: "Ungroup: the selected pieces' sound becomes clips of its own", cmd: "ungroup", once: true, keys: [edCombo("KeyU")] },
  { group: "Editing", what: "Group the selected pieces and clips", cmd: "group", once: true, keys: [edCombo("KeyG")] },
  { group: "Editing", what: "Mute or unmute the selected audio segment", cmd: "mute", once: true, keys: [edCombo("KeyM")] },
  { group: "Editing", what: "Raise the selected audio segment by 10%", cmd: "louder", keys: [edCombo("ArrowUp")] },
  { group: "Editing", what: "Lower the selected audio segment by 10%", cmd: "quieter", keys: [edCombo("ArrowDown")] },
  { group: "Editing", what: "Solo the selected audio segment", cmd: "solo", once: true, keys: [edCombo("KeyD")] },
  { group: "Editing", what: "Hide or show the video in the selected segment", cmd: "blank", once: true, keys: [edCombo("KeyB")] },
  { group: "Editing", what: "Cut out everything before the playhead", cmd: "trimStart", once: true, keys: [edCombo("KeyI")] },
  { group: "Editing", what: "Cut out everything from the playhead on", cmd: "trimEnd", once: true, keys: [edCombo("KeyO")] },
  { group: "Editing", what: "Undo", cmd: "undo", keys: [edCombo("KeyZ", { ctrl: true })] },
  { group: "Editing", what: "Redo", cmd: "redo", keys: [edCombo("KeyY", { ctrl: true }), edCombo("KeyZ", { ctrl: true, shift: true })] },
  { group: "Editing", what: "Export", cmd: "export", once: true, keys: [edCombo("KeyE", { ctrl: true })] },
  { group: "Timeline", what: "Zoom in", cmd: "zoomIn", keys: [edCombo("Equal NumpadAdd", { shift: null })] },
  { group: "Timeline", what: "Zoom out", cmd: "zoomOut", keys: [edCombo("Minus NumpadSubtract", { shift: null })] },
  { group: "Timeline", what: "Fit the whole clip", cmd: "fit", once: true, keys: [edCombo("KeyF", { shift: true })] },
  { group: "Timeline", what: "Add or remove a bookmark at the playhead", cmd: "mark", once: true, keys: [edCombo("KeyN")] },
  { group: "Timeline", what: "Go to the next bookmark", cmd: "markNext", keys: [edCombo("KeyN", { shift: true }), edCombo("BracketRight", { shift: true })] },
  { group: "Timeline", what: "Go to the previous bookmark", cmd: "markPrev", keys: [edCombo("KeyP", { shift: true }), edCombo("BracketLeft", { shift: true })] },
  { group: "Timeline", what: "Select every segment", cmd: "selectAll", once: true, keys: [edCombo("KeyA", { ctrl: true })] },
  { group: "Timeline", what: "Clear the selection", cmd: "clearSel", once: true, keys: [] },
  { group: "Other", what: "Close a pop-up, clear the selection, or leave the editor", cmd: "escape", keys: [edCombo("Escape")] },
];

/* User bindings: S.settings.editor_keys = { [cmd]: ["Ctrl+Shift+KeyZ", ...] }.
   Modifiers in the order Ctrl, Shift, then the KeyboardEvent.code. An entry
   replaces that cmd's defaults, [] means no shortcut, no entry means the defaults.
   The functions below take the map as a parameter so they stay pure (and testable). */
const edUserKeys = () => {
  const u = typeof S !== "undefined" ? S.settings?.editor_keys : null;
  return u && typeof u === "object" && !Array.isArray(u) ? u : {};
};
const ED_MOD_CODE = /^(Control|Shift|Alt|Meta|OS)/; // codes of the modifier keys themselves
const edComboStr = (code, ctrl, shift) => [ctrl && "Ctrl", shift && "Shift", code].filter(Boolean).join("+");
/** "Ctrl+Shift+KeyZ" -> a combo ({ codes: [code], ctrl, shift }), or null if malformed (Alt, Win and bare modifiers are refused). */
function edParseCombo(str) {
  if (typeof str !== "string" || !str) return null;
  const parts = str.split("+"), code = parts.pop();
  if (!/^[A-Za-z][A-Za-z0-9]*$/.test(code || "") || ED_MOD_CODE.test(code)) return null; // a KeyboardEvent.code, not a modifier
  let ctrl = false, shift = false;
  for (const p of parts) {
    if (p === "Ctrl" && !ctrl) ctrl = true;
    else if (p === "Shift" && !shift) shift = true;
    else return null;
  }
  return { codes: [code], ctrl, shift };
}
/** A list of combos as stored strings. A combo with several codes, or with shift: null (either), gives one string per variant. */
const edComboStrs = (list) => list.flatMap((k) => k.codes.flatMap((code) => (k.shift === null ? [false, true] : [k.shift]).map((sh) => edComboStr(code, k.ctrl, sh))));
/** The combos that run `entry` now: the user's, else the defaults. */
function edKeysOf(entry, user = edUserKeys()) {
  const own = user[entry.cmd];
  return Array.isArray(own) ? own.map(edParseCombo).filter(Boolean) : entry.keys;
}
const edComboHits = (k, code, ctrl, shift) => k.codes.includes(code) && k.ctrl === ctrl && (k.shift === null || k.shift === shift);
const edSameList = (a, b) => a.length === b.length && a.every((s) => b.includes(s));

/** Takes the combos `strs` away from every cmd but `keep`. Returns { user: new map, lost: [{ cmd, strs }] }. */
function edStrip(user, keep, strs) {
  const out = { ...user }, lost = [];
  for (const entry of ED_KEYS) {
    if (entry.cmd === keep) continue;
    const list = edComboStrs(edKeysOf(entry, out));
    const left = list.filter((s) => !strs.includes(s));
    if (left.length === list.length) continue;
    out[entry.cmd] = left;
    lost.push({ cmd: entry.cmd, strs: list.filter((s) => strs.includes(s)) });
  }
  return { user: out, lost };
}
/** Drops entries that equal their cmd's defaults, so the settings only hold real changes. */
function edTidy(user) {
  const out = { ...user };
  for (const [cmd, list] of Object.entries(out)) {
    const entry = ED_KEYS.find((x) => x.cmd === cmd);
    if (entry && Array.isArray(list) && edSameList(edComboStrs(list.map(edParseCombo).filter(Boolean)), edComboStrs(entry.keys))) delete out[cmd];
  }
  return out;
}
/** `str` becomes cmd's only shortcut; any other cmd that had it loses it. Returns { user, lost } or null. */
function edBind(user, cmd, str) {
  const k = edParseCombo(str);
  if (!k || !ED_KEYS.some((x) => x.cmd === cmd)) return null;
  const norm = edComboStr(k.codes[0], k.ctrl, k.shift);
  const r = edStrip(user, cmd, [norm]);
  r.user[cmd] = [norm];
  r.user = edTidy(r.user);
  return r;
}
/** Back to cmd's defaults; another cmd that was given one of them loses it. Returns { user, lost } or null. */
function edResetKey(user, cmd) {
  const entry = ED_KEYS.find((x) => x.cmd === cmd);
  if (!entry) return null;
  const out = { ...user };
  delete out[cmd];
  const r = edStrip(out, cmd, edComboStrs(entry.keys));
  r.user = edTidy(r.user);
  return r;
}

/** The shortcut a key press means in edit mode: { entry, combo }, or null. */
function edKeyFind(e, user = edUserKeys()) {
  if (e.altKey || e.metaKey) return null;
  for (const entry of ED_KEYS) {
    const combo = edKeysOf(entry, user).find((k) => edComboHits(k, e.code, e.ctrlKey, e.shiftKey));
    if (combo) return { entry, combo };
  }
  return null;
}
const edComboParts = (k) => [k.ctrl && "Ctrl", k.shift === true && "Shift", edKeyName(k.codes[0])].filter(Boolean);
function edComboHtml(k) {
  return edComboParts(k).map((p) => `<kbd>${esc(t(p))}</kbd>`).join('<span class="e-kplus">+</span>');
}
const edComboText = (k) => edComboParts(k).map((p) => t(p)).join("+");
/** The first shortcut of `cmd` as text ("Ctrl+Z"), or "" when it has none. */
const edKeyText = (cmd) => { const entry = ED_KEYS.find((x) => x.cmd === cmd); const k = entry && edKeysOf(entry)[0]; return k ? edComboText(k) : ""; };
/** A tooltip with the command's current shortcut: "Label (F)". */
const edTip = (label, cmd) => { const k = edKeyText(cmd); return k ? `${label} (${k})` : label; };

/* ---------- pop-ups (speed list, shortcut list) ---------- */
let edPopClose = null; // closes the open pop-up, if any
let edCap = null;      // the Shortcuts pop-up while it waits for a new key: { key(e) }
let edFsLeft = -1e9;   // performance.now() when fullscreen was last left (see the Escape rule below)
document.addEventListener("fullscreenchange", () => {
  if (!document.fullscreenElement) edFsLeft = performance.now();
  // Watching a clip fullscreen: #modal is the fullscreen element (see P.fullscreen); editor.css styles it by this class.
  document.getElementById("modal")?.classList.toggle("fs", document.fullscreenElement?.id === "modal");
});
/* 0.1.46: the Color section's looks dropdown (a <select> that dropdown.js draws). The open editor sets `edLookHook`
   ({ sel(), key(e), act(k), closed(), del(name) }). These listeners are registered at load, before dropdown.js and
   app.js add theirs, so Up / Down on the closed dropdown change the look here instead of opening the list. */
let edLookHook = null;
let edLookKeyAt = -1e9; // the last key press: a row made active just after it was reached with the keyboard
const edLookOpen = () => typeof DD !== "undefined" && DD.list && edLookHook && DD.sel === edLookHook.sel();
window.addEventListener("keydown", (e) => {
  edLookKeyAt = performance.now();
  const s = edLookHook?.sel();
  if (!s || e.target !== s || s.disabled || (typeof DD !== "undefined" && DD.list)) return;
  if ((e.code !== "ArrowDown" && e.code !== "ArrowUp") || e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  try { edLookHook.key(e); } catch (err) { console.error("Glint: look step failed", err); }
}, true);
// The delete x in a saved look's row: deletes it, never picks it (before dropdown.js's click, which picks the row).
window.addEventListener("click", (e) => {
  const x = e.target.closest?.(".e-look-x");
  if (!x || !edLookOpen() || !DD.list.contains(x)) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  const name = x.dataset.name;
  ddClose(true);
  edLookHook.del(name);
}, true);
/** Draws the looks list's extras: no "Custom" row, the divider, a delete x on each saved look. */
function edLookDecorate(list) {
  for (const r of DD.rows) {
    const o = r.__ddOpt;
    if (!o.value) { r.style.display = "none"; continue; }
    if (o.dataset.name && !r.querySelector(".e-look-x")) {
      const name = o.dataset.name;
      r.insertAdjacentHTML("beforeend", `<button type="button" tabindex="-1" class="e-x e-look-x" data-name="${esc(name)}" aria-label="${esc(t("Delete preset {name}", { name }))}" title="${esc(t("Delete preset"))}">${ICON.x}</button>`);
    }
  }
  // (Only when missing: classList.add rewrites the attribute, which would wake the observer below again, for ever.)
  if (!list.classList.contains("e-looks-list")) list.classList.add("e-looks-list");
}
let edLookWatch = null;
if (typeof MutationObserver !== "undefined" && typeof document !== "undefined" && document.body) {
  new MutationObserver(() => {
    const list = edLookOpen() ? DD.list : null;
    if (list === edLookWatch?.list) return;
    if (edLookWatch) { edLookWatch.mo.disconnect(); edLookWatch = null; try { edLookHook?.closed(); } catch (err) { console.error(err); } }
    if (!list) return;
    edLookDecorate(list);
    let act = DD.rows[DD.act] || null;
    const mo = new MutationObserver(() => {
      if (DD.list !== list) return;
      edLookDecorate(list);
      const now = DD.rows[DD.act] || null;
      if (now === act) return;
      act = now;
      if (now && performance.now() - edLookKeyAt < 150) { try { edLookHook?.act(now.__ddOpt.value); } catch (err) { console.error("Glint: look preview failed", err); } }
    });
    mo.observe(list, { childList: true, subtree: true, attributes: true, attributeFilter: ["class"] });
    edLookWatch = { list, mo };
  }).observe(document.body, { childList: true });
}
/** Set while the viewer goes to the previous or next clip in fullscreen: the player being replaced must not leave fullscreen. */
let edKeepFull = false;

// The one window key listener of this file. Registered at load time, before
// app.js adds its own capture listener, so it runs first (and its
// stopImmediatePropagation keeps app.js's handler from seeing the key at all):
// a key being captured for a shortcut goes nowhere else (no clip keybind fires),
// Escape closes an open pop-up (or leaves fullscreen) instead of the whole viewer,
// and the editor's Ctrl shortcuts are handled here, since app.js only passes
// keys without Ctrl to S.player.key.
window.addEventListener("keydown", (e) => {
  if (edCap) { e.preventDefault(); e.stopImmediatePropagation(); edCap.key(e); return; }
  if (typeof askKey !== "undefined" && askKey) return; // app.js's prompt is open: it owns the keyboard
  // The timeline's right-click menu is open: it has its own keys (arrows, Enter, Esc closes it).
  // This listener was added first, so without this it would take Esc before the menu does.
  // The same goes for the bug-report dialog (.bug-dlg), which can be opened over the editor.
  if (document.querySelector(".tl-menu, .bug-dlg, .wn-dlg")) return;
  if (edPopClose && e.code === "Escape") { e.preventDefault(); e.stopImmediatePropagation(); edPopClose(); return; }
  // P-HOOK 0.1.40: Escape hides the Library list's clip preview first (projects.js), before it can close the editor.
  if (e.code === "Escape" && typeof projPvEscape === "function" && projPvEscape()) { e.preventDefault(); e.stopImmediatePropagation(); return; }
  if (typeof S === "undefined" || !S.open || !S.player) return;
  if (e.code === "Escape" && !e.ctrlKey && !e.altKey && !e.metaKey) {
    // R2 #5 (0.1.41): the app's right-click menu (#menu) is open: Esc closes it and nothing else.
    const menu = document.getElementById("menu");
    if (menu && !menu.hidden && menu.innerHTML.trim()) { e.preventDefault(); e.stopImmediatePropagation(); if (typeof closeMenu === "function") closeMenu(); else menu.hidden = true; return; }
    // R2 #1 (0.1.41): Esc in a box of the viewer (a time, a number, a name) gives the box its value back and lets go of
    // it; the editor stays. A search box keeps its own Esc (projects.js clears it).
    if (edEscField(e.target)) { e.preventDefault(); e.stopImmediatePropagation(); return; }
  }
  // Escape while fullscreen only leaves fullscreen. The webview may leave it by
  // itself before the key arrives, so an Escape just after leaving is dropped too
  // (app.js would otherwise take it as "close the viewer").
  if (e.code === "Escape" && (document.fullscreenElement || performance.now() - edFsLeft < 400)) {
    e.preventDefault();
    e.stopImmediatePropagation();
    if (document.fullscreenElement) document.exitFullscreen().catch((err) => console.warn("Glint: exitFullscreen failed", err));
    return;
  }
  S.player.resumeAudio?.();
  if (!S.player.editing || edTextTarget(e.target)) return;
  // 0.1.41: Alt+Left / Alt+Right nudge what is selected on the timeline by one frame (Shift: ten). Alt cannot be bound
  // to a shortcut (edKeyFind), so these never clash with one.
  if (e.altKey && !e.ctrlKey && !e.metaKey && (e.code === "ArrowLeft" || e.code === "ArrowRight")) {
    e.preventDefault();
    e.stopImmediatePropagation();
    const fr = 1 / (S.player.clipMeta?.fps || 60), n = e.shiftKey ? 10 : 1;
    try { S.player.tl?.nudgeSel?.((e.code === "ArrowLeft" ? -1 : 1) * n * fr); } catch (err) { console.error("Glint: nudge failed", err); }
    return;
  }
  const hit = edKeyFind(e);
  // Escape rebound away from "escape": it does nothing in edit mode (rather than closing the viewer).
  if (!hit && e.code === "Escape") { e.preventDefault(); e.stopImmediatePropagation(); return; }
  if (!hit || !(hit.combo.ctrl || hit.entry.cmd === "escape")) return; // the rest arrives through S.player.key
  e.preventDefault();
  e.stopImmediatePropagation();
  if (e.repeat && hit.entry.once) return;
  S.player.command(hit.entry.cmd);
}, true);

/* R2 #1 (0.1.41): Esc in a box of the viewer. A box's value when it took the focus, given back on Esc. */
const edFieldWas = new WeakMap();
document.addEventListener("focusin", (e) => { if (e.target?.matches?.("#modal input, #modal textarea")) edFieldWas.set(e.target, e.target.value); });
/** True if `el` is a box of the open viewer that Esc should only leave: it gets the value it had when it took the focus
 *  back and loses the focus. A box that applies as it is typed in (the zoom %) is told (input + change) so what it
 *  applied goes back too; one that applies on change applies the same value again (no change). Not a search box (its
 *  own Esc clears it). */
function edEscField(el) {
  if (!el?.matches?.("#modal input, #modal textarea, #modal select") || el.matches("[type=search], .pj-q")) return false;
  if (el.matches("textarea, input:not([type=range]):not([type=checkbox]):not([type=radio]):not([type=button])") && edFieldWas.has(el) && el.value !== edFieldWas.get(el)) {
    el.value = edFieldWas.get(el);
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
  }
  el.blur();
  return true;
}

/** Opens a speed list next to `anchor`, positioned inside `host` (which must be positioned). */
function edOpenPop({ anchor, host, value, above = false, onPick, items = ED_SPEEDS, label = (s) => `${s}x` }) {
  edPopClose?.();
  const pop = document.createElement("div");
  pop.className = "e-pop";
  pop.setAttribute("role", "menu");
  pop.innerHTML = items.map((s) => `<button type="button" role="menuitemradio" aria-checked="${s === value}" tabindex="-1" data-e-pick="${s}" class="${s === value ? "on" : ""}">${esc(label(s))}</button>`).join("");
  host.appendChild(pop);
  const hr = host.getBoundingClientRect(), ar = anchor.getBoundingClientRect();
  pop.style.maxHeight = `${Math.max(120, hr.height - 8)}px`;
  const w = pop.offsetWidth, h = pop.offsetHeight;
  const up = above || ar.bottom + h + 8 > window.innerHeight;
  pop.style.left = `${clamp(ar.left + ar.width / 2 - w / 2 - hr.left, 4, Math.max(4, hr.width - w - 4))}px`;
  pop.style.top = `${Math.max(4, up ? ar.top - hr.top - h - 6 : ar.bottom - hr.top + 6)}px`;
  anchor.classList.add("open");
  anchor.setAttribute("aria-expanded", "true");

  const away = (e) => { if (!pop.contains(e.target) && !anchor.contains(e.target)) close(); };
  const scrolled = (e) => { if (!pop.contains(e.target)) close(); };
  function close() {
    if (edPopClose !== close) return;
    edPopClose = null;
    pop.remove();
    anchor.classList.remove("open");
    anchor.setAttribute("aria-expanded", "false");
    document.removeEventListener("pointerdown", away, true);
    document.removeEventListener("scroll", scrolled, true);
    window.removeEventListener("resize", close);
  }
  pop.addEventListener("mousedown", (e) => e.preventDefault()); // keep keyboard focus where it was
  pop.addEventListener("click", (e) => {
    const b = e.target.closest("[data-e-pick]");
    if (!b) return;
    close();
    onPick(+b.dataset.ePick);
  });
  document.addEventListener("pointerdown", away, true);
  document.addEventListener("scroll", scrolled, true);
  window.addEventListener("resize", close);
  edPopClose = close;
}

/** Saves new editor bindings and refreshes everything that shows them. */
function edSaveKeys(user) {
  if (typeof S === "undefined" || !S.settings) return;
  S.settings.editor_keys = user;
  saveSettings();
  S.player?.ed?.keysChanged?.();
}
/** The message for combos another command lost: '{key} was taken from "{other}".' */
function edLostText(lost) {
  return lost.filter((x) => x.strs.length).map((x) => {
    const entry = ED_KEYS.find((k) => k.cmd === x.cmd);
    const key = x.strs.map((s) => edComboText(edParseCombo(s))).join(", ");
    return t('{key} was taken from "{other}".', { key, other: entry ? t(entry.what) : x.cmd });
  }).join(" ");
}

/**
 * The Shortcuts pop-up over the viewer, built from ED_KEYS with the current
 * bindings. Clicking a row's keys waits for a new combination (Ctrl and/or
 * Shift plus one key; Esc cancels). While it waits, the window key listener
 * hands every key to `edCap` only, and the global hotkeys are paused the way
 * app.js's Keybinds page does it (the backend's pause lapses after 12 s, so it is renewed).
 */
function edOpenKeys(host) {
  edPopClose?.();
  const box = document.createElement("div");
  box.className = "e-keys";
  box.setAttribute("role", "dialog");
  box.setAttribute("aria-label", t("Keyboard shortcuts"));
  let cap = null, renew = 0, msg = "", bad = false, q = "";

  const keysHtml = (entry) => {
    const list = edKeysOf(entry);
    return list.length ? list.map(edComboHtml).join('<span class="e-kor"></span>') : `<span class="e-knone">${esc(t("No shortcut"))}</span>`;
  };
  function render() {
    const user = edUserKeys();
    let rows = "", group = "";
    for (const k of ED_KEYS) {
      if (k.group !== group) { group = k.group; rows += `<div class="e-kgroup">${esc(t(group))}</div>`; }
      const what = t(k.what), listening = cap === k.cmd;
      rows += `<div class="e-krow${listening ? " listening" : ""}">
        <button type="button" class="e-kcap" data-e-bind="${esc(k.cmd)}" aria-label="${esc(t("Change the shortcut for {what}", { what }))}">${listening ? `<span class="e-kwait">${esc(t("Press a key…"))}</span>` : keysHtml(k)}</button>
        <span class="e-kwhat">${esc(what)}</span>
        <button type="button" class="btn sm e-kreset" data-e-reset="${esc(k.cmd)}" aria-label="${esc(t("Reset the shortcut for {what}", { what }))}" ${k.cmd in user ? "" : "disabled"}>${esc(t("Reset"))}</button>
      </div>`;
    }
    const keepScroll = $(".e-keys-box", box)?.scrollTop || 0;
    const hadFocus = document.activeElement?.matches?.("[data-e-keys-q]") && box.contains(document.activeElement);
    box.innerHTML = `<div class="e-keys-box">
        <div class="e-keys-head"><div class="e-title">${esc(t("Keyboard shortcuts"))}</div>
          <label class="search e-keys-search"><svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="7"/><path d="m20 20-3.500-3.500"/></svg><input type="text" data-e-keys-q value="${esc(q)}" placeholder="${esc(t("Search shortcuts"))}" aria-label="${esc(t("Search shortcuts"))}" spellcheck="false" autocomplete="off"></label>
          <button type="button" class="btn sm" data-e-keys-resetall ${Object.keys(user).length ? "" : "disabled"}>${esc(t("Reset all"))}</button>
          <button type="button" class="btn sm" data-e-keys-close>${esc(t("Close"))}</button></div>
        <div class="small">${esc(t("These work while the editor is open."))} ${esc(t("Click a shortcut, then press the new keys. Esc cancels."))} ${esc(t("Right-click a shortcut to remove it."))}</div>
        <div class="small e-keys-msg${bad ? " e-bad" : ""}" role="status" ${msg ? "" : "hidden"}>${esc(msg)}</div>
        <div class="e-keys-list">${rows}</div>
        <div class="small e-keys-none" hidden>${esc(t("No shortcut matches."))}</div>
      </div>`;
    $(".e-keys-box", box).scrollTop = keepScroll;
    filter();
    if (hadFocus) { const inp = $("[data-e-keys-q]", box); inp.focus(); inp.setSelectionRange(q.length, q.length); }
  }
  /** Show only the rows whose action or keys contain the search text; a group's title only with a row under it. */
  function filter() {
    const needle = q.trim().toLowerCase();
    let shown = 0, head = null, any = false;
    for (const el of $$(".e-keys-list > *", box)) {
      if (el.classList.contains("e-kgroup")) { if (head) head.hidden = !any; head = el; any = false; continue; }
      const hit = !needle || el.textContent.toLowerCase().includes(needle);
      el.hidden = !hit;
      if (hit) { any = true; shown++; }
    }
    if (head) head.hidden = !any;
    const none = $(".e-keys-none", box);
    if (none) none.hidden = shown > 0;
  }
  const say = (text, isBad = false) => { msg = text; bad = isBad; };
  const suspend = (on) => { try { invoke("suspend_hotkeys", { on }).catch((e) => console.warn("Glint: suspend_hotkeys failed", e)); } catch (e) { console.warn("Glint: suspend_hotkeys failed", e); } };

  function stopCapture() {
    if (cap === null) return;
    cap = null;
    edCap = null;
    clearInterval(renew);
    renew = 0;
    window.removeEventListener("blur", cancelCapture);
    suspend(false);
  }
  // Leaving the window (back to a game, say) must not leave the clip hotkeys paused.
  function cancelCapture() { stopCapture(); render(); }
  function startCapture(cmd) {
    if (cap === null) {
      suspend(true);
      renew = setInterval(() => suspend(true), 5000);
      window.addEventListener("blur", cancelCapture);
    }
    cap = cmd;
    edCap = { key: onKey };
    say("");
    render();
  }
  function onKey(e) {
    if (e.repeat) return;
    if (e.code === "Escape") { stopCapture(); say(""); return render(); }
    if (!e.code || ED_MOD_CODE.test(e.code)) return; // a modifier on its own: wait for the key
    if (e.altKey || e.metaKey) { say(t("Alt and Win cannot be used here. Use Ctrl, Shift or a key on its own."), true); return render(); }
    const cmd = cap, r = edBind(edUserKeys(), cmd, edComboStr(e.code, e.ctrlKey, e.shiftKey));
    stopCapture();
    if (!r) return render();
    say(edLostText(r.lost));
    edSaveKeys(r.user);
    render();
  }

  render();
  host.appendChild(box);
  function close() {
    if (edPopClose !== close) return;
    edPopClose = null;
    stopCapture();
    box.remove();
  }
  box.addEventListener("input", (e) => {
    if (!e.target.matches?.("[data-e-keys-q]")) return;
    q = e.target.value;
    filter();
  });
  // A right-click on a shortcut removes it ("No shortcut"); Reset brings the default back.
  box.addEventListener("contextmenu", (e) => {
    const bind = e.target.closest("[data-e-bind]");
    if (!bind) return;
    e.preventDefault();
    e.stopPropagation();
    const cmd = bind.dataset.eBind, entry = ED_KEYS.find((k) => k.cmd === cmd);
    if (!entry) return;
    stopCapture();
    if (edKeysOf(entry).length) {
      edSaveKeys({ ...edUserKeys(), [cmd]: [] });
      say(t('"{what}" has no shortcut now. Reset brings it back.', { what: t(entry.what) }));
    }
    render();
  });
  box.addEventListener("click", async (e) => {
    if (e.target === box || e.target.closest("[data-e-keys-close]")) return close();
    const bind = e.target.closest("[data-e-bind]"), reset = e.target.closest("[data-e-reset]");
    if (bind) {
      if (cap === bind.dataset.eBind) { stopCapture(); return render(); } // a second click cancels
      return startCapture(bind.dataset.eBind);
    }
    if (cap !== null) { stopCapture(); render(); } // a click anywhere else in the pop-up cancels the wait
    if (reset) {
      const r = edResetKey(edUserKeys(), reset.dataset.eReset);
      if (!r) return;
      say(edLostText(r.lost));
      edSaveKeys(r.user);
      render();
    } else if (e.target.closest("[data-e-keys-resetall]")) {
      if (!(await ask(t("Reset every editor shortcut to its default?"), t("Reset all")))) return;
      if (edPopClose !== close) return; // closed meanwhile
      say(t("All editor shortcuts are back to their defaults."));
      edSaveKeys({});
      render();
    }
  });
  edPopClose = close;
}

/* ---------- media helpers ---------- */
/** Resolves once `a` can play its first frame; rejects on error, abort or after ED_LOAD_TIMEOUT. */
function edMediaReady(a, label) {
  if (a.readyState >= 2) return Promise.resolve();
  return new Promise((ok, bad) => {
    const done = (fn, arg) => {
      clearTimeout(timer);
      a.removeEventListener("loadeddata", onData);
      a.removeEventListener("error", onError);
      a.removeEventListener("emptied", onGone);
      fn(arg);
    };
    const onData = () => done(ok);
    const onError = () => done(bad, new Error(`Could not load ${label} (${a.error?.message || "media error " + a.error?.code})`));
    const onGone = () => done(bad, new Error(`${label} was unloaded while loading`));
    const timer = setTimeout(() => done(bad, new Error(`${label} took too long to load`)), ED_LOAD_TIMEOUT);
    a.addEventListener("loadeddata", onData);
    a.addEventListener("error", onError);
    a.addEventListener("emptied", onGone);
  });
}
function edKillMedia(a) {
  a.pause();
  a.removeAttribute("src");
  a.load();
  a.remove();
}

/* "Replace the original" in the Export box (0.1.23): the export takes the clip's place instead of
   being saved beside it; the original goes to the Recycle Bin. Remembered on this PC. */
const edReplaceOn = () => { try { return localStorage.getItem("glint.exportReplace") === "1"; } catch { return false; } };
/**
 * A clip's file as the viewer loads it (0.1.35). The page keeps the media it loaded by URL, and the
 * asset protocol's URL is the file's path: an export that took the place of a file (Replace the
 * original takes its name; a new export takes the name of one that was deleted) played the OLD
 * file's sound and picture from memory, so an added song was missing. The clip's id and size give
 * each file its own address; the asset protocol reads only the path.
 */
const edClipUrl = (c) => {
  const url = fileSrc(c.path);
  return `${url}${url.includes("?") ? "&" : "?"}v=${encodeURIComponent(`${c.id}-${c.size || 0}`)}`;
};
const edSetReplace = (on) => { try { localStorage.setItem("glint.exportReplace", on ? "1" : "0"); } catch { /* not remembered */ } };

/* ---------- viewer ---------- */
function blurPanel(c) {
  const job = S.jobs[c.id];
  const st = job?.state;
  if (st === "running" || st === "queued") {
    const pct = clamp(+job.pct || 0, 0, 100);
    // Before Blur's first percent the bar keeps moving and the line says what happens: "0%" looked stuck.
    const wait = jobWaits(job);
    return `<div class="small">${esc(st === "queued" ? t("Waiting to render") : wait ? blurWaitText(job.stage) : t("Rendering with Blur… {pct}%", { pct: pct.toFixed(0) }))}</div>
      ${st === "running" ? `<div class="progress${wait ? " wait" : ""}"><i style="width:${pct}%"></i></div>` : ""}
      ${wait ? `<div class="small">${esc(t("The first percent can take a while: Blur reads the clip in first."))}</div>` : ""}
      <div class="small">${esc(t("It keeps rendering in the background. You can follow it under the Renders button at the top of the window."))}</div>
      <button class="btn wide" data-v="blur-done">${esc(t("Done"))}</button>`;
  }
  // "cancelled", "error", "done" or no job: not rendering.
  const b = S.settings.blur;
  const note = st === "error" ? `<div class="small e-bad">${esc(job.message)}</div>` : "";
  // Under the button: just a link to the Blur page (with the reason first, when Blur is missing).
  // The current settings are its tooltip.
  const now = b.interpolate ? t("{fps} fps out, amount {amount}, interpolate {interp}", { fps: b.output_fps, amount: b.amount, interp: b.interpolated_fps }) : t("{fps} fps out, amount {amount}", { fps: b.output_fps, amount: b.amount });
  const link = `<button class="text-link" data-v="blur-page" title="${esc(now)}">${esc(t("Change Blur settings"))}</button>`;
  return `${note}<button class="btn wide" data-v="blur" ${S.blur ? "" : "disabled"}>${ICON.blur}<span>${esc(t("Render with Blur"))}</span></button>
    <div class="small">${S.blur ? "" : esc(t("blur-cli.exe was not found.")) + " "}${link}</div>`;
}

function edTracksText(n) {
  if (!n) return t("No audio");
  if (n === 1) return t("1 audio track");
  if (n === 3) return t("Mix + desktop + mic tracks");
  if (n === 4) return t("Mix + desktop + mic + game tracks");
  return t("{n} audio tracks", { n });
}

/**
 * The clips before and after clip `id` in the list the library shows now (its page, folder, game,
 * search and sort): { prev, next } ids, null at either end. A clip that is not in that list (opened
 * from somewhere else) uses the whole library, newest first.
 */
function viewerNeighbours(id) {
  let list = [];
  try { list = typeof visibleClips === "function" && typeof isLib === "function" && isLib() ? visibleClips().list : []; } catch { list = []; }
  if (!list.some((c) => c.id === id)) list = [...S.clips].sort((a, b) => b.created - a.created);
  const i = list.findIndex((c) => c.id === id);
  return { prev: i > 0 ? list[i - 1].id : null, next: i >= 0 && i < list.length - 1 ? list[i + 1].id : null };
}

/* P-HOOK (0.1.38, agent P, for agent B): `openClip(clip)` also takes a clip OBJECT. projects.js opens a
 * virtual project as openClip(await invoke("project_open", { id })): c.virtual === true, c.draft === true,
 * kind "joined", c.path === "", c.sources = [{ clip, path, title, duration, start, width, height, fps,
 * audio_tracks, thumb, missing }], and c.joins = the source STARTS INCLUDING the first 0 ([0, d0, d0+d1, ...],
 * one per source, as join_names), unlike a draft's joins (starts after the first). The same object is
 * S.projects[i].clip, so clipById(id) finds it too. A project made before 0.1.38 still opens by id (its draft).
 * When clips are added to the open virtual project, projects.js projGrow updates P.clipMeta in place (sources,
 * joins, join_names, duration), calls emAddParts on P.model and then P.onSources?.(c) (define it to load the new
 * sources without a reload), else P.refreshTimeline(). Export: edExport calls "export_project" when virtual. */
function openClip(id) {
  const given = id && typeof id === "object" ? id : null; // P-HOOK: a clip object (a virtual project)
  if (given) id = given.id;
  // clipById (join.js) also finds an "Edit together" draft, which is never in S.clips.
  const c = given || (typeof clipById === "function" ? clipById(id) : S.clips.find((x) => x.id === id));
  if (!c) return;
  // A clip whose file was deleted from this PC after an upload (share.js): offer the download instead.
  if (!given && typeof shareCloudOnlyOpen === "function" && shareCloudOnlyOpen(id)) return;
  const draft = !!c.draft; // a project's draft (projects.js): edit mode only, nothing to keep, copy, rename or delete
  // An open project saves its edit before it goes (projects.js); it never asks and is never deleted.
  if (S.player && typeof projFlush === "function") projFlush(S.player);
  if (S.player) { S.player.destroy(); S.player = null; }
  edDropInit();
  S.open = id;
  const m = $("#modal");
  m.hidden = false;
  const gammaFn = (ch) => `<feFunc${ch} type="gamma" amplitude="1" exponent="1" offset="0"/>`;
  const L = (s) => esc(t(s));
  m.innerHTML = `
    <div class="viewer">
      <div class="v-main">
        <div class="player">
          <svg class="e-defs" aria-hidden="true"><filter id="e-gamma" color-interpolation-filters="sRGB"><feComponentTransfer>${gammaFn("R")}${gammaFn("G")}${gammaFn("B")}</feComponentTransfer></filter></svg>
          <video src="${edClipUrl(c)}" autoplay playsinline></video>
          <div class="p-audio" aria-hidden="true"></div>
          <div class="e-blank-tag">${L("Video hidden in this segment")}</div>
          <div class="p-big" hidden>${PICON.play}</div>
          <div class="p-ui">
            <div class="p-bar">
              <div class="p-track"><div class="p-buf"></div><div class="p-prog"></div></div>
              <div class="p-knob"></div>
              <div class="p-tip" hidden></div>
            </div>
            <div class="p-row">
              <button data-p="play" tabindex="-1" aria-label="${L("Play or pause")}">${PICON.pause}</button>
              <button data-p="back" tabindex="-1" aria-label="${L("Back 5 seconds")}">${PICON.back}</button>
              <button data-p="fwd" tabindex="-1" aria-label="${L("Forward 5 seconds")}">${PICON.fwd}</button>
              <button data-p="mute" tabindex="-1" aria-label="${L("Mute")}">${PICON.vol}</button>
              <input class="p-vol" type="range" min="0" max="1" step="0.02" aria-label="${L("Volume")}">
              <span class="p-clock">0:00 / ${fmtClock(c.duration)}</span>
              <span class="sp"></span>
              <button data-p="speed" class="p-text" tabindex="-1" aria-label="${L("Playback speed")}" aria-haspopup="menu" aria-expanded="false">1x</button>
              <button data-p="loop" tabindex="-1" aria-label="${L("Loop")}">${PICON.loop}</button>
              <button data-p="full" tabindex="-1" aria-label="${L("Fullscreen")}">${PICON.full}</button>
            </div>
          </div>
        </div>
        <div class="e-tl"></div>
      </div>
      <div class="v-side">
        <div class="v-pane v-info">
          ${draft ? `<div class="v-title v-draft-title">${esc(c.title)}</div>` : `<input class="v-title" value="${esc(c.title)}" spellcheck="false" aria-label="${L("Clip title")}">`}
          <div class="v-meta"><b>${esc(c.game)}</b> · ${new Date(c.created).toLocaleString()}<br>${c.width}×${c.height} · ${c.fps} fps · ${fmtDur(c.duration)} · ${fmtSize(c.size)}<br>${esc(edTracksText(c.audio_tracks))}</div>
          ${draft ? `<button class="btn primary wide" data-v="edit">${ICON.cut}<span>${L("Edit")}</span></button>` : `
          <button class="btn primary wide" data-v="copy">${ICON.copy}<span>${L("Copy to clipboard")}</span></button>
          <div class="small">${L("Copies the file. Paste it into Discord or any chat.")}</div>
          <div class="v-sep"></div>
          <button class="btn wide" data-v="edit">${ICON.cut}<span>${L("Edit")}</span></button>
          <div id="v-blur">${blurPanel(c)}</div>
        ${typeof rulesViewerHtml === "function" ? rulesViewerHtml(c) : "" /* rules.js: run a render rule on this clip */}
          <div class="v-sep"></div>
          <button class="btn wide" data-v="fav">${ICON.star}<span>${L(c.favorite ? "Remove favorite" : "Add to favorites")}</span></button>
          <button class="btn wide" data-v="reveal">${ICON.folder}<span>${L("Show in folder")}</span></button>
          <button class="btn wide danger" data-v="delete">${ICON.trash}<span>${L("Delete")}</span></button>`}
          <span style="flex:1"></span>
          <button class="btn wide" data-v="close">${L("Close")}</button>
        </div>
        <div class="v-pane v-edit">
          <div class="e-head"><div class="e-h${draft ? " e-h-draft" : ""}">${draft ? esc(c.title) : L("Edit")}</div>
            <button type="button" class="e-ibtn" data-v="shortcuts" aria-label="${L("Keyboard shortcuts")}" title="${L("Keyboard shortcuts")}">${ICON.keys}</button>
            <button type="button" class="e-ibtn" data-v="library-window" aria-label="${L("Library in its own window")}" title="${L("Open the library in its own window, to drag clips from it into the editor")}">${PICON.libwin}</button>
            <button type="button" class="e-ibtn e-fsbtn" data-v="editor-full" aria-pressed="false"></button>
            <button class="btn sm" data-v="edit-done">${draft ? L("Close") : L("Done")}</button></div>
          ${draft ? `<div class="small e-draft-note">${L("Export saves the result as a new clip. The clips themselves stay as they are.")}</div>` : ""}
          <section class="e-sec e-speed-sec">
            <div class="e-title e-speed-title">${L("Speed")}</div>
            <div class="e-line"><button type="button" class="btn sm e-speed" aria-haspopup="menu" aria-expanded="false" aria-label="${L("Clip speed")}"></button><label class="e-speed-numw" hidden><input type="number" class="e-speed-num" min="0.25" max="4" step="0.05" aria-label="${L("Speed of the selected clip")}"> x</label><span class="small e-speed-note"></span></div>
          </section>
          <section class="e-sec e-color"></section>
          <section class="e-sec e-export"></section>
        </div>
      </div>
      <div class="e-drop" hidden><div class="e-drop-box">${L("Drop audio files to add them as new tracks")}</div></div>
    </div>`;
  const root = $(".viewer", m);
  // Arrows at the left and right of the viewer: the clip before and after this one in the list
  // the library shows. Not for a project, and hidden while editing (editor.css).
  if (!draft) {
    const near = viewerNeighbours(id);
    const arrow = (act, to, label, key, path) => (to ? `<button class="v-nav v-nav-${act}" data-v="${act}" data-sfx="tap" title="${esc(`${t(label)} (${key})`)}" aria-label="${L(label)}"><svg viewBox="0 0 24 24"><path d="${path}"/></svg></button>` : "");
    m.insertAdjacentHTML("beforeend", arrow("prev", near.prev, "Previous clip", "Page Up", "m15 5-7 7 7 7") + arrow("next", near.next, "Next clip", "Page Down", "m9 5 7 7-7 7"));
  }
  // In fullscreen the arrows lie over the player: the pointer moving over them keeps the controls (and them) up.
  m.onmousemove = (e) => { if (e.target.closest?.(".v-nav")) S.player?.wake?.(); };
  S.player = mountPlayer(root, c);
  S.player.ed = mountEditor(root, c, S.player);
  // Text, shapes, drawing, blur boxes and fades on the picture (overlays.js).
  if (typeof ovMount === "function") { try { S.player.ov = ovMount(root, c, S.player); } catch (e) { console.error("Glint: the overlay tools did not load", e); } }
  // The canvas: another shape for the result (vertical for TikTok / Shorts), with pieces of the picture placed on it (canvas.js).
  if (typeof cvMount === "function") { try { S.player.cv = cvMount(root, c, S.player); } catch (e) { console.error("Glint: the canvas tools did not load", e); } }
  if (typeof zmMount === "function") { try { S.player.zm = zmMount(root, c, S.player); } catch (e) { console.error("Glint: the zoom tools did not load", e); } } // 0.1.41: Zoom (zoom.js)
  // A project: the library's clips beside the editor, to add more (projects.js). P-HOOK 0.1.40: a single library
  // clip gets it too (shown only while editing; adding from it makes a project of the clip, projFromClip).
  if (typeof projMountPanel === "function") projMountPanel(root, id);
  // While editing, the viewer fills the window: top row, Library, player, inspector, timeline (workspace.js).
  if (typeof wsMount === "function") { try { wsMount(root, { title: c.title }); } catch (e) { console.error("Glint: the editor layout did not load", e); } }
  if (draft) {
    // A draft has nothing to view: it opens in edit mode, and leaving edit mode closes it
    // (closeClip asks first when there is work to lose).
    const P = S.player, setEdit = P.setEdit;
    P.setEdit = P.setTrim = (on) => (!on && P.editing && S.player === P ? (closeClip(), Promise.resolve(null)) : setEdit(on));
    const v = $("video", root);
    const go = () => { if (S.open === id && S.player === P && !P.editing) setEdit(true); };
    if (v && v.readyState >= 1) go(); else v?.addEventListener("loadedmetadata", go, { once: true });
  } else $(".v-title", root).onchange = async (e) => {
    try { replaceClip(await invoke("rename_clip", { id, title: e.target.value })); } catch (err) { fail(err); e.target.value = c.title; }
  };
  // Clicking a button keeps keyboard focus off it (so Space still plays / pauses),
  // but first takes it away from any text box or slider in the viewer.
  root.addEventListener("mousedown", (e) => {
    if (!e.target.closest("button")) return;
    if (document.activeElement?.matches?.(".viewer input")) document.activeElement.blur();
    e.preventDefault();
  });
  // Web Audio may only start after a user gesture.
  root.addEventListener("pointerdown", () => S.player?.resumeAudio?.());
}

/**
 * Where each piece of a project that references its clips starts, in project time, and its name (0.1.41, F1): one per
 * source, or one per clip inside a source that has `inner` ([{ at, title }], `at` in that source's own file: an old
 * project's draft file is its source 0, and the clips it was joined from are its pieces). { starts, titles }.
 */
function edPieceStarts(sources) {
  const starts = [], titles = [];
  (Array.isArray(sources) ? sources : []).forEach((s, i) => {
    const base = +s.start || 0, dur = +s.duration || 0;
    const inner = Array.isArray(s.inner) ? s.inner.filter((x) => x && Number.isFinite(+x.at) && +x.at >= 0 && +x.at < dur - 1e-3).sort((a, b) => a.at - b.at) : [];
    if (inner.length && inner[0].at > 1e-3) inner.unshift({ at: 0, title: s.title }); // footage before the first inner clip: the source's own name
    const list = inner.length ? inner : [{ at: 0, title: s.title }];
    for (const x of list) {
      const at = base + +x.at;
      if (starts.length && at < starts[starts.length - 1] + 1e-3) continue; // never twice at one place
      starts.push(at);
      titles.push((typeof x.title === "string" && x.title.trim()) || s.title || t("Clip {n}", { n: i + 1 }));
    }
  });
  return { starts, titles };
}

/* ---------- player: Glint's own transport and the edit preview ---------- */
// `root` is the .viewer element.
function mountPlayer(root, c) {
  const v0 = $("video", root), box = $(".player", root), bar = $(".p-bar", root);
  // Gapless cuts (0.1.41): a second <video>, the STANDBY, waits hidden and muted at the first frame of the next piece
  // that is not the following footage; at the cut the two swap roles. `v` is always the one shown (P.v follows, and
  // P.onVideo(fn) tells canvas.js); every listener sits on both and acts only for the one it is about.
  let v = v0;
  const v1 = document.createElement("video");
  v1.className = "e-standby";
  v1.muted = true;
  v1.preload = "auto";
  v1.playsInline = true;
  v1.tabIndex = -1;
  v1.setAttribute("aria-hidden", "true");
  v0.after(v1);
  let w = v1;
  const vids = [v0, v1];
  const el = (s) => $(s, root);
  const dur = () => (c.virtual && +c.duration > 0 ? +c.duration : Number.isFinite(v.duration) && v.duration > 0 ? v.duration : c.duration) || 1; // a project that references its clips: all of them (0.1.38)
  const clamp = (x, lo, hi) => Math.min(hi, Math.max(lo, x));
  const frame = 1 / (c.fps || 60);
  const P = {
    v, loop: false, editing: false,
    viewSpeed: 1,             // the player's own speed menu (the export speed is model.speed)
    vol: 0.6, muted: false,   // master volume, also applied to the preview audio
    model: null,              // the edit model, made on first entering edit mode, dropped with the player
    tl: null,                 // timeline handle while edit mode is on
    sel: null,                // last primary selection the timeline reported: { row, seg }
    sels: [],                 // every selected piece it reported (row = "video" or an audio row id)
    src: new Map(),           // preview audio by model row id: { id, kind, path, title, a, node, gain, state }
    trackState: "idle",       // the clip's own tracks: idle | loading | on | failed | none
    ctx: null,                // AudioContext: null = not made yet, false = not available
    waves: new Map(),         // waveforms by row id, kept to hand over again when the timeline is remounted
    thumbs: null,             // film strip urls
    duration: dur,
  };
  let raf = 0, idle = 0, drag = false, dead = false, driftTimer = 0, modelJob = null, extrasAsked = false, trackGen = 0;

  try { const x = parseFloat(localStorage.getItem("glint.vol") ?? "0.6"); P.vol = Number.isFinite(x) ? clamp(x, 0, 1) : 0.6; }
  catch (e) { console.warn("Glint: could not read the saved volume", e); }

  const ed = () => (P.editing && P.model ? P.model : null);
  const tl = (fn) => {
    if (!P.tl) return;
    try { fn(P.tl); } catch (e) { console.error("Glint: timeline call failed", e); }
  };
  P.alive = () => !dead;
  P.refreshTimeline = () => tl((h) => h.refresh());
  P.draft = !!c.draft; // a project's draft: Export is always possible (it saves the clips joined as they are)
  P.clipMeta = c;      // the clip as opened (projects.js saves a project's edit with its joins)

  /* ----- timeline time and the video's time ----- */
  // With parts ("Edit together", m.parts) the model, the seek bar, the timeline and added files count
  // TIMELINE seconds while the <video> and the clip's own tracks play the SOURCE. Without parts (and
  // outside edit mode) the two are the same. playSrc is the index of the part (piece) being played (0.1.37: an index,
  // since the same footage may lie on the timeline twice):
  // set on every seek, moved on at every jump, so the playhead stays inside that part while the video
  // runs past its end until the jump to the next part has landed (it never flashes elsewhere).
  let playSrc = null;
  const pm = () => { const m = ed(); return m && m.parts ? m : null; };
  const playPart = (m) => { const p = playSrc === null ? null : emParts(m)[playSrc]; return p && !p.gap ? p : null; };
  /* After the video (0.1.35): from m.duration on (up to emEnd, where the result ends, and the free space after it)
   * the <video> cannot be the clock, it stops at its end. There the player keeps its own: `aft` is the playhead
   * then ({ at, run, t0 }: timeline second `at` at performance time `t0`, moving at the preview's rate while `run`),
   * null while the video is the clock. The picture is black (e-blank), the clip's own tracks are silent, overlay
   * items and file rows go on (they follow tlNow()). */
  let aft = null;
  const aftRate = () => clamp(P.viewSpeed * (ed() ? P.model.speed || 1 : 1), 0.0625, 16);
  // The rate is taken when the clock first runs and kept; applyRate starts the clock afresh when it changes (a speed
  // picked while playing in a gap or after the video, 0.1.41: the time before that went at the old rate).
  const aftNow = () => (aft.run ? aft.at + ((performance.now() - aft.t0) / 1000) * (aft.rate ??= aftRate()) : aft.at);
  const aftRebase = () => { if (aft && aft.run && aft.rate !== undefined && aft.rate !== aftRate()) aft = { at: aftNow(), run: true, t0: performance.now(), rate: aftRate() }; };
  /** Put the playhead after the video at timeline second `sec`, running or standing; the video pauses at its end. */
  function setAft(sec, run) {
    aft = { at: sec, run: !!run, t0: performance.now() };
    if (!v.paused) v.pause();
    aftTick();
  }
  // Nothing of the video's (timeupdate) drives the picture after it, and requestAnimationFrame stops in a hidden
  // window: a slow timer keeps the clock checked (the end, the loop) while it runs.
  let aftTimer = 0;
  function aftTick() {
    const want = !!aft && aft.run && !dead;
    if (want && !aftTimer) aftTimer = setInterval(() => { if (aft && aft.run && !dead) paint(); else aftTick(); }, 100);
    else if (!want && aftTimer) { clearInterval(aftTimer); aftTimer = 0; }
  }
  /** Leave the after-the-video clock standing where it is now. */
  const aftStop = () => { if (aft && aft.run) aft = { at: aftNow(), run: false, t0: performance.now() }; };
  /** True while playing: the video, or the clock after it (or the video taking another source's file to play on). */
  const playing = () => (aft ? aft.run : !v.paused || !!vpend?.play);
  P.afterVideo = () => !!aft;
  P.playing = playing;

  /* ----- a project that references its clips (0.1.38) -----
   * c.virtual with c.sources: no joined file. The model's "source" time is project time (the clips end to end,
   * emSourceAt); the one <video> plays the file of the source the playhead needs (vk), at project time - its start.
   * Switching files: the last picture is held on a canvas over the video (.e-hold) until the new file has seeked, and
   * the clock waits meanwhile (vNow gives the target). A missing source has no picture: its pieces play on the own
   * clock like a gap, black with "Clip missing: <title>". */
  const VS = c.virtual && Array.isArray(c.sources) && c.sources.length
    ? c.sources.map((s, k) => ({ ...s, k, start: +s.start || 0, duration: +s.duration || 0 })) : null;
  let vk = -1, vpend = null;
  P.virtual = !!VS;
  P.sources = () => VS;
  const vUrl = (s) => { const u = fileSrc(s.path); return `${u}${u.includes("?") ? "&" : "?"}v=${encodeURIComponent(s.clip || s.k)}`; };
  /** The video's time as source (project) time. */
  const vNow = () => (!VS ? v.currentTime : vpend ? VS[vpend.k].start + vpend.local : (vk >= 0 ? VS[vk].start : 0) + v.currentTime);
  /** True if project second `s` lies in a source that is missing. */
  const vMissing = (s) => !!VS && !!VS[emSourceAt(VS, s)]?.missing;
  /** No video at timeline second `sec`: a gap, or a missing source's piece. */
  const noVid = (m, sec) => emGapAt(m, sec) >= 0 || (!!VS && vMissing(emToSrc(m, sec)));
  const hold = document.createElement("canvas");
  hold.className = "e-hold";
  hold.hidden = true;
  const missTag = document.createElement("div");
  missTag.className = "e-missing-tag";
  missTag.hidden = true;
  if (VS) { v1.after(hold); box.appendChild(missTag); } // over both videos (0.1.41)
  function holdFrame() {
    if (v.readyState < 2 || !v.videoWidth) return;
    const W = v.offsetWidth, H = v.offsetHeight;
    if (!W || !H) return;
    Object.assign(hold.style, { left: `${v.offsetLeft}px`, top: `${v.offsetTop}px`, width: `${W}px`, height: `${H}px` });
    const dpr = window.devicePixelRatio || 1;
    hold.width = Math.round(W * dpr);
    hold.height = Math.round(H * dpr);
    const g = hold.getContext("2d"), k = Math.min(hold.width / v.videoWidth, hold.height / v.videoHeight);
    try { g.drawImage(v, (hold.width - v.videoWidth * k) / 2, (hold.height - v.videoHeight * k) / 2, v.videoWidth * k, v.videoHeight * k); hold.hidden = false; } catch { hold.hidden = true; }
  }
  /** Put the video at source (project) second `s`: the right file first. */
  function vSet(s) {
    if (!VS) { v.currentTime = s; return; }
    const { k, local } = emProjToFile(VS, s);
    // That file is still loading: it seeks there when ready; once its length is known, there and then (0.1.41: a
    // scrub went on only after the 4 s timer and then snapped back to where it entered the clip).
    if (vpend && vpend.k === k) { vpend.local = local; if (v.readyState >= 1) v.currentTime = local; return; }
    if (k === vk && !vpend) { v.currentTime = local; return; }
    const play = !v.paused || !!vpend?.play;
    if (!vpend) holdFrame();
    vpend = { k, local, play, t0: performance.now() };
    vk = k;
    v.autoplay = false; // the viewer's video autoplays its first file; another clip's file plays only when asked
    v.src = vUrl(VS[k]);
    clearTimeout(vpendTimer);
    vpendTimer = setTimeout(() => vDone(true), 4000); // a file that never answers: go on without it
  }
  let vpendTimer = 0;
  function vDone(failed) {
    clearTimeout(vpendTimer);
    if (!vpend) return;
    const p = vpend;
    vpend = null;
    hold.hidden = true;
    P.lastSwitchMs = Math.round(performance.now() - p.t0);
    if (failed) console.warn("Glint: a project's clip did not load in time", VS[p.k]?.path);
    lateSeek = failed && p.k === vk ? { k: p.k, local: p.local } : null; // its length may still come: it seeks there then
    if (p.play && !aft) v.play().catch((e) => console.warn("Glint: play() was refused", e));
    else if (!v.paused) v.pause();
    state();
    if (P.editing) output(true); // this clip's own track sound, or the video's while it is not ready (0.1.41)
  }
  let lateSeek = null;
  if (VS) {
    // Both videos carry these; each acts only for the one shown (the standby has its own, below).
    for (const x of vids) {
      x.addEventListener("loadedmetadata", () => {
        if (x !== v) return;
        if (vpend) v.currentTime = vpend.local;
        else if (lateSeek && lateSeek.k === vk) { v.currentTime = lateSeek.local; lateSeek = null; }
      });
      x.addEventListener("seeked", () => { if (x === v && vpend && Math.abs(v.currentTime - vpend.local) < 0.25) vDone(false); });
      x.addEventListener("error", () => { if (x === v && vpend) vDone(true); });
    }
    v.muted = true;
    // The first source that has a file (the model and the timeline wait for its length; it is the project's anyway).
    const first = VS.find((s) => !s.missing);
    if (first) { vk = first.k; v.src = vUrl(first); }
  }
  /** The project's picture: the first source's size; a source of another size is fitted inside it. Both videos. */
  function fitVideo() {
    if (!VS || !P.editing || !(c.width > 0 && c.height > 0)) { if (VS) for (const x of vids) for (const k of ["left", "top", "width", "height", "inset"]) x.style.removeProperty(k); return; }
    const W = box.clientWidth, H = box.clientHeight;
    if (!W || !H) return;
    const k = Math.min(W / c.width, H / c.height), w = c.width * k, h = c.height * k;
    for (const x of vids) Object.assign(x.style, { inset: "auto", left: `${(W - w) / 2}px`, top: `${(H - h) / 2}px`, width: `${w}px`, height: `${h}px` });
  }
  if (VS && typeof ResizeObserver === "function") new ResizeObserver(() => { if (!dead) fitVideo(); }).observe(box);

  /* ----- gapless cuts (0.1.41): the standby video -----
   * While a piece plays, the standby (w) is put at the first frame of the piece after it when that is a JUMP (another
   * place in the file, another clip's file); `sb` says what it holds ({ k, local, ready }). At the cut the two swap:
   * the standby is shown and plays on from a frame it has already decoded, the old one pauses and becomes the standby.
   * No seek of the shown video at a cut, so no hitch. P.gapless = false cuts the old way (one video seeks), for
   * measuring. P.cutLog: the last cuts, { how: "swap" | "seek", gapMs } (gapMs: from the last frame of the piece
   * before to the first frame of the next one shown; one frame at 60 fps is 16.7 ms). */
  let sb = null, sbAt = 0, cutWatch = null;
  P.gapless = true;
  P.cutLog = [];
  const swapFns = [];
  /** Be told when the shown video changes: fn(newVideo, oldVideo). Returns the unsubscribe. */
  P.onVideo = (fn) => { swapFns.push(fn); return () => { const i = swapFns.indexOf(fn); if (i >= 0) swapFns.splice(i, 1); }; };
  P.v = v;
  /** The jump after the piece being played: { at, s } (timeline second, source second), or null (the footage follows on,
   *  a gap, a missing clip, the end). */
  function nextJump(m) {
    const p = playPart(m);
    if (!p) return null;
    const at = afterPart(m, p);
    if (at === null || noVid(m, at)) return null;
    const s = emToSrc(m, at), end = p.src + p.len * (p.speed || 1);
    if ((!VS || emSourceAt(VS, s) === vk) && Math.abs(s - end) < 0.05) return null;
    return { at, s };
  }
  const fileOf = (s) => (VS ? emProjToFile(VS, s) : { k: -1, local: s });
  /** Put the standby at the next jump's first frame (a few times a second while playing; it does nothing when it holds it). */
  function prepStandby(force) {
    const m = pm();
    if (!P.gapless || !m || !P.editing || aft || vpend || dead) return;
    const t0 = performance.now();
    if (!force && t0 - sbAt < 150) return;
    sbAt = t0;
    const j = nextJump(m);
    if (!j) return;
    const { k, local } = fileOf(j.s);
    if (VS && (!VS[k] || VS[k].missing)) return;
    if (sb && sb.k === k && Math.abs(sb.local - local) < 1e-3) return;
    sb = { k, local, ready: false, t0 };
    const url = VS ? vUrl(VS[k]) : v.currentSrc || v.src;
    if (w.dataset.url !== url) { w.dataset.url = url; w.src = url; } // it seeks once its length is known
    else if (w.readyState >= 1) w.currentTime = local;
  }
  for (const x of vids) {
    x.addEventListener("loadedmetadata", () => { if (x === w && sb) w.currentTime = sb.local; });
    x.addEventListener("seeked", () => { if (x === w && sb && Math.abs(w.currentTime - sb.local) < 0.05) { sb.ready = true; sb.ms = Math.round(performance.now() - sb.t0); } });
  }
  /** At the cut to timeline second `at`: swap when the standby holds it. True if it did. */
  function swapTo(at, m) {
    if (!P.gapless || !sb || !sb.ready || dead || w.readyState < 2) return false;
    const { k, local } = fileOf(emToSrc(m, at));
    if (sb.k !== k || Math.abs(sb.local - local) > 0.02) return false;
    const run = playing(), old = v, t0 = cutWatch?.last ?? performance.now();
    v = w;
    w = old;
    P.v = v;
    if (VS) { vk = k; lateSeek = null; }
    aft = null;
    const p = emParts(m)[emPartAt(m, at)];
    playSrc = p ? p.i : null;
    sb = null;
    v.loop = false;
    v.classList.remove("e-standby");
    old.classList.add("e-standby");
    shownCss = shownExp = "\u0000"; // the filter and the gamma go onto the video now shown
    applyRate();
    applyColor();
    if (run) v.play().catch((e) => console.warn("Glint: play() was refused", e));
    old.pause();
    output(true); // the shown video's sound rule; the standby stays muted; the clip's tracks jump with it
    for (const fn of swapFns.slice()) { try { fn(v, old); } catch (e) { console.error("Glint: a video swap listener failed", e); } }
    cutWatch = { how: "swap", t0, el: v };
    state();
    prepStandby(true);
    return true;
  }
  /* The frame shown (requestVideoFrameCallback): the picture's look follows the FRAME on screen, not the clock, which
   * runs up to a frame ahead (the color changed a frame before the cut); a jump is taken when the piece's last frame
   * is on screen, so the next piece's first frame is the very next one. Without the callback: the old way (paint). */
  let shownFrame = null; // { el, src, at }: the source second of the frame on screen and when it came
  const frameStep = () => Math.max(frame, (shownFrame?.step || 0));
  function onShownFrame(x, meta) {
    const now = performance.now();
    const src = (VS && vk >= 0 ? VS[vk].start : 0) + meta.mediaTime;
    const step = shownFrame && shownFrame.el === x && meta.mediaTime > shownFrame.media ? Math.min(0.5, meta.mediaTime - shownFrame.media) : 0;
    shownFrame = { el: x, src, media: meta.mediaTime, at: now, step };
    if (cutWatch && cutWatch.el === x && cutWatch.first === undefined) {
      cutWatch.first = now;
      P.cutLog.push({ how: cutWatch.how, gapMs: Math.round(now - cutWatch.t0) });
      if (P.cutLog.length > 50) P.cutLog.shift();
    }
    if (cutWatch) cutWatch.last = now;
    else cutWatch = { last: now, first: 0 };
    const m = pm();
    const p = m && P.editing && !aft && !vpend && !x.paused ? playPart(m) : null;
    if (p) {
      const end = p.src + p.len * (p.speed || 1), st = frameStep();
      if (src + st * 0.5 >= end - 1e-4) cutNow(p); // the piece's footage is over (this frame is the next one's): on now
      // Its last frame is on screen: the jump goes with the next screen refresh after it (tick checks `due`).
      else if (src + st * 1.5 >= end - 1e-4 && nextJump(m)) pendingCut = { i: p.i, el: x, due: meta.expectedDisplayTime + (st / Math.max(0.0625, x.playbackRate)) * 1000 };
    }
    if (P.editing) applyColor(); // after a cut that follows on in the file, the frame belongs to the next piece
  }
  let pendingCut = null;
  /** From the animation loop: take a jump that is due (see onShownFrame). */
  function cutDue(ts) {
    const pc = pendingCut;
    if (!pc || ts < pc.due - 4) return;
    pendingCut = null;
    const m = pm();
    if (m && v === pc.el && playSrc === pc.i && !v.paused) { const p = playPart(m); if (p) cutNow(p); }
  }
  function cutNow(p) {
    const m = pm();
    if (!m || dead || aft || vpend) return;
    const at = afterPart(m, p);
    if (at === null) return; // the end: guard() stops or loops
    pendingCut = null;
    const jump = nextJump(m);
    if (swapTo(at, m)) return;
    if (jump) cutWatch = { how: "seek", t0: cutWatch?.last ?? performance.now(), el: v };
    jumpTo(at);
  }
  for (const x of vids) {
    if (typeof x.requestVideoFrameCallback !== "function") continue;
    const cb = (_, meta) => { if (dead) return; x.requestVideoFrameCallback(cb); if (x === v) onShownFrame(x, meta); };
    x.requestVideoFrameCallback(cb);
  }
  /** The playhead for the picture's look: the frame on screen while playing (see above), else the clock. */
  function lookNow() {
    const m = pm();
    if (!m || aft || vpend || v.paused || !shownFrame || shownFrame.el !== v || performance.now() - shownFrame.at > 200) return tlNow();
    const p = playPart(m), src = shownFrame.src;
    if (p) {
      const sp = p.speed || 1, end = p.src + p.len * sp;
      if (src >= p.src - 1e-4 && src < end - 1e-4) return p.at + (src - p.src) / sp;
      if (src < p.src) return p.at; // the frame from before a seek or a swap: the piece's start
    }
    return tlNow();
  }

  /** The playhead in timeline seconds. */
  function tlNow() {
    if (aft) return aftNow();
    const m = pm(), src = vNow();
    if (!m) return src;
    const p = playPart(m), sp = p?.speed || 1; // a piece with its own speed (0.1.41): its footage is len * speed long
    if (p && src >= p.src - 0.25 * sp && src <= p.src + p.len * sp + sp) return p.at + clamp((src - p.src) / sp, 0, p.len);
    const at = emFromSrc(m, src); // the video was put somewhere else: find its part again
    playSrc = emPartAt(m, at) >= 0 ? emPartAt(m, at) : null;
    return at;
  }
  /** Put the video at timeline second `sec`. */
  function setTl(sec) {
    const m = pm();
    // A gap on the main video row (0.1.36), or a missing clip of a project (0.1.38), has no video either: the
    // player's own clock, as after the video.
    if (m && noVid(m, sec)) { setAft(sec, playing()); return; }
    if (!m && VS && ed() && vMissing(sec)) { setAft(sec, playing()); return; }
    aft = null; // the video is the clock again
    if (!m) { vSet(sec); return; }
    const p = emParts(m)[emPartAt(m, sec)];
    playSrc = p ? p.i : null;
    vSet(emToSrc(m, sec));
  }
  /** Put the playhead at timeline second `sec` and play from there (`run`) or stand: the video, or the player's
   *  own clock in a gap or after the video (0.1.36). */
  function go(sec, run) {
    setTl(sec);
    if (aft) {
      aft = { at: aft.at, run: !!run, t0: performance.now() };
      aftTick();
      state();
      syncAudio(true);
      drift();
    } else if (vpend) vpend.play = !!run; // another source's file is loading: it plays (or not) once it has seeked
    else if (run && v.paused) v.play().catch((e) => console.warn("Glint: play() was refused", e));
    else if (!run && !v.paused) v.pause();
  }
  /** The part being played, if the video has reached its end (null without parts). */
  function pastPart(m) {
    const p = m.parts ? playPart(m) : null;
    return p && !vpend && vNow() >= p.src + p.len * (p.speed || 1) - EM_EPS ? p : null;
  }
  /** Where playback goes on after the end of part `p`: the next playable moment after it in display order, or null after the last. */
  const afterPart = (m, p) => (p.end >= m.duration - EM_EPS ? null : emPlayable(m, p.end));
  /** Go on at timeline second `at`; no seek when it simply follows in the source (the next part starts where the video is). */
  function jumpTo(at) {
    const m = pm();
    // (a project's next piece in another clip's file is never "simply following", even when project time touches)
    if (m && !noVid(m, at) && !vpend && (!VS || emSourceAt(VS, emToSrc(m, at)) === vk) && Math.abs(emToSrc(m, at) - vNow()) < 0.05) { playSrc = emPartAt(m, at) >= 0 ? emPartAt(m, at) : null; applyRate(); return; } // its own speed at once (0.1.41)
    if (m && !noVid(m, at) && swapTo(at, m)) return; // the standby holds it (0.1.41)
    setTl(at);
  }

  /* ----- Web Audio: one context per player, a GainNode per preview element ----- */
  P.audioCtx = () => {
    if (P.ctx === null) {
      const AC = window.AudioContext || window.webkitAudioContext;
      if (!AC) { console.warn("Glint: no Web Audio here; levels above 100% are heard at 100% in the preview."); P.ctx = false; }
      else {
        try { P.ctx = new AC(); }
        catch (e) { console.warn("Glint: could not create an AudioContext; levels above 100% are heard at 100% in the preview.", e); P.ctx = false; }
      }
    }
    P.resumeAudio();
    return P.ctx || null;
  };
  P.resumeAudio = () => {
    if (P.ctx && P.ctx.state === "suspended") P.ctx.resume().catch((e) => console.warn("Glint: could not resume audio (needs a click or key press)", e));
  };
  function route(s) {
    const ctx = P.audioCtx();
    if (!ctx) return;
    let node;
    try { node = ctx.createMediaElementSource(s.a); }
    catch (e) { console.warn(`Glint: ${s.title} plays without Web Audio; levels above 100% are heard at 100%.`, e); return; }
    const gain = ctx.createGain();
    gain.gain.value = 0;
    node.connect(gain);
    gain.connect(edLimiter(ctx)); // 0.1.45: every row through the one limiter: 200 % is louder, never clipped
    s.node = node;
    s.gain = gain;
  }

  /* ----- preview audio elements ----- */
  function newSrc(id, kind, path, title) {
    const s = { id, kind, path, title, a: null, node: null, gain: null, state: "loading", error: null };
    P.src.set(id, s);
    s.ready = loadSrc(s);
    return s;
  }
  async function loadSrc(s) {
    // First with CORS, so Web Audio may read it (needed for levels above 100%).
    // A source that refuses CORS is retried as a plain element (levels then stop at 100%).
    for (const cors of [true, false]) {
      const a = document.createElement("audio");
      a.preload = "auto";
      if (cors) a.crossOrigin = "anonymous";
      a.src = fileSrc(s.path);
      el(".p-audio").appendChild(a);
      s.a = a;
      try { await edMediaReady(a, s.title); }
      catch (e) {
        edKillMedia(a);
        if (s.a === a) s.a = null;
        if (dead || s.state === "dropped") return;
        s.error = e;
        console.warn(`Glint: could not load ${s.title} for the preview${cors ? " with CORS, trying without" : ""}`, e);
        continue;
      }
      if (dead || s.state === "dropped") { edKillMedia(a); s.a = null; return; }
      if (cors) route(s);
      else console.warn(`Glint: ${s.title} plays without Web Audio; levels above 100% are heard at 100%.`);
      a.defaultPlaybackRate = a.playbackRate = rateOf(s);
      s.state = "on";
      return;
    }
    s.state = "failed";
  }
  function dropSrc(s) {
    s.state = "dropped";
    s.node?.disconnect();
    s.gain?.disconnect();
    s.node = s.gain = null;
    if (s.a) { const a = s.a; s.a = null; edKillMedia(a); }
  }
  /* A project that references its clips (0.1.41, F2): the preview sound of each row is made source by source, as soon
   * as that source's tracks are known, nearest the playhead first. While a source's own elements are not all ready
   * (or one failed), the video plays its own sound there (srcSoundOn). */
  const srcEls = (k) => [...P.src.values()].filter((e) => e.kind === "track" && e.srcK === k);
  const srcSoundOn = (k) => { const L = srcEls(k); return L.length > 0 && L.every((e) => e.state === "on"); };
  P.srcSoundOn = srcSoundOn;
  /** The track of source `s` that row `r` plays (0.1.44): by role (its stream), or by title; `stream` overrides. */
  function srcTrackOf(s, r, stream) {
    const all = s.tracksAll || s.tracks || [];
    if (Number.isInteger(stream)) return all.find((x) => +x.index === stream) || null;
    if (r.role) { const st = emRowStream(s, r); return st >= 0 ? all.find((x) => +x.index === st) || null : null; }
    return (s.tracks || []).find((x) => (x.title || `Track ${x.index + 1}`) === r.title) || null;
  }
  /** The streams pieces of source `k` play instead on row `r` (part.over, "only the mix was heard there"). */
  function overStreams(m, k, r) {
    const out = new Set();
    for (const p of emParts(m)) { const o = m.parts[p.i]?.over; if (!p.gap && o && Number.isInteger(o[r.title]) && emSourceAt(VS, p.src + 1e-6) === k) out.add(o[r.title]); }
    return [...out];
  }
  function syncTrackEls() {
    if (!VS || !P.model || dead) return;
    const rows = P.model.audio.filter((r) => r.kind === "track");
    for (const s of byNearness(VS)) {
      if (s.missing || s.trackFail || !Array.isArray(s.tracks)) continue;
      for (const r of rows) for (const ov of [null, ...overStreams(P.model, s.k, r)]) {
        const tr = srcTrackOf(s, r, ov), id = ov === null ? `${r.id}@${s.k}` : `${r.id}@${s.k}#${ov}`;
        if (!tr || P.src.has(id)) continue;
        if (!tr.path) { failSource(s, new Error(`the audio tracks of ${s.title} were not extracted`)); break; }
        const e = newSrc(id, "track", tr.path, r.title);
        Object.assign(e, { rowId: r.id, srcK: s.k, over: ov }); // over: the stream of a piece's own choice (0.1.44)
        if (P.trackState === "idle" || P.trackState === "none") P.trackState = "loading";
        e.ready.then(() => {
          if (dead || P.src.get(id) !== e) return;
          if (e.state !== "on") return failSource(s, e.error);
          if ([...P.src.values()].some((x) => x.kind === "track" && x.state === "on")) P.trackState = "on";
          applyRate();
          output(true);
          P.ed?.sync("audio");
          tl((h) => h.refresh());
        });
      }
    }
  }
  /** A source whose track sound cannot be previewed: its own elements go, the video's sound plays there. */
  function failSource(s, err) {
    if (s.trackFail) return;
    s.trackFail = true;
    console.warn("Glint: a project's clip plays the video's own sound in the preview; its separate tracks did not load.", s.title, err);
    for (const e of srcEls(s.k)) { dropSrc(e); P.src.delete(e.id); }
    output(true);
  }
  /** Loads the clip's own tracks once. Falls back to the video's own audio if any of them fails. */
  function loadTracks() {
    if (VS) return syncTrackEls(); // 0.1.41: a project's clips, source by source as their tracks come
    if (P.trackState !== "idle" || !P.model) return;
    if (singlePending) return; // 0.1.41: once the tracks are known (singleModel); the video's sound meanwhile
    const rows = P.model.audio.filter((r) => r.kind === "track");
    if (!rows.length) { P.trackState = "none"; return; }
    P.trackState = "loading";
    // A newer load (clips added meanwhile, P.addSources) makes this one stale: it then leaves everything alone
    // (0.1.41: it threw over its dropped elements and removed the new ones too, for good).
    const gen = ++trackGen;
    const stale = () => dead || gen !== trackGen;
    (async () => {
      let list;
      if (VS) {
        // A project that references its clips (0.1.38): one element per row and clip that has a track of that title;
        // it is heard only while its clip is the one in the video (syncAudio).
        list = [];
        for (const s of VS) {
          if (s.missing) continue;
          for (const r of rows) {
            const tr = (s.tracks || []).find((x) => (x.title || `Track ${x.index + 1}`) === r.title);
            if (!tr) continue;
            if (!tr.path) throw new Error(`the audio tracks of ${s.title} were not extracted`);
            const e = newSrc(`${r.id}@${s.k}`, "track", tr.path, r.title);
            Object.assign(e, { rowId: r.id, srcK: s.k });
            list.push(e);
          }
        }
      } else {
        if (rows.some((r) => !r.path)) throw new Error("the clip's audio tracks were not extracted");
        list = rows.map((r) => newSrc(r.id, "track", r.path, r.title));
        // A piece that plays another stream on a row (part.over, 0.1.44): an element of that stream too; syncAudio lets
        // each be heard only in its own pieces.
        const all = P.model.tracksAll || [];
        for (const r of rows) {
          const st = new Set((P.model.parts || []).map((p) => p?.over?.[r.title]).filter(Number.isInteger));
          for (const x of st) {
            const tr = all.find((y) => +y.index === x);
            if (!tr || !tr.path) continue;
            const e = newSrc(`${r.id}#${x}`, "track", tr.path, r.title);
            Object.assign(e, { rowId: r.id, over: x });
            list.push(e);
          }
        }
      }
      await Promise.all(list.map((s) => s.ready));
      if (stale()) return;
      const bad = list.find((s) => s.state !== "on");
      if (bad) throw bad.error || new Error(`${bad.title} did not load`);
      P.trackState = "on";
      applyRate();
      output(true);
      P.ed?.sync("audio");
    })().catch((e) => {
      if (stale()) return;
      console.warn("Glint: separate audio track preview failed, using the clip's own audio instead.", e);
      for (const [id, s] of [...P.src]) if (s.kind === "track") { dropSrc(s); P.src.delete(id); }
      P.trackState = "failed";
      output(true);
      P.ed?.sync("audio");
    });
  }
  /** Gives every file row of the model an audio element, and drops elements whose row is gone (undo). */
  /** An audio clip that is a clip's own track (0.1.39: ungrouped, `clip` set): the per-track file the editor already
   *  loads for that track (an <audio> on the video file would play its first stream only). Else its own path. */
  function clipPreviewPath(row) {
    if (row.clip === undefined || !Number.isFinite(row.stream)) return row.path;
    if (VS) { const s = VS.find((x) => x.clip === row.clip); const tr = s && (s.tracks || []).find((x) => x.index === row.stream); return tr?.path || row.path; }
    const tr = P.model?.audio.find((r) => r.kind === "track" && r.index === row.stream);
    return tr?.path || row.path;
  }
  /** Its waveform from the track row's, on the file's own time (a project's track rows are on project time). */
  function clipTrackWave(row) {
    if (row.clip === undefined || !Number.isFinite(row.stream) || !P.model) return null;
    const s = VS ? VS.find((x) => x.clip === row.clip) : null, title = s ? (s.tracks || []).find((x) => x.index === row.stream)?.title : null;
    const tr = P.model.audio.find((r) => r.kind === "track" && (s ? r.title === title : r.index === row.stream));
    const w = tr && P.waves.get(tr.id);
    if (!w || !(w.perSecond > 0)) return null;
    if (!s) return w;
    const a = Math.floor(s.start * w.perSecond), b = Math.ceil((s.start + s.duration) * w.perSecond);
    return { peaks: w.peaks.slice(a, b), perSecond: w.perSecond };
  }
  function syncRows() {
    const m = P.model;
    if (!m) return;
    const files = new Map(m.audio.filter((r) => r.kind === "file").map((r) => [r.id, r]));
    for (const [id, s] of [...P.src]) if (s.kind === "file" && !files.has(id)) { dropSrc(s); P.src.delete(id); }
    for (const [id, row] of files) {
      if (P.src.has(id)) continue;
      const s = newSrc(id, "file", clipPreviewPath(row), row.title);
      s.ready.then(() => {
        if (dead || P.src.get(id) !== s) return;
        if (s.state === "failed") flash(t("{name} cannot be played in the preview. The export still includes it.", { name: s.title }), true);
        applyRate();
        output(true);
        P.ed?.sync("audio");
      });
      const wave = P.waves.get(id) || clipTrackWave(row); // a row brought back by redo; an ungrouped sound (0.1.39)
      if (wave) { P.waves.set(id, wave); tl((h) => h.setWaveform(id, wave)); }
      else {
        // A file row that came back with a saved project has no waveform: read the file's loudness again.
        invoke("editor_audio_info", { path: row.path }).then((info) => {
          if (dead || P.waves.has(id) || !P.model?.audio.some((r) => r.id === id)) return;
          const w = { peaks: Array.isArray(info?.peaks) ? info.peaks : [], perSecond: +info?.per_second || 0 };
          P.waves.set(id, w);
          tl((h) => h.setWaveform(id, w));
        }).catch((e) => { if (!dead) console.warn("Glint: no waveform for the added audio file", row.path, e); });
      }
    }
  }

  /* ----- audio output: the video's own audio, or the preview elements while editing ----- */
  const heard = (s) => P.editing && (s.kind === "file" || P.trackState === "on" || (!!VS && s.srcK !== undefined && s.state === "on"));
  /** Sets each element's level from the model for the current moment. */
  function levels(now) {
    const m = ed(), master = (P.muted ? 0 : P.vol) * (P.ov ? P.ov.fadeGain(now) : 1); // the fade in / out is heard too
    for (const s of P.src.values()) {
      if (s.state !== "on" || !s.a) continue;
      const row = m && heard(s) ? m.audio.find((r) => r.id === (s.rowId || s.id)) : null;
      const want = row ? emGainAt(m, row, now) * master : 0;
      if (s.gain) { if (Math.abs(s.gain.gain.value - want) > 0.001) s.gain.gain.value = want; }
      else { const vol = clamp(want, 0, 1); if (Math.abs(s.a.volume - vol) > 0.001) s.a.volume = vol; }
    }
  }
  /** Keeps each element at its place for the video's time: tracks at the same (source) time, file rows inside their span (timeline time). */
  function syncAudio(hard) {
    // After the video (0.1.35) the player's own clock runs: the clip's tracks are silent there, file rows go on.
    const m = ed(), src = v.currentTime, now = tlNow(), run = aft ? aft.run : !v.paused && !v.ended && v.readyState >= 3;
    for (const s of P.src.values()) {
      const a = s.a;
      if (s.state !== "on" || !a) continue;
      const row = m && heard(s) ? m.audio.find((r) => r.id === (s.rowId || s.id)) : null;
      let want = null;
      // A row cut with the video is silent in a gap (0.1.36), as the clip's own tracks are.
      // A project's clip track (0.1.38) plays only while its own clip is the one in the video, at that file's time.
      if (row) want = s.kind === "file" ? (emRowCovers(row, now, m) ? emRowFileAt(m, row, now) : null) /* an added file is heard wherever its block lies (0.1.38) */ : aft ? null : s.srcK !== undefined ? (s.srcK === vk && !vpend && pieceStreamIs(m, row, s.over) ? v.currentTime : null) : pieceStreamIs(m, row, s.over) ? src : null; // a file that plays through cuts keeps its place in the file across a jump
      const end = Number.isFinite(a.duration) ? a.duration : Infinity;
      if (want === null || want >= end - 0.01) { if (!a.paused) a.pause(); continue; }
      const rate = rateOf(s); // 0.1.41: a track at the piece's speed, an added file at the timeline's
      if (a.playbackRate !== rate) a.playbackRate = rate;
      if (hard || (run && a.paused) || (!a.seeking && Math.abs(a.currentTime - want) > ED_DRIFT)) a.currentTime = want;
      if (run && a.paused) a.play().catch((e) => console.warn(`Glint: could not play ${s.title} in the preview`, e));
      else if (!run && !a.paused) a.pause();
    }
    levels(now);
  }
  /** True if the piece being played plays stream choice `over` on `row` (null: the row's own stream, 0.1.44). */
  function pieceStreamIs(m, row, over) {
    const o = m && m.parts && playSrc !== null ? m.parts[playSrc]?.over : null;
    const want = o && Number.isInteger(o[row.title]) ? o[row.title] : null;
    return (over ?? null) === want;
  }
  function drift() {
    const want = P.editing && P.src.size > 0 && playing() && !dead;
    if (want && !driftTimer) driftTimer = setInterval(() => syncAudio(false), 300);
    else if (!want && driftTimer) { clearInterval(driftTimer); driftTimer = 0; }
  }
  function output(hard = false) {
    // A project's clip (0.1.41): the video is quiet only where that clip's own track sound is ready.
    v.muted = (P.editing && (VS ? srcSoundOn(vk) : P.trackState === "on")) || P.muted;
    v.volume = P.vol;
    w.muted = true; // the standby is never heard (0.1.41)
    syncAudio(hard);
    drift();
  }
  P.output = output;

  /* ----- speed and color preview ----- */
  // Speed per piece (0.1.41): the timeline runs at the player's speed times the result's (tlRate); the video and the
  // clip's own tracks run at that times the speed of the piece being played (vidRate); added audio files and songs are
  // on timeline time (tlRate). The browser keeps the pitch (preservesPitch, on by default).
  const tlRate = () => clamp(P.viewSpeed * (ed() ? P.model.speed || 1 : 1), 0.0625, 16);
  const pieceSp = () => { const m = pm(); return (m && !aft && playPart(m)?.speed) || 1; };
  const vidRate = () => clamp(tlRate() * pieceSp(), 0.0625, 16);
  const rateOf = (s) => (s.kind === "file" ? tlRate() : vidRate());
  P.tlRate = tlRate;
  function applyRate() {
    aftRebase();
    const r = vidRate();
    if (v.defaultPlaybackRate !== r) v.defaultPlaybackRate = r;
    if (v.playbackRate !== r) v.playbackRate = r;
    for (const s of P.src.values()) if (s.a) { const x = rateOf(s); if (s.a.playbackRate !== x) { s.a.defaultPlaybackRate = x; s.a.playbackRate = x; } }
  }
  // Color and hiding are per segment, so the preview follows the playhead:
  // paint() calls this every frame while playing and after every seek or edit.
  // The DOM is only touched when something changed.
  let shownCss = "", shownExp = "", shownBlank = false, shownSeg = -1;
  function applyColor() {
    const m = ed(), now = lookNow(); // the frame on screen while playing (0.1.41), else the clock
    const col = m ? emColorAt(m, now) : ED_NO_COLOR;
    // Glow (0.1.45): its blur is measured in the shown picture's pixels (sigma = 1.6 % of its height).
    const picH = edCol(col, "glow") > 0.004 ? shownPicH() : 0;
    const css = edFilterCss(col, picH), exp = String(Math.round((1 / (col.gamma || 1)) * 10000) / 10000);
    if (css !== shownCss) { shownCss = css; v.style.filter = css; }
    if (exp !== shownExp) {
      shownExp = exp;
      for (const f of $$("#e-gamma feFuncR, #e-gamma feFuncG, #e-gamma feFuncB", root)) f.setAttribute("exponent", exp);
    }
    const blank = !!aft || (!!m && emBlankAt(m, now)); // black picture, the audio plays on (also after the video, 0.1.35)
    if (blank !== shownBlank) { shownBlank = blank; box.classList.toggle("e-blank", blank); }
    if (box.classList.contains("e-after") !== !!aft) box.classList.toggle("e-after", !!aft);
    // A project's clip that is missing (0.1.38): black, and says which.
    const gone = VS && aft && m && now < m.duration - EM_EPS && emGapAt(m, now) < 0 ? VS[emSourceAt(VS, emToSrc(m, now))] : null;
    const goneText = gone?.missing ? t("Clip missing: {title}", { title: gone.title || "?" }) : "";
    if (VS && missTag.textContent !== goneText) { missTag.textContent = goneText; missTag.hidden = !goneText; }
    const seg = m ? emSegmentAt(m, now) : -1;
    if (seg !== shownSeg) { shownSeg = seg; P.ed?.playheadSeg?.(); } // the Color pane may show the playhead's segment
    P.cv?.paint(now); // the canvas preview (canvas.js)
    P.zm?.paint(now); // the Zoom section follows the playhead (zoom.js, 0.1.41)
    P.ov?.paint(now); // overlays shown at the playhead, the fade cover
  }
  P.applyColor = applyColor;

  /* ----- the seek bar: the source clip, or the finished clip while editing ----- */
  /** The height in CSS px of the picture as shown (the video fitted in its box; the canvas preview's when it is on). */
  function shownPicH() {
    const view = box.querySelector(".cv-view");
    if (view && !view.hidden && view.clientHeight) return view.clientHeight;
    const w = v.clientWidth, h = v.clientHeight, vw = v.videoWidth, vh = v.videoHeight;
    return vw > 0 && vh > 0 && w > 0 ? Math.min(h, (w * vh) / vw) : h || 720;
  }
  // The finished clip runs on after the video up to emEnd (0.1.35); without anything after the video that is emKeptDuration.
  const barLen = () => { const m = ed(); return m ? emSrcToOut(m, emEnd(m)) || 1 : dur(); };
  const toBar = (sec) => { const m = ed(); return m ? emSrcToOut(m, sec) : sec; };
  const fromBar = (x) => { const m = ed(); return m ? emOutToSrc(m, x) : x; };
  const barSpeed = () => (ed() ? P.model.speed || 1 : 1);
  const pct = (x) => `${clamp((x / barLen()) * 100, 0, 100)}%`;

  /** While editing and playing: skip cut-out parts, go on with the next part (display order) at the end of
   *  each part, stop (or loop) after the last kept part. `now` is timeline time. */
  function guard(now) {
    const m = ed();
    if (!m || drag) return;
    if (aft) {
      if (!aft.run) return;
      // In a gap on the main video row (0.1.36): when it ends, on with what follows (a clip from its start).
      if (now < m.duration - EM_EPS) {
        const p = emPlayable(m, now);
        if (p !== null && noVid(m, now) && Math.abs(p - now) < 1e-3) return; // still in the gap (or a missing clip)
        if (p !== null) { go(p, true); return; }
      } else if (now < emEnd(m)) return; // after the video (0.1.35), before the result's end
      // The result's end: stop there or loop back to the start.
      if (P.loop) go(emPlayable(m, 0) ?? 0, true);
      else { aft = { at: Math.min(now, Math.max(m.duration, emEnd(m))), run: false, t0: performance.now() }; state(); syncAudio(false); drift(); }
      return;
    }
    if (v.paused || v.seeking) return;
    const past = pastPart(m);
    const p = past ? afterPart(m, past) : emPlayable(m, now);
    if (p === null) {
      const kept = emKept(m);
      if (!kept.length) return;
      // Something goes on after the video: playing runs on into it on the player's own clock.
      if (emTail(m) > EM_EPS) { setAft(m.duration, true); state(); syncAudio(true); drift(); return; }
      if (P.loop) setTl(kept[0].start);
      else { v.pause(); const last = kept[kept.length - 1]; setTl(Math.max(last.start, last.end - frame)); }
    } else if (past) {
      // The frames drive the cut while they come (onShownFrame, 0.1.41): the clock runs ahead of the frame on screen,
      // so going on here would cut the piece's last frame short. Only when they are late (a hidden window) is it ours.
      const fresh = shownFrame && shownFrame.el === v && performance.now() - shownFrame.at < 200;
      if (!fresh || vNow() >= past.src + past.len * (past.speed || 1) + 3 * frameStep()) jumpTo(p);
    } else if (p > now + 0.001) setTl(p);
  }

  /* ----- drawing ----- */
  function paint() {
    if (dead) return;
    guard(tlNow());
    applyRate(); // the piece being played may have another speed (0.1.41); only touches what differs
    if (playing()) prepStandby(false); // the next jump's first frame waits in the standby (0.1.41)
    const now = tlNow(), m = ed();
    el(".p-prog").style.width = pct(toBar(now));
    el(".p-knob").style.left = pct(toBar(now));
    el(".p-clock").textContent = m
      ? `${fmtClock(emSrcToOut(m, Math.min(now, emEnd(m))) / barSpeed())} / ${fmtClock(emSrcToOut(m, emEnd(m)) / barSpeed())}`
      : `${fmtClock(now)} / ${fmtClock(dur())}`;
    if (v.buffered.length) { const b = v.buffered.end(v.buffered.length - 1); el(".p-buf").style.width = pct(toBar(pm() ? emFromSrc(m, VS && vk >= 0 ? VS[vk].start + b : b) : b)); }
    if (P.editing) levels(now);
    applyColor();
  }
  const tick = (ts) => {
    if (Number.isFinite(ts)) cutDue(ts); // a jump due with this screen refresh (0.1.41)
    if (P.editing) syncAudio(false);
    paint();
    raf = !playing() || dead ? 0 : requestAnimationFrame(tick);
  };
  const wake = () => {
    box.classList.remove("idle");
    clearTimeout(idle);
    idle = setTimeout(() => { if (playing() && !drag && !edPopClose) box.classList.add("idle"); }, 2200);
  };
  const volUI = () => {
    el('[data-p="mute"]').innerHTML = P.muted || P.vol === 0 ? PICON.mute : PICON.vol;
    el(".p-vol").value = P.muted ? 0 : P.vol;
    // The part of the bar left of the knob is drawn in the theme color (see .p-vol in editor.css).
    el(".p-vol").style.setProperty("--fill", `${Math.round((P.muted ? 0 : P.vol) * 100)}%`);
  };
  const state = () => {
    if (dead) return;
    el('[data-p="play"]').innerHTML = playing() ? PICON.pause : PICON.play;
    el(".p-big").hidden = playing() || (!aft && v.currentTime === 0);
    el('[data-p="loop"]').classList.toggle("on", P.loop);
    el('[data-p="speed"]').textContent = `${P.viewSpeed}x`;
    volUI();
    if (playing() && !raf) tick();
    wake();
  };
  const setVol = (x, unmute = true) => {
    P.vol = clamp(x, 0, 1);
    if (unmute) P.muted = false;
    try { localStorage.setItem("glint.vol", P.vol); } catch (e) { console.warn("Glint: could not save the volume", e); }
    output();
    volUI();
  };
  const setMuted = (on) => { P.muted = on; output(); volUI(); };
  P.setLoop = (on) => { P.loop = !!on; v.loop = P.loop && !P.editing; state(); };

  P.toggle = () => {
    P.resumeAudio();
    // After the video (0.1.35): the player's own clock pauses and plays.
    const aftGo = () => { aft = { at: aft.at, run: true, t0: performance.now() }; aftTick(); state(); syncAudio(true); drift(); };
    if (aft && aft.run) { aftStop(); state(); syncAudio(false); drift(); paint(); return; }
    // Another clip's file is loading (0.1.41): it plays or not once it has seeked; Space just says which.
    if (vpend) { vpend.play = !vpend.play; state(); return; }
    if (!v.paused) { v.pause(); return; }
    const m = ed();
    if (m && aft) {
      const E = emEnd(m);
      if (aft.at > E + 1e-3) return; // past the result's end there is nothing to play
      if (aft.at < E - Math.max(frame * 2, 0.05)) return aftGo();
      return go(emPlayable(m, 0) ?? 0, true); // at the end: play starts over (a gap at the start too)
    } else if (m) {
      const now = tlNow(), past = pastPart(m);
      const p = past ? afterPart(m, past) : emPlayable(m, now);
      // At the end, play starts over. Playback stops one frame before the end (guard), so "at the end" is
      // the last two frames of the last kept part, not only past it. With something after the video, play goes on into it.
      const kept = emKept(m), end = kept.length ? kept[kept.length - 1].end : 0, tail = emTail(m) > EM_EPS;
      if (p === null && tail) { setAft(m.duration, false); return aftGo(); }
      if (p === null || (!past && !tail && now >= end - Math.max(frame * 2, 0.05))) return go(emPlayable(m, 0) ?? 0, true);
      if (past || p > now + 0.001) return go(p, true);
    } else if (v.ended) v.currentTime = 0;
    v.play().catch((e) => console.warn("Glint: play() was refused", e));
  };
  /** Stand still wherever the playhead is: the video, a file still loading, the player's own clock (0.1.41). */
  P.stop = () => {
    if (aft && aft.run) { aftStop(); aftTick(); }
    if (vpend) vpend.play = false;
    if (!v.paused) v.pause();
    state();
    if (P.editing) { syncAudio(false); drift(); }
  };
  /** Seek to `sec` (timeline time while editing; source time otherwise, the same without parts). While editing it never lands inside a cut-out part (see edLand), unless `raw`. */
  P.seek = (sec, back = false, raw = false) => {
    const m = ed(), run = playing(), wasAft = !!aft;
    // After the video (0.1.35): past m.duration, or forward past the last kept part when something goes on after
    // the video, the playhead goes onto the player's own clock (it may stand past the result's end, up to the end
    // of the timeline's free space; it plays on only before the end).
    if (m && Number.isFinite(sec) && (sec > m.duration + EM_EPS || (!back && !raw && emTail(m) > EM_EPS && emPlayable(m, clamp(sec, 0, m.duration)) === null))) {
      const E = emEnd(m), at = clamp(sec, m.duration, E + Math.max(10, E / 3));
      setAft(at, run && at < E - 1e-3);
      state();
      syncAudio(true);
      drift();
      paint();
      wake();
      return;
    }
    let at = clamp(sec, 0, m ? m.duration : dur()); // the row with its gaps (0.1.36)
    if (m && !raw) at = edLand(m, at, back, frame);
    if (m) go(at, run); // it plays on (or stands) wherever it lands: a clip, or a gap on the player's own clock
    else setTl(at);
    if (wasAft && !aft) state();
    paint();
    wake();
  };
  /** 0.1.46: show timeline second `sec` while a trim handle is dragged: the playhead goes there and the video stands.
   *  At most one seek is in flight; a newer position waits for it to land, and the last one always wins. */
  let peekWant = null, peekBusy = false;
  P.peek = (sec) => {
    if (!Number.isFinite(sec)) return;
    peekWant = sec;
    if (!peekBusy) peekRun();
  };
  function peekRun() {
    const m = ed();
    if (dead || peekWant === null || !m) { peekBusy = false; peekWant = null; return; }
    const at = clamp(peekWant, 0, m.duration);
    peekWant = null;
    peekBusy = true;
    go(at, false);
    paint();
    const el = v, done = () => { peekBusy = false; if (peekWant !== null) peekRun(); };
    if (aft || vpend || !el.seeking) { setTimeout(done, 16); return; } // (not requestAnimationFrame: it waits while the window is hidden)
    let to = 0;
    const landed = () => { clearTimeout(to); el.removeEventListener("seeked", landed); done(); };
    el.addEventListener("seeked", landed);
    to = setTimeout(landed, 400); // a seek that never reports back does not hold the next one forever
  }
  /** The playhead: timeline time while editing (see tlNow). */
  P.time = () => tlNow();
  const leaveFull = () => (document.fullscreenElement ? document.exitFullscreen().catch((e) => console.warn("Glint: exitFullscreen failed", e)) : Promise.resolve());
  /** Edit mode: the whole editor (video, timeline, side column) fullscreen, or back. */
  P.editorFull = () => (document.fullscreenElement ? leaveFull() : root.requestFullscreen().catch((e) => console.warn("Glint: fullscreen refused", e)));
  P.isEditorFull = () => document.fullscreenElement === root;
  // Outside edit mode the video alone goes fullscreen; in edit mode that would hide the timeline.
  // The fullscreen element is #modal, not the player (0.1.25): the previous / next arrows live in #modal, so they
  // are drawn in fullscreen too, and the next clip opens without leaving fullscreen (openClip only replaces
  // what is inside #modal). editor.css lays it out by `#modal.fs`.
  P.fullscreen = () => (P.editing ? P.editorFull() : document.fullscreenElement ? leaveFull() : document.getElementById("modal").requestFullscreen().catch((e) => console.warn("Glint: fullscreen refused", e)));
  /** Show the controls again (the pointer moved over something that is not the player, such as an arrow). */
  P.wake = () => wake();
  // app.js's prompt (#confirm) and messages (#flash) live outside the viewer, and
  // only the fullscreen element is drawn in fullscreen: move them into it meanwhile.
  function adoptOverlays(on) {
    const home = document.getElementById("modal");
    for (const id of ["confirm", "flash"]) { // back in index.html's order: #modal, #confirm, #flash
      const node = document.getElementById(id);
      if (!node) continue;
      if (on && node.parentNode !== root) root.appendChild(node);
      else if (!on && node.parentNode === root) {
        const before = id === "flash" ? document.getElementById("confirm") || home : home;
        if (before && before.parentNode !== root) before.after(node); else document.body.appendChild(node);
      }
    }
  }
  function onFullscreen() {
    if (dead) return;
    const on = P.isEditorFull();
    root.classList.toggle("e-full", on);
    adoptOverlays(on);
    P.ed?.fullChanged?.(on);
    tl((h) => h.refresh()); // the timeline's width may have changed
  }
  document.addEventListener("fullscreenchange", onFullscreen);

  /* ----- edit mode ----- */
  /** Resolves once the video's length is known (or failed to load). */
  const metadata = () => (v.readyState >= 1 ? Promise.resolve() : new Promise((ok) => {
    const done = () => { clearTimeout(timer); v.removeEventListener("loadedmetadata", done); v.removeEventListener("error", done); ok(); };
    const timer = setTimeout(done, 5000);
    v.addEventListener("loadedmetadata", done);
    v.addEventListener("error", done);
  }));
  /** The clip's edit model, made once per open clip. */
  function ensureModel() {
    if (P.model) return Promise.resolve(P.model);
    modelJob ||= (async () => {
      await metadata();
      if (VS) return virtualModel(); // a project that references its clips (0.1.38)
      if (!P.draft) return singleModel(); // a library clip: at once, its tracks after (0.1.41, F2)
      let tracks;
      try {
        tracks = await invoke("editor_audio_tracks", { id: c.id });
        if (!Array.isArray(tracks)) throw new Error("editor_audio_tracks did not return a list");
      } catch (e) {
        console.warn("Glint: editor_audio_tracks failed; the rows can still be edited, the preview uses the clip's own audio.", e);
        tracks = edFallbackTracks(c);
      }
      if (dead) return null;
      const m = emNew(dur(), tracks);
      m.tracksAll = tracks.map((x) => ({ index: x.index, title: x.title, path: x.path || "" })); // "Simplify the audio rows" (0.1.44)
      m.clipId = c.id;
      m.clipGame = c.game || "";
      m.clipTitle = c.title || ""; // a piece of a single clip is named after it (0.1.37)
      m.clipPath = c.path || "";   // the file an ungrouped piece's sound comes from (0.1.39; a draft: its file)
      // A project's draft: its saved edit comes back (projects.js); false = nothing usable saved.
      let restored = false;
      if (P.draft && typeof projRestore === "function") {
        try { restored = await projRestore(c, m); } catch (e) { console.warn("Glint: the project's saved edit could not be restored; it opens as it is.", e); restored = false; }
        if (dead) return null;
      }
      if (!restored) {
        emSetMarks(m, edMarksLoad(c.id)); // drops any past the clip's end
        // A joined clip or a project: each joined clip is its own part (block) on the timeline, in the order selected.
        if (c.kind === "joined" && Array.isArray(c.joins) && c.joins.length) {
          const starts = [0, ...c.joins], names = Array.isArray(c.join_names) ? c.join_names : [];
          emSetParts(m, starts, starts.map((_, i) => (typeof names[i] === "string" && names[i].trim()) || t("Clip {n}", { n: i + 1 })));
        }
      }
      P.model = m;
      if (P.draft && typeof projModelReady === "function") projModelReady(P, restored);
      return P.model;
    })();
    return modelJob;
  }
  P.whenModel = ensureModel;
  /**
   * A library clip's model at once (0.1.41, F2): its pieces and picture first; its own track rows when
   * editor_audio_tracks answers (the rows emNew would have made, added with full levels, into the undo history too).
   * Meanwhile the video's own sound plays. A joined clip's parts and the bookmarks are there from the start.
   */
  let singlePending = false;
  function singleModel() {
    const m = emNew(dur(), []);
    m.clipTitle = c.title || "";
    m.clipPath = c.path || "";
    m.clipId = c.id;
    m.rowsMode = "roles"; // a clip opened for the first time: rows by role (0.1.44)
    emSetMarks(m, edMarksLoad(c.id));
    if (c.kind === "joined" && Array.isArray(c.joins) && c.joins.length) {
      const starts = [0, ...c.joins], names = Array.isArray(c.join_names) ? c.join_names : [];
      emSetParts(m, starts, starts.map((_, i) => (typeof names[i] === "string" && names[i].trim()) || t("Clip {n}", { n: i + 1 })));
    }
    P.model = m;
    singlePending = true;
    const gen = loadGen, stale = () => dead || gen !== loadGen || P.model !== m;
    invoke("editor_audio_tracks", { id: c.id }).then((tracks) => {
      if (!Array.isArray(tracks)) throw new Error("editor_audio_tracks did not return a list");
      return tracks;
    }).catch((e) => {
      console.warn("Glint: editor_audio_tracks failed; the rows can still be edited, the preview uses the clip's own audio.", e);
      return edFallbackTracks(c);
    }).then((tracks) => {
      if (stale()) return;
      if (typeof emSetKnownGames === "function") emSetKnownGames([...new Set((S.clips || []).map((x) => x.game).filter(Boolean))]);
      const shape = emNewRoles(dur(), tracks, c.game || ""), n = m.removed.length; // rows by role (0.1.44); a Glint clip: emNew's rows
      m.tracksAll = shape.tracksAll;
      m.clipGame = shape.clipGame;
      const ids = new Set(m.audio.map((r) => r.id));
      const rows = shape.audio.filter((r) => !ids.has(r.id)).map((r) => ({ ...r, gains: Array(n).fill(1) }));
      m.audio = [...m.audio.filter((r) => r.kind === "track"), ...rows, ...m.audio.filter((r) => r.kind !== "track")];
      m.tracks0 = shape.tracks0;
      // The undo history learns the rows too (an undo must not take them away).
      const fix = (json) => { try { const o = JSON.parse(json); if (!Array.isArray(o.audio) || !Array.isArray(o.removed)) return json; const have = new Set(o.audio.map((r) => r.id)); o.audio = [...rows.filter((r) => !have.has(r.id)).map((r) => ({ ...r, gains: Array(o.removed.length).fill(1) })), ...o.audio]; return JSON.stringify(o); } catch { return json; } };
      m.undo = m.undo.map(fix);
      m.redo = m.redo.map(fix);
      singlePending = false;
      P.trackState = "idle";
      loadTracks();
      tl((h) => h.refresh());
      for (const [id, wave] of P.waves) if (rows.some((r) => r.id === id)) tl((h) => h.setWaveform(id, wave)); // waveforms that came first find their rows now
      P.edSync("tracks");
      checkGameApart(); // the waveforms may have come first
    });
    return P.model;
  }
  /** Each source's own tracks (editor_audio_tracks of its library clip), as the editor shows them. */
  async function sourceTracks(list) {
    await Promise.all(list.map(async (s) => {
      if (s.missing) { s.tracks = []; return; }
      try {
        const tr = await invoke("editor_audio_tracks", { id: s.clip });
        if (!Array.isArray(tr)) throw new Error("editor_audio_tracks did not return a list");
        s.tracksAll = tr; // every track, the mix included: rows by role (0.1.44) choose from these
        s.tracks = emShownTracks(tr);
      } catch (e) {
        console.warn("Glint: editor_audio_tracks failed for a project's clip; its rows play the video's own sound.", s.title, e);
        s.tracksAll = edFallbackTracks(s);
        s.tracks = emShownTracks(s.tracksAll);
      }
    }));
  }
  /** A piece put on the picture: in a project that references its clips (0.1.38) the item shows that clip's own
   *  file, from the second in it (and its own length, size and name), not project time. */
  function pieceFile(info) {
    if (!VS || !info) return info;
    const { k, local } = emProjToFile(VS, info.off + Math.min(info.len, 0.001)), s = VS[k];
    return { ...info, path: s.path, off: Math.max(0, local - Math.min(info.len, 0.001)), mlen: s.duration, title: s.title, width: s.width, height: s.height };
  }
  /** A model's track rows for these titles (row k = title k; a row it had already keeps its levels and its id).
   *  Every track row's `index` is its title's place in m.trackTitles (the export finds each source's track by it). */
  function titleRows(m, titles) {
    const n = m.removed.length, ids = new Set(m.audio.map((r) => r.id));
    const fresh = (k) => { let id = `t${k}`, j = 1; while (ids.has(id)) id = `t${k}_${j++}`; ids.add(id); return id; };
    m.audio = [
      ...titles.map((title, k) => m.audio.find((r) => r.kind === "track" && r.title === title) || { id: fresh(k), kind: "track", index: k, title, path: "", volume: 1, muted: false, gains: Array(n).fill(1) }),
      ...m.audio.filter((r) => r.kind !== "track"),
    ];
    for (const r of m.audio) if (r.kind === "track") r.index = titles.indexOf(r.title);
    // Rows by role (0.1.44): each row knows its role by its name ("Desktop", "Mic", "Game", "Clip audio").
    if (emRowsMode(m) === "roles") for (const r of m.audio) if (r.kind === "track") r.role = emRoleOfName(r.title) || "other"; // a named track's own row
    m.tracks0 = titles.map((title, k) => ({ index: k, title }));
    m.trackTitles = titles.slice();
  }
  /* ----- the editor is usable at once (0.1.41, F2) -----
   * A project that references its clips gets its model, its pieces and its picture straight away; each source's own
   * tracks (editor_audio_tracks: slow, the backend makes a file per track), then waveforms and film strips arrive
   * after, source by source, nearest the playhead first, at most 3 asked at once. Meanwhile the video's own sound
   * plays (the clip's mix) and a track row not known yet is simply not there (or has no waveform: drawn flat). A new
   * title adds its row at the end (titles only grow, so every row keeps its place and index) and the undo history
   * learns it too. Closing, or clips added meanwhile, makes a load stale: it writes nothing (`loadGen`). */
  let loadGen = 0;
  P.tracksPending = () => !!VS && VS.some((s) => !s.missing && !Array.isArray(s.tracks));
  /** The saved edit's track titles, by their index (to have the rows its levels belong to before the tracks come). */
  /** ...and its rows' mode (0.1.44): { titles, mode: "roles" | "titles" | null (nothing saved: a new project) }. */
  async function savedRows() {
    if (!P.draft) return { titles: [], mode: null };
    let text = null;
    try {
      if (typeof projCarry !== "undefined" && projCarry && projCarry.id === c.id) text = projCarry.state;
      else if (typeof projCache !== "undefined" && projCache.has(c.id)) text = projCache.get(c.id);
      else text = await invoke("project_state", { id: c.id });
      if (text == null || text === "") return { titles: [], mode: null };
      const s = typeof text === "string" ? JSON.parse(text) : text;
      const rows = (Array.isArray(s?.audio) ? s.audio : []).filter((r) => r && r.kind === "track" && typeof r.title === "string");
      return { titles: [...new Set(rows.slice().sort((a, b) => (+a.index || 0) - (+b.index || 0)).map((r) => r.title))], mode: s?.rows_mode === "roles" ? "roles" : "titles" };
    } catch { return { titles: [], mode: null }; }
  }
  /** Rows by role (0.1.44): the role rows of `known` sources and the titles `have`, Desktop / Mic / Game / Clip audio in
   *  that order, then the named tracks that keep a row of their own (in the order they come). */
  function roleTitles(known, have) {
    const rows = emRoleRows(known).rows.map((x) => x.title);
    const all = [...new Set([...(have || []), ...rows])];
    const roles = EM_ROLES.map((r) => r[1]);
    return [...roles.filter((x) => all.includes(x)), ...all.filter((x) => !roles.includes(x))];
  }
  /** Each source's game (a track named after it is its Game track), and the games Glint knows (0.1.44). */
  function sourceGames() {
    const lib = typeof S !== "undefined" && Array.isArray(S.clips) ? S.clips : [];
    if (typeof emSetKnownGames === "function") emSetKnownGames([...new Set(lib.map((x) => x.game).filter(Boolean))]);
    for (const s of VS || []) if (!s.game) s.game = lib.find((x) => x.id === s.clip)?.game || "";
  }
  /** Titles from every source whose tracks are known, after the ones the model has (append-only). */
  function grownTitles(m) {
    const have = Array.isArray(m.trackTitles) ? m.trackTitles.slice() : [];
    if (emRowsMode(m) === "roles") return roleTitles(VS.filter((s) => Array.isArray(s.tracksAll)), have);
    for (const x of emTrackTitles(VS.map((s) => s.tracks || []))) if (!have.includes(x)) have.push(x);
    return have;
  }
  /** New track rows into every undo / redo step too (an undo must not take a row away that only arrived late). */
  function rowsIntoHistory(m) {
    const tracks = m.audio.filter((r) => r.kind === "track");
    const fix = (json) => {
      try {
        const o = JSON.parse(json);
        if (!Array.isArray(o.audio) || !Array.isArray(o.removed)) return json;
        if ((o.rm || "titles") !== emRowsMode(m)) return json; // a step from the other mode keeps its own rows (0.1.44)
        if (Array.isArray(o.tt)) o.tt = m.trackTitles.slice();
        const known = new Set(o.audio.filter((r) => r.kind === "track").map((r) => r.title));
        const add = tracks.filter((r) => !known.has(r.title)).map((r) => ({ ...r, volume: 1, muted: false, gains: Array(o.removed.length).fill(1) }));
        const tr = [...o.audio.filter((r) => r.kind === "track"), ...add];
        for (const r of tr) { const k = m.trackTitles.indexOf(r.title); if (k >= 0) r.index = k; }
        o.audio = [...tr, ...o.audio.filter((r) => r.kind !== "track")];
        return JSON.stringify(o);
      } catch { return json; }
    };
    m.undo = m.undo.map(fix);
    m.redo = m.redo.map(fix);
  }
  /** Sources in the order they are needed: the one under the playhead first, then by distance from it. */
  function byNearness(list) {
    const m = P.model;
    let at = 0;
    try { at = m ? emToSrc(m, tlNow()) : 0; } catch { at = 0; }
    const dist = (s) => (at < s.start ? s.start - at : at > s.start + s.duration ? at - s.start - s.duration : 0);
    return list.slice().sort((a, b) => dist(a) - dist(b));
  }
  /* One background queue for a project's clips (0.1.41): at most 3 calls at once; the next is always the most needed:
   * the clip under the playhead: its tracks, its waveform, its film strip; then every other clip's TRACKS, nearest
   * first (so every row soon plays its own sound); then their waveforms; then their film strips. The playhead's place
   * is read at each pick. A task whose editor was closed or replaced is dropped (`stale`). */
  const bgQ = [];
  let bgBusy = 0;
  const BG_ORDER = { tracks: 0, waves: 1, thumbs: 2 };
  function bgAdd(kind, s, fn, stale) { bgQ.push({ kind, s, fn, stale }); bgPump(); }
  function bgPump() {
    while (bgBusy < 3) {
      for (let i = bgQ.length - 1; i >= 0; i--) if (bgQ[i].stale()) bgQ.splice(i, 1);
      if (!bgQ.length) return;
      const rank = new Map(byNearness(VS).map((s, i) => [s, i]));
      const key = (x) => { const r = rank.get(x.s) ?? 1e6; return [r === 0 ? 0 : BG_ORDER[x.kind], r, BG_ORDER[x.kind]]; };
      let best = 0;
      for (let i = 1; i < bgQ.length; i++) { const a = key(bgQ[i]), b = key(bgQ[best]); if (a[0] < b[0] || (a[0] === b[0] && (a[1] < b[1] || (a[1] === b[1] && a[2] < b[2])))) best = i; }
      const task = bgQ.splice(best, 1)[0];
      bgBusy++;
      Promise.resolve().then(task.fn).catch((e) => { if (!task.stale()) console.warn("Glint: a project's clip did not load its extras", task.s.title, e); })
        .finally(() => { bgBusy--; bgPump(); });
    }
  }
  /** Each source's tracks in the background; rows, history, preview sound and waveforms follow, source by source. */
  function loadSourceTracks(list) {
    const gen = loadGen, m = P.model, stale = () => dead || gen !== loadGen || P.model !== m;
    for (const s of list) {
      if (s.missing || Array.isArray(s.tracks)) continue;
      bgAdd("tracks", s, async () => {
        await sourceTracks([s]);
        if (stale()) return;
        const titles = grownTitles(m);
        if (titles.length !== (m.trackTitles || []).length) {
          titleRows(m, titles);
          rowsIntoHistory(m);
          tl((h) => h.refresh());
          P.edSync("tracks");
        }
        syncTrackEls();      // this clip's own sound, now
        waveOf(s, m, stale); // its waveforms next
      }, stale);
    }
    syncTrackEls(); // the clips whose tracks are known already
  }
  /**
   * The model of a project that references its clips (0.1.38): on project time (the clips end to end), one track row
   * per title across the clips, each clip its own part with its footage limits (m.bounds), its saved edit back.
   * 0.1.41 (F2): made at once; the track rows the saved edit has are there from the start, the sources' own tracks
   * come in the background (loadSourceTracks). F1: a source with `inner` (an old project's draft file) is one piece
   * per clip inside it, each with its own footage limits, all from that one file.
   */
  async function virtualModel() {
    const known = VS.every((s) => s.missing || Array.isArray(s.tracks));
    // Rows by role (0.1.44) for a new project; an edit saved before keeps its rows by title, exactly as it was.
    const saved = await savedRows();
    if (dead) return null;
    const mode = saved.mode || "roles";
    sourceGames();
    const titles = mode === "roles"
      ? roleTitles(known ? VS.filter((s) => !s.missing) : [], saved.titles)
      : known ? emTrackTitles(VS.map((s) => s.tracks || [])) : saved.titles;
    const m = emNew(dur(), []);
    m.rowsMode = mode;
    // A project of one clip: its piece is named after that clip, not after the project (0.1.40).
    m.clipTitle = (VS.length === 1 && !VS[0]?.inner?.length && VS[0]?.title) || c.title || "";
    m.virtual = true; // its ranges never run across two clips' files (emSrcSpans)
    m.sources = VS;   // an ungrouped piece's sound comes from its own clip's file and stream (0.1.39)
    titleRows(m, titles);
    let restored = false;
    if (P.draft && typeof projRestore === "function") {
      try { restored = await projRestore(c, m); } catch (e) { console.warn("Glint: the project's saved edit could not be restored; it opens as it is.", e); restored = false; }
      if (dead) return null;
    }
    const cut = edPieceStarts(VS);
    if (!restored && cut.starts.length > 1) emSetParts(m, cut.starts, cut.titles);
    m.bounds = cut.starts; // every clip's footage ends at its own end (an inner clip of a draft too)
    if (!Array.isArray(m.trackTitles) || !m.trackTitles.length) m.trackTitles = titles.slice();
    P.model = m;
    if (P.draft && typeof projModelReady === "function") projModelReady(P, restored);
    queueMicrotask(() => { if (!dead && P.model === m) loadSourceTracks(VS); });
    return P.model;
  }
  /**
   * Clips added to an open project that references its clips (0.1.38): `next` is the project as project_open returns it
   * now. The new clips are appended as parts (emAddParts); nothing reloads. True if it took them.
   */
  P.addSources = async (next) => {
    const m = P.model;
    if (!VS || !m || !next || !Array.isArray(next.sources) || next.sources.length <= VS.length) return false;
    const fresh = next.sources.slice(VS.length).map((s, j) => ({ ...s, k: VS.length + j, start: +s.start || 0, duration: +s.duration || 0 }));
    // 0.1.41 (F2): on the timeline at once; their tracks, waveforms and frames come after (loadSourceTracks).
    if (dead || P.model !== m) return false;
    VS.push(...fresh);
    sourceGames(); // their games (a track named after it is their Game track, 0.1.44)
    c.sources = next.sources;
    c.duration = +next.duration || VS.reduce((n, s) => n + s.duration, 0);
    c.joins = next.joins;
    c.join_names = next.join_names;
    const end = VS[VS.length - 1].start + VS[VS.length - 1].duration, cut = edPieceStarts(fresh);
    emAddParts(m, cut.starts, cut.titles, end, VS[0].title);
    m.bounds = edPieceStarts(VS).starts;
    titleRows(m, grownTitles(m)); // the rows it has, plus titles of sources already known
    rowsIntoHistory(m);
    loadSourceTracks(fresh); // the new clips' tracks, then their own preview sound (the others' stays as it is)
    extrasAsked = false;
    fetchExtras(); // the film strip of the whole project again (the strip is spread over its length)
    joinWaves(m);  // the rows' waveforms over the new length
    tl((h) => h.refresh());
    P.edSync("add");
    return true;
  };
  /* ----- rows by role or by title (0.1.44) ----- */
  /** Resolves true once every clip's tracks are known (the export's track_map needs them), false after `ms`. */
  P.whenTracks = async (ms) => {
    const t0 = performance.now();
    while (!dead && (singlePending || (VS && P.tracksPending()))) { if (performance.now() - t0 > ms) return false; await new Promise((r) => setTimeout(r, 100)); }
    return !dead;
  };
  P.whenTracks.ready = () => !singlePending && !(VS && P.tracksPending());
  /** True when the rows can be switched: every clip's tracks are known. */
  const rowsReady =() => !!P.model && !singlePending && !(VS && P.tracksPending()) && (!!VS || Array.isArray(P.model.tracksAll));
  /** What "Simplify the audio rows" would do now, without doing it: { before, after, mixesDropped } or null. */
  let rowsMemo = { key: "", res: null };
  function rowsPreview() {
    const m = P.model;
    if (!rowsReady() || emRowsMode(m) === "roles") return null;
    const key = emState(m);
    if (rowsMemo.key === key) return rowsMemo.res;
    rowsMemo = { key, res: rowsPreview0(m) };
    return rowsMemo.res;
  }
  function rowsPreview0(m) {
    const copy = { ...JSON.parse(JSON.stringify({ ...m, sources: null, undo: [], redo: [] })), sources: m.sources, undo: [], redo: [] };
    const res = emSimplifyRows(copy);
    if (!res) return null;
    const same = res.after === res.before && JSON.stringify(copy.audio.filter((r) => r.kind === "track").map((r) => [r.index, r.gains])) === JSON.stringify(m.audio.filter((r) => r.kind === "track").map((r) => [r.index, r.gains]));
    return same ? null : res;
  }
  /** The preview sound and the waveforms of the rows again, after the rows changed. */
  function rowsChanged() {
    for (const [id, s] of [...P.src]) if (s.kind === "track") { dropSrc(s); P.src.delete(id); }
    P.trackState = "idle";
    if (VS) { for (const s of VS) delete s.trackFail; syncTrackEls(); joinWaves(P.model); }
    else { loadTracks(); for (const [id, wave] of P.waves) if (P.model.audio.some((r) => r.id === id)) tl((h) => h.setWaveform(id, wave)); }
    tl((h) => h.refresh());
    P.edSync("rows");
    output(true);
  }
  /** "Simplify the audio rows": rows by role, one undo step; a flash says what changed. */
  P.simplifyRows = () => {
    const m = P.model;
    if (!rowsReady() || emRowsMode(m) === "roles") return false;
    emSnapshot(m);
    const res = emSimplifyRows(m);
    if (!res) { m.undo.pop(); return false; }
    rowsChanged();
    rowsOfferDone();
    flash(t("{before} audio rows are now {after}.", { before: res.before, after: res.after }) + (res.mixesDropped ? " " + t("Mixes that doubled the sound are left out.") : ""));
    return true;
  };
  /** "Show every track" on (rows by title) or off (rows by role), one undo step, levels kept either way. */
  P.everyTrack = (on) => {
    const m = P.model;
    if (!rowsReady() || (emRowsMode(m) === "titles") === !!on) return false;
    if (!on) return P.simplifyRows();
    emSnapshot(m);
    const res = emEveryTrack(m);
    if (!res) { m.undo.pop(); return false; }
    rowsChanged();
    return true;
  };
  const rowsOfferKey = () => `glint.rowsOffer.${c.id}`;
  function rowsOfferDone() { try { localStorage.setItem(rowsOfferKey(), "1"); } catch { /* the line just comes back next time */ } }
  /** What the timeline shows about the rows: { mode, ready, offer: null | { before, after, mixes } }. */
  P.rowsInfo = () => {
    if (!P.model) return { mode: "titles", ready: false, offer: null };
    let dismissed = false;
    try { dismissed = localStorage.getItem(rowsOfferKey()) === "1"; } catch { dismissed = false; }
    const pv = !dismissed ? rowsPreview() : null;
    return { mode: emRowsMode(P.model), ready: rowsReady(), offer: pv ? { before: pv.before, after: pv.after, mixes: pv.mixesDropped } : null };
  };
  P.rowsOfferDismiss = () => { rowsOfferDone(); tl((h) => h.refresh()); };
  /** Saves the bookmarks in source seconds, so they find their picture again whatever the part order. */
  const saveMarks = () => {
    const m = P.model;
    if (m) edMarksSave(c.id, (m.marks || []).map((x) => emToSrc(m, x)));
    if (P.draft && typeof projDirty === "function") projDirty(P, "marks"); // a project saves them with its edit
  };

  const tlFailHtml = () => `<div class="e-tl-fail">${esc(t("The timeline did not load. Speed, color, export and the I and O keys still work."))}</div>`;
  function mountTl() {
    const host = el(".e-tl");
    if (P.tl || !host) return;
    host.textContent = "";
    if (typeof mountTimeline !== "function") {
      console.error("Glint: mountTimeline is not defined (timeline.js is missing or failed to load)");
      host.innerHTML = tlFailHtml();
      return;
    }
    try {
      P.tl = mountTimeline(host, {
        model: P.model,
        title: c.title,
        game: c.game, // names the Game row ("Overwatch audio")
        getTime: () => tlNow(),
        onSeek: (sec, exact) => P.seek(sec, false, !!exact), // exact: onto a bookmark, even inside a cut-out part
        // 0.1.46: a piece's end being dragged: the preview shows the frame under it (P.peek, throttled).
        onTrimPeek: (r, side, speed) => P.peek(emTrimPeek(r, side, speed, frame).at),
        onChange: (what) => P.edSync(what),
        onMarks: () => saveMarks(),
        // sel: the primary piece { row, seg } or null; sels: every selected piece (older timelines pass only sel).
        onSelect: (sel, sels) => {
          P.sel = sel || null;
          P.sels = Array.isArray(sels) ? sels.filter((s) => s && Number.isInteger(s.seg)) : P.sel ? [P.sel] : [];
          if (P.sels.length) P.ov?.tlSelected(); // a segment picked: Delete is the timeline's again
          P.ed?.selChanged?.();
        },
        onAddAudio: () => edPickAudio(),
        // The timeline's own buttons: "shortcuts", "fullscreen", or any ED_KEYS cmd.
        onCommand: (name) => {
          if (name === "shortcuts") P.openKeys();
          else if (name === "fullscreen") P.editorFull();
          else if (ED_KEYS.some((k) => k.cmd === name)) P.command(name);
          else console.warn("Glint: unknown timeline command", name);
        },
        keyText: edKeyText, // the current shortcut of an ED_KEYS cmd as text ("Ctrl+Z"), for tooltips; "" if none
        // Overlay blocks (0.1.35): a click selects the item in the overlay code; its menu calls P.ov / P.cv where they have the function.
        onOverlayPick: (id) => { try { P.ov?.select?.(id); } catch (e) { console.error("Glint: ov.select failed", e); } },
        ovApi: () => P.ov || null,
        // A piece dragged up onto the rows above the video (0.1.37): media.js makes the clip item and its sound row.
        // Called INSIDE the timeline's own undo snapshot: medFromPiece must not snapshot itself.
        canPieceUp: () => typeof medFromPiece === "function",
        pieceUp: (info, m) => (typeof medFromPiece === "function" ? medFromPiece(P, pieceFile(info), m) : null),
        // A project that references its clips (0.1.38): a piece of a missing clip is marked on the video row.
        missingAt: (src) => (VS ? (vMissing(src) ? VS[emSourceAt(VS, src)].title || "?" : null) : null),
        // 0.1.41 (F2): a track row whose sound is still on its way says so, quietly.
        soundLoading: () => !!VS && (P.tracksPending() || bgQ.some((x) => x.kind !== "thumbs") || bgBusy > 0),
        cvApi: () => P.cv || null,
        // 0.1.44: rows by role or by title: what to show, and the actions.
        rowsInfo: () => P.rowsInfo(),
        rowsApi: { simplify: () => P.simplifyRows(), everyTrack: (on) => P.everyTrack(on), dismiss: () => P.rowsOfferDismiss() },
      });
    } catch (e) {
      console.error("Glint: mountTimeline failed", e);
      P.tl = null;
      host.innerHTML = tlFailHtml();
      return;
    }
    if (P.thumbs) tl((h) => h.setThumbs(P.thumbs));
    for (const [id, wave] of P.waves) if (P.model.audio.some((r) => r.id === id)) tl((h) => h.setWaveform(id, wave));
  }
  function dropTimeline() {
    const h = P.tl;
    P.tl = null;
    P.sel = null;
    P.sels = [];
    if (h) { try { h.destroy(); } catch (e) { console.error("Glint: timeline destroy failed", e); } }
    const host = el(".e-tl");
    if (host) host.textContent = "";
  }
  /**
   * A clip whose desktop sound is silent while its Game track is not: the game played on another output device than
   * the recorded one, so it is only on the Game row. That row is then heard beside Desktop (emGameHidden), in the
   * preview and in the export. Needs both the rows and their waveforms: called when either arrives (0.1.41: the rows
   * may now come after the waveforms).
   */
  function checkGameApart() {
    const mm = P.model, d = mm && emTrackRow(mm, "Desktop"), g = mm && emTrackRow(mm, "Game");
    if (dead || !d || !g || mm.gameApart || !P.waves.has(d.id) || !P.waves.has(g.id)) return false;
    const loudest = (row) => { let top = 0; for (const p of P.waves.get(row.id)?.peaks || []) if (p > top) top = p; return top; };
    if (!(loudest(d) < 0.01 && loudest(g) > 0.03)) return false;
    mm.gameApart = true;
    P.edSync("volume");
    return true;
  }
  P.checkGameApart = checkGameApart;
  /** Waveforms and film strip, fetched once per open clip in the background. */
  function fetchExtras() {
    if (extrasAsked) return;
    extrasAsked = true;
    if (VS) return fetchExtrasVirtual();
    invoke("editor_waveforms", { id: c.id }).then((list) => {
      if (dead) return;
      if (!Array.isArray(list)) throw new Error("editor_waveforms did not return a list");
      for (const w of list) {
        const id = "t" + w.index, wave = { peaks: Array.isArray(w.peaks) ? w.peaks : [], perSecond: +w.per_second || 0 };
        P.waves.set(id, wave);
        if (P.model?.audio.some((r) => r.id === id)) tl((h) => h.setWaveform(id, wave));
      }
      checkGameApart();
    }).catch((e) => { if (!dead) console.warn("Glint: editor_waveforms failed; the audio rows show no waveform.", e); });
    invoke("editor_thumbs", { id: c.id, count: ED_THUMBS }).then((paths) => {
      if (dead) return;
      if (!Array.isArray(paths)) throw new Error("editor_thumbs did not return a list");
      P.thumbs = paths.map(fileSrc);
      tl((h) => h.setThumbs(P.thumbs));
    }).catch((e) => { if (!dead) console.warn("Glint: editor_thumbs failed; the video row shows no film strip.", e); });
  }

  /* A project that references its clips (0.1.38): each clip's waveforms and film strip, laid on project time.
   * 0.1.41 (F2): source by source, nearest the playhead first, 3 at a time; what has come is shown at once (the rest
   * of a row stays flat, the rest of the strip empty until its frames come). */
  const srcWaves = new Map(); // source index -> its editor_waveforms list
  const waveAsked = new Set();
  /** Every track row's waveform from the sources' that have come. */
  function joinWaves(m) {
    if (dead || P.model !== m) return;
    const total = dur(), lists = [...srcWaves.values()].flat();
    const rates = lists.map((w) => +w?.per_second || 0).filter((x) => x > 0), ps = rates.length ? Math.max(...rates) : 50;
    for (const row of m.audio.filter((r) => r.kind === "track")) {
      const parts = VS.map((s) => {
        const tr = srcTrackOf(s, row, null); // by role or by title (0.1.44)
        const w = tr && (srcWaves.get(s.k) || []).find((x) => x.index === tr.index);
        return { start: s.start, duration: s.duration, peaks: w && Array.isArray(w.peaks) ? w.peaks : null, ps: +w?.per_second || 0 };
      });
      const wave = { peaks: emJoinPeaks(parts, total, ps), perSecond: ps };
      P.waves.set(row.id, wave);
      tl((h) => h.setWaveform(row.id, wave));
    }
  }
  /** Source `s`'s waveforms (once its tracks are known), then the rows again. */
  function waveOf(s, m, stale) {
    if (!extrasAsked || s.missing || !Array.isArray(s.tracks) || waveAsked.has(s.k)) return;
    waveAsked.add(s.k);
    bgAdd("waves", s, async () => {
      const list = await invoke("editor_waveforms", { id: s.clip });
      if (stale()) return;
      srcWaves.set(s.k, Array.isArray(list) ? list : []);
      joinWaves(m);
    }, stale);
  }
  function fetchExtrasVirtual() {
    const gen = loadGen, m = P.model, stale = () => dead || gen !== loadGen || P.model !== m;
    for (const s of VS) waveOf(s, m, stale); // the sources whose tracks are known already; the rest as theirs come
    // Film strip: frames from each clip in proportion to its length, end to end (a missing clip: none, see the timeline).
    // A newer strip (clips added: it is spread over the new length) makes this one's frames stale.
    const tg = ++thumbGen, tStale = () => stale() || tg !== thumbGen;
    const list = VS.slice(), total = dur();
    const counts = list.map((s) => Math.max(1, Math.round((ED_THUMBS * s.duration) / Math.max(1e-6, total))));
    const parts = list.map((s, k) => Array(counts[k]).fill(""));
    const show = () => { if (!tStale()) { P.thumbs = parts.flat(); tl((h) => h.setThumbs(P.thumbs)); } };
    for (const s of list) {
      if (s.missing) continue;
      bgAdd("thumbs", s, async () => {
        const k = list.indexOf(s), p = await invoke("editor_thumbs", { id: s.clip, count: counts[k] }).catch(() => null);
        if (!tStale() && Array.isArray(p)) { parts[k] = p.map(fileSrc); show(); }
      }, tStale);
    }
  }
  let thumbGen = 0;
  /** The timeline's selection now: { sel, sels } (see onSelect). Asks the timeline when it can, else uses what onSelect last said. */
  P.selection = () => {
    let sel = P.sel, sels = P.sels;
    if (P.tl) {
      try { if (typeof P.tl.selection === "function") sel = P.tl.selection() ?? null; } catch (e) { console.error("Glint: timeline selection() failed", e); }
      try { if (typeof P.tl.selections === "function") { const x = P.tl.selections(); if (Array.isArray(x)) sels = x; } } catch (e) { console.error("Glint: timeline selections() failed", e); }
    }
    sels = Array.isArray(sels) ? sels.filter((s) => s && Number.isInteger(s.seg)) : [];
    if (!sels.length && sel && Number.isInteger(sel.seg)) sels = [sel];
    return { sel: sel || null, sels };
  };

  /** Re-sync the preview and the side column after the model changed (from the timeline, a key or the side column). */
  P.edSync = (what) => {
    if (dead) return;
    // The main video row was laid out anew (0.1.36): the playhead may now be in a gap, or its gap gone.
    const em0 = ed();
    if (em0) {
      const now = tlNow();
      if (!aft && noVid(em0, now)) setAft(now, playing());
      else if (aft && now < em0.duration - EM_EPS && !noVid(em0, now)) go(now, playing());
      // The footage under the playhead left the row (its piece was cut out or trimmed away, an undo): the video still
      // stood in it, so Play showed and sounded what is no longer on the timeline while the playhead waited at the
      // start of the next piece (0.1.52). It goes to what the playhead shows now.
      else if (!aft && !vpend && em0.parts && !noVid(em0, now) && Math.abs(emToSrc(em0, now) - vNow()) > 0.05) go(edLand(em0, now, false, frame), playing());
    }
    syncRows();
    applyRate();
    applyColor();
    output(["load", "undo", "redo", "file", "order"].includes(what));
    paint();
    P.ed?.sync(what);
    P.cv?.sync(what); // before the overlays: they are placed on the canvas
    P.zm?.sync(what); // the Zoom section (0.1.41)
    P.ov?.sync(what);
    if (P.draft && typeof projDirty === "function") projDirty(P, what); // a project saves its edit (debounced)
  };
  /** Change the model: snapshot, run `fn`, then refresh the timeline and the preview.
   *  If `fn` changed nothing, the snapshot is taken back and `failMsg` (if any) is shown. */
  P.edit = (what, fn, failMsg) => {
    const m = P.model;
    if (!m) return false;
    const before = edState(m), undo = m.undo.slice(), redo = m.redo.slice();
    emSnapshot(m);
    fn(m);
    if (edState(m) === before) {
      m.undo = undo;
      m.redo = redo;
      if (failMsg) flash(failMsg);
      return false;
    }
    tl((h) => h.refresh());
    P.edSync(what);
    return true;
  };
  /** Add a file (an editor_audio_info result) as a new row at the playhead, or at timeline second `at` (dropped on the timeline). */
  P.addFileRow = (info, at) => {
    const m = P.model;
    emSnapshot(m);
    const row = emAddFile(m, info, Number.isFinite(at) ? at : tlNow());
    const wave = { peaks: Array.isArray(info.peaks) ? info.peaks : [], perSecond: +info.per_second || 0 };
    P.waves.set(row.id, wave); // before syncRows, which would otherwise read the file again for it
    syncRows(); // creates its audio element
    tl((h) => h.refresh());
    tl((h) => h.setWaveform(row.id, wave));
    P.edSync("file");
    return row;
  };

  /** Edit mode: timeline under the video, edit column, and the speed / color / audio preview.
   *  Returns a promise of the model (null when leaving edit mode or if it could not be made). */
  P.setEdit = (on) => {
    on = !!on;
    if (on !== P.editing) {
      P.editing = on;
      root.classList.toggle("editing", on);
      v.loop = P.loop && !on; // while editing, looping is handled by guard() and "ended"
      if (!on && aft) { aft = null; state(); } // the time after the video is only there while editing (0.1.35)
      if (on) v.pause();
      else { edPopClose?.(); dropTimeline(); }
      w.pause(); // the standby waits only while editing (0.1.41)
      sb = null;
      pendingCut = null;
      if (document.fullscreenElement) leaveFull(); // video-only or editor fullscreen does not fit the other mode
      P.cv?.setEdit(on);
      P.ov?.setEdit(on);
      applyRate();
      applyColor();
      output(true);
      paint();
      P.onEdit?.(on);
    }
    if (!on) return Promise.resolve(null);
    return ensureModel().then((m) => {
      if (!m || dead || !P.editing) return m;
      mountTl();
      loadTracks();
      fetchExtras();
      P.edSync("load");
      return m;
    }).catch((e) => {
      console.error("Glint: the editor could not start", e);
      fail(e);
      return null;
    });
  };
  P.setTrim = P.setEdit; // app.js opens a clip "in the editor" with setTrim(true)
  P.openKeys = () => edOpenKeys(root);

  /** Runs one editor shortcut (an ED_KEYS cmd). */
  /** 0.1.46: after O, the playhead stood on the first frame cut away (the new end): it goes back one frame, onto the
   *  piece's new last frame (I needs nothing: the playhead is on the new first frame already). */
  function trimEndPeek(at) {
    const m = P.model;
    const p = m && emPieceList(m).find((q) => !q.gap && Math.abs(q.at + q.len - at) < 0.02);
    if (p) P.peek(emTrimPeek(p, "end", p.speed, frame).at);
  }
  P.command = (cmd) => {
    const now = tlNow();
    // Delete and Esc first go to the overlays (a selected text or shape, a drawing tool, a text being typed).
    if ((cmd === "remove" || cmd === "escape") && !edPopClose && (P.cv?.command(cmd) || P.ov?.command(cmd))) return;
    switch (cmd) {
      case "play": return P.toggle();
      case "back5": return P.seek(now - edSeekStep(), true);
      case "fwd5": return P.seek(now + edSeekStep());
      case "back1": return P.seek(now - 1, true);
      case "fwd1": return P.seek(now + 1);
      // A frame step stands still: the video, a file still loading, and the player's own clock in a gap (0.1.41).
      case "framePrev": P.stop(); return P.seek(now - frame, true);
      case "frameNext": P.stop(); return P.seek(now + frame);
      case "home": return P.seek(0);
      case "end": return P.seek(P.model ? Math.max(P.model.duration, emEnd(P.model)) : dur(), true); // the result's end (after the video when something goes on there)
      case "loop": return P.setLoop(!P.loop);
      case "fullscreen": return P.editorFull();
      case "zoomIn": return tl((h) => h.zoom(ED_ZOOM_STEP));
      case "zoomOut": return tl((h) => h.zoom(1 / ED_ZOOM_STEP));
      case "fit": return tl((h) => h.fit());
      case "export": return edExport(false);
      case "escape": // in turn: the pop-up, the selection, the editor
        if (edPopClose) return edPopClose();
        if (P.tl?.tool?.() === "blade") return tl((h) => h.setTool("select")); // the blade ends first (0.1.36)
        if (P.selection().sels.length && typeof P.tl?.run === "function") {
          try { if (P.tl.run("clearSel")) return; } catch (e) { console.error("Glint: timeline run(clearSel) failed", e); }
        }
        return P.setEdit(false);
    }
    const m = P.model;
    if (!m) return flash(t("The editor is still loading"));
    // Selection commands go to the timeline, which applies them to every selected
    // piece, snapshots and reports the change itself. The code further down is the
    // fallback for a timeline without run() (or none at all): primary selection only.
    if (ED_TL_CMDS.has(cmd) && typeof P.tl?.run === "function") {
      const { sel: before, sels } = P.selection(), marksBefore = (m.marks || []).length;
      let ok, threw = false;
      try { ok = !!P.tl.run(cmd); } catch (e) { threw = true; console.error(`Glint: timeline run(${cmd}) failed, using the editor's own code`, e); }
      if (!threw) {
        if (ok) {
          if (cmd === "trimEnd") trimEndPeek(now);
          if (cmd === "mark") flash((m.marks || []).length > marksBefore ? t("Bookmark added") : t("Bookmark removed"));
          if (cmd === "louder" || cmd === "quieter") {
            const sel = P.selection().sel || before, row = sel && sel.row !== "video" ? emRow(P.model, sel.row) : null;
            // An audio clip's level is its own (R1 #5, 0.1.41); a track row's piece has its segment's.
            if (row && row.kind === "file") flash(t("Level {pct}%", { pct: Math.round((row.muted ? 0 : row.volume) * 100) }));
            else if (row && Number.isInteger(sel.seg) && sel.seg < row.gains.length) flash(t("Level {pct}%", { pct: Math.round(row.gains[sel.seg] * 100) }));
          }
          return;
        }
        // Nothing happened: say why when it is the usual reason.
        if (cmd === "split") return flash(t("Too close to a cut point to split here"));
        if (cmd === "trimStart" || cmd === "trimEnd") return edTrimRefused(m, cmd, now) ? flash(t("Nothing would be left to keep")) : undefined;
        if (cmd === "markNext") return flash(t("No bookmark after the playhead"));
        if (cmd === "markPrev") return flash(t("No bookmark before the playhead"));
        if ((cmd === "remove" || cmd === "blank") && !sels.length) return flash(t("Select a segment on the timeline first"));
        if (cmd === "remove") return flash(t("At least one part of the clip has to stay"));
        if (["mute", "louder", "quieter", "solo"].includes(cmd) && !sels.some((s) => s.row !== "video")) return flash(t("Select an audio segment on the timeline first"));
        return;
      }
    }
    switch (cmd) {
      case "cutPrev": {
        const pts = [0, ...m.cuts].filter((x) => x < now - 0.02);
        return P.seek(pts.length ? pts[pts.length - 1] : 0, false, true);
      }
      case "cutNext": {
        const pts = [...m.cuts, m.duration].filter((x) => x > now + 0.02);
        return P.seek(Math.min(pts.length ? pts[0] : m.duration, dur() - frame), false, true);
      }
      case "mark": { // without the timeline: same rule as its run("mark")
        const i = emMarkNear(m, now, EM_MARK_GAP);
        if (i >= 0) emRemoveMark(m, i); else emAddMark(m, now);
        saveMarks();
        return flash(i >= 0 ? t("Bookmark removed") : t("Bookmark added"));
      }
      case "markNext": case "markPrev": {
        const at = cmd === "markNext" ? emNextMark(m, now) : emPrevMark(m, now);
        if (at === null) return flash(cmd === "markNext" ? t("No bookmark after the playhead") : t("No bookmark before the playhead"));
        return P.seek(at, false, true);
      }
      case "split": return P.edit("split", (mm) => emSplit(mm, now), t("Too close to a cut point to split here"));
      // Without the timeline's run(): the same model functions as its trim brackets.
      case "trimStart": case "trimEnd":
        if (edTrimRefused(m, cmd, now)) return flash(t("Nothing would be left to keep"));
        if (!P.edit("trim", (mm) => (cmd === "trimStart" ? emSetTrimStart(mm, now, true) : emSetTrimEnd(mm, now, true)))) return false;
        if (cmd === "trimEnd") trimEndPeek(now);
        return true;
      case "undo": if (emUndo(m)) { tl((h) => h.refresh()); P.edSync("undo"); } else flash(t("Nothing to undo")); return;
      case "redo": if (emRedo(m)) { tl((h) => h.refresh()); P.edSync("redo"); } else flash(t("Nothing to redo")); return;
    }
    if (cmd === "hideCuts") {
      if (typeof P.tl?.setHideCuts === "function" && typeof P.tl.hideCuts === "function") tl((h) => h.setHideCuts(!h.hideCuts()));
      return;
    }
    if (cmd === "selectAll" || cmd === "clearSel") return; // need the timeline's run()
    if (!ED_TL_CMDS.has(cmd)) return;
    const { sel } = P.selection();
    const seg = sel && Number.isInteger(sel.seg) ? sel.seg : -1;
    if (cmd === "remove" || cmd === "blank") {
      if (seg < 0 || seg >= m.removed.length) return flash(t("Select a segment on the timeline first"));
      if (cmd === "blank") return P.edit("blank", (mm) => emSetBlank(mm, seg, !mm.blank[seg]));
      const off = !m.removed[seg];
      return P.edit(off ? "cut" : "restore", (mm) => emSetRemoved(mm, seg, off), t("At least one part of the clip has to stay"));
    }
    // mute, louder, quieter, solo: the selected audio segment. With the video row selected they do nothing.
    if (sel?.row === "video") return;
    const row = sel ? m.audio.find((r) => r.id === sel.row) : null;
    if (!row || seg < 0 || seg >= row.gains.length) return flash(t("Select an audio segment on the timeline first"));
    const g = row.gains[seg];
    if (cmd === "mute") return P.edit("mute", (mm) => emSetGain(mm, row.id, seg, g > 0.005 ? 0 : 1));
    if (cmd === "solo") return P.edit("solo", (mm) => emSolo(mm, row.id, seg), t("Solo needs at least two audio tracks"));
    const next = clamp(Math.round((g + (cmd === "louder" ? ED_GAIN_STEP : -ED_GAIN_STEP)) * 100) / 100, 0, EM_MAX_GAIN);
    if (P.edit("gain", (mm) => emSetGain(mm, row.id, seg, next))) flash(t("Level {pct}%", { pct: Math.round(next * 100) }));
  };

  P.destroy = () => {
    dead = true;
    loadGen++; // what is still loading writes nothing now (0.1.41)
    bgQ.length = 0;
    // ... and the backend drops the track / waveform / film-strip jobs still queued for this editor (edcache.rs).
    // Sent now, before the next editor (if one opens) asks for its own.
    if (P.editing || VS) { try { invoke("editor_prep_cancel").catch((e) => console.warn("Glint: editor_prep_cancel failed", e)); } catch (e) { console.warn("Glint: editor_prep_cancel failed", e); } }
    edPopClose?.();
    try { P.cv?.destroy(); } catch (e) { console.error("Glint: canvas destroy failed", e); }
    try { P.zm?.destroy(); } catch (e) { console.error("Glint: zoom destroy failed", e); }
    // The overlays too (0.1.41: their clip items' videos, observer and timer outlived the editor).
    try { P.ov?.destroy?.(); } catch (e) { console.error("Glint: overlays destroy failed", e); }
    clearTimeout(vpendTimer);
    clearInterval(aftTimer);
    aftTimer = 0;
    swapFns.length = 0;
    cancelAnimationFrame(raf);
    raf = 0;
    clearTimeout(idle);
    clearInterval(driftTimer);
    driftTimer = 0;
    dropTimeline();
    for (const s of P.src.values()) dropSrc(s);
    P.src.clear();
    if (P.ctx) P.ctx.close().catch((e) => console.warn("Glint: AudioContext close failed", e));
    P.ctx = false;
    P.model = null;
    P.waves.clear();
    P.thumbs = null;
    document.removeEventListener("fullscreenchange", onFullscreen);
    adoptOverlays(false); // now: the caller is about to empty #modal, and they must not go with it
    if (document.fullscreenElement && !(edKeepFull && document.fullscreenElement.id === "modal")) document.exitFullscreen().catch((e) => console.warn("Glint: exitFullscreen failed", e));
    for (const x of vids) { x.pause(); x.removeAttribute("src"); x.load(); } // the standby too (0.1.41)
  };
  /** Player shortcuts. Returns true when the key was used. In edit mode the keys are ED_KEYS. */
  P.key = (e) => {
    if (P.editing) {
      const hit = edKeyFind(e);
      if (!hit || hit.combo.ctrl) return false;
      if (!(e.repeat && hit.entry.once)) P.command(hit.entry.cmd);
      return true;
    }
    switch (e.code) {
      case "Space": case "KeyK": P.toggle(); break;
      case "ArrowLeft": P.seek(v.currentTime - (e.shiftKey ? 1 : edSeekStep())); break;
      case "ArrowRight": P.seek(v.currentTime + (e.shiftKey ? 1 : edSeekStep())); break;
      case "Comma": v.pause(); P.seek(v.currentTime - frame); break;
      case "Period": v.pause(); P.seek(v.currentTime + frame); break;
      case "ArrowUp": setVol(P.vol + 0.05); break;
      case "ArrowDown": setVol(P.vol - 0.05, false); break;
      case "KeyM": setMuted(!P.muted); break;
      case "KeyF": P.fullscreen(); break;
      case "KeyL": P.setLoop(!P.loop); break;
      case "Home": P.seek(0); break;
      case "End": P.seek(dur()); break;
      case "PageUp": viewerAction("prev"); break;   // the clip before this one in the library's list
      case "PageDown": viewerAction("next"); break; // the clip after it
      default: return false;
    }
    return true;
  };

  // On both videos (0.1.41); each acts only while its video is the one shown (the standby's own are above).
  const onShown = (x, fn) => (e) => { if (x === v) fn(e); };
  for (const x of vids) {
    for (const ev of ["play", "pause", "ended", "loadedmetadata"]) x.addEventListener(ev, onShown(x, state));
    for (const ev of ["timeupdate", "progress", "seeked", "loadedmetadata"]) x.addEventListener(ev, onShown(x, paint));
    // Keep the preview audio following the video.
    for (const ev of ["play", "playing", "pause", "waiting", "seeked", "ended", "ratechange"]) {
      x.addEventListener(ev, onShown(x, (e) => {
        if (!P.editing || dead) return;
        syncAudio(e.type === "play" || e.type === "seeked");
        drift();
      }));
    }
    // While editing: the video's end may be the end of a part in the middle of the timeline (parts moved): go on with
    // the next part. At the end of the timeline, loop back to the first kept part if looping.
    x.addEventListener("ended", onShown(x, () => {
      if (!P.editing || !P.model || dead) return;
      const past = P.model.parts ? playPart(P.model) : null, next = past ? afterPart(P.model, past) : null;
      if (aft) return;
      if (next !== null) { if (!noVid(P.model, next) && swapTo(next, P.model)) return; return go(next, true); } // the next part (the standby when it holds it), or a gap (0.1.36) on the player's own clock
      // Something goes on after the video (0.1.35): playing runs on into it on the player's own clock.
      if (emTail(P.model) > EM_EPS) { setAft(P.model.duration, true); state(); syncAudio(true); drift(); return; }
      if (P.loop) go(emPlayable(P.model, 0) ?? 0, true);
    }));
    x.addEventListener("click", P.toggle);
    x.addEventListener("dblclick", P.fullscreen);
  }
  box.addEventListener("mousemove", wake);
  box.addEventListener("mousedown", (e) => { if (e.target.closest("button")) e.preventDefault(); }); // keep keyboard focus off the buttons
  el(".p-vol").addEventListener("input", (e) => setVol(+e.target.value));
  el(".p-vol").addEventListener("change", (e) => e.target.blur()); // so Space plays / pauses again
  box.addEventListener("click", (e) => {
    const b = e.target.closest("[data-p]");
    if (!b) return;
    switch (b.dataset.p) {
      case "play": P.toggle(); break;
      case "back": P.seek(tlNow() - edSeekStep(), true); break;
      case "fwd": P.seek(tlNow() + edSeekStep()); break;
      case "mute": setMuted(!P.muted); break;
      case "speed":
        if (b.classList.contains("open")) { edPopClose?.(); break; }
        edOpenPop({ anchor: b, host: box, value: P.viewSpeed, above: true, onPick: (s) => { P.viewSpeed = s; applyRate(); state(); } });
        wake();
        break;
      case "loop": P.setLoop(!P.loop); break;
      case "full": P.fullscreen(); break;
    }
  });

  // The back / forward buttons say how far they jump; a right-click on either picks the step
  // (1 to 30 s). The Left and Right keys use the same step.
  const paintStep = () => {
    const n = edSeekStep(), hint = t("Right-click to change the step");
    for (const [p, text] of [["back", t("Back {n} seconds", { n })], ["fwd", t("Forward {n} seconds", { n })]]) {
      const b = el(`[data-p="${p}"]`);
      b.setAttribute("aria-label", text);
      b.title = `${text}\n${hint}`;
    }
  };
  paintStep();
  box.addEventListener("contextmenu", (e) => {
    const b = e.target.closest('[data-p="back"], [data-p="fwd"]');
    if (!b) return;
    e.preventDefault();
    e.stopPropagation();
    if (b.classList.contains("open")) { edPopClose?.(); return; }
    edOpenPop({ anchor: b, host: box, value: edSeekStep(), above: true, items: ED_SEEK_STEPS, label: (s) => t("{n}s", { n: s }), onPick: (s) => { edSetSeekStep(s); paintStep(); } });
    wake();
  });

  // Seek bar: drag to scrub. While editing it shows the finished clip.
  const barAt = (e) => { const r = bar.getBoundingClientRect(); return clamp((e.clientX - r.left) / r.width, 0, 1) * barLen(); };
  bar.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    bar.setPointerCapture(e.pointerId);
    drag = true;
    bar.classList.add("drag");
    P.seek(fromBar(barAt(e)));
  });
  bar.addEventListener("pointermove", (e) => {
    const tip = el(".p-tip"), r = bar.getBoundingClientRect();
    tip.hidden = false;
    tip.textContent = fmtDurMs(barAt(e) / barSpeed());
    tip.style.left = `${clamp(e.clientX - r.left, 24, r.width - 24)}px`;
    if (drag) P.seek(fromBar(barAt(e)));
  });
  bar.addEventListener("pointerleave", () => { if (!drag) el(".p-tip").hidden = true; });
  const dropDrag = () => { drag = false; bar.classList.remove("drag"); el(".p-tip").hidden = true; wake(); };
  bar.addEventListener("pointerup", dropDrag);
  bar.addEventListener("pointercancel", dropDrag);

  output();
  state();
  paint();
  return P;
}

/* ---------- editor side column: speed, color, export ---------- */
function mountEditor(root, c, P) {
  const pane = $(".v-edit", root);
  const speedBtn = $(".e-speed", pane);
  let lastExport = "", lastTarget = "";
  // during a slider gesture: the segments it edits (null = every segment, see target()); false between gestures.
  // R2 #2 (0.1.41): it starts as false (it was null, so the first drag after opening skipped its undo step, edited every
  // segment whatever was selected, and the sliders did not follow the selection until then).
  let colorDrag = false;
  const model = () => P.model;
  /** What the Color pane edits now (edColorTarget: the timeline's selection, or the whole clip). */
  const target = () => { const m = model(), s = P.selection(); return m ? edColorTarget(m, s.sels, s.sel, P.time()) : { segs: null, shown: 0 }; };
  /** The values the sliders show: the target's (primary) segment. */
  const color = () => P.model?.colors[target().shown] ?? ED_NO_COLOR;

  /** "Applies to: …" and "Apply to the whole clip", at the top of the pane. */
  function renderTarget() {
    const box = $(".e-target", pane), m = model();
    if (!box) return;
    let html = "";
    if (m) {
      const tg = target(), n = m.colors.length;
      const line = !tg.segs || n < 2 ? t("Applies to: the whole clip")
        : tg.segs.length === 1 ? t("Applies to: segment {n} of {total}", { n: tg.segs[0] + 1, total: n })
        : t("Applies to: {n} segments", { n: tg.segs.length });
      const shown = m.colors[tg.shown], same = m.colors.every((col) => emSameColor(col, shown));
      html = `<div class="small e-target-line">${esc(line)}</div>`;
      if (!tg.segs && !same) html += `<div class="small e-note">${esc(t("The segments have different colors. The sliders show the one at the playhead."))}</div>`;
      if (n > 1) html += `<button type="button" class="btn sm e-all" data-e-act="color-all" ${same ? "disabled" : ""}>${esc(t("Apply to the whole clip"))}</button>`;
    }
    if (html === lastTarget) return;
    lastTarget = html;
    box.innerHTML = html;
  }

  const presetList = () => [...ED_PRESETS.map((p, i) => ["b" + i, p]), ...edUserPresets().map((p, i) => ["u" + i, p])];

  /** Speed per piece (0.1.41): the selected pieces on V1, { n, speed } (n 0: none, the section is the result's speed). */
  const pieceSp = () => { try { return (model() && P.tl?.pieceSpeed?.()) || { n: 0, speed: 1 }; } catch { return { n: 0, speed: 1 }; } };
  function renderSpeed() {
    const m = model(), ps = pieceSp(), own = ps.n > 0;
    speedBtn.disabled = !m;
    speedBtn.innerHTML = `<span>${own ? +ps.speed.toFixed(2) : m ? m.speed : 1}x</span>${ED_CHEV}`;
    speedBtn.setAttribute("aria-label", own ? t("Speed of the selected clip") : t("Clip speed"));
    $(".e-speed-title", pane).textContent = own ? (ps.n > 1 ? t("Speed of the {n} selected clips", { n: ps.n }) : t("Speed of the selected clip")) : t("Speed");
    const numW = $(".e-speed-numw", pane), num = $(".e-speed-num", pane);
    numW.hidden = !own;
    if (own && document.activeElement !== num) num.value = String(+ps.speed.toFixed(2));
    $(".e-speed-note", pane).textContent = m ? t("Length {len}", { len: fmtDurMs(emResultDuration(m)) }) : t("Loading…");
  }
  $(".e-speed-num", pane).addEventListener("change", (e) => {
    const v = +e.target.value;
    if (Number.isFinite(v) && v > 0) P.tl?.setPieceSpeed?.(Math.min(EM_SPEED_MAX, Math.max(EM_SPEED_MIN, v)));
    renderSpeed();
  });

  function renderColor() {
    const user = edUserPresets(), col = color(), off = model() ? "" : "disabled";
    $(".e-color", pane).innerHTML = `
      <div class="e-title">${esc(t("Color"))}</div>
      <div class="e-target"></div>
      ${/* 0.1.46: the looks are one dropdown (dropdown.js draws it; edLookList adds the divider and the delete x). */ ""}
      <div class="e-look-row"><select class="field e-looks" id="e-looks" aria-label="${esc(t("Look"))}" ${off}>
        <option value="" disabled>${esc(t("Custom"))}</option>
        ${ED_PRESETS.map((p, i) => `<option value="b${i}">${esc(t(p.name))}</option>`).join("")}
        ${user.length ? `<optgroup label="">${user.map((p, i) => `<option value="u${i}" data-name="${esc(p.name)}">${esc(p.name)}</option>`).join("")}</optgroup>` : ""}
      </select></div>
      <div class="e-sliders">
        ${ED_SLIDERS.map(([k, label, min, max]) => `<label class="e-slider"><span>${esc(t(label))}</span><input type="range" min="${min}" max="${max}" step="0.01" value="${edCol(col, k)}" data-e-color="${k}" ${off}><output data-e-val="${k}">${edFmtVal(k, edCol(col, k))}</output>${k === "temperature" ? `<span class="e-ends" aria-hidden="true"><span>${esc(t("Cool"))}</span><span>${esc(t("Warm"))}</span></span>` : ""}</label>`).join("")}
      </div>
      <button type="button" class="btn sm e-save" data-e-act="preset-save" ${off}>${esc(t("Save as preset"))}</button>`;
    lastTarget = "";
    renderTarget();
    markPreset();
  }
  function syncSliders() {
    const col = color();
    for (const k of ED_COLOR_KEYS) {
      const input = $(`[data-e-color="${k}"]`, pane);
      if (!input) continue;
      input.value = edCol(col, k);
      input.disabled = !model();
      $(`[data-e-val="${k}"]`, pane).textContent = edFmtVal(k, edCol(col, k));
    }
  }
  function markPreset() {
    const col = color();
    const hit = presetList().find(([, p]) => edSameColor(p, col))?.[0];
    const sel = $("#e-looks", pane);
    // Not while the open list previews a look: the select keeps the look it had, so Enter on another one is a change.
    if (sel && !lookPv && sel.value !== (hit || "")) sel.value = hit || "";
    if (sel) sel.disabled = !model();
  }

  /* 0.1.46: the looks dropdown. While its list is open, a row reached with the keyboard is shown on the preview
     (the model's colors are put back when the list closes without a pick); a pick is one edit, one undo step.
     With the list closed and the dropdown focused, Up / Down pick the previous / next look; a run of them is one
     undo step. */
  let lookPv = null;      // { orig: the colors before the preview, segs }
  let lookRun = { at: -1e9, len: -1 }; // the last Up / Down pick: when, and the undo length it left
  const lookSel = () => $("#e-looks", pane);
  const lookOf = (k) => presetList().find(([key]) => key === k)?.[1];
  const putColors = (m, list) => m.colors.forEach((col, i) => { for (const k of Object.keys(col)) delete col[k]; Object.assign(col, list[i]); });
  function lookPreview(k) {
    const m = model(), p = lookOf(k);
    if (!m || !p) return;
    if (!lookPv) lookPv = { orig: structuredClone(m.colors), segs: target().segs };
    putColors(m, structuredClone(lookPv.orig));
    emSetColor(m, lookPv.segs, { ...ED_LOOK_ZERO, ...p });
    P.applyColor();
    syncSliders();
  }
  function lookUnpreview(apply) {
    if (!lookPv) return;
    const m = model(), pv = lookPv;
    lookPv = null;
    if (!m) return;
    putColors(m, pv.orig);
    if (apply) { P.applyColor(); syncSliders(); markPreset(); }
  }
  function lookPick(k, run) {
    const p = lookOf(k), m = model(), segs = target().segs;
    if (!p || !m) return;
    const n0 = m.undo.length, now = performance.now();
    const merge = run && now - lookRun.at < 1500 && n0 === lookRun.len;
    if (!P.edit("color", (mm) => emSetColor(mm, segs, { ...ED_LOOK_ZERO, ...p }))) return; // a preset without Sharpness, Temperature or Glow (older, or a look) sets them to 0
    if (merge && m.undo.length > n0) m.undo.splice(n0, 1);
    lookRun = run ? { at: now, len: m.undo.length } : { at: -1e9, len: -1 };
  }
  pane.addEventListener("change", (e) => {
    if (e.target !== lookSel()) return;
    const k = e.target.value;
    lookUnpreview(false);
    lookPick(k, false);
    markPreset();
  });
  edLookHook = {
    sel: lookSel,
    key(e) { // a key on the closed, focused dropdown
      const keys = presetList().map(([k]) => k), cur = keys.indexOf(lookSel().value);
      const next = e.code === "ArrowDown" ? (cur < 0 ? 0 : Math.min(keys.length - 1, cur + 1)) : (cur < 0 ? keys.length - 1 : Math.max(0, cur - 1));
      if (next !== cur) { lookPick(keys[next], true); markPreset(); }
    },
    act(k) { if (k) lookPreview(k); },
    closed() { lookUnpreview(true); },
    del(name) { deletePreset(name).catch(fail); },
  };
  async function savePreset() {
    const m = model();
    if (!m) return;
    const col = { ...color() }; // the target's values as they are now, before the prompt
    const name = await askText(t("Name this color preset"), "", t("Save preset"));
    if (!name) return;
    const low = name.toLowerCase();
    if (ED_PRESETS.some((p) => p.name.toLowerCase() === low || t(p.name).toLowerCase() === low)) return flash(t("A built-in preset already has that name"), true);
    if (!Array.isArray(S.settings.color_presets)) S.settings.color_presets = [];
    const list = S.settings.color_presets;
    const p = { name, ...Object.fromEntries(ED_COLOR_KEYS.map((k) => [k, Math.round(edCol(col, k) * 1000) / 1000])) };
    const i = list.findIndex((x) => x?.name?.toLowerCase() === low);
    if (i >= 0) list[i] = p; else list.push(p);
    saveSettings();
    renderColor();
    flash(t('Saved preset "{name}"', { name }));
  }
  async function deletePreset(name) {
    if (!(await ask(t('Delete the color preset "{name}"?', { name }), t("Delete")))) return;
    const list = S.settings.color_presets;
    const i = Array.isArray(list) ? list.findIndex((x) => x?.name === name) : -1;
    if (i < 0) return;
    list.splice(i, 1);
    saveSettings();
    renderColor();
  }

  /** Notes about what the preview cannot play as exported. */
  function audioNotes() {
    const m = model(), notes = [];
    if (!m) return notes;
    if (P.trackState === "loading") notes.push(t("Loading the audio tracks…"));
    if (P.trackState === "failed") notes.push(t("The separate tracks cannot be previewed, so you hear the clip's own audio. The export still uses your levels."));
    const srcs = [...P.src.values()];
    if (srcs.some((s) => s.kind === "file" && s.state === "failed")) notes.push(t("Some added audio cannot be played in the preview. The export still includes it."));
    const boosted = (s) => { const row = m.audio.find((r) => r.id === (s.rowId || s.id)); return row && (row.volume > 1.005 || row.gains.some((g) => g > 1.005)); };
    if (srcs.some((s) => s.state === "on" && !s.gain && boosted(s))) notes.push(t("The preview plays levels above 100% at 100%. The export uses the real levels."));
    return notes;
  }

  function renderExport() {
    const m = model(), job = S.exports?.[c.id];
    // A draft ("Edit together") can always be exported: unchanged, it saves the clips joined as they are.
    const changed = !!m && (emChanged(m) || !!c.draft);
    const lines = [];
    if (!m) lines.push(t("Loading the editor…"));
    else if (!emChanged(m) && c.draft) lines.push(t("Saves the clips joined, as they are."));
    else if (!changed) lines.push(t("No changes yet."));
    else {
      lines.push(edReplaceOn() && !c.draft ? t("The clip will be {len} long.", { len: fmtDurMs(emResultDuration(m)) }) : t("New clip, {len} long.", { len: fmtDurMs(emResultDuration(m)) }));
      lines.push(emIsPlainCut(m) ? t("Quick cut, no quality loss.") : t("Re-encodes the video."));
      if (emToOpts(m).audio.length && m.audio.length > 1) lines.push(t("The audio is mixed into one track."));
      if (m.canvas) lines.push(t("The picture is {w} × {h}.", { w: m.canvas.width, h: m.canvas.height }));
    }
    // Replace instead of saving beside the original. A project asks which of its clips go when it is exported.
    const rep = edReplaceOn();
    const repNote = !rep ? "" : c.draft ? t("You choose which clips go to the Recycle Bin when you export.") : t("The original goes to the Recycle Bin. The export takes its name and its place.");
    // 0.1.46 (workspace.js): the summary, the progress and the two buttons are the pane's footer on every tab;
    // what has `e-exp-opt` shows on the Export tab only.
    const html = `<div class="e-title">${esc(t("Export"))}</div>
      <div class="small e-exp-sum">${lines.map(esc).join(" ")}</div>
      ${audioNotes().map((n) => `<div class="small e-note e-exp-opt">${esc(n)}</div>`).join("")}
      <label class="check e-replace e-exp-opt"><input type="checkbox" data-e-replace ${rep ? "checked" : ""} ${job ? "disabled" : ""}><span>${esc(c.draft ? t("Replace clips of the project") : t("Replace the original clip"))}</span></label>
      ${repNote ? `<div class="small e-replace-note e-exp-opt">${esc(repNote)}</div>` : ""}
      ${/* P 0.1.41 (projects.js): the bar with what it does and Cancel; after a failed or stalled export its message and "Show the log". */
        job ? (typeof projExpProgHtml === "function" ? projExpProgHtml(job, c.id) : `<div class="e-prog"><div class="small e-prog-txt">${esc(t("Exporting… {pct}%", { pct: job.pct.toFixed(0) }))}</div><div class="progress"><i style="width:${job.pct}%"></i></div></div>`)
          : typeof projExpFailHtml === "function" ? projExpFailHtml(c.id) : ""}
      <button class="btn primary wide" data-v="export" ${job || !changed ? "disabled" : ""}><span>${esc(rep && !c.draft ? t("Export and replace") : t("Export"))}</span></button>
      <button class="btn wide" data-v="export-blur" ${job || !S.blur || !m ? "disabled" : ""}>${ICON.blur}<span>${esc(changed ? t("Export and render with Blur") : t("Render with Blur"))}</span></button>
      <div class="e-exp-opt e-exp-rules">${typeof rulesExportHtml === "function" ? rulesExportHtml(c, !!job) : "" /* rules.js: "then run a render rule" */}</div>
      ${S.blur ? "" : `<div class="small e-exp-opt">${esc(t("blur-cli.exe not found. Set its path under Blur."))}</div>`}`;
    if (html === lastExport) return;
    lastExport = html;
    $(".e-export", pane).innerHTML = html;
  }
  function paintProgress() {
    const job = S.exports?.[c.id];
    const bar = $(".e-prog i", pane), txt = $(".e-prog-txt", pane);
    if (!job || !bar || !txt) return renderExport();
    bar.style.width = `${job.pct}%`;
    const line = typeof projExpText === "function" ? projExpText(job) : t("Exporting… {pct}%", { pct: job.pct.toFixed(0) }); // P 0.1.41
    if (typeof chromeText === "function") chromeText(txt, line); else txt.textContent = line; // 0.1.46: the text node's data, no node replaced
  }

  pane.addEventListener("change", (e) => {
    if (!e.target.matches?.("[data-e-replace]")) return;
    edSetReplace(e.target.checked);
    e.target.blur(); // Space goes on playing the video, not ticking the box
    renderExport();
  });
  pane.addEventListener("input", (e) => {
    const d = e.target.dataset, m = model();
    if (!("eColor" in d) || !m) return;
    // One undo step per drag. The target is fixed for the whole gesture.
    if (colorDrag === false) { emSnapshot(m); colorDrag = target().segs; }
    const x = +e.target.value;
    emSetColor(m, colorDrag, { [d.eColor]: x });
    P.applyColor();
    $(`[data-e-val="${d.eColor}"]`, pane).textContent = edFmtVal(d.eColor, x);
    markPreset();
    renderTarget();
    renderExport();
  });
  // Sliders are inputs, and app.js skips player keys while an input has focus.
  pane.addEventListener("change", (e) => {
    if (e.target.type !== "range") return;
    e.target.blur();
    if (colorDrag === false) return;
    colorDrag = false;
    P.refreshTimeline(); // the timeline may mark segments that have a color
    P.edSync("color");
  });
  pane.addEventListener("click", async (e) => {
    const btn = e.target.closest("[data-e-delpreset],[data-e-act],.e-speed");
    if (!btn) return;
    const d = btn.dataset;
    try {
      if (d.eAct === "color-all") {
        const shown = { ...ED_LOOK_ZERO, ...color() }; // its Sharpness, Temperature, Glow too, also when off (0.1.41, 0.1.45)
        P.edit("color", (m) => emSetColor(m, null, shown));
      } else if ("eDelpreset" in d) await deletePreset(d.eDelpreset);
      else if (d.eAct === "preset-save") await savePreset();
      else if (btn === speedBtn && model()) {
        if (speedBtn.classList.contains("open")) return edPopClose?.();
        const ps = pieceSp(); // 0.1.41: with pieces selected, their own speed
        if (ps.n > 0) edOpenPop({ anchor: speedBtn, host: root, value: ps.speed, items: TL_SPEEDS, onPick: (s) => P.tl?.setPieceSpeed?.(s) });
        else edOpenPop({ anchor: speedBtn, host: root, value: model().speed, onPick: (s) => P.edit("speed", (m) => { m.speed = s; }) });
      }
    } catch (err) { fail(err); }
  });

  P.onEdit = (on) => { if (on) renderExport(); };

  /** The header's icon buttons: their tooltips name the current shortcut, the fullscreen one shows its state. */
  function renderHead() {
    const full = P.isEditorFull(), fsBtn = $(".e-fsbtn", root); // root: workspace.js moves it into the top row
    const label = full ? t("Leave fullscreen") : t("Fullscreen editor");
    fsBtn.innerHTML = full ? ED_ICON_UNFULL : PICON.full;
    fsBtn.setAttribute("aria-label", label);
    fsBtn.setAttribute("aria-pressed", String(full));
    fsBtn.title = edTip(label, "fullscreen");
    fsBtn.classList.toggle("on", full);
  }
  /** The color target or the shown values changed without an edit (new selection, the playhead crossed a segment). */
  function colorTarget() {
    if (!model() || colorDrag !== false) return; // never move a slider under the pointer
    syncSliders();
    markPreset();
    renderTarget();
  }

  /** Called by P.edSync after any change of the model or the preview audio. */
  function sync(what) {
    renderSpeed();
    if (what === "load") renderColor(); // enables the controls once the model exists
    else colorTarget();
    renderExport();
  }

  renderSpeed();
  renderColor();
  renderExport();
  renderHead();
  return {
    renderExport, paintProgress, sync,
    selChanged: () => { colorTarget(); renderSpeed(); }, // the Speed section follows the selected pieces (0.1.41)
    // Only the whole-clip target follows the playhead.
    playheadSeg: () => { if (!target().segs) colorTarget(); },
    fullChanged: renderHead,
    keysChanged: () => { renderHead(); P.refreshTimeline(); }, // the timeline's tooltips show the shortcuts too
  };
}

/* ---------- adding audio files ---------- */
let edDropSub = null; // Promise of the drag-and-drop subscriptions, made once
/*
 * Files dropped on the open editor (0.1.40, CONTRACT_0140 point 1). edHandleDrop(paths, position) is what the Tauri
 * listener calls (the mock / a test may call it too; `position` in physical pixels as Tauri gives it):
 *   - pictures and GIFs anywhere, and a video dropped ON THE PLAYER: on the picture (media.js medTakeDrop);
 *   - a video anywhere else (the timeline, V1, the rows, the Library list, the empty editor): P's
 *     projDropClips(paths, at) in ui/projects.js imports it and adds it to the project (a single clip becomes one);
 *     `at` = the V1 time under the drop, or undefined (then at the end). To place the new pieces P calls the model's
 *     emPlaceNew(m, first, at) (ui/editmodel.js) after emAddParts / P.addSources appended them: `first` = the display
 *     index of the first new piece; free placement as in 0.1.36; one undo step is P's (emSnapshot before).
 *   - an audio file: an audio clip, on the audio row it was dropped on, else on a new row;
 *   - anything else: a short flash naming it.
 * While dragging, the drop hint says what happens where the pointer is (edDropHint).
 */
const ED_AUDIO_EXT = ["mp3", "wav", "ogg", "oga", "opus", "flac", "m4a", "aac", "wma", "aif", "aiff"];
const edExtOf = (p) => { const x = /\.([a-z0-9]+)$/i.exec(String(p || "").split(/[?#]/)[0]); return x ? x[1].toLowerCase() : ""; };
/** "video", "image", "gif", "audio" or null, from a file's name. */
const edDropKind = (p) => (typeof medKindOfPath === "function" && medKindOfPath(p)) || (ED_AUDIO_EXT.includes(edExtOf(p)) ? "audio" : null);
/** Where a drop at client (x, y) lands: { onPlayer, lane: the timeline row's id or null, at: V1 time or undefined }. */
function edDropPlace(x, y) {
  const box = document.querySelector("#modal .player"), r = box?.getBoundingClientRect();
  const onPlayer = !!r && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
  const laneEl = document.elementFromPoint?.(x, y)?.closest?.("#modal [data-tl-lane]") || null;
  const lane = laneEl ? laneEl.dataset.tlLane : null;
  let at;
  if (lane === "video") { try { at = S.player?.tl?.timeAtClient?.(x, y) ?? undefined; } catch { at = undefined; } }
  return { onPlayer, lane, laneKind: laneEl?.dataset.tlKind || null, at };
}
/** The drop hint's text for `paths` at client (x, y) (the paths come with drag-enter). */
function edDropHint(paths, x, y) {
  const kinds = (paths || []).map(edDropKind), pl = edDropPlace(x, y);
  if (kinds.includes("video") && !pl.onPlayer) return t("Add to the timeline");
  if (kinds.some((k) => k === "image" || k === "gif" || (k === "video" && pl.onPlayer))) return t("Put on the picture");
  if (kinds.includes("audio")) return t("Add as audio");
  return t("Drop audio files to add them as new tracks");
}
async function edHandleDrop(paths, position) {
  if (!S.open) return;
  paths = (Array.isArray(paths) ? paths : []).filter((p) => typeof p === "string" && p);
  if (!paths.length) return;
  // A project with no clips yet (projOpenEmpty: no player): videos become its first clips; nothing else fits there yet.
  if (!S.player) {
    if (!document.querySelector("#modal .pj-empty")) return;
    const videos = paths.filter((p) => edDropKind(p) === "video"), rest = paths.filter((p) => edDropKind(p) !== "video");
    if (rest.length) flash(t("{name} cannot be added.", { name: edBaseName(rest[0]) }), true);
    if (videos.length && typeof projDropClips === "function") { try { await projDropClips(videos, undefined); } catch (e) { console.error("Glint: adding the dropped clips failed", e); fail(e); } }
    return;
  }
  const k = window.devicePixelRatio || 1, x = position ? position.x / k : NaN, y = position ? position.y / k : NaN;
  const pl = Number.isFinite(x) ? edDropPlace(x, y) : { onPlayer: false, lane: null, at: undefined };
  const odd = paths.filter((p) => !edDropKind(p));
  if (odd.length) flash(odd.length > 1 ? t("{name} and {n} more files cannot be added.", { name: edBaseName(odd[0]), n: odd.length - 1 }) : t("{name} cannot be added.", { name: edBaseName(odd[0]) }), true);
  paths = paths.filter((p) => edDropKind(p));
  // Pictures and GIFs (and a video dropped on the player) go on the picture (media.js).
  if (paths.length && typeof medTakeDrop === "function") paths = medTakeDrop(paths, position);
  const videos = paths.filter((p) => edDropKind(p) === "video"), audio = paths.filter((p) => edDropKind(p) === "audio");
  if (videos.length) {
    if (typeof projDropClips === "function") { try { await projDropClips(videos, pl.at); } catch (e) { console.error("Glint: adding the dropped clips failed", e); fail(e); } }
    else flash(t("{name} cannot be added.", { name: edBaseName(videos[0]) }), true);
  }
  // Audio: on the audio row it was dropped on (0.1.39), else a new row; the clip starts where it was dropped.
  if (audio.length) {
    let at;
    try { if (Number.isFinite(x) && S.player?.tl?.timeAtClient) at = S.player.tl.timeAtClient(x, y) ?? undefined; } catch { at = undefined; }
    edAddAudio(audio, at, pl.laneKind === "track" || pl.laneKind === "lane" ? pl.lane : null);
  }
}
function edDropInit() {
  if (edDropSub || !window.__TAURI__?.event?.listen) return;
  const listen = window.__TAURI__.event.listen;
  const open = () => !!(S.open && S.player);
  let dragPaths = [];
  const mark = (on, position) => {
    const o = $("#modal .e-drop");
    if (!o) return;
    if (o.hidden === on) o.hidden = !on;
    const b = o.querySelector(".e-drop-box"), k = window.devicePixelRatio || 1;
    if (on && b && position) { const txt = edDropHint(dragPaths, position.x / k, position.y / k); if (b.textContent !== txt) b.textContent = txt; }
  };
  edDropSub = Promise.all([
    listen("tauri://drag-enter", ({ payload }) => { dragPaths = Array.isArray(payload?.paths) ? payload.paths : []; mark(open(), payload?.position); }),
    listen("tauri://drag-over", ({ payload }) => mark(open(), payload?.position)),
    listen("tauri://drag-leave", () => mark(false)),
    listen("tauri://drag-drop", ({ payload }) => {
      mark(false);
      // Also a project with no clips yet (no player): edHandleDrop gives it the dropped videos.
      if (open() || (S.open && document.querySelector("#modal .pj-empty"))) edHandleDrop(payload?.paths, payload?.position);
    }),
  ]).catch((e) => { edDropSub = null; console.error("Glint: could not subscribe to file drops", e); });
}

/** Adds each file as a new audio row of the open clip, entering edit mode first if needed (`lane`: onto that audio row). */
async function edAddAudio(paths, at, lane) {
  const P = S.player;
  if (!P || !S.open) return;
  try {
    const m = await (P.editing ? P.whenModel() : P.setEdit(true));
    if (!m || S.player !== P || !P.alive()) return;
    const added = [], errors = [];
    for (const path of paths) {
      const name = edBaseName(path);
      let info;
      try { info = await invoke("editor_audio_info", { path }); }
      catch (e) {
        console.warn("Glint: editor_audio_info refused", path, e);
        errors.push(t("Could not add {name}: {error}", { name, error: String(e?.message ?? e) }));
        continue;
      }
      if (S.player !== P || !P.alive() || P.model !== m) return; // the clip was closed or changed meanwhile
      if (!info || !(+info.duration > 0)) { errors.push(t("{name} has no audio.", { name })); continue; }
      const row = P.addFileRow(info, at);
      // Dropped on an audio row (0.1.40): it lies there, not on a row of its own.
      if (lane && emMoveClip(P.model, row.id, { lane })) { P.refreshTimeline(); P.edSync("file"); }
      added.push(row.title);
    }
    if (errors.length) flash(errors.length > 1 ? `${errors[0]} ${t("{n} more files could not be added.", { n: errors.length - 1 })}` : errors[0], true);
    else if (added.length === 1) flash(t("Added {name} as a new audio track", { name: added[0] }));
    else if (added.length > 1) flash(t("Added {n} audio tracks", { n: added.length }));
  } catch (e) {
    console.error("Glint: adding audio failed", e);
    fail(e);
  }
}

/** The timeline's "Add audio": a file picker, then the same path as a drop. */
async function edPickAudio() {
  try {
    const path = await invoke("pick_audio_file");
    if (path) await edAddAudio([path]);
  } catch (e) {
    console.error("Glint: pick_audio_file failed", e);
    fail(e);
  }
}

/* ---------- export ---------- */
let edListening = null; // Promise of the "export-progress" subscription, made once
function edListen() {
  edListening ||= window.__TAURI__.event.listen("export-progress", ({ payload: p }) => {
    const job = S.exports?.[p?.id];
    if (!job) return;
    job.pct = clamp(+p.pct || 0, 0, 100);
    // P 0.1.41: what it is doing ("join" while a project's clips are joined, else "export"), and that step's own percent.
    job.stage = typeof p.stage === "string" ? p.stage : "";
    job.stagePct = Number.isFinite(+p.stage_pct) && p.stage_pct != null ? +p.stage_pct : null;
    if (S.open === p.id && S.player?.ed) S.player.ed.paintProgress();
  });
  return edListening;
}

/** Exports the open clip's edit as a new clip, then opens it (and starts a Blur render if asked). */
/** The question before an export of a timeline with gaps (0.1.40): resolves to "back", "export" or null (closed).
 *  Built like app.js's ask() (#confirm, Enter / Esc) with its own two buttons. */
function edAskGap(gaps) {
  return new Promise((resolve) => {
    const el = $("#confirm");
    const what = gaps.count === 1 ? t("The timeline has a gap ({sec} s).", { sec: (Math.round(gaps.total * 10) / 10).toFixed(1) }) : t("The timeline has {n} gaps, {sec} s in all.", { n: gaps.count, sec: (Math.round(gaps.total * 10) / 10).toFixed(1) });
    el.innerHTML = `<div class="confirm-box"><p>${esc(what + " " + t("It will be black and silent in the export."))}</p><div class="confirm-row"><button class="btn" data-c="back">${esc(t("Go back"))}</button><button class="btn primary" data-c="export">${esc(t("Export with black"))}</button></div></div>`;
    el.hidden = false;
    const done = (v) => { el.hidden = true; el.innerHTML = ""; el.onclick = null; askKey = null; resolve(v); };
    askKey = (e) => { if (e.code === "Enter" || e.code === "NumpadEnter") done("export"); else if (e.code === "Escape") done("back"); };
    el.onclick = (e) => { const b = e.target.closest("[data-c]"); if (b) done(b.dataset.c); else if (e.target === el) done(null); };
  });
}
async function edExport(withBlur) {
  const id = S.open, P = S.player;
  if (!id || !P) return;
  if (!S.exports) S.exports = {};
  if (S.exports[id]) return;
  const m = P.model;
  if (!m) return flash(t("The editor is still loading"));
  if (!emChanged(m) && !P.draft) { // a draft is always exported (unchanged: the clips joined as they are)
    if (!withBlur) return flash(t("Nothing to export yet"));
    P.setEdit(false); // show the Blur panel
    return viewerAction("blur");
  }
  // A gap on the main video row (0.1.40): ask first. Go back = nothing is exported, the playhead goes to the first gap
  // and it is selected; Export with black = as before.
  const gaps = emGapStats(m);
  if (gaps.count) {
    const go = await edAskGap(gaps);
    if (S.open !== id || S.player !== P) return;
    if (go !== "export") {
      if (go === "back" && gaps.first) { P.seek(gaps.first.at, false, true); P.tl?.select?.({ row: "video", seg: gaps.first.seg }); }
      return;
    }
  }
  // Rows by role (0.1.44): the export needs every clip's tracks (track_map). Wait for the ones still loading.
  if (emRowsMode(m) === "roles" && typeof P.whenTracks === "function" && !P.whenTracks.ready()) {
    flash(t("Waiting for the clips' audio tracks…"));
    const ok = await P.whenTracks(20000);
    if (S.open !== id || S.player !== P) return;
    if (!ok) return flash(t("The audio tracks of a clip could not be read, so the export cannot place its sound. Try again in a moment."), true);
  }
  const opts = emToOpts(m);
  // "Replace the original": one clip replaces itself; a project asks which of its clips go (projects.js).
  let repIds = [];
  if (edReplaceOn()) {
    if (P.draft) {
      const picked = typeof projAskReplace === "function" ? await projAskReplace(id) : [];
      if (picked === null || S.open !== id || S.player !== P) return; // cancelled, or closed meanwhile: no export
      repIds = picked;
    } else repIds = [id];
  }
  // Text, shapes and drawings go as transparent PNGs of the clip's size (overlays.js; the backend removes them after the export).
  if (P.ov && (m.overlays || []).some((o) => o.kind !== "blur")) {
    try {
      const overlays = await P.ov.exportPngs();
      if (overlays.length) opts.overlays = overlays;
    } catch (e) {
      console.error("Glint: the overlay pictures could not be made", e);
      return fail(e);
    }
    if (S.open !== id || S.player !== P || S.exports[id]) return; // closed, or exported meanwhile
  }
  try { await edListen(); }
  catch (e) { edListening = null; console.error("Glint: could not subscribe to export-progress; exporting without a progress bar.", e); }
  S.exports[id] = { pct: 0, stage: "", stagePct: null, cancelling: false };
  if (S.exportErr) delete S.exportErr[id]; // P 0.1.41: the last failure's message goes with a new export
  P.ed?.renderExport();
  console.info("Glint: export_clip", id, opts);
  let meta;
  try {
    // P-HOOK (0.1.38): a virtual project (sources, no file) is exported by agent A's export_project, its opts in project time.
    meta = await invoke(P.clipMeta?.virtual ? "export_project" : "export_clip", { id, opts });
  } catch (e) {
    const job = S.exports[id];
    delete S.exports[id];
    // P 0.1.41 (projects.js): cancelled = a flash; failed or stalled = the message stays in the Export section.
    const kept = typeof projExpFailed === "function";
    if (kept) projExpFailed(id, e, job);
    if (S.open === id) S.player?.ed?.renderExport();
    if (job?.cancelling) return;
    console.error("Glint: export_clip failed", id, opts, e);
    return kept ? undefined : fail(e);
  }
  delete S.exports[id];
  if (P.model) P.exportedState = edState(P.model); // R2 #1 (0.1.41): this edit is kept now, closing need not ask (edCloseAsk)
  const stillOpen = S.open === id;
  const fromProject = !!P.draft; // a project stays open after an export (the new clip is in the library)
  let gone = [], kept = [];
  if (repIds.length) {
    // The player lets go of the original's file first: a file that is still open cannot go to the Recycle Bin.
    if (stillOpen && !fromProject) closeClip();
    try {
      const r = await invoke("replace_clips", { id: meta.id, ids: repIds, adopt: !fromProject });
      if (r && r.clip && r.clip.id) meta = r.clip; // with the original's name, date and favourite now
      gone = (r && r.replaced) || [];
      kept = (r && r.failed) || [];
    } catch (e) {
      console.error("Glint: replace_clips failed", meta.id, repIds, e);
      kept = [t(String(e?.message ?? e))];
    }
    if (gone.length) S.clips = S.clips.filter((x) => !gone.includes(x.id));
  }
  replaceClip(meta);
  if (stillOpen && !fromProject) openClip(meta.id); // the source clip is still (or again) open: show the result
  // A project stays open: its Export box was drawn while the export ran (buttons off) and nothing else redraws it.
  else if (S.open === id) S.player?.ed?.renderExport();
  if (gone.length && typeof projRenderPanel === "function") projRenderPanel();
  if (kept.length) flash(t("Exported as a new clip, but not everything could be replaced: {reason}", { reason: kept.join("; ") }), true);
  else if (gone.length && fromProject) flash(t("Exported as a new clip. {clips} moved to the Recycle Bin.", { clips: plural(gone.length) }));
  else if (gone.length) flash(withBlur ? t("Exported and replaced. Rendering with Blur…") : t("Exported. The original is in the Recycle Bin."));
  else flash(withBlur ? t("Exported. Rendering with Blur…") : stillOpen && fromProject ? t("Exported as a new clip. It is in your library.") : stillOpen ? t("Exported as a new clip") : t("Export finished: {title}", { title: meta.title }));
  // rules.js: the render rule picked in the Export box (if any) now runs on the exported clip.
  if (!kept.length && typeof rulesAfterExport === "function") { try { rulesAfterExport(meta, withBlur); } catch (e) { console.error("Glint: rulesAfterExport failed", e); } }
  if (!withBlur) return;
  const show = () => { if (S.open === meta.id) $("#v-blur").innerHTML = blurPanel(meta); else render(); };
  S.jobs[meta.id] = { pct: 0, state: "running", message: "" };
  show();
  try { await invoke("blur_clip", { id: meta.id }); }
  catch (e) { S.jobs[meta.id] = { pct: 0, state: "error", message: String(e) }; show(); fail(e); }
}

/**
 * R2 #1 (0.1.41): a close the user asked for (Esc, Close, a click beside the viewer). A single library clip with an edit
 * that was not exported asks first, because closing drops that edit (a project keeps its edit and never asks).
 */
async function edCloseAsk() {
  const P = S.player, m = P?.model, id = S.open;
  if (P && !P.draft && m && !S.exports?.[id] && emChanged(m) && P.exportedState !== edState(m)) {
    if (!(await ask(t("Close this clip? Its edit is not saved: only an export keeps it."), t("Close and lose the edit")))) return;
    if (S.open !== id || S.player !== P) return;
  }
  closeClip();
}

/** Close the viewer. Every way out (Close, Done on a draft, Esc, the backdrop, the window's close
 *  button) comes here. A project saves its edit first and stays in the Editor's list (projects.js):
 *  closing never asks and never deletes. */
function closeClip() {
  const m = $("#modal");
  if (S.player && typeof projFlush === "function") projFlush(S.player);
  if (S.player) { S.player.destroy(); S.player = null; }
  m.hidden = true;
  m.innerHTML = "";
  S.open = null;
  render();
}

function replaceClip(meta) {
  if (!meta) return;
  // A draft is held by join.js, never in S.clips; any other clip arriving while a draft is open is an export of it.
  if (meta.draft && typeof joinKeepDraft === "function") return joinKeepDraft(meta);
  if (typeof joinDraftExported === "function") joinDraftExported(meta);
  const i = S.clips.findIndex((c) => c.id === meta.id);
  if (i >= 0) S.clips[i] = meta; else S.clips.push(meta);
  render();
}

async function viewerAction(act) {
  const id = S.open;
  const c = typeof clipById === "function" ? clipById(id) : S.clips.find((x) => x.id === id);
  const m = $("#modal");
  if (!c) return closeClip();
  // A draft ("Edit together") is not a file to keep: no copy, folder, favourite, delete or Blur.
  if (c.draft && ["copy", "reveal", "fav", "delete", "blur", "blur-page"].includes(act)) return;
  try {
    if (act === "close") edCloseAsk(); // R2 #1: an unexported edit of a single clip asks first
    else if (act === "blur-done") closeClip();
    else if (act === "prev" || act === "next") {
      // The clip before or after this one. Not while editing: an edit is not thrown away by an arrow.
      if (c.draft || (S.player && S.player.editing) || !$("#confirm").hidden) return;
      const to = viewerNeighbours(id)[act];
      if (!to) return;
      // In fullscreen the next clip takes the screen over: the player that goes does not leave fullscreen.
      edKeepFull = document.fullscreenElement?.id === "modal";
      try { openClip(to); } finally { edKeepFull = false; }
    }
    else if (act === "blur-page") { S.page = "blur"; closeClip(); } // closeClip() draws the page
    else if (act === "copy") { await invoke("copy_clip", { id }); flash(t("Clip copied. Paste it anywhere.")); }
    else if (act === "reveal") await invoke("reveal_clip", { id });
    else if (act === "fav") { const meta = await invoke("set_favorite", { id, favorite: !c.favorite }); replaceClip(meta); $('[data-v="fav"] span', m).textContent = meta.favorite ? t("Remove favorite") : t("Add to favorites"); }
    else if (act === "delete") {
      const used = typeof projUsedNote === "function" ? projUsedNote([c.id]) : ""; // projects.js (0.1.38): projects that use this clip
      if (!(await ask(t('Move "{title}" to the Recycle Bin?', { title: c.title }) + (used ? " " + used : ""), t("Move to Recycle Bin")))) return;
      closeClip();
      await invoke("delete_clip", { id });
      S.clips = S.clips.filter((x) => x.id !== id);
      render();
      flash(t("Clip moved to the Recycle Bin"));
    }
    else if (act === "edit" || act === "trim") S.player.setEdit(!S.player.editing);
    else if (act === "edit-done" || act === "trim-cancel") S.player.setEdit(false);
    else if (act === "shortcuts") S.player.openKeys();
    else if (act === "editor-full") S.player.editorFull();
    else if (act === "library-window") {
      // A window cannot come to the front over a fullscreen editor: the editor leaves fullscreen first.
      if (document.fullscreenElement) { try { await document.exitFullscreen(); } catch {} }
      await invoke("open_library_window");
    }
    else if (act === "export") await edExport(false);
    else if (act === "export-blur") await edExport(true);
    else if (act === "blur") { S.jobs[id] = { pct: 0, state: "running", message: "" }; $("#v-blur").innerHTML = blurPanel(c); await invoke("blur_clip", { id }); }
  } catch (e) {
    if (act === "blur") { S.jobs[id] = { pct: 0, state: "error", message: String(e) }; $("#v-blur").innerHTML = blurPanel(c); }
    fail(e);
  }
}
