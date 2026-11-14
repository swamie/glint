"use strict";
// The import dialog. Loaded before app.js: nothing here may touch its globals at load time.
//
// openImport() shows it: pick a program (or any folder), scan it, tick the clips
// to bring in (grouped by game), choose whether to copy them and which library
// folder they go in (import_run's `place` "games" | "one" | "none" and `folder`),
// import. Backend: import_sources, import_scan, import_run, pick_import_folder, and the
// "import-progress" event ({ done, total, title, skipped? }).

/** Dialog state. `el` is the dialog's backdrop element while it is open. */
const IMP = {
  el: null,
  view: "where",        // "where" | "scanning" | "found" | "running" | "done"
  sources: [],          // { id, name, path, found }
  path: "",             // folder scanned
  items: [],            // Found: { path, title, game, size, modified, known }
  groups: [],           // { game, idx: [item indexes] }
  sel: new Set(),       // indexes of ticked items
  expanded: new Set(),  // games whose clips are shown
  copy: false,
  // "Put the imported clips in": "games" | "one" | "none"; null = as the game folders setting says.
  place: null,
  folderName: "",       // the folder for "one"
  nameTyped: false,     // the user changed it (else it follows the source: "Medal", or the folder's name)
  srcName: "",          // the program (or folder) scanned
  srcId: "",            // its source id for the logo (IMP_LOGO), "folder" for Another folder
  scanId: 0,            // ignores a scan the user has moved on from
  running: false,
  progress: { done: 0, total: 0, title: "" },
  skipped: null,        // from the last progress event of a run
  result: null,         // { added, skipped }
  listening: false,
  downOnBack: false,
};

const IMP_ICON = {
  chev: '<svg viewBox="0 0 24 24"><path d="m9 6 6 6-6 6"/></svg>',
  download: '<svg viewBox="0 0 24 24"><path d="M12 4v11M7.5 10.5 12 15l4.5-4.5M5 19h14"/></svg>',
  search: '<svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="6.5"/><path d="m16 16 4 4"/></svg>',
};

/**
 * A one-colour mark before each source's name (currentColor), by source id (import.rs `sources`).
 * nvidia, steelseries: path data from the Simple Icons set (simple-icons@13.21.0, CC0 1.0; the
 * marks themselves are their owners' trademarks, shown only to name the program).
 * Simple Icons has no Medal or Overwolf/Outplayed mark, so those two are plain neutral glyphs
 * drawn for Glint (a medal; a play sign in a ring), not the vendors' logos. "folder" = Another folder.
 */
