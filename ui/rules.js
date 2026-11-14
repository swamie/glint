"use strict";
// 0.1.24: Render rules. A rule is a name and a row of blocks (steps) run in order on a clip with one
// click: Color (a look), Canvas (a saved canvas layout, 0.1.27), Blur, Upscale, Compress (to N MB), GIF, Copy. Saved in S.settings.rules;
// the backend (src-tauri/src/rules.rs) runs them as jobs of kind "rule" (command rules_run).
//
// The builder looks and drags like the clip-name builder (naming.js / naming.css: its classes are reused,
// with this file's own data-rl-… attributes so naming.js never sees these blocks).
// Hooks other files call: renderRulesPage (app.js), rulesMenuItem (clip menu), rulesViewerHtml,
// rulesExportHtml and rulesAfterExport (editor.js), rulesJobLine (jobs.js).
//
// Loads before app.js: app.js globals (S, invoke, ICON, esc, the translation function, ...) are only
// touched inside functions. The pure model at the top runs in Node (tests). No local is called `t`.

/* ---------- model (pure) ---------- */
const RL_KINDS = ["color", "canvas", "blur", "upscale", "compress", "gif", "copy"];
// Why an order was changed: the same sentences as rules.rs (WHY_*).
const RL_WHY_ONCE = "Each kind of block can be used once.";
const RL_WHY_GIF = "A GIF is not a video, so only Copy can come after it.";
const RL_WHY_COPY = "Copy is always the last block.";
const RL_WHY_UNKNOWN = "An unknown block was left out.";
const RL_WHY_CANVAS = "Canvas goes first, with Color next to it (one export does both).";
const RL_MIN_MB = 5, RL_MAX_MB = 1000;
const RL_GIF_WIDTHS = [0, 720, 480, 320];
const RL_GIF_FPS = [10, 15, 20, 30, 50];
const RL_GIF_MB = [0, 10, 25, 50];
const RL_MB_QUICK = [10, 25, 50, 100, 500];

/** Mirror of rules.rs `fix_order`: { steps, why } — unknown and repeated kinds left out, Copy last,
 *  anything but Copy that came after a GIF moved before it. `why` is the first reason, or null. */
function rlFixOrder(steps) {
  let why = null;
  const note = (w) => { if (!why) why = w; };
  const out = [];
  for (const st of Array.isArray(steps) ? steps : []) {
    if (!st || !RL_KINDS.includes(st.kind)) note(RL_WHY_UNKNOWN);
    else if (out.some((o) => o.kind === st.kind)) note(RL_WHY_ONCE);
    else out.push(st);
  }
  const g = out.findIndex((s) => s.kind === "gif");
  if (g >= 0) {
    const after = out.slice(g + 1).filter((s) => s.kind !== "copy");
    if (after.length) {
      note(RL_WHY_GIF);
      const rest = out.filter((s, i) => i <= g || s.kind === "copy");
      const gi = rest.findIndex((s) => s.kind === "gif");
      rest.splice(gi, 0, ...after);
      out.length = 0;
      out.push(...rest);
    }
  }
  const c = out.findIndex((s) => s.kind === "copy");
  if (c >= 0 && c !== out.length - 1) { note(RL_WHY_COPY); out.push(...out.splice(c, 1)); }
  // 0.1.27: Canvas (and Color, made in the same export) first, in the order they had.
  if (out.some((s) => s.kind === "canvas")) {
    const front = (s) => s.kind === "canvas" || s.kind === "color";
    const group = out.filter(front);
    if (!out.slice(0, group.length).every(front)) {
      note(RL_WHY_CANVAS);
      const rest = out.filter((s) => !front(s));
      out.length = 0;
      out.push(...group, ...rest);
    }
  }
  return { steps: out, why };
}
/** The kinds that can still be added (each kind once). */
const rlFree = (steps) => RL_KINDS.filter((k) => !steps.some((s) => s.kind === k));
/** A new block of `kind`. `last` = the dialogs' last choices ({ gif, upscale } from the settings;
 *  `layout`: the saved canvas layout a new Canvas block starts with, or none). */
function rlNewStep(kind, last = {}) {
  const lay = kind === "canvas" && last.layout ? last.layout : null;
  return { kind, look: "Vivid", mb: 50, upscale: { height: 2160, ...(last.upscale || {}) }, gif: { width: 480, fps: 20, max_mb: 0, ...(last.gif || {}) }, layout: lay ? String(lay.id || "") : "", layout_name: lay ? String(lay.name || "") : "" };
}
/** The saved layout a Canvas block names, out of `list` (S.settings.canvas_presets): by id, else by name (rules.rs `find_layout`). */
function rlLayoutOf(st, list) {
  const all = Array.isArray(list) ? list : [];
  return (st?.layout && all.find((p) => p && p.id === st.layout)) || (st?.layout_name && all.find((p) => p && p.name === st.layout_name)) || null;
}
/** A saved layout with its size: "Overwatch vertical · 1080 × 1920". */
const rlLayoutLabel = (p) => `${p.name} · ${Math.round(+p.canvas?.width || 0)} × ${Math.round(+p.canvas?.height || 0)}`;
/** `steps` with block `from` moved to index `to`. */
function rlMove(steps, from, to) {
  const out = steps.slice();
  if (from < 0 || from >= out.length) return out;
  const [b] = out.splice(from, 1);
  out.splice(Math.max(0, Math.min(to, out.length)), 0, b);
  return out;
}
/** `steps` with a new block inserted at `at` (a kind already in use is not added). */
function rlInsert(steps, kind, at, last) {
  if (!RL_KINDS.includes(kind) || steps.some((s) => s.kind === kind)) return steps.slice();
  const out = steps.slice();
  out.splice(Math.max(0, Math.min(at, out.length)), 0, rlNewStep(kind, last));
  return out;
}
/** A target height in words: "4K", "1440p" (upscale.rs `label`). */
const rlHeight = (h) => (h === 2160 ? "4K" : h === 4320 ? "8K" : `${h}p`);
/** The first free "Render rule N" among `names`. */
function rlFreeName(names, base = "Render rule {n}", tr = (s, p) => s.replace("{n}", p.n)) {
  for (let n = 1; ; n++) { const name = tr(base, { n }); if (!names.includes(name)) return name; }
}

