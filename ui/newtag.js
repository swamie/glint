"use strict";
// newtag (0.1.46): a small red "New" tag beside new features, shown only during the FIRST RUN of Glint after
// it was updated to the version that brought them. A run is the app's process: closing the window to the tray
// and opening it again is the same run (same `launch_id` from the backend, app.rs), so the tags stay; once Glint
// is quit and started again they are gone for good. A fresh install shows none (to a new user everything is
// new), and after skipped versions only the installed version's tags show.
// localStorage["glint.newtag"] = { version, launch, show }: written the first time a page sees a version.
// Loaded after changelog.js (it reads wnPrevSeen, the version seen before; wnStartup calls ntStartup).
// Names start with `nt` / `NT_`; the tag is <span class="nt-tag"> (newtag.css).

/**
 * The registry: ONE line per new feature. `where` is a CSS selector; the tag goes inside each match, at the
 * end (default), at the start (`place: "start"`) or on its top right corner (`place: "corner"`: nothing
 * moves or wraps, for a button in a full row). The items of the installed release's "New" group in What's new are tagged as well.
 */
const GLINT_NEW = [
  { since: "0.1.46", where: "#view .bd-pick > .txt > .name" },                                     // Settings: the Backdrop picker
  { since: "0.1.46", where: '#view [data-act="thumbs-missing"]', place: "corner" }, // Library: the thumbnail refresh button
  { since: "0.1.46", where: ".ws-tabs", place: "corner" },                                        // the editor's inspector tabs (one tag)
];
const NT_KEY = "glint.newtag";
let ntOn = false;      // tags are shown in this run
let ntVer = "";        // the version they are for
let ntBegun = false;
let ntObs = null;
let ntQueued = false;

/**
 * Show the tags or not (pure, tested in node). `rec`: the stored { version, launch, show } or null;
 * `prevSeen`: the version seen before this page stored the current one ("" = none ever: a fresh install);
 * `version`, `launch`: this run's. Returns { show, rec } with the record to store.
 */
function ntDecide(rec, prevSeen, version, launch) {
  if (!version || !launch) return { show: false, rec: rec || null };
  if (rec && rec.version === version) return { show: !!rec.show && rec.launch === launch, rec };
  // First sight of this version: an update (an earlier version was seen) shows, a fresh install does not.
  const show = !!prevSeen && prevSeen !== version;
  return { show, rec: { version, launch, show } };
}

/** The rules for this version, plus the installed release's "New" items in What's new (changelog.js). */
function ntRules() {
  return GLINT_NEW.filter((r) => r.since === ntVer).concat([{ where: `.wn-rel[data-ver="${ntVer}"] .wn-grp-new li > .wn-txt`, place: "start" }]);
}

/** Put the missing tags in. Idempotent: a second pass right after changes nothing, so the observer settles. */
function ntApply() {
  ntQueued = false;
  if (!ntOn) return;
  const word = t("New");
  for (const r of ntRules()) {
    let list;
    try { list = document.querySelectorAll(r.where); } catch { continue; }
    for (const el of list) {
      const have = el.querySelector(":scope > .nt-tag");
      if (have) { if (have.textContent !== word) have.textContent = word; continue; }
      const tag = document.createElement("span");
      tag.className = r.place === "corner" ? "nt-tag nt-corner" : "nt-tag";
      tag.textContent = word;
      if (r.place === "corner" && getComputedStyle(el).position === "static") el.classList.add("nt-host"); // a sticky or absolute host keeps its own
      if (r.place === "start") el.prepend(tag); else el.append(tag);
    }
  }
}

/** Pages are rebuilt by render() and the editor draws its own: one observer, one pass per batch of changes. */
function ntWatch() {
  if (ntObs || typeof MutationObserver === "undefined") return;
  ntObs = new MutationObserver(() => { if (!ntQueued) { ntQueued = true; setTimeout(ntApply, 0); } });
  ntObs.observe(document.body, { childList: true, subtree: true });
  ntApply();
}

/** Once per page, after wnStartup (changelog.js) knew the version seen before. */
async function ntStartup() {
  if (ntBegun || typeof S === "undefined" || !S.version || typeof wnPrevSeen === "undefined") return;
  ntBegun = true;
  let launch = "";
  try { launch = String((await invoke("launch_id")) || ""); } catch {}
  let rec = null;
  try { rec = JSON.parse(localStorage.getItem(NT_KEY) || "null"); } catch {}
  const d = ntDecide(rec, wnPrevSeen, S.version, launch);
  try { if (d.rec && d.rec !== rec) localStorage.setItem(NT_KEY, JSON.stringify(d.rec)); } catch {}
  ntVer = S.version;
  ntOn = d.show;
  if (ntOn) ntWatch();
}

if (typeof module !== "undefined" && module.exports) module.exports = { ntDecide, GLINT_NEW };
else ntStartup(); // wnStartup may have run before this file loaded
