"use strict";
// Share links (backend: share.rs; server: server/share). Optional: everything here stays hidden
// until Settings → Share links has the address of the user's own Cloudflare Worker.
// - shareMenuItem(clips): the share entries of a clip's right-click menu; shareCloudMenu(clip): the
//   whole menu of a cloud-only clip (its file was deleted from this PC after the upload).
// - shareCardBadge(clip): the cloud tag on an uploaded clip's card. Clicking it copies the link; a
//   cloud-only clip also gets a download button. shareCloudOnlyOpen(id): opening such a clip offers
//   the download instead (called by openClip in editor.js).
// - Publisher name: asked for before the first share, claimed on the server (2 to 16 characters,
//   one holder each), changed from Settings.
// - shareSettingsSection(): the Settings section (address, upload key, name, copy, test).
// - shareJobLine(row): the line of a "share" job in the renders list (jobs.js).
// - "clip-shared" { clip, url, copied }: a flash with the link.
// Names here start with `share` / `SHARE_`; markup uses `share-` classes and `data-shm` attributes.

const SHARE_ICON = '<svg viewBox="0 0 24 24"><path d="M10 14a4 4 0 0 0 5.700.3l3-3a4 4 0 0 0-5.700-5.700l-1.200 1.200"/><path d="M14 10a4 4 0 0 0-5.700-.3l-3 3a4 4 0 0 0 5.700 5.700l1.200-1.200"/></svg>';
const SHARE_DEL_ICON = '<svg viewBox="0 0 24 24"><path d="M10 14a4 4 0 0 0 5.700.3l3-3a4 4 0 0 0-5.700-5.700l-1.200 1.200"/><path d="M14 10a4 4 0 0 0-5.700-.3l-3 3a4 4 0 0 0 5.700 5.700l1.200-1.200"/><path d="m4 4 16 16"/></svg>';
/** Clips over this size get a warning: Discord may not play very large files inline. */
const SHARE_WARN_BYTES = 100e6;
let shareLinks = {}; // clip id -> link, for the current Worker
const shareMine = new Set(); // ids of the share jobs this window queued: a failure among them is flashed

const shareEsc = (s) => (typeof esc === "function" ? esc(s) : String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]));
// The menu entries show once an upload key is set (Settings → Share links), or always in a private
// build (version tagged by GLINT_BUILD_TAG, e.g. "0.1.48-dev.share"), which asks for the key on first use.
// A public release shows nothing in the menu to people who have not set up a server.
const shareHasAddr = () => !!(typeof S !== "undefined" && S.settings && S.settings.share && String(S.settings.share.url || "").trim());
const shareHasKey = () => shareHasAddr() && !!String(S.settings.share.token || "").trim();
const sharePrivateBuild = () => typeof S !== "undefined" && !/^\d+\.\d+\.\d+$/.test(String(S.version || "0.0.0"));
const shareOn = () => shareHasKey() || (shareHasAddr() && sharePrivateBuild());
const SHARE_OPEN_ICON = '<svg viewBox="0 0 24 24"><path d="M14 4h6v6M20 4l-9 9M18 14v4a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4"/></svg>';
const SHARE_UP_ICON = '<svg viewBox="0 0 24 24"><path d="M7 18a4.500 4.500 0 0 1-.500-8.970A6 6 0 0 1 18 10.500 3.800 3.800 0 0 1 17.500 18"/><path d="M12 20v-8M9 14.500l3-3 3 3"/></svg>';

const SHARE_CLOUD_ICON = '<svg viewBox="0 0 24 24"><path d="M7 18a4.500 4.500 0 0 1-.500-8.970A6 6 0 0 1 18 10.500 3.800 3.800 0 0 1 17.500 18Z"/></svg>';
const SHARE_DOWN_ICON = '<svg viewBox="0 0 24 24"><path d="M7 18a4.500 4.500 0 0 1-.500-8.970A6 6 0 0 1 18 10.500 3.800 3.800 0 0 1 17.500 18"/><path d="M12 11v8M9 16.500l3 3 3-3"/></svg>';
const SHARE_PC_ICON = '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4M9 8l6 4M15 8l-6 4"/></svg>';

