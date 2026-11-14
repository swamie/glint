"use strict";
// Settings → Appearance → Custom colours, and Glint's own colour picker.
// Loaded before app.js: app.js globals (S, $, row, esc, saveSettings, ...) are
// used only inside functions. The colour maths is in themes.js (glint* functions).
//
// Data: S.settings.theme_colors = { variable: "#rrggbb" } (only what the user set).
// The picker previews on the whole window while it is open (glintPreviewColors);
// Enter or a click outside keeps the colour, Esc puts the old one back.
// Names here start with `cp` / `CP`; markup uses `cp-` / `cpk-` classes and `data-cp*` attributes.

/** Name and hint of each colour a user can set (GLINT_CUSTOM_VARS in themes.js). */
function cpLabel(v) {
  return ({
    accent: [t("Accent"), t("Buttons, highlights and progress bars")],
    bg: [t("Background"), t("Behind everything. Panels and lines follow it unless you set them")],
    panel: [t("Panels"), t("The sidebar, sections and boxes")],
    raised: [t("Buttons and cards"), t("Raised parts such as buttons and clip cards")],
    line: [t("Lines"), t("Borders and dividers")],
    text: [t("Text"), t("Softer texts follow it unless you set them")],
    muted: [t("Secondary text"), t("Hints and labels")],
  })[v] || [v, ""];
}

const cpColors = () => glintCleanColors((S.settings && S.settings.theme_colors) || {});
const cpTheme = () => (S.settings && S.settings.theme) || "glint";

/** The "Custom colours" rows, drawn by renderSettings under the theme tiles. */
function colorRowsHtml() {
  const user = cpColors();
  const any = Object.keys(user).length > 0;
  const rows = GLINT_CUSTOM_VARS.map((v) => {
    const [name, hint] = cpLabel(v);
    return row(esc(name), esc(hint), cpCtl(v, user));
  }).join("");
  return `<div class="cp-head"><div class="txt"><div class="name">${t("Custom colours")}</div><div class="hint">${t("Your own colours on top of the theme. They stay when you pick another theme. Custom CSS still wins over them.")}</div></div>
      <button class="btn sm" data-cp-reset-all${any ? "" : " disabled"}>${t("Reset all")}</button></div>
    <div class="cp-rows">${rows}</div>`;
}

/** The hex text and the round swatch button of one row. */
function cpCtl(v, user) {
  const hex = glintEffectiveColor(cpTheme(), user, v);
  const mine = v in user;
  const [name] = cpLabel(v);
  const label = t("Change the colour: {name} ({hex})", { name, hex });
  return `<span class="cp-hex${mine ? " mine" : ""}" data-cp-hex="${v}">${mine ? esc(t("Custom")) + " · " : ""}${esc(hex)}</span><button type="button" class="cp-swatch${mine ? " mine" : ""}" data-cp-var="${v}" style="background:${esc(hex)}" title="${esc(label)}" aria-label="${esc(label)}"></button>`;
}

/** Bring the rows up to date in place (no redraw, so a click on another swatch is not lost). */
function cpRefreshRows() {
  const user = cpColors();
  for (const v of GLINT_CUSTOM_VARS) {
    const btn = document.querySelector(`.cp-swatch[data-cp-var="${v}"]`);
    if (!btn) continue;
    // Updated in place: replacing the button under a pressed pointer would swallow its click.
    const hex = glintEffectiveColor(cpTheme(), user, v);
    const mine = v in user;
    const label = t("Change the colour: {name} ({hex})", { name: cpLabel(v)[0], hex });
    btn.style.background = hex;
    btn.title = label;
    btn.setAttribute("aria-label", label);
    btn.classList.toggle("mine", mine);
    const txt = document.querySelector(`[data-cp-hex="${v}"]`);
    if (txt) { txt.textContent = (mine ? t("Custom") + " · " : "") + hex; txt.classList.toggle("mine", mine); }
  }
  const all = document.querySelector("[data-cp-reset-all]");
  if (all) all.disabled = !Object.keys(user).length;
}

