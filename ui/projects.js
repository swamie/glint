"use strict";
// Editor projects (backend: projects.rs; design: CONTRACT_0116 section D). A PROJECT is an editing
// session that is kept: a draft video made of the library clips put in it (`p.clip`, a draft:
// kind "joined", `joins`, `join_names`; null while the project is empty) plus the editor's saved
// edit (`project_save_state` / `project_state`, a text this file writes: emStateSave, `{ v: 1, ... }`).
//
// - The Editor page (S.page "editor", renderEditorPage) lists the projects; it IS the question
//   "a new session or an unfinished one?".
// - A project opens in the viewer in edit mode (openClip on the draft id = the project id), with
//   the library's clips in a panel beside it (projMountPanel) to add more. An empty project opens a
//   frame of its own with a hint (projOpenEmpty).
// - Adding runs `project_add_clips`; when "project-changed" comes for the open project the new
//   draft is loaded and every edit kept (emStateLoad + emAddParts in editmodel.js).
// - The edit is saved debounced (projDirty) and when the project is left (projFlush), never when
//   nothing changed. Closing never asks and never deletes.
// A normal library clip never comes here: editor.js calls these only for a draft (P.draft).
// Loaded after join.js, before app.js: at load time it only defines things and listens.
// Names start with `proj` / `PROJ_`, plus renderEditorPage (called by app.js render()).

const PROJ_SAVE_MS = 1500;          // debounce of the saved edit
const projCache = new Map();        // project id -> newest state text of this session (wins over the backend's copy)
const projSavedText = new Map();    // project id -> the state text last saved or loaded (an unchanged edit is not saved)
const projJobs = new Map();         // job id -> project id: adds started here
const projJobsSeen = new Set();     // ended jobs already reported
let projJobsNow = [];               // the last "jobs-changed" list
let projCarry = null;               // { id, state }: the open edit handed to the reloaded draft (with undo)
let projSaveTimer = 0;
// The clip list beside the editor. q / game stay while the app runs; sel is the selection, in the order picked.
const projLib = { id: null, q: "", game: "all", sel: [], anchor: null };
let projDrag = null;                // a press in the clip list: { x, y, item, ctrl, shift, mode, ... }

/* ---------- pure helpers (tested under node) ---------- */

/** S.projects, always an array. */
const projList = () => (typeof S !== "undefined" && Array.isArray(S.projects) ? S.projects : []);
const projById = (id) => projList().find((p) => p && p.id === id) || null;
/** Number of clips and total length (seconds) of project `p`. */
function projSummary(p) {
  const clips = Array.isArray(p?.clips) ? p.clips : [];
  const secs = p?.clip && +p.clip.duration > 0 ? +p.clip.duration : clips.reduce((s, c) => s + (+c.duration || 0), 0);
  return { n: clips.length, secs };
}
/** Why `clips` cannot be added to project `p` (limits as join.rs: 20 clips, 30 minutes for the whole project), or "". */
function projAddRefusal(p, clips) {
  const have = Array.isArray(p?.clips) ? p.clips : [];
  if (!clips.length) return t("Pick clips in the list first");
  if (have.length + clips.length > JOIN_MAX_CLIPS) return t("A project takes at most {n} clips", { n: JOIN_MAX_CLIPS });
  const secs = have.reduce((s, c) => s + (+c.duration || 0), 0) + clips.reduce((s, c) => s + (+c.duration || 0), 0);
  if (secs > JOIN_MAX_MIN * 60) return t("A project takes at most {n} minutes of clips", { n: JOIN_MAX_MIN });
  return "";
}
/** True if the saved joins `saved` are the start of the draft's joins `now` (the old ones never move when clips are added). */
function projJoinsPrefix(saved, now) {
  if (!Array.isArray(saved) || !Array.isArray(now) || saved.length > now.length) return false;
  return saved.every((x, i) => Math.abs(+x - +now[i]) < 0.05);
}
/** The library clips the list shows: not drafts, matching the search and the game, newest first. */
function projLibFilter(clips, q, game) {
  q = String(q || "").trim().toLowerCase();
  return (Array.isArray(clips) ? clips : []).filter((c) => c && !c.draft && (game === "all" || !game || c.game === game)
    && (!q || String(c.title).toLowerCase().includes(q) || String(c.game).toLowerCase().includes(q)))
    .sort((a, b) => (b.created || 0) - (a.created || 0));
}
/** The games in the list, most clips first. */
function projLibGames(clips) {
  const n = {};
  for (const c of clips || []) if (c && !c.draft) n[c.game] = (n[c.game] || 0) + 1;
  return Object.keys(n).sort((a, b) => n[b] - n[a] || a.localeCompare(b));
}
/**
 * A click in the clip list. `ids`: the list as shown; `sel`: the selection in the order picked.
 * Plain: only that clip. Ctrl: toggle it. Shift: the range from the anchor (Ctrl+Shift adds it to
 * the selection). Returns { sel, anchor }.
 */