/** Redraw the library when the set of uploaded clips changed (the cards carry the cloud tag). */
function shareRepaint() {
  if (typeof S !== "undefined" && !S.open && (S.page === "library" || S.page === "favorites" || S.page === "rendered") && typeof render === "function") render();
}
function shareRefresh() {
  if (!shareHasKey()) { shareLinks = {}; return Promise.resolve(); }
  return invoke("share_links").then((m) => {
    const before = Object.keys(shareLinks).sort().join();
    shareLinks = m || {};
    if (Object.keys(shareLinks).sort().join() !== before) shareRepaint();
  }).catch(() => {});
}

/** The cloud tag of a card: nothing for a clip that is not uploaded. */
function shareCardBadge(c) {
  if (!c || c.draft || (!shareLinks[c.id] && !c.cloud_only)) return "";
  const id = shareEsc(c.id);
  const copy = shareLinks[c.id] ? `<button type="button" class="share-badge${c.cloud_only ? " only" : ""}" data-shm="share" data-arg="${id}" title="${shareEsc(t("Uploaded. Click to copy the share link"))}">${SHARE_CLOUD_ICON}<span>${shareEsc(c.cloud_only ? t("Cloud only") : t("Uploaded"))}</span></button>` : `<span class="share-badge only">${SHARE_CLOUD_ICON}<span>${shareEsc(t("Cloud only"))}</span></span>`;
  const down = c.cloud_only ? `<button type="button" class="share-badge share-down" data-shm="redownload" data-arg="${id}" title="${shareEsc(t("Download it to this PC again"))}">${SHARE_DOWN_ICON}<span>${shareEsc(t("Redownload"))}</span></button>` : "";
  return `<span class="share-badges">${copy}${down}</span>`;
}

/** The right-click menu of a cloud-only clip: what still works without its file. */
function shareCloudMenu(c) {
  const btn = (what, icon, label, cls = "") => `<button data-shm="${what}" data-arg="${shareEsc(c.id)}"${cls ? ` class="${cls}"` : ""}>${icon}<span>${shareEsc(label)}</span></button>`;
  return `<div class="menu-head">${shareEsc(t("Cloud only"))}</div>`
    + btn("redownload", SHARE_DOWN_ICON, t("Redownload"))
    + (shareLinks[c.id] ? btn("share", SHARE_ICON, t("Copy share link")) + btn("open", SHARE_OPEN_ICON, t("Open share page")) : "")
    + "<hr>" + btn("delete", SHARE_DEL_ICON, t("Delete from cloud"), "danger");
}

/** Opening a cloud-only clip: offer the download. True when `id` is such a clip (the caller stops). */
function shareCloudOnlyOpen(id) {
  const c = (typeof S !== "undefined" ? S.clips || [] : []).find((k) => k.id === id);
  if (!c || !c.cloud_only) return false;
  ask(t("This clip is only in the cloud. Download it to this PC again?"), t("Redownload")).then((yes) => { if (yes) shareRedownload(id); });
  return true;
}

async function shareRedownload(id) {
  try {
    const job = await invoke("share_redownload", { id });
    if (job && job.id) {
      shareMine.add(job.id);
      shareDownloads.add(job.id);
      if (typeof jobsSet === "function" && typeof jobsList !== "undefined" && !jobsList.some((j) => j.id === job.id)) jobsSet([...jobsList, job]);
    }
    flash(t("Downloading the clip again…"));
  } catch (e) { flash(t(String(e?.message ?? e)), true); }
}
const shareDownloads = new Set(); // job ids that are downloads, for their line in the renders list

/** Delete an uploaded clip's file from this PC, after asking. The backend checks the cloud copy first. */
async function shareOffload(id, asked) {
  const c = (S.clips || []).find((k) => k.id === id);
  if (!c || c.cloud_only) return;
  if (!asked && !(await ask(shareOffloadText(c), t("Delete from this PC")))) return;
  try {
    const clip = await invoke("share_offload", { id });
    const i = S.clips.findIndex((k) => k.id === id);
    if (i >= 0 && clip) S.clips[i] = clip;
    shareRepaint();
    flash(t("Deleted from this PC. It is in the Recycle Bin, and you can download it again from the cloud."));
  } catch (e) { flash(t(String(e?.message ?? e)), true); }
}
const shareOffloadText = (c) => t("Delete “{title}” from this PC? It stays in the cloud and in your library, and you can download it again. The cloud copy keeps the picture and the main sound; extra sound tracks and any quality above 1080p 60 are not kept.", { title: c.title });

