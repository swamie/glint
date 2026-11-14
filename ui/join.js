"use strict";
// "Edit together" (backend: join.rs, projects.rs). The selected clips, in the order they were
// selected, go to a background job of kind "join" (the renders list shows it). Since 0.1.16 the
// result is an editor PROJECT (projects.js): a kept draft (`clip.draft`, kind "joined",
// `clip.joins` = where each clip after the first starts, `clip.join_names` = their titles) plus
// its saved edit. The project opens in the editor when it is ready and stays in the Editor's list
// until the user deletes it: closing the editor never asks and never deletes it.
// Loaded after editor.js and jobs.js, before projects.js and app.js: at load time this file only
// defines things. Names start with `join` / `JOIN_`, plus `clipById` (used by the viewer).

const JOIN_MAX_CLIPS = 20;          // as join.rs MAX_CLIPS (also a project's limit)
const JOIN_MAX_MIN = 30;            // as join.rs MAX_TOTAL_SECS, in minutes
const JOIN_ICON = '<svg viewBox="0 0 24 24"><rect x="2.500" y="6" width="7.500" height="12" rx="1.500"/><rect x="14" y="6" width="7.500" height="12" rx="1.500"/><path d="M10 12h4"/></svg>';
const joinOpenWhenDone = new Set(); // job ids started here ("Edit together"): their project opens when ready
const joinPending = [];             // project ids waiting for the viewer to be free, oldest first
let joinPendingTimer = 0;

/** The project whose draft has id `id` (the project id), or null. */
function joinProjectOf(id) {
  if (id == null || typeof S === "undefined" || !Array.isArray(S.projects)) return null;
  return S.projects.find((p) => p && p.id === id) || null;
}

/** A clip by id: a library clip (S.clips) or a project's draft. null when neither. */
function clipById(id) {
  if (id == null) return null;
  const lib = typeof S !== "undefined" && Array.isArray(S.clips) ? S.clips.find((c) => c.id === id) : null;
  if (lib) return lib;
  const p = joinProjectOf(id);
  return p && p.clip && p.clip.id === id ? p.clip : null;
}
/** True for the id of a project's draft. */
const joinIsDraft = (id) => !!clipById(id)?.draft;

/** A draft arrived from the backend (an export result or a rebuilt draft): it belongs to its project, never to S.clips. */
function joinKeepDraft(clip) {
  if (!clip || clip.id == null) return;
  const p = joinProjectOf(clip.id);
  if (p) p.clip = clip;
  if (typeof S !== "undefined" && Array.isArray(S.clips)) {
    const i = S.clips.findIndex((c) => c.id === clip.id);
    if (i >= 0) S.clips.splice(i, 1);
  }
}

/**
 * The "Edit together" item of the right-click menu of a selection (`clips`: the selected clips).
 * Nothing for one clip; disabled, with the reason under it, past the limits.
 */
function joinMenuItem(clips) {
  if (!Array.isArray(clips) || clips.length < 2) return "";
  const secs = clips.reduce((s, c) => s + (+c.duration || 0), 0);
  const why = clips.length > JOIN_MAX_CLIPS ? t("At most {n} clips can be edited together", { n: JOIN_MAX_CLIPS })
    : secs > JOIN_MAX_MIN * 60 ? t("At most {n} minutes of clips can be edited together", { n: JOIN_MAX_MIN }) : "";
  const tip = t("Opens the clips in the editor as a new project, in the order you selected them. Each one stays its own clip there, to move, trim or cut out. The clips themselves stay as they are.");
  if (why) {
    return `<button data-m="join" disabled aria-disabled="true" title="${esc(why)}" style="opacity:.5;cursor:default">${JOIN_ICON}<span>${esc(t("Edit together"))}</span></button><div class="menu-hint">${esc(why)}</div>`;
  }
  return `<button data-m="join" data-arg="" title="${esc(tip)}" data-sfx="launch">${JOIN_ICON}<span>${esc(t("Edit together"))}</span></button>`;
}