function projSelClick(sel, anchor, ids, id, mods) {
  const ctrl = !!mods?.ctrl, shift = !!mods?.shift;
  const from = ids.indexOf(anchor), to = ids.indexOf(id);
  if (shift && from >= 0 && to >= 0) {
    const range = ids.slice(Math.min(from, to), Math.max(from, to) + 1);
    const base = ctrl ? sel.filter((x) => !range.includes(x)) : [];
    return { sel: [...base, ...range], anchor };
  }
  if (ctrl) return { sel: sel.includes(id) ? sel.filter((x) => x !== id) : [...sel, id], anchor: id };
  return { sel: [id], anchor: id };
}
/** A selection box: the ids of `rects` (each an id and its edges l / r / t / b) it touches, after those of `base` (kept in their order). */
function projBoxHits(base, rects, box) {
  const out = base.slice();
  for (const c of rects) if (c.l < box.r && c.r > box.l && c.t < box.b && c.b > box.t && !out.includes(c.id)) out.push(c.id);
  return out;
}

/* ---------- the project list ---------- */

/** Put project `p` (as the backend returned it) into S.projects, newest change first. */
function projUpsert(p) {
  if (!p || !p.id || typeof S === "undefined") return;
  const list = projList().filter((x) => x.id !== p.id);
  list.push(p);
  list.sort((a, b) => (b.modified || 0) - (a.modified || 0));
  S.projects = list;
}
async function projRefresh() {
  // An event can arrive while the page's scripts are still loading: app.js (S, invoke) comes after this file.
  if (typeof S === "undefined" || typeof invoke !== "function") { setTimeout(projRefresh, 50); return; }
  try {
    const list = await invoke("list_projects");
    S.projects = Array.isArray(list) ? list : [];
  } catch (e) {
    console.warn("Glint: list_projects failed", e);
    if (!Array.isArray(S.projects)) S.projects = [];
  }
  projRefreshUI();
}
/** Bring whatever shows projects up to date: the sidebar badge, the Editor page, the open project's panel. */
function projRefreshUI() {
  if (typeof S === "undefined" || !S.settings) return;
  if (S.page === "editor" && !S.open && !S.renaming) render();
  else if (typeof renderChrome === "function") renderChrome();
  projRenderPanel();
}

/** The job adding clips to project `pid` now (queued or running), or null. */
function projJobOf(pid) {
  const list = projJobsNow.length ? projJobsNow : (typeof jobsList !== "undefined" ? jobsList : []);
  return list.find((j) => j && j.kind === "join" && (j.state === "queued" || j.state === "running")
    && (projJobs.get(j.id) === pid || j.result_id === pid || j.clip_id === pid)) || null;
}
/** The progress line of a busy project: text and percent, or null. */
function projBusyInfo(p) {
  const job = p ? projJobOf(p.id) : null;
  if (!job && !p?.busy) return null;
  const pct = job ? Math.max(0, Math.min(100, +job.pct || 0)) : 0;
  return { pct, text: job && job.state === "running" ? t("Adding clips… {pct} %", { pct: Math.floor(pct) }) : t("Waiting to add the clips") };
}

/* ---------- the Editor page ---------- */

function projCardHtml(p) {
  const { n, secs } = projSummary(p), busy = projBusyInfo(p), c = p.clip;
  const bg = c && c.thumb ? "" : `style="background:linear-gradient(135deg,hsl(${hue(p.name)} 40% 22%),hsl(${(hue(p.name) + 50) % 360} 45% 12%))"`;
  const meta = [n === 1 ? t("1 clip") : t("{n} clips", { n }), ...(secs > 0 ? [fmtDur(secs)] : [])].join(" · ");
  return `<article class="card pj-card" data-pj="${esc(p.id)}" tabindex="0" data-hover title="${esc(p.name)}">
    <div class="thumb" ${bg}>
      ${c && c.thumb ? `<img class="thumb-img" src="${esc(fileSrc(c.thumb))}" alt="" draggable="false" decoding="async" onerror="this.remove()" style="${typeof THUMB_IMG_STYLE === "string" ? THUMB_IMG_STYLE : ""}">` : `<div class="ph">${n ? JOIN_ICON : ICON.plus}</div>`}
      ${p.exported ? `<span class="tag pj-tag-exp">${esc(t("Exported"))}</span>` : !n ? `<span class="tag pj-tag-empty">${esc(t("Empty"))}</span>` : ""}
      ${secs > 0 ? `<span class="dur">${fmtDur(secs)}</span>` : ""}
      ${busy ? `<div class="pj-card-busy"><span>${esc(busy.text)}</span></div><div class="job"><i style="width:${busy.pct}%"></i></div>` : ""}
    </div>
    <h3>${esc(p.name)}</h3>
    <p><span>${esc(meta)}</span><span title="${esc(t("Last edited"))}">${esc(timeAgo(p.modified || p.created || Date.now()))}</span>${p.size > 0 ? `<span>${fmtSize(p.size)}</span>` : ""}</p>
  </article>`;
}

