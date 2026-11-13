"use strict";
// "Edit together" (backend: join.rs). The selected clips, in the order they were selected, go to a
// background job of kind "join" (the renders list shows it). Its result is a DRAFT: a hidden working
// file (`clip.draft`, kind "joined", `clip.joins` = where each clip after the first starts,
// `clip.join_names` = their titles) that the editor shows as one movable block per clip. A draft is
// never in the library: it is kept here (`joinDrafts`), found by `clipById`, and discarded
// (`discard_draft`) when the editor is done with it. Exporting makes a normal library clip.
// Loaded after editor.js and jobs.js, before app.js: at load time this file only defines things and
// listens for its event. Names start with `join` / `JOIN_`, plus `clipById` (used by the viewer).

const JOIN_MAX_CLIPS = 20;          // as join.rs MAX_CLIPS
const JOIN_MAX_MIN = 30;            // as join.rs MAX_TOTAL_SECS, in minutes
const JOIN_ICON = '<svg viewBox="0 0 24 24"><rect x="2.500" y="6" width="7.500" height="12" rx="1.500"/><rect x="14" y="6" width="7.500" height="12" rx="1.500"/><path d="M10 12h4"/></svg>';
const joinOpenWhenDone = new Set(); // job ids started here
const joinDrafts = new Map();       // draft id -> { clip, exported }: drafts the UI holds (never in S.clips)
const joinPending = [];             // draft ids waiting for the viewer to be free, oldest first
let joinPendingTimer = 0;
let joinAsking = false;             // the "Close without exporting?" prompt is up

/** A clip by id: a library clip (S.clips) or a draft held here. null when neither. */
function clipById(id) {
  if (id == null) return null;
  const lib = typeof S !== "undefined" && Array.isArray(S.clips) ? S.clips.find((c) => c.id === id) : null;
  return lib || joinDrafts.get(id)?.clip || null;
}
/** True for the id of a draft the UI holds. */
const joinIsDraft = (id) => id != null && joinDrafts.has(id);

/** Hold draft `clip` (or update the one held). It never goes into S.clips. */
function joinKeepDraft(clip) {
  if (!clip || clip.id == null) return;
  const d = joinDrafts.get(clip.id);
  if (d) d.clip = clip; else joinDrafts.set(clip.id, { clip, exported: false });
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
  const tip = t("Opens the clips in the editor in the order you selected them. Each one stays its own clip there, to move, trim or cut out. The clips themselves stay as they are.");
  if (why) {
    return `<button data-m="join" disabled aria-disabled="true" title="${esc(why)}" style="opacity:.5;cursor:default">${JOIN_ICON}<span>${esc(t("Edit together"))}</span></button><div class="menu-hint">${esc(why)}</div>`;
  }
  return `<button data-m="join" data-arg="" title="${esc(tip)}" data-sfx="launch">${JOIN_ICON}<span>${esc(t("Edit together"))}</span></button>`;
}

/** A backend refusal in the user's language (the ones that name a clip are matched by pattern). */
function joinError(e) {
  const msg = String(e?.message ?? e).trim();
  let m = /The video file of "(.*)" is missing/.exec(msg);
  if (m) return t('The video file of "{title}" is missing', { title: m[1] });
  m = /^"(.*)" has no picture/.exec(msg);
  if (m) return t('"{title}" has no picture', { title: m[1] });
  m = /at most (\d+) clips/i.exec(msg);
  if (m) return t("At most {n} clips can be edited together", { n: +m[1] });
  m = /at most (\d+) minutes/i.exec(msg);
  if (m) return t("At most {n} minutes of clips can be edited together", { n: +m[1] });
  return t(msg);
}

/** Queue "Edit together" of clips `ids`, in that order (the order selected). */
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

/** Open clip `id` in the editor (as "Open in editor" does). A draft opens in edit mode by itself. */
function joinOpenEditor(id) {
  openClip(id);
  if (joinIsDraft(id)) return;
  const v = $("#modal video");
  const go = () => { if (S.open === id && S.player) S.player.setTrim(true); };
  if (v && v.readyState >= 1) go(); else v?.addEventListener("loadedmetadata", go, { once: true });
}

/** Something is open that a draft must not push aside: the viewer, a prompt or a dialog. */
function joinBusy() {
  if (typeof S === "undefined") return true;
  if (S.open || !$("#modal").hidden || !$("#confirm").hidden) return true;
  return !!document.querySelector(".bug-dlg, .wn-dlg");
}

