"use strict";
// The renders button in the top bar and its list. Loaded before app.js: nothing here may touch its globals at load time.
//
// Data: background jobs come from the backend: Blur renders (kind "blur") and Compress for
// Discord (kind "compress"), both in one queue ("jobs-changed" carries the whole list,
// `list_jobs` loads it); editor exports are followed read-only from
// "export-progress" events. Nothing polls: the only timer is the one that drops
// finished or stalled export rows, and it runs only while such rows exist.
// All names here start with `jobs` / `JOBS_`; markup uses `jobs-` classes and `data-j-` attributes.

const JOBS_EXPORT_DONE_MS = 4000;   // an export row stays this long after 100 %
const JOBS_EXPORT_STALL_MS = 30000; // and is dropped after this long without news
const JOBS_PARALLEL = [1, 2, 3, 4];

let jobsList = [];                  // Blur jobs, as the backend sends them (oldest first)
let jobsGotEvent = false;           // a "jobs-changed" arrived (newer than the first list_jobs answer)
const jobsExports = new Map();      // source clip id -> { pct, start, at }
let jobsExportTimer = 0;
const jobsSeen = new Set();         // ended job ids shown in the open list (or already ended at start-up)
let jobsUnseen = false;             // something finished or failed since the list was last opened
let jobsBtnKey = "";
let jobsPopKey = "";
let jobsPop = null;                 // the pop-up panel, made on first open
let jobsStarted = false;

function jobsInit() {
  if (jobsStarted) return;
  jobsStarted = true;
  const ev = window.__TAURI__?.event;
  if (ev) {
    ev.listen("jobs-changed", ({ payload }) => { jobsGotEvent = true; jobsSet(payload); });
    ev.listen("export-progress", ({ payload }) => jobsExportProgress(payload));
  }
  invoke("list_jobs")
    .then((list) => {
      if (jobsGotEvent) return;
      for (const j of list || []) if (jobsEnded(j)) jobsSeen.add(j.id);
      jobsSet(list);
    })
    .catch((e) => console.error("Glint: list_jobs failed", e));
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && jobsIsOpen()) { e.preventDefault(); e.stopPropagation(); jobsClose(); }
  }, true);
  document.addEventListener("mousedown", (e) => {
    if (!jobsIsOpen()) return;
    if (jobsPop.contains(e.target) || e.target.closest?.(".jobs-btn")) return;
    jobsClose();
  }, true);
  window.addEventListener("resize", () => { if (jobsIsOpen()) jobsPlace(); });
  jobsRender();
}

const jobsEnded = (j) => j.state !== "queued" && j.state !== "running";
const jobsIsOpen = () => !!jobsPop && !jobsPop.hidden;

function jobsSet(list) {
  jobsList = Array.isArray(list) ? list : [];
  const ids = new Set(jobsList.map((j) => j.id));
  for (const id of jobsSeen) if (!ids.has(id)) jobsSeen.delete(id);
  jobsRender();
}

function jobsExportProgress(p) {
  if (!p || p.id == null) return;
  const now = Date.now();
  const pct = Math.min(100, Math.max(0, +p.pct || 0));
  const row = jobsExports.get(p.id);
  if (row && row.pct >= 100 && pct < 100) jobsExports.delete(p.id); // a new export of the same clip
  const cur = jobsExports.get(p.id) || { pct: 0, start: now, at: now };
  cur.pct = Math.max(cur.pct, pct);
  cur.at = now;
  jobsExports.set(p.id, cur);
  jobsRender();
}

/** Drop export rows that finished a while ago or went quiet, and wake up again only while some remain. */
function jobsExportSweep() {
  clearTimeout(jobsExportTimer);
  jobsExportTimer = 0;
  const now = Date.now();
  let next = Infinity, dropped = false;
  for (const [id, x] of jobsExports) {
    const due = x.at + (x.pct >= 100 ? JOBS_EXPORT_DONE_MS : JOBS_EXPORT_STALL_MS);
    if (due <= now) { jobsExports.delete(id); dropped = true; } else next = Math.min(next, due);
  }
  if (next < Infinity) jobsExportTimer = setTimeout(() => { jobsExportTimer = 0; jobsExportSweep(); }, next - now + 50);
  return dropped;
}