/* ---------- words (translated) ---------- */
const rlTr = (s, p) => (typeof t === "function" ? t(s, p) : String(s).replace(/\{(\w+)\}/g, (_, k) => (p && k in p ? p[k] : `{${k}}`)));
const rlKindLabel = (k) => ({ color: t("Color"), canvas: t("Canvas"), blur: t("Blur"), upscale: t("Upscale"), compress: t("Compress"), gif: t("GIF"), copy: t("Copy") }[k] || k);
/** A look's name to show: the built-in looks are translated, the user's own are not. */
const rlLookName = (name) => (typeof ED_PRESETS !== "undefined" && ED_PRESETS.some((p) => p.name === name) ? rlTr(name) : name);
/** One block in words: "Color: Vivid", "Blur", "Upscale to 4K", "Compress to 50 MB", "GIF 480 px, 20 fps", "Copy". */
function rlWords(st) {
  switch (st.kind) {
    case "color": return t("Color: {look}", { look: rlLookName(st.look) });
    case "canvas": {
      const name = rlLayoutOf(st, rlLayouts())?.name || st.layout_name;
      return name ? t("Canvas: {name}", { name }) : t("Canvas");
    }
    case "upscale": return t("Upscale to {target}", { target: rlHeight(+st.upscale?.height || 2160) });
    case "compress": return t("Compress to {n} MB", { n: st.mb });
    case "gif": {
      const g = st.gif || {};
      const size = g.width ? t("{n} px", { n: g.width }) : t("full size");
      return g.max_mb ? t("GIF {size}, {fps} fps, up to {mb} MB", { size, fps: g.fps, mb: g.max_mb }) : t("GIF {size}, {fps} fps", { size, fps: g.fps });
    }
    default: return rlKindLabel(st.kind);
  }
}
/** What a rule does: "Color: Vivid → Blur → Compress to 50 MB → Copy". */
const rlDescribe = (rule) => (rule?.steps?.length ? rule.steps.map(rlWords).join(" → ") : t("No blocks yet"));

/* ---------- state ---------- */
const RL = {
  open: undefined, // id of the rule whose builder is open on the page (null: all closed)
  sel: -1,         // its block whose options are shown (-1: none)
  why: "",         // the last reason an order was changed (shown under the row)
  whyTimer: 0,
  drag: null,      // a block being dragged (like naming.js NM.drag)
  suppress: false, // the click after a drag
  menuClips: [],   // the clips the clip menu's "Render rule" items act on
  names: new Map(),// job id -> rule name, for the flash when a job ends
  seen: new Set(), // ended rule job ids already flashed
  listening: false,
};

const rlList = () => { if (!Array.isArray(S.settings.rules)) S.settings.rules = []; return S.settings.rules; };
const rlById = (id) => rlList().find((r) => r.id === id) || null;
/** The canvas layouts saved in the editor (canvas.js "Save this layout…"). */
const rlLayouts = () => (typeof S !== "undefined" && Array.isArray(S.settings?.canvas_presets) ? S.settings.canvas_presets : []);
const rlLast = () => ({ gif: S.settings.gif, upscale: S.settings.upscale, layout: rlLayouts()[0] || null });
/** Why rule `rule` cannot run because of its Canvas block (the same words as rules.rs `check`), or "". */
function rlCanvasProblem(rule) {
  const st = rule?.steps?.find((s) => s.kind === "canvas");
  if (!st) return "";
  if (!st.layout && !st.layout_name) return t("Pick a layout for the Canvas block.");
  return rlLayoutOf(st, rlLayouts()) ? "" : t('The canvas layout "{name}" no longer exists. Pick another one in the rule.', { name: st.layout_name || st.layout });
}
/** Why rule `rule` cannot run right now (Blur missing, its canvas layout gone), or "". */
const rlCannotWhy = (rule) => (rlHasBlur(rule) && !S.blur ? t("blur-cli.exe was not found.") : rlCanvasProblem(rule));
const rlNewId = () => "r" + Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
const rlHasBlur = (rule) => !!rule?.steps?.some((s) => s.kind === "blur");
/** The looks a Color block can use: the editor's built-in ones (not "None") and the user's own. */
function rlLooks() {
  const builtin = (typeof ED_PRESETS !== "undefined" ? ED_PRESETS : []).filter((p) => p.name !== "None").map((p) => p.name);
  const mine = (typeof edUserPresets === "function" ? edUserPresets() : (S.settings.color_presets || [])).map((p) => p.name).filter((n) => !builtin.includes(n));
  return { builtin, mine };
}
function rlSave(rerender = true) {
  saveSettings(false);
  if (rerender && S.page === "rules" && !S.open) renderRulesPage();
}
function rlShowWhy(why) {
  RL.why = why ? rlTr(why) : "";
  clearTimeout(RL.whyTimer);
  if (why) RL.whyTimer = setTimeout(() => { RL.why = ""; const el = document.querySelector(".rl-why"); if (el) { el.textContent = ""; el.hidden = true; } }, 6000);
}
/** Set the open rule's blocks, fixed into an order that works (the reason is shown when it had to be fixed). */
function rlSetSteps(rule, steps, focus) {
  const fixed = rlFixOrder(steps);
  rule.steps = fixed.steps;
  rlShowWhy(fixed.why);
  if (RL.sel >= rule.steps.length) RL.sel = -1;
  rlSave(true);
  if (focus !== undefined) rlFocus(focus);
}
function rlFocus(focus) {
  const b = document.querySelector(".rl-builder");
  if (!b) return;
  const el = focus.i !== undefined ? b.querySelector(`[data-rl-i="${Math.min(focus.i, b.querySelectorAll("[data-rl-i]").length - 1)}"]`) : focus.add ? b.querySelector(`[data-rl-add="${focus.add}"]`) : null;
  (el || b.querySelector("[data-rl-i]") || b.querySelector("[data-rl-add]"))?.focus();
}

