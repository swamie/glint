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
    insp: [250, Math.max(250, Math.min(560, w * 0.30))],
    tl: [140, Math.max(140, h - WS_TOP - WS_PLAYER_MIN)],
  };
}
const wsClamp = (v, [lo, hi]) => Math.round(Math.min(hi, Math.max(lo, v)));

/** The name a section's fold state is kept under: its own e-* classes ("e-color"), else its title's text. */
function wsSecKey(sec) {
  const own = [...sec.classList].filter((c) => c.startsWith("e-") && c !== "e-sec").sort().join(".");
  if (own) return own;
  const head = sec.firstElementChild;
  return head && head.classList.contains("e-title") ? `t:${head.textContent.trim().slice(0, 40)}` : "";
}

const WS_ICON_POOL = '<svg viewBox="0 0 24 24"><rect x="3" y="4.5" width="18" height="15" rx="2"/><path d="M9 4.5v15"/></svg>';
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
  for (const k of ["pool", "insp", "tl"]) {
    if (k === "pool" && !pool) continue;
    root.insertAdjacentHTML("beforeend", `<div class="ws-grip ws-grip-${k}" data-ws-grip="${k}" role="separator" aria-orientation="${k === "tl" ? "horizontal" : "vertical"}" title="${gripTip}"></div>`);
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
    delete cur[k];
    wsSave(cur);
    root.style.removeProperty(`--ws-${k}`);
  });

  // Folding the inspector's sections: a click on a section's title (not on a control in it). The class sits
  // on the .e-sec itself, so it stays when the section's owner redraws its inside.
  const pane = root.querySelector(".v-edit");
  if (!pane) return;
  const applyFolds = () => {
    const folds = wsLoad().fold || {};
    for (const sec of pane.querySelectorAll(":scope > .e-sec")) {
      const key = wsSecKey(sec);
      sec.classList.toggle("ws-fold", !!(key && folds[key]));
    }
  };
  applyFolds();
  new MutationObserver(applyFolds).observe(pane, { childList: true });
  pane.addEventListener("click", (e) => {
    const title = e.target.closest(".e-title");
    const sec = title?.parentElement;
    if (!sec || sec.parentElement !== pane || !sec.classList.contains("e-sec") || sec.firstElementChild !== title) return;
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