/** The Editor page (app.js render() calls it for S.page "editor"). */
function renderEditorPage() {
  const list = projList();
  const empty = `<div class="empty pj-empty-page"><h2>${esc(t("No projects yet"))}</h2>
    <p>${esc(t("A project is an editing session that is kept: the clips you put in it, your cuts, their order and levels. Close it at any time and go on later, right where you left off."))}</p>
    <p>${esc(t("Start one with New project, or select clips in the library and pick Edit together. Export saves the result as a new clip; the clips themselves stay as they are."))}</p>
    <button class="btn primary" data-pjp="new">${ICON.plus}${esc(t("New project"))}</button></div>`;
  $("#view").innerHTML = `
    <div class="page-head">
      <div><h1>${esc(t("Editor"))}</h1><div class="sub">${esc(list.length ? t("Your editing sessions. Open one to go on, or start a new one.") : t("Edit clips together and keep your work"))}</div></div>
      <div class="head-actions"><button class="btn primary" data-pjp="new">${ICON.plus}${esc(t("New project"))}</button></div>
    </div>
    ${list.length ? `<div class="grid m pj-grid">${list.map(projCardHtml).join("")}</div>` : empty}`;
}

async function projNew() {
  const name = await askText(t("Name the new project"), t("Project {n}", { n: projList().length + 1 }), t("Create"));
  if (!name) return;
  try {
    const p = await invoke("create_project", { name });
    if (!p || !p.id) throw new Error("create_project returned nothing");
    projUpsert(p);
    projRefreshUI();
    projOpen(p.id);
  } catch (e) { fail(e); }
}
async function projRename(id) {
  const p = projById(id);
  if (!p) return;
  const name = await askText(t('Rename the project "{name}"', { name: p.name }), p.name, t("Rename"));
  if (!name || name === p.name) return;
  try { projUpsert(await invoke("rename_project", { id, name })); } catch (e) { fail(e); }
  projRefreshUI();
}
async function projDelete(id) {
  const p = projById(id);
  if (!p) return;
  if (!(await ask(t('Delete the project "{name}"? Your edit in it is lost. The clips themselves stay in your library, and so do the clips you exported from it.', { name: p.name }), t("Delete project")))) return;
  try {
    const ok = await invoke("delete_project", { id });
    if (!ok) return flash(t("Clips are still being added to this project. Try again when that is done."), true);
    projCache.delete(id);
    projSavedText.delete(id);
    S.projects = projList().filter((x) => x.id !== id);
    sfx("close");
    flash(t("Project deleted"));
  } catch (e) { fail(e); }
  projRefreshUI();
}
function projOpenMenu(id, x, y) {
  const p = projById(id);
  if (!p) return closeMenu();
  $("#menu").innerHTML = projMItem("open", t("Open"), PICON.play, id) + projMItem("rename", t("Rename…"), ICON.edit, id) + "<hr>" + projMItem("delete", t("Delete"), ICON.trash, id, "danger");
  placeMenu(x, y);
}
/** A menu item of this file: data-pjm instead of app.js's data-m, so menuAction never sees it. */
const projMItem = (act, label, icon, arg = "", cls = "", disabled = false) =>
  `<button data-pjm="${act}" data-arg="${esc(arg)}" class="${cls}"${disabled ? ' disabled aria-disabled="true" style="opacity:.5;cursor:default"' : ""}>${icon || "<svg></svg>"}<span>${esc(label)}</span></button>`;

/* ---------- opening a project ---------- */

/** Open project `id`: its draft in the editor, or the empty frame. */
function projOpen(id) {
  const p = projById(id);
  if (!p) return;
  if (projLib.id !== id) { projLib.id = id; projLib.sel = []; projLib.anchor = null; }
  if (p.clip && p.clip.id === id) return openClip(id);
  projOpenEmpty(p);
}

/** An empty project: the editor frame, with the clip list and a hint where the video goes. */
function projOpenEmpty(p) {
  if (S.player && typeof projFlush === "function") projFlush(S.player);
  if (S.player) { S.player.destroy(); S.player = null; }
  closeMenu();
  S.open = p.id;
  const m = $("#modal");
  m.hidden = false;
  m.innerHTML = `
    <div class="viewer editing pj-blank">
      <div class="v-main"><div class="pj-empty"><div class="pj-empty-box">
        ${JOIN_ICON}
        <h2>${esc(t("This project has no clips yet"))}</h2>
        <p>${esc(t("Pick clips in the list on the left, then double-click them, press Enter or drag them here. They are put in the order you picked them."))}</p>
        <div class="pj-empty-busy small" hidden></div>
      </div></div></div>
      <div class="v-side"><div class="v-pane">
        <div class="v-title v-draft-title">${esc(p.name)}</div>
        <div class="small">${esc(t("Your edit is kept in the project. Close it at any time and go on later from the Editor page."))}</div>
        <span style="flex:1"></span>
        <button class="btn wide" data-v="close">${esc(t("Close"))}</button>
      </div></div>
    </div>`;
  projMountPanel($(".viewer", m), p.id);
}