const IMP_LOGO = {
  shadowplay: '<svg class="imp-logo" viewBox="0 0 24 24" aria-hidden="true"><path fill="currentColor" stroke="none" d="M8.948 8.798v-1.43a6.7 6.7 0 0 1 .424-.018c3.922-.124 6.493 3.374 6.493 3.374s-2.774 3.851-5.75 3.851c-.398 0-.787-.062-1.158-.185v-4.346c1.528.185 1.837.857 2.747 2.385l2.04-1.714s-1.492-1.952-4-1.952a6.016 6.016 0 0 0-.796.035m0-4.735v2.138l.424-.027c5.45-.185 9.01 4.47 9.01 4.47s-4.08 4.964-8.33 4.964c-.37 0-.733-.035-1.095-.097v1.325c.3.035.61.062.91.062 3.957 0 6.82-2.023 9.593-4.408.459.371 2.34 1.263 2.73 1.652-2.633 2.208-8.772 3.984-12.253 3.984-.335 0-.653-.018-.971-.053v1.864H24V4.063zm0 10.326v1.131c-3.657-.654-4.673-4.46-4.673-4.46s1.758-1.944 4.673-2.262v1.237H8.94c-1.528-.186-2.73 1.245-2.73 1.245s.68 2.412 2.739 3.11M2.456 10.9s2.164-3.197 6.5-3.533V6.201C4.153 6.59 0 10.653 0 10.653s2.35 6.802 8.948 7.42v-1.237c-4.84-.6-6.492-5.936-6.492-5.936z"/></svg>',
  medal: '<svg class="imp-logo" viewBox="0 0 24 24" aria-hidden="true"><path d="M8 3h3l1.6 5.2M16 3h-3l-1 3.2"/><circle cx="12" cy="15" r="5.5"/><path d="m12 12.4.9 1.8 2 .3-1.45 1.4.35 2-1.8-.95-1.8.95.35-2-1.45-1.4 2-.3z"/></svg>',
  outplayed: '<svg class="imp-logo" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="8.5"/><path d="M10 8.6v6.8l5.4-3.4z" fill="currentColor"/></svg>',
  steelseries: '<svg class="imp-logo" viewBox="0 0 24 24" aria-hidden="true"><path fill="currentColor" stroke="none" d="M14.08008 0c-1.00234 0-1.8125.80893-1.8125 1.8086v2.57226c-4.01871.7444-7.19505 3.9119-7.93946 7.91992H1.8125c-1.001 0-1.8125.80698-1.8125 1.80664 0 .99833.8115 1.8086 1.8125 1.8086h2.51563C5.18077 20.5094 9.22875 24 14.08008 24 19.54884 24 24 19.56148 24 14.10742c0-4.83662-3.50067-8.87524-8.10742-9.72656V1.80859C15.89258.80893 15.08108 0 14.08008 0ZM4.69336 3.17578c-1.00368 0-1.8164.80955-1.8164 1.81055 0 .99966.81272 1.8125 1.8164 1.8125 1.001 0 1.8164-.81284 1.8164-1.8125 0-1.001-.8154-1.81055-1.8164-1.81055zm9.38672 4.65625c3.46809 0 6.29297 2.81398 6.29297 6.2754 0 3.46006-2.82488 6.27734-6.29297 6.27734-3.46943 0-6.29297-2.81728-6.29297-6.27735 0-3.4614 2.82354-6.27539 6.29297-6.27539zm-.01758 2.4043c-2.14634 0-3.89258 1.73986-3.89258 3.88086S11.91616 18 14.0625 18c2.14634 0 3.89258-1.74182 3.89258-3.88281 0-2.141-1.74624-3.88086-3.89258-3.88086zm0 2.7168c.6455 0 1.16797.51989 1.16797 1.16406 0 .64283-.52246 1.16797-1.16797 1.16797-.64417 0-1.16992-.52514-1.16992-1.16797 0-.64417.52575-1.16407 1.16992-1.16407z"/></svg>',
};
/**
 * Medal's own icon, from its brand page (medal.tv/brand), used as that page allows: to name Medal
 * in a list of apps, the files as they are (`ui/logos/medal-icon-*.svg`, never recoloured or
 * stretched), the white one on a dark background and the black one on a light one. Medal's logos
 * are trademarks of Medal B.V.; Glint is not sponsored or endorsed by Medal. If the file does not
 * load, the drawn medal in IMP_LOGO takes its place.
 */
function impMedalLogo() {
  let light = false;
  try {
    const hex = glintNormHex(getComputedStyle(document.documentElement).getPropertyValue("--panel").trim());
    light = !!hex && glintLuminance(hex) > 0.4;
  } catch {}
  return `<img class="imp-logo imp-logo-brand" src="logos/medal-icon-${light ? "black" : "white"}.svg" alt="" aria-hidden="true" draggable="false" onerror="this.outerHTML = IMP_LOGO.medal">`;
}
/** The mark of source `id` ("folder" = the folder icon), or "" for none. */
const impLogo = (id) => (id === "folder" ? (typeof ICON === "object" ? ICON.folder.replace("<svg", '<svg class="imp-logo" aria-hidden="true"') : "") : id === "medal" ? impMedalLogo() : IMP_LOGO[id] || "");

