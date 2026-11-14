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

/* ---------- presets (0.1.20): the tiles, the "Customise colours" box, the right-click menu ---------- */
// The model is in themes.js (glintPreset* functions); here it is drawn and wired.
// theme_colors / custom_css are the mirror of the active preset (see themes.js), so the
// picker below keeps reading cpColors() and S.settings.custom_css.

const prActive = () => (S.settings ? glintPresetActive(S.settings) : null);
const prAutoName = () => t("My theme");
let prBySearch = false; // the box was opened by the settings search, not by the user

/** The tile grid: the built-in themes, the user's presets, then "+ New preset". */
function presetTilesHtml() {
  const s = S.settings;
  const act = prActive();
  const builtin = window.GLINT_THEMES.map((th) => `<button class="theme${!act && (s.theme || "glint") === th.id ? " active" : ""}" data-theme="${th.id}" style="background:${th.vars.panel || "#0a0e16"};color:${th.vars.text || "#e8edf6"}"><i style="background:${th.vars.accent || "#62b6f7"}"></i>${esc(th.name)}</button>`).join("");
  const mine = (s.theme_presets || []).map((p) => {
    const c = (v) => glintEffectiveColor(p.base, p.colors, v);
    return `<button class="theme tp-tile${act && act.id === p.id ? " active" : ""}" data-tp="${esc(p.id)}" style="background:${c("panel")};color:${c("text")}" title="${esc(t("Your preset. Right-click to rename, duplicate or delete it"))}"><i style="background:${c("accent")}"></i><span class="tp-name">${esc(p.name)}</span></button>`;
  }).join("");
  return `<div class="themes">${builtin}${mine}<button class="theme tp-new" data-tp-new title="${esc(t("Make a new preset from the look you have now"))}"><b>+</b>${t("New preset")}</button></div>`;
}

/** The summary's hint: what an edit changes. */
function prSummaryHint() {
  const act = prActive();
  if (act) return t("Editing {name}", { name: act.name });
  const th = window.GLINT_THEMES.find((x) => x.id === S.settings.theme) || window.GLINT_THEMES[0];
  return t("{theme} stays as it is: a change makes a new preset", { theme: th.name });
}

/** The "Customise colours" box, drawn by renderSettings under the tiles: the colour rows, Reset all, the custom CSS. */
function colorRowsHtml() {
  const user = cpColors();
  const any = Object.keys(user).length > 0;
  const css = S.settings.custom_css || "";
  const rows = GLINT_CUSTOM_VARS.map((v) => {
    const [name, hint] = cpLabel(v);
    return row(esc(name), esc(hint), cpCtl(v, user));
  }).join("");
  const open = prBySearch || uiPref("glint.colorsOpen", false);
  return `<details class="css-box cp-box"${open ? " open" : ""}>
    <summary><span class="name">${t("Customise colours")}</span><span class="hint" data-cp-sum>${esc(prSummaryHint())}</span></summary>
    <div class="cp-head"><div class="txt"><div class="name">${t("Colours")}</div><div class="hint">${t("Your own colours on top of the preset's theme. Custom CSS still wins over them.")}</div></div>
      <button class="btn sm" data-cp-reset-all${any ? "" : " disabled"}>${t("Reset all")}</button></div>
    <div class="cp-rows">${rows}</div>
    <div class="cp-css">
      <div class="cp-css-title"><span class="name">${t("Custom theme (CSS)")}</span><span class="hint" data-cp-css-state>${css ? t("In use") : t("Make your own look")}</span></div>
      <div class="css-help">
        <p>${t("The easiest way to get your own theme is to ask an AI for it. Press Copy AI prompt, paste it into Claude, ChatGPT or any other AI, add a line about the look you want, then paste the CSS it gives you into the box below.")}</p>
        <div class="css-actions">
          <button class="btn sm primary" data-act="css-prompt">${ICON.copy}${t("Copy AI prompt")}</button>
          <button class="btn sm" data-cp-css-clear${css ? "" : " disabled"}>${t("Clear")}</button>
        </div>
        <p class="hint">${t("Your CSS is applied on top of the theme picked above. Colours are variables, so one line recolours everything:")} <code>:root { --accent: #ff4fd8; }</code></p>
      </div>
      <textarea class="code" data-cp-css spellcheck="false" placeholder=":root { --accent: #ff4fd8; }&#10;.card h3 { font-style: italic; }">${esc(css)}</textarea>
    </div>
  </details>`;
}

