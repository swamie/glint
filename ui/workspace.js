"use strict";
// The editor as a workspace (0.1.35). While editing, the viewer fills the window like an editing
// program: a top row (the editor's buttons on the left, the name in the middle, the window buttons on
// the right), the Library on the left (a project), the player in the middle, the edit column (the
// inspector) on the right and the timeline across the bottom. The borders between them can be dragged,
// the side panes folded away, and each section of the inspector folded by a click on its title. Sizes
// and folds are remembered (localStorage "glint.ws"). The layout is workspace.css; the plain viewer
// (watching a clip) is not touched. Nothing here renames or removes what other files look for (the
// classes listed in editor.js openClip): the editor head's buttons move into the top row as they are.

const WS_KEY = "glint.ws";
const WS_TOP = 46; // the top row, as tall as the app's top bar (workspace.css --ws-top)
const WS_PLAYER_MIN = 200; // the player keeps at least this much height (workspace.css)
// The timeline's smallest height (0.1.46): its toolbar, ruler, the video row, scrollbar and row grip (workspace.css).
const WS_TL_MIN = 148;

function wsLoad() {
  try {
    const o = JSON.parse(localStorage.getItem(WS_KEY) || "{}");
    return o && typeof o === "object" && !Array.isArray(o) ? o : {};
  } catch { return {}; }
}
function wsSave(o) { try { localStorage.setItem(WS_KEY, JSON.stringify(o)); } catch {} }

/** The sizes the layout allows in a viewer of w x h CSS px: [min, max] each (the same bounds as workspace.css). */
function wsBounds(w, h) {
  return {
    pool: [180, Math.max(180, Math.min(480, w * 0.24))],
    insp: [280, Math.max(280, w * 0.40)], // 0.1.46: 280 .. 40 % of the window (it was 250 .. the smaller of 560 and 30 %)
    tl: [WS_TL_MIN, Math.max(WS_TL_MIN, h - WS_TOP - WS_PLAYER_MIN)],
  };
}
const wsClamp = (v, [lo, hi]) => Math.round(Math.min(hi, Math.max(lo, v)));
/** The timeline area's height that shows every row (0.1.46): its fixed parts plus all of its scrolling body. */
function wsTlFit(root) {
  const area = root.querySelector(".e-tl"); // the timeline is mounted on it (its toolbar, body, scrollbar, grip)
  if (!area) return 0;
  let h = 0;
  for (const ch of area.children) h += ch.classList.contains("tl-body") ? ch.scrollHeight : ch.getBoundingClientRect().height;
  return Math.ceil(h + (area.offsetHeight - area.clientHeight) + 1);
}

/** The name a section's fold state is kept under: its own e-* classes ("e-color"), else its title's text. */
function wsSecKey(sec) {
  const own = [...sec.classList].filter((c) => c.startsWith("e-") && c !== "e-sec").sort().join(".");
  if (own) return own;
  const head = sec.firstElementChild;
  return head && head.classList.contains("e-title") ? `t:${head.textContent.trim().slice(0, 40)}` : "";
}

/* ---------- the inspector's tabs (0.1.46) ----------
 * The inspector's sections in groups, one group shown at a time: a strip of tabs (a real tablist: arrows, Home, End)
 * at its top. Each section is in exactly one tab by its class (an unknown section goes to Clip); a tab without a
 * section is not shown. The Export section is the pane's footer on every tab (its summary, progress and buttons);
 * its options (`e-exp-opt`, editor.js) show on the Export tab, with the note about where an export goes. The open tab
 * is remembered (glint.ws "tab"); the sections still fold inside it. A section that something scrolls to
 * (scrollIntoView, e.g. rules.js's "open the color panel") opens its tab first; picking an overlay item (the
 * Overlays section gets its controls) opens the Overlays tab. */