/** Open the import dialog (the Library page's Import button). */
async function openImport() {
  if (IMP.el) { IMP.el.querySelector(".imp-box")?.focus(); return; }
  const back = document.createElement("div");
  back.className = "imp-back";
  back.innerHTML = `<div class="imp-box" role="dialog" aria-modal="true" aria-labelledby="imp-title" tabindex="-1"></div>`;
  document.body.appendChild(back);
  IMP.el = back;
  back.addEventListener("mousedown", (e) => { IMP.downOnBack = e.target === back; });
  back.addEventListener("click", impClick);
  back.addEventListener("change", impChange);
  back.addEventListener("input", impInput);
  if (!IMP.running && IMP.view !== "done") IMP.view = IMP.items.length ? "found" : "where";
  impRender();
  back.querySelector(".imp-box").focus();
  impListen();
  try {
    IMP.sources = (await invoke("import_sources")) || [];
    if (IMP.el && IMP.view === "where") impRender();
  } catch (e) { fail(e); }
}

/** Close the dialog. An import that is running carries on and reports when it ends. */
function closeImport() {
  if (!IMP.el) return;
  IMP.el.remove();
  IMP.el = null;
  if (IMP.view === "done") impReset();
}

function impReset() {
  Object.assign(IMP, { view: "where", path: "", items: [], groups: [], result: null, skipped: null });
  IMP.sel.clear();
  IMP.expanded.clear();
}

function impListen() {
  if (IMP.listening || !window.__TAURI__?.event) return;
  IMP.listening = true;
  window.__TAURI__.event.listen("import-progress", ({ payload }) => {
    if (!payload) return;
    IMP.progress = { done: payload.done || 0, total: payload.total || 0, title: payload.title || "" };
    if ("skipped" in payload) IMP.skipped = payload.skipped;
    impProgressPaint();
  }).catch(() => { IMP.listening = false; });
}

// Escape closes the dialog; Ctrl+A, Ctrl+K and Delete must not reach the
// library behind it. Registered now, before app.js adds its own window
// handler, so this one runs first and can stop it.
window.addEventListener("keydown", (e) => {
  if (!IMP.el) return;
  const box = IMP.el.querySelector(".imp-box");
  if (e.code === "Escape") { e.preventDefault(); e.stopImmediatePropagation(); closeImport(); return; }
  if ((e.ctrlKey && (e.code === "KeyA" || e.code === "KeyK")) || e.code === "Delete") {
    e.stopImmediatePropagation();
    if (!e.target.matches?.("input[type=text], textarea")) e.preventDefault();
  }
  // Keep Tab inside the dialog.
  if (e.code === "Tab" && box && !box.contains(document.activeElement)) { e.preventDefault(); box.focus(); }
}, true);

/* ---------- helpers ---------- */

const impPlural = (n, one, many) => (n === 1 ? t(one) : t(many, { n }));
const impClips = (n) => impPlural(n, "1 clip", "{n} clips");
const impDate = (ms) => { try { return new Date(ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }); } catch { return new Date(ms).toLocaleString(); } };

function impGroupsOf(items) {
  const by = new Map();
  items.forEach((it, i) => {
    const g = it.game || "Imported";
    if (!by.has(g)) by.set(g, []);
    by.get(g).push(i);
  });
  // Most recent first ("Imported" last): the items are already newest first.
  const groups = [...by].map(([game, idx]) => ({ game, idx }));
  groups.sort((a, b) => (a.game === "Imported") - (b.game === "Imported") || items[b.idx[0]].modified - items[a.idx[0]].modified);
  return groups;
}

/** The "Put the imported clips in" choice now. */
const impPlace = () => IMP.place || (S.settings?.game_folders ? "games" : "none");
/** Whether the Import button can run: "One folder" needs a name. */
const impPlaceOk = () => impPlace() !== "one" || !!IMP.folderName.trim().replace(/^\.+$/, "");

function impSelected() {
  let n = 0, size = 0;
  for (const i of IMP.sel) { n++; size += IMP.items[i].size || 0; }
  return { n, size };
}