function jobsClipTitle(clipId, fallback) {
  const c = (typeof S !== "undefined" && S.clips || []).find((x) => x.id === clipId);
  return c?.title || fallback || t("Untitled clip");
}

/** Every row, newest first: { key, kind, title, state, pct, message, job }. */
function jobsRows() {
  const rows = jobsList.map((j) => ({ key: j.id, kind: j.kind === "compress" ? "compress" : "blur", title: jobsClipTitle(j.clip_id, j.title), state: j.state, pct: +j.pct || 0, message: j.message || "", start: +j.queued || 0, job: j }));
  for (const [id, x] of jobsExports) rows.push({ key: "export:" + id, kind: "export", title: jobsClipTitle(id, ""), state: x.pct >= 100 ? "exported" : "exporting", pct: x.pct, message: "", start: x.start, job: null });
  return rows.sort((a, b) => b.start - a.start);
}

const jobsActive = (r) => r.state === "queued" || r.state === "running" || r.state === "exporting";

function jobsRender() {
  jobsExportSweep();
  for (const j of jobsList) {
    if (!jobsEnded(j) || jobsSeen.has(j.id)) continue;
    if (jobsIsOpen()) jobsSeen.add(j.id);
    else if (j.state === "done" || j.state === "error") jobsUnseen = true;
    else jobsSeen.add(j.id); // cancelled: the user did that, no dot
  }
  const rows = jobsRows();
  jobsRenderButton(rows);
  if (jobsIsOpen()) {
    if (!rows.length) jobsClose();
    else jobsRenderPop(rows);
  }
}

function jobsRing(pct) {
  const p = Math.round(Math.min(100, Math.max(0, pct)));
  return `<svg class="jobs-ring" viewBox="0 0 20 20" aria-hidden="true"><circle class="jobs-ring-track" cx="10" cy="10" r="8" pathLength="100"/><circle class="jobs-ring-fill" cx="10" cy="10" r="8" pathLength="100" stroke-dasharray="${p} 100"/></svg>`;
}

const JOBS_ICON_STACK = '<svg class="jobs-icon" viewBox="0 0 24 24" aria-hidden="true"><rect x="4" y="4" width="16" height="11" rx="2"/><path d="M7 19h10"/></svg>';

/** The button is made once and then only updated in place, so a click is never lost to a redraw. */
function jobsButton(slot) {
  let btn = slot.querySelector(".jobs-btn");
  if (btn) return btn;
  slot.innerHTML = `<button type="button" class="btn sm jobs-btn" aria-haspopup="dialog" aria-expanded="false">${jobsRing(0)}${JOBS_ICON_STACK}<span class="jobs-label"></span><i class="jobs-dot" aria-hidden="true"></i></button>`;
  btn = slot.firstElementChild;
  btn.addEventListener("click", jobsToggle);
  jobsBtnKey = "";
  return btn;
}

function jobsRenderButton(rows) {
  const slot = document.getElementById("jobs-slot");
  if (!slot) return;
  // "auto": only while something is rendering or waiting (and while its list is open, or a
  // render failed and has not been looked at yet). "always" and "never" are what they say.
  const mode = (typeof S !== "undefined" && S.settings && S.settings.renders_button) || "auto";
  const active = rows.filter(jobsActive);
  const failed = jobsUnseen && jobsList.some((j) => j.state === "error" && !jobsSeen.has(j.id));
  if (mode === "never" || (mode !== "always" && !active.length && !failed && !jobsIsOpen())) {
    slot.hidden = true;
    return;
  }
  slot.hidden = false;
  const btn = jobsButton(slot);
  const overall = active.length ? Math.round(active.reduce((s, r) => s + (r.state === "queued" ? 0 : r.pct), 0) / active.length) : 0;
  // "rendering" while only renders and exports run; "working" once a compression is among them.
  const rendering = active.every((r) => r.kind !== "compress");
  const label = !active.length ? t("Renders") : rendering ? t("{n} rendering", { n: active.length }) : t("{n} working", { n: active.length });
  const bad = !active.length && jobsUnseen && jobsList.some((j) => j.state === "error" && !jobsSeen.has(j.id));
  const fresh = !active.length && jobsUnseen;
  const tip = fresh ? t("Renders (new results)") : !active.length ? label
    : rendering ? t("{n} rendering, {pct} % overall", { n: active.length, pct: overall }) : t("{n} working, {pct} % overall", { n: active.length, pct: overall });
  const open = jobsIsOpen();
  const key = [label, tip, overall, !!active.length, fresh, bad, open].join("|");
  if (key === jobsBtnKey) return;
  jobsBtnKey = key;
  btn.classList.toggle("jobs-busy", !!active.length);
  btn.classList.toggle("jobs-new", fresh);
  btn.classList.toggle("jobs-new-bad", bad);
  btn.querySelector(".jobs-ring-fill").setAttribute("stroke-dasharray", `${overall} 100`);
  btn.querySelector(".jobs-label").textContent = label;
  btn.title = tip;
  btn.setAttribute("aria-label", tip);
  btn.setAttribute("aria-expanded", String(open));
}