/* ---------- the page ---------- */
function renderRulesPage() {
  const list = rlList();
  if (RL.open && !rlById(RL.open)) RL.open = undefined;
  if (RL.open === undefined && list.length) RL.open = list[0].id; // the first rule is open to begin with; null = all closed by the user
  const head = `<div class="page-head">
      <div><h1>${esc(t("Render rules"))}</h1><div class="sub">${esc(t("Several steps on a clip with one click"))}</div></div>
      ${list.length ? `<div class="head-actions"><button class="btn primary" data-rlp="new">${ICON.plus}${esc(t("New rule"))}</button></div>` : ""}
    </div>`;
  const empty = `<div class="empty rl-empty"><h2>${esc(t("No render rules yet"))}</h2>
      <p>${esc(t("A render rule does several things to a clip in one go, for example a color look, Blur and a compress to 50 MB."))}</p>
      <p>${esc(t("Build one from blocks here, then run it from a clip's right-click menu, the viewer, or the editor's Export box."))}</p>
      <button class="btn primary" data-rlp="new">${ICON.plus}${esc(t("New rule"))}</button></div>`;
  $("#view").innerHTML = head + (list.length ? `<div class="rl-list">${list.map(rlCardHtml).join("")}</div>` : empty);
}

function rlCardHtml(rule, i) {
  const open = rule.id === RL.open;
  const n = rule.steps?.length || 0;
  const canvasBad = rlCanvasProblem(rule);
  const blurWarn = (rlHasBlur(rule) && !S.blur ? `<div class="small rl-bad">${esc(t("blur-cli.exe was not found."))} ${esc(t("This rule cannot run until Blur is installed."))}</div>` : "")
    + (canvasBad ? `<div class="small rl-bad">${esc(canvasBad)}</div>` : "");
  return `<div class="rl-card${open ? " open" : ""}" data-rl-card="${esc(rule.id)}">
      <button type="button" class="rl-head" data-rlp="pick" data-arg="${esc(rule.id)}" aria-expanded="${open}" title="${esc(t("Right-click for rename, duplicate, delete and order"))}">
        <span class="rl-icon">${ICON.rules}</span>
        <span class="rl-head-txt"><span class="rl-name">${esc(rule.name)}</span><span class="rl-words">${esc(rlDescribe(rule))}</span></span>
        <span class="rl-count">${esc(n === 1 ? t("1 block") : t("{n} blocks", { n }))}</span>
        <b class="rl-chev" aria-hidden="true">${open ? "▾" : "▸"}</b>
      </button>
      ${open ? `<div class="rl-body">
        <label class="rl-name-row"><span class="small">${esc(t("Name"))}</span><input class="field rl-name-in" type="text" maxlength="60" spellcheck="false" data-rl-name="${esc(rule.id)}" value="${esc(rule.name)}"></label>
        ${rlBuilderHtml(rule)}
        ${blurWarn}
        <div class="small rl-hint">${esc(t("Drag the blocks to change the order, or drag one out to remove it. Click a block for its options. Steps with nothing to do (the clip is already small enough, or already that tall) are skipped."))}</div>
      </div>` : ""}
    </div>`;
}

function rlBuilderHtml(rule) {
  const steps = rule.steps || [];
  const n = steps.length;
  const chips = steps.map((st, i) => {
    const label = rlWords(st);
    const aria = t("{block}, block {i} of {n}. Alt+Left or Alt+Right moves it, Delete removes it, Enter shows its options", { block: label, i: i + 1, n });
    // A Canvas block whose layout is gone (or never picked) is shown broken.
    const bad = st.kind === "canvas" && !rlLayoutOf(st, rlLayouts()) ? (st.layout || st.layout_name ? t("This layout no longer exists") : t("Pick a layout")) : "";
    return `<span class="nm-block rl-block k-${st.kind}${RL.sel === i ? " on" : ""}${bad ? " bad" : ""}" role="listitem" tabindex="0" data-rl-i="${i}" aria-label="${esc(bad ? `${aria}. ${bad}` : aria)}"${bad ? ` title="${esc(bad)}"` : ""}>`
      + `<span class="nm-grip" aria-hidden="true"></span><span class="rl-step-no">${i + 1}</span><span class="nm-lbl">${esc(label)}</span>`
      + `<button type="button" class="nm-x" tabindex="-1" data-rl-x="${i}" aria-label="${esc(t("Remove {block}", { block: label }))}" title="${esc(t("Remove {block}", { block: label }))}">${ICON.x}</button></span>`
      + (i < n - 1 ? `<span class="rl-arrow" aria-hidden="true">→</span>` : "");
  }).join("");
  const free = rlFree(steps).map((k) => `<button type="button" class="nm-add rl-add k-${k}" data-rl-add="${k}"><b aria-hidden="true">+</b>${esc(rlKindLabel(k))}</button>`).join("");
  return `<div class="rl-builder" data-rl-rule="${esc(rule.id)}">
      <div class="nm-used rl-used${n ? "" : " empty"}" role="list" aria-label="${esc(t("Blocks in the order they run"))}">${chips || `<span class="nm-empty">${esc(t("No blocks yet: add one below"))}</span>`}</div>
      <div class="rl-why small" aria-live="polite"${RL.why ? "" : " hidden"}>${esc(RL.why)}</div>
      ${RL.sel >= 0 && steps[RL.sel] ? rlOptsHtml(rule, RL.sel) : ""}
      ${free ? `<div class="nm-free" role="group" aria-label="${esc(t("Blocks you can add"))}"><span class="nm-free-lbl">${esc(t("Add"))}</span>${free}</div>` : ""}
    </div>`;
}