/** Apply what the mirror holds now, save, and bring the tiles and the box's hints up to date in place. */
function prApplied(rerender) {
  window.applyGlintTheme(S.settings.theme, S.settings.custom_css);
  saveSettings(!!rerender);
  if (rerender) return;
  const grid = document.querySelector("#view .themes");
  if (grid) grid.outerHTML = presetTilesHtml();
  const sum = document.querySelector("[data-cp-sum]");
  if (sum) sum.textContent = prSummaryHint();
  const css = S.settings.custom_css || "";
  const st = document.querySelector("[data-cp-css-state]");
  if (st) st.textContent = css ? t("In use") : t("Make your own look");
  const clr = document.querySelector("[data-cp-css-clear]");
  if (clr) clr.disabled = !css;
  cpRefreshRows();
}

/** A colour or CSS change: into the active preset, or a new "My theme" one when a built-in theme is active. */
function prEdit(patch) {
  const { created, preset } = glintPresetEdit(S.settings, patch, prAutoName());
  prApplied(false);
  if (created) flash(t("Saved as a new preset: {name}", { name: preset.name }));
}

/** A built-in tile was clicked (app.js's data-theme case). */
function prPickBuiltin(id) {
  glintPresetPickBuiltin(S.settings, id);
  prApplied(true);
}

/** Ask a name and make a preset; `from` = { base, colors, css } (what is on screen when left out). */
async function prNew(from, suggested) {
  const name = await askText(t("Name the new preset"), glintPresetName(S.settings, suggested || prAutoName()), t("Create"));
  if (!name) return;
  glintPresetCreate(S.settings, name, from || glintPresetFromScreen(S.settings));
  prApplied(true);
}

/** Right-click on a theme tile (app.js's contextmenu listener). True when it was one. */
function prMenu(e) {
  const tile = e.target.closest?.("#view .themes .theme");
  if (!tile || tile.matches("[data-tp-new]")) return false;
  menuIds = [];
  const id = tile.dataset.tp || tile.dataset.theme;
  $("#menu").innerHTML = tile.dataset.tp
    ? mItem("tp-rename", t("Rename…"), ICON.edit, id) + mItem("tp-dup", t("Duplicate"), ICON.copy, id) + "<hr>" + mItem("tp-del", t("Delete"), ICON.trash, id, "danger")
    : mItem("tp-dup", t("Duplicate"), ICON.copy, id);
  placeMenu(e.clientX, e.clientY);
  return true;
}

/** The menu's items (app.js's menuAction passes "tp-*" here). */
async function prMenuAction(m, id) {
  const s = S.settings;
  const p = (s.theme_presets || []).find((x) => x.id === id);
  if (m === "tp-rename" && p) {
    const name = await askText(t('Rename the preset "{name}"', { name: p.name }), p.name, t("Rename"));
    if (!name || name === p.name) return;
    glintPresetRename(s, id, name);
    return prApplied(true);
  }
  if (m === "tp-dup") {
    const th = window.GLINT_THEMES.find((x) => x.id === id);
    const src = p || (th && { base: th.id, colors: {}, css: "" });
    if (!src) return;
    return prNew({ base: src.base, colors: src.colors, css: src.css }, t("{name} copy", { name: p ? p.name : th.name }));
  }
  if (m === "tp-del" && p) {
    if (!(await ask(t('Delete the preset "{name}"? This cannot be undone.', { name: p.name }), t("Delete")))) return;
    glintPresetDelete(s, id);
    return prApplied(true);
  }
}

/** Startup (app.js's refreshState): old custom colours / CSS become a preset; saved only if something changed. */
function prMigrate() {
  if (S.settings && glintPresetMigrate(S.settings, prAutoName())) saveSettings(false);
}