/** Save `map` as the custom colours and apply it (the taskbar icon follows through applyGlintTheme). */
function cpSave(map, rerender) {
  S.settings.theme_colors = glintCleanColors(map);
  window.applyGlintTheme(S.settings.theme, S.settings.custom_css);
  saveSettings(!!rerender);
  if (!rerender) cpRefreshRows();
}

/* ---------- the picker ---------- */

const CP = {
  el: null,        // the open picker
  v: "",           // the variable being picked
  start: {},       // theme_colors when it opened (Esc goes back to these)
  reset: "",       // the colour without the user's own for this variable
  hsv: { h: 0, s: 0, v: 0 },
  hex: "",
  drag: null,      // "sv" | "hue" while the pointer holds one
  raf: 0,
};

function cpOpen(v, anchor) {
  if (CP.el) cpClose(true);
  const start = cpColors();
  const without = { ...start };
  delete without[v];
  CP.v = v;
  CP.start = start;
  CP.reset = glintEffectiveColor(cpTheme(), without, v);
  CP.hex = glintEffectiveColor(cpTheme(), start, v) || CP.reset || "#808080";
  CP.hsv = glintRgbToHsv(glintHexToRgb(CP.hex));
  const [name] = cpLabel(v);
  const el = document.createElement("div");
  el.className = "cpk";
  el.setAttribute("role", "dialog");
  el.setAttribute("aria-label", t("Pick a colour: {name}", { name }));
  el.innerHTML = `<div class="cpk-head"><b>${esc(name)}</b><span class="cpk-cmp"><i class="cpk-old" style="background:${esc(CP.hex)}" title="${esc(t("Before"))}"></i><i class="cpk-new"></i></span></div>
    <div class="cpk-sv" tabindex="0" role="slider" aria-label="${esc(t("Saturation and brightness"))}"><i class="cpk-sv-thumb"></i></div>
    <div class="cpk-hue" tabindex="0" role="slider" aria-label="${esc(t("Hue"))}" aria-valuemin="0" aria-valuemax="360"><i class="cpk-hue-thumb"></i></div>
    <div class="cpk-fields">
      <label class="cpk-f cpk-f-hex"><span>${t("Hex")}</span><input class="field cpk-hex" type="text" maxlength="7" spellcheck="false" autocomplete="off"></label>
      ${["r", "g", "b"].map((c) => `<label class="cpk-f"><span>${c.toUpperCase()}</span><input class="field cpk-n" data-cpk-c="${c}" type="number" min="0" max="255" step="1"></label>`).join("")}
    </div>
    <div class="cpk-foot"><button type="button" class="btn sm" data-cpk="reset" title="${esc(t("The theme's own colour: {hex}", { hex: CP.reset }))}"><i class="cpk-dot" style="background:${esc(CP.reset)}"></i>${t("Reset")}</button><span class="cpk-tip">${t("Enter keeps it, Esc cancels")}</span></div>`;
  document.body.appendChild(el);
  CP.el = el;
  cpPlace(el, anchor);
  cpBind(el);
  cpShow(null);
  el.querySelector(".cpk-sv").focus();
}

/** Beside the swatch (to its left: the swatch is at the right edge), kept inside the window. */
function cpPlace(el, anchor) {
  const r = anchor ? anchor.getBoundingClientRect() : { left: innerWidth / 2, right: innerWidth / 2, top: innerHeight / 3, bottom: innerHeight / 3 };
  const w = el.offsetWidth || 260, h = el.offsetHeight || 330;
  let x = r.left - w - 10;
  if (x < 8) x = Math.min(innerWidth - w - 8, r.right + 10);
  const y = Math.min(Math.max(8, r.top - 40), innerHeight - h - 8);
  el.style.left = `${Math.max(8, x)}px`;
  el.style.top = `${Math.max(8, y)}px`;
}