/** The options of block `i`: a small row under the blocks. */
function rlOptsHtml(rule, i) {
  const st = rule.steps[i];
  const opt = (v, label, cur) => `<option value="${esc(v)}"${String(v) === String(cur) ? " selected" : ""}>${esc(label)}</option>`;
  const sel = (key, inner, label) => `<label class="rl-opt"><span>${esc(label)}</span><select class="field" data-rl-opt="${key}" aria-label="${esc(label)}">${inner}</select></label>`;
  let body = "";
  if (st.kind === "color") {
    const { builtin, mine } = rlLooks();
    const known = builtin.includes(st.look) || mine.includes(st.look);
    body = sel("look", (known ? "" : opt(st.look, t("{name} (no longer exists)", { name: st.look }), st.look))
      + builtin.map((n) => opt(n, rlTr(n), st.look)).join("")
      + (mine.length ? `<optgroup label="${esc(t("Your presets"))}">${mine.map((n) => opt(n, n, st.look)).join("")}</optgroup>` : ""), t("Look"))
      + `<span class="small">${esc(t("The editor's color looks. Make your own in the editor's {link}.")).replace("{link}", () => `<a href="#" class="rl-link" data-rl-colorpanel>${esc(t("color panel"))}</a>`)}</span>`;
  } else if (st.kind === "canvas") {
    const list = rlLayouts(), cur = rlLayoutOf(st, list);
    const gone = !cur && (st.layout || st.layout_name);
    if (list.length || gone) {
      body = sel("layout", (cur ? "" : opt("", gone ? t("{name} (no longer exists)", { name: st.layout_name || st.layout }) : t("Pick a layout"), ""))
        + list.map((p) => opt(p.id, rlLayoutLabel(p), cur ? cur.id : "")).join(""), t("Layout"));
    }
    body += `<span class="small">${esc(list.length
      ? t("A layout saved in the editor. Its pieces are fitted to each clip's picture, so it works on clips of any size.")
      : t("No saved layouts yet. Make one in the editor's Canvas panel, then click “Save this layout…”."))}</span>`;
  } else if (st.kind === "compress") {
    body = `<label class="rl-opt"><span>${esc(t("Size"))}</span><input class="field rl-mb" type="number" min="${RL_MIN_MB}" max="${RL_MAX_MB}" step="1" data-rl-opt="mb" value="${+st.mb || 50}" aria-label="${esc(t("Size in MB"))}"><span>MB</span></label>`
      + `<span class="rl-quick">${RL_MB_QUICK.map((n) => `<button type="button" class="btn sm${+st.mb === n ? " on" : ""}" data-rl-mb="${n}">${n}</button>`).join("")}</span>`
      + `<span class="small">${esc(t("With the Compress page's settings. Skipped when the clip is already smaller."))}</span>`;
  } else if (st.kind === "upscale") {
    const heights = [1080, 1440, 2160];
    const h = +st.upscale?.height || 2160;
    body = sel("height", (heights.includes(h) ? "" : opt(h, rlHeight(h), h)) + heights.map((x) => opt(x, rlHeight(x), h)).join(""), t("Target"))
      + `<span class="small">${esc(t("Best as the last video step, right before uploading. Skipped when the clip is already that tall."))}</span>`;
  } else if (st.kind === "gif") {
    const g = st.gif || {};
    body = sel("gif-width", RL_GIF_WIDTHS.map((w) => opt(w, w ? t("{n} px", { n: w }) : t("The clip's own width"), g.width)).join(""), t("Width"))
      + sel("gif-fps", RL_GIF_FPS.map((f) => opt(f, t("{n} fps", { n: f }), g.fps)).join(""), t("Frames a second"))
      + sel("gif-mb", RL_GIF_MB.map((m) => opt(m, m ? t("{n} MB", { n: m }) : t("No limit"), g.max_mb || 0)).join(""), t("Size limit"))
      + `<span class="small">${esc(t("A GIF has no sound. It is saved beside the clip, not in the library."))}</span>`;
  } else if (st.kind === "blur") {
    body = `<span class="small">${esc(t("Uses your Blur settings as they are."))}</span><button type="button" class="text-link" data-rlp="blur-page">${esc(t("Change Blur settings"))}</button>`;
  } else if (st.kind === "copy") {
    body = `<span class="small">${esc(t("Puts the result on the clipboard, ready to paste."))}</span>`;
  }
  return `<div class="rl-opts" data-rl-opts="${i}"><b class="rl-opts-title">${esc(rlKindLabel(st.kind))}</b>${body}</div>`;
}

async function rlNew() {
  const list = rlList();
  const rule = { id: rlNewId(), name: rlFreeName(list.map((r) => r.name), "Render rule {n}", (_s, p) => t("Render rule {n}", p)), steps: [] };
  list.push(rule);
  RL.open = rule.id;
  RL.sel = -1;
  rlShowWhy(null);
  rlSave(true);
  rlFocus({ add: "color" });
}
async function rlRename(id) {
  const rule = rlById(id);
  if (!rule) return;
  const name = await askText(t('Rename the rule "{name}"', { name: rule.name }), rule.name, t("Rename"));
  if (!name || name === rule.name) return;
  rule.name = name;
  rlSave(true);
}
function rlDuplicate(id) {
  const rule = rlById(id);
  if (!rule) return;
  const list = rlList();
  const copy = JSON.parse(JSON.stringify(rule));
  copy.id = rlNewId();
  const names = list.map((r) => r.name);
  const base = t("{name} copy", { name: rule.name });
  copy.name = base;
  for (let n = 2; names.includes(copy.name); n++) copy.name = `${base} ${n}`;
  list.splice(list.indexOf(rule) + 1, 0, copy);
  RL.open = copy.id;
  rlSave(true);
}
async function rlDelete(id) {
  const rule = rlById(id);
  if (!rule) return;
  if (!(await ask(t('Delete the rule "{name}"? This cannot be undone.', { name: rule.name }), t("Delete")))) return;
  S.settings.rules = rlList().filter((r) => r.id !== id);
  if (S.settings.rules_after_export === id) S.settings.rules_after_export = "";
  if (RL.open === id) { RL.open = undefined; RL.sel = -1; }
  rlSave(true);
}
function rlShift(id, by) {
  const list = rlList();
  const i = list.findIndex((r) => r.id === id);
  const j = i + by;
  if (i < 0 || j < 0 || j >= list.length) return;
  [list[i], list[j]] = [list[j], list[i]];
  rlSave(true);
}