/* ---------- publisher name ---------- */

const shareHasName = () => !!(String(S.settings.share.name || "").trim() && String(S.settings.share.name_secret || "").length >= 32);
const shareNameOk = (name) => { const n = [...String(name || "").trim()].length; return n >= 2 && n <= 16; };

/** Ask for a publisher name and claim it on the server; asks again while the name is taken or
 *  breaks the rule. True once this install holds a name. `change`: asked from Settings. */
async function shareNeedName(change) {
  if (!change && shareHasName()) return true;
  let note = "";
  for (;;) {
    const asked = askText((note ? note + " " : "") + t("Choose your publisher name. It shows on your shared clips, and nobody else can use it. 2 to 16 characters."), S.settings.share.name || "", t("Use this name"));
    document.querySelector("#confirm .ask-input")?.setAttribute("maxlength", "16");
    const name = await asked;
    if (!name) return false;
    if (!shareNameOk(name)) { note = t("That name is too short or too long."); continue; }
    try {
      const r = await invoke("share_claim_name", { name });
      S.settings.share.name = r.name;
      S.settings.share.name_secret = r.secret;
      S.status = await invoke("save_settings", { settings: S.settings });
      if (change) { flash(t("Your publisher name is now {name}", { name: r.name })); if (S.page === "settings" && typeof render === "function") render(); }
      return true;
    } catch (e) { note = t(String(e?.message ?? e)); }
  }
}

/** Right-click menu entries. One clip: Share link (or Copy share link / Open share page / Delete from
 *  cloud once it is uploaded). Several: one entry that uploads those not yet shared. */
function shareMenuItem(clips) {
  const list = (clips || []).filter((c) => c && !c.draft);
  if (!shareOn() || !list.length) return "";
  const btn = (what, arg, icon, label) => `<button data-shm="${what}" data-arg="${shareEsc(arg)}">${icon}<span>${shareEsc(label)}</span></button>`;
  if (list.length > 1) {
    const todo = list.filter((c) => !shareLinks[c.id]);
    return todo.length ? btn("share-many", todo.map((c) => c.id).join(","), SHARE_UP_ICON, t("Upload {n} to cloud", { n: todo.length })) : "";
  }
  const id = list[0].id;
  if (!shareLinks[id]) return btn("share", id, SHARE_ICON, t("Share link"));
  return btn("share", id, SHARE_ICON, t("Copy share link")) + btn("open", id, SHARE_OPEN_ICON, t("Open share page"))
    + btn("offload", id, SHARE_PC_ICON, t("Delete from this PC (keep in cloud)")) + btn("delete", id, SHARE_DEL_ICON, t("Delete from cloud"));
}

/** Ask for the upload key once, when it is missing. True when there is one afterwards. */
async function shareNeedKey() {
  if (shareHasKey()) return true;
  const asked = askText(t("Paste your upload key to turn on share links. It stays on this PC."), "", t("Save key"));
  document.querySelector("#confirm .ask-input")?.setAttribute("maxlength", "200"); // keys are longer than a name
  const key = await asked;
  if (!key || !key.trim()) return false;
  S.settings.share.token = key.trim();
  try { S.status = await invoke("save_settings", { settings: S.settings }); } catch (e) { flash(t(String(e?.message ?? e)), true); return false; }
  return true;
}

async function shareMany(ids) {
  if (!(await shareNeedKey()) || !(await shareNeedName())) return;
  let n = 0, err = "";
  for (const id of ids) {
    try {
      const r = await invoke("share_clip", { id });
      if (r && r.url) shareLinks[id] = r.url;
      if (r && r.job) { shareMine.add(r.job.id); n++; }
    } catch (e) { err = t(String(e?.message ?? e)); }
  }
  if (err && !n) return flash(err, true);
  flash(t("Uploading {n} clips. Each link is ready when its upload ends: right-click the clip to copy it.", { n }));
}