/* ---------- rendering ---------- */

function impRender() {
  if (!IMP.el) return;
  const box = IMP.el.querySelector(".imp-box");
  const body = box.querySelector(".imp-body");
  const keep = body ? body.scrollTop : 0;
  box.innerHTML = `
    <header class="imp-head">
      <h2 id="imp-title">${esc(t("Import clips"))}</h2>
      <button class="imp-x" data-imp-act="close" title="${esc(t("Close"))}" aria-label="${esc(t("Close"))}">${ICON.x}</button>
    </header>
    <div class="imp-body">${impBody()}</div>
    <footer class="imp-foot">${impFoot()}</footer>`;
  const nb = box.querySelector(".imp-body");
  if (nb && IMP.view === "found") nb.scrollTop = keep;
  // A group's box shows "some ticked" as a dash.
  box.querySelectorAll("input[data-imp-mixed]").forEach((cb) => { cb.indeterminate = true; });
  impProgressPaint();
}

function impBody() {
  switch (IMP.view) {
    case "scanning":
      return `<div class="imp-wait"><span class="imp-spin" aria-hidden="true"></span>
        <div><div class="imp-wait-h">${esc(t("Looking for clips..."))}</div>
        <div class="imp-path" title="${esc(IMP.path)}">${esc(IMP.path)}</div>
        <div class="imp-hint">${esc(t("A big folder can take a few seconds."))}</div></div></div>`;
    case "found": return impFound();
    case "running": return impRunning();
    case "done": return impDone();
    default: return impWhere();
  }
}

function impWhere() {
  const rows = IMP.sources.length
    ? IMP.sources.map((s, i) => `
      <div class="imp-src${s.found ? "" : " off"}">
        <div class="imp-src-txt">
          <div class="imp-src-name">${impLogo(s.id)}<span>${esc(s.name)}</span></div>
          ${s.found
            ? `<div class="imp-path" title="${esc(s.path)}">${esc(s.path)}</div>`
            : `<div class="imp-hint">${esc(t("Not found on this PC"))}</div>`}
        </div>
        <button class="btn sm" data-imp-act="scan" data-imp-src="${i}" ${s.found ? "" : "disabled"}>${IMP_ICON.search}<span>${esc(t("Scan"))}</span></button>
      </div>`).join("")
    : `<div class="imp-src"><div class="imp-src-txt"><div class="imp-hint">${esc(t("Looking for other clipping programs..."))}</div></div></div>`;
  return `
    <section class="imp-sec">
      <h3>${esc(t("Where from"))}</h3>
      <p class="imp-hint">${esc(t("Glint finds the clips other programs saved and sorts them by game. The original files are never moved or changed."))}</p>
      <div class="imp-list">${rows}
        <div class="imp-src">
          <div class="imp-src-txt">
            <div class="imp-src-name">${impLogo("folder")}<span>${esc(t("Another folder"))}</span></div>
            <div class="imp-hint">${esc(t("Any folder with videos in it, such as OBS recordings or an old backup."))}</div>
          </div>
          <button class="btn sm" data-imp-act="pick">${ICON.folder}<span>${esc(t("Choose a folder"))}</span></button>
        </div>
      </div>
    </section>`;
}

