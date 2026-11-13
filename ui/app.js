"use strict";
const T = window.__TAURI__;
const invoke = (cmd, args) => T.core.invoke(cmd, args);
const fileSrc = (p) => T.core.convertFileSrc(p);
const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

const S = {
  settings: null, monitors: [], status: {}, clips: [], ffmpeg: null, blur: null,
  page: "library", tab: "all", q: "", sort: "new", size: "m",
  sounds: [],        // built-in clip sounds: { id, name }
  hardware: null,    // this PC: { gpus, gpu, cpu, encoder }
  cssOpen: false,    // the custom theme box in Settings is unfolded
  games: [],         // games Glint has detected: { name, exe, icon }
  gamesOpen: false,  // the game chip row is showing every game
  folders: [],       // library folders: subfolders of the clips folder
  folder: null,      // folder filter: null = all clips, "" = clips in the main folder, else a folder name
  sel: new Set(),    // ids of selected clips
  anchor: null,      // last clip toggled, the start of a Shift+click range
  jobs: {},          // blur renders by source clip id
  open: null,        // clip id shown in the viewer
  capturing: null,   // keybind index waiting for a key press
};

const ICON = {
  library: '<svg viewBox="0 0 24 24"><rect x="3" y="5" width="18" height="14" rx="2"/><path d="m10 9 5 3-5 3z"/></svg>',
  star: '<svg viewBox="0 0 24 24"><path d="m12 3 2.7 5.6 6.1.8-4.5 4.2 1.1 6-5.4-3-5.4 3 1.1-6L3.2 9.4l6.1-.8z"/></svg>',
  blur: '<svg viewBox="0 0 24 24"><path d="M12 3s6 6.5 6 11a6 6 0 0 1-12 0c0-4.500 6-11 6-11z"/></svg>',
  keys: '<svg viewBox="0 0 24 24"><rect x="2.500" y="6" width="19" height="12" rx="2"/><path d="M6.500 10h1M10 10h1M13.500 10h1M17 10h.5M7 14h10"/></svg>',
  rec: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="8.500"/><circle cx="12" cy="12" r="3" fill="currentColor"/></svg>',
  gear: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="3"/><path d="M19 12a7 7 0 0 0-.1-1.300l2-1.500-2-3.400-2.300 1a7 7 0 0 0-2.200-1.300L14 3h-4l-.4 2.500a7 7 0 0 0-2.200 1.300l-2.300-1-2 3.400 2 1.500a7 7 0 0 0 0 2.600l-2 1.500 2 3.400 2.300-1a7 7 0 0 0 2.200 1.300L10 21h4l.4-2.500a7 7 0 0 0 2.200-1.300l2.300 1 2-3.400-2-1.500c.1-.4.1-.8.1-1.300z"/></svg>',
  copy: '<svg viewBox="0 0 24 24"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/></svg>',
  cut: '<svg viewBox="0 0 24 24"><circle cx="6" cy="6" r="2.500"/><circle cx="6" cy="18" r="2.500"/><path d="M8 7.500 20 18M8 16.500 20 6"/></svg>',
  folder: '<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>',
  trash: '<svg viewBox="0 0 24 24"><path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/></svg>',
  refresh: '<svg viewBox="0 0 24 24"><path d="M20 11a8 8 0 1 0-2.300 6.300M20 5v6h-6"/></svg>',
  plus: '<svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"/></svg>',
  x: '<svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6 6 18"/></svg>',
  check: '<svg viewBox="0 0 24 24"><path d="m5 12.500 4.500 4.500L19 7.500"/></svg>',
  edit: '<svg viewBox="0 0 24 24"><path d="M4 20h4L19 9l-4-4L4 16zM13.500 6.500l4 4"/></svg>',
  globe: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="8.500"/><path d="M3.500 12h17M12 3.500c2.600 2.400 3.900 5.200 3.900 8.500s-1.300 6.100-3.900 8.500c-2.600-2.400-3.900-5.200-3.900-8.500s1.300-6.100 3.900-8.500z"/></svg>',
  monitor: '<svg viewBox="0 0 24 24"><rect x="3" y="4.500" width="18" height="12" rx="2"/><path d="M9 20h6M12 16.500V20"/></svg>',
  image: '<svg viewBox="0 0 24 24"><rect x="3.500" y="4.500" width="17" height="15" rx="2"/><circle cx="9" cy="10" r="1.600"/><path d="m5 18 5-5 3 3 2.500-2.500L20 18"/></svg>',
  game: '<svg viewBox="0 0 24 24"><rect x="2.500" y="7" width="19" height="10" rx="5"/><path d="M7 10.500v3M5.500 12h3M15.500 11h.01M18 13h.01"/></svg>',
  compress: '<svg viewBox="0 0 24 24"><path d="M4 14h6v6M20 10h-6V4M14 10l6.500-6.500M3.500 20.500 10 14"/></svg>',
  warn: '<svg viewBox="0 0 24 24"><path d="M12 3.500 2.500 20h19z"/><path d="M12 9.500v5M12 17.500h.01"/></svg>',
};
const COMPRESS_MB = [10, 20, 50, 500]; // "Compress for Discord" sizes; Discord's upload limits are 10 MB free, 50 MB Nitro Basic, 500 MB Nitro

const PICON = {
  play: '<svg viewBox="0 0 24 24"><path d="M7 4.5v15l12.5-7.5z" fill="currentColor" stroke="none"/></svg>',
  pause: '<svg viewBox="0 0 24 24"><path d="M7 5h3.5v14H7zM13.5 5H17v14h-3.5z" fill="currentColor" stroke="none"/></svg>',
  vol: '<svg viewBox="0 0 24 24"><path d="M4 9.5v5h3.5l4.500 4v-13l-4.500 4z" fill="currentColor" stroke="none"/><path d="M15.5 9a4.200 4.200 0 0 1 0 6M18 6.500a7.800 7.800 0 0 1 0 11"/></svg>',
  mute: '<svg viewBox="0 0 24 24"><path d="M4 9.5v5h3.5l4.500 4v-13l-4.500 4z" fill="currentColor" stroke="none"/><path d="m16 9.500 5 5M21 9.500l-5 5"/></svg>',
  full: '<svg viewBox="0 0 24 24"><path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/></svg>',
  loop: '<svg viewBox="0 0 24 24"><path d="m17 3 3 3-3 3M4 11V9a3 3 0 0 1 3-3h13M7 21l-3-3 3-3M20 13v2a3 3 0 0 1-3 3H4"/></svg>',
  back: '<svg viewBox="0 0 24 24"><path d="m11 6-6 6 6 6M19 6l-6 6 6 6"/></svg>',
  fwd: '<svg viewBox="0 0 24 24"><path d="m13 6 6 6-6 6M5 6l6 6-6 6"/></svg>',
};

/* ---------- formatting ---------- */
const fmtClock = (s) => { s = Math.max(0, Math.floor(s)); return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`; };
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const fmtDur = (s) => { s = Math.max(0, Math.round(s)); const m = Math.floor(s / 60); return `${m}:${String(s % 60).padStart(2, "0")}`; };
const fmtDurMs = (s) => { const m = Math.floor(s / 60); return `${m}:${(s % 60).toFixed(1).padStart(4, "0")}`; };
const fmtLen = (s) => (s >= 60 ? t("{n} min", { n: Math.round(s / 60 * 10) / 10 }) : t("{n}s", { n: s }));
const fmtSize = (b) => (b >= 1e9 ? `${(b / 1e9).toFixed(2)} GB` : b >= 1e6 ? `${(b / 1e6).toFixed(b >= 1e8 ? 0 : 1)} MB` : `${Math.max(1, Math.round(b / 1e3))} KB`);
function timeAgo(ms) {
  const s = (Date.now() - ms) / 1000;
  if (s < 60) return t("Just now");
  if (s < 3600) return t("{n} min ago", { n: Math.floor(s / 60) });
  if (s < 86400) { const n = Math.floor(s / 3600); return n === 1 ? t("1 hour ago") : t("{n} hours ago", { n }); }
  if (s < 86400 * 30) { const n = Math.floor(s / 86400); return n === 1 ? t("1 day ago") : t("{n} days ago", { n }); }
  return new Date(ms).toLocaleDateString();
}
function keyLabel(code) {
  if (!code) return "";
  const map = { Backquote: "`", Minus: "-", Equal: "=", BracketLeft: "[", BracketRight: "]", Backslash: "\\", Semicolon: ";", Quote: "'", Comma: ",", Period: ".", Slash: "/", ArrowLeft: "Left", ArrowRight: "Right", ArrowUp: "Up", ArrowDown: "Down", PageUp: "PgUp", PageDown: "PgDn", PrintScreen: "PrtSc", NumpadAdd: "Num +", NumpadSubtract: "Num -", NumpadMultiply: "Num *", NumpadDivide: "Num /", NumpadDecimal: "Num ." };
  if (map[code]) return map[code];
  return code.replace(/^Key/, "").replace(/^Digit/, "").replace(/^Numpad/, "Num ");
}
function comboParts(k) {
  if (!k.key) return [];
  return [k.ctrl && "Ctrl", k.shift && "Shift", k.alt && "Alt", k.win && "Win", keyLabel(k.key)].filter(Boolean);
}
const comboText = (k) => comboParts(k).join(" + ");
const comboHtml = (k) => comboParts(k).map((p) => `<kbd>${esc(p)}</kbd>`).join("");
const hue = (s) => { let h = 0; for (const c of String(s)) h = (h * 31 + c.charCodeAt(0)) % 360; return h; };

let flashTimer;
function flash(msg, bad = false) {
  const el = $("#flash");
  el.textContent = msg;
  el.className = "flash" + (bad ? " bad" : "");
  el.hidden = false;
  clearTimeout(flashTimer);
  flashTimer = setTimeout(() => (el.hidden = true), bad ? 5000 : 2200);
  if (bad && typeof sfxPlay === "function") sfxPlay("error"); // every failure, fail() included
}
const sfx = (name) => { if (typeof sfxPlay === "function") sfxPlay(name); }; // sfx.js loads after this file
const fail = (e) => flash(t(String(e?.message ?? e)), true);

/** Glint's own yes/no prompt. Enter confirms, Escape or a click outside cancels. */
function ask(message, okLabel) {
  return new Promise((resolve) => {
    okLabel = okLabel || t("OK");
    const el = $("#confirm");
    el.innerHTML = `<div class="confirm-box"><p>${esc(message)}</p><div class="confirm-row"><button class="btn" data-c="no">${t("Cancel")}</button><button class="btn solid-danger" data-c="yes">${esc(okLabel)}</button></div></div>`;
    el.hidden = false;
    const done = (yes) => { el.hidden = true; el.innerHTML = ""; el.onclick = null; askKey = null; resolve(yes); };
    askKey = (e) => { if (e.code === "Enter" || e.code === "NumpadEnter") done(true); else if (e.code === "Escape") done(false); };
    el.onclick = (e) => { const b = e.target.closest("[data-c]"); if (b) done(b.dataset.c === "yes"); else if (e.target === el) done(false); };
  });
}
let askKey = null; // key handler of the open prompt, if any

/** Glint's own one-line text prompt. Resolves to the trimmed text, or null if cancelled or empty. */
function askText(message, value = "", okLabel) {
  return new Promise((resolve) => {
    okLabel = okLabel || t("OK");
    const el = $("#confirm");
    el.innerHTML = `<div class="confirm-box"><p>${esc(message)}</p><input class="field ask-input" type="text" maxlength="60" spellcheck="false" value="${esc(value)}"><div class="confirm-row"><button class="btn" data-c="no">${t("Cancel")}</button><button class="btn primary" data-c="yes">${esc(okLabel)}</button></div></div>`;
    el.hidden = false;
    const input = $(".ask-input", el);
    input.focus();
    input.select();
    const done = (yes) => { const v = input.value.trim(); el.hidden = true; el.innerHTML = ""; el.onclick = null; askKey = null; resolve(yes && v ? v : null); };
    // "pass" lets the key through to the text box.
    askKey = (e) => { if (e.code === "Enter" || e.code === "NumpadEnter") done(true); else if (e.code === "Escape") done(false); else return "pass"; };
    el.onclick = (e) => { const b = e.target.closest("[data-c]"); if (b) done(b.dataset.c === "yes"); else if (e.target === el) done(false); };
  });
}

/* ---------- settings persistence ---------- */
let saveTimer;
function saveSettings(rerender = false) {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(async () => {
    try { S.status = await invoke("save_settings", { settings: S.settings }); renderChrome(); } catch (e) { fail(e); }
  }, 200);
  if (rerender) render();
  else renderChrome();
}
function setPath(obj, path, value) {
  const parts = path.split(".");
  const last = parts.pop();
  parts.reduce((o, p) => o[p], obj)[last] = value;
}
const getPath = (obj, path) => path.split(".").reduce((o, p) => o?.[p], obj);