/** Put the clip list beside the editor of project `id` (editor.js calls it for a draft; projOpenEmpty too). */
function projMountPanel(root, id) {
  if (!root || !projById(id)) return;
  if (projLib.id !== id) { projLib.id = id; projLib.sel = []; projLib.anchor = null; }
  root.classList.add("pj-on");
  const aside = document.createElement("aside");
  aside.className = "pj-lib";
  aside.dataset.pjFor = id;
  aside.innerHTML = `
    <div class="pj-head"><div class="pj-h">${esc(t("Library"))}</div>
      <div class="small pj-hint">${esc(t("Double-click, press Enter or drag clips onto the timeline to add them to the project."))}</div></div>
    <div class="pj-tools">
      <input class="field pj-q" type="search" spellcheck="false" autocomplete="off" placeholder="${esc(t("Search clips"))}" aria-label="${esc(t("Search clips"))}" value="${esc(projLib.q)}">
      <select class="field pj-game" aria-label="${esc(t("Game"))}"></select>
    </div>
    <div class="pj-list" tabindex="0" role="listbox" aria-multiselectable="true" aria-label="${esc(t("Library clips"))}"></div>
    <div class="pj-foot">
      <div class="pj-busy small" hidden></div>
      <button type="button" class="btn primary wide pj-add" data-pja="add" disabled></button>
    </div>`;
  root.prepend(aside);
  // Buttons here keep the focus off themselves, as in the viewer (Space still plays).
  aside.addEventListener("input", (e) => {
    if (!e.target.classList.contains("pj-q")) return;
    projLib.q = e.target.value;
    projRenderList();
  });
  aside.addEventListener("change", (e) => {
    if (!e.target.classList.contains("pj-game")) return;
    projLib.game = e.target.value;
    e.target.blur();
    projRenderList();
  });
  aside.addEventListener("mousedown", projListDown);
  aside.addEventListener("dblclick", (e) => {
    const item = e.target.closest(".pj-item");
    if (!item) return;
    const id2 = item.dataset.pjc;
    if (!projLib.sel.includes(id2)) projLib.sel = [id2];
    projAddSel();
  });
  aside.addEventListener("contextmenu", (e) => {
    if (e.target.closest("input, textarea, select")) return; // app.js leaves those alone too
    e.preventDefault();
    e.stopPropagation(); // app.js's own handler would close the menu again
    const item = e.target.closest(".pj-item");
    if (item && !projLib.sel.includes(item.dataset.pjc)) { projLib.sel = [item.dataset.pjc]; projLib.anchor = item.dataset.pjc; projPaintSel(); }
    projOpenLibMenu(e.clientX, e.clientY);
  });
  projRenderPanel();
}

/** The panel of the open project, or null. */
const projPanel = () => (typeof document !== "undefined" ? document.querySelector("#modal .pj-lib") : null);

/** Redraw the open project's panel: game filter, list, busy line and Add button. */
function projRenderPanel() {
  const aside = projPanel();
  if (!aside) return;
  const sel = $(".pj-game", aside), games = projLibGames(S.clips);
  if (projLib.game !== "all" && !games.includes(projLib.game)) projLib.game = "all";
  const opts = `<option value="all">${esc(t("All games"))}</option>` + games.map((g) => `<option value="${esc(g)}"${g === projLib.game ? " selected" : ""}>${esc(g)}</option>`).join("");
  if (sel.innerHTML !== opts) sel.innerHTML = opts;
  sel.value = projLib.game;
  projRenderList();
  projRenderBusy();
}

function projItemHtml(c, inProj) {
  const on = projLib.sel.includes(c.id);
  const bg = c.thumb ? "" : ` style="background:linear-gradient(135deg,hsl(${hue(c.game)} 45% 22%),hsl(${(hue(c.game) + 50) % 360} 50% 12%))"`;
  return `<div class="pj-item${on ? " sel" : ""}${inProj ? " in" : ""}" data-pjc="${esc(c.id)}" role="option" aria-selected="${on}" title="${esc(c.title + (inProj ? "\n" + t("Already in the project") : ""))}">
    <div class="pj-thumb"${bg}>${c.thumb ? `<img src="${esc(fileSrc(c.thumb))}" alt="" draggable="false" decoding="async" onerror="this.remove()">` : `<span class="pj-ph">${esc(String(c.game || "?").slice(0, 1).toUpperCase())}</span>`}<span class="pj-dur">${fmtDur(c.duration)}</span></div>
    <div class="pj-txt"><div class="pj-title">${esc(c.title)}</div><div class="pj-meta">${esc(c.game)} · ${esc(timeAgo(c.created))}</div></div>
    ${inProj ? `<span class="pj-in" aria-label="${esc(t("Already in the project"))}">${ICON.check}</span>` : ""}
  </div>`;
}
/** The clips shown in the list now. */
const projShown = () => projLibFilter(typeof S !== "undefined" ? S.clips : [], projLib.q, projLib.game);