/** Show the current colour everywhere in the picker except `skip` (the field being typed in), and preview it. */
function cpShow(skip) {
  const el = CP.el;
  if (!el) return;
  const { h, s, v } = CP.hsv;
  const rgb = glintHexToRgb(CP.hex);
  el.querySelector(".cpk-sv").style.backgroundColor = glintRgbToHex(glintHsvToRgb({ h, s: 1, v: 1 }));
  const thumb = el.querySelector(".cpk-sv-thumb");
  thumb.style.left = `${s * 100}%`;
  thumb.style.top = `${(1 - v) * 100}%`;
  thumb.style.background = CP.hex;
  el.querySelector(".cpk-sv").setAttribute("aria-valuetext", t("Saturation {s} %, brightness {v} %", { s: Math.round(s * 100), v: Math.round(v * 100) }));
  el.querySelector(".cpk-hue-thumb").style.left = `${(h / 360) * 100}%`;
  el.querySelector(".cpk-hue").setAttribute("aria-valuenow", String(Math.round(h)));
  el.querySelector(".cpk-new").style.background = CP.hex;
  const hexIn = el.querySelector(".cpk-hex");
  if (hexIn !== skip) hexIn.value = CP.hex;
  for (const n of el.querySelectorAll(".cpk-n")) if (n !== skip) n.value = rgb[n.dataset.cpkC];
  cpPreview();
}

/** The whole window in the colour being picked, once per frame at most. */
function cpPreview() {
  if (CP.raf) return;
  CP.raf = requestAnimationFrame(() => {
    CP.raf = 0;
    if (!CP.el) return;
    window.glintPreviewColors({ ...CP.start, [CP.v]: CP.hex });
  });
}

function cpSetHsv(hsv, skip) {
  CP.hsv = { h: Math.min(360, Math.max(0, hsv.h)), s: Math.min(1, Math.max(0, hsv.s)), v: Math.min(1, Math.max(0, hsv.v)) };
  CP.hex = glintRgbToHex(glintHsvToRgb(CP.hsv));
  cpShow(skip);
}

function cpSetHex(hex, skip) {
  const n = glintNormHex(hex);
  if (!n) return false;
  CP.hex = n;
  const hsv = glintRgbToHsv(glintHexToRgb(n));
  // A grey has no hue and black no saturation: keep the strip and the square where they were.
  if (hsv.s === 0 || hsv.v === 0) hsv.h = CP.hsv.h;
  if (hsv.v === 0) hsv.s = CP.hsv.s;
  CP.hsv = hsv;
  cpShow(skip);
  return true;
}

function cpBind(el) {
  const sv = el.querySelector(".cpk-sv"), hue = el.querySelector(".cpk-hue");
  const fromSv = (e) => { const r = sv.getBoundingClientRect(); cpSetHsv({ h: CP.hsv.h, s: (e.clientX - r.left) / r.width, v: 1 - (e.clientY - r.top) / r.height }); };
  const fromHue = (e) => { const r = hue.getBoundingClientRect(); cpSetHsv({ ...CP.hsv, h: ((e.clientX - r.left) / r.width) * 360 }); };
  for (const [part, fn, name] of [[sv, fromSv, "sv"], [hue, fromHue, "hue"]]) {
    part.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      part.focus();
      CP.drag = name;
      try { part.setPointerCapture(e.pointerId); } catch {}
      fn(e);
    });
    part.addEventListener("pointermove", (e) => { if (CP.drag === name) fn(e); });
    const stop = () => { if (CP.drag === name) CP.drag = null; };
    part.addEventListener("pointerup", stop);
    part.addEventListener("pointercancel", stop);
  }
  el.querySelector(".cpk-hex").addEventListener("input", (e) => {
    const v = e.target.value.trim();
    if (/^#?([0-9a-f]{6}|[0-9a-f]{3})$/i.test(v)) cpSetHex(v, e.target);
  });
  el.querySelector(".cpk-hex").addEventListener("blur", (e) => { if (CP.el) e.target.value = CP.hex; });
  for (const n of el.querySelectorAll(".cpk-n")) {
    n.addEventListener("input", () => {
      if (n.value === "") return;
      const rgb = glintHexToRgb(CP.hex);
      rgb[n.dataset.cpkC] = Math.round(Math.min(255, Math.max(0, +n.value || 0)));
      cpSetHex(glintRgbToHex(rgb), n);
    });
    n.addEventListener("blur", () => { if (CP.el) n.value = glintHexToRgb(CP.hex)[n.dataset.cpkC]; });
  }
  el.querySelector('[data-cpk="reset"]').addEventListener("click", () => { cpSetHex(CP.reset); el.querySelector(".cpk-sv").focus(); });
}