const WS_TABS = [
  ["clip", "Clip", '<svg viewBox="0 0 24 24"><rect x="3" y="5" width="18" height="14" rx="2"/><path d="M3 9h18M7 5v4M11 5v4M15 5v4"/></svg>'],
  ["color", "Color", '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="8.5"/><path d="M12 3.5v17M12 3.5a8.5 8.5 0 0 1 0 17"/></svg>'],
  ["sound", "Sound", '<svg viewBox="0 0 24 24"><path d="M4 9.5h4l5-4v13l-5-4H4zM16.5 9a4 4 0 0 1 0 6M19 6.5a7.5 7.5 0 0 1 0 11"/></svg>'],
  ["overlays", "Overlays", '<svg viewBox="0 0 24 24"><path d="M5 6h14M12 6v12M8.5 18h7"/></svg>'],
  ["export", "Export", '<svg viewBox="0 0 24 24"><path d="M12 4v11M7.5 10.5 12 15l4.5-4.5M5 19h14"/></svg>'],
];
/** Which tab each part of the inspector is in (by its class). */
const WS_SEC_TAB = { "e-speed-sec": "clip", "e-zoom": "clip", "e-cv": "clip", "e-fade": "clip", "e-color": "color", "e-ov": "overlays", "e-export": "export", "e-draft-note": "export" };
/** The tab an element of the inspector belongs to ("" for none: the hidden head, the tab strip itself). */
function wsTabOf(el) {
  for (const c of el.classList) if (WS_SEC_TAB[c]) return WS_SEC_TAB[c];
  return el.classList.contains("e-sec") ? "clip" : "";
}
function wsTabsMount(root, pane) {
  const strip = document.createElement("div");
  strip.className = "ws-tabs";
  strip.setAttribute("role", "tablist");
  strip.setAttribute("aria-label", t("Edit panel"));
  const head = pane.querySelector(":scope > .e-head");
  pane.insertBefore(strip, head ? head.nextSibling : pane.firstChild);
  let cur = wsLoad().tab || "clip";
  let sig = "";
  const present = () => {
    const have = new Set();
    for (const el of pane.children) { const k = wsTabOf(el); if (k && el.classList.contains("e-sec")) have.add(k); }
    return WS_TABS.filter(([k]) => have.has(k));
  };
  const paint = () => {
    const tabs = present();
    if (!tabs.some(([k]) => k === cur)) cur = tabs[0]?.[0] || "clip";
    const s = tabs.map(([k]) => k).join(",");
    if (s !== sig) {
      sig = s;
      strip.innerHTML = tabs.map(([k, label, icon]) => `<button type="button" role="tab" class="ws-tab" data-ws-tab="${k}" id="ws-tab-${k}" title="${esc(t(label))}">${icon}<span>${esc(t(label))}</span></button>`).join("");
    }
    for (const b of strip.querySelectorAll("[data-ws-tab]")) {
      const on = b.dataset.wsTab === cur;
      b.setAttribute("aria-selected", String(on));
      b.tabIndex = on ? 0 : -1;
      b.classList.toggle("on", on);
    }
    pane.dataset.wsTab = cur;
    for (const el of pane.children) {
      const k = wsTabOf(el);
      if (!k) continue;
      // The Export section is the footer: always there; the rest only on its own tab.
      el.classList.toggle("ws-out", k !== cur && !el.classList.contains("e-export"));
      if (el.classList.contains("e-sec")) { el.setAttribute("role", "tabpanel"); el.setAttribute("aria-labelledby", `ws-tab-${k}`); }
      if (el.classList.contains("e-sec") && !el.__wsScroll) {
        el.__wsScroll = true;
        el.scrollIntoView = function (...a) { setTab(wsTabOf(this), false); return HTMLElement.prototype.scrollIntoView.apply(this, a); };
      }
    }
  };
  const setTab = (k, focus) => {
    if (!k || k === cur) { if (focus) strip.querySelector(`[data-ws-tab="${k}"]`)?.focus(); return; }
    cur = k;
    const st = wsLoad();
    st.tab = k;
    wsSave(st);
    paint();
    (pane.closest(".v-side") || pane).scrollTop = 0; // a tab starts at its top
    if (focus) strip.querySelector(`[data-ws-tab="${k}"]`)?.focus();
  };
  root.__wsSetTab = setTab;
  strip.addEventListener("click", (e) => { const b = e.target.closest("[data-ws-tab]"); if (b) setTab(b.dataset.wsTab, false); });
  strip.addEventListener("keydown", (e) => {
    const list = [...strip.querySelectorAll("[data-ws-tab]")], i = list.findIndex((b) => b.dataset.wsTab === cur);
    let j = -1;
    if (e.key === "ArrowRight" || e.key === "ArrowDown") j = (i + 1) % list.length;
    else if (e.key === "ArrowLeft" || e.key === "ArrowUp") j = (i - 1 + list.length) % list.length;
    else if (e.key === "Home") j = 0;
    else if (e.key === "End") j = list.length - 1;
    if (j < 0) return;
    e.preventDefault();
    e.stopPropagation(); // the editor's shortcuts must not take the arrows meanwhile
    setTab(list[j].dataset.wsTab, true);
  });
  paint();
  new MutationObserver(paint).observe(pane, { childList: true });
  // Picking an overlay item gives the Overlays section its controls: that tab opens (where today it unfolded).
  let ovHad = false;
  const watchOv = () => {
    const ov = pane.querySelector(":scope > .e-ov");
    if (!ov || ov.__wsWatch) return;
    ov.__wsWatch = true;
    new MutationObserver(() => {
      const has = !!ov.querySelector("input, select, textarea");
      if (has && !ovHad && !ov.contains(document.activeElement)) setTab("overlays", false);
      ovHad = has;
    }).observe(ov, { childList: true, subtree: true });
  };
  watchOv();
  new MutationObserver(watchOv).observe(pane, { childList: true });
}