function projRenderList() {
  const aside = projPanel();
  if (!aside) return;
  const list = $(".pj-list", aside), shown = projShown();
  const have = new Set((projById(aside.dataset.pjFor)?.clips || []).map((c) => c.id).filter(Boolean));
  // The selection keeps only clips that still exist (a search does not drop it).
  const all = new Set((S.clips || []).map((c) => c.id));
  projLib.sel = projLib.sel.filter((id) => all.has(id));
  const html = shown.length ? shown.map((c) => projItemHtml(c, have.has(c.id))).join("")
    : `<div class="pj-none small">${esc((S.clips || []).length ? t("No clips match the search.") : t("No clips in your library yet."))}</div>`;
  const top = list.scrollTop;
  list.innerHTML = html;
  list.scrollTop = top;
  projRenderAddBtn();
}
/** Update the selection marks without rebuilding the list. */
function projPaintSel() {
  const aside = projPanel();
  if (!aside) return;
  for (const el of aside.querySelectorAll(".pj-item")) {
    const on = projLib.sel.includes(el.dataset.pjc);
    el.classList.toggle("sel", on);
    el.setAttribute("aria-selected", String(on));
  }
  projRenderAddBtn();
}
function projRenderAddBtn() {
  const btn = $(".pj-add", projPanel() || document.createElement("div"));
  if (!btn) return;
  const n = projLib.sel.length;
  btn.disabled = !n;
  btn.innerHTML = `${ICON.plus}<span>${esc(n ? t("Add {clips} to the project", { clips: plural(n) }) : t("Add to the project"))}</span>`;
}
function projRenderBusy() {
  const aside = projPanel();
  if (!aside) return;
  const info = projBusyInfo(projById(aside.dataset.pjFor));
  const html = info ? `<div>${esc(info.text)}</div><div class="progress"><i style="width:${info.pct}%"></i></div>` : "";
  for (const el of [$(".pj-busy", aside), $("#modal .pj-empty-busy")]) {
    if (!el) continue;
    el.hidden = !info;
    if (el.innerHTML !== html) el.innerHTML = html;
  }
}

/** Right-click in the clip list. */
function projOpenLibMenu(x, y) {
  const n = projLib.sel.length;
  $("#menu").innerHTML = (n > 1 ? `<div class="menu-head">${esc(t("{n} selected", { n }))}</div>` : "")
    + projMItem("add", n ? t("Add {clips} to the project", { clips: plural(n) }) : t("Add to the project"), ICON.plus, "", "", !n)
    + "<hr>" + projMItem("all", t("Select all"), ICON.check)
    + projMItem("none", t("Clear selection"), ICON.x, "", "", !n)
    + projMItem("show", t("Show in library"), ICON.library, "", "", !n);
  placeMenu(x, y);
}

/** Close the editor and show clips `ids` selected in the library. */
function projShowInLibrary(ids) {
  if (!ids.length) return;
  closeClip();
  S.page = "library"; S.folder = null; S.tab = "all"; S.q = "";
  const q = $("#q");
  if (q) q.value = "";
  S.sel = new Set(ids);
  S.anchor = ids[ids.length - 1];
  render();
  const card = $$("#view .card").find((el) => el.dataset.clip === ids[0]);
  card?.scrollIntoView?.({ block: "center" });
}

/* ---------- adding clips ---------- */

/** Add the selected clips (in the order picked) to the open project. */
function projAddSel() {
  const id = projPanel()?.dataset.pjFor;
  if (id) projAdd(id, projLib.sel.slice());
}
async function projAdd(pid, ids) {
  const p = projById(pid);
  if (!p) return;
  const byId = new Map((S.clips || []).map((c) => [c.id, c]));
  const clips = [...new Set(ids)].map((x) => byId.get(x)).filter((c) => c && !c.draft);
  const why = projAddRefusal(p, clips);
  if (why) return flash(why, true);
  try {
    const job = await invoke("project_add_clips", { id: pid, ids: clips.map((c) => c.id) });
    if (job && job.id) {
      projJobs.set(job.id, pid);
      if (!projJobsNow.some((j) => j.id === job.id)) projJobsNow = [...projJobsNow, job];
      // Shown at once; the backend's "jobs-changed" brings the real list right after.
      if (typeof jobsSet === "function" && !jobsList.some((j) => j.id === job.id)) jobsSet([...jobsList, job]);
    }
    p.busy = true;
    projLib.sel = [];
    projLib.anchor = null;
    sfx("launch");
    flash(t("Adding {clips} to the project…", { clips: plural(clips.length) }));
  } catch (e) {
    flash(joinError(e), true);
  }
  projRefreshUI();
}

/** "project-changed": an add ended well. The open project loads its new draft and keeps its edit. */
function projChanged(pl) {
  if (!pl || !pl.project || !pl.project.id) return;
  const p = pl.project, mine = projJobs.has(pl.job_id);
  projJobs.delete(pl.job_id);
  projUpsert(p);
  if (typeof joinDone === "function" && joinDone(pl)) return projRefreshUI(); // "Edit together": it opens the project
  const n = Array.isArray(pl.added) ? pl.added.length : 0;
  if (S.open === p.id) {
    projReload(p);
    if (n) flash(t("Added {clips} to the project", { clips: plural(n) }));
  } else if (mine && n) flash(t('Added {clips} to "{name}"', { clips: plural(n), name: p.name }));
  if (mine) sfx("success");
  projRefreshUI();
}

/** The open project's draft was rebuilt: open it again, handing over the edit (with its undo), and go to the new clips. */
function projReload(p) {
  if (!p.clip || p.clip.id !== p.id) return;
  const P = S.player;
  let at = null;
  if (P && P.draft && P.model && P.clipMeta && P.clipMeta.id === p.id) {
    try {
      projCarry = { id: p.id, state: emStateSave(P.model, P.clipMeta.joins || [], true) };
      at = P.model.duration; // where the new clips start
    } catch (e) { console.warn("Glint: could not hand the edit over; it comes back from the last save", e); projCarry = null; }
  }
  openClip(p.id);
  const N = S.player;
  if (N && at != null) N.whenModel?.().then((m) => { if (m && S.player === N && N.alive()) N.seek(Math.min(at, m.duration), false, true); });
}

