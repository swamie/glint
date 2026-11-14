"use strict";
// 0.1.18: clip names made of blocks (Settings → Storage → Clip names) and bulk rename.
//
// A name is a row of blocks joined by spaces: Game, Date, Time, Length, Text (any number) and, in
// the bulk rename dialog only, Number and Current name. The backend makes the real names
// (src-tauri/src/naming.rs, `build`); nmFormat here is its mirror for the settings preview.
// The bulk dialog's list comes from the backend (`rename_plan`), so it shows the real clashes.
//
// This file loads before app.js: app.js globals (S, invoke, saveSettings, ICON and the translation function) are only
// touched inside functions. The pure model functions at the top run in Node (tests).

/* ---------- model (pure) ---------- */
const NM_KINDS = ["game", "date", "time", "length", "text", "number", "current"];
const NM_BULK_ONLY = ["number", "current"];
const NM_MAX_NAME = 120;
const NM_MAX_TEXT = 60;
const nmDefault = () => [{ kind: "game" }, { kind: "date" }, { kind: "time" }];
const nmBlock = (kind, text = "") => (kind === "text" ? { kind, text } : { kind });

/** Blocks as saved: known kinds only, Number / Current name only where `extra`, each of the others once. */
function nmNormalize(blocks, extra = false) {
  const seen = new Set();
  const out = [];
  for (const b of Array.isArray(blocks) ? blocks : []) {
    const kind = b && typeof b.kind === "string" ? b.kind : "";
    if (!NM_KINDS.includes(kind) || (!extra && NM_BULK_ONLY.includes(kind))) continue;
    if (kind !== "text") { if (seen.has(kind)) continue; seen.add(kind); }
    out.push(nmBlock(kind, kind === "text" ? String(b.text ?? "").slice(0, NM_MAX_TEXT) : ""));
  }
  return out;
}
/** The kinds that can still be added: each unused one, and Text always. */
function nmFree(blocks, extra = false) {
  const used = new Set(blocks.map((b) => b.kind));
  return NM_KINDS.filter((k) => (extra || !NM_BULK_ONLY.includes(k)) && (k === "text" || !used.has(k)));
}
/** A new block of `kind` at `at` (the end when left out). A kind already used is not added twice. */
function nmInsert(blocks, kind, at = blocks.length) {
  if (!NM_KINDS.includes(kind) || (kind !== "text" && blocks.some((b) => b.kind === kind))) return blocks.slice();
  const out = blocks.slice();
  out.splice(Math.max(0, Math.min(at, out.length)), 0, nmBlock(kind));
  return out;
}
/** Block `from` moved so that it ends up at index `to`. */
function nmMove(blocks, from, to) {
  if (from < 0 || from >= blocks.length) return blocks.slice();
  const out = blocks.slice();
  const [b] = out.splice(from, 1);
  out.splice(Math.max(0, Math.min(to, out.length)), 0, b);
  return out;
}
const nmRemove = (blocks, i) => blocks.filter((_, j) => j !== i);
const nmSetText = (blocks, i, text) => blocks.map((b, j) => (j === i ? { kind: "text", text: String(text).slice(0, NM_MAX_TEXT) } : b));