/* ---------- the Library pane (0.1.46) ----------
 * One compact head: the help line is one line (its whole text in a tooltip) and can be put away for good (glint.ws
 * "noHint"); the search box and the game filter side by side (workspace.css). */
function wsLibMount(root, lib) {
  if (!lib) return;
  if (wsLoad().noHint) root.classList.add("ws-no-hint");
  const head = lib.querySelector(".pj-head");
  if (!head || head.querySelector(".ws-hint-x")) return;
  const x = document.createElement("button");
  x.type = "button";
  x.className = "ws-hint-x";
  x.title = t("Hide this hint");
  x.setAttribute("aria-label", t("Hide this hint"));
  x.innerHTML = '<svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6 6 18"/></svg>';
  head.appendChild(x);
  x.addEventListener("click", () => {
    root.classList.add("ws-no-hint");
    const st = wsLoad();
    st.noHint = true;
    wsSave(st);
  });
  const tip = () => { const h = head.querySelector(".pj-hint"); if (h && h.title !== h.textContent.trim()) h.title = h.textContent.trim(); };
  tip();
  new MutationObserver(tip).observe(head, { childList: true, subtree: true, characterData: true });
}

const WS_ICON_POOL ='<svg viewBox="0 0 24 24"><rect x="3" y="4.5" width="18" height="15" rx="2"/><path d="M9 4.5v15"/></svg>';
const WS_ICON_INSP = '<svg viewBox="0 0 24 24"><rect x="3" y="4.5" width="18" height="15" rx="2"/><path d="M15 4.5v15"/></svg>';
const WS_WINCTL = `<div class="winctl" data-sfx="none">
  <button data-win="min" aria-label="Minimize"><svg viewBox="0 0 24 24"><path d="M5 12h14"/></svg></button>
  <button data-win="max" aria-label="Maximize"><svg viewBox="0 0 24 24"><rect x="6" y="6" width="12" height="12" rx="1"/></svg></button>
  <button data-win="close" aria-label="Close"><svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6 6 18"/></svg></button>
</div>`;

