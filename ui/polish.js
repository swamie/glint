"use strict";
// Small library and settings fixes from 0.1.11 (loaded after app.js):
// clip card pictures, the folder shown on a card, the Windows 10 capture hint,
// and the taskbar icon in the theme's colour.

/** Inline style of a card's picture: the whole frame, centred, letterboxed when it is not 16:9.
 *  Inline so a custom theme's `.thumb` rules cannot crop or shift it. */
const THUMB_IMG_STYLE = "position:absolute;inset:0;width:100%;height:100%;object-fit:contain;object-position:50% 50%;background:#000;display:block";

/** The folder name Glint files a game's clips under (mirror of util::game_folder in Rust). */
function gameFolderName(game) {
  const g = String(game || "");
  if (!g.trim().replace(/^\.+|\.+$/g, "").trim()) return "Other";
  const cleaned = g.replace(/[<>:"/\\|?*\u0000-\u001f\u007f-\u009f]/g, " ").split(/\s+/).filter(Boolean).join(" ").replace(/^[. ]+|[. ]+$/g, "");
  return cleaned ? [...cleaned].slice(0, 60).join("") : "Clip";
}

/** A card shows the clip's folder only when it says something the game name does not. */
function folderDiffers(c) {
  const f = String(c.folder || "").toLowerCase();
  if (!f) return false;
  return f !== String(c.game || "").toLowerCase() && f !== gameFolderName(c.game).toLowerCase();
}

/** Capture page: Windows 10 cannot hide the Graphics Capture border. */
function win10CaptureHint() {
  const build = (S.hardware && S.hardware.windows_build) || 0;
  if (!build || build >= 22000) return "";
  return `<br><span style="color:var(--warn)">${t("On Windows 10, Windows Graphics Capture draws a yellow border around the screen. Auto uses Desktop Duplication instead.")}</span>`;
}

/* ---------- taskbar icon in the theme's colour ---------- */

/** A CSS colour (any syntax the browser knows) as "#rrggbb", or "" if it is not one. */
function cssColorHex(value) {
  const v = String(value || "").trim();
  if (!v) return "";
  const probe = document.createElement("i");
  probe.style.color = "";
  probe.style.color = v;
  if (!probe.style.color) return "";
  probe.style.display = "none";
  document.body.append(probe);
  const rgb = getComputedStyle(probe).color;
  probe.remove();
  const m = rgb.match(/rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)/);
  if (!m) return "";
  return "#" + m.slice(1, 4).map((n) => Math.round(Math.min(255, Math.max(0, +n))).toString(16).padStart(2, "0")).join("");
}

let iconSent = "";
/** Read the accent the theme and custom CSS give, then tell the backend (and remember it for the next start). */
function syncThemeIcon() {
  if (!S.settings || !window.__TAURI__) return;
  const on = !!S.settings.icon_theme;
  const css = getComputedStyle(document.documentElement);
  const accent = on ? cssColorHex(css.getPropertyValue("--accent")) : "";
  const accent2 = on ? cssColorHex(css.getPropertyValue("--accent-2")) : "";
  const key = `${on}|${accent}|${accent2}`;
  if (key === iconSent) return;
  iconSent = key;
  invoke("set_app_icon", { on, accent, accent2 }).catch(() => {});
  if (on && accent && (S.settings.icon_accent !== accent || (S.settings.icon_accent_2 || "") !== accent2)) {
    S.settings.icon_accent = accent;
    S.settings.icon_accent_2 = accent2;
    saveSettings();
  }
}

// Every theme change (picker, custom CSS, settings loaded at start) goes through applyGlintTheme.
{
  const apply = window.applyGlintTheme;
  window.applyGlintTheme = (id, css) => {
    apply(id, css);
    // After this tick: the new <style> elements are in place and S.settings is current.
    setTimeout(syncThemeIcon, 0);
  };
}
document.addEventListener("change", (e) => {
  if (e.target?.dataset?.set === "icon_theme") setTimeout(syncThemeIcon, 0);
});
