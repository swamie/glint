"use strict";
// thumbs-refresh (0.1.46): make thumbnails again. In the Library, F5 makes the MISSING ones again and Ctrl+F5 all of
// them (also two entries beside "Rescan folder"). The backend (polish.rs `refresh_thumbs`, command `thumbs_refresh`)
// runs it as a job (kind "thumbs" in the jobs list, cancellable); each picture that arrives redraws its card
// ("clips-changed"), and "thumbs-done" { all, total, made, failed, reasons: [[reason, count]], cancelled } ends it:
// a flash with the result, or, when some failed, a small dialog with the reasons and "Show the log".
// Loaded after app.js (it uses S, invoke, flash, t, esc). Names start with `thr`.

/** What a key does in the Library: "missing" (F5), "all" (Ctrl+F5) or null. Not in a text box, not while a clip
 *  or the editor is open, not with Alt / Shift / Win (those stay the user's), only on the Library's pages. */
function thrKeyAction(e, ctx) {
  if (!e || e.code !== "F5" || e.altKey || e.shiftKey || e.metaKey || e.repeat) return null;
  if (!ctx || !ctx.library || ctx.open || ctx.inField) return null;
  return e.ctrlKey ? "all" : "missing";
}

/** A backend reason in the user's language. */
function thrReason(x) {
  const s = String(x || "");
  const m = /^ffmpeg could not read the video: (.*)$/.exec(s);
  if (m) return t("ffmpeg could not read the video: {error}", { error: m[1] });
  const w = /^the file did not answer within (\d+) s$/.exec(s);
  if (w) return t("the file did not answer within {n} s", { n: +w[1] });
  return t(s);
}

/** The result as one line: "Thumbnails: 12 made." / "Thumbnails: 9 made, 3 failed: the file is missing (2), …". */
function thrResultText(r) {
  if (!r) return "";
  const made = +r.made || 0, failed = +r.failed || 0;
  if (r.cancelled) return t("Thumbnails: stopped, {made} made.", { made });
  if (!failed) return made === 1 ? t("Thumbnails: 1 made.") : t("Thumbnails: {made} made.", { made });
  const why = (Array.isArray(r.reasons) ? r.reasons : []).map(([x, n]) => `${thrReason(x)} (${n})`).join(", ");
  return t("Thumbnails: {made} made, {failed} failed: {reasons}", { made, failed, reasons: why });
}

/** Start it: "missing" or "all". */
async function thrStart(what) {
  const all = what === "all";
  try {
    const r = await invoke("thumbs_refresh", { all });
    const n = +(r && r.count) || 0;
    if (!n) return flash(all ? t("There are no clips to make thumbnails for.") : t("No thumbnails are missing."));
    if (r.job && typeof jobsSet === "function" && Array.isArray(jobsList) && !jobsList.some((j) => j.id === r.job.id)) jobsSet([...jobsList, r.job]);
    flash(n === 1 ? t("Making 1 thumbnail…") : t("Making {n} thumbnails…", { n }));
  } catch (e) {
    fail(e);
  }
}

/** "thumbs-done": the result; with failures, a small dialog that can open the log. */
function thrDone(r) {
  const text = thrResultText(r);
  if (!r || !(+r.failed > 0) || r.cancelled) return flash(text);
  const el = document.getElementById("confirm");
  if (!el || !el.hidden) return flash(text, true); // something else is asking: the flash says it
  el.innerHTML = `<div class="confirm-box"><p>${esc(text)}</p><div class="confirm-row"><button class="btn" data-c="log">${esc(t("Show the log"))}</button><button class="btn primary" data-c="ok">${esc(t("OK"))}</button></div></div>`;
  el.hidden = false;
  const done = (log) => {
    el.hidden = true; el.innerHTML = ""; el.onclick = null; askKey = null;
    if (log) { if (typeof projExpShowLog === "function") projExpShowLog(); else if (typeof bugOpen === "function") bugOpen(); }
  };
  askKey = (e) => { if (e.code === "Enter" || e.code === "NumpadEnter" || e.code === "Escape") done(false); };
  el.onclick = (e) => { const b = e.target.closest("[data-c]"); if (b) done(b.dataset.c === "log"); else if (e.target === el) done(false); };
}

if (typeof window !== "undefined" && window.__TAURI__) {
  window.__TAURI__.event?.listen?.("thumbs-done", ({ payload }) => { try { thrDone(payload); } catch (e) { console.error("Glint: thumbs-done failed", e); } });
}
if (typeof module !== "undefined" && module.exports) module.exports = { thrKeyAction, thrResultText, thrReason };