/* ---------- chrome: nav, top bar, status bar ---------- */
const clipBinds = () => S.settings.keybinds.filter((k) => k.action === "clip");
const quickSecs = () => Math.min(...clipBinds().map((k) => k.seconds), 3600) || 30;
const maxSecs = () => Math.max(30, ...clipBinds().map((k) => k.seconds));
// Monitors. A clip keybind's `monitor` is an index into S.monitors; -1, a missing field or a
// monitor that is not connected all mean the capture monitor (Capture → Monitor).
const monLabel = (m) => `${m.name}  ${m.width}×${m.height} @ ${m.hz} Hz${m.primary ? "  " + t("(primary)") : ""}`; // the Capture page's dropdown
const monShort = (m) => `${m.name} · ${m.width}×${m.height}`; // the same monitor in a keybind row, where there is less room
const capMon = () => S.monitors.find((m) => m.index === S.settings.monitor) || S.monitors[0] || { index: 0, name: "", width: 0, height: 0 };
const kbNamed = (k) => Number.isInteger(k.monitor) && k.monitor >= 0; // names a monitor, connected or not (null >= 0 is true, hence the integer test)
const kbMon = (k) => (kbNamed(k) && S.monitors.some((m) => m.index === k.monitor) ? k.monitor : -1);
/** Monitors buffered besides the capture monitor because a clip keybind names them: each is its own capture in the backend. */
const kbExtraMons = () => {
  const cap = capMon().index;
  const idx = new Set(clipBinds().map(kbMon).filter((i) => i >= 0 && i !== cap));
  return S.monitors.filter((m) => idx.has(m.index));
};

// Layout choices that belong to this window, not to the settings file: remembered in localStorage.
const uiPref = (key, def) => { try { const v = localStorage.getItem(key); return v === null ? def : v === "1"; } catch { return def; } };
const setUiPref = (key, on) => { try { localStorage.setItem(key, on ? "1" : "0"); } catch {} };
S.sideMin = uiPref("glint.sideMin", false);         // sidebar folded to icons
S.foldersOpen = uiPref("glint.foldersOpen", false); // the Folders row in the sidebar unfolded

function renderChrome() {
  if (!S.settings) return;
  // Texts that live in index.html: data-i18n holds the English text, data-i18n-ph a placeholder.
  for (const el of $$("[data-i18n]")) el.textContent = t(el.dataset.i18n);
  for (const el of $$("[data-i18n-ph]")) el.placeholder = t(el.dataset.i18nPh);
  if (typeof updRender === "function") updRender(); // its tooltip is text too
  if (typeof jobsRender === "function") jobsRender(); // shown or hidden by a setting
  if (typeof sfxFromSettings === "function") sfxFromSettings(S.settings); // a sound setting applies at once
  // The sidebar can be folded to a strip of icons, so every item carries its name as a tooltip.
  $("#app").classList.toggle("side-min", S.sideMin);
  $("#side-toggle").title = S.sideMin ? t("Show the sidebar") : t("Hide the sidebar");
  $("#side-toggle").setAttribute("aria-label", $("#side-toggle").title);
  $("#search").title = t("Search clips and games");
  const navItem = (id, label, icon, count) =>
    `<button class="nav-item${S.page === id ? " active" : ""}" data-page="${id}" data-hover title="${esc(label)}">${ICON[icon]}<span class="label">${label}</span>${count ? `<span class="badge">${count}</span>` : ""}</button>`;
  // Folders: one row that unfolds the list (closed to begin with). In the list: click to open,
  // right-click to rename or delete, drop dragged clips on one to move them.
  const inFolder = S.page === "library" && !!S.folder;
  const folderList = S.folders.map((f) => {
    const n = S.clips.filter((c) => c.folder === f).length;
    return `<button class="nav-item small${inFolder && S.folder === f ? " active" : ""}" data-folder="${esc(f)}" data-hover title="${esc(f)}">${ICON.folder}<span class="label">${esc(f)}</span>${n ? `<span class="badge">${n}</span>` : ""}</button>`;
  }).join("") + `<button class="nav-item small ghost" data-act="folder-new" data-hover>${ICON.plus}<span class="label">${t("New folder")}</span></button>`;
  $("#nav-general").innerHTML =
    navItem("library", t("Library"), "library", S.clips.length).replace(' active"', S.folder ? '"' : ' active"') +
    navItem("favorites", t("Favorites"), "star", S.clips.filter((c) => c.favorite).length) +
    navItem("rendered", t("Rendered"), "blur", S.clips.filter((c) => c.kind === "blur").length) +
    // While closed, the row itself is lit when the folder being viewed is hidden inside it.
    `<button class="nav-item nav-fold${inFolder && (!S.foldersOpen || S.sideMin) ? " active" : ""}" data-act="folders-toggle" data-sfx="tap" data-hover aria-expanded="${S.foldersOpen}" title="${esc(t("Folders"))}">${ICON.folder}<span class="label">${t("Folders")}</span>${S.folders.length ? `<span class="badge">${S.folders.length}</span>` : ""}<svg class="nav-chev" viewBox="0 0 24 24"><path d="m8 10 4 4 4-4"/></svg></button>` +
    `<div class="nav-sub" id="nav-folders"${S.foldersOpen ? "" : " hidden"}>${folderList}</div>`;
  $("#nav-tools").innerHTML =
    navItem("keybinds", t("Keybinds"), "keys", S.settings.keybinds.length) +
    navItem("recording", t("Capture"), "rec") +
    navItem("blur", t("Blur"), "blur") +
    navItem("settings", t("Settings"), "gear");

  const st = S.status || {};
  const on = !!st.recording;
  $("#btn-toggle").textContent = on ? t("Pause") : t("Resume");
  // In the narrowest windows the clip button shows only its icon, so its text is also its tooltip.
  $("#btn-clip span").textContent = $("#btn-clip").title = t("Clip last {length}", { length: fmtLen(quickSecs()) });
  $("#btn-clip").disabled = !on;

  // Glint keeps a replay buffer and saves clips from it; it never records to disk on its own, so the word is "Clipping".
  const enc = { nvenc: "NVENC", amf: "AMF", qsv: "Quick Sync", x264: "x264 (CPU)" }[st.encoder] || st.encoder || "";
  $("#sb-state").innerHTML = !on
    ? t("Paused")
    : st.error
      ? `<b style="color:var(--danger)">${esc(st.error)}</b>${typeof depsErrorExtra === "function" ? depsErrorExtra(st.error) : ""}`
      : (() => {
        const info = `${st.width}×${st.height} @ ${st.fps} fps${enc ? " · " + enc : ""}`;
        return st.game ? t("<b>Clipping</b> {game} · {info}", { game: esc(st.game), info }) : t("<b>Clipping</b> {info}", { info });
      })();
  // More than one monitor buffered (clip keybinds that name another monitor): the text above stays
  // the capture monitor's; a short "+1 monitor" lists them all in its tooltip, and turns yellow
  // with a warning mark when one of the others fails while the capture monitor is fine.
  const caps = Array.isArray(st.captures) ? [...st.captures].sort((a, b) => !!b.primary - !!a.primary) : [];
  if (on && caps.length > 1) {
    const n = caps.length - 1;
    const bad = !st.error && caps.some((c) => !c.primary && c.error);
    const tip = [t("Buffered monitors:"), ...caps.map((c) => `${c.name}  ${c.width}×${c.height} · ${Math.round(c.fps_actual || 0)}/${c.fps || 0} fps${c.primary ? "  " + t("(capture monitor)") : ""}${c.error ? "\n    " + t("Capture problem:") + " " + c.error : ""}`)].join("\n");
    $("#sb-state").insertAdjacentHTML("beforeend", ` · <span class="sb-more${bad ? " bad" : ""}" title="${esc(tip)}">${bad ? ICON.warn : ""}${n === 1 ? t("+1 monitor") : t("+{n} monitors", { n })}</span>`);
  }
  const bufVars = { now: fmtDur(st.buffer_secs || 0), max: fmtDur(st.buffer_max || 0), size: fmtSize(st.buffer_bytes || 0), disk: fmtSize(st.buffer_disk_bytes || 0) };
  $("#sb-buffer").innerHTML = !on ? "" : st.buffer_disk_bytes ? t("Buffer <b>{now}</b> / {max} · {size} RAM + {disk} disk", bufVars) : t("Buffer <b>{now}</b> / {max} · {size}", bufVars);
  // Frames arriving now / the frame rate set. RAM is the whole program (replay buffer, ffmpeg,
  // the WebView): ram_total, or the window plus ffmpeg from a backend that does not send it.
  $("#sb-load").textContent = on
    ? t("{now}/{max} FPS · CPU {cpu}% · RAM {ram}", { now: Math.round(st.fps_actual || 0), max: st.fps || 0, cpu: (st.cpu_pct || 0).toFixed(1), ram: fmtSize(st.ram_total > 0 ? st.ram_total : (st.ram_app || 0) + (st.ram_ffmpeg || 0)) })
    : "";
}

/* ---------- library ---------- */
function visibleClips() {
  let list = S.clips;
  if (S.page === "favorites") list = list.filter((c) => c.favorite);
  if (S.page === "rendered") list = list.filter((c) => c.kind === "blur");
  if (S.page === "library" && S.folder !== null) list = list.filter((c) => (c.folder || "") === S.folder);
  const base = list;
  if (S.tab !== "all") list = list.filter((c) => c.game === S.tab);
  const q = S.q.trim().toLowerCase();
  if (q) list = list.filter((c) => c.title.toLowerCase().includes(q) || c.game.toLowerCase().includes(q));
  const by = { new: (a, b) => b.created - a.created, old: (a, b) => a.created - b.created, long: (a, b) => b.duration - a.duration, big: (a, b) => b.size - a.size, name: (a, b) => a.title.localeCompare(b.title) }[S.sort];
  return { base, list: [...list].sort(by) };
}

/** A card's name: its own hit area (see editTitle). The tooltip shows the full name, which the card may cut short. */
const titleHtml = (c) => `<span class="ctitle" title="${esc(c.title + "\n" + t("Double-click to rename"))}">${esc(c.title)}</span>`;
function cardHtml(c) {
  const job = S.jobs[c.id];
  const bg = c.thumb
    ? ""
    : `style="background:linear-gradient(135deg,hsl(${hue(c.game)} 45% 22%),hsl(${(hue(c.game) + 50) % 360} 50% 12%))"`;
  // The tag names what made the clip; a Discord copy's is the brand name, so it is not translated.
  // The picture is an <img> fitted by inline styles: a custom theme's `.thumb { background: … }`
  // once reset a background-image's size, so cards showed only the frame's top-left corner.
  return `<article class="card${S.sel.has(c.id) ? " sel" : ""}" data-clip="${c.id}" data-hover>
    <div class="thumb" ${bg}>
      ${c.thumb ? `<img class="thumb-img" src="${esc(fileSrc(c.thumb))}" alt="" draggable="false" decoding="async" onerror="this.remove()" style="${THUMB_IMG_STYLE}">` : `<div class="ph">${esc(c.game.slice(0, 1).toUpperCase())}</div>`}
      <span class="check" title="${esc(t("Select"))}">${ICON.check}</span>
      ${c.favorite ? `<span class="star">${ICON.star}</span>` : ""}
      ${c.kind !== "clip" ? `<span class="tag tag-${esc(c.kind)}">${esc({ trim: t("trim"), edit: t("edit"), import: t("import"), discord: "Discord", joined: t("joined") }[c.kind] || c.kind)}</span>` : ""}
      <span class="dur">${fmtDur(c.duration)}</span>
      ${job && job.state === "running" ? `<div class="job"><i style="width:${job.pct}%"></i></div>` : ""}
    </div>
    <h3>${titleHtml(c)}</h3>
    <p><span>${esc(c.game)}</span>${S.folder === null && folderDiffers(c) ? `<span>${esc(c.folder)}</span>` : ""}<span>${timeAgo(c.created)}</span><span>${fmtSize(c.size)}</span></p>
  </article>`;
}

/** A game's icon: its own (taken from the game's exe, or picked by the user), else a coloured initial. */
function gameIcon(name) {
  const g = S.games.find((x) => x.name.toLowerCase() === String(name).toLowerCase());
  if (g?.icon) return `<img src="${fileSrc(g.icon)}" alt="" draggable="false">`;
  if (name === "Desktop") return `<span class="gi" style="background:var(--raised-2)">${ICON.monitor}</span>`;
  if (name === "Imported") return `<span class="gi" style="background:var(--raised-2)">${ICON.folder}</span>`;
  return `<span class="gi" style="background:hsl(${hue(name)} 45% 34%)">${esc(String(name).slice(0, 1).toUpperCase())}</span>`;
}

/** Game chips above the grid: filter the library by detected game. Right-click one to change its icon. */
const GAMES_SHOWN = 14;
function gameRowHtml(base) {
  const counts = Object.entries(base.reduce((m, c) => ((m[c.game] = (m[c.game] || 0) + 1), m), {})).sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  if (S.tab !== "all" && !counts.some(([g]) => g === S.tab)) S.tab = "all";
  const shown = S.gamesOpen || counts.length <= GAMES_SHOWN ? counts : counts.filter(([g], i) => i < GAMES_SHOWN - 1 || g === S.tab);
  const chip = (id, label, n, icon, cls = "") => `<button class="chip game${cls}${S.tab === id ? " active" : ""}" data-tab="${esc(id)}" data-hover title="${esc(label)}">${icon}<span>${esc(label)}</span><i>${n}</i></button>`;
  return `<div class="games">
    ${chip("all", t("All"), base.length, ICON.game, " all")}
    ${shown.map(([g, n]) => chip(g, g, n, gameIcon(g))).join("")}
    ${counts.length > GAMES_SHOWN ? `<button class="chip game more" data-act="games-more" data-sfx="tap" data-hover><span>${S.gamesOpen ? t("Show fewer") : t("{n} more", { n: counts.length - shown.length })}</span></button>` : ""}
  </div>`;
}