function impFound() {
  const head = `
    <div class="imp-from">
      <div class="imp-src-txt"><div class="imp-hint imp-scanned">${esc(t("Scanned"))}${IMP.srcId ? `<span class="imp-scanned-src">${impLogo(IMP.srcId)}<span>${esc(IMP.srcName)}</span></span>` : ""}</div><div class="imp-path" title="${esc(IMP.path)}">${esc(IMP.path)}</div></div>
      <button class="btn sm" data-imp-act="back">${esc(t("Choose another"))}</button>
    </div>`;
  if (!IMP.items.length) {
    return `${head}<div class="imp-empty"><h3>${esc(t("No clips found"))}</h3><p>${esc(t("There are no MP4, MKV or MOV videos in this folder or the folders inside it."))}</p></div>`;
  }
  const fresh = IMP.items.filter((x) => !x.known).length;
  const groups = IMP.groups.map((g, gi) => impGroup(g, gi)).join("");
  const { size } = impSelected();
  return `${head}
    <section class="imp-sec">
      <div class="imp-bar">
        <h3>${esc(t("What was found"))}</h3>
        <div class="imp-bar-btns">
          <button class="btn sm" data-imp-act="all" ${fresh ? "" : "disabled"}>${esc(t("Select all"))}</button>
          <button class="btn sm" data-imp-act="none" ${IMP.sel.size ? "" : "disabled"}>${esc(t("Select none"))}</button>
        </div>
      </div>
      ${fresh ? "" : `<p class="imp-note">${esc(t("Every clip here is already in the library."))}</p>`}
      <div class="imp-groups">${groups}</div>
    </section>
    <section class="imp-sec">
      <h3>${esc(t("How"))}</h3>
      <div class="imp-how">
        <label class="imp-opt${IMP.copy ? "" : " on"}">
          <input type="radio" name="imp-copy" value="keep" data-imp-copy="0" ${IMP.copy ? "" : "checked"}>
          <span class="imp-opt-txt"><b>${esc(t("Leave the files where they are"))}</b>
          <span class="imp-hint">${esc(t("Nothing is copied. Glint lists them from where they are now."))}</span></span>
        </label>
        <label class="imp-opt${IMP.copy ? " on" : ""}">
          <input type="radio" name="imp-copy" value="copy" data-imp-copy="1" ${IMP.copy ? "checked" : ""}>
          <span class="imp-opt-txt"><b>${esc(t("Copy them into Glint's folder"))}</b>
          <span class="imp-hint">${esc(t("Needs {size} of free space. The originals stay where they are.", { size: fmtSize(size) }))}</span></span>
        </label>
      </div>
    </section>
    ${impWhereTo()}`;
}

/**
 * "Put the imported clips in": game folders, one folder (named after the
 * source; an existing folder may be picked), or none. A library folder is a
 * folder inside Glint's clips folder: copies go into it; files left where they
 * are cannot be, so the folder only lists them, and the dialog says so.
 */
function impWhereTo() {
  const place = impPlace();
  const opt = (v, title, hint, extra = "") => `
    <label class="imp-opt${place === v ? " on" : ""}" data-imp-opt="${v}">
      <input type="radio" name="imp-place" value="${v}" data-imp-place="${v}" ${place === v ? "checked" : ""}>
      <span class="imp-opt-txt"><b>${esc(title)}</b>${hint ? `<span class="imp-hint">${esc(hint)}</span>` : ""}${extra}</span>
    </label>`;
  const names = (Array.isArray(S.folders) ? S.folders : []).map((f) => `<option value="${esc(f)}"></option>`).join("");
  // While the name is still the one taken from the source, the source's logo sits in the field.
  const logo = !IMP.nameTyped && IMP.srcId && IMP.folderName === IMP.srcName ? impLogo(IMP.srcId) : "";
  const field = `<span class="imp-name-wrap${logo ? " logo" : ""}">${logo}<input type="text" class="field imp-name" data-imp-name maxlength="60" list="imp-folder-list" value="${esc(IMP.folderName)}"
      placeholder="${esc(t("Folder name"))}" aria-label="${esc(t("Folder name"))}" spellcheck="false"></span><datalist id="imp-folder-list">${names}</datalist>`;
  const note = !IMP.copy
    ? `<p class="imp-note" data-imp-placenote ${place === "none" ? "hidden" : ""}>${esc(t("The files stay where they are, outside Glint's clips folder. The library folder only lists them: nothing is moved into it."))}</p>`
    : "";
  return `
    <section class="imp-sec">
      <h3>${esc(t("Put the imported clips in"))}</h3>
      <div class="imp-how">
        ${opt("games", t("Game folders"), t("A folder for each game."))}
        ${opt("one", t("One folder"), t("Every clip of this import goes in it. Type a name or pick one of your folders."), field)}
        ${opt("none", t("No folder"), t("They go in the main folder."))}
      </div>
      ${note}
    </section>`;
}

