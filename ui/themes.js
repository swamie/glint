"use strict";
// Colour themes, shared by the main window and the clip overlay.
// A theme is a set of values for the CSS variables in style.css's :root.
// "glint" is empty: it is the stylesheet's own colours.
//
// 0.1.16: the user's custom colours (settings.theme_colors, variable -> "#rrggbb",
// set with the picker in colorpick.js) go on top of the theme and under the
// custom CSS. What the user did not set but must follow is derived here
// (glintDeriveColors): a readable accent-ink and an accent-2 for a new accent,
// panels and lines for a new background, softer texts for a new text colour.
// The colour maths is plain functions, testable in Node (module.exports at the end).
const GLINT_W = typeof window !== "undefined" ? window : globalThis;

GLINT_W.GLINT_THEMES = [
  { id: "glint", name: "Glint", vars: {} },
  { id: "midnight", name: "Midnight", vars: { bg: "#05060c", panel: "#090b14", raised: "#0e1120", "raised-2": "#151a2e", line: "#171c30", "line-2": "#272e4c", muted: "#8a90ad", dim: "#565c7a", accent: "#7c8cff", "accent-2": "#a3aeff", "accent-ink": "#070a24" } },
  { id: "abyss", name: "Abyss", vars: { bg: "#000000", panel: "#06070a", raised: "#0c0d11", "raised-2": "#14161c", line: "#15171d", "line-2": "#272b35", muted: "#8a909c", dim: "#565b66", accent: "#5fd4ff", "accent-2": "#9ae4ff", "accent-ink": "#001a24" } },
  { id: "crimson", name: "Crimson", vars: { bg: "#0c0709", panel: "#120a0d", raised: "#190e12", "raised-2": "#22141a", line: "#26161c", "line-2": "#3e242d", muted: "#a58b90", dim: "#6e565b", accent: "#ff5c6c", "accent-2": "#ff8a95", "accent-ink": "#2a0408" } },
  { id: "ember", name: "Ember", vars: { bg: "#0c0906", panel: "#120d09", raised: "#19130d", "raised-2": "#221a12", line: "#261d14", "line-2": "#3e3021", muted: "#a59a8b", dim: "#6e6456", accent: "#ff9a3d", "accent-2": "#ffb870", "accent-ink": "#2a1300" } },
  { id: "gold", name: "Gold", vars: { bg: "#0b0a06", panel: "#110f09", raised: "#18150d", "raised-2": "#211d12", line: "#252014", "line-2": "#3c3521", muted: "#a59f8b", dim: "#6e6956", accent: "#f2c94c", "accent-2": "#f7da80", "accent-ink": "#2a2000" } },
  { id: "emerald", name: "Emerald", vars: { bg: "#060b08", panel: "#09110c", raised: "#0d1812", "raised-2": "#132119", line: "#15251c", "line-2": "#223c2e", muted: "#86a593", dim: "#557060", accent: "#4ade80", "accent-2": "#86efac", "accent-ink": "#03200e" } },
  { id: "lagoon", name: "Lagoon", vars: { bg: "#050b0c", panel: "#081112", raised: "#0c1819", "raised-2": "#122123", line: "#142628", "line-2": "#213d40", muted: "#84a3a5", dim: "#546e70", accent: "#2dd4bf", "accent-2": "#7ee8d9", "accent-ink": "#02201c" } },
  { id: "amethyst", name: "Amethyst", vars: { bg: "#09070d", panel: "#0e0a14", raised: "#140f1c", "raised-2": "#1c1527", line: "#20182c", "line-2": "#342848", muted: "#9a8fae", dim: "#655a78", accent: "#b28cff", "accent-2": "#cdb4ff", "accent-ink": "#170733" } },
  { id: "rose", name: "Rose", vars: { bg: "#0c070a", panel: "#120a0f", raised: "#190e15", "raised-2": "#22141d", line: "#26161f", "line-2": "#3e2434", muted: "#a58b9a", dim: "#6e5664", accent: "#ff7ab8", "accent-2": "#ffa3cf", "accent-ink": "#2a0418" } },
  { id: "nord", name: "Nord", vars: { bg: "#20242d", panel: "#252a34", raised: "#2b313c", "raised-2": "#343b48", line: "#353c49", "line-2": "#465064", text: "#eceff4", muted: "#a3adbf", dim: "#707b90", accent: "#88c0d0", "accent-2": "#a9d6e2", "accent-ink": "#1c2733" } },
  { id: "mono", name: "Mono", vars: { bg: "#0a0a0a", panel: "#101010", raised: "#161616", "raised-2": "#1e1e1e", line: "#202020", "line-2": "#343434", muted: "#9a9a9a", dim: "#666666", accent: "#f2f2f2", "accent-2": "#ffffff", "accent-ink": "#0a0a0a" } },
];