/** Right-click on a rule's card: rename, duplicate, order, delete (like the theme presets' menu). */
function rlMenu(e) {
  const card = e.target.closest?.("#view .rl-card");
  if (!card || S.page !== "rules" || S.open || e.target.closest("input, textarea")) return false;
  const id = card.dataset.rlCard;
  const list = rlList();
  const i = list.findIndex((r) => r.id === id);
  menuIds = [];
  $("#menu").innerHTML = rlMItem("rename", t("Rename…"), ICON.edit, id) + rlMItem("dup", t("Duplicate"), ICON.copy, id)
    + "<hr>" + rlMItem("up", t("Move up"), "", id, "", i <= 0) + rlMItem("down", t("Move down"), "", id, "", i >= list.length - 1)
    + "<hr>" + rlMItem("del", t("Delete"), ICON.trash, id, "danger");
  placeMenu(e.clientX, e.clientY);
  return true;
}
/** A menu item of this file: data-rlm instead of app.js's data-m, so menuAction never sees it. */
const rlMItem = (act, label, icon, arg = "", cls = "", disabled = false) =>
  `<button data-rlm="${act}" data-arg="${esc(arg)}" class="${cls}"${disabled ? ' disabled aria-disabled="true" style="opacity:.5;cursor:default"' : ""}>${icon || "<svg></svg>"}<span>${esc(label)}</span></button>`;

/* ---------- where a rule is started ---------- */

/** Clip right-click menu (app.js openClipMenu): "Render rule" with every rule, and "Edit rules…". */
function rulesMenuItem(clips) {
  RL.menuClips = (clips || []).map((c) => c.id).filter(Boolean);
  if (!RL.menuClips.length || (clips || []).some((c) => c.draft)) return "";
  const list = rlList();
  const inner = list.length
    ? list.map((r) => rlMItem("run", r.name, rlCannotWhy(r) ? ICON.warn : "", r.id, "", !!rlCannotWhy(r))).join("") + "<hr>" + rlMItem("page", t("Edit rules…"), ICON.gear)
    : `<div class="menu-hint">${esc(t("No render rules yet"))}</div>` + rlMItem("page-new", t("Make a render rule…"), ICON.plus);
  return mSub(t("Render rule"), ICON.rules, inner);
}

/** The viewer's side panel (editor.js), under the Blur panel: run a rule on this clip. */
function rulesViewerHtml(c) {
  if (!c || c.draft) return "";
  const list = rlList();
  if (!list.length) return `<div class="rl-v small"><button type="button" class="text-link" data-rlv="new">${esc(t("Make a render rule"))}</button></div>`;
  const pick = rlById(rlViewerPick()) || list[0];
  const cannot = rlCannotWhy(pick);
  return `<div class="rl-v">
      <div class="rl-v-row"><select class="field" data-rlv-pick aria-label="${esc(t("Render rule"))}">${list.map((r) => `<option value="${esc(r.id)}"${r.id === pick.id ? " selected" : ""}>${esc(r.name)}</option>`).join("")}</select>
      <button type="button" class="btn" data-rlv="run" data-arg="${esc(c.id)}" ${cannot ? "disabled" : ""}>${ICON.rules}<span>${esc(t("Run"))}</span></button></div>
      <div class="small rl-v-words">${esc(cannot || rlDescribe(pick))}</div>
    </div>`;
}
const RL_PICK_KEY = "glint.rulesPick";
const rlViewerPick = () => { try { return localStorage.getItem(RL_PICK_KEY) || ""; } catch { return ""; } };
const rlSetViewerPick = (id) => { try { localStorage.setItem(RL_PICK_KEY, id); } catch {} };

/** The editor's Export box (editor.js renderExport): "Then run" a rule on the exported clip. */
function rulesExportHtml(c, busy) {
  const list = rlList();
  if (!list.length) return "";
  const cur = rlById(S.settings.rules_after_export) ? S.settings.rules_after_export : "";
  const rule = rlById(cur);
  return `<label class="rl-after"><span class="small">${esc(t("Then run"))}</span>
      <select class="field" data-rl-after aria-label="${esc(t("Render rule to run after the export"))}" ${busy ? "disabled" : ""}>
        <option value=""${cur ? "" : " selected"}>${esc(t("None"))}</option>${list.map((r) => `<option value="${esc(r.id)}"${r.id === cur ? " selected" : ""}>${esc(r.name)}</option>`).join("")}
      </select></label>
    ${rule ? `<div class="small rl-after-words">${esc(t("After Export: {words}", { words: rlDescribe(rule) }))}</div>` : ""}`;
}

/** editor.js, after a successful export: the rule picked in "Then run" runs on the new clip. */
function rulesAfterExport(meta, withBlur) {
  const rule = rlById(S.settings.rules_after_export);
  if (!rule || !meta?.id || withBlur) return; // "Export and render with Blur" is its own way out
  rulesRun(rule.id, [meta.id]);
}