/** The settings search (app.js's applySettingsFilter): a match inside the box opens it; clearing the search puts it back. */
function cpSearchOpen(q) {
  const box = document.querySelector("#view .cp-box");
  if (!box) return;
  const hit = !!q && box.textContent.toLowerCase().includes(q);
  if (hit) { if (!box.open) { prBySearch = true; box.open = true; } }
  else if (prBySearch) { prBySearch = false; box.open = uiPref("glint.colorsOpen", false); }
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

/**
 * Save `map` as the custom colours and apply it (the taskbar icon follows through applyGlintTheme).
 * 0.1.20: into the active preset, or a new one when a built-in theme is active (prEdit).
 * Updated in place, never redrawn: a click on another swatch must not be lost.
 */
function cpSave(map) {
  prEdit({ colors: map });
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
  custom: null,    // cpOpen's `custom` while it picks something other than a theme variable
};

/**
 * Open the picker for theme variable `v` beside `anchor` (an element, or anything with getBoundingClientRect).
 * `custom` (0.1.18, folder colours) picks a colour that is not a theme variable:
 * { name, hex, reset, resetTitle, onPreview(hex), onKeep(hex), onCancel() }; the window's colours are left alone.
 */
function cpOpen(v, anchor, custom) {
  if (CP.el) cpClose(true);
  CP.custom = custom || null;
  if (custom) {
    CP.v = "";
    CP.start = {};
    CP.reset = glintNormHex(custom.reset || "") || "#808080";
    CP.hex = glintNormHex(custom.hex || "") || CP.reset;
  } else {
    const start = cpColors();
    const without = { ...start };
    delete without[v];
    CP.v = v;
    CP.start = start;
    CP.reset = glintEffectiveColor(cpTheme(), without, v);
    CP.hex = glintEffectiveColor(cpTheme(), start, v) || CP.reset || "#808080";
  }
  CP.hsv = glintRgbToHsv(glintHexToRgb(CP.hex));
  const name = custom ? String(custom.name || "") : cpLabel(v)[0];
  const resetTitle = custom ? String(custom.resetTitle || "") : t("The theme's own colour: {hex}", { hex: CP.reset });
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
    <div class="cpk-foot"><button type="button" class="btn sm" data-cpk="reset" title="${esc(resetTitle)}"><i class="cpk-dot" style="background:${esc(CP.reset)}"></i>${t("Reset")}</button><span class="cpk-tip">${t("Enter keeps it, Esc cancels")}</span></div>`;
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
    if (CP.custom) { if (typeof CP.custom.onPreview === "function") CP.custom.onPreview(CP.hex); return; }
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
  const custom = CP.custom;
  CP.custom = null;
  if (custom) {
    const fn = keep ? custom.onKeep : custom.onCancel;
    if (typeof fn === "function") fn(CP.hex);
    return;
  }
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
  if (all && !all.disabled) return cpSave({});
  const tp = e.target.closest?.("[data-tp]");
  if (tp) { if (glintPresetPick(S.settings, tp.dataset.tp)) prApplied(true); return; }
  if (e.target.closest?.("[data-tp-new]")) return void prNew().catch((err) => fail(err));
  const clr = e.target.closest?.("[data-cp-css-clear]");
  if (clr && !clr.disabled) {
    const box = document.querySelector("[data-cp-css]");
    if (box) box.value = "";
    return prEdit({ css: "" });
  }
});

// The custom CSS box: kept when it loses focus, like the other settings fields.
document.addEventListener("change", (e) => {
  if (!e.target.matches?.("[data-cp-css]")) return;
  if (e.target.value === (S.settings.custom_css || "")) return;
  prEdit({ css: e.target.value });
});

// The open state of "Customise colours" is remembered, except while the settings search opened it.
// ("toggle" does not bubble, hence capture. It also fires when the box is drawn already open.)
document.addEventListener("toggle", (e) => {
  if (!e.target.classList?.contains("cp-box")) return;
  if (e.target.open && prBySearch) return;
  prBySearch = false;
  setUiPref("glint.colorsOpen", e.target.open);
}, true);
window.addEventListener("resize", () => cpClose(true));