const GLINT_THEME_VARS = ["bg", "panel", "raised", "raised-2", "line", "line-2", "text", "muted", "dim", "accent", "accent-2", "accent-ink"];

/** style.css's own :root colours (the "glint" theme), the base every theme fills in. */
const GLINT_BASE_COLORS = {
  bg: "#070a10", panel: "#0a0e16", raised: "#0f141e", "raised-2": "#151b27", line: "#19212f", "line-2": "#263043",
  text: "#e8edf6", muted: "#8591a5", dim: "#556073", accent: "#62b6f7", "accent-2": "#8ccbff", "accent-ink": "#051322",
};

/** The colours a user can pick in Settings → Appearance → Custom colours, in that order (names in colorpick.js). */
const GLINT_CUSTOM_VARS = ["accent", "bg", "panel", "raised", "line", "text", "muted"];

/* ---------- colour maths ---------- */

/** "#rgb" / "#rrggbb" (any case, with or without #) -> { r, g, b } 0..255, or null. */
function glintHexToRgb(hex) {
  let h = String(hex || "").trim().replace(/^#/, "");
  if (/^[0-9a-f]{3}$/i.test(h)) h = h.replace(/./g, (c) => c + c);
  if (!/^[0-9a-f]{6}$/i.test(h)) return null;
  const n = parseInt(h, 16);
  return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255 };
}

/** { r, g, b } (rounded and held in 0..255) -> "#rrggbb". */
function glintRgbToHex({ r, g, b }) {
  const c = (x) => Math.round(Math.min(255, Math.max(0, +x || 0))).toString(16).padStart(2, "0");
  return "#" + c(r) + c(g) + c(b);
}

/** A colour as "#rrggbb" (lower case), or "" if it is not a hex colour. */
function glintNormHex(hex) {
  const rgb = glintHexToRgb(hex);
  return rgb ? glintRgbToHex(rgb) : "";
}

/** { r, g, b } 0..255 -> { h: 0..360, s: 0..1, v: 0..1 }. A grey keeps h = 0. */
function glintRgbToHsv({ r, g, b }) {
  r /= 255; g /= 255; b /= 255;
  const max = Math.max(r, g, b), min = Math.min(r, g, b), d = max - min;
  let h = 0;
  if (d > 0) {
    if (max === r) h = ((g - b) / d) % 6;
    else if (max === g) h = (b - r) / d + 2;
    else h = (r - g) / d + 4;
    h *= 60;
    if (h < 0) h += 360;
  }
  return { h, s: max === 0 ? 0 : d / max, v: max };
}

/** { h: 0..360, s: 0..1, v: 0..1 } -> { r, g, b } 0..255 (not rounded). */
function glintHsvToRgb({ h, s, v }) {
  h = (((+h || 0) % 360) + 360) % 360;
  s = Math.min(1, Math.max(0, +s || 0));
  v = Math.min(1, Math.max(0, +v || 0));
  const c = v * s, x = c * (1 - Math.abs(((h / 60) % 2) - 1)), m = v - c;
  const [r, g, b] = h < 60 ? [c, x, 0] : h < 120 ? [x, c, 0] : h < 180 ? [0, c, x] : h < 240 ? [0, x, c] : h < 300 ? [x, 0, c] : [c, 0, x];
  return { r: (r + m) * 255, g: (g + m) * 255, b: (b + m) * 255 };
}