/** Arrow keys on the square and the strip; Enter keeps; Esc cancels. Returns true when the key was the picker's. */
function cpKey(e) {
  if (!CP.el) return false;
  if (e.key === "Escape") { cpClose(false); return true; }
  if (e.key === "Enter") {
    const hexIn = CP.el.querySelector(".cpk-hex");
    if (document.activeElement === hexIn) cpSetHex(hexIn.value);
    cpClose(true);
    return true;
  }
  const on = document.activeElement;
  const step = e.shiftKey ? 10 : 1;
  const d = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, 1], ArrowDown: [0, -1] }[e.key];
  if (!d || !CP.el.contains(on)) return false;
  if (on.classList.contains("cpk-sv")) { cpSetHsv({ h: CP.hsv.h, s: CP.hsv.s + (d[0] * step) / 100, v: CP.hsv.v + (d[1] * step) / 100 }); return true; }
  if (on.classList.contains("cpk-hue")) { cpSetHsv({ ...CP.hsv, h: CP.hsv.h + (d[0] || d[1]) * step }); return true; }
  return false;
}

/** Close the picker: `keep` saves the colour (the theme's own colour means "not set"), else the old colours come back. */
function cpClose(keep) {
  const el = CP.el;
  if (!el) return;
  CP.el = null;
  CP.drag = null;
  if (CP.raf) { cancelAnimationFrame(CP.raf); CP.raf = 0; }
  el.remove();
  if (keep) {
    const map = { ...CP.start };
    if (CP.hex === glintNormHex(CP.reset)) delete map[CP.v];
    else map[CP.v] = CP.hex;
    const changed = JSON.stringify(glintCleanColors(map)) !== JSON.stringify(CP.start);
    if (changed) cpSave(map, false);
    else window.applyGlintTheme(S.settings.theme, S.settings.custom_css);
  } else {
    window.applyGlintTheme(S.settings.theme, S.settings.custom_css);
  }
  document.querySelector(`.cp-swatch[data-cp-var="${CP.v}"]`)?.focus();
}

// Registered before app.js's own listeners (this file loads first), so the picker sees its keys first.
window.addEventListener("keydown", (e) => {
  if (!CP.el) return;
  if (cpKey(e)) { e.preventDefault(); e.stopImmediatePropagation(); return; }
  // Typing in the picker's fields: no keybind or page shortcut may fire.
  if (CP.el.contains(document.activeElement)) e.stopImmediatePropagation();
}, true);

// A press outside the open picker keeps the colour.
document.addEventListener("pointerdown", (e) => {
  if (CP.el && !CP.el.contains(e.target)) cpClose(true);
}, true);

document.addEventListener("click", (e) => {
  const sw = e.target.closest?.("[data-cp-var]");
  if (sw) { cpOpen(sw.dataset.cpVar, sw); return; }
  const all = e.target.closest?.("[data-cp-reset-all]");
  if (all && !all.disabled) cpSave({}, true);
});
window.addEventListener("resize", () => cpClose(true));
