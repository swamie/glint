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
// 0.1.38 (CONTRACT_0138): a project made now is VIRTUAL (`p.virtual`): only its clip list and the edit, no
// draft file. `p.clip` is then what `project_open` returns (virtual: true, `sources` laid end to end, `joins` =
// every source's start INCLUDING 0, one `join_names` entry per source). It opens with
// openClip(<that object>) (projOpenVirtual); adding answers at once and the open editor takes the new sources
// with P.addSources (editor.js, no reload); "Edit together" makes one at once (join.js). A clip gone from the
// library is a `missing` source: the card and the clip list say so. Projects made before keep their draft and
// the job path below. projJoinsOf gives both kinds the draft's joins (starts after the first) for the saved edit.
// 0.1.40 (CONTRACT_0140): video files dropped into the editor come to projDropClips(paths, at) (B's edDropInit);
// one click in the Library list previews the clip at the top of it (projPvShow, its own player); a single library
// clip open in the editor gets the list too, and adding to it makes a project of it (projFromClip, edit carried).
// Otherwise a normal library clip never comes here: editor.js calls the rest only for a draft (P.draft).
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
// The Editor page's selection: projects picked with Ctrl / Shift, their tick or a box, in the order picked.
const projSel = { ids: [], anchor: null };
let projPageDrag = null;            // a press on the Editor page: { x, y, cx, cy, card, add, mode, ... }
let projBoxAt = -1e9;               // when a selection box was last let go (that release is not a click on a card)

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
/**
 * The length of the RESULT of a saved edit (0.1.43: a project's card showed the length of all its footage, 10:27
 * for a 3:36 montage). `s` is the parsed state text (emStateSave). Timeline seconds are the result's before the
 * whole-result speed: the pieces on the video row, or whatever runs on after them (an overlay item, an audio clip).
 * 0 when the state says nothing usable.
 */