/* ---------- the saved edit ---------- */

/** The open project's edit as the text that is saved. */
const projStateText = (P) => JSON.stringify(emStateSave(P.model, Array.isArray(P.clipMeta?.joins) ? P.clipMeta.joins : []));

/**
 * editor.js, making a project's model `m` (a fresh emNew of draft `c`): put the saved edit on it.
 * The newest state of this session wins over the backend's copy. When the draft has grown since
 * (clips added), the new clips are appended as parts. Returns false when nothing usable was
 * saved (the project then opens clean), true when restored, "added" when clips were appended too.
 */
async function projRestore(c, m) {
  const id = c.id;
  let text = null;
  if (projCarry && projCarry.id === id) { text = projCarry.state; projCarry = null; }
  else if (projCache.has(id)) text = projCache.get(id);
  else {
    try { text = await invoke("project_state", { id }); }
    catch (e) { console.warn("Glint: project_state failed; the project opens without its saved edit", e); return false; }
  }
  if (text == null || text === "") return false;
  let s = text;
  if (typeof s === "string") { try { s = JSON.parse(s); } catch { console.warn("Glint: the project's saved edit is not readable; it opens clean"); return false; } }
  const joins = Array.isArray(c.joins) ? c.joins.map(Number) : [];
  if (!s || !projJoinsPrefix(s.joins, joins)) { console.warn("Glint: the project's saved edit does not fit its clips; it opens clean"); return false; }
  const end = m.duration; // the draft's length now
  const k = s.joins.length, fresh = joins.slice(k);
  // New clips start where the old draft ended. One that starts inside it means the old seconds moved
  // (not what projects.rs does): the edit would land on other pictures, so it opens clean instead.
  if (fresh.length && !(fresh[0] >= +s.duration - 0.25)) { console.warn("Glint: the project's draft no longer matches its saved edit; it opens clean"); return false; }
  if (!emStateLoad(m, s)) { console.warn("Glint: the project's saved edit is not readable; it opens clean"); return false; }
  if (!fresh.length) return true;
  const names = Array.isArray(c.join_names) ? c.join_names : [];
  const name = (i) => (typeof names[i] === "string" && names[i].trim()) || t("Clip {n}", { n: i + 1 });
  return emAddParts(m, fresh, fresh.map((_, j) => name(k + 1 + j)), end, name(0)) ? "added" : true;
}

/** editor.js: the project's model is ready. An edit opened as it was saved is not saved again; one that grew is. */
function projModelReady(P, restored) {
  if (!P || !P.model || !P.clipMeta) return;
  const id = P.clipMeta.id;
  let text;
  try { text = projStateText(P); } catch (e) { return console.warn("Glint: could not read the project's edit", e); }
  if (restored === "added") return projDirty(P, "add");
  projSavedText.set(id, text);
  projCache.set(id, text);
}

/** The open project's edit changed (editor.js P.edSync / bookmarks): save it in a moment. */
function projDirty(P, what) {
  if (!P || !P.draft || what === "load") return;
  clearTimeout(projSaveTimer);
  projSaveTimer = setTimeout(() => { projSaveTimer = 0; if (S.player === P && P.alive?.()) projFlush(P); }, PROJ_SAVE_MS);
}

/** Save the project's edit now if it changed (leaving it, the window closing). Returns the save's promise, or null. */
function projFlush(P) {
  clearTimeout(projSaveTimer);
  projSaveTimer = 0;
  if (!P || !P.draft || !P.model || !P.clipMeta || !projById(P.clipMeta.id)) return null;
  const id = P.clipMeta.id;
  let text;
  try { text = projStateText(P); } catch (e) { console.warn("Glint: could not read the project's edit", e); return null; }
  projCache.set(id, text);
  if (projSavedText.get(id) === text) return null;
  projSavedText.set(id, text);
  return Promise.resolve().then(() => invoke("project_save_state", { id, state: text })).then((p) => {
    if (p && p.id) { projUpsert(p); projRefreshUI(); }
  }).catch((e) => {
    console.warn("Glint: project_save_state failed; trying again at the next change", e);
    if (projSavedText.get(id) === text) projSavedText.delete(id);
  });
}

/* ---------- the clip list: click, Ctrl / Shift, box, drag onto the editor ---------- */

function projListDown(e) {
  projDragEnd(false);
  if (e.button !== 0) return;
  const list = e.target.closest(".pj-list");
  if (!list) return;
  const r = list.getBoundingClientRect();
  if (e.clientX - r.left >= list.clientWidth) return; // the scroll bar
  e.preventDefault(); // no text selection; the list takes the focus for Enter / Ctrl+A
  list.focus({ preventScroll: true });
  projDrag = { x: e.clientX, y: e.clientY, cx: e.clientX, cy: e.clientY, list, item: e.target.closest(".pj-item")?.dataset.pjc || null, ctrl: e.ctrlKey, shift: e.shiftKey, mode: null };
}
/** The drop target under the pointer: the timeline, the video or the empty project's frame. */
const projDropAt = (x, y) => document.elementFromPoint(x, y)?.closest?.("#modal .e-tl, #modal .player, #modal .pj-empty") || null;