/**
 * The folder chips after "All clips", in order: { id, label, n }, id "" being the main folder.
 * Fullest first (ties by name), the main folder included. Without other folders the main folder
 * would only repeat "All clips", so it is left out; while empty it is left out too, unless
 * `showEmptyMain` or it is the folder being viewed (`cur`).
 */
function folderChipList(folders, clips, cur, showEmptyMain, mainLabel) {
  const counts = {};
  for (const c of clips) counts[c.folder || ""] = (counts[c.folder || ""] || 0) + 1;
  const list = folders.map((f) => ({ id: f, label: f, n: counts[f] || 0 }));
  const main = { id: "", label: mainLabel, n: counts[""] || 0 };
  if (folders.length && (main.n || showEmptyMain || cur === "")) list.push(main);
  return list.sort((a, b) => b.n - a.n || a.label.localeCompare(b.label));
}

/** Folder chips above the grid. Each one filters the library and is a drop target for dragged clips. */
function folderRowHtml() {
  if (S.folder && !S.folders.includes(S.folder)) S.folder = null;
  const cur = S.folder === null ? "*" : S.folder;
  const chip = (id, label, n, icon = "") => `<button class="chip${cur === id ? " active" : ""}" data-folder="${esc(id)}" data-hover title="${esc(label)}">${icon}<span>${esc(label)}</span><i>${n}</i></button>`;
  const list = folderChipList(S.folders, S.clips, S.folder, !!S.settings.show_empty_main, t("Main folder"));
  return `<div class="folders">
    ${chip("*", t("All clips"), S.clips.length)}
    ${list.map((f) => chip(f.id, f.label, f.n, f.id ? ICON.folder : "")).join("")}
    <button class="chip ghost" data-act="folder-new" data-hover>${ICON.plus}<span>${t("New folder")}</span></button>
    ${S.folder ? `<span class="sp"></span><button class="btn sm" data-act="folder-rename">${t("Rename")}</button><button class="btn sm danger" data-act="folder-del">${ICON.trash}${t("Delete folder")}</button>` : ""}
  </div>`;
}

function renderLibrary() {
  const titles = { library: [t("Library"), t("Every clip Glint has saved")], favorites: [t("Favorites"), t("Clips you starred")], rendered: [t("Rendered"), t("Clips finished with Blur")] };
  const [title, sub] = titles[S.page];
  const gameRow = gameRowHtml(visibleClips().base); // may reset S.tab, so it runs before the list is built
  const { list } = visibleClips();
  const binds = clipBinds().filter((k) => k.key);
  const empty = S.clips.length
    ? `<div class="empty"><h2>${t("Nothing here")}</h2>${t("No clips match this view.")}</div>`
    : `<div class="empty"><h2>${t("No clips yet")}</h2>${t("Glint keeps the last {length} of your screen in memory. Press a clip key at any time to save it.", { length: fmtLen(maxSecs()) })}
        <div class="keys">${binds.map((k) => `<div>${t("{keys} saves the last {length}", { keys: comboHtml(k), length: fmtLen(k.seconds) })}</div>`).join("")}</div></div>`;
  $("#view").innerHTML = `
    <div class="page-head">
      <div><h1>${title}</h1><div class="sub">${sub}</div></div>
      <div class="head-actions">
        <button class="btn" data-act="import">${ICON.plus}${t("Import")}</button>
        <button class="btn" data-act="rescan">${ICON.refresh}${t("Rescan folder")}</button>
        <button class="btn primary" data-act="open-dir">${ICON.folder}${t("Open folder")}</button>
      </div>
    </div>
    <div class="toolbar games-bar">
      ${gameRow}
      <select class="field" id="sort">
        ${[["new", t("Newest")], ["old", t("Oldest")], ["long", t("Longest")], ["big", t("Largest")], ["name", t("Name")]].map(([v, l]) => `<option value="${v}"${S.sort === v ? " selected" : ""}>${l}</option>`).join("")}
      </select>
      <div class="seg">${["s", "m", "l"].map((z) => `<button data-size="${z}" class="${S.size === z ? "active" : ""}">${z.toUpperCase()}</button>`).join("")}</div>
    </div>
    ${S.page === "library" ? folderRowHtml() : ""}
    ${list.length ? `<div class="grid ${S.size}">${list.map(cardHtml).join("")}</div>` : empty}`;
}

/* ---------- selection, folders, bulk actions ---------- */
const isLib = () => ["library", "favorites", "rendered"].includes(S.page);
const plural = (n) => (n === 1 ? t("1 clip") : t("{n} clips", { n }));

function renderSelBar() {
  const el = $("#selbar");
  const sel = S.clips.filter((c) => S.sel.has(c.id));
  const on = isLib() && sel.length > 0 && !S.open;
  document.body.classList.toggle("has-sel", on);
  el.hidden = !on;
  if (!on) return closeMenu();
  const allFav = sel.every((c) => c.favorite);
  el.innerHTML = `<b>${t("{n} selected", { n: sel.length })}</b><span class="muted">${fmtSize(sel.reduce((n, c) => n + c.size, 0))}</span>
    <button class="btn sm" data-act="sel-move">${ICON.folder}${t("Move to")}</button>
    <button class="btn sm" data-act="sel-fav" data-sfx="none">${ICON.star}${allFav ? t("Unfavorite") : t("Favorite")}</button>
    <button class="btn sm" data-act="sel-copy">${ICON.copy}${t("Copy")}</button>
    <button class="btn sm danger" data-act="sel-del">${ICON.trash}${t("Delete")}</button>
    <button class="icon-btn" data-act="sel-clear" aria-label="${esc(t("Clear selection"))}" title="${esc(t("Clear selection (Esc)"))}">${ICON.x}</button>`;
}
/** Bring the cards and the selection bar in line with S.sel without rebuilding the grid. */
function syncSel() {
  for (const el of $$("#view .card")) el.classList.toggle("sel", S.sel.has(el.dataset.clip));
  renderSelBar();
}
function clearSel() {
  if (!S.sel.size) return;
  S.sel.clear();
  S.anchor = null;
  syncSel();
}
function clickSelect(id, e) {
  const ids = visibleClips().list.map((c) => c.id);
  const from = ids.indexOf(S.anchor);
  if (e.shiftKey && from >= 0) {
    const to = ids.indexOf(id);
    for (const x of ids.slice(Math.min(from, to), Math.max(from, to) + 1)) S.sel.add(x);
  } else {
    if (S.sel.has(id)) S.sel.delete(id);
    else S.sel.add(id);
    S.anchor = id;
  }
  syncSel();
}

/**
 * Rename a clip in place: its name on the card turns into a text box with the name selected.
 * Enter or a click elsewhere saves, Esc (or an empty name) cancels. The backend renames the
 * file on disk too and returns the clip as saved, whose name may differ from what was typed.
 * While the box is open, render() waits (S.renaming), so a refresh cannot wipe the edit.
 */
function editTitle(id) {
  const c = S.clips.find((k) => k.id === id);
  const h = $$("#view .card").find((el) => el.dataset.clip === id)?.querySelector("h3");
  if (!c || !h || S.renaming) return;
  S.renaming = id;
  const input = document.createElement("input");
  Object.assign(input, { type: "text", className: "title-edit", value: c.title, spellcheck: false, autocomplete: "off", maxLength: 120 });
  input.setAttribute("aria-label", t("Clip name"));
  h.classList.add("editing");
  h.replaceChildren(input);
  input.focus();
  input.select();
  let done = false;
  const finish = async (save) => {
    if (done) return;
    done = true;
    const title = input.value.trim();
    const later = S.renderLater;
    S.renaming = null;
    S.renderLater = false;
    h.classList.remove("editing");
    h.innerHTML = titleHtml(save && title ? { title } : c); // the typed name at once; the saved one follows
    if (!save || !title || title === c.title) return later && render();
    try {
      const meta = await invoke("rename_clip", { id, title });
      // Swapped by the old id: the clip's id may follow its new file name.
      const i = S.clips.findIndex((k) => k.id === id);
      if (i >= 0 && meta) S.clips[i] = meta;
      if (meta && meta.id !== id) {
        if (S.sel.has(id)) S.sel = new Set([...S.sel].map((x) => (x === id ? meta.id : x))); // keeps its place in the selection order
        if (S.anchor === id) S.anchor = meta.id;
      }
    } catch (e) { fail(e); }
    render();
  };
  input.addEventListener("keydown", (e) => {
    e.stopPropagation(); // the keys are the text box's, not the app's
    if (e.code === "Enter" || e.code === "NumpadEnter") { e.preventDefault(); finish(true); }
    else if (e.code === "Escape") { e.preventDefault(); finish(false); }
  });
  // Leaving the window (Alt+Tab) also takes the focus away; the edit stays open for the return.
  input.addEventListener("blur", () => { if (document.hasFocus()) finish(true); });
}
document.addEventListener("dblclick", (e) => {
  const name = e.target.closest("#view .card .ctitle");
  if (name && isLib() && !S.open) editTitle(name.closest(".card").dataset.clip);
});

/* One pop-up menu, used by the selection bar's "Move to" and by right-click.
   Items are <button data-m="action" data-arg="…">; menuIds are the clips it acts on. */
let menuIds = [];
// `sfx`: the item's own sound instead of the click (sfx.js), e.g. "none" where the action plays one.
const mItem = (m, label, icon = "", arg = "", cls = "", sfx = "") => `<button data-m="${m}" data-arg="${esc(arg)}" class="${cls}"${sfx ? ` data-sfx="${sfx}"` : ""}>${icon || "<svg></svg>"}<span>${esc(label)}</span></button>`;
const mSub = (label, icon, inner) => `<div class="has-sub"><button tabindex="-1">${icon}<span>${esc(label)}</span><b class="chev">›</b></button><div class="submenu">${inner}</div></div>`;
function closeMenu() { const m = $("#menu"); m.hidden = true; m.innerHTML = ""; m.className = "menu"; m.style.cssText = ""; }
/** Folder choices for `ids`: the main folder, every folder, and "New folder…". */
function folderItems(ids) {
  const clips = S.clips.filter((c) => ids.includes(c.id));
  const here = (f) => (clips.length && clips.every((c) => (c.folder || "") === f) ? "on" : "");
  return mItem("move", t("Main folder"), ICON.library, "", here("")) + S.folders.map((f) => mItem("move", f, ICON.folder, f, here(f))).join("") + "<hr>" + mItem("move-new", t("New folder…"), ICON.plus);
}
function openMoveMenu(anchor) {
  const m = $("#menu");
  if (!m.hidden) return closeMenu();
  menuIds = [...S.sel];
  m.innerHTML = folderItems(menuIds);
  m.hidden = false;
  const r = anchor.getBoundingClientRect();
  m.style.left = `${r.left}px`;
  m.style.bottom = `${window.innerHeight - r.top + 8}px`;
}
/** Show the menu at the pointer, kept inside the window. Submenus open towards the side with room. */
function placeMenu(x, y) {
  const m = $("#menu");
  m.className = "menu ctx";
  m.hidden = false;
  const r = m.getBoundingClientRect();
  m.style.left = `${clamp(x, 8, window.innerWidth - r.width - 8)}px`;
  m.style.top = `${clamp(y, 8, window.innerHeight - r.height - 8)}px`;
  m.classList.toggle("flip", x + r.width + 240 > window.innerWidth);
  m.classList.toggle("up", y > window.innerHeight * 0.55);
}
/** Right-click on a clip. On a clip that is part of a selection of several, the menu acts on all of them. */
function openClipMenu(id, x, y) {
  const c = S.clips.find((k) => k.id === id);
  if (!c) return closeMenu();
  const many = S.sel.has(id) && S.sel.size > 1;
  menuIds = many ? [...S.sel] : [id];
  const clips = S.clips.filter((k) => menuIds.includes(k.id));
  const allFav = clips.every((k) => k.favorite);
  const counts = S.clips.reduce((map, k) => ((map[k.game] = (map[k.game] || 0) + 1), map), {});
  const games = Object.keys(counts).sort((a, b) => counts[b] - counts[a]).slice(0, 14);
  const gameItems = games.map((g) => mItem("game", g, "", g, clips.every((k) => k.game === g) ? "on" : "")).join("") + "<hr>" + mItem("game-new", t("New game…"), ICON.plus);
  // Compress for Discord: the size used last comes first and is ticked. With several clips, one job each.
  const lastMb = COMPRESS_MB.includes(+S.settings.compress_mb) ? +S.settings.compress_mb : 10;
  const compressItems = [lastMb, ...COMPRESS_MB.filter((n) => n !== lastMb)].map((n) => mItem("compress", t("{n} MB", { n }), "", String(n), n === lastMb ? "on" : "", "launch")).join("")
    + `<div class="menu-hint">${esc(t("10 MB: free · 50 MB: Nitro Basic · 500 MB: Nitro"))}</div>`;
  $("#menu").innerHTML = (many ? `<div class="menu-head">${t("{n} selected", { n: menuIds.length })}</div>` : mItem("open", t("Open"), PICON.play))
    + mItem("copy", many ? t("Copy files") : t("Copy file"), ICON.copy)
    + mItem("fav", allFav ? t("Remove favorite") : t("Favorite"), ICON.star, "", "", "none")
    + (many ? "" : mItem("rename", t("Rename…"), ICON.edit))
    + "<hr>" + mSub(t("Game"), ICON.game, gameItems) + mSub(t("Folder"), ICON.folder, folderItems(menuIds)) + "<hr>"
    + (many ? (typeof joinMenuItem === "function" ? joinMenuItem(clips) : "") + mSub(t("Compress for Discord"), ICON.compress, compressItems) + "<hr>"
      : mItem("edit", t("Open in editor"), ICON.cut) + (S.blur ? mItem("blur", t("Render with Blur"), ICON.blur, "", "", "launch") : "") + mSub(t("Compress for Discord"), ICON.compress, compressItems) + mItem("reveal", t("Open file location"), ICON.folder) + "<hr>")
    + (many ? mItem("deselect", t("Clear selection"), ICON.x) : mItem("select", S.sel.has(id) ? t("Deselect") : t("Select"), ICON.check))
    + mItem("delete", t("Delete"), ICON.trash, "", "danger");
  placeMenu(x, y);
}
/** Right-click on a game chip. */
function openGameMenu(name, x, y) {
  menuIds = [];
  $("#menu").innerHTML = mItem("game-filter", t("Show {game} clips", { game: name }), ICON.game, name) + mItem("game-icon", t("Change icon…"), ICON.image, name);
  placeMenu(x, y);
}