function projStateLen(s) {
  if (!s || typeof s !== "object") return 0;
  const num = (x) => (typeof x === "number" && Number.isFinite(x) ? x : 0);
  let video = 0;
  if (Array.isArray(s.parts) && s.parts.length) video = s.parts.reduce((a, p) => a + Math.max(0, num(p && p.len)), 0);
  else if (Array.isArray(s.removed) && num(s.duration) > 0) {
    // An edit without pieces (a plain clip): what is kept between its cuts.
    const edges = [0, ...(Array.isArray(s.cuts) ? s.cuts.map(num) : []), num(s.duration)];
    s.removed.forEach((gone, i) => { if (!gone && i + 1 < edges.length) video += Math.max(0, edges[i + 1] - edges[i]); });
  }
  let end = video;
  for (const o of Array.isArray(s.overlays) ? s.overlays : []) end = Math.max(end, num(o && o.to));
  for (const r of Array.isArray(s.audio) ? s.audio : []) if (r && r.kind === "file") end = Math.max(end, num(r.offset) + num(r.length));
  const speed = num(s.speed) > 0 ? num(s.speed) : 1;
  return end > 0 ? end / speed : 0;
}
// Project id -> { mod, secs }: the result length of its saved edit, read once per change of the project.
const projLens = new Map();
let projLenTimer = 0;
/** The edit's length of project `p` if it is known (else 0), asking for it in the background when it is not. */
function projEditLen(p) {
  if (!p || !p.id) return 0;
  const mod = p.modified || p.created || 0, hit = projLens.get(p.id);
  if (hit && hit.mod === mod) return hit.secs;
  if (hit && hit.pending === mod) return hit.secs || 0;
  projLens.set(p.id, { mod: hit ? hit.mod : -1, secs: hit ? hit.secs : 0, pending: mod });
  if (typeof invoke === "function") {
    invoke("project_state", { id: p.id }).then((text) => {
      let secs = 0;
      try { secs = text ? projStateLen(JSON.parse(text)) : 0; } catch { secs = 0; }
      projLens.set(p.id, { mod, secs });
      // One repaint for however many answers arrive together.
      clearTimeout(projLenTimer);
      projLenTimer = setTimeout(() => { if (typeof S !== "undefined" && S.page === "editor" && !S.open && !S.renaming && typeof render === "function") render(); }, 60);
    }).catch(() => projLens.set(p.id, { mod, secs: 0 }));
  }
  return hit ? hit.secs || 0 : 0;
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
/**
 * The joins the saved edit uses: where each clip after the first starts. A draft's `joins` are that; a virtual
 * project's start with the first clip's 0 (one per source), which is dropped here, so its `join_names[i + 1]`
 * is the name of join i for both kinds.
 */
function projJoinsOf(c) {
  const j = Array.isArray(c?.joins) ? c.joins.map(Number) : [];
  return c?.virtual ? j.slice(1) : j;
}
/** The titles of a project's clips that are gone from the library (its missing sources). */
function projMissing(p) {
  const src = p?.virtual && Array.isArray(p?.clip?.sources) ? p.clip.sources : [];
  return src.filter((s) => s && s.missing).map((s) => String(s.title || ""));
}
/** The extra line of the question before library clips `ids` are deleted: how many projects use them ("" for none). */
function projUsedNote(ids) {
  const set = new Set(ids || []);
  const n = projList().filter((p) => p && p.virtual && (p.clips || []).some((c) => set.has(c.id))).length;
  if (!n) return "";
  const one = set.size <= 1;
  if (n === 1) return one ? t("Used in 1 project: its pieces there will be empty.") : t("Used in 1 project: their pieces there will be empty.");
  return one ? t("Used in {n} projects: its pieces there will be empty.", { n }) : t("Used in {n} projects: their pieces there will be empty.", { n });
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

/** The names a bulk rename gives `n` projects: the name alone for one, "<name> 1" ... "<name> n" for more. */
function projBulkNames(base, n) {
  base = String(base || "").trim();
  return n === 1 ? [base] : Array.from({ length: n }, (_, i) => `${base} ${i + 1}`);
}
/** A name without the number at its end ("Montage 3" gives "Montage"): what a bulk rename offers. */
function projBaseName(name) {
  name = String(name || "").trim();
  return name.replace(/\s+\d+$/, "").trim() || name;
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
function projRefreshUI(opts) {
  if (typeof S === "undefined" || !S.settings) return;
  if (S.page === "editor" && !S.open && !S.renaming) render();
  else if (typeof renderChrome === "function") renderChrome();
  if (opts && opts.list === false) return projRenderBusy(); // R2 #11: only the busy line, not the whole list
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
  const { n, secs: footage } = projSummary(p), busy = projBusyInfo(p), c = p.clip, lost = projMissing(p);
  // The length shown is the edit's result when the project has a saved edit (0.1.43), else its footage.
  const edit = n ? projEditLen(p) : 0, secs = edit > 0 ? edit : footage;
  const lenTip = edit > 0 && Math.abs(edit - footage) > 0.5 ? t("Length of the edit. Its clips are {len} long in all.", { len: fmtDur(footage) }) : "";
  const bg = c && c.thumb ? "" : `style="background:linear-gradient(135deg,hsl(${hue(p.name)} 40% 22%),hsl(${(hue(p.name) + 50) % 360} 45% 12%))"`;
  const meta = [n === 1 ? t("1 clip") : t("{n} clips", { n }), ...(secs > 0 ? [fmtDur(secs)] : [])].join(" · ");
  return `<article class="card pj-card${projSel.ids.includes(p.id) ? " sel" : ""}" data-pj="${esc(p.id)}" tabindex="0" data-hover title="${esc(p.name)}">
    <div class="thumb" ${bg}>
      <span class="check" data-pjp="pick" title="${esc(t("Select"))}">${ICON.check}</span>
      ${c && c.thumb ? `<img class="thumb-img" src="${esc(fileSrc(c.thumb))}" alt="" draggable="false" decoding="async" onerror="this.remove()" style="${typeof THUMB_IMG_STYLE === "string" ? THUMB_IMG_STYLE : ""}">` : `<div class="ph">${n ? JOIN_ICON : ICON.plus}</div>`}
      ${p.exported ? `<span class="tag pj-tag-exp">${esc(t("Exported"))}</span>` : !n ? `<span class="tag pj-tag-empty">${esc(t("Empty"))}</span>` : ""}
      ${lost.length ? `<span class="tag pj-tag-missing" title="${esc(t("Missing from the library: {titles}", { titles: lost.join(", ") }))}">${esc(lost.length === 1 ? t("1 clip missing") : t("{n} clips missing", { n: lost.length }))}</span>` : ""}
      ${secs > 0 ? `<span class="dur"${lenTip ? ` title="${esc(lenTip)}"` : ""}>${fmtDur(secs)}</span>` : ""}
      ${busy ? `<div class="pj-card-busy"><span>${esc(busy.text)}</span></div><div class="job"><i style="width:${busy.pct}%"></i></div>` : ""}
    </div>
    <h3>${esc(p.name)}</h3>
    <p><span>${esc(meta)}</span><span title="${esc(t("Last edited"))}">${esc(timeAgo(p.modified || p.created || Date.now()))}</span>${p.size > 0 ? `<span>${fmtSize(p.size)}</span>` : ""}</p>
  </article>`;
}

/** The Editor page (app.js render() calls it for S.page "editor"). */
function renderEditorPage() {
  const list = projList();
  projSel.ids = projSel.ids.filter((id) => list.some((p) => p.id === id));
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
/** Rename projects `ids` at once: one name, numbered in the order the page shows them. */
async function projRenameMany(ids) {
  const list = projList().filter((p) => ids.includes(p.id));
  if (!list.length) return;
  if (list.length === 1) return projRename(list[0].id);
  const base = await askText(t("Rename {n} projects. Each gets this name and a number, in the order shown.", { n: list.length }), projBaseName(list[0].name), t("Rename"));
  if (!base) return;
  const names = projBulkNames(base, list.length);
  let done = 0;
  for (let i = 0; i < list.length; i++) {
    if (list[i].name === names[i]) { done++; continue; }
    try { projUpsert(await invoke("rename_project", { id: list[i].id, name: names[i] })); done++; }
    catch (e) { console.warn("Glint: rename_project failed", e); }
  }
  flash(done === list.length ? t("{n} projects renamed", { n: done }) : t("{done} of {n} projects renamed", { done, n: list.length }), done !== list.length);
  projRefreshUI();
}
/** Delete projects `ids` at once, after one question. One that clips are still being added to stays. */
async function projDeleteMany(ids) {
  const list = projList().filter((p) => ids.includes(p.id));
  if (!list.length) return;
  if (list.length === 1) return projDelete(list[0].id);
  const n = list.length;
  if (!(await ask(t("Delete {n} projects? Your edits in them are lost. The clips themselves stay in your library, and so do the clips you exported from them.", { n }), t("Delete {n} projects", { n })))) return;
  const gone = new Set();
  for (const p of list) {
    try {
      if (await invoke("delete_project", { id: p.id })) { gone.add(p.id); projCache.delete(p.id); projSavedText.delete(p.id); }
    } catch (e) { console.warn("Glint: delete_project failed", e); }
  }
  S.projects = projList().filter((x) => !gone.has(x.id));
  projSel.ids = projSel.ids.filter((id) => !gone.has(id));
  projSel.anchor = null;
  if (gone.size) sfx("close");
  flash(gone.size === n ? t("{n} projects deleted", { n }) : t("{done} of {n} projects deleted. Clips are still being added to the others.", { done: gone.size, n }), gone.size !== n);
  projRefreshUI();
}
function projOpenMenu(id, x, y) {
  const p = projById(id);
  if (!p) return closeMenu();
  const n = projSel.ids.length;
  // A right-click on one of several picked projects is about all of them, as in the library.
  $("#menu").innerHTML = projSel.ids.includes(id) && n > 1
    ? `<div class="menu-head">${esc(t("{n} selected", { n }))}</div>` + projMItem("rename-sel", t("Rename…"), ICON.edit) + projMItem("clear-sel", t("Clear selection"), ICON.x) + "<hr>" + projMItem("delete-sel", t("Delete"), ICON.trash, "", "danger")
    : projMItem("open", t("Open"), PICON.play, id) + projMItem("rename", t("Rename…"), ICON.edit, id) + projMItem("pick", projSel.ids.includes(id) ? t("Deselect") : t("Select"), ICON.check, id)
      + "<hr>" + projMItem("delete", t("Delete"), ICON.trash, id, "danger");
  placeMenu(x, y);
}

/* ---------- the Editor page: picking several projects ---------- */

/** Bring the cards and the selection bar in line with projSel without rebuilding the grid. */
function projSyncSel() {
  for (const el of document.querySelectorAll("#view .pj-card")) el.classList.toggle("sel", projSel.ids.includes(el.dataset.pj));
  if (typeof renderSelBar === "function") renderSelBar();
}
/** Drop the Editor page's selection (app.js goPage calls it too). */
function projSelClear() {
  if (!projSel.ids.length) return;
  projSel.ids = [];
  projSel.anchor = null;
  projSyncSel();
}
/** app.js renderSelBar: draw the bar for the Editor page's selection into `el`. True when it is shown. */
function projSelBar(el) {
  if (typeof S === "undefined" || S.page !== "editor" || S.open) return false;
  const picked = projList().filter((p) => projSel.ids.includes(p.id));
  if (!picked.length) return false;
  const secs = picked.reduce((s, p) => s + projSummary(p).secs, 0);
  document.body.classList.add("has-sel");
  el.hidden = false;
  el.innerHTML = `<b>${esc(t("{n} selected", { n: picked.length }))}</b><span class="muted">${secs > 0 ? fmtDur(secs) : ""}</span>
    <button class="btn sm" data-pjp="sel-rename">${ICON.edit}${esc(t("Rename"))}</button>
    <button class="btn sm danger" data-pjp="sel-del">${ICON.trash}${esc(t("Delete"))}</button>
    <button class="icon-btn" data-pjp="sel-clear" aria-label="${esc(t("Clear selection"))}" title="${esc(t("Clear selection (Esc)"))}">${ICON.x}</button>`;
  return true;
}
/** A click that picks: Shift takes the range from the last one picked, otherwise the project is added or taken out. */
function projPageClick(id, e) {
  const r = projSelClick(projSel.ids, projSel.anchor, projList().map((p) => p.id), id, { ctrl: true, shift: !!e.shiftKey && projSel.anchor != null });
  projSel.ids = r.sel;
  projSel.anchor = r.anchor ?? id;
  projSyncSel();
}
function projPageDown(e) {
  projPageDragEnd(false);
  if (e.button !== 0 || typeof S === "undefined" || S.page !== "editor" || S.open) return;
  const view = e.target.closest?.("#view");
  if (!view || !view.querySelector(".pj-grid") || e.target.closest("button, select, input, a, .check")) return;
  if (e.clientX - view.getBoundingClientRect().left >= view.clientWidth) return; // the scroll bar
  projPageDrag = { x: e.clientX, y: e.clientY, cx: e.clientX, cy: e.clientY, view, card: e.target.closest(".pj-card")?.dataset.pj || null, add: e.ctrlKey || e.shiftKey, mode: null };
}
function projPageMove(e) {
  const d = projPageDrag;
  if (!d) return;
  if (e.buttons === 0) return projPageDragEnd(false); // the button was let go where Glint could not see it
  d.cx = e.clientX;
  d.cy = e.clientY;
  if (!d.mode) {
    if (Math.hypot(d.cx - d.x, d.cy - d.y) < 6) return;
    // Card positions are taken once, in the scrolling page's own coordinates.
    const vr = d.view.getBoundingClientRect(), top = d.view.scrollTop - vr.top;
    d.mode = "box";
    d.base = d.add ? projSel.ids.slice() : [];
    d.ox = d.x - vr.left;
    d.oy = d.y + top;
    d.rects = [...d.view.querySelectorAll(".pj-card")].map((el) => { const b = el.getBoundingClientRect(); return { id: el.dataset.pj, l: b.left - vr.left, r: b.right - vr.left, t: b.top + top, b: b.bottom + top }; });
    d.box = document.createElement("div");
    d.box.className = "marquee";
    d.view.append(d.box);
    document.body.classList.add("selecting");
    d.timer = setInterval(() => {
      const b = d.view.getBoundingClientRect();
      const dy = d.cy < b.top + 48 ? -Math.ceil((b.top + 48 - d.cy) / 3) : d.cy > b.bottom - 48 ? Math.ceil((d.cy - b.bottom + 48) / 3) : 0;
      if (dy) { d.view.scrollTop += Math.max(-40, Math.min(40, dy)); projPageBox(d); }
    }, 16);
  }
  projPageBox(d);
}
function projPageBox(d) {
  const vr = d.view.getBoundingClientRect();
  const x = Math.max(0, Math.min(d.view.clientWidth, d.cx - vr.left));
  const y = Math.max(0, Math.min(d.view.scrollHeight, d.cy - vr.top + d.view.scrollTop));
  const box = { l: Math.min(x, d.ox), r: Math.max(x, d.ox), t: Math.min(y, d.oy), b: Math.max(y, d.oy) };
  if (!d.box.isConnected) d.view.append(d.box); // the page was redrawn under the drag
  Object.assign(d.box.style, { left: `${box.l}px`, top: `${box.t}px`, width: `${box.r - box.l}px`, height: `${box.b - box.t}px` });
  projSel.ids = projBoxHits(d.base, d.rects, box);
  projSel.anchor = null;
  projSyncSel();
}
/** End the press on the Editor page. `drop`: a real release; otherwise it is dropped without effect. */
function projPageDragEnd(drop) {
  const d = projPageDrag;
  if (!d) return;
  projPageDrag = null;
  clearInterval(d.timer);
  document.body.classList.remove("selecting");
  if (d.box) {
    const box = d.box;
    box.classList.add("out");
    setTimeout(() => box.remove(), 170);
    projBoxAt = performance.now();
  }
  // A plain click beside the cards drops the selection.
  if (drop && !d.mode && !d.card && !d.add) projSelClear();
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
  if (p.virtual && p.clips && p.clips.length) return void projOpenVirtual(id);
  if (p.clip && p.clip.id === id) return openClip(id);
  projOpenEmpty(p);
}

let projOpening = 0; // the newest projOpenVirtual: an older answer arriving late opens nothing
/** A virtual project (0.1.38): the editor opens what project_open returns (openClip takes the object). */
async function projOpenVirtual(id) {
  const ticket = ++projOpening, was = typeof S !== "undefined" ? S.open : null;
  let c;
  try { c = await invoke("project_open", { id }); } catch (e) { return fail(e); }
  if (ticket !== projOpening || !c || !c.id) return;
  // R2 #14: what was open when it was asked for (nothing, the project itself, or the clip that becomes it) must still be:
  // a slow answer never opens over another clip, nor reopens an editor that was closed meanwhile.
  if (S.open !== was) return;
  const p = projById(id);
  if (p) p.clip = c; // clipById finds the same object
  openClip(c);
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
  if (typeof wsMount === "function") wsMount($(".viewer", m), { title: p.name }); // the editor's full-window layout (workspace.js)
  if (typeof edDropInit === "function") edDropInit(); // files dropped here become the project's first clips (0.1.40)
}

/**
 * Put the clip list beside the editor of project `id` (editor.js openClip calls it for every clip it opens;
 * projOpenEmpty too). 0.1.40: also for a single library clip (`.pj-single`, shown only while editing): adding
 * from it makes a project of [that clip, the added clips] (projFromClip). Nothing for anything else.
 */
function projMountPanel(root, id) {
  const single = !projById(id) && typeof S !== "undefined" && (S.clips || []).some((c) => c && c.id === id && !c.draft);
  if (!root || (!projById(id) && !single) || root.querySelector(":scope > .pj-lib")) return;
  if (projLib.id !== id) { projLib.id = id; projLib.sel = []; projLib.anchor = null; projPvClose(); }
  if (!single) root.classList.add("pj-on");
  const aside = document.createElement("aside");
  aside.className = "pj-lib" + (single ? " pj-single" : "");
  aside.dataset.pjFor = id;
  if (single) aside.dataset.pjSingle = "1";
  aside.innerHTML = `
    <div class="pj-head"><div class="pj-h">${esc(t("Library"))}</div>
      <div class="small pj-hint">${esc(single ? t("Click a clip to preview it. Double-click, press Enter or drag it onto the timeline to add it: this clip then becomes a project.") : t("Click a clip to preview it. Double-click, press Enter or drag clips onto the timeline to add them to the project."))}</div>
      <div class="small pj-missing" hidden></div>
      <div class="small pj-warn-slot"></div></div>
    <div class="pj-pv" hidden></div>
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
  // A clip's preview (0.1.33): rest the pointer on a clip and it plays, without sound, in a small player
  // beside the list. Gone when the pointer leaves the clip, on a press (a drag may start) and on a scroll.
  let pvTimer = 0, pvWait = null, pvFor = null;
  const pvHide = () => { clearTimeout(pvTimer); pvTimer = 0; pvWait = pvFor = null; root.querySelector(".pj-preview")?.remove(); };
  const pvShow = (id2) => {
    const item = aside.querySelector(`.pj-item[data-pjc="${CSS.escape(id2)}"]`), c = S.clips.find((x) => x.id === id2);
    if (!item || !c || !aside.isConnected) return pvHide();
    root.querySelector(".pj-preview")?.remove();
    const el = document.createElement("div");
    el.className = "pj-preview";
    el.innerHTML = `<video muted autoplay loop playsinline disablepictureinpicture${c.thumb ? ` poster="${esc(fileSrc(c.thumb))}"` : ""} src="${esc(typeof edClipUrl === "function" ? edClipUrl(c) : fileSrc(c.path))}"></video>
      <div class="pj-pv-cap"><b>${esc(c.title)}</b><span>${fmtDur(c.duration)}</span></div>`;
    const r = item.getBoundingClientRect(), ar = aside.getBoundingClientRect(), W = Math.min(400, Math.max(240, window.innerWidth - ar.right - 30)), H = Math.round((W * 9) / 16) + 32;
    Object.assign(el.style, { left: `${ar.right + 10}px`, top: `${clamp(r.top + r.height / 2 - H / 2, 8, Math.max(8, window.innerHeight - H - 8))}px`, width: `${W}px` });
    root.appendChild(el);
    pvWait = null;
    pvFor = id2;
  };
  aside.addEventListener("mousemove", (e) => {
    const id2 = e.target.closest(".pj-item")?.dataset.pjc || null;
    if (e.buttons || !id2 || id2 === projPv.id) return pvHide(); // the clicked clip is in the preview above (0.1.40)
    if (id2 === pvFor || id2 === pvWait) return;
    pvHide();
    pvWait = id2;
    pvTimer = setTimeout(() => pvShow(id2), 450);
  });
  aside.addEventListener("mouseleave", pvHide);
  aside.addEventListener("mousedown", pvHide, true);
  aside.querySelector(".pj-list").addEventListener("scroll", pvHide, { passive: true });
  projPvWire(aside.querySelector(".pj-pv"));
  projRenderPanel();
  if (projPv.id && projPv.for === id) projPvShow(projPv.id); // the editor was opened again (a project made of the clip)
}

/* ---------- the clicked clip's preview at the top of the Library list (0.1.40) ---------- */
// One click on a clip shows it here: its own small player (never the editor's), play / pause, a seek bar that
// scrubs, the time and the length; Space over it plays / pauses, Esc or × hides it; double-click on the picture
// or "Add to the timeline" adds it. Its sound is its own, at the editor's volume; in the browser preview
// (no Tauri) it is always muted.
const projPv = { id: null, for: null, over: false, scrub: null, raf: 0 };
const projPvEl = () => projPanel()?.querySelector(".pj-pv") || null;
const projPvVideo = () => projPvEl()?.querySelector("video") || null;
/** "m:ss" of `s` seconds (the preview's clock). */
function projClock(s) {
  s = Math.max(0, Math.floor(+s || 0));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
/** The seek bar's fraction for a pointer at client x `x` over a bar from `left`, `width` px wide (0..1). */
const projPvFrac = (x, left, width) => (width > 0 ? Math.max(0, Math.min(1, (x - left) / width)) : 0);

function projPvShow(id) {
  const el = projPvEl(), c = (S.clips || []).find((x) => x.id === id);
  if (!el || !c) return projPvClose();
  const aside = projPanel();
  projPv.id = id;
  projPv.for = aside?.dataset.pjFor || null;
  const url = typeof edClipUrl === "function" ? edClipUrl(c) : fileSrc(c.path);
  const v0 = el.querySelector("video");
  if (v0 && el.dataset.pvClip === id) { el.hidden = false; return projPvPaint(); }
  el.dataset.pvClip = id;
  el.innerHTML = `
    <div class="pj-pv-head"><b class="pj-pv-title" title="${esc(c.title)}">${esc(c.title)}</b>
      <button type="button" class="e-ibtn pj-pv-x" data-pjv="close" aria-label="${esc(t("Close the preview"))}" title="${esc(t("Close the preview (Esc)"))}">${ICON.x}</button></div>
    <div class="pj-pv-pic" title="${esc(t("Double-click to add it to the timeline"))}"><video muted playsinline disablepictureinpicture preload="auto"${c.thumb ? ` poster="${esc(fileSrc(c.thumb))}"` : ""} src="${esc(url)}"></video></div>
    <div class="pj-pv-bar" role="slider" aria-label="${esc(t("Position in the clip"))}" aria-valuemin="0" aria-valuemax="${Math.round(+c.duration || 0)}"><div class="pj-pv-buf"></div><div class="pj-pv-fill"></div><div class="pj-pv-knob"></div></div>
    <div class="pj-pv-row"><button type="button" class="e-ibtn pj-pv-play" data-pjv="play" aria-label="${esc(t("Play or pause"))}"></button><span class="pj-pv-time"></span></div>
    <button type="button" class="btn sm wide pj-pv-add" data-pjv="add">${ICON.plus}<span>${esc(t("Add to the timeline"))}</span></button>`;
  el.hidden = false;
  const v = el.querySelector("video");
  // Its own sound at the editor's volume; always muted in the browser preview (mock.js, no real Tauri).
  const P = S.player;
  v.muted = typeof window === "undefined" || !window.__TAURI__ || typeof window.__mockEmit === "function" || !!P?.muted;
  v.volume = Math.max(0, Math.min(1, Number.isFinite(P?.vol) ? P.vol : 0.6));
  for (const ev of ["timeupdate", "seeked", "play", "pause", "loadedmetadata", "progress", "ended"]) v.addEventListener(ev, projPvPaint);
  v.addEventListener("play", projPvTick);
  v.play?.()?.catch?.(() => {});
  projPvPaint();
  projPrewarm(id);
}

/* F3 (0.1.41): pre-warm. A clip shown in the click preview is likely added next: its tracks and waveform are
 * prepared quietly (the backend caches them per file), so a double-click right after finds them ready. Once per
 * clip per session, after the preview has stayed up for a moment; failures are ignored (the editor asks again). */
const projWarmed = new Set();
let projWarmTimer = 0;
function projPrewarm(id) {
  clearTimeout(projWarmTimer);
  if (!id || projWarmed.has(id) || typeof invoke !== "function") return;
  projWarmTimer = setTimeout(() => {
    if (projPv.id !== id || projWarmed.has(id)) return;
    projWarmed.add(id);
    invoke("editor_audio_tracks", { id }).then(() => invoke("editor_waveforms", { id })).catch(() => projWarmed.delete(id));
  }, 600);
}
function projPvClose() {
  cancelAnimationFrame?.(projPv.raf);
  projPv.raf = 0;
  projPv.id = null;
  projPv.scrub = null;
  const el = projPvEl();
  if (!el) return;
  el.querySelector("video")?.pause?.();
  el.hidden = true;
  el.innerHTML = "";
  delete el.dataset.pvClip;
}
/** editor.js's key handler (it runs before this file's): Escape hides a shown preview. True if it did. */
function projPvEscape() {
  const el = projPvEl();
  if (!projPv.id || !el || el.hidden || document.activeElement?.matches?.("input, textarea, select")) return false;
  projPvClose();
  return true;
}
function projPvToggle() {
  const v = projPvVideo();
  if (!v) return;
  if (v.paused || v.ended) v.play?.()?.catch?.(() => {}); else v.pause();
}
function projPvTick() {
  cancelAnimationFrame(projPv.raf);
  const step = () => {
    const v = projPvVideo();
    if (!v || v.paused) { projPv.raf = 0; return; }
    projPvPaint();
    projPv.raf = requestAnimationFrame(step);
  };
  projPv.raf = requestAnimationFrame(step);
}
function projPvPaint() {
  const el = projPvEl(), v = projPvVideo();
  if (!el || !v) return;
  const c = (S.clips || []).find((x) => x.id === projPv.id);
  const D = Number.isFinite(v.duration) && v.duration > 0 ? v.duration : +c?.duration || 0;
  const at = projPv.scrub ? projPv.scrub.at : v.currentTime || 0;
  const f = D > 0 ? Math.min(1, at / D) : 0;
  el.querySelector(".pj-pv-fill").style.width = `${f * 100}%`;
  el.querySelector(".pj-pv-knob").style.left = `${f * 100}%`;
  let buf = 0;
  try { if (v.buffered.length && D > 0) buf = v.buffered.end(v.buffered.length - 1) / D; } catch {}
  el.querySelector(".pj-pv-buf").style.width = `${Math.min(1, buf) * 100}%`;
  el.querySelector(".pj-pv-bar").setAttribute("aria-valuenow", String(Math.round(at)));
  const txt = `${projClock(at)} / ${projClock(D)}`;
  const time = el.querySelector(".pj-pv-time");
  if (time.textContent !== txt) time.textContent = txt;
  const play = el.querySelector(".pj-pv-play"), icon = v.paused ? PICON.play : PICON.pause;
  if (play.dataset.icon !== (v.paused ? "play" : "pause")) { play.innerHTML = icon; play.dataset.icon = v.paused ? "play" : "pause"; }
}
/** The preview's own events: buttons, scrubbing, double-click to add, the pointer over it (for Space). */
function projPvWire(el) {
  if (!el) return;
  el.addEventListener("mouseenter", () => { projPv.over = true; });
  el.addEventListener("mouseleave", () => { projPv.over = false; });
  el.addEventListener("click", (e) => {
    const b = e.target.closest("[data-pjv]");
    if (b) {
      e.stopPropagation();
      if (b.dataset.pjv === "close") return projPvClose();
      if (b.dataset.pjv === "play") return projPvToggle();
      if (b.dataset.pjv === "add") return projPvAdd();
    }
    if (e.target.closest(".pj-pv-pic")) projPvToggle();
  });
  el.addEventListener("dblclick", (e) => { if (e.target.closest(".pj-pv-pic")) { e.preventDefault(); projPvAdd(); } });
  el.addEventListener("pointerdown", (e) => {
    const bar = e.target.closest(".pj-pv-bar"), v = projPvVideo();
    if (!bar || !v || e.button !== 0) return;
    e.preventDefault();
    try { bar.setPointerCapture(e.pointerId); } catch {}
    projPv.scrub = { bar, play: !v.paused, at: v.currentTime || 0 };
    v.pause();
    projPvScrub(e.clientX);
  });
  el.addEventListener("pointermove", (e) => { if (projPv.scrub) projPvScrub(e.clientX); });
  const end = () => {
    const s = projPv.scrub, v = projPvVideo();
    if (!s) return;
    projPv.scrub = null;
    if (v && s.play) v.play?.()?.catch?.(() => {});
    projPvPaint();
  };
  el.addEventListener("pointerup", end);
  el.addEventListener("pointercancel", end);
  el.addEventListener("lostpointercapture", end);
}
/** Scrub to the bar's place under client x `x`: the picture follows the pointer. */
function projPvScrub(x) {
  const s = projPv.scrub, v = projPvVideo();
  if (!s || !v) return;
  const r = s.bar.getBoundingClientRect();
  const D = Number.isFinite(v.duration) && v.duration > 0 ? v.duration : 0;
  if (!D) return;
  s.at = projPvFrac(x, r.left, r.width) * D;
  if (typeof v.fastSeek === "function") v.fastSeek(s.at); else v.currentTime = s.at;
  projPvPaint();
}
/** "Add to the timeline" (or a double-click on the preview's picture): the previewed clip, as a double-click in the list. */
function projPvAdd() {
  const id = projPv.id, aside = projPanel();
  if (!id || !aside) return;
  projAddTo(aside.dataset.pjFor, [id]);
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
  // The project's clips that are gone from the library (0.1.38): their pieces show empty in the editor.
  const lost = projMissing(projById(aside.dataset.pjFor)), miss = $(".pj-missing", aside);
  if (miss) {
    const txt = lost.length ? t("Missing from the library: {titles}", { titles: lost.join(", ") }) : "";
    miss.hidden = !lost.length;
    if (miss.textContent !== txt) miss.textContent = txt;
  }
  // R2 #3: a saved edit that could not be opened (or a backup of one): what the user can do about it.
  const warn = $(".pj-warn-slot", aside), wh = projById(aside.dataset.pjFor) ? projWarnHtml(aside.dataset.pjFor) : "";
  if (warn && warn.innerHTML !== wh) warn.innerHTML = wh;
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
const projListHtml = new WeakMap(); // the list element -> the HTML it was last given (R2 #11)
/** The clips shown in the list now. */
const projShown = () => projLibFilter(typeof S !== "undefined" ? S.clips : [], projLib.q, projLib.game);

function projRenderList() {
  const aside = projPanel();
  if (!aside) return;
  const list = $(".pj-list", aside), shown = projShown();
  // A single clip open in the editor (0.1.40) counts as in it.
  const have = new Set((projById(aside.dataset.pjFor)?.clips || [{ id: aside.dataset.pjSingle ? aside.dataset.pjFor : null }]).map((c) => c.id).filter(Boolean));
  // The selection keeps only clips that still exist (a search does not drop it).
  const all = new Set((S.clips || []).map((c) => c.id));
  projLib.sel = projLib.sel.filter((id) => all.has(id));
  const html = shown.length ? shown.map((c) => projItemHtml(c, have.has(c.id))).join("")
    : `<div class="pj-none small">${esc((S.clips || []).length ? t("No clips match the search.") : t("No clips in your library yet."))}</div>`;
  // R2 #11: the same list is not built again (every thumbnail would be laid out anew).
  if (projListHtml.get(list) !== html || !list.firstChild) {
    const top = list.scrollTop;
    list.innerHTML = html;
    list.scrollTop = top;
    projListHtml.set(list, html);
  }
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
  const n = projLib.sel.length, single = !!projPanel()?.dataset.pjSingle; // a single clip (0.1.40): it becomes a project
  btn.disabled = !n;
  btn.innerHTML = `${ICON.plus}<span>${esc(single ? (n ? t("Add {clips} to the timeline", { clips: plural(n) }) : t("Add to the timeline")) : n ? t("Add {clips} to the project", { clips: plural(n) }) : t("Add to the project"))}</span>`;
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
    + projMItem("show", t("Show in library"), ICON.library, "", "", !n)
    // One clip picked: "Put on the picture" of the open edit, as a clip in the picture (0.1.35, media.js).
    + (n === 1 && typeof medLibMenuItem === "function" ? medLibMenuItem(projLib.sel[0]) : "");
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

/** Add the selected clips (in the order picked) to the open project (or make one of the open clip). */
function projAddSel() {
  const id = projPanel()?.dataset.pjFor;
  if (id) projAddTo(id, projLib.sel.slice());
}
/**
 * Add library clips `ids` to what is open as `id`: a project (projAdd), or a single library clip, which then
 * becomes a project (projFromClip). `at`: the V1 time to place them at (a drop), undefined = at the end.
 */
function projAddTo(id, ids, at) {
  if (projById(id)) return projAdd(id, ids, at);
  return projFromClip(id, ids, at);
}

/* ---------- files dropped into the editor (0.1.40) ----------
 * B's edDropInit (editor.js) hands every VIDEO file dropped anywhere in the open editor except the player to
 * projDropClips(paths, at): `at` = the V1 time it was dropped at (timeline seconds), undefined = anywhere else
 * (the end). Also with no player (an empty project's frame: S.open is the project id). Each file is imported
 * as the Import page imports it (import_files: copy or reference as the Import dialog's choice IMP.copy is,
 * into the library folder of the open clip or project); one already in the library is used as it is. Then the
 * clips go on V1: into a project (project_add_clips + P.addSources), or the open single clip becomes a project
 * (projFromClip). The placing at `at` is one undo step (P.edit "order", projPlaceNew: 0.1.36's free placement
 * with emPlacePart, one piece after the other). */
const PROJ_VIDEO_EXT = /\.(mp4|mkv|mov|webm|avi|m4v|flv|ts|wmv)$/i; // as import.rs VIDEO_EXTS
/** The file name of a path, for flashes. */
const projBaseOf = (p) => String(p || "").split(/[\\/]/).pop() || String(p || "");
/** The library folder dropped files go in: the open clip's, or the project's first clip's ("" = none). */
function projDropFolder(id) {
  const lib = (cid) => (S.clips || []).find((c) => c.id === cid) || null;
  const p = projById(id);
  if (!p) return lib(id)?.folder || "";
  for (const c of p.clips || []) { const k = lib(c.id); if (k) return k.folder || ""; }
  return "";
}
/** A library clip that came back from the backend: into S.clips (the list beside the editor shows it). */
function projKeepClip(clip) {
  if (!clip || !clip.id || clip.draft || typeof S === "undefined" || !Array.isArray(S.clips)) return;
  const i = S.clips.findIndex((c) => c.id === clip.id);
  if (i >= 0) S.clips[i] = clip; else S.clips.push(clip);
}
async function projDropClips(paths, at) {
  if (typeof S === "undefined" || !S.open) return;
  const target = S.open;
  const list = (Array.isArray(paths) ? paths : []).filter((p) => typeof p === "string" && p);
  const vids = list.filter((p) => PROJ_VIDEO_EXT.test(p));
  for (const p of list) if (!PROJ_VIDEO_EXT.test(p)) flash(t("{name} is not a video, a picture or audio, so it was not added.", { name: projBaseOf(p) }), true);
  if (!vids.length) return;
  let res;
  try {
    res = await invoke("import_files", { paths: vids, copy: typeof IMP !== "undefined" && !!IMP.copy, folder: projDropFolder(target) });
  } catch (e) { return fail(e); }
  const ids = [];
  for (const d of Array.isArray(res) ? res : []) {
    if (d && d.clip && d.clip.id) { projKeepClip(d.clip); ids.push(d.clip.id); }
    else if (d) flash(t("Could not add {name}: {error}", { name: projBaseOf(d.path), error: t(String(d.error || "")) }), true);
  }
  projRenderPanel();
  if (!ids.length || S.open !== target) return;
  return projAddTo(target, ids, Number.isFinite(at) ? at : undefined);
}

/**
 * Pure (editmodel.js): put the new clips' pieces, appended at the end of V1, at timeline second `at`, one after
 * the other in the order of `srcs` (their project-time starts = the `starts` project_add_clips answers). 0.1.36's
 * free placement (emPlacePart): onto a piece beside it (the nearer side), on empty space exactly there; at or past
 * the end they stay where they are. True if the layout changed (the caller makes it one undo step).
 */
function projPlaceNew(m, srcs, at) {
  if (!m || !Array.isArray(m.parts) || !Array.isArray(srcs) || !srcs.length || !Number.isFinite(at)) return false;
  const near = (a, b) => Math.abs(a - b) < 1e-6;
  const find = (s) => emParts(m).findIndex((p) => !p.gap && near(p.src, s));
  const first = emParts(m)[find(+srcs[0])];
  if (!first || at >= first.at - 1e-6) return false; // dropped at or past the end: they are there already
  let pos = Math.max(0, at), moved = false;
  for (const s of srcs) {
    const i = find(+s);
    if (i < 0) continue;
    if (emPlacePart(m, i, pos)) moved = true;
    const q = emParts(m)[find(+s)];
    if (q) pos = q.end;
  }
  return moved;
}
/** Place the new pieces of the open editor's project (`srcs`) at `at` as one undo step. */
function projPlaceOpen(P, srcs, at) {
  if (!P || !Number.isFinite(at) || !Array.isArray(srcs) || !srcs.length || typeof P.edit !== "function") return false;
  try { return P.edit("order", (m) => projPlaceNew(m, srcs, at)); } catch (e) { console.warn("Glint: the dropped clips stay at the end", e); return false; }
}

/** "<first title> + <n> more", as join.rs joined_title names an "Edit together" project. */
function projJoinedTitle(first, more) {
  const head = [...String(first || "").trim()].slice(0, 48).join("").trimEnd();
  return `${head} + ${more} more`;
}
/**
 * The single library clip `cid` open in the editor gets clips `ids`: it becomes a project at once, a virtual one of
 * [this clip, the added clips], named as "Edit together" names one. The edit made so far is carried over unchanged
 * (the clip is source 1 at project time 0, so none of its times move; projRestore appends the new clips), it opens
 * in the same editor at the same playhead, the new clips go to `at` if given, and a flash names the project. The
 * clip in the library stays as it is.
 */
async function projFromClip(cid, ids, at) {
  const byId = new Map((S.clips || []).map((c) => [c.id, c]));
  const c = byId.get(cid);
  if (!c || c.draft) return;
  const add = [...new Set(ids)].map((x) => byId.get(x)).filter((x) => x && !x.draft);
  const why = projAddRefusal({ clips: [c] }, add);
  if (why) return flash(why, true);
  const P = S.player;
  let state = null, now = null;
  if (P && P.clipMeta && P.clipMeta.id === cid && P.model) {
    try { state = emStateSave(P.model, [], true); now = typeof P.time === "function" ? P.time() : null; }
    catch (e) { console.warn("Glint: could not hand the clip's edit to the new project", e); state = null; }
  }
  let p = null, res;
  try {
    p = await invoke("create_project", { name: projJoinedTitle(c.title, add.length) });
    if (!p || !p.id) throw new Error("create_project returned nothing");
    await invoke("project_add_clips", { id: p.id, ids: [c.id] });
    res = await invoke("project_add_clips", { id: p.id, ids: add.map((x) => x.id) });
    if (!res || !res.project) throw new Error("project_add_clips did not answer as for a project of references");
  } catch (e) {
    if (p && p.id) invoke("delete_project", { id: p.id }).catch(() => {});
    return flash(joinError(e), true);
  }
  const proj = res.project;
  projUpsert(proj);
  if (S.open !== cid) return projRefreshUI(); // closed meanwhile: the project is made, nothing opens
  if (state) projCarry = { id: proj.id, state };
  projLib.id = proj.id;
  projLib.sel = [];
  projLib.anchor = null;
  if (projPv.id) projPv.for = proj.id; // the preview stays in the new editor's list
  sfx("success");
  await projOpenVirtual(proj.id);
  flash(t("Made a project: {name}", { name: proj.name }));
  projRefreshUI();
  const N = S.player;
  if (!N || S.open !== proj.id) return;
  const m = await N.whenModel?.();
  if (!m || S.player !== N || !N.alive?.()) return;
  if (Number.isFinite(at)) projPlaceOpen(N, res.starts, at);
  if (Number.isFinite(now)) N.seek(now, false, true);
}

async function projAdd(pid, ids, at) {
  const p = projById(pid);
  if (!p) return;
  const byId = new Map((S.clips || []).map((c) => [c.id, c]));
  const clips = [...new Set(ids)].map((x) => byId.get(x)).filter((c) => c && !c.draft);
  const why = projAddRefusal(p, clips);
  if (why) return flash(why, true);
  try {
    const job = await invoke("project_add_clips", { id: pid, ids: clips.map((c) => c.id) });
    // 0.1.38: a virtual project took them at once ({ project, added, starts, titles }; a job has a kind).
    if (job && job.project && !job.kind) {
      projLib.sel = [];
      projLib.anchor = null;
      await projAdded(pid, job, at);
      return projRefreshUI();
    }
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

/**
 * Clips added to a virtual project at once (0.1.38; `res` = project_add_clips' answer). The open editor takes
 * the new sources without reloading (P.addSources, editor.js); the empty frame, or an editor that could not
 * take them, opens the project again with its edit handed over (projReload).
 */
async function projAdded(pid, res, at) {
  const p = res.project;
  projUpsert(p);
  const n = Array.isArray(res.added) ? res.added.length : 0;
  sfx("success");
  if (S.open === pid) {
    const P = S.player;
    let took = false;
    if (P && P.clipMeta && P.clipMeta.id === pid && P.clipMeta.virtual && typeof P.addSources === "function" && p.clip) {
      try { took = !!(await P.addSources(p.clip)); } catch (e) { console.warn("Glint: the editor could not take the new clips; the project opens again", e); took = false; }
    }
    if (took && Number.isFinite(at) && S.player === P) projPlaceOpen(P, res.starts, at); // a drop on V1 (0.1.40)
    if (!took && S.open === pid) projReload(projById(pid) || p);
    if (n) flash(t("Added {clips} to the project", { clips: plural(n) }));
  } else if (n) flash(t('Added {clips} to "{name}"', { clips: plural(n), name: p.name }));
}

/** The open project's draft was rebuilt (or a virtual one grew): open it again, handing over the edit (with its undo), and go to the new clips. */
function projReload(p) {
  if (!p.clip || p.clip.id !== p.id) return;
  const P = S.player;
  let at = null;
  if (P && P.draft && P.model && P.clipMeta && P.clipMeta.id === p.id) {
    try {
      projCarry = { id: p.id, state: emStateSave(P.model, projJoinsOf(P.clipMeta), true) };
      at = P.model.duration; // where the new clips start
    } catch (e) { console.warn("Glint: could not hand the edit over; it comes back from the last save", e); projCarry = null; }
  }
  openClip(p.id);
  const N = S.player;
  if (N && at != null) N.whenModel?.().then((m) => { if (m && S.player === N && N.alive()) N.seek(Math.min(at, m.duration), false, true); });
}

/* ---------- an export that replaces clips of the project ---------- */

/**
 * editor.js, before a project's export with "Replace clips of the project" on: ask which of the
 * project's clips the export replaces. Resolves to the ids picked (they go to the Recycle Bin after
 * the export; [] = none, or none of the clips is in the library any more), or null when cancelled.
 */
function projAskReplace(pid) {
  const byId = new Map((S.clips || []).map((c) => [c.id, c]));
  const seen = new Set();
  const clips = (projById(pid)?.clips || []).map((c) => byId.get(c.id)).filter((c) => c && !c.draft && !seen.has(c.id) && seen.add(c.id));
  if (!clips.length) return Promise.resolve([]);
  return new Promise((resolve) => {
    const el = $("#confirm");
    el.innerHTML = `<div class="confirm-box pj-rep">
      <p>${esc(t("Which clips does this export replace? The ticked clips go to the Recycle Bin. The others stay in your library."))}</p>
      <div class="pj-rep-list">${clips.map((c) => `<label class="pj-rep-item"><input type="checkbox" checked data-rep="${esc(c.id)}"><span class="pj-rep-title" title="${esc(c.title)}">${esc(c.title)}</span><span class="pj-rep-dur">${fmtDur(c.duration)}</span></label>`).join("")}</div>
      <div class="pj-rep-tools"><button class="btn sm" data-rep-all="1">${esc(t("Select all"))}</button><button class="btn sm" data-rep-all="0">${esc(t("Clear selection"))}</button></div>
      <div class="confirm-row"><button class="btn" data-c="no">${esc(t("Cancel"))}</button><button class="btn" data-c="yes"></button></div></div>`;
    el.hidden = false;
    const picked = () => [...el.querySelectorAll("[data-rep]")].filter((i) => i.checked).map((i) => i.dataset.rep);
    const paint = () => {
      const n = picked().length, b = el.querySelector('[data-c="yes"]');
      b.textContent = n ? t("Export and replace {clips}", { clips: plural(n) }) : t("Export without replacing");
      b.classList.toggle("solid-danger", n > 0);
      b.classList.toggle("primary", !n);
    };
    const done = (v) => { el.hidden = true; el.innerHTML = ""; el.onclick = null; el.onchange = null; askKey = null; resolve(v); };
    // "pass" lets the key through (Space ticks a box).
    askKey = (e) => { if (e.code === "Enter" || e.code === "NumpadEnter") done(picked()); else if (e.code === "Escape") done(null); else return "pass"; };
    el.onchange = paint;
    el.onclick = (e) => {
      const all = e.target.closest("[data-rep-all]");
      if (all) { for (const i of el.querySelectorAll("[data-rep]")) i.checked = all.dataset.repAll === "1"; return paint(); }
      const b = e.target.closest("[data-c]");
      if (b) done(b.dataset.c === "yes" ? picked() : null);
      else if (e.target === el) done(null);
    };
    paint();
  });
}

/* ---------- the running export: what it does, Cancel, why it failed (0.1.41) ----------
 * editor.js keeps a running export in S.exports[id] = { pct, stage, stagePct, cancelling } (edListen reads the
 * "export-progress" event: { id, pct, stage?: "join" | "export", stage_pct? }) and, after a failed or stalled
 * export, its message in S.exportErr[id] (cleared when the next export starts). The Export section draws
 * projExpProgHtml / projExpFailHtml; their buttons (data-pje) are handled here: Cancel calls A's cancel_export,
 * "Show the log" opens the bug report dialog, which shows the end of glint.log. */

/** The progress line's text for export `job`: "Joining clips… 12%" while the clips of a project are joined, "Cancelling…", else "Exporting… 48%". */
function projExpText(job) {
  if (!job) return "";
  if (job.cancelling) return t("Cancelling…");
  const pct = (x) => Math.max(0, Math.min(100, Math.floor(+x || 0)));
  if (job.stage === "join") return t("Joining clips… {pct}%", { pct: pct(job.stagePct != null && Number.isFinite(+job.stagePct) ? job.stagePct : job.pct) });
  return t("Exporting… {pct}%", { pct: pct(job.pct) });
}
/** The running export's bar, its text and its Cancel button. */
function projExpProgHtml(job, id) {
  const w = Math.max(0, Math.min(100, +job?.pct || 0));
  return `<div class="e-prog"><div class="e-prog-head"><div class="small e-prog-txt">${esc(projExpText(job))}</div>
    <button type="button" class="btn sm e-cancel" data-pje="cancel" data-arg="${esc(id)}"${job?.cancelling ? " disabled" : ""}>${esc(t("Cancel"))}</button></div>
    <div class="progress"><i style="width:${w}%"></i></div></div>`;
}
/** The message of a failed or stalled export of `id` (A's text), with "Show the log"; "" when there is none. */
function projExpFailHtml(id) {
  const err = typeof S !== "undefined" && S.exportErr ? S.exportErr[id] : null;
  if (!err) return "";
  return `<div class="e-fail" role="alert"><div class="e-fail-head"><b>${esc(t("The export did not finish"))}</b>
      <button type="button" class="e-ibtn e-fail-x" data-pje="dismiss" data-arg="${esc(id)}" aria-label="${esc(t("Dismiss"))}" title="${esc(t("Dismiss"))}">${ICON.x}</button></div>
    <div class="small e-fail-msg">${esc(err.message)}</div>
    <button type="button" class="btn sm e-fail-log" data-pje="log">${esc(t("Show the log"))}</button></div>`;
}
/** The text of a failed export's error, in the user's language where it is a known one. */
function projExpMessage(e) {
  const msg = String(e?.message ?? e ?? "").trim();
  return (typeof joinError === "function" ? joinError(msg) : t(msg)) || t("The export failed.");
}
/** edExport, when export_clip / export_project rejected: a cancelled export flashes; any other failure is kept for the Export section. */
function projExpFailed(id, e, job) {
  if (job && job.cancelling) { flash(t("Export cancelled")); return; }
  if (!S.exportErr) S.exportErr = {};
  S.exportErr[id] = { message: projExpMessage(e), at: Date.now() };
  fail(e);
}
/** The Cancel button: ask the backend to end the running export of `id` (A's cancel_export). */
async function projExpCancel(id) {
  const job = S.exports?.[id];
  if (!job || job.cancelling) return;
  job.cancelling = true;
  if (S.open === id) S.player?.ed?.renderExport?.();
  try {
    const ok = await invoke("cancel_export", { id });
    if (ok === false && S.exports?.[id] === job) { job.cancelling = false; flash(t("The export could not be cancelled"), true); }
  } catch (e) {
    console.warn("Glint: cancel_export failed", e);
    if (S.exports?.[id] === job) job.cancelling = false;
    fail(e);
  }
  if (S.open === id) S.player?.ed?.renderExport?.();
}
/** "Show the log": the bug report dialog (bugreport.js), the only place the app shows glint.log, opened at the log's end. */
async function projExpShowLog() {
  if (typeof bugOpen !== "function") return;
  await bugOpen();
  const dlg = document.querySelector(".bug-dlg");
  const sent = dlg?.querySelector("details.bug-sent");
  if (sent) sent.open = true;
  const log = dlg?.querySelector(".bug-log");
  if (log) { log.scrollTop = log.scrollHeight; log.scrollIntoView?.({ block: "nearest" }); }
}
function projExpDismiss(id) {
  if (S.exportErr) delete S.exportErr[id];
  if (S.open === id) S.player?.ed?.renderExport?.();
}

/* ---------- the saved edit ---------- */

/** The open project's edit as the text that is saved. */
const projStateText = (P) => JSON.stringify(emStateSave(P.model, projJoinsOf(P.clipMeta)));

/**
 * editor.js, making a project's model `m` (a fresh emNew of draft `c`): put the saved edit on it.
 * The newest state of this session wins over the backend's copy. When the draft has grown since
 * (clips added), the new clips are appended as parts. Returns false when nothing usable was
 * saved (the project then opens clean), true when restored, "added" when clips were appended too.
 */
async function projRestore(c, m) {
  const id = c.id;
  let text = null;
  // "Try again" after a failed load (R2 #3): read the backend's copy again, not this session's.
  if (projLoadFailed.get(id)?.retry) { projCache.delete(id); projSavedText.delete(id); }
  if (projCarry && projCarry.id === id) { text = projCarry.state; projCarry = null; }
  else if (projCache.has(id)) text = projCache.get(id);
  else {
    try { text = await invoke("project_state", { id }); }
    catch (e) { return projFailLoad(id, null, "read", e); }
  }
  if (text == null || text === "") { projLoadFailed.delete(id); return false; } // nothing saved yet: it opens clean, and saving is fine
  let s = text;
  if (typeof s === "string") { try { s = JSON.parse(s); } catch { return projFailLoad(id, text, "unreadable"); } }
  // A virtual project's joins start with 0 (one per source): projJoinsOf drops it, so names[k + 1 + j] below fits both kinds.
  const joins = projJoinsOf(c);
  if (!s || !projJoinsPrefix(s.joins, joins)) return projFailLoad(id, text, "fit");
  const end = m.duration; // the draft's length now
  const k = s.joins.length, fresh = joins.slice(k);
  // R1 (0.1.41): a state saved by 0.1.37-0.1.40 may hold the ROW's length (gaps included) as `duration`; the model
  // repairs it (emStateSrcLen). The same repair here, against the footage before the clips added since (where the
  // first new clip starts), so the checks below and emAddParts see the true source length.
  if (typeof emStateSrcLen === "function" && typeof emStateOk === "function" && emStateOk(s)) {
    const D = emStateSrcLen(s, fresh.length ? fresh[0] : end);
    if (D !== s.duration) s = { ...s, duration: D };
  }
  // New clips start where the old draft ended. One that starts inside it means the old seconds moved
  // (not what projects.rs does): the edit would land on other pictures, so it opens clean instead.
  if (fresh.length && !(fresh[0] >= +s.duration - 0.25)) return projFailLoad(id, text, "fit");
  if (!emStateLoad(m, s)) return projFailLoad(id, text, "unreadable");
  projLoadFailed.delete(id);
  if (!fresh.length) return true;
  const names = Array.isArray(c.join_names) ? c.join_names : [];
  const name = (i) => (typeof names[i] === "string" && names[i].trim()) || t("Clip {n}", { n: i + 1 });
  return emAddParts(m, fresh, fresh.map((_, j) => name(k + 1 + j)), end, name(0)) ? "added" : true;
}

/* ---------- a saved edit that could not be opened (R2 #3, 0.1.41) ----------
 * The project then opens clean, but the clean edit is NEVER saved over the one on disk by itself: projFlush stands
 * aside until the user picks, in the Library panel's notice: "Try again" (reads it again and reopens), or "Keep
 * this edit instead" (from then on this edit is saved; the old text, when it could be read, is kept as a backup in
 * localStorage, which "Bring back the earlier edit" restores). */
const projLoadFailed = new Map(); // project id -> { text (the saved text, or null when it could not be read), why, keep, retry }
const PROJ_BACKUP_KEY = "glint.projBackup.";
/** projRestore found a saved edit it cannot use: remember it, so nothing is saved over it. Returns false (opens clean). */
function projFailLoad(id, text, why, err) {
  const raw = typeof text === "string" ? text : text != null ? (() => { try { return JSON.stringify(text); } catch { return null; } })() : null;
  console.warn(`Glint: the project's saved edit could not be opened (${why}); it opens clean and nothing is saved over it`, err || "");
  projLoadFailed.set(id, { text: raw, why, keep: false, retry: false });
  return false;
}
/** True while project `id`'s saved edit failed to load and the user has not chosen to keep the clean edit. */
const projSaveBlocked = (id) => { const f = projLoadFailed.get(id); return !!f && !f.keep; };
const projBackupOf = (id) => { try { return localStorage.getItem(PROJ_BACKUP_KEY + id); } catch { return null; } };
/** The notice in the Library panel for project `id`, or "". */
function projWarnHtml(id) {
  const f = projLoadFailed.get(id);
  if (f && !f.keep) {
    const why = f.why === "read" ? t("The saved edit of this project could not be read, so it opened without it.")
      : f.why === "fit" ? t("The saved edit no longer fits this project's clips, so it opened without it.")
      : t("The saved edit of this project is damaged, so it opened without it.");
    return `<div class="pj-warn" role="alert"><div>${esc(why)} ${esc(t("Nothing is saved over it until you choose."))}</div>
      <div class="pj-warn-row"><button type="button" class="btn sm" data-pjw="retry">${esc(t("Try again"))}</button>
      <button type="button" class="btn sm" data-pjw="keep">${esc(t("Keep this edit instead"))}</button></div></div>`;
  }
  if (projBackupOf(id)) {
    return `<div class="pj-warn pj-warn-soft"><div>${esc(t("An earlier edit of this project is kept as a backup."))}</div>
      <div class="pj-warn-row"><button type="button" class="btn sm" data-pjw="restore">${esc(t("Bring back the earlier edit"))}</button></div></div>`;
  }
  return "";
}
/** "Try again": read the saved edit again and reopen the project with it. */
function projRetryLoad(id) {
  const f = projLoadFailed.get(id);
  if (f) f.retry = true;
  const p = projById(id);
  if (!p) return;
  if (p.virtual) projOpenVirtual(id); else openClip(id);
}
/** "Keep this edit instead": from now on the open edit is saved; the old one is kept as a backup when it could be read. */
async function projKeepClean(id) {
  const f = projLoadFailed.get(id);
  if (!f) return;
  if (f.text == null) {
    if (!(await ask(t("The saved edit could not be read, so it cannot be kept as a backup. Save this edit over it?"), t("Save over it")))) return;
  } else {
    try { localStorage.setItem(PROJ_BACKUP_KEY + id, f.text); }
    catch { if (!(await ask(t("The saved edit could not be kept as a backup. Save this edit over it?"), t("Save over it")))) return; }
  }
  f.keep = true;
  projSavedText.delete(id);
  const P = S.player;
  if (P && P.clipMeta?.id === id) projFlush(P);
  flash(f.text != null ? t("This edit is saved from now on. The earlier one is kept as a backup.") : t("This edit is saved from now on."));
  projRenderPanel();
}
/** "Bring back the earlier edit": the backup becomes the saved edit (the current one becomes the backup) and the project reopens with it. */
async function projBringBack(id) {
  const old = projBackupOf(id);
  if (!old) return;
  const P = S.player;
  const cur = P && P.clipMeta?.id === id && P.model ? (() => { try { return projStateText(P); } catch { return null; } })() : projCache.get(id) ?? null;
  try {
    await invoke("project_save_state", { id, state: old });
    try { if (cur) localStorage.setItem(PROJ_BACKUP_KEY + id, cur); else localStorage.removeItem(PROJ_BACKUP_KEY + id); } catch { /* the swap is best effort */ }
  } catch (e) { return fail(e); }
  projLoadFailed.set(id, { text: null, why: "", keep: false, retry: true }); // read it back from disk; nothing is saved meanwhile
  projCache.delete(id);
  projSavedText.delete(id);
  const p = projById(id);
  if (p && S.open === id) { if (p.virtual) projOpenVirtual(id); else openClip(id); }
}

/** editor.js: the project's model is ready. An edit opened as it was saved is not saved again; one that grew is. */
function projModelReady(P, restored) {
  if (!P || !P.model || !P.clipMeta) return;
  const id = P.clipMeta.id;
  // R2 #3: a saved edit that could not be opened: the clean model is neither cached nor saved; the panel says so.
  if (projSaveBlocked(id)) {
    flash(t("The saved edit of this project could not be opened. Nothing is saved over it until you choose."), true);
    projRenderPanel();
    return;
  }
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
  if (projSaveBlocked(id)) return null; // R2 #3: never over a saved edit that could not be opened (projWarnHtml)
  let text;
  try { text = projStateText(P); } catch (e) { console.warn("Glint: could not read the project's edit", e); return null; }
  projCache.set(id, text);
  if (projSavedText.get(id) === text) return null;
  projSavedText.set(id, text);
  return Promise.resolve().then(() => invoke("project_save_state", { id, state: text })).then((p) => {
    // R2 #11: a save changes the card and the badge, not the Library list beside the editor: it is not rebuilt.
    if (p && p.id) { projUpsert(p); projRefreshUI({ list: false }); }
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
      d.box.className = "marquee pj-marquee";
      document.body.append(d.box);
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
  // The box is drawn on the window, where the pointer is, also outside the narrow list (as on the Library
  // page: inside the list it was cut off at the list's right edge). Only the list's top and bottom end it,
  // because the list scrolls under it. What it picks is still worked out in the list's own coordinates above.
  const lim = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
  const ax = d.ox + r.left, ay = lim(d.oy - d.list.scrollTop + r.top, r.top, r.bottom); // where the drag began, on screen
  const sx = lim(d.cx, 0, window.innerWidth), sy = lim(d.cy, r.top, r.bottom);
  if (!d.box.isConnected) document.body.append(d.box); // the page was redrawn under the drag
  Object.assign(d.box.style, { left: `${Math.min(ax, sx)}px`, top: `${Math.min(ay, sy)}px`, width: `${Math.abs(sx - ax)}px`, height: `${Math.abs(sy - ay)}px` });
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
      if (projLib.sel.includes(d.item)) projPvShow(d.item); // one click previews it at the top of the list (0.1.40)
    } else if (!d.ctrl && !d.shift) { projLib.sel = []; projLib.anchor = null; }
    return projPaintSel();
  }
  if (d.mode === "add" && d.target) projAddSel();
}

/** Keys while the clip list (or its search box) has the focus. True if the key was used. */
function projKey(e) {
  const aside = projPanel();
  const at = document.activeElement;
  // The clicked clip's preview (0.1.40): Space with the pointer over it plays / pauses it; Esc hides it.
  if (aside && projPv.id && projPvEl() && !projPvEl().hidden && !e.ctrlKey && !e.altKey && !e.metaKey && !at?.matches?.("input, textarea, select")) {
    if (e.code === "Space" && projPv.over) { if (!e.repeat) projPvToggle(); return true; }
    if (e.code === "Escape") { projPvClose(); return true; }
  }
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
    if (projPv.id && !e.shiftKey) projPvShow(next); // an open preview follows the arrows
    aside.querySelector(`.pj-item[data-pjc="${CSS.escape(next)}"]`)?.scrollIntoView?.({ block: "nearest" });
    return true;
  }
  return false;
}

/* ---------- wiring (in the app only; node tests load this file without a page) ---------- */
if (typeof document !== "undefined" && typeof window !== "undefined" && window.__TAURI__) {
  document.addEventListener("click", (e) => {
    const b = e.target.closest?.("[data-pjm],[data-pja],[data-pjp],[data-pj],[data-pje],[data-pjw]");
    if (!b || b.disabled) return;
    const d = b.dataset;
    try {
      // The Library panel's notice about a saved edit that could not be opened (R2 #3).
      if (d.pjw) {
        const id = projPanel()?.dataset.pjFor;
        if (!id) return;
        if (d.pjw === "retry") return projRetryLoad(id);
        if (d.pjw === "keep") return void projKeepClean(id);
        if (d.pjw === "restore") return void projBringBack(id);
        return;
      }
      // The Export section's buttons (0.1.41).
      if (d.pje === "cancel") return void projExpCancel(d.arg);
      if (d.pje === "dismiss") return projExpDismiss(d.arg);
      if (d.pje === "log") return void projExpShowLog();
      if (d.pjp === "new") return void projNew();
      if (d.pjp === "pick") { const card = b.closest(".pj-card"); if (card && !S.open) projPageClick(card.dataset.pj, e); return; }
      if (d.pjp === "sel-rename") return void projRenameMany(projSel.ids.slice());
      if (d.pjp === "sel-del") return void projDeleteMany(projSel.ids.slice());
      if (d.pjp === "sel-clear") return projSelClear();
      if (d.pja === "add") return projAddSel();
      if (d.pjm) {
        closeMenu();
        if (d.pjm === "open") return projOpen(d.arg);
        if (d.pjm === "rename") return void projRename(d.arg);
        if (d.pjm === "delete") return void projDelete(d.arg);
        if (d.pjm === "pick") return projPageClick(d.arg, e);
        if (d.pjm === "rename-sel") return void projRenameMany(projSel.ids.slice());
        if (d.pjm === "delete-sel") return void projDeleteMany(projSel.ids.slice());
        if (d.pjm === "clear-sel") return projSelClear();
        if (d.pjm === "add") return projAddSel();
        if (d.pjm === "all") { const ids = projShown().map((c) => c.id); projLib.sel = [...projLib.sel, ...ids.filter((x) => !projLib.sel.includes(x))]; return projPaintSel(); }
        if (d.pjm === "none") { projLib.sel = []; projLib.anchor = null; return projPaintSel(); }
        if (d.pjm === "show") return projShowInLibrary(projLib.sel.slice());
        return;
      }
      if (d.pj && S.page === "editor" && !S.open) {
        if (performance.now() - projBoxAt < 350) return; // the release of a selection box
        // With projects picked, or Ctrl / Shift held, a click picks; otherwise it opens the project.
        if (projSel.ids.length || e.ctrlKey || e.shiftKey) return projPageClick(d.pj, e);
        return projOpen(d.pj);
      }
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
    else if (S.page === "editor" && !e.repeat && !e.target.closest?.("input, textarea, select, [contenteditable]")) {
      // The picked projects, else the card that has the focus.
      const id = e.target.closest?.(".pj-card")?.dataset.pj || null;
      const ids = projSel.ids.length ? projSel.ids.slice() : id ? [id] : [];
      if ((e.code === "Enter" || e.code === "NumpadEnter") && id) { projOpen(id); used = true; }
      else if (e.code === "Delete" && ids.length) { projDeleteMany(ids); used = true; }
      else if (e.code === "F2" && ids.length) { projRenameMany(ids); used = true; }
      else if (e.ctrlKey && e.code === "KeyA" && projList().length) { projSel.ids = [...projSel.ids, ...projList().map((p) => p.id).filter((x) => !projSel.ids.includes(x))]; projSyncSel(); used = true; }
      else if (e.code === "Escape" && projSel.ids.length && document.getElementById("menu")?.hidden !== false) { projSelClear(); used = true; }
    }
    if (used) { e.preventDefault(); e.stopImmediatePropagation(); }
  }, true);
  window.addEventListener("mousemove", projDragMove);
  window.addEventListener("mouseup", () => projDragEnd(true));
  window.addEventListener("blur", () => projDragEnd(false));
  document.addEventListener("visibilitychange", () => { if (document.hidden) projDragEnd(false); });
  // The Editor page: a box drawn over the cards picks them.
  document.addEventListener("mousedown", projPageDown);
  window.addEventListener("mousemove", projPageMove);
  window.addEventListener("mouseup", () => projPageDragEnd(true));
  window.addEventListener("blur", () => projPageDragEnd(false));
  window.addEventListener("pointercancel", () => projPageDragEnd(false));
  document.addEventListener("visibilitychange", () => { if (document.hidden) projPageDragEnd(false); });
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
    // A deleted clip makes sources of virtual projects missing (0.1.38): the list is read again, which redraws the panel.
    listen("clips-changed", () => setTimeout(projRefresh, 400));
  }
  // S.projects for the sidebar badge. A timer can fire between two script files, so wait until
  // app.js (loaded after this file) has defined S and invoke.
  const boot = () => (typeof S === "undefined" || typeof invoke !== "function" ? setTimeout(boot, 50) : projRefresh());
  setTimeout(boot, 0);
}