/** Run rule `id` on clips `ids`: one job each (the backend refuses what cannot run). */
async function rulesRun(id, ids) {
  const rule = rlById(id);
  if (!rule) return flash(t("That render rule no longer exists"), true);
  if (!rule.steps?.length) return flash(t("This rule has no blocks yet. Add one first."), true);
  rlListen();
  let jobs;
  try { jobs = (await invoke("rules_run", { rule: id, ids })) || []; } catch (e) { return fail(e); }
  const queued = jobs.filter((j) => j.id);
  const refused = jobs.filter((j) => !j.id);
  for (const j of queued) RL.names.set(j.id, rule.name);
  if (queued.length && typeof jobsSet === "function" && typeof jobsList !== "undefined") jobsSet([...jobsList, ...queued.filter((j) => !jobsList.some((x) => x.id === j.id))]);
  if (refused.length) {
    const why = t(String(refused[0].message || ""));
    return flash(queued.length ? t("{n} of the clips were not started: {reason}", { n: refused.length, reason: why }) : why, true);
  }
  sfx("launch");
  flash(ids.length === 1 ? t('Running "{name}"…', { name: rule.name }) : t('Running "{name}" on {clips}…', { name: rule.name, clips: plural(ids.length) }));
}

/* ---------- the Renders list and the end of a job ---------- */

/** jobs.js: the status line of a rule's row ("Color 40 %", "Blur…", "Compressing 70 %"). "" = the default. */
function rulesJobLine(r) {
  const pct = Math.floor(+r.pct || 0);
  if (r.state === "queued") return t("Waiting to run the render rule");
  if (r.state === "done") return t("Render rule finished");
  if (r.state !== "running") return "";
  switch (r.job?.stage) {
    case "color": return t("Color {pct} %", { pct });
    case "canvas": return t("Canvas {pct} %", { pct });
    case "blur": return pct < 1 ? t("Blur…") : t("Blur {pct} %", { pct });
    case "upscale": return t("Upscaling {pct} %", { pct });
    case "compress": return t("Compressing {pct} %", { pct });
    case "gif": return t("Making the GIF {pct} %", { pct });
    case "copy": return t("Copying…");
    default: return t("Starting…");
  }
}

/** Listen once for the end of rule jobs: "rule-done" says how it went; a failure comes with "jobs-changed". */
function rlListen() {
  if (RL.listening || !window.__TAURI__?.event?.listen) return;
  RL.listening = true;
  const listen = window.__TAURI__.event.listen;
  listen("rule-done", ({ payload: p }) => {
    if (!p) return;
    RL.seen.add(p.job_id);
    const name = p.name || RL.names.get(p.job_id) || "";
    const size = typeof fmtSize === "function" ? fmtSize(p.size || 0) : "";
    let msg;
    if (p.nothing) msg = p.copied ? t('"{name}" had nothing to change. The clip was copied as it is.', { name }) : t('"{name}" had nothing to change: the clip is already as the rule asks.', { name });
    else if (p.gif) msg = p.copied ? t('"{name}" is done: GIF saved ({size}) and copied.', { name, size }) : t('"{name}" is done: GIF saved ({size}) beside the clip.', { name, size });
    else msg = p.copied ? t('"{name}" is done ({size}) and copied. Paste it anywhere.', { name, size }) : t('"{name}" is done ({size}). The new clip is in your library.', { name, size });
    if (p.over_limit) msg += " " + t("It is still over the size limit: this was the smallest try.");
    if (p.clip && Array.isArray(S.clips) && !S.clips.some((c) => c.id === p.clip.id)) S.clips.push(p.clip);
    sfx("success");
    flash(msg, !!p.over_limit);
  });
  listen("jobs-changed", ({ payload }) => {
    for (const j of Array.isArray(payload) ? payload : []) {
      if (j.kind !== "rule" || j.state !== "error" || RL.seen.has(j.id)) continue;
      RL.seen.add(j.id);
      const name = RL.names.get(j.id);
      flash(name ? t('"{name}" failed: {reason}', { name, reason: j.message }) : t("A render rule failed: {reason}", { reason: j.message }), true);
    }
  });
}

