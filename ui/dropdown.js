"use strict";
/* Glint's own dropdowns (0.1.24).

   Every <select> in the window keeps living where its page put it, still the source of truth (its
   value, its options, every change / input listener and data-set stay as they were), but it is
   hidden and drawn as a button beside it. The button shows the chosen option and a chevron; a
   press opens a list on <body> that looks like the right-click menu (style.css .menu). Picking
   sets select.value and sends bubbling input and change events from the select.

   The focus stays on the (hidden) select, so Glint's shortcut code keeps treating it as a field
   the way it always did (app.js, editor.js and projects.js skip their shortcuts in a select).
   While a list is open it owns the keyboard: it takes app.js's prompt hook (askKey) for that time,
   so no keybind or page shortcut sees the keys, and gives it back when it closes.

   `data-native` on a select leaves it alone; multiple / sized selects (none in Glint) too. */

const DD = {
  btns: new Set(),   // every button made (swept when pages are rebuilt)
  dirty: new Set(),  // selects to bring up to date at the end of this task
  queued: false,
  seq: 0,
  // The open list: its element, select, button, the option elements in list order, the active row.
  list: null, sel: null, btn: null, rows: [], act: -1, key: "", prevAsk: null,
  typed: "", typedAt: 0,
};

const ddSkip = (s) => s.multiple || s.size > 1 || s.hasAttribute("data-native");
const ddLabel = (o) => (o.label || o.text || "").replace(/\s+/g, " ").trim();
const ddEsc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
/** What identifies a select across a rebuild of its page (to keep an open list open on the new one). */
const ddIdent = (s) => {
  const d = s.dataset;
  if (d.set) return "set:" + d.set;
  if (s.id) return "id:" + s.id;
  if (d.kb !== undefined) return `kb:${d.kb}:${d.kbf || ""}`;
  if (d.lang !== undefined) return "lang";
  return "";
};

function ddQueue(s) {
  if (!s || s.tagName !== "SELECT") return;
  DD.dirty.add(s);
  if (!DD.queued) { DD.queued = true; queueMicrotask(ddFlush); }
}

function ddFlush() {
  DD.queued = false;
  const list = [...DD.dirty];
  DD.dirty.clear();
  for (const s of list) { try { ddSync(s); } catch (e) { console.error("Glint: dropdown", e); } }
}

/** Removes buttons whose select went away or moved, and follows (or closes) the open list. */
function ddSweep() {
  for (const b of DD.btns) {
    const s = b.__ddSel;
    if (!b.isConnected) { DD.btns.delete(b); if (s && s.__ddBtn === b) s.__ddBtn = null; continue; }
    if (!s || !s.isConnected || b.previousElementSibling !== s || s.__ddBtn !== b) {
      b.remove(); DD.btns.delete(b);
      if (s && s.__ddBtn === b) { s.__ddBtn = null; if (s.isConnected) ddQueue(s); }
    }
  }
  if (DD.list && (!DD.sel.isConnected || DD.sel.__ddBtn !== DD.btn || !DD.btn.isConnected)) {
    // The page was drawn again: a select in the same place with the same setting keeps the list.
    const again = DD.key && [...document.querySelectorAll("select.dd-src")].find((s) => s !== DD.sel && ddIdent(s) === DD.key && s.__ddBtn);
    if (again && !again.disabled) ddReanchor(again);
    else ddClose(false);
  }
}