/**
 * Make the viewer `root` a workspace while it edits (editor.js openClip and projects.js projOpenEmpty call
 * it once, after the Library list is mounted). opts.title: the clip's or project's name for the top row.
 */
function wsMount(root, opts = {}) {
  if (!root || root.querySelector(":scope > .ws-top")) return;
  const pool = root.querySelector(":scope > .pj-lib");
  const st = wsLoad();
  root.classList.add("ws");
  root.classList.toggle("ws-has-pool", !!pool);
  root.classList.toggle("ws-no-pool", !!st.noPool);
  root.classList.toggle("ws-no-insp", !!st.noInsp);
  for (const k of ["pool", "insp", "tl"]) if (Number.isFinite(st[k])) root.style.setProperty(`--ws-${k}`, `${st[k]}px`);

  const top = document.createElement("div");
  top.className = "ws-top";
  top.setAttribute("data-tauri-drag-region", "");
  const tog = (k, icon, label) => `<button type="button" class="e-ibtn ws-tog" data-ws-tog="${k}" title="${esc(label)}" aria-label="${esc(label)}">${icon}</button>`;
  top.innerHTML = `
    <div class="ws-left" data-tauri-drag-region>${pool ? tog("pool", WS_ICON_POOL, t("Show or hide the library")) : ""}</div>
    <div class="ws-name" data-tauri-drag-region></div>
    <div class="ws-right" data-tauri-drag-region>${tog("insp", WS_ICON_INSP, t("Show or hide the edit panel"))}</div>`;
  const left = top.querySelector(".ws-left"), right = top.querySelector(".ws-right"), name = top.querySelector(".ws-name");
  name.textContent = opts.title || "";
  name.title = opts.title || "";
  // The editor head's buttons (shortcuts, fullscreen, Done / Close) come along as they are, so their
  // handlers (data-v, app.js) and editor.js's updates (renderHead) keep working. An empty project has none.
  const head = root.querySelector(".v-edit > .e-head");
  const moved = head ? [...head.querySelectorAll(":scope > button")] : [];
  for (const b of moved) left.appendChild(b);
  if (!moved.length) left.insertAdjacentHTML("beforeend", `<button class="btn sm" data-v="close">${esc(t("Close"))}</button>`);
  // The window buttons: the top bar's own (app.js handles every [data-win] click, Close included).
  const wc = document.querySelector(".topbar .winctl");
  if (wc) right.appendChild(wc.cloneNode(true)); else right.insertAdjacentHTML("beforeend", WS_WINCTL);
  root.prepend(top);

  const gripTip = esc(t("Drag to change the size. Double-click for the usual size."));
  const tlTip = esc(t("Drag to change the size. Double-click to show every row, again for the size before."));
  for (const k of ["pool", "insp", "tl"]) {
    if (k === "pool" && !pool) continue;
    root.insertAdjacentHTML("beforeend", `<div class="ws-grip ws-grip-${k}" data-ws-grip="${k}" role="separator" aria-orientation="${k === "tl" ? "horizontal" : "vertical"}" title="${k === "tl" ? tlTip : gripTip}"></div>`);
  }

  const paintTogs = () => {
    for (const b of top.querySelectorAll("[data-ws-tog]")) {
      const off = root.classList.contains(`ws-no-${b.dataset.wsTog}`);
      b.classList.toggle("on", !off);
      b.setAttribute("aria-pressed", String(!off));
    }
  };
  paintTogs();
  top.addEventListener("click", (e) => {
    const b = e.target.closest("[data-ws-tog]");
    if (!b) return;
    const k = b.dataset.wsTog, off = !root.classList.contains(`ws-no-${k}`);
    root.classList.toggle(`ws-no-${k}`, off);
    const cur = wsLoad();
    cur[k === "pool" ? "noPool" : "noInsp"] = off;
    wsSave(cur);
    paintTogs();
  });

  // A renamed clip shows its new name here too.
  root.addEventListener("change", (e) => {
    if (!e.target.matches?.(".v-info .v-title")) return;
    name.textContent = name.title = e.target.value;
  });

  // Dragging a border. The sizes live in CSS variables on the viewer; workspace.css clamps them to the window.
  root.addEventListener("pointerdown", (e) => {
    const g = e.target.closest?.(".ws-grip");
    if (!g || e.button !== 0) return;
    e.preventDefault();
    const k = g.dataset.wsGrip;
    let v = null;
    const move = (ev) => {
      const r = root.getBoundingClientRect(), b = wsBounds(r.width, r.height);
      const raw = k === "pool" ? ev.clientX - r.left : k === "insp" ? r.right - ev.clientX : r.bottom - ev.clientY;
      v = wsClamp(raw, b[k]);
      root.style.setProperty(`--ws-${k}`, `${v}px`);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      root.classList.remove("ws-dragging");
      delete root.dataset.wsDrag;
      if (v === null) return;
      const cur = wsLoad();
      cur[k] = v;
      wsSave(cur);
    };
    root.classList.add("ws-dragging");
    root.dataset.wsDrag = k;
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
  });
  root.addEventListener("dblclick", (e) => {
    const g = e.target.closest?.(".ws-grip");
    if (!g) return;
    const k = g.dataset.wsGrip, cur = wsLoad();
    if (k === "tl") {
      // The timeline's border (0.1.46): "fits every row" (up to the largest size), and back to the size before.
      const tlEl = root.querySelector(".e-tl"), fit = wsTlFit(root), r = root.getBoundingClientRect(), b = wsBounds(r.width, r.height);
      const now = Math.round(tlEl?.getBoundingClientRect().height || 0), want = wsClamp(fit, b.tl);
      const v = now < want - 2 ? want : Number.isFinite(cur.tlPrev) ? wsClamp(cur.tlPrev, b.tl) : null;
      if (now < want - 2) cur.tlPrev = now; else delete cur.tlPrev;
      if (v === null) { delete cur.tl; root.style.removeProperty("--ws-tl"); }
      else { cur.tl = v; root.style.setProperty("--ws-tl", `${v}px`); }
      wsSave(cur);
      return;
    }
    delete cur[k];
    wsSave(cur);
    root.style.removeProperty(`--ws-${k}`);
  });

  // Folding the inspector's sections: a click on a section's title (not on a control in it). The class sits
  // on the .e-sec itself, so it stays when the section's owner redraws its inside.
  wsLibMount(root, pool);
  const pane = root.querySelector(".v-edit");
  if (!pane) return;
  const applyFolds = () => {
    const folds = wsLoad().fold || {};
    for (const sec of pane.querySelectorAll(":scope > .e-sec")) {
      const key = wsSecKey(sec);
      sec.classList.toggle("ws-fold", !!(key && folds[key]) && !sec.classList.contains("e-export")); // Export is the footer (0.1.46)
    }
  };
  applyFolds();
  new MutationObserver(applyFolds).observe(pane, { childList: true });
  wsTabsMount(root, pane);
  pane.addEventListener("click", (e) => {
    const title = e.target.closest(".e-title");
    const sec = title?.parentElement;
    if (!sec || sec.parentElement !== pane || !sec.classList.contains("e-sec") || sec.firstElementChild !== title) return;
    if (sec.classList.contains("e-export")) return; // the footer does not fold (0.1.46)
    if (e.target.closest("button, input, select, textarea, label, a, [role='switch'], [contenteditable]") && e.target !== title) return;
    const key = wsSecKey(sec);
    const fold = !sec.classList.contains("ws-fold");
    sec.classList.toggle("ws-fold", fold);
    if (!key) return;
    const cur = wsLoad();
    cur.fold = cur.fold && typeof cur.fold === "object" ? cur.fold : {};
    if (fold) cur.fold[key] = true; else delete cur.fold[key];
    wsSave(cur);
  });
}
