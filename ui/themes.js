"use strict";
// Colour themes, shared by the main window and the clip overlay.
// A theme is a set of values for the CSS variables in style.css's :root.
// "glint" is empty: it is the stylesheet's own colours.
window.GLINT_THEMES = [
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

/** Apply a theme by id, then the user's own CSS on top of it. */
window.applyGlintTheme = (id, css) => {
  const theme = window.GLINT_THEMES.find((t) => t.id === id) || window.GLINT_THEMES[0];
  // Two <style> elements at the end of <head>, theme first: the user's CSS comes
  // later in the cascade, so it can override the theme's variables. (Inline
  // styles on <html> would beat the user's rules.)
  const sheet = (id) => {
    let el = document.getElementById(id);
    if (!el) {
      el = document.createElement("style");
      el.id = id;
    }
    document.head.append(el);
    return el;
  };
  const vars = GLINT_THEME_VARS.filter((v) => theme.vars[v]).map((v) => `--${v}: ${theme.vars[v]};`).join(" ");
  sheet("theme-css").textContent = vars ? `:root { ${vars} }` : "";
  sheet("custom-css").textContent = css || "";
  // Remembered so the next start paints in the right colours before settings have loaded.
  try { localStorage.setItem("glint.theme", theme.id); localStorage.setItem("glint.css", css || ""); } catch {}
};

try { window.applyGlintTheme(localStorage.getItem("glint.theme") || "glint", localStorage.getItem("glint.css") || ""); } catch {}