/** A backend refusal in the user's language (the ones that name a clip or a number are matched by pattern). */
function joinError(e) {
  const msg = String(e?.message ?? e).trim();
  let m = /The video file of "(.*)" is missing/.exec(msg);
  if (m) return t('The video file of "{title}" is missing', { title: m[1] });
  m = /^"(.*)" has no picture/.exec(msg);
  if (m) return t('"{title}" has no picture', { title: m[1] });
  m = /project takes at most (\d+) clips/i.exec(msg);
  if (m) return t("A project takes at most {n} clips", { n: +m[1] });
  m = /project takes at most (\d+) minutes/i.exec(msg);
  if (m) return t("A project takes at most {n} minutes of clips", { n: +m[1] });
  m = /at most (\d+) clips/i.exec(msg);
  if (m) return t("At most {n} clips can be edited together", { n: +m[1] });
  m = /at most (\d+) minutes/i.exec(msg);
  if (m) return t("At most {n} minutes of clips can be edited together", { n: +m[1] });
  return t(msg);
}

/** Queue "Edit together" of clips `ids`, in that order (the order selected). It makes a project. */
async function joinClips(ids) {
  ids = [...new Set(ids)];
  try {
    const job = await invoke("join_clips", { ids });
    if (job && job.id) {
      joinOpenWhenDone.add(job.id);
      // Shown at once; the backend's "jobs-changed" brings the real list right after.
      if (typeof jobsSet === "function" && !jobsList.some((j) => j.id === job.id)) jobsSet([...jobsList, job]);
    }
    flash(t("Preparing {clips} for the editor. They open there when they are ready.", { clips: plural(ids.length) }));
  } catch (e) {
    flash(joinError(e), true);
  }
}

/** Something is open that a project must not push aside: the viewer, a prompt or a dialog. */
function joinBusy() {
  if (typeof S === "undefined") return true;
  if (S.open || !$("#modal").hidden || !$("#confirm").hidden) return true;
  return !!document.querySelector(".bug-dlg, .wn-dlg");
}

/** Open the oldest waiting project once nothing else is open (checks again every half second until then). */
function joinOpenPending() {
  clearTimeout(joinPendingTimer);
  joinPendingTimer = 0;
  while (joinPending.length && !joinProjectOf(joinPending[0])) joinPending.shift();
  if (!joinPending.length) return;
  if (joinBusy()) { joinPendingTimer = setTimeout(joinOpenPending, 500); return; }
  const id = joinPending.shift();
  if (typeof projOpen === "function") projOpen(id); else openClip(id);
  if (joinPending.length) joinPendingTimer = setTimeout(joinOpenPending, 500);
}

/**
 * A project's add job ended well ("project-changed", called by projects.js after it updated
 * S.projects). True if it was an "Edit together" started here: its project opens now, or as soon
 * as what is open now is closed.
 */
function joinDone(p) {
  if (!p || !p.project || !joinOpenWhenDone.has(p.job_id)) return false;
  joinOpenWhenDone.delete(p.job_id);
  const id = p.project.id;
  sfx("success");
  if (!joinPending.includes(id)) joinPending.push(id);
  if (joinBusy()) flash(t("The clips are ready for the editor. They open when you close what is open now."));
  joinOpenPending();
  return true;
}

/** An export finished (`meta`, the new library clip). If it came from the open project, it is now tagged "Exported". */
function joinDraftExported(meta) {
  if (!meta || meta.draft || typeof S === "undefined") return;
  const p = joinProjectOf(S.open);
  if (p && meta.id !== S.open) p.exported = true;
}

/** The window's close button: the open project saves its edit first (at most 1.5 s), then `go`. */
function joinWindowClose(go) {
  const P = typeof S !== "undefined" ? S.player : null;
  const save = P && typeof projFlush === "function" ? projFlush(P) : null;
  if (!save) return go();
  let done = false;
  const finish = () => { if (!done) { done = true; go(); } };
  Promise.resolve(save).catch(() => {}).then(finish);
  setTimeout(finish, 1500);
}