/** Right-click on a folder chip or a folder in the sidebar. */
function openFolderMenu(name, x, y) {
  menuIds = [];
  $("#menu").innerHTML = mItem("folder-open", t("Open"), ICON.folder, name) + mItem("folder-rename", t("Rename…"), ICON.edit, name) + "<hr>" + mItem("folder-del", t("Delete folder"), ICON.trash, name, "danger");
  placeMenu(x, y);
}

async function renameFolder(name) {
  const to = await askText(t('Rename the folder "{name}"', { name }), name, t("Rename"));
  if (!to || to === name) return;
  const made = (await invoke("rename_folder", { from: name, to })) || to;
  if (S.folder === name) S.folder = made;
  await refreshClips();
}
async function deleteFolder(name) {
  const n = S.clips.filter((c) => c.folder === name).length;
  if (!(await ask(n ? t('Delete the folder "{name}"? Its {clips} move back to the main folder. No clip is deleted.', { name, clips: plural(n) }) : t('Delete the empty folder "{name}"?', { name }), t("Delete folder")))) return;
  try { await invoke("delete_folder", { name }); flash(t("Folder deleted")); } finally { if (S.folder === name) S.folder = null; await refreshClips(); }
}

async function menuAction(m, arg) {
  const ids = menuIds;
  const id = ids[0];
  const c = S.clips.find((k) => k.id === id);
  // With game folders on, a retag can also move the file, so reload the list.
  const setGame = async (game) => { try { await invoke("set_game", { ids, game }); } finally { await refreshClips(); } };
  switch (m) {
    case "open": return openClip(id);
    case "copy": {
      if (ids.length === 1) { await invoke("copy_clip", { id }); return flash(t("Clip copied. Paste it anywhere.")); }
      const n = await invoke("copy_clips", { ids });
      return flash(t("{clips} copied. Paste them anywhere.", { clips: plural(n ?? ids.length) }));
    }
    case "fav": {
      const clips = S.clips.filter((k) => ids.includes(k.id));
      const favorite = !clips.every((k) => k.favorite);
      await invoke("set_favorites", { ids, favorite });
      for (const k of clips) k.favorite = favorite;
      sfx(favorite ? "star" : "off");
      return render();
    }
    case "rename": return editTitle(id);
    case "compress": return compressIds(ids, +arg || 10);
    case "join": return joinClips(ids); // join.js
    case "game": return setGame(arg);
    case "game-new": { const game = await askText(t("Game name"), c?.game || "", t("Set game")); if (game) await setGame(game); return; }
    case "move": return moveIds(ids, arg);
    case "move-new": { const name = await newFolder(); if (name) await moveIds(ids, name); return; }
    case "edit": {
      openClip(id);
      // Trim mode needs the clip's length, which arrives with the video's metadata.
      const v = $("#modal video");
      const go = () => { if (S.open === id && S.player) S.player.setTrim(true); };
      if (v && v.readyState >= 1) go(); else v?.addEventListener("loadedmetadata", go, { once: true });
      return;
    }
    case "blur":
      S.jobs[id] = { pct: 0, state: "running", message: "" };
      render();
      try { await invoke("blur_clip", { id }); } catch (e) { S.jobs[id] = { pct: 0, state: "error", message: String(e) }; render(); throw e; }
      return;
    case "reveal": return invoke("reveal_clip", { id });
    case "select": if (S.sel.has(id)) S.sel.delete(id); else S.sel.add(id); S.anchor = id; return syncSel();
    case "deselect": return clearSel();
    case "delete": {
      if (!(await ask(ids.length === 1 ? t('Move "{title}" to the Recycle Bin?', { title: c.title }) : t("Move {clips} to the Recycle Bin?", { clips: plural(ids.length) }), t("Move to Recycle Bin")))) return;
      try { await invoke("delete_clips", { ids }); sfx("close"); flash(t("{clips} moved to the Recycle Bin", { clips: plural(ids.length) })); } finally { await refreshClips(); }
      return;
    }
    case "game-filter": S.tab = arg; S.sel.clear(); return render();
    case "game-icon": {
      const g = await invoke("pick_game_icon", { game: arg });
      if (!g) return;
      S.games = (await invoke("list_games")) || [];
      return render();
    }
    case "folder-open": S.page = "library"; S.folder = arg; S.tab = "all"; S.sel.clear(); return render();
    case "folder-rename": return renameFolder(arg);
    case "folder-del": return deleteFolder(arg);
  }
}

async function moveIds(ids, folder) {
  if (!ids.length) return;
  try {
    const n = await invoke("move_clips", { ids, folder });
    for (const id of ids) S.sel.delete(id);
    await refreshClips();
    flash(n ? (folder ? t("Moved {clips} to the folder {folder}", { clips: plural(n), folder }) : t("Moved {clips} to the main folder", { clips: plural(n) })) : ids.length === 1 ? t("That clip is already there") : t("Those clips are already there"));
  } catch (e) { await refreshClips(); fail(e); }
}
const moveSel = (folder) => moveIds([...S.sel], folder);

/**
 * Compress each clip in `ids` for Discord, aiming under `mb` megabytes. Each one is its own job in
 * the background queue (the renders button lists it); "clip-compressed" reports each result.
 * The size picked is remembered as the one to list first.
 */
async function compressIds(ids, mb) {
  S.settings.compress_mb = mb;
  const errors = [];
  for (const id of ids) {
    try {
      const job = await invoke("compress_clip", { id, mb });
      // Shown at once; the backend's "jobs-changed" brings the real list right after.
      if (job && job.id && typeof jobsSet === "function" && !jobsList.some((j) => j.id === job.id)) jobsSet([...jobsList, job]);
    } catch (e) { errors.push(t(String(e?.message ?? e))); }
  }
  if (errors.length && ids.length === 1) return flash(errors[0], true);
  if (errors.length) return flash(errors.length === ids.length ? errors[0] : t("{n} of the clips were not compressed: {reason}", { n: errors.length, reason: errors[0] }), true);
  flash(ids.length === 1 ? t("Compressing for Discord…") : t("Compressing {clips} for Discord…", { clips: plural(ids.length) }));
}

document.addEventListener("contextmenu", (e) => {
  if (e.target.closest("input, textarea")) return; // typing fields keep the cut / copy / paste menu
  e.preventDefault();
  const card = e.target.closest("#view .card");
  const folder = e.target.closest("[data-folder]")?.dataset.folder;
  const game = e.target.closest(".chip.game[data-tab]")?.dataset.tab;
  if (card && isLib() && !S.open) openClipMenu(card.dataset.clip, e.clientX, e.clientY);
  else if (game && game !== "all" && !S.open) openGameMenu(game, e.clientX, e.clientY);
  else if (folder && folder !== "*" && !S.open) openFolderMenu(folder, e.clientX, e.clientY);
  else closeMenu();
});
/** Ask for a name and create the folder. Returns the name as created, or null. */
async function newFolder() {
  const name = await askText(t("Name the new folder"), "", t("Create folder"));
  if (!name) return null;
  const made = await invoke("create_folder", { name });
  S.folders = (await invoke("list_folders")) || [];
  return made || name;
}
async function bulkDelete() {
  const ids = [...S.sel];
  if (!ids.length || !(await ask(t("Move {clips} to the Recycle Bin?", { clips: plural(ids.length) }), t("Move to Recycle Bin")))) return;
  try {
    const n = await invoke("delete_clips", { ids });
    sfx("close");
    S.sel.clear();
    await refreshClips();
    flash(t("{clips} moved to the Recycle Bin", { clips: plural(n ?? ids.length) }));
  } catch (e) { await refreshClips(); fail(e); }
}

/* Dragging in the library. A drag that starts on a selected clip carries the
   selection (drop it on a folder chip to move it); any other drag draws a
   selection box. Ctrl or Shift adds to the current selection. */
let drag = null;
let suppressClick = false; // swallow the click that follows a drag
const view = $("#view");
const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));

view.addEventListener("mousedown", (e) => {
  endDrag(false); // a drag whose release was missed must not linger under the new one
  if (e.button !== 0 || !isLib() || S.open || !$(".grid", view)) return;
  if (e.target.closest("button, select, input, .check")) return;
  const vr = view.getBoundingClientRect();
  if (e.clientX - vr.left >= view.clientWidth) return; // the scroll bar
  drag = { x: e.clientX, y: e.clientY, cx: e.clientX, cy: e.clientY, card: e.target.closest(".card")?.dataset.clip || null, add: e.ctrlKey || e.shiftKey, mode: null };
});

/**
 * Finish the drag in progress, if any, and clear away everything it put on screen.
 * `drop` is a real release: a box drag fades its box out, a clip drag returns its drop target.
 * Otherwise (the release was missed: Alt+Tab, a screenshot tool, a lost pointer) the drag is
 * dropped without moving anything. Returns the drag for the caller.
 */
function endDrag(drop) {
  const d = drag;
  if (!d) return null;
  drag = null;
  clearInterval(d.timer);
  document.body.classList.remove("selecting", "moving-clips");
  d.ghost?.remove();
  d.target?.classList.remove("drop");
  if (d.box) {
    const box = d.box;
    box.classList.add("out"); // style.css: fade out and shrink a little toward the centre (150 ms)
    setTimeout(() => box.remove(), 170);
  }
  if (d.mode === "box") S.anchor = null;
  return drop ? d : null;
}
// Every way the release can go missing ends the drag: the window losing focus or being hidden,
// the pointer being taken away, or the mouse coming back with no button held.
window.addEventListener("blur", () => endDrag(false));
document.addEventListener("visibilitychange", () => { if (document.hidden) endDrag(false); });
window.addEventListener("pointercancel", () => endDrag(false));
// Fired around a normal release too, so it waits a moment: by then a real mouseup has run.
window.addEventListener("lostpointercapture", () => { const d = drag; if (d) setTimeout(() => { if (drag === d) endDrag(false); }, 0); });
document.documentElement.addEventListener("mouseleave", (e) => { if (e.buttons === 0) endDrag(false); });

function boxUpdate() {
  const vr = view.getBoundingClientRect();
  const x = clamp(drag.cx - vr.left, 0, view.clientWidth);
  const y = clamp(drag.cy - vr.top + view.scrollTop, 0, drag.maxY);
  const l = Math.min(x, drag.ox), r = Math.max(x, drag.ox), tp = Math.min(y, drag.oy), b = Math.max(y, drag.oy);
  if (!drag.box.isConnected) view.append(drag.box); // the page was re-rendered under the drag
  Object.assign(drag.box.style, { left: `${l}px`, top: `${tp}px`, width: `${r - l}px`, height: `${b - tp}px` });
  const next = new Set(drag.base);
  for (const c of drag.cards) if (c.l < r && c.r > l && c.t < b && c.b > tp) next.add(c.id);
  S.sel = next;
  syncSel();
}