/** Open the oldest waiting draft once nothing else is open (checks again every half second until then). */
function joinOpenPending() {
  clearTimeout(joinPendingTimer);
  joinPendingTimer = 0;
  while (joinPending.length && !joinDrafts.has(joinPending[0])) joinPending.shift();
  if (!joinPending.length) return;
  if (joinBusy()) { joinPendingTimer = setTimeout(joinOpenPending, 500); return; }
  joinOpenEditor(joinPending.shift());
  if (joinPending.length) joinPendingTimer = setTimeout(joinOpenPending, 500);
}

/** A join finished. A draft opens in the editor now, or as soon as what is open now is closed. */
function joinDone(p) {
  if (!p || !p.clip || typeof S === "undefined") return;
  const clip = p.clip;
  // The event comes just before the job is marked done: find it by its result or title.
  const job = (typeof jobsList !== "undefined" ? jobsList : []).find((j) => j.kind === "join" && joinOpenWhenDone.has(j.id) && (j.result_id === clip.id || j.title === clip.title));
  const mine = job ? joinOpenWhenDone.delete(job.id) : false;
  sfx("success");
  if (!clip.draft) { // a backend from before drafts: the result is a library clip
    const i = S.clips.findIndex((c) => c.id === clip.id);
    if (i >= 0) S.clips[i] = clip; else S.clips.push(clip);
    if (mine && !joinBusy()) { if (typeof render === "function") render(); return joinOpenEditor(clip.id); }
    if (typeof render === "function" && !S.open) render();
    return flash(t('"{title}" is ready in your library', { title: clip.title }));
  }
  joinKeepDraft(clip);
  if (!joinPending.includes(clip.id)) joinPending.push(clip.id);
  if (joinBusy()) flash(t("The clips are ready for the editor. They open when you close what is open now."));
  joinOpenPending();
}

/** The user changed something in the open draft: an edit the model counts, or the clips put in another order. */
function joinDraftChanged(P) {
  const m = P && P.model;
  if (!m) return false;
  try {
    if (typeof emChanged === "function" && emChanged(m)) return true;
  } catch (e) {
    console.warn("Glint: emChanged failed; asking before closing the draft", e);
    return true;
  }
  const parts = Array.isArray(m.parts) ? m.parts : [];
  return parts.some((x, i) => i > 0 && +x.src < +parts[i - 1].src);
}

/** Discard draft `id`: drop it here, and ask the backend to delete it (after an export of it still running). */
function joinDiscard(id) {
  joinDrafts.delete(id);
  const i = joinPending.indexOf(id);
  if (i >= 0) joinPending.splice(i, 1);
  if (typeof S !== "undefined" && S.exports && S.exports[id]) { setTimeout(() => joinDiscard(id), 1000); return; }
  // false (still in use) is fine: the backend clears drafts at the next start.
  Promise.resolve().then(() => invoke("discard_draft", { id })).catch((e) => console.warn("Glint: discard_draft failed", id, e));
}
function joinDiscardAll() { for (const id of [...joinDrafts.keys()]) joinDiscard(id); }

/**
 * Leave the open draft, then run `then` (close the viewer, or open another clip). When the user
 * changed something and nothing was exported from it, ask first. The draft is discarded after `then`.
 */
function joinLeaveDraft(then) {
  const id = typeof S !== "undefined" ? S.open : null;
  const d = joinDrafts.get(id);
  if (!d) return then();
  if (joinAsking) return;
  const go = () => { joinDrafts.delete(id); then(); joinDiscard(id); };
  if (d.exported || (S.exports && S.exports[id]) || !joinDraftChanged(S.player)) return go();
  joinAsking = true;
  ask(t("Close without exporting? The clips themselves stay as they are."), t("Close")).then((yes) => {
    joinAsking = false;
    if (yes && S.open === id) go();
  });
}

/** An export finished (`meta`, the new library clip). If it came from the open draft, closing it asks no more. */
function joinDraftExported(meta) {
  if (!meta || meta.draft || typeof S === "undefined") return;
  const d = joinDrafts.get(S.open);
  if (d && meta.id !== S.open) d.exported = true;
}

/** The window's close button: leave an open draft first (it may ask), discard every draft, then `go`. */
function joinWindowClose(go) {
  const finish = () => { joinDiscardAll(); setTimeout(go, 60); };
  if (typeof S !== "undefined" && joinIsDraft(S.open)) return joinLeaveDraft(() => { closeClip(true); finish(); });
  finish();
}

// The page going away (the window closed some other way, e.g. Alt+F4): best effort; the backend also clears drafts at start-up.
window.addEventListener?.("pagehide", () => { try { joinDiscardAll(); } catch {} });

window.__TAURI__?.event?.listen("clip-joined", ({ payload }) => {
  try { joinDone(payload); } catch (e) { console.error("Glint: clip-joined failed", e); }
});