/** Mirror of naming.rs `clean`: safe for a file name, "" when nothing is left. */
function nmClean(s, max) {
  let c = String(s ?? "").replace(/[<>:"/\\|?*\u0000-\u001f\u007f-\u009f]/g, " ").split(/\s+/).filter(Boolean).join(" ");
  c = c.replace(/^[. ]+|[. ]+$/g, "");
  return [...c].slice(0, max).join("").replace(/[. ]+$/, "");
}
/** Mirror of util::sanitize_filename (the Game block): "Clip" when nothing is left. */
const nmSanitize = (s) => nmClean(s, 60) || "Clip";
/** The Length block: "30s", "1m05s", "10m". */
function nmLength(secs) {
  const r = Number.isFinite(secs) && secs > 0 ? Math.round(secs) : 0;
  if (r < 60) return `${r}s`;
  return r % 60 === 0 ? `${Math.floor(r / 60)}m` : `${Math.floor(r / 60)}m${String(r % 60).padStart(2, "0")}s`;
}
const nmNumber = (n, pad) => String(n).padStart(Math.max(1, Math.min(6, pad | 0)), "0");
const nmPad2 = (v) => String(v).padStart(2, "0");
const nmDate = (d) => `${d.getFullYear()}-${nmPad2(d.getMonth() + 1)}-${nmPad2(d.getDate())}`;
const nmTime = (d) => `${nmPad2(d.getHours())}-${nmPad2(d.getMinutes())}-${nmPad2(d.getSeconds())}`;
function nmReserved(name) {
  const stem = name.split(".")[0].trim().toUpperCase();
  return /^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$/.test(stem);
}
function nmAssemble(blocks, p) {
  const pieces = [];
  for (const b of blocks) {
    const piece = b.kind === "game" ? nmSanitize(p.game)
      : b.kind === "date" ? nmDate(p.when)
      : b.kind === "time" ? nmTime(p.when)
      : b.kind === "length" ? nmLength(p.secs)
      : b.kind === "text" ? nmClean(b.text, NM_MAX_TEXT)
      : b.kind === "number" ? (p.number ?? "")
      : b.kind === "current" ? nmClean(p.current, NM_MAX_NAME) : "";
    if (piece) pieces.push(piece);
  }
  let name = [...pieces.join(" ")].slice(0, NM_MAX_NAME).join("").replace(/[. ]+$/, "");
  if (name && nmReserved(name)) name += "_";
  return name;
}
/** Mirror of naming.rs `build`. p = { game, when: Date, secs, number: "007" | null, current }. */
function nmFormat(blocks, p) {
  return nmAssemble(blocks || [], p) || nmAssemble(nmDefault(), p);
}
/** Where a dragged block lands: the index (0..n) among `rects` (left, right, top, bottom edges as l r t b; in order) for the pointer at x, y. */
function nmDropIndex(rects, x, y) {
  if (!rects.length) return 0;
  // The line of blocks nearest the pointer (the row wraps), then the first block whose middle is right of it.
  let best = null;
  for (const r of rects) {
    const dy = y < r.t ? r.t - y : y > r.b ? y - r.b : 0;
    if (best === null || dy < best) best = dy;
  }
  const line = rects.map((r, i) => ({ r, i })).filter(({ r }) => (y < r.t ? r.t - y : y > r.b ? y - r.b : 0) === best);
  for (const { r, i } of line) if (x < (r.l + r.r) / 2) return i;
  return line[line.length - 1].i + 1;
}
/**
 * What a key does on block `i` of `n`: { focus: j } | { move: j } | { remove: true } | { edit: true } | null.
 * Left / Right (and Home / End) walk the blocks; with Alt, Ctrl or Shift they move the block;
 * Delete or Backspace removes it; Enter or F2 on a Text block goes into its words.
 */
function nmKeyOp(key, mods, i, n, isText = false) {
  const shift = mods.alt || mods.ctrl || mods.shift;
  const to = key === "ArrowLeft" ? i - 1 : key === "ArrowRight" ? i + 1 : key === "Home" ? 0 : key === "End" ? n - 1 : null;
  if (to !== null) {
    const j = Math.max(0, Math.min(n - 1, to));
    return j === i ? { focus: i } : shift ? { move: j } : { focus: j };
  }
  if (key === "Delete" || key === "Backspace") return { remove: true };
  if (isText && (key === "Enter" || key === "NumpadEnter" || key === "F2")) return { edit: true };
  return null;
}

/* ---------- builder (DOM) ---------- */
const nmEsc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
function nmLabel(kind) {
  return { game: t("Game"), date: t("Date"), time: t("Time"), length: t("Length"), text: t("Text"), number: t("Number"), current: t("Current name") }[kind] || kind;
}
const nmX = () => (typeof ICON === "object" && ICON.x) || "×";
const NM = { ctx: {}, drag: null, suppress: false, lastBulk: null, dlg: null, undoTimer: 0 };

/**
 * The block builder: the blocks in use, the ones that can be added, and (with `preview`) the
 * name they make. `ctx` names the entry in NM.ctx that holds its blocks: "set" (Settings) or "bulk".
 */
function nmBuilderHtml(ctx) {
  const c = NM.ctx[ctx];
  const blocks = c.get();
  const n = blocks.length;
  const chips = blocks.map((b, i) => {
    const label = nmLabel(b.kind);
    const aria = t("{block}, block {i} of {n}. Alt+Left or Alt+Right moves it, Delete removes it", { block: label, i: i + 1, n });
    const inner = b.kind === "text"
      ? `<span class="nm-grip" aria-hidden="true"></span><input class="nm-text" type="text" data-nm-ti="${i}" value="${nmEsc(b.text)}" placeholder="${nmEsc(t("Your words"))}" maxlength="${NM_MAX_TEXT}" spellcheck="false" autocomplete="off" aria-label="${nmEsc(t("Text"))}" tabindex="-1" size="${Math.max(6, Math.min(24, [...(b.text || t("Your words"))].length + 1))}">`
      : `<span class="nm-grip" aria-hidden="true"></span><span class="nm-lbl">${nmEsc(label)}</span>`;
    return `<span class="nm-block k-${b.kind}" role="listitem" tabindex="0" data-nm-i="${i}" aria-label="${nmEsc(aria)}">${inner}<button type="button" class="nm-x" tabindex="-1" data-nm-x="${i}" aria-label="${nmEsc(t("Remove {block}", { block: label }))}" title="${nmEsc(t("Remove {block}", { block: label }))}">${nmX()}</button></span>`;
  }).join("");
  const free = nmFree(blocks, c.extra).map((k) => `<button type="button" class="nm-add k-${k}" data-nm-add="${k}"><b aria-hidden="true">+</b>${nmEsc(nmLabel(k))}</button>`).join("");
  return `<div class="nm-builder" data-nm="${ctx}">
    <div class="nm-used${n ? "" : " empty"}" role="list" aria-label="${nmEsc(t("Name blocks in order"))}">${chips || `<span class="nm-empty">${nmEsc(t("No blocks: new clips get the default name"))}</span>`}</div>
    <div class="nm-free" role="group" aria-label="${nmEsc(t("Blocks you can add"))}"><span class="nm-free-lbl">${nmEsc(t("Add"))}</span>${free}</div>
    ${c.preview ? `<div class="nm-preview" aria-live="polite">${nmPreviewHtml(ctx)}</div>` : ""}
  </div>`;
}
function nmPreviewHtml(ctx) {
  const c = NM.ctx[ctx];
  const blocks = c.get();
  const name = nmFormat(blocks, c.parts());
  const dated = blocks.some((b) => b.kind === "date" || b.kind === "time") || !nmAssemble(blocks, c.parts());
  return `<span class="nm-pv-lbl">${nmEsc(t("Preview"))}</span><span class="nm-pv-name"><b>${nmEsc(name)}</b>.mp4</span>`
    + (dated ? "" : `<div class="nm-pv-hint">${nmEsc(t("Without a date or time, clips with the same name get (2), (3) and so on"))}</div>`);
}
/** Draw builder `ctx` again in place, then put the focus on `focus`: { i } a block, { ti } its words, { add: kind }, or "free". */
function nmRedraw(ctx, focus) {
  const old = document.querySelector(`.nm-builder[data-nm="${ctx}"]`);
  if (!old) return;
  const box = document.createElement("div");
  box.innerHTML = nmBuilderHtml(ctx);
  const el = box.firstElementChild;
  old.replaceWith(el);
  if (!focus) return;
  let target = null;
  if (focus.ti !== undefined) target = el.querySelector(`[data-nm-ti="${focus.ti}"]`);
  else if (focus.i !== undefined) target = el.querySelector(`[data-nm-i="${Math.min(focus.i, el.querySelectorAll("[data-nm-i]").length - 1)}"]`);
  else if (focus.add) target = el.querySelector(`[data-nm-add="${focus.add}"]`);
  if (!target) target = el.querySelector("[data-nm-i]") || el.querySelector("[data-nm-add]");
  target?.focus();
  if (target?.matches("input")) { target.tabIndex = 0; target.setSelectionRange(target.value.length, target.value.length); }
}
function nmChange(ctx, blocks, focus) {
  const c = NM.ctx[ctx];
  c.set(nmNormalize(blocks, c.extra));
  nmRedraw(ctx, focus);
}
const nmCtxOf = (el) => el?.closest?.(".nm-builder")?.dataset.nm || null;

/* Settings → Storage → Clip names. */
NM.ctx.set = {
  extra: false,
  preview: true,
  // [] is kept as it is (the backend then uses the default), so removing every block shows an empty row.
  get: () => (Array.isArray(S.settings.clip_name) ? S.settings.clip_name : nmDefault()),
  set: (blocks) => { S.settings.clip_name = blocks; saveSettings(false); },
  parts: () => {
    // A real-looking example: the newest clip's game (or the game being played), now, the quick clip's length.
    const newest = (S.clips || []).filter((c) => c.kind === "clip").sort((a, b) => b.created - a.created)[0];
    const game = (S.status && S.status.game) || newest?.game || "Overwatch";
    return { game, when: new Date(), secs: typeof quickSecs === "function" ? quickSecs() : 30, number: null, current: "" };
  },
};
/** The row in Settings → Storage (renderSettings calls it). */
function namingRowHtml() {
  return `<div class="row nm-setrow"><div class="txt"><div class="name">${t("Clip names")}</div><div class="hint">${t("How new clips are named. Drag the blocks to reorder them, or drag one out to remove it")}</div></div>`
    + `<div class="ctl"><button type="button" class="btn sm" data-nm-reset>${t("Reset")}</button></div>${nmBuilderHtml("set")}</div>`;
}

/* ---------- builder events (delegated: the builders are redrawn often) ---------- */
document.addEventListener("click", (e) => {
  if (NM.suppress) { NM.suppress = false; if (e.target.closest?.(".nm-builder")) { e.stopPropagation(); return; } }
  const reset = e.target.closest?.("[data-nm-reset]");
  if (reset) { nmChange("set", nmDefault(), { i: 0 }); return; }
  const ctx = nmCtxOf(e.target);
  if (!ctx) return;
  const c = NM.ctx[ctx];
  const x = e.target.closest("[data-nm-x]");
  if (x) { const i = +x.dataset.nmX; return nmChange(ctx, nmRemove(c.get(), i), { i: Math.max(0, i - (i >= c.get().length - 1 ? 1 : 0)) }); }
  const add = e.target.closest("[data-nm-add]");
  if (add) {
    const kind = add.dataset.nmAdd;
    const next = nmInsert(c.get(), kind);
    return nmChange(ctx, next, kind === "text" ? { ti: next.length - 1 } : { i: next.length - 1 });
  }
  const chip = e.target.closest("[data-nm-i]");
  if (chip && !e.target.closest("input")) { const ti = chip.querySelector("[data-nm-ti]"); if (ti) { ti.tabIndex = 0; ti.focus(); } else chip.focus(); }
});
document.addEventListener("input", (e) => {
  const ti = e.target.closest?.("[data-nm-ti]");
  const ctx = ti && nmCtxOf(ti);
  if (!ctx) return;
  const c = NM.ctx[ctx];
  ti.size = Math.max(6, Math.min(24, [...(ti.value || t("Your words"))].length + 1));
  c.set(nmSetText(c.get(), +ti.dataset.nmTi, ti.value));
  const pv = ti.closest(".nm-builder").querySelector(".nm-preview");
  if (pv) pv.innerHTML = nmPreviewHtml(ctx);
});
// A Text block's words leave the tab order again once the focus moves on (the block itself is the stop).
document.addEventListener("focusout", (e) => { if (e.target.matches?.("[data-nm-ti]")) e.target.tabIndex = -1; });

/** Keys on a builder. Returns true when the key was used. Also called by the bulk dialog (askKey). */
function nmKey(e) {
  const ctx = nmCtxOf(e.target);
  if (!ctx) return false;
  const c = NM.ctx[ctx];
  const ti = e.target.closest("[data-nm-ti]");
  if (ti) {
    // In the words: Enter or Esc goes back to the block; the other keys are typing.
    if (e.code === "Enter" || e.code === "NumpadEnter" || e.code === "Escape") { e.preventDefault(); e.stopPropagation(); ti.closest("[data-nm-i]")?.focus(); return true; }
    return false;
  }
  const chip = e.target.closest("[data-nm-i]");
  if (!chip || e.target !== chip) return false;
  const i = +chip.dataset.nmI;
  const blocks = c.get();
  const op = nmKeyOp(e.key, { alt: e.altKey, ctrl: e.ctrlKey, shift: e.shiftKey }, i, blocks.length, blocks[i]?.kind === "text");
  if (!op) return false;
  e.preventDefault();
  e.stopPropagation();
  if (op.focus !== undefined) chip.parentElement.querySelector(`[data-nm-i="${op.focus}"]`)?.focus();
  else if (op.move !== undefined) nmChange(ctx, nmMove(blocks, i, op.move), { i: op.move });
  else if (op.remove) { const next = nmRemove(blocks, i); nmChange(ctx, next, next.length ? { i: Math.min(i, next.length - 1) } : { add: "game" }); }
  else if (op.edit) { const input = chip.querySelector("[data-nm-ti]"); input.tabIndex = 0; input.focus(); }
  return true;
}
document.addEventListener("keydown", (e) => { nmKey(e); });

/* Drag with the mouse: a block in use moves (dropped away from the row it is removed); a block
   that is not in use is added where it is dropped. Mouse events, as everywhere in Glint. */
document.addEventListener("mousedown", (e) => {
  if (e.button !== 0) return;
  const ctx = nmCtxOf(e.target);
  if (!ctx || e.target.closest("input, .nm-x")) return;
  const chip = e.target.closest("[data-nm-i]");
  const add = e.target.closest("[data-nm-add]");
  if (!chip && !add) return;
  e.preventDefault(); // no text selection while dragging; the click still follows
  (chip || add).focus();
  NM.drag = { ctx, el: chip || add, from: chip ? +chip.dataset.nmI : -1, kind: add ? add.dataset.nmAdd : null, x: e.clientX, y: e.clientY, on: false, at: null, out: false };
});
function nmDragEnd(drop) {
  const d = NM.drag;
  if (!d) return;
  NM.drag = null;
  d.ghost?.remove();
  d.mark?.remove();
  d.el.classList.remove("nm-lifted");
  document.body.classList.remove("nm-dragging");
  if (!drop || !d.on) return;
  NM.suppress = true;
  setTimeout(() => (NM.suppress = false), 0);
  const blocks = NM.ctx[d.ctx].get();
  if (d.from >= 0) {
    if (d.out) nmChange(d.ctx, nmRemove(blocks, d.from), { add: blocks[d.from].kind === "text" ? "text" : blocks[d.from].kind });
    else if (d.at !== null) { const to = d.at > d.from ? d.at - 1 : d.at; nmChange(d.ctx, nmMove(blocks, d.from, to), { i: to }); }
  } else if (d.at !== null) {
    nmChange(d.ctx, nmInsert(blocks, d.kind, d.at), d.kind === "text" ? { ti: d.at } : { i: d.at });
  }
}
window.addEventListener("mousemove", (e) => {
  const d = NM.drag;
  if (!d) return;
  if (e.buttons === 0) return nmDragEnd(false);
  if (!d.on) {
    if (Math.hypot(e.clientX - d.x, e.clientY - d.y) < 5) return;
    d.on = true;
    const r = d.el.getBoundingClientRect();
    d.dx = d.x - r.left;
    d.dy = d.y - r.top;
    d.ghost = d.el.cloneNode(true);
    d.ghost.classList.add("nm-ghost");
    d.ghost.removeAttribute("data-nm-i");
    d.ghost.style.width = `${r.width}px`;
    document.body.append(d.ghost);
    d.mark = document.createElement("i");
    d.mark.className = "nm-mark";
    document.body.append(d.mark);
    d.el.classList.add("nm-lifted");
    document.body.classList.add("nm-dragging");
  }
  d.ghost.style.transform = `translate(${e.clientX - d.dx}px, ${e.clientY - d.dy}px)`;
  const builder = document.querySelector(`.nm-builder[data-nm="${d.ctx}"]`);
  const row = builder?.querySelector(".nm-used");
  if (!row) return;
  const rr = row.getBoundingClientRect();
  const near = e.clientX > rr.left - 24 && e.clientX < rr.right + 24 && e.clientY > rr.top - 18 && e.clientY < rr.bottom + 18;
  const chips = [...row.querySelectorAll("[data-nm-i]")];
  const rects = chips.map((el) => { const b = el.getBoundingClientRect(); return { l: b.left, r: b.right, t: b.top, b: b.bottom }; });
  d.out = d.from >= 0 && !near;
  d.at = near ? nmDropIndex(rects, e.clientX, e.clientY) : null;
  d.ghost.classList.toggle("nm-out", d.out);
  // The marker: a thin bar where the block would go.
  if (d.at === null || (d.from >= 0 && (d.at === d.from || d.at === d.from + 1))) { d.mark.hidden = true; return; }
  d.mark.hidden = false;
  const ref = rects[d.at] || rects[rects.length - 1];
  const x = !ref ? rr.left + 8 : rects[d.at] ? ref.l - 4 : ref.r + 3;
  const top = ref ? ref.t : rr.top + 4;
  const h = ref ? ref.b - ref.t : rr.height - 8;
  Object.assign(d.mark.style, { left: `${x}px`, top: `${top}px`, height: `${h}px` });
});
window.addEventListener("mouseup", () => nmDragEnd(true));
window.addEventListener("blur", () => nmDragEnd(false));

/* ---------- bulk rename ---------- */
NM.ctx.bulk = {
  extra: true,
  preview: false,
  get: () => NM.dlg?.blocks || [],
  set: (blocks) => { if (!NM.dlg) return; NM.dlg.blocks = blocks; NM.lastBulk = blocks; nmPlanSoon(); nmNumberRow(); },
  parts: () => ({ game: "", when: new Date(), secs: 0, number: "1", current: "" }),
};
const nmPlural = (n) => (typeof plural === "function" ? plural(n) : `${n}`);
/** The backend's reasons (naming.rs) in the user's language. */
function nmReason(why) {
  const known = {
    "This clip is being rendered": t("This clip is being rendered"),
    "The clip's file is missing": t("The clip's file is missing"),
    "it is open in another program": t("it is open in another program"),
    "access denied": t("access denied"),
    "its file is missing": t("its file is missing"),
    "Clip not found": t("Clip not found"),
  };
  return known[why] || t(why);
}
/** "Title (reason)" from the backend, with the reason translated (the title is the clip's own). */
const nmFailText = (s) => String(s).replace(/\(([^()]*)\)$/, (_, why) => `(${nmReason(why)})`);
/** The "Rename {n} clips…" dialog for `ids`, in the order they were selected. */
function nmBulkOpen(ids) {
  if (!ids.length) return;
  const el = document.getElementById("confirm");
  NM.dlg = { ids: ids.slice(), blocks: NM.lastBulk || [{ kind: "game" }, { kind: "number" }], start: NM.dlg?.start ?? 1, pad: NM.dlg?.pad ?? 1, plan: [], seq: 0, timer: 0, busy: false };
  const n = ids.length;
  el.innerHTML = `<div class="confirm-box nm-dlg" role="dialog" aria-modal="true" aria-labelledby="nm-dlg-h">
    <h3 id="nm-dlg-h">${nmEsc(t("Rename {n} clips", { n }))}</h3>
    ${nmBuilderHtml("bulk")}
    <div class="nm-num">
      <label>${nmEsc(t("Start at"))} <input class="field nm-start" type="number" min="0" max="999999" step="1" value="${NM.dlg.start}"></label>
      <label>${nmEsc(t("Digits"))} <select class="field nm-pad">${[1, 2, 3, 4].map((p) => `<option value="${p}"${p === NM.dlg.pad ? " selected" : ""}>${[1, 2, 3].map((k) => nmNumber(k, p)).join(", ")}</option>`).join("")}</select></label>
    </div>
    <div class="nm-list" role="list" aria-label="${nmEsc(t("Preview"))}"></div>
    <div class="nm-sum" aria-live="polite"></div>
    <div class="confirm-row"><button type="button" class="btn" data-nm-c="no">${nmEsc(t("Cancel"))}</button><button type="button" class="btn primary" data-nm-c="yes" disabled>${nmEsc(t("Rename"))}</button></div>
  </div>`;
  el.hidden = false;
  nmNumberRow();
  // Glint's prompts take every key through askKey (app.js stops the event there); the builder's keys go on to nmKey.
  askKey = (e) => {
    if (nmKey(e)) return;
    if (e.code === "Escape") { e.preventDefault(); return nmBulkClose(); }
    const field = e.target.matches?.("input, select");
    if ((e.code === "Enter" || e.code === "NumpadEnter") && (field || e.target === el || e.target === document.body)) { e.preventDefault(); return nmBulkGo(); }
    return "pass";
  };
  el.onclick = (e) => {
    const b = e.target.closest("[data-nm-c]");
    if (b) return b.dataset.nmC === "yes" ? nmBulkGo() : nmBulkClose();
    if (e.target === el && !NM.suppress) nmBulkClose();
  };
  el.oninput = (e) => {
    if (e.target.matches(".nm-start")) { const v = Math.floor(+e.target.value); if (Number.isFinite(v) && v >= 0) { NM.dlg.start = Math.min(v, 999999); nmPlanSoon(); } }
  };
  el.onchange = (e) => { if (e.target.matches(".nm-pad")) { NM.dlg.pad = +e.target.value || 1; nmPlanSoon(); } };
  el.querySelector("[data-nm-i]")?.focus();
  nmPlanSoon(0);
}
function nmBulkClose() {
  const el = document.getElementById("confirm");
  el.hidden = true;
  el.innerHTML = "";
  el.onclick = el.oninput = el.onchange = null;
  askKey = null;
  if (NM.dlg) clearTimeout(NM.dlg.timer);
  NM.dlg = null;
}
/** The start and padding controls show only while a Number block is in the name. */
function nmNumberRow() {
  const row = document.querySelector(".nm-dlg .nm-num");
  if (row && NM.dlg) row.hidden = !NM.dlg.blocks.some((b) => b.kind === "number");
}
const nmOpts = () => ({ blocks: NM.dlg.blocks, start: NM.dlg.start, pad: NM.dlg.pad });
function nmPlanSoon(ms = 120) {
  const d = NM.dlg;
  if (!d) return;
  clearTimeout(d.timer);
  d.timer = setTimeout(async () => {
    const seq = ++d.seq;
    let plan;
    try { plan = await invoke("rename_plan", { ids: d.ids, opts: nmOpts() }); } catch (e) { plan = null; if (typeof fail === "function") fail(e); }
    if (NM.dlg !== d || seq !== d.seq || !plan) return;
    d.plan = plan;
    nmPlanDraw();
  }, ms);
}
const nmChanges = (plan) => plan.filter((r) => !r.skip && r.new !== r.old);
function nmPlanDraw() {
  const d = NM.dlg;
  const list = document.querySelector(".nm-dlg .nm-list");
  if (!d || !list) return;
  const keep = list.scrollTop;
  list.innerHTML = d.plan.map((r) => {
    const tag = r.skip ? `<span class="nm-tag bad" title="${nmEsc(nmReason(r.skip))}">${nmEsc(nmReason(r.skip))}</span>`
      : r.clash ? `<span class="nm-tag warn" title="${nmEsc(t("Another clip has this name, so (2), (3) is added"))}">${nmEsc(t("Made unique"))}</span>`
      : r.title_only ? `<span class="nm-tag" title="${nmEsc(t("Imported in place: the other program's file keeps its name"))}">${nmEsc(t("Title only"))}</span>`
      : r.new === r.old ? `<span class="nm-tag">${nmEsc(t("No change"))}</span>` : "";
    return `<div class="nm-li${r.skip ? " skip" : r.clash ? " clash" : ""}" role="listitem"><span class="nm-old" title="${nmEsc(r.old)}">${nmEsc(r.old)}</span><span class="nm-arr" aria-hidden="true">→</span><span class="nm-new" title="${nmEsc(r.new)}">${nmEsc(r.skip ? r.old : r.new)}</span>${tag}</div>`;
  }).join("");
  list.scrollTop = keep;
  const go = nmChanges(d.plan).length;
  const clashes = d.plan.filter((r) => !r.skip && r.clash).length;
  const sum = document.querySelector(".nm-dlg .nm-sum");
  if (sum) sum.textContent = go ? t("{clips} will be renamed", { clips: nmPlural(go) }) + (clashes ? ` · ${t("{n} names made unique", { n: clashes })}` : "") : t("Nothing to rename");
  const btn = document.querySelector('.nm-dlg [data-nm-c="yes"]');
  if (btn) btn.disabled = !go || d.busy;
}
async function nmBulkGo() {
  const d = NM.dlg;
  if (!d || d.busy || !nmChanges(d.plan).length) return;
  d.busy = true;
  nmPlanDraw();
  let done;
  try { done = await invoke("rename_clips", { ids: d.ids, opts: nmOpts() }); } catch (e) { d.busy = false; nmPlanDraw(); return typeof fail === "function" ? fail(e) : null; }
  nmBulkClose();
  if (typeof refreshClips === "function") { try { await refreshClips(); } catch (e) { /* the list comes with the next event */ } }
  if (typeof render === "function") render();
  nmUndoBar(done);
}
/** The message after a bulk rename, with "Undo" while the old names can still be put back. */
function nmUndoBar(done) {
  document.getElementById("nm-undo")?.remove();
  clearTimeout(NM.undoTimer);
  const renamed = done?.renamed || 0;
  const failed = done?.failed || [];
  const reason = failed.length ? nmFailText(failed[0]) : "";
  const text = (renamed ? t("Renamed {clips}", { clips: nmPlural(renamed) }) : t("Nothing to rename"))
    + (failed.length ? ` · ${t("{n} left as they were: {reason}", { n: failed.length, reason })}` : "");
  const bar = document.createElement("div");
  bar.id = "nm-undo";
  bar.className = "flash nm-undo" + (failed.length && !renamed ? " bad" : "");
  bar.setAttribute("role", "status");
  bar.innerHTML = `<span>${nmEsc(text)}</span>${done?.token ? `<button type="button" class="btn sm" data-nm-undo>${nmEsc(t("Undo"))}</button>` : ""}`;
  document.body.append(bar);
  bar.querySelector("[data-nm-undo]")?.addEventListener("click", async () => {
    bar.remove();
    clearTimeout(NM.undoTimer);
    try {
      const back = await invoke("undo_rename_clips", { token: done.token });
      if (typeof refreshClips === "function") await refreshClips();
      if (typeof render === "function") render();
      const left = back?.failed || [];
      if (typeof flash === "function") flash(left.length ? t("{n} left as they were: {reason}", { n: left.length, reason: nmFailText(left[0]) }) : t("Names put back"), !!left.length);
    } catch (e) { if (typeof fail === "function") fail(e); }
  });
  NM.undoTimer = setTimeout(() => bar.remove(), failed.length ? 12000 : 9000);
}

if (typeof module === "object" && module.exports) {
  module.exports = { NM_KINDS, nmDefault, nmNormalize, nmFree, nmInsert, nmMove, nmRemove, nmSetText, nmClean, nmSanitize, nmLength, nmNumber, nmFormat, nmDropIndex, nmKeyOp, nmBuilderHtml, NM };
}