window.addEventListener("mousemove", (e) => {
  if (!drag) return;
  if (e.buttons === 0) return endDrag(false); // the button was let go where Glint could not see it
  drag.cx = e.clientX;
  drag.cy = e.clientY;
  if (!drag.mode) {
    if (Math.hypot(drag.cx - drag.x, drag.cy - drag.y) < 6) return;
    for (const el of $$(".marquee, .drag-ghost")) el.remove(); // leftovers of an earlier drag
    if (drag.card && S.sel.has(drag.card)) {
      drag.mode = "move";
      drag.ghost = document.createElement("div");
      drag.ghost.className = "drag-ghost";
      drag.ghost.textContent = t("Move {clips}", { clips: plural(S.sel.size) });
      document.body.append(drag.ghost);
      document.body.classList.add("moving-clips");
    } else {
      // Card positions are taken once, in the scrolling content's own coordinates.
      const vr = view.getBoundingClientRect();
      const top = view.scrollTop - vr.top;
      drag.mode = "box";
      drag.base = drag.add ? new Set(S.sel) : new Set();
      drag.ox = drag.x - vr.left;
      drag.oy = drag.y + top;
      drag.maxY = view.scrollHeight;
      drag.cards = $$(".card", view).map((el) => { const r = el.getBoundingClientRect(); return { id: el.dataset.clip, l: r.left - vr.left, r: r.right - vr.left, t: r.top + top, b: r.bottom + top }; });
      drag.box = document.createElement("div");
      drag.box.className = "marquee";
      view.append(drag.box);
      document.body.classList.add("selecting");
      // Keep scrolling while the pointer rests near the top or bottom edge.
      drag.timer = setInterval(() => {
        const r = view.getBoundingClientRect();
        const dy = drag.cy < r.top + 48 ? -Math.ceil((r.top + 48 - drag.cy) / 3) : drag.cy > r.bottom - 48 ? Math.ceil((drag.cy - r.bottom + 48) / 3) : 0;
        if (dy) { view.scrollTop += clamp(dy, -40, 40); boxUpdate(); }
      }, 16);
    }
  }
  if (drag.mode === "box") return boxUpdate();
  drag.ghost.style.transform = `translate(${drag.cx + 14}px, ${drag.cy + 16}px)`;
  const over = document.elementFromPoint(drag.cx, drag.cy)?.closest('[data-folder], [data-act="folder-new"]');
  const target = over && over.dataset.folder !== "*" ? over : null;
  if (target !== drag.target) {
    drag.target?.classList.remove("drop");
    target?.classList.add("drop");
    drag.target = target;
  }
});

window.addEventListener("mouseup", async () => {
  const d = endDrag(true);
  if (!d) return;
  if (!d.mode) {
    // A plain click on empty space drops the selection.
    if (!d.card && !d.add) clearSel();
    return;
  }
  suppressClick = true;
  setTimeout(() => (suppressClick = false), 0);
  if (d.mode === "box" || !d.target) return;
  try {
    if ("folder" in d.target.dataset) return await moveSel(d.target.dataset.folder);
    const name = await newFolder();
    if (name) await moveSel(name);
  } catch (e) { fail(e); }
});

/* ---------- settings pages ---------- */
/** Put text on the clipboard; falls back to a hidden text box where the clipboard API is refused. */
async function copyText(text) {
  try { await navigator.clipboard.writeText(text); return; } catch {}
  const box = document.createElement("textarea");
  box.value = text;
  box.style.cssText = "position:fixed;opacity:0";
  document.body.append(box);
  box.select();
  document.execCommand("copy");
  box.remove();
}

// What the user pastes into an AI to get a theme. It is addressed to the AI, so it is not translated.
const THEME_PROMPT = `Write a CSS theme for a desktop app called Glint (a game clip library, dark UI). Reply with CSS only, in one code block, nothing else.

How theming works: the app reads its colours from CSS variables on :root. Set these to recolour everything:
  --bg         window background (darkest)
  --panel      cards, side panels, dialogs
  --raised     buttons and raised surfaces
  --raised-2   hover state of raised surfaces
  --line       faint borders
  --line-2     stronger borders
  --text       main text
  --muted      secondary text
  --dim        faint text and disabled items
  --accent     highlight colour: active items, primary buttons, progress bars
               (it also colours the big star of the taskbar and tray icon when "Match the taskbar icon to the theme" is on)
  --accent-2   lighter accent, for hover (also the small star of that icon)
  --accent-ink text drawn on top of the accent colour (must contrast with --accent)
  --danger, --ok, --warn   red, green, yellow status colours
  --radius     corner radius, e.g. 8px
  --sans, --serif, --mono  font stacks (--serif is used for the big page titles and the logo)

Useful selectors if you want to go further than colours:
  .topbar (top bar), .side (left sidebar), .nav-item and .nav-item.active (sidebar entries),
  .page-head h1 (page title), .card and .thumb (clip cards and their thumbnails), .chip and .chip.active (filter chips),
  .btn and .btn.primary (buttons), .section and .section > h2 (settings groups and their titles),
  .row (one setting), .statusbar (bottom bar), .viewer (clip viewer), .toast (the clip-saved overlay)

Rules: keep text readable (contrast of at least 4.5:1 between --text and --bg, and between --accent-ink and --accent). Keep it a dark theme unless I ask for a light one. Do not hide or move anything; only restyle. Do not use @import or url() to load anything from the internet.

The look I want: `;

/** Settings that follow from this PC's hardware: [settings path, recommended value, why]. */
function recommended() {
  const hw = S.hardware || {};
  const gpu = hw.gpu ? hw.gpu.name : "";
  const encName = { nvenc: "NVIDIA NVENC", amf: "AMD AMF", qsv: "Intel Quick Sync", x264: "x264" }[hw.encoder] || "";
  return {
    encoder: { value: hw.encoder || "auto", also: ["auto"], label: encName, why: gpu ? t("Your graphics card: {gpu}", { gpu }) : t("No graphics card with a video encoder was found") },
    method: { value: "auto", also: [], label: t("Auto"), why: t("Picks the fastest capture your Windows version has") },
    aspect: { value: "auto", also: [], label: t("Auto"), why: t("Fixes stretched resolutions and leaves everything else alone") },
    bitrate: { value: 0, also: [], label: t("Auto"), why: t("Uses only as much as the picture needs, which keeps the buffer small") },
    height: { value: 0, also: [], label: t("Native"), why: t("Your graphics card scales and encodes this without trouble") },
    "blur.gpu_type": { value: hw.gpu ? hw.gpu.vendor : "nvidia", also: [], label: "", why: "" },
  };
}

/** Outline the controls that are on their recommended value, and offer the recommended one where they are not. */
function markRecommended(paths) {
  const rec = recommended();
  for (const path of paths) {
    const el = $(`#view [data-set="${path}"]`);
    const r = rec[path];
    if (!el || !r) continue;
    const cur = getPath(S.settings, path);
    const good = String(cur) === String(r.value) || r.also.includes(cur);
    el.classList.toggle("rec", good);
    const name = el.closest(".row")?.querySelector(".name");
    if (!name) continue;
    name.insertAdjacentHTML("beforeend", good
      ? `<span class="rec-tag" title="${esc(r.why)}">${t("Recommended")}</span>`
      : `<button class="rec-use" data-act="rec-use" data-rec-path="${path}" data-rec-value="${esc(r.value)}"${typeof r.value === "number" ? " data-num" : ""} title="${esc(r.why)}">${t("Use recommended: {value}", { value: r.label })}</button>`);
  }
}

/**
 * Picking a hardware encoder (or Blur GPU type) made for another brand of
 * graphics card than the one in this PC: ask first. Resolves to true to go ahead.
 */
async function confirmHardwareChoice(path, value) {
  const hw = S.hardware || {};
  if (!hw.gpu) return true;
  const brand = { nvidia: "NVIDIA", amd: "AMD", intel: "Intel" };
  const wants = path === "encoder" ? { nvenc: "nvidia", amf: "amd", qsv: "intel" }[value] : value;
  const has = (v) => (hw.gpus || []).some((g) => g.vendor === v);
  if (path === "encoder" && value === "x264") {
    return ask(t("x264 encodes on the processor, not the graphics card. It works, but it uses far more CPU while you play. Your graphics card ({gpu}) can do this job almost for free. Use x264 anyway?", { gpu: hw.gpu.name }), t("Use x264"));
  }
  if (!wants || has(wants)) return true;
  return ask(t("Are you sure? The graphics card in this PC is: {gpu}. The {brand} option only works with {brand} graphics, so it will most likely fail here.", { gpu: hw.gpu.name, brand: brand[wants] || wants }), t("Use it anyway"));
}
const row = (name, hint, ctl) => `<div class="row"><div class="txt"><div class="name">${name}</div>${hint ? `<div class="hint">${hint}</div>` : ""}</div><div class="ctl">${ctl}</div></div>`;
const toggle = (path) => `<label class="switch"><input type="checkbox" data-set="${path}" ${getPath(S.settings, path) ? "checked" : ""}><i></i></label>`;
const select = (path, opts, num = false) => {
  const cur = getPath(S.settings, path);
  return `<select class="field" data-set="${path}" ${num ? "data-num" : ""}>${opts.map(([v, l]) => `<option value="${esc(v)}"${String(cur) === String(v) ? " selected" : ""}>${esc(l)}</option>`).join("")}</select>`;
};
const number = (path, min, max, step = 1) => `<input class="field" type="number" data-set="${path}" data-num min="${min}" max="${max}" step="${step}" value="${getPath(S.settings, path)}">`;
const pageHead = (title, sub) => `<div class="page-head"><div><h1>${title}</h1><div class="sub">${sub}</div></div></div>`;

const DURATIONS = [15, 30, 60, 120, 180, 300, 600, 900, 1800];
function renderKeybinds() {
  const s = S.settings;
  const seen = {};
  // With more than one monitor, a clip keybind also picks the monitor it saves (data-kbf="monitor").
  const multi = S.monitors.length > 1;
  const cap = capMon();
  const rows = s.keybinds.map((k, i) => {
    const combo = comboText(k); // the keys alone: the same keys on two monitors are still a duplicate
    const dup = combo && seen[combo] !== undefined;
    if (combo) seen[combo] = i;
    const listening = S.capturing === i;
    const clip = k.action === "clip";
    const swap = k.action === "monitor"; // switches the capture monitor: to its own monitor, or to the next one (-1)
    const durs = DURATIONS.includes(k.seconds) ? DURATIONS : [...DURATIONS, k.seconds].sort((a, b) => a - b);
    const mon = kbMon(k);
    const lost = multi && clip && kbNamed(k) && mon < 0; // names a monitor that is not connected (the setting is kept for when it is back)
    const monSel = `<select class="field kb-mon" data-kb="${i}" data-kbf="monitor" title="${esc(swap ? t("Which monitor this keybind switches to") : t("Which monitor this keybind clips"))}">
        <option value="-1"${mon < 0 ? " selected" : ""}>${swap ? t("Next monitor") : t("Capture monitor ({name})", { name: esc(cap.name) })}</option>
        ${S.monitors.map((m) => `<option value="${m.index}"${mon === m.index ? " selected" : ""}>${esc(monShort(m))}</option>`).join("")}
      </select>`;
    // Each cell has its own grid area (style.css), so the row can wrap in narrow windows.
    return `<div class="kb-row${clip || (swap && multi) ? "" : " kb-other"}">
      <button class="kb-keys${listening ? " listening" : ""}${k.key ? "" : " unset"}" data-kb="${i}" data-act="capture">${listening ? t("Press a key combination…") : k.key ? comboHtml(k) : t("Click to set key")}</button>
      <select class="field kb-act" data-kb="${i}" data-kbf="action">
        ${[["clip", t("Save a clip")], ["toggle", t("Pause / resume clipping")], ["show", t("Open Glint")], ...(multi || swap ? [["monitor", t("Switch the capture monitor")]] : [])].map(([v, l]) => `<option value="${v}"${k.action === v ? " selected" : ""}>${l}</option>`).join("")}
      </select>
      ${clip ? `<select class="field kb-len" data-kb="${i}" data-kbf="seconds">${durs.map((d) => `<option value="${d}"${k.seconds === d ? " selected" : ""}>${t("Last {length}", { length: fmtLen(d) })}</option>`).join("")}</select>` : ""}
      ${(clip || swap) && multi ? monSel : ""}
      <button class="icon-btn kb-del" data-kb="${i}" data-act="kb-del" aria-label="${esc(t("Remove keybind"))}">${ICON.trash}</button>
      ${dup ? `<div class="kb-warn">${t("{combo} is already used above. Only the first binding will fire.", { combo: esc(combo) })}</div>` : ""}
      ${lost ? `<div class="kb-warn">${t("Its monitor is not connected, so this keybind clips the capture monitor until it is back.")}</div>` : ""}
    </div>`;
  });
  const extra = kbExtraMons();
  const bold = (m) => `<b style="color:var(--text)">${esc(m.name)}</b>`;
  const mbps = S.status.max_mbps || 0;
  const worst = (mbps * maxSecs()) / 8 * 1e6;
  const longest = `<b style="color:var(--text)">${fmtLen(maxSecs())}</b>`;
  $("#view").innerHTML = pageHead(t("Keybinds"), t("Any number of keys, each with its own action and clip length")) + `
    <div class="note">${t("The same key can carry several bindings. <b>F8</b> and <b>Shift + F8</b> are separate: the binding with the most matching modifiers wins, and keys are never blocked from reaching your game.")}</div>
    <div class="section"><h2>${t("Bindings")}</h2>
      ${rows.length ? `<div class="kb-list${multi ? " multi" : ""}">${rows.join("")}</div>` : `<div class="row"><div class="txt hint">${t("No keybinds yet.")}</div></div>`}
      <div class="kb-foot"><span>${!mbps ? t("Buffer length follows the longest clip: {length}", { length: longest }) : bufferPlan().disk ? t("Buffer length follows the longest clip: {length} · at most {size}, usually far less", { length: longest, size: fmtSize(worst) }) : t("Buffer length follows the longest clip: {length} · at most {size} of RAM, usually far less", { length: longest, size: fmtSize(worst) })}${extra.length ? `<span class="kb-multi">${t("Buffered at once: {list}. Each monitor is its own capture, so each one uses the graphics card's encoder, memory and disk space again.", { list: [cap, ...extra].map(bold).join(", ") })}</span>` : ""}</span>
      <button class="btn sm" data-act="kb-add">${ICON.plus}${t("Add keybind")}</button></div>
    </div>`;
}