/* ---------- events (delegated: the page is redrawn often) ---------- */
if (typeof document !== "undefined" && typeof window !== "undefined" && window.__TAURI__) {
  rlListen();
  document.addEventListener("click", (e) => {
    if (RL.suppress) { RL.suppress = false; if (e.target.closest?.(".rl-builder")) { e.stopPropagation(); return; } }
    const b = e.target.closest?.("[data-rlm],[data-rlp],[data-rlv],[data-rl-x],[data-rl-add],[data-rl-i],[data-rl-mb]");
    if (!b || b.disabled) return;
    const d = b.dataset;
    try {
      if (d.rlm) {
        closeMenu();
        if (d.rlm === "run") return void rulesRun(d.arg, RL.menuClips.slice());
        if (d.rlm === "page") { if (S.open && typeof closeClip === "function") closeClip(); return goPage("rules"); }
        if (d.rlm === "page-new") { if (S.open && typeof closeClip === "function") closeClip(); goPage("rules"); return void rlNew(); }
        if (d.rlm === "rename") return void rlRename(d.arg);
        if (d.rlm === "dup") return rlDuplicate(d.arg);
        if (d.rlm === "del") return void rlDelete(d.arg);
        if (d.rlm === "up") return rlShift(d.arg, -1);
        if (d.rlm === "down") return rlShift(d.arg, 1);
        return;
      }
      if (d.rlp === "new") return void rlNew();
      if (d.rlp === "pick") { RL.open = RL.open === d.arg ? null : d.arg; RL.sel = -1; rlShowWhy(null); return renderRulesPage(); }
      if (d.rlp === "blur-page") return goPage("blur");
      if (d.rlv === "run") { const pick = document.querySelector("[data-rlv-pick]")?.value || rlList()[0]?.id; return void rulesRun(pick, [d.arg]); }
      if (d.rlv === "new") { if (typeof closeClip === "function") closeClip(); goPage("rules"); return void rlNew(); }
      const builder = b.closest(".rl-builder");
      const rule = builder && rlById(builder.dataset.rlRule);
      if (!rule) return;
      if ("rlX" in d) { const i = +d.rlX; RL.sel = -1; return rlSetSteps(rule, rule.steps.filter((_, j) => j !== i), { i: Math.max(0, i - 1) }); }
      if (d.rlAdd) { const next = rlInsert(rule.steps, d.rlAdd, rule.steps.length, rlLast()); const fixed = rlFixOrder(next).steps; RL.sel = fixed.findIndex((s) => s.kind === d.rlAdd); return rlSetSteps(rule, next, { i: RL.sel }); }
      if (d.rlMb) { const i = RL.sel; if (rule.steps[i]?.kind === "compress") { rule.steps[i].mb = +d.rlMb; rlSave(true); } return; }
      if ("rlI" in d && !e.target.closest(".nm-x")) { const i = +d.rlI; RL.sel = RL.sel === i ? -1 : i; renderRulesPage(); return rlFocus({ i }); }
    } catch (err) { fail(err); }
  });
  // Options of a block, the rule's name, the viewer's pick, the Export box's "Then run".
  document.addEventListener("change", (e) => {
    const el = e.target;
    if (el.matches?.("[data-rl-after]")) { S.settings.rules_after_export = el.value; saveSettings(false); if (S.player?.ed) S.player.ed.renderExport(); return; }
    if (el.matches?.("[data-rlv-pick]")) {
      rlSetViewerPick(el.value);
      const rule = rlById(el.value), words = el.closest(".rl-v")?.querySelector(".rl-v-words"), run = el.closest(".rl-v")?.querySelector('[data-rlv="run"]');
      const cannot = rlCannotWhy(rule);
      if (words) words.textContent = cannot || rlDescribe(rule);
      if (run) run.disabled = !!cannot;
      return;
    }
    const opt = el.dataset?.rlOpt;
    if (!opt) return;
    const rule = rlById(el.closest(".rl-builder")?.dataset.rlRule);
    const st = rule?.steps[RL.sel];
    if (!st) return;
    const v = el.value;
    if (opt === "look") st.look = v;
    else if (opt === "layout") { const p = rlLayouts().find((x) => x.id === v); if (!p) return; st.layout = p.id; st.layout_name = p.name; }
    else if (opt === "mb") st.mb = Math.round(clamp(+v || 50, RL_MIN_MB, RL_MAX_MB));
    else if (opt === "height") st.upscale = { ...(st.upscale || {}), height: +v };
    else if (opt === "gif-width") st.gif = { ...(st.gif || {}), width: +v };
    else if (opt === "gif-fps") st.gif = { ...(st.gif || {}), fps: +v };
    else if (opt === "gif-mb") st.gif = { ...(st.gif || {}), max_mb: +v };
    rlSave(true);
  });
  document.addEventListener("input", (e) => {
    const id = e.target.dataset?.rlName;
    if (id === undefined) return;
    const rule = rlById(id);
    if (!rule) return;
    const v = e.target.value.trim();
    if (!v) return; // an empty name is not saved; the old one stays until something is typed
    rule.name = v.slice(0, 60);
    const nameEl = e.target.closest(".rl-card")?.querySelector(".rl-name");
    if (nameEl) nameEl.textContent = rule.name;
    saveSettings(false);
  });
  document.addEventListener("focusout", (e) => { if (e.target.dataset?.rlName !== undefined && !e.target.value.trim()) { const rule = rlById(e.target.dataset.rlName); if (rule) e.target.value = rule.name; } });
  // Registered before app.js's handler on the same node: stopImmediatePropagation keeps it from closing this menu.
  document.addEventListener("contextmenu", (e) => {
    if (rlMenu(e)) { e.preventDefault(); e.stopImmediatePropagation(); }
  });
  // Keys on a block: Left / Right walk, with Alt (Ctrl, Shift) they move it, Delete removes it, Enter shows its options.
  document.addEventListener("keydown", (e) => {
    const chip = e.target.closest?.("[data-rl-i]");
    if (!chip || e.target !== chip) return;
    const rule = rlById(chip.closest(".rl-builder")?.dataset.rlRule);
    if (!rule) return;
    const i = +chip.dataset.rlI;
    if (e.key === "Enter" || e.key === " ") { e.preventDefault(); e.stopPropagation(); RL.sel = RL.sel === i ? -1 : i; renderRulesPage(); return rlFocus({ i }); }
    const op = typeof nmKeyOp === "function" ? nmKeyOp(e.key, { alt: e.altKey, ctrl: e.ctrlKey, shift: e.shiftKey }, i, rule.steps.length) : null;
    if (!op) return;
    e.preventDefault();
    e.stopPropagation();
    if (op.focus !== undefined) return rlFocus({ i: op.focus });
    if (op.move !== undefined) {
      const kind = rule.steps[i].kind;
      const next = rlMove(rule.steps, i, op.move);
      RL.sel = -1;
      rlSetSteps(rule, next);
      return rlFocus({ i: rule.steps.findIndex((s) => s.kind === kind) });
    }
    if (op.remove) { RL.sel = -1; return rlSetSteps(rule, rule.steps.filter((_, j) => j !== i), { i: Math.max(0, i - 1) }); }
  }, true);

  /* Drag with the mouse, as in the clip-name builder: a block moves (dropped away from the row it is
     removed); a "+" block is added where it is dropped. While dragging, an order that cannot work shows
     its reason, and on the drop it is fixed. */
  document.addEventListener("mousedown", (e) => {
    if (e.button !== 0) return;
    const builder = e.target.closest?.(".rl-builder");
    if (!builder || e.target.closest("input, select, .nm-x, .rl-opts")) return;
    const chip = e.target.closest("[data-rl-i]");
    const add = e.target.closest("[data-rl-add]");
    if (!chip && !add) return;
    e.preventDefault();
    (chip || add).focus();
    RL.drag = { rule: builder.dataset.rlRule, el: chip || add, from: chip ? +chip.dataset.rlI : -1, kind: add ? add.dataset.rlAdd : null, x: e.clientX, y: e.clientY, on: false, at: null, out: false };
  });
  window.addEventListener("mousemove", (e) => {
    const d = RL.drag;
    if (!d) return;
    if (e.buttons === 0) return rlDragEnd(false);
    if (!d.on) {
      if (Math.hypot(e.clientX - d.x, e.clientY - d.y) < 5) return;
      d.on = true;
      const r = d.el.getBoundingClientRect();
      d.dx = d.x - r.left;
      d.dy = d.y - r.top;
      d.ghost = d.el.cloneNode(true);
      d.ghost.classList.add("nm-ghost");
      d.ghost.removeAttribute("data-rl-i");
      d.ghost.removeAttribute("data-rl-add");
      d.ghost.style.width = `${r.width}px`;
      document.body.append(d.ghost);
      d.mark = document.createElement("i");
      d.mark.className = "nm-mark";
      document.body.append(d.mark);
      d.tip = document.createElement("div");
      d.tip.className = "rl-drag-tip";
      d.tip.hidden = true;
      document.body.append(d.tip);
      d.el.classList.add("nm-lifted");
      document.body.classList.add("nm-dragging");
    }
    d.ghost.style.transform = `translate(${e.clientX - d.dx}px, ${e.clientY - d.dy}px)`;
    const row = document.querySelector(`.rl-builder[data-rl-rule="${CSS.escape(d.rule)}"] .rl-used`);
    const rule = rlById(d.rule);
    if (!row || !rule) return;
    const rr = row.getBoundingClientRect();
    const near = e.clientX > rr.left - 24 && e.clientX < rr.right + 24 && e.clientY > rr.top - 18 && e.clientY < rr.bottom + 18;
    const chips = [...row.querySelectorAll("[data-rl-i]")];
    const rects = chips.map((el) => { const b = el.getBoundingClientRect(); return { l: b.left, r: b.right, t: b.top, b: b.bottom }; });
    d.out = d.from >= 0 && !near;
    d.at = near ? nmDropIndex(rects, e.clientX, e.clientY) : null;
    d.ghost.classList.toggle("nm-out", d.out);
    // The order this drop would make, and whether it needs fixing.
    const why = d.at === null ? null : rlFixOrder(rlDropResult(rule.steps, d)).why;
    d.tip.hidden = !why;
    if (why) { d.tip.textContent = rlTr(why); d.tip.style.transform = `translate(${e.clientX + 14}px, ${e.clientY + 18}px)`; }
    d.mark.classList.toggle("rl-mark-bad", !!why);
    if (d.at === null || (d.from >= 0 && (d.at === d.from || d.at === d.from + 1))) { d.mark.hidden = true; return; }
    d.mark.hidden = false;
    const ref = rects[d.at] || rects[rects.length - 1];
    const x = !ref ? rr.left + 8 : rects[d.at] ? ref.l - 4 : ref.r + 3;
    Object.assign(d.mark.style, { left: `${x}px`, top: `${ref ? ref.t : rr.top + 4}px`, height: `${ref ? ref.b - ref.t : rr.height - 8}px` });
  });
  window.addEventListener("mouseup", () => rlDragEnd(true));
  window.addEventListener("blur", () => rlDragEnd(false));
}

