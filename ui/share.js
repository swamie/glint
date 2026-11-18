"use strict";
// Share links (backend: share.rs; server: server/share). Optional: everything here stays hidden
// until Settings → Share links has the address of the user's own Cloudflare Worker.
// - shareMenuItem(clips): "Share link" / "Copy share link" and "Delete share link" in a clip's
//   right-click menu (one clip at a time).
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

function shareRefresh() {
  if (!shareHasKey()) { shareLinks = {}; return Promise.resolve(); }
  return invoke("share_links").then((m) => { shareLinks = m || {}; }).catch(() => {});
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
  return btn("share", id, SHARE_ICON, t("Copy share link")) + btn("open", id, SHARE_OPEN_ICON, t("Open share page")) + btn("delete", id, SHARE_DEL_ICON, t("Delete from cloud"));
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
  if (!(await shareNeedKey())) return;
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
  if (!(await ask(t("Delete this clip from the cloud? Anyone with the link can no longer watch it. Messages that already show it in Discord may keep the picture for a while. The clip on this PC stays."), t("Delete from cloud")))) return;
  try {
    await invoke("share_delete", { id });
    delete shareLinks[id];
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
    ${row(t("Your name"), t("Shown on the embed before the game, as in “Rubust · Minecraft”. Empty shows the game alone"), field("name", s.name, t("Name")))}
    ${row(t("Copy the link when it is ready"), "", `<label class="switch"><input type="checkbox" data-set="share.copy" ${s.copy !== false ? "checked" : ""}><i></i></label>`)}
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
    if (j.state === "error") flash(t("The upload failed: {message}", { message: t(j.message || "unknown error") }), true);
  }
}

/* ---------- wiring (in the app only) ---------- */
if (typeof document !== "undefined" && typeof window !== "undefined" && window.__TAURI__) {
  document.addEventListener("click", (e) => {
    const b = e.target.closest?.("[data-shm]");
    if (!b || b.disabled) return;
    const what = b.dataset.shm, id = b.dataset.arg || "";
    if (what === "test") return void shareTest();
    closeMenu();
    if (what === "share") shareClip(id);
    else if (what === "share-many") shareMany(id.split(",").filter(Boolean));
    else if (what === "open") invoke("share_open", { id }).catch((e2) => flash(t(String(e2?.message ?? e2)), true));
    else if (what === "delete") shareDelete(id);
  });
  // The link list follows the address: read it at start and whenever the settings are saved.
  setTimeout(shareRefresh, 1500);
  document.addEventListener("change", (e) => { if (e.target?.dataset?.set?.startsWith("share.")) setTimeout(shareRefresh, 600); });
  const ev = window.__TAURI__.event;
  if (ev) {
    ev.listen("clip-shared", ({ payload: p }) => {
      if (!p || !p.url) return;
      shareLinks[p.clip] = p.url;
      if (typeof sfx === "function") sfx("success");
      flash(p.copied ? t("Share link copied: {url}", { url: p.url }) : t("Share link ready: {url}", { url: p.url }));
    });
    ev.listen("jobs-changed", ({ payload }) => shareWatch(payload));
  }
}

if (typeof module === "object" && module.exports) {
  module.exports = { shareMenuItem, shareJobLine };
}