/** WCAG relative luminance of a hex colour, 0..1. */
function glintLuminance(hex) {
  const rgb = glintHexToRgb(hex) || { r: 0, g: 0, b: 0 };
  const lin = (c) => { c /= 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; };
  return 0.2126 * lin(rgb.r) + 0.7152 * lin(rgb.g) + 0.0722 * lin(rgb.b);
}

/** WCAG contrast ratio of two hex colours, 1..21. */
function glintContrast(a, b) {
  const la = glintLuminance(a), lb = glintLuminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** `a` moved `amount` (0..1) of the way to `b`, in sRGB. */
function glintMix(a, b, amount) {
  const x = glintHexToRgb(a) || { r: 0, g: 0, b: 0 }, y = glintHexToRgb(b) || { r: 0, g: 0, b: 0 };
  const k = Math.min(1, Math.max(0, amount));
  return glintRgbToHex({ r: x.r + (y.r - x.r) * k, g: x.g + (y.g - x.g) * k, b: x.b + (y.b - x.b) * k });
}

const glintIsDark = (hex) => glintLuminance(hex) < 0.18;
/** A surface `amount` lighter (on a dark background) or darker (on a light one). */
const glintLift = (hex, amount) => glintMix(hex, glintIsDark(hex) ? "#ffffff" : "#000000", amount);

/** The more readable of a dark and a light ink tinted with `accent`; plain black or white when neither reads well (4.5:1). */
function glintInkFor(accent) {
  const dark = glintMix(accent, "#000000", 0.86), light = glintMix(accent, "#ffffff", 0.92);
  const best = (a, b) => (glintContrast(a, accent) >= glintContrast(b, accent) ? a : b);
  const tinted = best(dark, light);
  return glintContrast(tinted, accent) >= 4.5 ? tinted : best("#000000", "#ffffff");
}

/** The theme's full palette: style.css's colours with the theme's own on top. */
function glintThemePalette(id) {
  const theme = GLINT_W.GLINT_THEMES.find((x) => x.id === id) || GLINT_W.GLINT_THEMES[0];
  return { ...GLINT_BASE_COLORS, ...theme.vars };
}

/** Only well-formed entries for known variables, as "#rrggbb". */
function glintCleanColors(colors) {
  const out = {};
  if (!colors || typeof colors !== "object") return out;
  for (const [k, v] of Object.entries(colors)) {
    const hex = glintNormHex(v);
    if (GLINT_THEME_VARS.includes(k) && hex) out[k] = hex;
  }
  return out;
}

/**
 * The variables to set on top of `palette` (a full theme palette) for the user's
 * `colors`: what they set, plus what must follow and they did not set. Returns
 * only variables that change; {} when `colors` is empty.
 */
function glintDeriveColors(palette, colors) {
  const user = glintCleanColors(colors);
  const out = { ...user };
  const has = (v) => v in user;
  const set = (v, value) => { if (!has(v)) out[v] = value; };
  // Accent: a lighter hover colour and an ink that stays readable on it.
  if (has("accent")) {
    set("accent-2", glintMix(user.accent, "#ffffff", 0.3));
    set("accent-ink", glintInkFor(user.accent));
  }
  // Background: panels, raised parts and lines step away from it, so none stays from the theme's palette.
  const bg = user.bg || palette.bg;
  if (has("bg")) {
    set("panel", glintLift(bg, 0.02));
    set("raised", glintLift(bg, 0.045));
    set("raised-2", glintLift(bg, 0.085));
    set("line", glintLift(bg, 0.09));
    set("line-2", glintLift(bg, 0.17));
  }
  if (has("raised")) set("raised-2", glintLift(user.raised, 0.04));
  if (has("line")) set("line-2", glintLift(user.line, 0.08));
  // Text: keep it readable on the background; softer texts follow it.
  let text = user.text || palette.text;
  if (!has("text") && has("bg") && glintContrast(text, bg) < 4.5) {
    text = glintIsDark(bg) ? "#e8edf6" : "#14171c";
    out.text = text;
  }
  // A new text colour, or a new background (the theme's softer texts were mixed for its own): mix them again.
  if (has("text") || out.text || has("bg")) {
    set("muted", glintMix(text, bg, 0.4));
    set("dim", glintMix(text, bg, 0.62));
  }
  if (has("muted")) set("dim", glintMix(user.muted, bg, 0.4));
  for (const k of Object.keys(out)) if (palette[k] === out[k] && !has(k)) delete out[k];
  return out;
}

/** The colour a variable has now: the user's, a derived one, or the theme's. */
function glintEffectiveColor(themeId, colors, v) {
  const pal = glintThemePalette(themeId);
  return glintDeriveColors(pal, colors)[v] || pal[v] || "";
}

/** The CSS for the user's colours over theme `themeId` ("" when none). */
function glintColorsCss(themeId, colors) {
  const d = glintDeriveColors(glintThemePalette(themeId), colors);
  const vars = GLINT_THEME_VARS.filter((v) => d[v]).map((v) => `--${v}: ${d[v]};`).join(" ");
  return vars ? `:root { ${vars} }` : "";
}

/* ---------- applying ---------- */

let glintLastTheme = "glint";
/** The custom colours in force: the settings' once they are loaded, else the ones remembered from the last run. */
function glintCurrentColors() {
  try { if (typeof S !== "undefined" && S && S.settings) return S.settings.theme_colors || {}; } catch {}
  try { return JSON.parse(localStorage.getItem("glint.colors") || "{}") || {}; } catch { return {}; }
}

/** One of the three <style> elements at the end of <head>, moved there in the order asked. */
function glintSheet(id) {
  let el = document.getElementById(id);
  if (!el) {
    el = document.createElement("style");
    el.id = id;
  }
  document.head.append(el);
  return el;
}

/**
 * Apply a theme by id, then the user's custom colours (`colors`; left out =
 * the ones in force), then the user's own CSS on top of both.
 */
GLINT_W.applyGlintTheme = (id, css, colors) => {
  const theme = GLINT_W.GLINT_THEMES.find((x) => x.id === id) || GLINT_W.GLINT_THEMES[0];
  glintLastTheme = theme.id;
  if (colors === undefined || colors === null) colors = glintCurrentColors();
  colors = glintCleanColors(colors);
  // Three <style> elements at the end of <head>: theme, colours, then the user's CSS
  // later in the cascade, so it can override both. (Inline styles on <html> would beat the user's rules.)
  const vars = GLINT_THEME_VARS.filter((v) => theme.vars[v]).map((v) => `--${v}: ${theme.vars[v]};`).join(" ");
  glintSheet("theme-css").textContent = vars ? `:root { ${vars} }` : "";
  glintSheet("colors-css").textContent = glintColorsCss(theme.id, colors);
  glintSheet("custom-css").textContent = css || "";
  // Remembered so the next start paints in the right colours before settings have loaded.
  try { localStorage.setItem("glint.theme", theme.id); localStorage.setItem("glint.css", css || ""); localStorage.setItem("glint.colors", JSON.stringify(colors)); } catch {}
};

/** Live preview while a colour is being picked: only the colours sheet changes, nothing is remembered. */
GLINT_W.glintPreviewColors = (colors) => {
  const el = document.getElementById("colors-css");
  if (el) el.textContent = glintColorsCss(glintLastTheme, colors);
};

if (typeof document !== "undefined") {
  try { GLINT_W.applyGlintTheme(localStorage.getItem("glint.theme") || "glint", localStorage.getItem("glint.css") || ""); } catch {}
}

if (typeof module !== "undefined" && module.exports) {
  module.exports = { GLINT_BASE_COLORS, GLINT_THEME_VARS, GLINT_CUSTOM_VARS, glintHexToRgb, glintRgbToHex, glintNormHex, glintRgbToHsv, glintHsvToRgb, glintLuminance, glintContrast, glintMix, glintLift, glintInkFor, glintThemePalette, glintCleanColors, glintDeriveColors, glintEffectiveColor, glintColorsCss };
}