function impGroup(g, gi) {
  const fresh = g.idx.filter((i) => !IMP.items[i].known);
  const ticked = fresh.filter((i) => IMP.sel.has(i)).length;
  const known = g.idx.length - fresh.length;
  const size = g.idx.reduce((s, i) => s + (IMP.items[i].size || 0), 0);
  const open = IMP.expanded.has(g.game);
  const all = fresh.length > 0 && ticked === fresh.length;
  const mixed = ticked > 0 && !all;
  const clips = open ? `<div class="imp-clips">${g.idx.map((i) => impClip(i)).join("")}</div>` : "";
  return `
    <div class="imp-group${open ? " open" : ""}">
      <div class="imp-grow">
        <input type="checkbox" class="imp-cb" data-imp-group="${gi}" ${all ? "checked" : ""} ${mixed ? "data-imp-mixed" : ""} ${fresh.length ? "" : "disabled"} aria-label="${esc(g.game)}">
        <button class="imp-gbtn" data-imp-act="toggle" data-imp-g="${gi}" aria-expanded="${open}">
          <span class="imp-gi">${gameIcon(g.game)}</span>
          <span class="imp-gname">${esc(g.game)}</span>
          <span class="imp-gmeta"><span>${esc(impClips(g.idx.length))}</span><span>${esc(fmtSize(size))}</span>${known ? `<span class="imp-known">${esc(t("{n} in the library", { n: known }))}</span>` : ""}</span>
          <span class="imp-chev">${IMP_ICON.chev}</span>
        </button>
      </div>
      ${clips}
    </div>`;
}

function impClip(i) {
  const it = IMP.items[i];
  return `
    <label class="imp-clip${it.known ? " known" : ""}">
      <input type="checkbox" class="imp-cb" data-imp-item="${i}" ${IMP.sel.has(i) ? "checked" : ""} ${it.known ? "disabled" : ""}>
      <span class="imp-ctitle" title="${esc(it.path)}">${esc(it.title)}</span>
      <span class="imp-cmeta">${it.known ? `<span class="imp-known">${esc(t("In the library"))}</span>` : ""}<span title="${esc(timeAgo(it.modified))}">${esc(impDate(it.modified))}</span><span>${esc(fmtSize(it.size || 0))}</span></span>
    </label>`;
}

function impRunning() {
  return `
    <div class="imp-run">
      <h3>${esc(t("Importing..."))}</h3>
      <div class="progress imp-prog"><i style="width:0%"></i></div>
      <div class="imp-run-line" data-imp-line></div>
      <p class="imp-hint">${esc(t("You can close this window. The import carries on and the clips appear in the library as they are added."))}</p>
    </div>`;
}

function impDone() {
  const r = IMP.result || { added: 0, skipped: 0 };
  return `
    <div class="imp-run">
      <h3>${esc(r.added === 1 ? t("Added 1 clip") : t("Added {n} clips", { n: r.added }))}</h3>
      ${r.skipped ? `<p class="imp-bad">${esc(r.skipped === 1 ? t("1 could not be read") : t("{k} could not be read", { k: r.skipped }))}</p>` : ""}
      <p class="imp-hint">${esc(r.folder ? t("They are in the library, in the folder {folder}.", { folder: r.folder }) : t("They are in the library, grouped under their games."))}</p>
    </div>`;
}