function jobsStateLine(r) {
  const pct = Math.floor(r.pct);
  const squeeze = r.kind === "compress";
  switch (r.state) {
    case "queued": return squeeze ? t("Waiting to compress for Discord") : t("Waiting");
    case "running": return squeeze ? t("Compressing for Discord {pct} %", { pct }) : t("Rendering {pct} %", { pct });
    case "done": return squeeze ? t("Compressed for Discord") : t("Finished");
    case "error": return r.message ? t("Failed: {message}", { message: r.message }) : t("Failed");
    case "cancelled": return t("Cancelled");
    case "exporting": return t("Exporting {pct} %", { pct });
    case "exported": return t("Exported");
    default: return r.state;
  }
}

const jobsHasBar = (r) => r.state === "running" || r.state === "exporting";

/** What makes a row need rebuilding; a progress change alone is updated in place. */
const jobsRowSig = (r) => [r.state, r.title, r.message, r.job ? r.job.result_id : ""].join("\u0001");

function jobsRowEl(r) {
  const line = jobsStateLine(r);
  const bar = jobsHasBar(r) ? `<div class="progress jobs-bar"><i style="width:${r.pct.toFixed(1)}%"></i></div>` : "";
  let act = "";
  // Backend jobs (Blur and compress) can be cancelled and their result opened; export rows only follow along.
  if (r.job && (r.state === "queued" || r.state === "running")) act = `<button type="button" class="btn sm jobs-act" data-j-cancel="${esc(r.job.id)}">${esc(t("Cancel"))}</button>`;
  else if (r.job && r.state === "done" && r.job.result_id) act = `<button type="button" class="btn sm jobs-act" data-j-open="${esc(r.job.result_id)}">${esc(t("Open"))}</button>`;
  const el = document.createElement("div");
  el.className = `jobs-row jobs-st-${r.state}`;
  el.innerHTML = `<div class="jobs-main">
      <div class="jobs-title" title="${esc(r.title)}">${esc(r.title)}</div>
      <div class="jobs-state" title="${esc(line)}">${esc(line)}</div>
      ${bar}
    </div>${act}`;
  el.jobsKey = r.key;
  el.jobsSig = jobsRowSig(r);
  return el;
}

/** Bring the open list up to date, touching only what changed (no node is replaced for a progress tick). */
function jobsRenderPop(rows) {
  const list = jobsPop.querySelector(".jobs-rows");
  const have = new Map([...list.children].map((el) => [el.jobsKey, el]));
  const want = [];
  for (const r of rows) {
    let el = have.get(r.key);
    if (el && el.jobsSig === jobsRowSig(r)) {
      if (jobsHasBar(r)) {
        const line = jobsStateLine(r), st = el.querySelector(".jobs-state");
        if (st.textContent !== line) { st.textContent = line; st.title = line; }
        el.querySelector(".jobs-bar i").style.width = `${r.pct.toFixed(1)}%`;
      }
    } else {
      const fresh = jobsRowEl(r);
      if (el) el.replaceWith(fresh); else list.appendChild(fresh);
      el = fresh;
    }
    have.delete(r.key);
    want.push(el);
  }
  for (const el of have.values()) el.remove();
  // Reorder only if needed (moving a node mid-click would swallow the click).
  if (want.some((el, i) => list.children[i] !== el)) want.forEach((el) => list.appendChild(el));

  const par = clamp(+(S.settings && S.settings.blur_parallel) || 2, 1, 4);
  const anyEnded = rows.some((r) => !jobsActive(r));
  const foot = `${par}|${anyEnded}`;
  if (foot !== jobsPopKey) {
    jobsPopKey = foot;
    jobsPop.querySelector("[data-j-clear]").disabled = !anyEnded;
    for (const b of jobsPop.querySelectorAll("[data-j-par]")) {
      const on = +b.dataset.jPar === par;
      b.classList.toggle("jobs-on", on);
      b.setAttribute("aria-pressed", String(on));
    }
  }
}