function projDragMove(e) {
  const d = projDrag;
  if (!d) return;
  if (e.buttons === 0) return projDragEnd(false);
  d.cx = e.clientX;
  d.cy = e.clientY;
  if (!d.mode) {
    if (Math.hypot(d.cx - d.x, d.cy - d.y) < 6) return;
    if (d.item) {
      d.mode = "add";
      if (!projLib.sel.includes(d.item)) { projLib.sel = [d.item]; projLib.anchor = d.item; projPaintSel(); }
      d.ghost = document.createElement("div");
      d.ghost.className = "drag-ghost";
      d.ghost.textContent = t("Add {clips}", { clips: plural(projLib.sel.length) });
      document.body.append(d.ghost);
      document.body.classList.add("pj-dragging");
    } else {
      const r = d.list.getBoundingClientRect(), top = d.list.scrollTop - r.top;
      d.mode = "box";
      d.base = d.ctrl || d.shift ? projLib.sel.slice() : [];
      d.ox = d.x - r.left;
      d.oy = d.y + top;
      d.rects = [...d.list.querySelectorAll(".pj-item")].map((el) => { const b = el.getBoundingClientRect(); return { id: el.dataset.pjc, l: b.left - r.left, r: b.right - r.left, t: b.top + top, b: b.bottom + top }; });
      d.box = document.createElement("div");
      d.box.className = "marquee";
      d.list.append(d.box);
      document.body.classList.add("selecting");
      d.timer = setInterval(() => {
        const b = d.list.getBoundingClientRect();
        const dy = d.cy < b.top + 32 ? -Math.ceil((b.top + 32 - d.cy) / 3) : d.cy > b.bottom - 32 ? Math.ceil((d.cy - b.bottom + 32) / 3) : 0;
        if (dy) { d.list.scrollTop += Math.max(-30, Math.min(30, dy)); projBoxUpdate(d); }
      }, 16);
    }
  }
  if (d.mode === "box") return projBoxUpdate(d);
  d.ghost.style.transform = `translate(${d.cx + 14}px, ${d.cy + 16}px)`;
  const target = projDropAt(d.cx, d.cy);
  if (target !== d.target) {
    d.target?.classList.remove("pj-drop");
    target?.classList.add("pj-drop");
    d.target = target;
  }
}
function projBoxUpdate(d) {
  const r = d.list.getBoundingClientRect();
  const x = Math.max(0, Math.min(d.list.clientWidth, d.cx - r.left));
  const y = Math.max(0, Math.min(d.list.scrollHeight, d.cy - r.top + d.list.scrollTop));
  const box = { l: Math.min(x, d.ox), r: Math.max(x, d.ox), t: Math.min(y, d.oy), b: Math.max(y, d.oy) };
  if (!d.box.isConnected) d.list.append(d.box); // the list was redrawn under the drag
  Object.assign(d.box.style, { left: `${box.l}px`, top: `${box.t}px`, width: `${box.r - box.l}px`, height: `${box.b - box.t}px` });
  projLib.sel = projBoxHits(d.base, d.rects, box);
  projPaintSel();
}
/** End the press in progress. `drop`: a real release (a click, a box, a drop); otherwise it is dropped without effect. */
function projDragEnd(drop) {
  const d = projDrag;
  if (!d) return;
  projDrag = null;
  clearInterval(d.timer);
  d.ghost?.remove();
  d.target?.classList.remove("pj-drop");
  document.body.classList.remove("pj-dragging", "selecting");
  if (d.box) { const box = d.box; box.classList.add("out"); setTimeout(() => box.remove(), 170); }
  if (!drop) return;
  if (!d.mode) {
    if (d.item) {
      const r = projSelClick(projLib.sel, projLib.anchor, projShown().map((c) => c.id), d.item, { ctrl: d.ctrl, shift: d.shift });
      projLib.sel = r.sel;
      projLib.anchor = r.anchor;
    } else if (!d.ctrl && !d.shift) { projLib.sel = []; projLib.anchor = null; }
    return projPaintSel();
  }
  if (d.mode === "add" && d.target) projAddSel();
}

/** Keys while the clip list (or its search box) has the focus. True if the key was used. */
function projKey(e) {
  const aside = projPanel();
  const at = document.activeElement;
  if (!aside || !at || !aside.contains(at)) return false;
  if (at.classList.contains("pj-q")) {
    if (e.code === "Escape") { if (at.value) { at.value = ""; projLib.q = ""; projRenderList(); } else at.blur(); return true; }
    if (e.code === "ArrowDown") { $(".pj-list", aside)?.focus(); return true; }
    return false;
  }
  if (!at.classList.contains("pj-list")) return false;
  const ids = projShown().map((c) => c.id);
  if ((e.code === "Enter" || e.code === "NumpadEnter") && !e.altKey) { if (projLib.sel.length) projAddSel(); return true; }
  if (e.ctrlKey && e.code === "KeyA") { projLib.sel = [...projLib.sel, ...ids.filter((x) => !projLib.sel.includes(x))]; projPaintSel(); return true; }
  if (e.code === "Escape" && projLib.sel.length) { projLib.sel = []; projLib.anchor = null; projPaintSel(); return true; }
  if ((e.code === "ArrowDown" || e.code === "ArrowUp") && ids.length && !e.altKey) {
    const cur = ids.indexOf(projLib.sel[projLib.sel.length - 1]);
    const next = ids[Math.max(0, Math.min(ids.length - 1, cur < 0 ? 0 : cur + (e.code === "ArrowDown" ? 1 : -1)))];
    const r = projSelClick(projLib.sel, projLib.anchor, ids, next, { shift: e.shiftKey, ctrl: false });
    projLib.sel = r.sel;
    projLib.anchor = e.shiftKey ? projLib.anchor ?? next : next;
    projPaintSel();
    aside.querySelector(`.pj-item[data-pjc="${CSS.escape(next)}"]`)?.scrollIntoView?.({ block: "nearest" });
    return true;
  }
  return false;
}