async function shareClip(id) {
  const c = (S.clips || []).find((k) => k.id === id);
  if (!(await shareNeedKey())) return;
  if (!shareLinks[id] && !(await shareNeedName())) return; // copying an existing link needs no name
  try {
    const r = await invoke("share_clip", { id });
    if (r && r.url) {
      shareLinks[id] = r.url;
      return flash(r.copied ? t("Share link copied: {url}", { url: r.url }) : r.url);
    }
    if (r && r.job) {
      shareMine.add(r.job.id);
      if (typeof jobsSet === "function" && typeof jobsList !== "undefined" && !jobsList.some((j) => j.id === r.job.id)) jobsSet([...jobsList, r.job]);
    }
    flash(c && c.size > SHARE_WARN_BYTES
      ? t("Uploading… Large clips may not play inside Discord: trim or compress it if the embed shows no player.")
      : t("Uploading… The link is copied when it is ready."));
  } catch (e) { flash(t(String(e?.message ?? e)), true); }
}

async function shareDelete(id) {
  const only = !!(S.clips || []).find((k) => k.id === id)?.cloud_only;
  const text = only
    ? t("Delete this clip from the cloud? It is not on this PC any more, so it will be gone for good. Redownload it first if you want to keep it.")
    : t("Delete this clip from the cloud? Anyone with the link can no longer watch it. Messages that already show it in Discord may keep the picture for a while. The clip on this PC stays.");
  if (!(await ask(text, only ? t("Delete for good") : t("Delete from cloud")))) return;
  try {
    await invoke("share_delete", { id });
    delete shareLinks[id];
    if (only && typeof refreshClips === "function") await refreshClips(); else shareRepaint();
    flash(t("Deleted from the cloud"));
  } catch (e) { flash(t(String(e?.message ?? e)), true); }
}

/* ---------- Settings ---------- */

function shareSettingsSection() {
  const s = (S.settings && S.settings.share) || {};
  const field = (path, value, ph, type = "text") => `<input class="field" type="${type}" data-set="share.${path}" value="${shareEsc(value || "")}" placeholder="${shareEsc(ph)}" spellcheck="false" autocomplete="off" style="width:340px">`;
  return `<div class="section" data-cat="program"><h2>${t("Share links")}</h2>
    ${row(t("Share links"), t("Optional. Upload a clip to your own Cloudflare account and copy a link that plays inside Discord. Set up the Worker in server/share of Glint's code first. Without an upload key, Glint never uploads anything"), "")}
    ${row(t("Worker address"), t("Where your share Worker runs, such as https://glint-share.yourname.workers.dev"), field("url", s.url, "https://…workers.dev"))}
    ${row(t("Upload key"), t("The UPLOAD_TOKEN you gave the Worker. It stays on this PC"), field("token", s.token, "", "password"))}
    ${row(t("Publisher name"), t("Shown on your shared clips before the game, as in “Rubust · Minecraft”. 2 to 16 characters, and nobody else on this server can use yours"), `<span class="path">${shareEsc(s.name && String(s.name_secret || "").length >= 32 ? s.name : t("Not chosen yet"))}</span><button class="btn sm" data-shm="name">${t("Change")}</button>`)}
    ${row(t("Copy the link when it is ready"), "", `<label class="switch"><input type="checkbox" data-set="share.copy" ${s.copy !== false ? "checked" : ""}><i></i></label>`)}
    ${row(t("Ask to delete from this PC after an upload"), t("A clip you delete this way stays in your library with a cloud tag, and you can download it again"), `<label class="switch"><input type="checkbox" data-set="share.ask_offload" ${s.ask_offload !== false ? "checked" : ""}><i></i></label>`)}
    ${row(t("Check the connection"), `<span id="share-test-out">${t("Asks the Worker how much space is used")}</span>`, `<button class="btn sm" data-shm="test">${t("Check")}</button>`)}
  </div>`;
}

async function shareTest() {
  const out = document.getElementById("share-test-out");
  if (out) out.textContent = t("Checking…");
  try {
    const u = await invoke("share_test");
    const mb = (n) => Math.round((+n || 0) / 1e6);
    const msg = t("Connected. {clips} shared, {used} of {limit} MB used.", { clips: u.clips ?? 0, used: mb(u.used), limit: mb(u.limit) });
    if (out) out.textContent = msg; else flash(msg);
    shareRefresh();
  } catch (e) {
    const msg = t(String(e?.message ?? e));
    if (out) out.textContent = msg; else flash(msg, true);
  }
}