/** Where the replay buffer will live with the current settings, and roughly how big each part gets. Mirrors spill::window(). */
function bufferPlan() {
  const s = S.settings;
  const total = maxSecs() + 5;
  const window = s.buffer_ram_secs || 30;
  const mode = s.buffer === "ram" || s.buffer === "disk" ? s.buffer : "mixed";
  // "RAM + disk" on a buffer barely longer than its RAM part stays in RAM.
  const allRam = mode === "ram" || (mode === "mixed" && total <= window + 15);
  const ramSecs = allRam ? total : mode === "disk" ? 0 : window;
  const perSec = ((s.bitrate || S.status.max_mbps || 0) * 1e6) / 8 + ((s.desktop_audio ? 1 : 0) + (s.mic ? 1 : 0) + (s.game_audio ? 1 : 0)) * 192000;
  return { mode, allRam, window, ram: perSec * Math.max(ramSecs, 1), disk: allRam ? 0 : perSec * (total - ramSecs) };
}

function renderRecording() {
  const s = S.settings;
  const mon = S.monitors[s.monitor] || S.monitors[0] || { width: 1920, height: 1080, hz: 60 };
  const heights = [720, 900, 1080, 1200, 1440, 1600, 2160].filter((h) => h < mon.height);
  const even = (v) => Math.floor(v / 2) * 2;
  // Mirror of the backend's target_ratio(): the shape clips are saved in.
  const src = mon.width / mon.height;
  const display = (mon.native_width || mon.width) / (mon.native_height || mon.height);
  const parsed = /^(\d+(?:\.\d+)?):(\d+(?:\.\d+)?)$/.exec(s.aspect || "");
  const ratio = s.aspect === "source" ? src : parsed ? parsed[1] / parsed[2] : src < display * 0.92 ? display : src;
  const widthAt = (h) => even(Math.round(Math.abs(ratio / src - 1) < 0.01 ? (mon.width * h) / mon.height : h * ratio));
  const stretched = Math.abs(ratio / src - 1) >= 0.01;
  const resOpts = [...heights.map((h) => [h, `${h}p  (${widthAt(h)}×${h})`]), [0, `${t("Native")}  (${widthAt(mon.height)}×${mon.height})`]];
  const aspectHint = stretched
    ? t("Your screen is set to {res}, which is narrower than the display. Clips are widened to {wide} so they fill the frame instead of showing black bars", { res: `${mon.width}×${mon.height}`, wide: `${widthAt(mon.height)}×${mon.height}` })
    : t("Auto keeps clips as they are, and widens them only when you play on a stretched resolution");
  const fpsList = [...new Set([15, 24, 30, 48, 60, 75, 90, 100, 120, 144, 165, 180, 200, 240, 280, 360, mon.hz])].filter((f) => f <= Math.max(60, mon.hz)).sort((a, b) => a - b);
  const st = S.status;
  const plan = bufferPlan();
  const sizes = { secs: fmtLen(plan.window), ram: fmtSize(plan.ram), disk: fmtSize(plan.disk) };
  const bufferHint = plan.mode === "ram" ? t("Fastest, and nothing is written to disk. Uses up to {ram} of RAM", sizes)
    : plan.mode === "disk" ? t("Lowest RAM use: the whole buffer is kept in files. Up to {disk} on disk", sizes)
    : plan.allRam ? t("Your longest clip is short enough to stay in RAM, so nothing is written to disk. Up to {ram} of RAM", sizes)
    : t("The newest {secs} stay in RAM and the rest is kept in files. Up to {ram} of RAM and {disk} on disk", sizes);
  const bufferDir = s.buffer_dir || "%LOCALAPPDATA%\\Glint\\buffer";
  const extra = kbExtraMons(); // these keep their own capture whatever the Monitor dropdown says
  $("#view").innerHTML = pageHead(t("Capture"), t("What the replay buffer captures")) + `
    ${st.error && st.recording ? `<div class="note bad"><b>${t("Capture problem:")}</b> ${esc(st.error)}${typeof depsErrorExtra === "function" ? depsErrorExtra(st.error) : ""}</div>` : ""}
    <div class="section"><h2>${t("Video")}</h2>
      ${row(t("Monitor"), t("The screen that is captured") + (extra.length ? `<br>${t("Also buffered for a keybind: {list}", { list: extra.map((m) => esc(m.name)).join(", ") })}` : ""), select("monitor", S.monitors.map((m) => [m.index, monLabel(m)]), true))}
      ${row(t("Resolution"), t("Scaled on the GPU. Lower is lighter and smaller"), select("height", resOpts, true))}
      ${row(t("Aspect ratio"), aspectHint, select("aspect", [["auto", t("Auto (fix stretched res)")], ["16:9", "16:9"], ["16:10", "16:10"], ["21:9", "21:9"], ["32:9", "32:9"], ["4:3", "4:3"], ["source", t("As captured (no fix)")]]))}
      ${row(t("Frame rate"), t("Up to your monitor's {hz} Hz. High rates suit Blur renders", { hz: mon.hz }), select("fps", fpsList.map((f) => [f, `${f} fps`]), true))}
      ${row(t("Bitrate"), s.bitrate ? (plan.disk ? t("Fixed. Moving pictures use all of it; the {length} buffer can reach {size}", { length: fmtLen(maxSecs()), size: fmtSize((s.bitrate * 1e6 * maxSecs()) / 8) }) : t("Fixed. Moving pictures use all of it; the {length} buffer can reach {size} of RAM", { length: fmtLen(maxSecs()), size: fmtSize((s.bitrate * 1e6 * maxSecs()) / 8) })) : t("Auto picks the bitrate from the Quality level below"), select("bitrate", [[0, t("Auto")], ...[...new Set([5, 8, 10, 15, 20, 30, 40, 50, 60, 80, 100, s.bitrate])].filter(Boolean).sort((a, b) => a - b).map((b) => [b, `${b} Mbps`])], true))}
      ${s.bitrate ? "" : row(t("Quality"), st.max_mbps ? t("Current ceiling {mbps} Mbps; quiet scenes use much less", { mbps: st.max_mbps.toFixed(0) }) : t("Quality-targeted bitrate"), select("quality", [["low", t("Low")], ["medium", t("Medium")], ["high", t("High")], ["ultra", t("Ultra")]]))}
      ${row(t("Encoder"), t("Auto picks your GPU's hardware encoder"), select("encoder", [["auto", t("Auto")], ["nvenc", "NVIDIA NVENC"], ["amf", "AMD AMF"], ["qsv", "Intel Quick Sync"], ["x264", t("x264 (CPU, heavy)")]]))}
      ${row(t("Capture method"), t("Graphics Capture is smoother at high frame rates") + win10CaptureHint(), select("method", [["auto", t("Auto")], ["wgc", "Windows Graphics Capture"], ["dda", "Desktop Duplication"]]))}
      ${row(t("Show cursor"), "", toggle("cursor"))}
    </div>
    <div class="section"><h2>${t("Replay buffer")}</h2>
      ${row(t("Keep the buffer in"), bufferHint, select("buffer", [["mixed", t("RAM + disk")], ["ram", t("RAM only")], ["disk", t("Disk only")]]))}
      ${plan.mode === "mixed" ? row(t("Kept in RAM"), t("Clips up to this length are saved straight from RAM"), select("buffer_ram_secs", [...new Set([15, 30, 60, 120, plan.window])].sort((a, b) => a - b).map((n) => [n, t("Last {length}", { length: fmtLen(n) })]), true)) : ""}
      ${plan.mode === "ram" ? "" : row(t("Buffer folder"), `<span class="path" title="${esc(bufferDir)}">${esc(bufferDir)}</span><br>${t("Temporary files, deleted as the buffer moves on. An SSD is best")}`, `<button class="btn sm" data-act="pick-buffer-dir">${t("Change")}</button>${s.buffer_dir ? `<button class="btn sm" data-act="reset-buffer-dir">${t("Default")}</button>` : ""}`)}
    </div>
    <div class="section"><h2>${t("Audio")}</h2>
      ${row(t("Desktop audio"), t("Everything you hear, from the default output device"), toggle("desktop_audio"))}
      ${row(t("Microphone"), t("Default input device, saved as its own track"), toggle("mic"))}
      ${row(t("Microphone noise suppression"), t("Cuts background noise such as fans and hum from your voice (RNNoise). Applied when a clip is saved, so it costs nothing while clipping"), toggle("mic_denoise"))}
      ${row(t("Separate game audio track"), t("Adds a track with only the game's sound, so you can keep the game and drop everything else (voice chat, music) when editing. Needs Windows 10 version 2004 or newer.") + (s.game_audio && st.game_audio === "unavailable" ? `<br><b>${t("Not available on this version of Windows.")}</b>` : s.game_audio && st.game_audio === "failed" ? `<br><b>${t("The current game's sound could not be captured.")}</b>` : ""), toggle("game_audio"))}
    </div>
    <div class="section"><h2>${t("Behaviour")}</h2>
      ${row(t("Start clipping when Glint starts"), t("Otherwise the buffer stays off until you resume"), toggle("record_on_start"))}
    </div>`;
  markRecommended(["encoder", "method", "aspect", "bitrate", "height"]);
}