/** Gives `s` its button (once) and makes the button show what the select holds now. */
function ddSync(s) {
  if (!s.isConnected) return;
  let b = s.__ddBtn;
  if (ddSkip(s)) {
    if (b) { b.remove(); DD.btns.delete(b); s.__ddBtn = null; }
    // (Only when present: classList.remove rewrites the attribute, which would wake the observer again.)
    if (s.classList.contains("dd-src")) s.classList.remove("dd-src");
    if (s.hasAttribute("aria-haspopup")) s.removeAttribute("aria-haspopup");
    return;
  }
  if (!b || !b.isConnected || b.previousElementSibling !== s) {
    if (b) { b.remove(); DD.btns.delete(b); }
    b = document.createElement("button");
    b.type = "button";
    b.tabIndex = -1; // the select itself is what Tab reaches
    b.setAttribute("aria-hidden", "true");
    b.__ddSel = s;
    b.__ddSig = "";
    s.__ddBtn = b;
    s.after(b);
    DD.btns.add(b);
  }
  if (!s.classList.contains("dd-src")) s.classList.add("dd-src");
  if (s.getAttribute("aria-haspopup") !== "listbox") s.setAttribute("aria-haspopup", "listbox");
  const cls = ("dd-btn " + s.className.replace(/\bdd-src\b/g, "")).replace(/\s+/g, " ").trim() + (DD.sel === s ? " dd-open" : "");
  const opts = [...s.options];
  const labels = opts.map((o) => (o.hidden ? "" : ddLabel(o)));
  const i = s.selectedIndex;
  const sig = [cls, s.disabled, s.hidden, s.title, s.getAttribute("style") || "", i, labels.join("\u0001")].join("\u0002");
  if (sig === b.__ddSig) return;
  b.__ddSig = sig;
  if (b.className !== cls) b.className = cls;
  b.disabled = s.disabled;
  b.hidden = s.hidden;
  if (s.title) b.title = s.title; else b.removeAttribute("title");
  const st = s.getAttribute("style");
  if (st) b.setAttribute("style", st); else b.removeAttribute("style");
  // Every label stacked in one cell, only the chosen one visible: the button is as wide as the
  // widest option, the way the select was, so picking another one moves nothing.
  b.innerHTML = `<span class="dd-val">${labels.map((l, k) => `<span${k === i ? ' class="on"' : ""}>${ddEsc(l) || "&nbsp;"}</span>`).join("") || "<span>&nbsp;</span>"}</span>`;
  if (DD.sel === s && DD.list) ddFill();
}

/* ---------- the open list ---------- */

function ddOpen(s) {
  if (DD.sel === s) return;
  ddClose(false);
  const b = s.__ddBtn;
  if (!b || s.disabled || !b.isConnected) return;
  const list = document.createElement("div");
  list.className = "dd-list";
  list.id = "dd-list-" + ++DD.seq;
  list.setAttribute("role", "listbox");
  list.dataset.sfx = "none"; // the pick sounds through the select's change, like before
  // Its name: aria-label, title, else the words of the <label> around it (not the options' words).
  const lbl = s.closest("label");
  const name = s.getAttribute("aria-label") || s.title || (lbl ? [...lbl.childNodes].filter((n) => n.nodeType === 3).map((n) => n.textContent).join(" ").replace(/\s+/g, " ").trim() : "");
  if (name) list.setAttribute("aria-label", name);
  Object.assign(DD, { list, sel: s, btn: b, key: ddIdent(s), typed: "" });
  ddFill();
  document.body.appendChild(list);
  b.classList.add("dd-open");
  s.setAttribute("aria-expanded", "true");
  s.setAttribute("aria-controls", list.id);
  if (document.activeElement !== s) s.focus({ preventScroll: true });
  ddPlace();
  ddSetAct(DD.rows.findIndex((o) => o.__ddOpt.selected && !o.__ddOpt.disabled), true);
  if (typeof askKey !== "undefined" && askKey !== ddKey) { DD.prevAsk = askKey; askKey = ddKey; }
}

/** (Re)builds the rows from the select's options and groups. */
function ddFill() {
  const { list, sel } = DD;
  const activeOpt = DD.rows[DD.act]?.__ddOpt;
  const frag = document.createDocumentFragment();
  const rows = [];
  const addOpt = (o, inGroup) => {
    if (o.hidden) return;
    const r = document.createElement("div");
    r.className = "dd-opt" + (o.selected ? " on" : "") + (inGroup ? " in-group" : "");
    r.id = `${list.id}-${rows.length}`;
    r.setAttribute("role", "option");
    r.setAttribute("aria-selected", o.selected ? "true" : "false");
    const off = o.disabled || o.parentElement?.disabled;
    if (off) r.setAttribute("aria-disabled", "true");
    r.innerHTML = `<span>${ddEsc(ddLabel(o)) || "&nbsp;"}</span>`;
    r.__ddOpt = o;
    rows.push(r);
    frag.appendChild(r);
  };
  for (const c of sel.children) {
    if (c.tagName === "OPTGROUP") {
      const h = document.createElement("div");
      h.className = "dd-group";
      h.setAttribute("role", "presentation");
      h.textContent = c.label;
      frag.appendChild(h);
      for (const o of c.children) if (o.tagName === "OPTION") addOpt(o, true);
    } else if (c.tagName === "OPTION") addOpt(c, false);
  }
  list.replaceChildren(frag);
  DD.rows = rows;
  DD.act = -1;
  if (activeOpt) { const k = rows.findIndex((r) => r.__ddOpt === activeOpt); if (k >= 0) ddSetAct(k, false); }
}