/* ---------- the renders list ---------- */

function shareJobLine(r) {
  const pct = Math.floor(r.pct);
  if (r.job && shareDownloads.has(r.job.id)) {
    switch (r.state) {
      case "queued": return t("Waiting to download");
      case "running": return t("Downloading {pct} %", { pct });
      case "done": return t("Back on this PC");
      default: return "";
    }
  }
  switch (r.state) {
    case "queued": return t("Waiting to upload");
    case "running": return pct < 15 ? t("Preparing the upload…") : t("Uploading {pct} %", { pct });
    case "done": return t("Share link ready");
    default: return "";
  }
}

function shareWatch(list) {
  for (const j of list || []) {
    if (!shareMine.has(j.id) || j.state === "queued" || j.state === "running") continue;
    shareMine.delete(j.id);
    if (j.state === "error") flash(shareDownloads.has(j.id) ? t("The download failed: {message}", { message: t(j.message || "unknown error") }) : t("The upload failed: {message}", { message: t(j.message || "unknown error") }), true);
  }
}

/* ---------- wiring (in the app only) ---------- */
if (typeof document !== "undefined" && typeof window !== "undefined" && window.__TAURI__) {
  // Capture phase: a tag on a card is handled here and the click never reaches the card (which would open the clip).
  document.addEventListener("click", (e) => {
    const b = e.target.closest?.("[data-shm]");
    if (!b || b.disabled) return;
    if (b.closest(".card")) { e.stopPropagation(); e.preventDefault(); }
    const what = b.dataset.shm, id = b.dataset.arg || "";
    if (what === "test") return void shareTest();
    if (what === "name") return void shareNeedKey().then((ok) => ok && shareNeedName(true));
    closeMenu();
    if (what === "redownload") return void shareRedownload(id);
    if (what === "offload") return void shareOffload(id);
    if (what === "share") shareClip(id);
    else if (what === "share-many") shareMany(id.split(",").filter(Boolean));
    else if (what === "open") invoke("share_open", { id }).catch((e2) => flash(t(String(e2?.message ?? e2)), true));
    else if (what === "delete") shareDelete(id);
  }, true);
  // A press on a tag must not start the library's drag-select or open the clip on double click.
  for (const type of ["mousedown", "dblclick"]) document.addEventListener(type, (e) => { if (e.target.closest?.(".card [data-shm]")) e.stopPropagation(); }, true);
  // The link list follows the address: read it at start and whenever the settings are saved.
  setTimeout(shareRefresh, 1500);
  document.addEventListener("change", (e) => { if (e.target?.dataset?.set?.startsWith("share.")) setTimeout(shareRefresh, 600); });
  const ev = window.__TAURI__.event;
  if (ev) {
    ev.listen("clip-shared", ({ payload: p }) => {
      if (!p || !p.url) return;
      shareLinks[p.clip] = p.url;
      shareRepaint();
      if (typeof sfx === "function") sfx("success");
      flash(p.copied ? t("Share link copied: {url}", { url: p.url }) : t("Share link ready: {url}", { url: p.url }));
      // Asked once per upload, only when the setting is on; "Cancel" keeps the clip, and nothing is deleted without a yes.
      const c = (S.clips || []).find((k) => k.id === p.clip);
      if (c && !c.cloud_only && S.settings.share.ask_offload !== false) {
        ask(t("Uploaded.") + " " + shareOffloadText(c), t("Delete from this PC")).then((yes) => { if (yes) shareOffload(p.clip, true); });
      }
    });
    ev.listen("clip-redownloaded", () => { if (typeof sfx === "function") sfx("success"); flash(t("The clip is back on this PC")); });
    ev.listen("jobs-changed", ({ payload }) => shareWatch(payload));
  }
}

if (typeof module === "object" && module.exports) {
  module.exports = { shareMenuItem, shareJobLine, shareCardBadge, shareCloudMenu, shareNameOk };
}