function renderSettings() {
  const s = S.settings;
  const slider = (path, min, max, step, unit) => `<input class="range" type="range" min="${min}" max="${max}" step="${step}" data-set="${path}" data-num value="${getPath(s, path)}"><span class="range-val">${getPath(s, path)}${unit}</span>`;
  const dc = S.status.discord || "off";
  const discordHint = !s.discord_rpc
    ? t("Shows “Clipping Overwatch” on your Discord profile while the buffer is on")
    : dc === "connected" ? `<span style="color:var(--ok)">${t("Connected.")}</span> ${t("Your profile shows what you are clipping")}`
    : `<span style="color:var(--warn)">${esc(dc === "off" ? t("Connecting…") : dc)}</span>`;
  const searchBox = `<label class="search set-search"><svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="7"/><path d="m20 20-3.500-3.500"/></svg><input id="set-q" type="text" value="${esc(S.setQ || "")}" placeholder="${esc(t("Search settings"))}" aria-label="${esc(t("Search settings"))}" spellcheck="false" autocomplete="off"></label>`;
  const langSelect = `<select class="field" data-lang aria-label="${esc(t("Language"))}">${Object.entries(window.GLINT_LANGS).map(([code, lang]) => `<option value="${code}"${(s.language || "en") === code ? " selected" : ""}>${esc(lang.name)}</option>`).join("")}</select>`;
  $("#view").innerHTML = pageHead(t("Settings"), t("Look, storage, startup and notifications")).replace(/<\/div>$/, () => `<div class="head-actions">${searchBox}</div></div>`) + `
    ${S.ffmpeg ? "" : `<div class="note bad">${t("<b>ffmpeg.exe was not found.</b> Glint needs ffmpeg 8 or newer. Install it or set its path below.")}</div>`}
    <div class="section"><h2>${t("Appearance")}</h2>
      <div class="themes">${window.GLINT_THEMES.map((th) => `<button class="theme${(s.theme || "glint") === th.id ? " active" : ""}" data-theme="${th.id}" style="background:${th.vars.panel || "#0a0e16"};color:${th.vars.text || "#e8edf6"}"><i style="background:${th.vars.accent || "#62b6f7"}"></i>${esc(th.name)}</button>`).join("")}</div>
      ${row(t("Match the taskbar icon to the theme"), t("Colours Glint's icon in the taskbar and the tray with the theme's accent, including a custom theme's"), toggle("icon_theme"))}
      ${row(t("Renders button"), t("The button in the top bar that lists Blur renders and their progress"), select("renders_button", [["auto", t("While rendering")], ["always", t("Always")], ["never", t("Never")]]))}
      ${row(t("Interface size"), t("Makes everything in the window bigger or smaller, so more or less fits on screen"), select("ui_zoom", [70, 80, 90, 100, 110, 125, 150].map((z) => [z, `${z}%`]), true))}
      <details class="css-box"${S.cssOpen || s.custom_css ? " open" : ""}>
        <summary><span class="name">${t("Custom theme (CSS)")}</span><span class="hint">${s.custom_css ? t("In use") : t("Make your own look")}</span></summary>
        <div class="css-help">
          <p>${t("The easiest way to get your own theme is to ask an AI for it. Press Copy AI prompt, paste it into Claude, ChatGPT or any other AI, add a line about the look you want, then paste the CSS it gives you into the box below.")}</p>
          <div class="css-actions">
            <button class="btn sm primary" data-act="css-prompt">${ICON.copy}${t("Copy AI prompt")}</button>
            <button class="btn sm" data-act="css-clear">${t("Clear")}</button>
          </div>
          <p class="hint">${t("Your CSS is applied on top of the theme picked above. Colours are variables, so one line recolours everything:")} <code>:root { --accent: #ff4fd8; }</code></p>
        </div>
        <textarea class="code" data-set="custom_css" spellcheck="false" placeholder=":root { --accent: #ff4fd8; }&#10;.card h3 { font-style: italic; }">${esc(s.custom_css || "")}</textarea>
      </details>
    </div>
    <div class="section"><h2>${t("Language")}</h2>
      ${row(t("Language"), t("Used in Glint's window, the clip overlay and the tray menu. The tray menu changes the next time Glint starts"), langSelect)}
    </div>
    <div class="section"><h2>${t("Storage")}</h2>
      ${row(t("Clips folder"), `<span class="path" title="${esc(S.settings.clips_dir)}">${esc(S.settings.clips_dir)}</span>`, `<button class="btn sm" data-act="pick-dir">${t("Change")}</button><button class="btn sm" data-act="open-dir">${t("Open")}</button>`)}
      ${row(t("Game folders"), t("Save each new clip into a subfolder named after its game, such as Overwatch or Risk of Rain 2"), toggle("game_folders"))}
      ${row(t("Show the main folder when it is empty"), t("Otherwise its chip above the library is hidden while no clip sits loose in the clips folder"), toggle("show_empty_main"))}
      ${row(t("Sort existing clips"), t("Move the clips that sit loose in the clips folder into their game's folder. Clips you already filed stay put"), `<button class="btn sm" data-act="sort-games">${t("Sort by game")}</button>`)}
    </div>
    <div class="section"><h2>${t("Startup")}</h2>
      ${row(t("Start with Windows"), t("Launches hidden in the tray with the buffer running"), toggle("autostart"))}
      ${row(t("Keep running in the tray"), t("Closing the window leaves the recorder on. The window uses no memory while closed"), toggle("close_to_tray"))}
    </div>
    <div class="section"><h2>${t("Clip sound")}</h2>
      ${row(t("Clip sound"), t("Plays the moment a clip key is pressed"), toggle("sound"))}
      ${row(t("Sound"), t("Picking one plays it"), `${select("sound_name", [...S.sounds.map((x) => [x.id, x.name]), ["custom", t("Custom file…")]])}<button class="btn sm" data-act="sound-play">${t("Play")}</button>`)}
      ${S.settings.sound_name !== "custom" ? row(t("Volume"), "", `<input class="range" type="range" min="5" max="100" step="5" data-set="sound_volume" data-num value="${S.settings.sound_volume}"><span class="range-val">${S.settings.sound_volume}%</span>`)
        : row(t("Sound file"), S.settings.sound_file ? `<span class="path" title="${esc(S.settings.sound_file)}">${esc(S.settings.sound_file)}</span>` : t("A .wav file. It plays at its own loudness"), `<button class="btn sm" data-act="sound-pick">${t("Choose file")}</button>`)}
    </div>
    <div class="section"><h2>${t("Interface sounds")}</h2>
      ${row(t("Interface sounds"), t("Soft sounds when you click, switch, open and close things in this window"), toggle("ui_sounds"))}
      ${row(t("Sounds when the pointer moves over things"), t("A very quiet tick over the sidebar, clips, chips and top bar buttons"), toggle("ui_sounds_hover"))}
      ${row(t("Volume"), "", `<input class="range" type="range" min="0" max="100" step="5" data-set="ui_sounds_volume" data-num value="${s.ui_sounds_volume ?? 50}"><span class="range-val">${s.ui_sounds_volume ?? 50}%</span>`)}
      ${row(t("Hear them"), t("Each button plays its sound, even while interface sounds are off"), `<div class="sfx-try">${sfxTryButtons()}</div>`)}
    </div>
    <div class="section"><h2>${t("Clip overlay")}</h2>
      ${row(t("Clip overlay"), t("A small box on the captured monitor when a clip is saved. It never takes focus or clicks"), toggle("toast"))}
      ${row(t("Compact"), t("Shows only the two stars in a small square, with no text. Problems, such as a clip that failed, still show the full box"), toggle("toast_compact"))}
      ${row(t("Position"), t("The corner it appears in"), `<div class="corner-pick" role="group" aria-label="${esc(t("Overlay position"))}">${[["tl", t("Top left")], ["tr", t("Top right")], ["bl", t("Bottom left")], ["br", t("Bottom right")]].map(([v, l]) => `<button class="${S.settings.toast_pos === v ? "active" : ""}" data-corner="${v}" title="${esc(l)}" aria-label="${esc(l)}"><i></i></button>`).join("")}</div><button class="btn sm" data-act="toast-show">${t("Show it")}</button>`)}
      ${row(t("Opacity"), t("Lower lets the game show through"), slider("toast_opacity", 20, 100, 5, "%"))}
      ${row(t("Size"), "", slider("toast_scale", 70, 160, 5, "%"))}
      ${row(t("Time on screen"), "", slider("toast_secs", 1.5, 10, 0.5, " " + t("s")))}
      ${row(t("Show clip length"), t("“Captured last 30s”"), toggle("toast_length"))}
      ${row(t("Show game"), t("“Captured last 30s of Overwatch”"), toggle("toast_game"))}
      ${row(t("Show microphone icon"), t("Appears while your microphone is being recorded"), toggle("toast_mic"))}
    </div>
    <div class="section"><h2>Discord</h2>
      ${row(t("Rich Presence"), discordHint, toggle("discord_rpc"))}
      ${row(t("Application ID"), t("Discord only shows a presence from an app registered with it. Create one named Glint at discord.com/developers/applications (free, about a minute) and paste its Application ID here"), `<input class="field" type="text" inputmode="numeric" data-set="discord_app_id" value="${esc(s.discord_app_id || "")}" placeholder="${esc(t("18 or 19 digits"))}" spellcheck="false" style="width:210px">`)}
    </div>
    <div class="section"><h2>${t("Updates")}</h2>
      ${row(t("Version"), "", `<span class="path">Glint ${esc(S.version || "")}</span><button class="btn sm" data-act="whats-new" data-sfx="open">${t("What's new")}</button>`)}
      ${row(t("Check for updates daily"), t("Asks GitHub once a day whether a newer Glint has been released. Nothing about you or this PC is sent"), toggle("update_check"))}
    </div>
    <div class="section"><h2>${t("Program")}</h2>
      ${row(t("ffmpeg.exe path"), S.ffmpeg ? t("Using {path}", { path: `<span class="path" title="${esc(S.ffmpeg)}">${esc(S.ffmpeg)}</span>` }) : t("Not found"), `<input class="field" type="text" data-set="ffmpeg_path" value="${esc(S.settings.ffmpeg_path)}" placeholder="${esc(t("Auto"))}" style="width:340px">`)}
    </div>
    ${typeof depsSection === "function" ? depsSection() : ""}`;
  // The Capture page's settings can be found from here too: its sections are
  // added to this page, hidden until a search asks for them.
  const own = $("#view").innerHTML;
  renderRecording();
  const capture = $$("#view .section").map((sec) => {
    sec.dataset.searchOnly = "1";
    const h = sec.querySelector("h2");
    if (h) h.textContent = `${t("Capture")} · ${h.textContent}`;
    return sec.outerHTML;
  }).join("");
  $("#view").innerHTML = own + capture + `<div class="note" id="set-none" hidden></div>`;
  markRecommended(["encoder", "method", "aspect", "bitrate", "height"]);
  applySettingsFilter();
}

/** The "Hear them" buttons of Settings → Interface sounds: one per sound in sfx.js. */
function sfxTryButtons() {
  const names = { click: t("Click"), tap: t("Tap"), hover: t("Hover"), on: t("On"), off: t("Off"), open: t("Open"), close: t("Close"), star: t("Star"), launch: t("Launch"), success: t("Success"), error: t("Error") };
  const list = typeof SFX_NAMES !== "undefined" ? SFX_NAMES : Object.keys(names);
  // data-sfx="none": the button's own sound is the one it plays, not a click on top.
  return list.map((n) => `<button class="btn sm" data-act="sfx-try" data-name="${esc(n)}" data-sfx="none">${esc(names[n] || n)}</button>`).join("");
}

/** Show only the settings that match the search box on the Settings page. */
function applySettingsFilter() {
  const q = (S.setQ || "").trim().toLowerCase();
  let shown = 0;
  for (const sec of $$("#view .section")) {
    // A match in the section's title shows all of it; otherwise only the matching settings.
    const whole = q && (sec.querySelector("h2")?.textContent || "").toLowerCase().includes(q);
    let any = false;
    for (const el of sec.children) {
      if (el.tagName === "H2") continue;
      const hit = !q || whole || el.textContent.toLowerCase().includes(q);
      el.hidden = !hit;
      any = any || hit;
    }
    sec.hidden = !any || (!q && "searchOnly" in sec.dataset);
    if (!sec.hidden) shown++;
  }
  const none = $("#set-none");
  if (none) {
    none.hidden = shown > 0;
    none.textContent = t("No setting matches “{text}”", { text: (S.setQ || "").trim() });
  }
}

function render() {
  renderChrome();
  if (!S.settings) return;
  if (S.renaming) { S.renderLater = true; return; } // a clip name is being edited in place: redraw once it is done
  const keep = $("#view").scrollTop;
  ({ library: renderLibrary, favorites: renderLibrary, rendered: renderLibrary, keybinds: renderKeybinds, recording: renderRecording, blur: renderBlur, settings: renderSettings })[S.page]();
  $("#view").scrollTop = keep;
  renderSelBar();
}

/* ---------- events ---------- */
function endCapture() {
  if (S.capturing === null) return;
  S.capturing = null;
  invoke("suspend_hotkeys", { on: false });
}
// The backend's hotkey pause expires after 12 s; renew it while still listening for a key.
setInterval(() => { if (S.capturing !== null) invoke("suspend_hotkeys", { on: true }); }, 5000);

document.addEventListener("click", async (e) => {
  if (suppressClick) { suppressClick = false; return; }
  if (e.target.closest(".title-edit")) return; // clicks in a clip's name box stay there
  const el = e.target.closest("[data-page],[data-tab],[data-size],[data-clip],[data-act],[data-v],[data-win],[data-folder],[data-m],[data-corner],[data-theme]");
  if (e.target.id === "modal") return closeClip();
  if (!$("#menu").hidden && el?.dataset.act !== "sel-move" && !(e.target.closest(".has-sub") && !el?.dataset.m)) setTimeout(closeMenu, 0);
  if (!el) return;
  const d = el.dataset;
  try {
    // Closing the window leaves an "Edit together" draft first (join.js: it may ask, then discards it).
    if (d.win === "close" && typeof joinWindowClose === "function") return joinWindowClose(() => invoke("win_action", { action: d.win }));
    if (d.win) return invoke("win_action", { action: d.win });
    if (d.m) return await menuAction(d.m, d.arg || "");
    if (d.page) { endCapture(); S.page = d.page; S.tab = "all"; S.folder = null; S.sel.clear(); $("#view").scrollTop = 0; return render(); }
    if (d.tab) { S.tab = d.tab; S.sel.clear(); return render(); }
    if (d.size) { S.size = d.size; try { localStorage.setItem("glint.size", d.size); } catch {} return render(); }
    if ("folder" in d) { endCapture(); S.page = "library"; S.folder = d.folder === "*" ? null : d.folder; S.tab = "all"; S.sel.clear(); $("#view").scrollTop = 0; return render(); }
    if (d.corner) { S.settings.toast_pos = d.corner; saveSettings(true); setTimeout(() => invoke("preview_toast"), 350); return; }
    if (d.theme) { S.settings.theme = d.theme; window.applyGlintTheme(d.theme, S.settings.custom_css); return saveSettings(true); }
    if (d.clip) {
      // The name is its own hit area (a double click renames): a plain click on it does nothing,
      // Ctrl / Shift still select.
      if (e.target.closest(".ctitle")) return e.ctrlKey || e.shiftKey ? clickSelect(d.clip, e) : undefined;
      // With a selection going, or Ctrl / Shift / the tick box, a click selects. Otherwise it opens the clip.
      if (S.sel.size || e.ctrlKey || e.shiftKey || e.target.closest(".check")) return clickSelect(d.clip, e);
      return openClip(d.clip);
    }
    if (d.v) return viewerAction(d.v);
    switch (d.act) {
      case "open-dir": return await invoke("open_clips_dir");
      case "rescan": await invoke("rescan_library"); await refreshClips(); return flash(t("Library rescanned"));
      case "sel-clear": return clearSel();
      case "sel-del": return await bulkDelete();
      case "sel-move": return openMoveMenu(el);
      case "sort-games": {
        const loose = S.clips.filter((c) => !c.folder).length;
        if (!loose) return flash(t("No loose clips to sort"));
        if (!(await ask(t("Move {clips} from the clips folder into game folders?", { clips: plural(loose) }), t("Sort by game")))) return;
        try { const n = await invoke("sort_by_game"); flash(t("Moved {clips} into game folders", { clips: plural(n ?? loose) })); } finally { await refreshClips(); }
        return;
      }
      case "games-more": S.gamesOpen = !S.gamesOpen; return render();
      case "side-toggle": S.sideMin = !S.sideMin; setUiPref("glint.sideMin", S.sideMin); return renderChrome();
      case "folders-toggle":
        // From the icon strip, the row opens the sidebar with the folders showing.
        if (S.sideMin) { S.sideMin = false; S.foldersOpen = true; setUiPref("glint.sideMin", false); } else S.foldersOpen = !S.foldersOpen;
        setUiPref("glint.foldersOpen", S.foldersOpen);
        return renderChrome();
      case "css-clear": S.settings.custom_css = ""; S.cssOpen = true; window.applyGlintTheme(S.settings.theme, ""); return saveSettings(true);
      case "css-prompt": await copyText(THEME_PROMPT); return flash(t("Prompt copied. Paste it into an AI and say what look you want."));
      case "rec-use": {
        // "Use recommended" beside a setting: data-rec-path / data-rec-value name it.
        setPath(S.settings, d.recPath, "num" in d ? +d.recValue : d.recValue);
        return saveSettings(true);
      }
      case "import": return typeof openImport === "function" ? openImport() : flash(t("Import is not available"), true);
      case "sound-play": return await invoke("preview_sound");
      case "sound-pick": { const file = await invoke("pick_sound_file"); if (file) { S.settings.sound_file = file; saveSettings(true); setTimeout(() => invoke("preview_sound"), 350); } return; }
      case "toast-show": return await invoke("preview_toast");
      case "sfx-try": return typeof sfxPlay === "function" && sfxPlay(d.name, true); // heard even while sounds are off
      case "whats-new": return typeof wnOpen === "function" && wnOpen();
      case "sel-copy": {
        const n = await invoke("copy_clips", { ids: [...S.sel] });
        return flash(t("{clips} copied. Paste them anywhere.", { clips: plural(n ?? S.sel.size) }));
      }
      case "sel-fav": {
        const sel = S.clips.filter((c) => S.sel.has(c.id));
        const favorite = !sel.every((c) => c.favorite);
        await invoke("set_favorites", { ids: sel.map((c) => c.id), favorite });
        for (const c of sel) c.favorite = favorite;
        sfx(favorite ? "star" : "off");
        return render();
      }
      case "folder-new": { const name = await newFolder(); if (name) { S.page = "library"; S.folder = name; S.tab = "all"; render(); } return; }
      case "folder-rename": return await renameFolder(S.folder);
      case "folder-del": return await deleteFolder(S.folder);
      case "pick-dir": {
        const dir = await invoke("pick_folder");
        if (dir) { S.settings.clips_dir = dir; saveSettings(true); }
        return;
      }
      case "pick-buffer-dir": {
        const dir = await invoke("pick_folder");
        if (dir) { S.settings.buffer_dir = dir; saveSettings(true); }
        return;
      }
      case "reset-buffer-dir": S.settings.buffer_dir = ""; return saveSettings(true);
      case "kb-add":
        S.settings.keybinds.push({ id: "kb-" + Date.now().toString(36), ctrl: false, shift: false, alt: false, win: false, key: "", action: "clip", seconds: 60, monitor: -1 });
        return saveSettings(true);
      case "kb-del":
        endCapture();
        S.settings.keybinds.splice(+d.kb, 1);
        return saveSettings(true);
      case "capture":
        if (S.capturing === +d.kb) { endCapture(); return render(); }
        S.capturing = +d.kb;
        await invoke("suspend_hotkeys", { on: true });
        return render();
    }
  } catch (err) { fail(err); }
});

// Remember whether the custom theme box is unfolded: the page is re-rendered often. ("toggle" does not bubble, hence capture.)
document.addEventListener("toggle", (e) => { if (e.target.classList?.contains("css-box")) S.cssOpen = e.target.open; }, true);

// The Settings page's own search box. Filtering hides rows in place, so the box keeps its focus.
document.addEventListener("input", (e) => {
  if (e.target.id !== "set-q") return;
  S.setQ = e.target.value;
  applySettingsFilter();
});

document.addEventListener("change", (e) => {
  const el = e.target;
  if (el.id === "sort") { S.sort = el.value; return render(); }
  if ("lang" in el.dataset) { S.settings.language = glintSetLanguage(el.value); return saveSettings(true); }
  if (el.dataset.kbf) {
    const k = S.settings.keybinds[+el.dataset.kb];
    k[el.dataset.kbf] = el.dataset.kbf === "seconds" || el.dataset.kbf === "monitor" ? +el.value : el.value; // monitor: -1 or an S.monitors index
    return saveSettings(true);
  }
  if (el.dataset.set) {
    let v = el.type === "checkbox" ? el.checked : "num" in el.dataset ? +el.value : el.value;
    if ("num" in el.dataset && !Number.isFinite(v)) return;
    if (el.dataset.set === "encoder" || el.dataset.set === "blur.gpu_type") {
      // The setting is only written once the user has confirmed; a "no" puts the control back.
      confirmHardwareChoice(el.dataset.set, v).then((yes) => {
        if (!yes) return render();
        setPath(S.settings, el.dataset.set, v);
        saveSettings(true);
      });
      return;
    }
    setPath(S.settings, el.dataset.set, v);
    saveSettings(el.tagName === "SELECT" || el.type === "checkbox");
    if (el.dataset.set.endsWith("_path")) setTimeout(refreshState, 500);
    if (el.dataset.set === "bitrate" || el.dataset.set === "buffer" || el.dataset.set === "buffer_ram_secs") render();
    if (el.type === "range") render(); // refresh the number shown beside the slider (and apply a sound setting)
    if (el.dataset.set === "ui_sounds_volume" && typeof sfxPlay === "function") sfxPlay("click", true); // the new loudness
    // Let the user hear a sound choice, or see an overlay change, once the setting has been saved.
    if (el.dataset.set === "sound_name" || el.dataset.set === "sound_volume") { if (S.settings.sound_name !== "custom" || S.settings.sound_file) setTimeout(() => invoke("preview_sound"), 350); }
    if (/^toast_(opacity|scale|secs|compact|length|game|mic)$/.test(el.dataset.set)) setTimeout(() => invoke("preview_toast"), 350);
    if (el.dataset.set === "custom_css") window.applyGlintTheme(S.settings.theme, S.settings.custom_css);
  }
});

window.addEventListener("keydown", (e) => {
  if (askKey) { e.stopPropagation(); if (askKey(e) !== "pass") e.preventDefault(); return; }
  if (S.capturing !== null) {
    e.preventDefault();
    e.stopPropagation();
    if (e.code === "Escape") { endCapture(); return render(); }
    if (/^(Control|Shift|Alt|Meta|OS)/.test(e.code)) return;
    const k = S.settings.keybinds[S.capturing];
    Object.assign(k, { ctrl: e.ctrlKey, shift: e.shiftKey, alt: e.altKey, win: e.metaKey, key: e.code });
    endCapture();
    return saveSettings(true);
  }
  // A clip's name being edited in place, or the bug report dialog being typed in: every key is
  // the text box's (its own keydown handles Enter and Esc), so no shortcut or keybind fires.
  if (e.target.closest?.(".title-edit, .bug-dlg")) return;
  // Windows does not pass keys to Glint's global hook while this window has
  // focus, so the window matches its own keybinds (same rule: most modifiers wins).
  const typing = e.target.matches?.("input, textarea") && !e.ctrlKey && !e.altKey && !e.metaKey && !/^F\d+$/.test(e.code);
  if (!e.repeat && !typing && S.settings) {
    let best = null, bestN = -1;
    for (const k of S.settings.keybinds) {
      if (!k.key || k.key !== e.code) continue;
      if ((k.ctrl && !e.ctrlKey) || (k.shift && !e.shiftKey) || (k.alt && !e.altKey) || (k.win && !e.metaKey)) continue;
      const n = k.ctrl + k.shift + k.alt + k.win;
      if (n > bestN) { best = k; bestN = n; }
    }
    if (best) { e.preventDefault(); invoke("fire_keybind", { id: best.id }).catch(() => {}); return; }
  }
  // The editor timeline's right-click menu, the bug report dialog or What's new is open: the keyboard
  // is its own (arrows, Enter, Esc closes it). This listener runs before theirs, so it has to stand aside.
  if (document.querySelector(".tl-menu, .bug-dlg, .wn-dlg")) return;
  const inField = e.target.matches?.("input, select, textarea");
  if (e.ctrlKey && e.code === "KeyK") { e.preventDefault(); $("#search").classList.add("open"); $("#q").focus(); $("#q").select(); }
  else if (isLib() && !S.open && !inField && e.ctrlKey && e.code === "KeyA") { e.preventDefault(); S.sel = new Set([...S.sel, ...visibleClips().list.map((c) => c.id)]); syncSel(); } // what was picked keeps its order, the rest follow as on screen
  else if (isLib() && !S.open && !inField && e.code === "Delete" && S.sel.size) { e.preventDefault(); bulkDelete(); }
  else if (e.code === "Escape" && !S.open && (S.sel.size || !$("#menu").hidden)) { if (!$("#menu").hidden) closeMenu(); else clearSel(); }
  else if (e.code === "Escape" && S.open) { if (!document.fullscreenElement) closeClip(); }
  else if (S.open && S.player && !e.ctrlKey && !e.altKey && !inField && S.player.key(e)) e.preventDefault();
}, true);

// The search box is an icon until it is used: open while it has the focus or holds text.
const syncSearch = () => $("#search").classList.toggle("open", document.activeElement === $("#q") || !!$("#q").value);
$("#q").addEventListener("focus", syncSearch);
$("#q").addEventListener("blur", syncSearch);
// Opened on the press itself, so the field is already wide when the click puts the caret in it.
$("#search").addEventListener("mousedown", () => $("#search").classList.add("open"));
$("#q").addEventListener("input", (e) => {
  syncSearch();
  S.q = e.target.value;
  S.sel.clear();
  if (!["library", "favorites", "rendered"].includes(S.page)) S.page = "library";
  render();
});
$("#btn-toggle").onclick = async () => { try { S.status = await invoke("toggle_recording"); render(); } catch (e) { fail(e); } };
$("#btn-clip").onclick = async () => {
  try { await invoke("clip_now", { seconds: quickSecs() }); } catch (e) { fail(e); }
};

/* ---------- data ---------- */
async function refreshClips() {
  S.clips = ((await invoke("list_clips")) || []).filter((c) => !c.draft); // drafts ("Edit together") are held by join.js only
  S.folders = (await invoke("list_folders")) || [];
  S.games = (await invoke("list_games")) || [];
  const ids = new Set(S.clips.map((c) => c.id));
  for (const id of [...S.sel]) if (!ids.has(id)) S.sel.delete(id);
  render();
}
async function refreshState() {
  const st = await invoke("get_state");
  Object.assign(S, { settings: st.settings, monitors: st.monitors, status: st.status, ffmpeg: st.ffmpeg, blur: st.blur, hardware: st.hardware || null });
  if (!S.sounds.length) S.sounds = (await invoke("list_sounds")) || [];
  window.applyGlintTheme(S.settings.theme, S.settings.custom_css);
  glintSetLanguage(S.settings.language || "en");
  S.version = st.version;
  $("#ver").title = "Glint v" + st.version; // the logo in the top bar
  render();
}
async function pollStatus() {
  if (document.hidden || !S.settings) return;
  try {
    // What the settings pages show from the status: re-render them when it changes, unless the user is in a field.
    const key = () => `${S.status.error}|${S.status.max_mbps}|${S.status.recording}|${S.status.discord}|${S.status.width}x${S.status.height}`;
    const before = key();
    S.status = await invoke("get_status");
    renderChrome();
    if (before !== key() && ["recording", "keybinds", "settings"].includes(S.page) && S.capturing === null && !document.activeElement?.matches?.("#view input, #view select, #view textarea")) render();
  } catch {}
}

(async function init() {
  try { S.size = localStorage.getItem("glint.size") || "m"; } catch {}
  await refreshState();
  await refreshClips();
  if (typeof jobsInit === "function") jobsInit();
  if (typeof wnStartup === "function") wnStartup(); // What's new, once after an update
  setInterval(pollStatus, 1000);
  // The payload is the id of a clip that was added or changed ("" for many). A new plain clip is one
  // just saved from the buffer: its sound is "success" (renders and compressions have their own).
  T.event.listen("clips-changed", async ({ payload: id } = {}) => {
    const fresh = !!id && !S.clips.some((c) => c.id === id);
    try { await refreshClips(); } catch (e) { return console.error("Glint: list_clips failed", e); }
    if (fresh && S.clips.find((c) => c.id === id)?.kind === "clip") sfx("success");
  });
  // A "Compress for Discord" job finished: { clip, mb, size, copied, secs }. The clip is new (kind "discord").
  T.event.listen("clip-compressed", ({ payload: p }) => {
    if (!p || !p.clip) return;
    const i = S.clips.findIndex((c) => c.id === p.clip.id);
    if (i >= 0) S.clips[i] = p.clip; else S.clips.push(p.clip);
    render();
    sfx("success");
    const size = fmtSize(p.size || p.clip.size || 0);
    flash(p.copied ? t("Compressed to {size} and copied. Paste it into Discord.", { size }) : t("Compressed to {size}. The new clip is in your library.", { size }));
  });
  T.event.listen("status-changed", pollStatus);
  // A keybind changed a setting behind the window's back (it switched the capture monitor): read them again.
  T.event.listen("settings-changed", () => refreshState().catch((e) => console.warn("Glint: could not refresh the settings", e)));
  // A game was seen for the first time, or its icon was just taken from its exe.
  T.event.listen("games-changed", async () => { S.games = (await invoke("list_games")) || []; if (isLib() && !S.open && !S.sel.size) render(); });
  T.event.listen("blur-progress", ({ payload: p }) => {
    S.jobs[p.id] = { pct: p.pct, state: p.state, message: p.message };
    if (p.state === "done") { delete S.jobs[p.id]; sfx("success"); flash(t("Blur render finished")); }
    if (p.state === "cancelled") delete S.jobs[p.id];
    if (p.state === "error") flash(p.message, true);
    const c = S.clips.find((x) => x.id === p.id);
    if (S.open === p.id && c) $("#v-blur").innerHTML = blurPanel(c);
    else if (["library", "favorites", "rendered"].includes(S.page) && !S.open) render();
  });
})().catch(fail);