const ddOff = (r) => r.getAttribute("aria-disabled") === "true";

/** Puts the list under the button, or above it when there is more room there. */
function ddPlace() {
  const { list, btn } = DD;
  if (!list) return;
  const r = btn.getBoundingClientRect();
  const vw = window.innerWidth, vh = window.innerHeight, pad = 8, gap = 4;
  list.style.minWidth = `${Math.max(120, r.width)}px`;
  list.style.maxWidth = `${Math.max(r.width, 340)}px`;
  list.style.maxHeight = "";
  list.style.left = "0px";
  list.style.top = "0px";
  const want = list.scrollHeight + 2; // + the border
  const below = vh - r.bottom - gap - pad, above = r.top - gap - pad;
  const up = want > below && above > below;
  const room = Math.max(60, up ? above : below);
  const h = Math.min(want, room, 320);
  list.style.maxHeight = `${h}px`;
  const w = list.getBoundingClientRect().width;
  list.style.left = `${Math.max(pad, Math.min(r.left, vw - w - pad))}px`;
  list.style.top = `${up ? r.top - gap - h : r.bottom + gap}px`;
  list.classList.toggle("up", up);
}

function ddSetAct(k, center) {
  const rows = DD.rows;
  if (DD.rows[DD.act]) DD.rows[DD.act].classList.remove("act");
  DD.act = k >= 0 && k < rows.length ? k : -1;
  const r = rows[DD.act];
  if (r) {
    r.classList.add("act");
    DD.sel.setAttribute("aria-activedescendant", r.id);
    const l = DD.list, top = r.offsetTop, bot = top + r.offsetHeight;
    if (center) l.scrollTop = top - (l.clientHeight - r.offsetHeight) / 2;
    else if (top < l.scrollTop + 5) l.scrollTop = top - 5;
    else if (bot > l.scrollTop + l.clientHeight - 5) l.scrollTop = bot - l.clientHeight + 5;
  } else DD.sel?.removeAttribute("aria-activedescendant");
}

/** The next usable row from `from` going `dir` (1 / -1), or `from` itself when there is none. */
function ddStep(from, dir, n = 1) {
  let k = from, last = from;
  while (n > 0) {
    k += dir;
    if (k < 0 || k >= DD.rows.length) break;
    if (!ddOff(DD.rows[k])) { last = k; n--; }
  }
  return last;
}

function ddPick(k) {
  const r = DD.rows[k], s = DD.sel;
  if (!r || ddOff(r)) return;
  const o = r.__ddOpt;
  ddClose(true);
  if (o.selected) return; // the same choice again: nothing changes, as with the native list
  o.selected = true;
  s.dispatchEvent(new Event("input", { bubbles: true }));
  s.dispatchEvent(new Event("change", { bubbles: true }));
  ddQueue(s);
}

/** `keepFocus`: the select keeps the focus (after a pick or Esc); otherwise it is left as it is. */
function ddClose(keepFocus) {
  const { list, sel, btn } = DD;
  if (!list) return;
  list.remove();
  btn?.classList.remove("dd-open");
  if (sel) {
    sel.removeAttribute("aria-expanded");
    sel.removeAttribute("aria-controls");
    sel.removeAttribute("aria-activedescendant");
    if (keepFocus && sel.isConnected && document.activeElement !== sel) sel.focus({ preventScroll: true });
  }
  if (typeof askKey !== "undefined" && askKey === ddKey) askKey = DD.prevAsk;
  Object.assign(DD, { list: null, sel: null, btn: null, rows: [], act: -1, key: "", prevAsk: null, typed: "" });
}

/** The page was rebuilt with the list open: the list moves to the new select of the same setting. */
function ddReanchor(s) {
  const { list } = DD;
  DD.sel.removeAttribute?.("aria-expanded");
  DD.sel = s;
  DD.btn = s.__ddBtn;
  DD.btn.classList.add("dd-open");
  s.setAttribute("aria-expanded", "true");
  s.setAttribute("aria-controls", list.id);
  if (document.activeElement !== s && (document.activeElement === document.body || !document.activeElement)) s.focus({ preventScroll: true });
  ddFill();
  ddPlace();
  if (DD.act < 0) ddSetAct(DD.rows.findIndex((r) => r.__ddOpt.selected), true);
}