/** The blocks after drop `d` (before any fixing). */
function rlDropResult(steps, d) {
  if (d.from >= 0) {
    if (d.out) return steps.filter((_, j) => j !== d.from);
    if (d.at === null) return steps.slice();
    return rlMove(steps, d.from, d.at > d.from ? d.at - 1 : d.at);
  }
  return d.at === null ? steps.slice() : rlInsert(steps, d.kind, d.at, typeof S !== "undefined" && S.settings ? rlLast() : {});
}

function rlDragEnd(drop) {
  const d = RL.drag;
  if (!d) return;
  RL.drag = null;
  d.ghost?.remove();
  d.mark?.remove();
  d.tip?.remove();
  d.el.classList.remove("nm-lifted");
  document.body.classList.remove("nm-dragging");
  if (!drop || !d.on) return;
  RL.suppress = true;
  setTimeout(() => (RL.suppress = false), 0);
  const rule = rlById(d.rule);
  if (!rule || (!d.out && d.at === null)) return;
  const kind = d.from >= 0 ? rule.steps[d.from]?.kind : d.kind;
  RL.sel = -1;
  rlSetSteps(rule, rlDropResult(rule.steps, d));
  const at = rule.steps.findIndex((s) => s.kind === kind);
  if (at >= 0) rlFocus({ i: at }); else rlFocus({ add: kind });
}

if (typeof module !== "undefined") module.exports = { rlFixOrder, rlFree, rlNewStep, rlMove, rlInsert, rlWords, rlDescribe, rlFreeName, rlDropResult, rlHeight, rlLayoutOf, rlLayoutLabel, RL_WHY_GIF, RL_WHY_COPY, RL_WHY_ONCE, RL_WHY_UNKNOWN, RL_WHY_CANVAS };

/* "…the editor's color panel" in a Color block's options: opens the newest clip in the editor at its Color section (0.1.25). */
function rlOpenColorPanel() {
  const newest = (S.clips || []).filter((c) => c && !c.draft).sort((a, b) => (b.created || 0) - (a.created || 0))[0];
  if (!newest) return flash(t("Save or import a clip first: the color panel is in the editor."), true);
  openClip(newest.id);
  const P = S.player;
  if (!P) return;
  P.setEdit(true);
  const show = () => document.querySelector("#modal .e-color")?.scrollIntoView({ block: "center" });
  Promise.resolve(P.whenModel?.()).then(() => setTimeout(show, 60)).catch(() => {});
}
if (typeof document !== "undefined") {
  document.addEventListener("click", (e) => {
    const link = e.target.closest?.("[data-rl-colorpanel]");
    if (!link) return;
    e.preventDefault();
    try { rlOpenColorPanel(); } catch (err) { fail(err); }
  });
}