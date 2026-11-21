"use strict";
/*
 * The library in a window of its own: opened from the editor's top bar (editor.js, the "library-window"
 * button; app.rs open_library_window), so the clips can stand beside the editor, or on another monitor.
 *
 * Clips leave this window as FILES: a drag here starts a real Windows file drag (tauri-plugin-drag), the same
 * kind of drag as one out of Explorer. So a drop on the editor is handled by what already handles dropped files
 * (editor.js edDropInit / edHandleDrop): on the timeline the clips go where they were dropped, on the video they
 * go on the picture, and a clip that is in the library already is used as it is (never imported twice). The
 * drag also works into other apps (a chat, a folder), which get the clip's file.
 *
 * Without a drag: "Add to the editor" (or a double click) sends the selection to the main window ("libwin-add",
 * projects.js), which adds it at the end of what is open in the editor and answers with "libwin-note".
 */
const LT = window.__TAURI__;
const lwInvoke = (cmd, args) => LT.core.invoke(cmd, args);
const lwSrc = (p) => LT.core.convertFileSrc(p);
const lw$ = (s) => document.querySelector(s);
const lwEsc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const lwDur = (s) => { s = Math.max(0, Math.round(s)); return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`; };
const lwHue = (s) => { let h = 0; for (const c of String(s)) h = (h * 31 + c.charCodeAt(0)) % 360; return h; };
const lwPlural = (n) => (n === 1 ? t("1 clip") : t("{n} clips", { n }));
function lwAgo(ms) {
  const s = (Date.now() - ms) / 1000;
  if (s < 60) return t("Just now");
  if (s < 3600) return t("{n} min ago", { n: Math.floor(s / 60) });
  if (s < 86400) { const n = Math.floor(s / 3600); return n === 1 ? t("1 hour ago") : t("{n} hours ago", { n }); }
  if (s < 86400 * 30) { const n = Math.floor(s / 86400); return n === 1 ? t("1 day ago") : t("{n} days ago", { n }); }
  return new Date(ms).toLocaleDateString();
}

const LW = { clips: [], q: "", game: "all", sel: [], anchor: null, press: null, dragging: false, html: "" };

/* ---------- pure parts (tested under Node) ---------- */

/** The clips shown: ones with a file of their own (not drafts, not projects), matching the search and the game, newest first. */
function lwFilter(clips, q, game) {
  q = String(q || "").trim().toLowerCase();
  return (Array.isArray(clips) ? clips : []).filter((c) => c && !c.draft && typeof c.path === "string" && c.path
    && (game === "all" || !game || c.game === game)
    && (!q || String(c.title).toLowerCase().includes(q) || String(c.game).toLowerCase().includes(q)))
    .sort((a, b) => (b.created || 0) - (a.created || 0));
}
/** The games of those clips, most clips first. */
function lwGames(clips) {
  const n = {};
  for (const c of lwFilter(clips, "", "all")) n[c.game] = (n[c.game] || 0) + 1;
  return Object.keys(n).sort((a, b) => n[b] - n[a] || a.localeCompare(b));
}
/** A click on clip `id` of the shown `ids`: plain = only it, Ctrl = toggle it, Shift = the range from the anchor. */
function lwClick(sel, anchor, ids, id, mods) {
  const from = ids.indexOf(anchor), to = ids.indexOf(id);
  if (mods?.shift && from >= 0 && to >= 0) {
    const range = ids.slice(Math.min(from, to), Math.max(from, to) + 1);
    return { sel: [...(mods.ctrl ? sel.filter((x) => !range.includes(x)) : []), ...range], anchor };
  }
  if (mods?.ctrl) return { sel: sel.includes(id) ? sel.filter((x) => x !== id) : [...sel, id], anchor: id };
  return { sel: [id], anchor: id };
}
/** What a drag that starts on clip `id` carries: the selection if the clip is in it, else that clip alone. As shown, top to bottom. */
function lwDragIds(sel, ids, id) {
  const set = sel.includes(id) ? sel : [id];
  return ids.filter((x) => set.includes(x));
}

/* ---------- the page ---------- */

const lwShown = () => lwFilter(LW.clips, LW.q, LW.game);
const lwById = (id) => LW.clips.find((c) => c.id === id) || null;

let lwFlashTimer;
function lwFlash(msg, bad) {
  const el = lw$("#flash");
  el.textContent = msg;
  el.className = "flash" + (bad ? " bad" : "");
  el.hidden = false;
  clearTimeout(lwFlashTimer);
  lwFlashTimer = setTimeout(() => (el.hidden = true), 3200);
}

function lwCardHtml(c) {
  const on = LW.sel.includes(c.id);
  const bg = c.thumb ? "" : ` style="background-image:linear-gradient(135deg,hsl(${lwHue(c.game)} 45% 22%),hsl(${(lwHue(c.game) + 50) % 360} 50% 12%))"`;
  return `<div class="card lw-card${on ? " sel" : ""}" data-clip="${lwEsc(c.id)}" title="${lwEsc(c.title)}">
    <div class="thumb"${bg}>${c.thumb ? `<img class="thumb-img lw-img" src="${lwEsc(lwSrc(c.thumb))}" alt="" draggable="false" decoding="async" loading="lazy" onerror="this.remove()">` : `<div class="ph">${lwEsc(String(c.game || "?").slice(0, 1).toUpperCase())}</div>`}<span class="dur">${lwDur(c.duration)}</span></div>
    <h3>${lwEsc(c.title)}</h3><p><span>${lwEsc(c.game)}</span><span>${lwEsc(lwAgo(c.created))}</span></p>
  </div>`;
}

function lwRender() {
  const view = lw$("#lw-view"), shown = lwShown();
  const all = new Set(LW.clips.map((c) => c.id));
  LW.sel = LW.sel.filter((id) => all.has(id));
  // The game filter: rebuilt only when the games change (an open list would close).
  const sel = lw$("#lw-game"), games = lwGames(LW.clips);
  if (LW.game !== "all" && !games.includes(LW.game)) LW.game = "all";
  const opts = `<option value="all">${lwEsc(t("All games"))}</option>` + games.map((g) => `<option value="${lwEsc(g)}">${lwEsc(g)}</option>`).join("");
  if (sel.__opts !== opts) { sel.innerHTML = opts; sel.__opts = opts; }
  if (sel.value !== LW.game) sel.value = LW.game;
  const html = shown.length ? `<div class="grid s">${shown.map(lwCardHtml).join("")}</div>`
    : `<div class="empty"><p>${lwEsc(lwFilter(LW.clips, "", "all").length ? t("No clips match the search.") : t("No clips in your library yet."))}</p></div>`;
  if (html !== LW.html) {
    const top = view.scrollTop;
    view.innerHTML = html;
    view.scrollTop = top;
    LW.html = html;
  }
  lwPaintFoot();
}
/** The selection marks and the foot, without building the grid again. */
function lwPaintSel() {
  for (const el of document.querySelectorAll("#lw-view .card")) el.classList.toggle("sel", LW.sel.includes(el.dataset.clip));
  LW.html = ""; // the marks are part of the grid's HTML: the next lwRender compares afresh
  lwPaintFoot();
}
function lwPaintFoot() {
  const btn = lw$("#lw-add"), n = LW.sel.length;
  btn.disabled = !n;
  btn.textContent = n > 1 ? t("Add {clips} to the editor", { clips: lwPlural(n) }) : t("Add to the editor");
  lw$("#lw-hint").textContent = t("Drag clips onto the editor: on the timeline, or on the video to put them on the picture.");
}

async function lwLoad() {
  try {
    const list = await lwInvoke("list_clips");
    LW.clips = Array.isArray(list) ? list : [];
  } catch (e) { console.error("Glint: the library window could not list the clips", e); }
  lwRender();
}

/** The selection goes to the editor without a drag: the main window adds it at the end of what is open there. */
async function lwAdd(ids) {
  ids = (ids || []).filter((id) => lwById(id));
  if (!ids.length) return;
  try { await LT.event.emitTo("main", "libwin-add", { ids }); }
  catch (e) { console.error("Glint: could not reach the main window", e); lwFlash(t("Could not reach the editor."), true); }
}

/* ---------- dragging clips out as files ---------- */

/**
 * The picture that follows the pointer during the drag: the clip's thumbnail (when the page may read it) with its
 * title, and the number of clips when there are several. A PNG data URL, which is what the drag plugin takes.
 * `img`: an ImageBitmap of the thumbnail, or null.
 */
function lwGhost(clip, n, img) {
  const k = Math.max(1, Math.min(3, window.devicePixelRatio || 1)), W = 176, H = 99 + 28;
  const cv = document.createElement("canvas");
  cv.width = Math.round(W * k);
  cv.height = Math.round(H * k);
  const g = cv.getContext("2d"), css = getComputedStyle(document.documentElement), col = (v, d) => css.getPropertyValue(v).trim() || d;
  g.scale(k, k);
  g.fillStyle = col("--raised-2", "#151b27");
  g.beginPath();
  g.roundRect(0, 0, W, H, 7);
  g.fill();
  g.save();
  g.beginPath();
  g.roundRect(4, 4, W - 8, 95, 5);
  g.clip();
  const grad = g.createLinearGradient(0, 0, W, 99), h = lwHue(clip.game);
  grad.addColorStop(0, `hsl(${h} 45% 22%)`);
  grad.addColorStop(1, `hsl(${(h + 50) % 360} 50% 12%)`);
  g.fillStyle = grad;
  g.fillRect(4, 4, W - 8, 95);
  if (img) {
    const s = Math.max((W - 8) / img.width, 95 / img.height), w = img.width * s, hh = img.height * s;
    g.drawImage(img, 4 + (W - 8 - w) / 2, 4 + (95 - hh) / 2, w, hh);
  }
  g.restore();
  g.fillStyle = col("--text", "#e8edf6");
  g.font = "600 12px 'Segoe UI', system-ui, sans-serif";
  g.textBaseline = "middle";
  let title = String(clip.title || "");
  while (title.length > 1 && g.measureText(title).width > W - 16) title = title.slice(0, -2).trimEnd() + "…";
  g.fillText(title, 8, 99 + 13);
  if (n > 1) {
    const label = String(n), bw = Math.max(22, g.measureText(label).width + 12);
    g.fillStyle = col("--accent", "#62b6f7");
    g.beginPath();
    g.roundRect(W - bw - 8, 8, bw, 20, 10);
    g.fill();
    g.fillStyle = col("--accent-ink", "#051322");
    g.font = "700 12px 'Segoe UI', system-ui, sans-serif";
    g.textAlign = "center";
    g.fillText(label, W - 8 - bw / 2, 18.5);
  }
  return cv.toDataURL("image/png");
}
/** The thumbnail of `clip` as an ImageBitmap the page may draw and read back, or null (no thumbnail, or not readable). */
async function lwThumbBitmap(clip) {
  if (!clip?.thumb) return null;
  try {
    const res = await fetch(lwSrc(clip.thumb));
    if (!res.ok) return null;
    return await createImageBitmap(await res.blob());
  } catch { return null; }
}

/** Start the file drag for the press in progress. It ends when the button is let go (or Esc), wherever that is. */
async function lwStartDrag(p) {
  const shown = lwShown().map((c) => c.id);
  const ids = lwDragIds(LW.sel, shown, p.id), clips = ids.map(lwById).filter((c) => c && c.path);
  if (!clips.length) return;
  if (!LW.sel.includes(p.id)) { LW.sel = [p.id]; LW.anchor = p.id; lwPaintSel(); }
  const first = lwById(p.id) || clips[0];
  let image;
  try { image = lwGhost(first, clips.length, p.bitmap || null); }
  catch (e) { console.warn("Glint: no drag picture", e); image = lwGhost(first, clips.length, null); }
  LW.dragging = true;
  document.body.classList.add("lw-dragging");
  try {
    const Channel = LT.core.Channel, onEvent = Channel ? new Channel() : null;
    await lwInvoke("plugin:drag|start_drag", { item: clips.map((c) => c.path), image, options: { mode: "copy" }, onEvent });
  } catch (e) {
    console.error("Glint: the drag could not start", e);
    lwFlash(t("Could not start the drag. Use \"Add to the editor\" instead."), true);
  } finally {
    LW.dragging = false;
    document.body.classList.remove("lw-dragging");
  }
}

function lwInit() {
  const view = lw$("#lw-view"), q = lw$("#lw-q");
  lw$("#lw-name").textContent = t("Library");
  q.placeholder = t("Search clips and games");
  for (const [k, label] of [["min", "Minimize"], ["max", "Maximize"], ["close", "Close"]]) lw$(`[data-win="${k}"]`).setAttribute("aria-label", t(label));
  document.title = `Glint · ${t("Library")}`;

  q.addEventListener("input", () => { LW.q = q.value; lwRender(); });
  lw$("#lw-game").addEventListener("change", (e) => { LW.game = e.target.value; lwRender(); });
  lw$(".winctl").addEventListener("click", (e) => { const b = e.target.closest("[data-win]"); if (b) lwInvoke("win_action", { action: b.dataset.win }); });
  lw$("#lw-add").addEventListener("click", () => lwAdd(lwShown().map((c) => c.id).filter((id) => LW.sel.includes(id))));

  // A press on a clip: a click selects (on release), a drag past 6 px carries it (or the selection) out as files.
  view.addEventListener("mousedown", (e) => {
    LW.press = null;
    if (e.button !== 0 || LW.dragging) return;
    if (e.clientX - view.getBoundingClientRect().left >= view.clientWidth) return; // the scroll bar
    const id = e.target.closest(".card")?.dataset.clip || null;
    const p = (LW.press = { id, x: e.clientX, y: e.clientY, ctrl: e.ctrlKey || e.metaKey, shift: e.shiftKey, bitmap: null });
    // The thumbnail for the drag picture is fetched while the pointer travels its first pixels.
    if (id) lwThumbBitmap(lwById(id)).then((b) => { if (LW.press === p) p.bitmap = b; });
  });
  window.addEventListener("mousemove", (e) => {
    const p = LW.press;
    if (!p || !p.id) return;
    if (e.buttons === 0) { LW.press = null; return; }
    if (Math.hypot(e.clientX - p.x, e.clientY - p.y) < 6) return;
    LW.press = null;
    lwStartDrag(p);
  });
  window.addEventListener("mouseup", () => {
    const p = LW.press;
    LW.press = null;
    if (!p) return;
    if (!p.id) { if (!p.ctrl && !p.shift) { LW.sel = []; LW.anchor = null; lwPaintSel(); } return; }
    const r = lwClick(LW.sel, LW.anchor, lwShown().map((c) => c.id), p.id, p);
    LW.sel = r.sel;
    LW.anchor = r.anchor;
    lwPaintSel();
  });
  window.addEventListener("blur", () => { LW.press = null; });
  view.addEventListener("dblclick", (e) => { const id = e.target.closest(".card")?.dataset.clip; if (id) lwAdd([id]); });
  document.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && e.code === "KeyA" && document.activeElement !== q) { e.preventDefault(); LW.sel = lwShown().map((c) => c.id); lwPaintSel(); }
    else if (e.code === "Escape") { if (document.activeElement === q && q.value) { q.value = ""; LW.q = ""; lwRender(); } else if (LW.sel.length) { LW.sel = []; lwPaintSel(); } }
    else if ((e.code === "Enter" || e.code === "NumpadEnter") && document.activeElement !== q && LW.sel.length) lw$("#lw-add").click();
    else if ((e.ctrlKey || e.metaKey) && (e.code === "KeyK" || e.code === "KeyF")) { e.preventDefault(); q.focus(); q.select(); }
  });
  // Files dragged over this window are not for it (its own clips on their way out included): the page must
  // not open them.
  for (const type of ["dragenter", "dragover", "drop"]) document.addEventListener(type, (e) => { e.preventDefault(); if (e.dataTransfer) e.dataTransfer.dropEffect = "none"; });
  document.addEventListener("contextmenu", (e) => e.preventDefault());

  LT.event.listen("clips-changed", () => lwLoad());
  LT.event.listen("libwin-note", ({ payload } = {}) => { if (payload?.text) lwFlash(String(payload.text), !!payload.bad); });
  // The theme, its colors and the language follow the main window (it writes them to localStorage).
  window.addEventListener("storage", (e) => {
    if (e.key === "glint.lang") return location.reload();
    if (e.key && /^glint\.(theme|css|colors)$/.test(e.key)) { try { window.applyGlintTheme(localStorage.getItem("glint.theme") || "glint", localStorage.getItem("glint.css") || ""); } catch {} }
  });
  setInterval(() => { if (!document.hidden && !LW.dragging) lwRender(); }, 60000); // "5 min ago" moves on
  lwLoad();
}

if (typeof document !== "undefined" && typeof window !== "undefined" && window.__TAURI__ && document.getElementById("lw")) lwInit();

// Lets the pure functions be tested under Node; ignored in the app.
if (typeof module !== "undefined" && module.exports) module.exports = { lwFilter, lwGames, lwClick, lwDragIds };