/** Type to find: the next option starting with what was typed (one letter again and again cycles). */
function ddType(ch, from) {
  const now = performance.now();
  DD.typed = now - DD.typedAt > 800 ? ch : DD.typed + ch;
  DD.typedAt = now;
  const q = DD.typed.toLocaleLowerCase();
  const same = [...q].every((c) => c === q[0]);
  const n = DD.rows.length;
  const starts = (k, text) => !ddOff(DD.rows[k]) && ddLabel(DD.rows[k].__ddOpt).toLocaleLowerCase().startsWith(text);
  // A longer word is looked for from the active row on; a single (repeated) letter from the next one.
  const first = q.length > 1 && !same ? Math.max(from, 0) : from + 1;
  for (let j = 0; j < n; j++) { const k = (first + j) % n; if (starts(k, q)) return k; }
  if (same) for (let j = 0; j < n; j++) { const k = (from + 1 + j) % n; if (starts(k, q[0])) return k; }
  return -1;
}

/** The keyboard while a list is open (app.js calls it first, as the open prompt's handler). */
function ddKey(e) {
  e.stopImmediatePropagation(); // no other listener on window hears it either
  if (e.type !== "keydown" || !DD.list) return;
  const k = DD.act, last = DD.rows.length - 1;
  switch (e.code) {
    case "ArrowDown": if (e.altKey) return ddPick(k); ddSetAct(k < 0 ? ddStep(-1, 1) : ddStep(k, 1)); return;
    case "ArrowUp": if (e.altKey) return ddPick(k); ddSetAct(k < 0 ? ddStep(last + 1, -1) : ddStep(k, -1)); return;
    case "PageDown": ddSetAct(ddStep(Math.max(k, 0), 1, 8)); return;
    case "PageUp": ddSetAct(ddStep(Math.max(k, 0), -1, 8)); return;
    case "Home": ddSetAct(ddStep(-1, 1)); return;
    case "End": ddSetAct(ddStep(last + 1, -1)); return;
    case "Enter": case "NumpadEnter": ddPick(k); return;
    case "Escape": ddClose(true); return;
    case "Tab": ddClose(false); return "pass"; // the focus moves on as usual
    case "F4": ddClose(true); return;
  }
  if (e.code === "Space" && !(DD.typed && performance.now() - DD.typedAt < 800)) { ddPick(k); return; }
  if (e.key && e.key.length === 1 && !e.ctrlKey && !e.altKey && !e.metaKey) {
    const hit = ddType(e.key, k);
    if (hit >= 0) ddSetAct(hit);
  }
  // Anything else does nothing while the list is open.
}

/* ---------- wiring ---------- */

// A press on a button opens (or closes) its list; the focus goes to the select, not the button.
document.addEventListener("mousedown", (e) => {
  const b = e.target.closest?.(".dd-btn");
  if (DD.list && !DD.list.contains(e.target) && b !== DD.btn) ddClose(false); // a press outside closes it
  if (!b || e.button !== 0) return;
  e.preventDefault();
  const s = b.__ddSel;
  if (!s || b.disabled) return;
  if (DD.sel === s) return ddClose(true);
  s.focus({ preventScroll: true });
  ddOpen(s);
}, true);

// Inside the list: no focus change on a press; the row under the pointer is the active one; a click picks.
document.addEventListener("mousedown", (e) => { if (DD.list?.contains(e.target)) e.preventDefault(); });
document.addEventListener("pointermove", (e) => {
  if (!DD.list) return;
  const r = e.target.closest?.(".dd-opt");
  if (!r || !DD.list.contains(r) || ddOff(r)) return;
  const k = DD.rows.indexOf(r);
  if (k !== DD.act) ddSetAct(k, false);
}, true);
document.addEventListener("click", (e) => {
  if (!DD.list || !DD.list.contains(e.target)) return;
  e.stopPropagation();
  const r = e.target.closest(".dd-opt");
  if (r) ddPick(DD.rows.indexOf(r));
}, true);
document.addEventListener("contextmenu", (e) => { if (DD.list?.contains(e.target)) e.preventDefault(); }, true);