function impFoot() {
  if (IMP.view === "found" && IMP.items.length) {
    const { n, size } = impSelected();
    const sum = n === 1 ? t("1 clip, {size}", { size: fmtSize(size) }) : t("{n} clips, {size}", { n, size: fmtSize(size) });
    return `<div class="imp-sum">${esc(sum)}</div>
      <button class="btn" data-imp-act="close">${esc(t("Cancel"))}</button>
      <button class="btn primary" data-imp-act="run" ${n && impPlaceOk() ? "" : "disabled"}>${IMP_ICON.download}<span>${esc(t("Import"))}</span></button>`;
  }
  if (IMP.view === "running") return `<div class="imp-sum"></div><button class="btn" data-imp-act="close">${esc(t("Hide"))}</button>`;
  if (IMP.view === "done") {
    return `<div class="imp-sum"></div>
      <button class="btn" data-imp-act="again">${esc(t("Import more"))}</button>
      <button class="btn primary" data-imp-act="close">${esc(t("Done"))}</button>`;
  }
  if (IMP.view === "scanning") return `<div class="imp-sum"></div><button class="btn" data-imp-act="back">${esc(t("Cancel"))}</button>`;
  return `<div class="imp-sum"></div><button class="btn" data-imp-act="close">${esc(t("Close"))}</button>`;
}

/** Update the progress bar and line in place (no full redraw per clip). */
function impProgressPaint() {
  if (!IMP.el || IMP.view !== "running") return;
  const p = IMP.progress;
  const pct = p.total ? Math.round((p.done / p.total) * 100) : 0;
  const bar = IMP.el.querySelector(".imp-prog i");
  if (bar) bar.style.width = pct + "%";
  const line = IMP.el.querySelector("[data-imp-line]");
  if (line) line.textContent = p.total ? (p.title ? t("{done} of {total}: {title}", { done: p.done, total: p.total, title: p.title }) : t("{done} of {total}", { done: p.done, total: p.total })) : t("Starting...");
}

/* ---------- actions ---------- */

async function impClick(e) {
  const back = IMP.el;
  if (!back) return;
  if (e.target === back) { if (IMP.downOnBack) closeImport(); return; }
  const btn = e.target.closest("[data-imp-act]");
  if (!btn || btn.disabled) return;
  const d = btn.dataset;
  try {
    switch (d.impAct) {
      case "close": return closeImport();
      case "scan": { const s = IMP.sources[+d.impSrc]; if (s?.found) await impScan(s.path, s.name, s.id); return; }
      case "pick": { const dir = await invoke("pick_import_folder"); if (dir) await impScan(dir, String(dir).replace(/[\\/]+$/, "").split(/[\\/]/).pop(), "folder"); return; }
      case "back": IMP.scanId++; impReset(); impRender(); if (!IMP.sources.length) IMP.sources = (await invoke("import_sources")) || []; return impRender();
      case "again": impReset(); return impRender();
      case "all": IMP.items.forEach((it, i) => { if (!it.known) IMP.sel.add(i); }); return impRender();
      case "none": IMP.sel.clear(); return impRender();
      case "toggle": { const g = IMP.groups[+d.impG]; if (!g) return; if (IMP.expanded.has(g.game)) IMP.expanded.delete(g.game); else IMP.expanded.add(g.game); return impRender(); }
      case "run": return await impRun();
    }
  } catch (err) { fail(err); }
}

function impChange(e) {
  const el = e.target;
  if ("impCopy" in el.dataset) { IMP.copy = el.dataset.impCopy === "1"; return impRender(); }
  if ("impPlace" in el.dataset) {
    IMP.place = el.dataset.impPlace;
    impRender();
    if (IMP.place === "one") IMP.el?.querySelector("[data-imp-name]")?.focus();
    return;
  }
  if ("impGroup" in el.dataset) {
    const g = IMP.groups[+el.dataset.impGroup];
    if (!g) return;
    for (const i of g.idx) if (!IMP.items[i].known) { if (el.checked) IMP.sel.add(i); else IMP.sel.delete(i); }
    return impRender();
  }
  if ("impItem" in el.dataset) {
    const i = +el.dataset.impItem;
    if (IMP.items[i]?.known) return;
    if (el.checked) IMP.sel.add(i); else IMP.sel.delete(i);
    return impRender();
  }
}