/* ---------- wiring (in the app only; node tests load this file without a page) ---------- */
if (typeof document !== "undefined" && typeof window !== "undefined" && window.__TAURI__) {
  document.addEventListener("click", (e) => {
    const b = e.target.closest?.("[data-pjm],[data-pja],[data-pjp],[data-pj]");
    if (!b || b.disabled) return;
    const d = b.dataset;
    try {
      if (d.pjp === "new") return void projNew();
      if (d.pja === "add") return projAddSel();
      if (d.pjm) {
        closeMenu();
        if (d.pjm === "open") return projOpen(d.arg);
        if (d.pjm === "rename") return void projRename(d.arg);
        if (d.pjm === "delete") return void projDelete(d.arg);
        if (d.pjm === "add") return projAddSel();
        if (d.pjm === "all") { const ids = projShown().map((c) => c.id); projLib.sel = [...projLib.sel, ...ids.filter((x) => !projLib.sel.includes(x))]; return projPaintSel(); }
        if (d.pjm === "none") { projLib.sel = []; projLib.anchor = null; return projPaintSel(); }
        if (d.pjm === "show") return projShowInLibrary(projLib.sel.slice());
        return;
      }
      if (d.pj && S.page === "editor" && !S.open) return projOpen(d.pj);
    } catch (err) { fail(err); }
  });
  // Registered before app.js's handler on the same node: stopImmediatePropagation keeps it from closing this menu.
  document.addEventListener("contextmenu", (e) => {
    const card = e.target.closest?.("#view .pj-card");
    if (!card || S.open || S.page !== "editor") return;
    e.preventDefault();
    e.stopImmediatePropagation();
    projOpenMenu(card.dataset.pj, e.clientX, e.clientY);
  });
  // Before app.js's own key handler (registered later on the same node, also capturing).
  window.addEventListener("keydown", (e) => {
    if (typeof askKey !== "undefined" && askKey) return; // a prompt is up: its keys
    let used = false;
    if (S.open) used = projKey(e);
    else if (S.page === "editor" && e.target.closest?.(".pj-card") && !e.repeat) {
      const id = e.target.closest(".pj-card").dataset.pj;
      if (e.code === "Enter" || e.code === "NumpadEnter") { projOpen(id); used = true; }
      else if (e.code === "Delete") { projDelete(id); used = true; }
      else if (e.code === "F2") { projRename(id); used = true; }
    }
    if (used) { e.preventDefault(); e.stopImmediatePropagation(); }
  }, true);
  window.addEventListener("mousemove", projDragMove);
  window.addEventListener("mouseup", () => projDragEnd(true));
  window.addEventListener("blur", () => projDragEnd(false));
  document.addEventListener("visibilitychange", () => { if (document.hidden) projDragEnd(false); });
  // The window going away some other way (Alt+F4): best effort; the debounced save has usually run.
  window.addEventListener("pagehide", () => { try { if (S.player) projFlush(S.player); } catch {} });

  const listen = window.__TAURI__.event?.listen;
  if (listen) {
    listen("projects-changed", () => projRefresh());
    listen("project-changed", ({ payload }) => { try { projChanged(payload); } catch (e) { console.error("Glint: project-changed failed", e); } });
    listen("jobs-changed", ({ payload }) => {
      projJobsNow = Array.isArray(payload) ? payload : [];
      for (const j of projJobsNow) {
        if (!projJobs.has(j.id) || projJobsSeen.has(j.id) || (j.state !== "error" && j.state !== "cancelled")) continue;
        projJobsSeen.add(j.id);
        projJobs.delete(j.id);
        if (j.state === "error") flash(j.message ? t("The clips could not be added: {reason}", { reason: joinError(j.message) }) : t("The clips could not be added"), true);
        projRefresh();
      }
      projRenderBusy();
      if (S.page === "editor" && !S.open && projList().some((p) => p.busy || projJobOf(p.id))) projRefreshUI();
    });
    // The library changed under the open project's list.
    listen("clips-changed", () => setTimeout(projRenderPanel, 400));
  }
  // S.projects for the sidebar badge. A timer can fire between two script files, so wait until
  // app.js (loaded after this file) has defined S and invoke.
  const boot = () => (typeof S === "undefined" || typeof invoke !== "function" ? setTimeout(boot, 50) : projRefresh());
  setTimeout(boot, 0);
}