// A closed dropdown with the focus: these keys open it (the native list never shows).
window.addEventListener("keydown", (e) => {
  const s = e.target;
  if (DD.list || !s || s.tagName !== "SELECT" || !s.__ddBtn || !s.classList.contains("dd-src")) return;
  if (e.defaultPrevented || e.ctrlKey || e.metaKey) return; // an open prompt or a keybind took it
  const c = e.code;
  const opens = c === "Enter" || c === "NumpadEnter" || c === "Space" || c === "ArrowDown" || c === "ArrowUp" || c === "F4" || c === "Home" || c === "End" || c === "PageUp" || c === "PageDown";
  const letter = !opens && !e.altKey && e.key && e.key.length === 1;
  if (!opens && !letter) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  if (s.disabled) return;
  ddOpen(s);
  if (!DD.list) return;
  if (c === "Home" || c === "End") ddKey(e);
  else if (letter) { const hit = ddType(e.key, DD.act); if (hit >= 0) ddSetAct(hit); }
}, true);

// Leaving the window, scrolling something else, a resize or the focus going elsewhere closes it.
window.addEventListener("blur", () => ddClose(false));
window.addEventListener("resize", () => ddClose(false));
window.addEventListener("scroll", (e) => { if (DD.list && !DD.list.contains(e.target)) ddClose(false); }, true);
document.addEventListener("focusout", (e) => {
  if (!DD.list || e.target !== DD.sel) return;
  // The focus left the select for something other than the list (the window blur closes it anyway).
  setTimeout(() => { if (DD.list && document.activeElement !== DD.sel && document.hasFocus()) ddClose(false); }, 0);
});

// Values set from code (select.value = …, selectedIndex, option.selected) change no attribute:
// these setters tell the button too.
for (const [proto, prop] of [[HTMLSelectElement.prototype, "value"], [HTMLSelectElement.prototype, "selectedIndex"], [HTMLOptionElement.prototype, "selected"]]) {
  const d = Object.getOwnPropertyDescriptor(proto, prop);
  if (!d || !d.set || !d.configurable) continue;
  Object.defineProperty(proto, prop, {
    configurable: true,
    enumerable: d.enumerable,
    get: d.get,
    set(v) { d.set.call(this, v); ddQueue(this.tagName === "OPTION" ? this.closest("select") : this); },
  });
}
document.addEventListener("change", (e) => { if (e.target?.tagName === "SELECT") ddQueue(e.target); }, true);
window.addEventListener("pageshow", () => document.querySelectorAll("select").forEach(ddQueue)); // restored form values

// Pages are drawn again and again: new selects get their button in the same task, before paint.
const ddMine = (n) => n.nodeType === 1 && (n.classList.contains("dd-btn") || n.classList.contains("dd-list") || n.closest(".dd-btn, .dd-list"));
new MutationObserver((recs) => {
  let removed = false;
  for (const r of recs) {
    const tg = r.target;
    if (r.type === "attributes") {
      const tag = tg.tagName;
      if (tag === "SELECT") ddQueue(tg);
      else if (tag === "OPTION" || tag === "OPTGROUP") ddQueue(tg.closest("select"));
      continue;
    }
    if (r.type === "characterData") {
      const p = tg.parentElement;
      if (p && p.tagName === "OPTION") ddQueue(p.closest("select"));
      continue;
    }
    // childList
    if (tg.nodeType === 1 && (tg.tagName === "SELECT" || tg.tagName === "OPTION" || tg.tagName === "OPTGROUP")) { ddQueue(tg.tagName === "SELECT" ? tg : tg.closest("select")); continue; }
    if (ddMine(tg)) continue;
    if (r.removedNodes.length) removed = true;
    for (const n of r.addedNodes) {
      if (n.nodeType !== 1) continue;
      if (n.tagName === "SELECT") ddQueue(n);
      else if (n.firstElementChild && n.tagName !== "BUTTON") for (const s of n.getElementsByTagName("select")) ddQueue(s);
    }
  }
  if (DD.queued) ddFlush();
  if (removed) ddSweep();
}).observe(document.documentElement, { subtree: true, childList: true, characterData: true, attributes: true, attributeFilter: ["disabled", "class", "style", "hidden", "title", "label", "selected", "value", "data-native", "multiple", "size"] });

for (const s of document.getElementsByTagName("select")) ddQueue(s);