function jobsMakePop() {
  if (jobsPop) return;
  jobsPop = document.createElement("div");
  jobsPop.className = "jobs-pop";
  jobsPop.setAttribute("role", "dialog");
  jobsPop.setAttribute("aria-label", t("Renders"));
  jobsPop.hidden = true;
  jobsPop.innerHTML = `<div class="jobs-head">${esc(t("Renders"))}</div>
  <div class="jobs-rows"></div>
  <div class="jobs-foot">
    <button type="button" class="btn sm" data-j-clear>${esc(t("Clear finished"))}</button>
    <div class="jobs-par" role="group" aria-label="${esc(t("Renders at once"))}" title="${esc(t("Renders at once"))}">
      <span class="jobs-par-label">${esc(t("At once"))}</span>
      ${JOBS_PARALLEL.map((n) => `<button type="button" class="jobs-par-btn" data-j-par="${n}" aria-pressed="false">${n}</button>`).join("")}
    </div>
  </div>`;
  jobsPop.addEventListener("click", jobsPopClick);
  document.body.appendChild(jobsPop);
}

function jobsPlace() {
  const btn = document.querySelector("#jobs-slot .jobs-btn");
  if (!btn || !jobsPop) return;
  const r = btn.getBoundingClientRect();
  const w = jobsPop.offsetWidth || 340;
  jobsPop.style.top = `${Math.round(r.bottom + 6)}px`;
  jobsPop.style.left = `${Math.round(Math.max(8, Math.min(r.right - w, window.innerWidth - w - 8)))}px`;
}

function jobsOpen() {
  const rows = jobsRows();
  if (!rows.length) return;
  // Made fresh on every open, so its fixed texts follow a language change.
  if (jobsPop) { jobsPop.remove(); jobsPop = null; }
  jobsMakePop();
  jobsPop.hidden = false;
  for (const j of jobsList) if (jobsEnded(j)) jobsSeen.add(j.id);
  jobsUnseen = false;
  jobsPopKey = "";
  jobsRenderPop(rows);
  jobsPlace();
  jobsRenderButton(rows);
}

function jobsClose() {
  if (!jobsIsOpen()) return;
  jobsPop.hidden = true;
  jobsRenderButton(jobsRows());
}

function jobsToggle() {
  if (jobsIsOpen()) jobsClose(); else jobsOpen();
}

async function jobsPopClick(e) {
  const el = e.target.closest("[data-j-cancel],[data-j-open],[data-j-clear],[data-j-par]");
  if (!el || el.disabled) return;
  const d = el.dataset;
  try {
    if (d.jCancel) {
      el.disabled = true;
      await invoke("cancel_job", { id: d.jCancel });
    } else if (d.jOpen) {
      const id = d.jOpen;
      jobsClose();
      if (!S.clips.some((c) => c.id === id)) await refreshClips();
      if (S.clips.some((c) => c.id === id)) openClip(id);
      else flash(t("That clip is no longer in the library"), true);
    } else if ("jClear" in d) {
      for (const [id, x] of jobsExports) if (x.pct >= 100) jobsExports.delete(id);
      await invoke("clear_jobs");
      // The backend sends "jobs-changed"; reload as well in case that event is missed.
      jobsSet(await invoke("list_jobs"));
    } else if (d.jPar) {
      const n = clamp(+d.jPar || 2, 1, 4);
      if (S.settings && S.settings.blur_parallel !== n) {
        S.settings.blur_parallel = n;
        saveSettings();
      }
      jobsRender();
    }
  } catch (err) {
    fail(err);
    jobsRender();
  }
}