/** Typing a folder name: no redraw (the field keeps focus); it picks "One folder" and updates the Import button. */
function impInput(e) {
  const el = e.target;
  if (!("impName" in el.dataset)) return;
  IMP.folderName = el.value;
  IMP.nameTyped = true;
  el.closest(".imp-name-wrap")?.classList.remove("logo"); // the logo marks only the name taken from the source
  if (impPlace() !== "one") {
    IMP.place = "one";
    for (const lab of IMP.el.querySelectorAll("[data-imp-opt]")) {
      const on = lab.dataset.impOpt === "one";
      lab.classList.toggle("on", on);
      const radio = lab.querySelector("input[type=radio]");
      if (radio) radio.checked = on;
    }
    const note = IMP.el.querySelector("[data-imp-placenote]");
    if (note) note.hidden = false;
  }
  const run = IMP.el.querySelector('[data-imp-act="run"]');
  if (run) run.disabled = !(impSelected().n && impPlaceOk());
}

async function impScan(path, srcName = "", srcId = "") {
  const id = ++IMP.scanId;
  Object.assign(IMP, { view: "scanning", path, items: [], groups: [], srcName: srcName || "", srcId: srcId || "" });
  // "One folder" is named after the program (or folder) scanned until the user types a name.
  if (!IMP.nameTyped || !IMP.folderName.trim()) Object.assign(IMP, { folderName: IMP.srcName, nameTyped: false });
  IMP.sel.clear();
  IMP.expanded.clear();
  impRender();
  let items;
  try {
    items = (await invoke("import_scan", { path })) || [];
  } catch (e) {
    if (id !== IMP.scanId) return;
    IMP.view = "where";
    impRender();
    throw e;
  }
  if (id !== IMP.scanId) return;
  IMP.items = items;
  IMP.groups = impGroupsOf(items);
  items.forEach((it, i) => { if (!it.known) IMP.sel.add(i); });
  // With a single game, show its clips straight away.
  if (IMP.groups.length === 1) IMP.expanded.add(IMP.groups[0].game);
  IMP.view = "found";
  impRender();
}

async function impRun() {
  if (IMP.running) return;
  const items = [...IMP.sel].sort((a, b) => a - b).map((i) => IMP.items[i]).filter((it) => it && !it.known);
  if (!items.length || !impPlaceOk()) return;
  const copy = IMP.copy;
  const place = impPlace();
  const folder = place === "one" ? IMP.folderName.trim() : "";
  Object.assign(IMP, { running: true, view: "running", skipped: null, progress: { done: 0, total: items.length, title: "" } });
  impRender();
  let added = 0;
  try {
    added = (await invoke("import_run", { items, copy, place, folder })) ?? 0;
  } catch (e) {
    IMP.running = false;
    IMP.view = "found";
    if (IMP.el) impRender();
    refreshClips().catch(() => {});
    throw e;
  }
  // The last progress event (with the skipped count) is sent just before the command returns.
  await new Promise((r) => setTimeout(r, 60));
  const skipped = IMP.skipped ?? 0;
  IMP.running = false;
  // The backend may spell the folder as an existing one ("medal" -> "Medal"): show the one the library has.
  const shown = folder ? ((Array.isArray(S.folders) && S.folders.find((f) => f.toLowerCase() === folder.toLowerCase())) || folder) : "";
  IMP.result = { added, skipped, folder: shown };
  IMP.view = "done";
  try { await refreshClips(); } catch (e) { fail(e); }
  if (IMP.el) impRender();
  else {
    const msg = !skipped ? (added === 1 ? t("Added 1 clip") : t("Added {n} clips", { n: added }))
      : t("Added {n} clips. {k} could not be read.", { n: added, k: skipped });
    flash(msg, !!skipped && !added);
    impReset();
  }
}
