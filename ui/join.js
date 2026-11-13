"use strict";
// "Edit together" (backend: join.rs). Several selected clips are joined, oldest first, into ONE new
// clip (kind "joined") by a background job of kind "join" (the renders list shows it). When it is
// done ("clip-joined" { clip, joins, mode, secs }) the new clip opens in the editor with a split at
// every join, so each original clip is a piece of its own. The splits come from `clip.joins`
// (seconds), kept in the library, so they are there again every time the clip is opened.
// Loaded after editor.js and jobs.js, before app.js: at load time this file only defines things and
// listens for its event. Names start with `join` / `JOIN_`.

const JOIN_MAX_CLIPS = 20;          // as join.rs MAX_CLIPS
const JOIN_MAX_MIN = 30;            // as join.rs MAX_TOTAL_SECS, in minutes
const JOIN_ICON = '<svg viewBox="0 0 24 24"><rect x="2.500" y="6" width="7.500" height="12" rx="1.500"/><rect x="14" y="6" width="7.500" height="12" rx="1.500"/><path d="M10 12h4"/></svg>';
const joinOpenWhenDone = new Set(); // job ids started here: open their result in the editor

/**
 * The "Edit together" item of the right-click menu of a selection (`clips`: the selected clips).
 * Nothing for one clip; disabled, with the reason under it, past the limits.
 */
function joinMenuItem(clips) {
  if (!Array.isArray(clips) || clips.length < 2) return "";
  const secs = clips.reduce((s, c) => s + (+c.duration || 0), 0);
  const why = clips.length > JOIN_MAX_CLIPS ? t("At most {n} clips can be edited together", { n: JOIN_MAX_CLIPS })
    : secs > JOIN_MAX_MIN * 60 ? t("At most {n} minutes of clips can be edited together", { n: JOIN_MAX_MIN }) : "";
  const tip = t("Joins the clips, oldest first, into one new clip and opens it in the editor. The clips themselves stay as they are.");
  if (why) {
    return `<button data-m="join" disabled aria-disabled="true" title="${esc(why)}" style="opacity:.5;cursor:default">${JOIN_ICON}<span>${esc(t("Edit together"))}</span></button><div class="menu-hint">${esc(why)}</div>`;
  }
  return `<button data-m="join" data-arg="" title="${esc(tip)}" data-sfx="launch">${JOIN_ICON}<span>${esc(t("Edit together"))}</span></button>`;
}

/** A backend refusal in the user's language (the ones that name a clip are matched by pattern). */
function joinError(e) {
  const msg = String(e?.message ?? e);
  let m = /^The video file of "(.*)" is missing$/.exec(msg);
  if (m) return t('The video file of "{title}" is missing', { title: m[1] });
  m = /^"(.*)" has no picture$/.exec(msg);
  if (m) return t('"{title}" has no picture', { title: m[1] });
  m = /^Edit together takes at most (\d+) clips$/.exec(msg);
  if (m) return t("At most {n} clips can be edited together", { n: +m[1] });
  m = /^Edit together takes at most (\d+) minutes of clips$/.exec(msg);
  if (m) return t("At most {n} minutes of clips can be edited together", { n: +m[1] });
  return t(msg);
}

/** Queue "Edit together" of clips `ids`. */
async function joinClips(ids) {
  try {
    const job = await invoke("join_clips", { ids });
    if (job && job.id) {
      joinOpenWhenDone.add(job.id);
      // Shown at once; the backend's "jobs-changed" brings the real list right after.
      if (typeof jobsSet === "function" && !jobsList.some((j) => j.id === job.id)) jobsSet([...jobsList, job]);
    }
    flash(t("Joining {clips}. The new clip opens in the editor when it is ready.", { clips: plural(ids.length) }));
  } catch (e) {
    flash(joinError(e), true);
  }
}

/** Open clip `id` in the editor (as "Open in editor" does). */
function joinOpenEditor(id) {
  openClip(id);
  const v = $("#modal video");
  const go = () => { if (S.open === id && S.player) S.player.setTrim(true); };
  if (v && v.readyState >= 1) go(); else v?.addEventListener("loadedmetadata", go, { once: true });
}

/**
 * Called by the editor when it makes a clip's edit model: a joined clip gets a split at every
 * join (each source clip is a piece). Splits that would leave too short a piece are skipped.
 */
function joinApplyCuts(m, c) {
  if (!m || !c || c.kind !== "joined" || !Array.isArray(c.joins)) return;
  for (const at of c.joins) {
    const x = +at;
    if (x > 0 && x < m.duration) emSplit(m, x);
  }
}

/** A join finished: show the clip and, when it was started here and nothing else is open, edit it. */
function joinDone(p) {
  if (!p || !p.clip || typeof S === "undefined") return;
  const i = S.clips.findIndex((c) => c.id === p.clip.id);
  if (i >= 0) S.clips[i] = p.clip; else S.clips.push(p.clip);
  // The event comes just before the job is marked done: find it by its title (the new clip's).
  const job = (typeof jobsList !== "undefined" ? jobsList : []).find((j) => j.kind === "join" && joinOpenWhenDone.has(j.id) && (j.result_id === p.clip.id || j.title === p.clip.title));
  const mine = job ? joinOpenWhenDone.delete(job.id) : false;
  sfx("success");
  const busy = !!S.open || !$("#modal").hidden || !$("#confirm").hidden;
  if (mine && !busy) {
    if (typeof render === "function") render();
    joinOpenEditor(p.clip.id);
    return;
  }
  if (typeof render === "function" && !S.open) render();
  flash(t('"{title}" is ready in your library', { title: p.clip.title }));
}

window.__TAURI__?.event?.listen("clip-joined", ({ payload }) => {
  try { joinDone(payload); } catch (e) { console.error("Glint: clip-joined failed", e); }
});
