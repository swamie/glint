"use strict";
// Export as GIF (0.1.24, gif.rs). Loaded before app.js: nothing here touches app.js's globals at load time.
//
// - gifMenuItem(clips): "Export as GIF…" in a clip's right-click menu (one clip or several).
// - gifOpen(ids): the dialog (size, frames a second, size limit, copy), remembered in S.settings.gif.
// - gifJobLine(row): the line of a "gif" job in the renders list (jobs.js).
// - "gif-done" { path, size, source, job_id, copied, over, max_mb }: a message with Show in folder / Copy.
// Names here start with `gif` / `GIF_`; markup uses `gif-` classes and `data-gif…` attributes.

const GIF_ICON = '<svg viewBox="0 0 24 24"><rect x="3" y="5" width="18" height="14" rx="2.500"/><path d="M10 10.200H8.400a1.400 1.400 0 0 0-1.400 1.400v.8a1.400 1.400 0 0 0 1.400 1.400H10v-1.600H9M12.500 10.200v3.600M15 13.800v-3.600h2.300M15 12h1.800"/></svg>';
const GIF_WIDTHS = [0, 720, 480, 320];
const GIF_RATES = [10, 15, 20, 30, 50];
const GIF_LIMITS = [0, 10, 25, 50];
/** Clips longer than this (seconds) get the warning about very large files. */
const GIF_LONG_SECS = 20;
const gifMine = new Set(); // ids of the GIF jobs this window queued: a failure among them is flashed

const gifEsc = (s) => (typeof esc === "function" ? esc(s) : String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]));
const gifDur = (s) => (typeof fmtDur === "function" ? fmtDur(s) : `${Math.floor(s / 60)}:${String(Math.round(s) % 60).padStart(2, "0")}`);
const gifSize = (b) => (typeof fmtSize === "function" ? fmtSize(b) : `${(b / 1e6).toFixed(1)} MB`);

/** The dialog's last choices, with defaults for anything missing. */
function gifOpts() {
  const g = (typeof S !== "undefined" && S.settings && S.settings.gif) || {};
  return {
    width: GIF_WIDTHS.includes(+g.width) ? +g.width : 480,
    fps: GIF_RATES.includes(+g.fps) ? +g.fps : 20,
    max_mb: GIF_LIMITS.includes(+g.max_mb) ? +g.max_mb : 0,
    copy: !!g.copy,
  };
}

/** The clip's right-click menu entry. Clips that are not finished videos (drafts) get none. */
function gifMenuItem(clips) {
  const list = (clips || []).filter((c) => c && !c.draft);
  if (!list.length) return "";
  const label = list.length > 1 ? t("Export {n} as GIF…", { n: list.length }) : t("Export as GIF…");
  return `<button data-gifm="open" data-arg="${gifEsc(list.map((c) => c.id).join(","))}">${GIF_ICON}<span>${gifEsc(label)}</span></button>`;
}

/** The one line under the dialog's choices: no sound, the length, a warning for long clips. */
function gifNote(clips) {
  const long = clips.reduce((m, c) => Math.max(m, +c.duration || 0), 0);
  let line = clips.length === 1
    ? t("A GIF has no sound. The whole clip ({len}) becomes the GIF, saved beside it.", { len: gifDur(long) })
    : t("A GIF has no sound. Each whole clip becomes its own GIF, saved beside it.");
  if (long > GIF_LONG_SECS) line += " " + (clips.length === 1
    ? t("Long clips make very large GIFs: trim it to the moment first.")
    : t("The longest is {len}: long clips make very large GIFs, trim them first.", { len: gifDur(long) }));
  return { line, warn: long > GIF_LONG_SECS };
}

/** The dialog for clips `ids` (from the menu). Queues one job per clip. */
function gifOpen(ids) {
  const clips = (S.clips || []).filter((c) => ids.includes(c.id) && !c.draft);
  if (!clips.length) return;
  const o = gifOpts();
  const one = clips.length === 1;
  const own = one && clips[0].width ? t("The clip's own ({w} px)", { w: clips[0].width }) : t("Each clip's own");
  const opt = (v, label, cur) => `<option value="${v}"${v === cur ? " selected" : ""}>${gifEsc(label)}</option>`;
  const widths = GIF_WIDTHS.map((w) => opt(w, w ? t("{w} px wide", { w }) : own, o.width)).join("");
  const rates = GIF_RATES.map((f) => opt(f, t("{n} a second", { n: f }), o.fps)).join("");
  const limits = GIF_LIMITS.map((m) => opt(m, m ? t("{n} MB", { n: m }) : t("No limit"), o.max_mb)).join("");
  const note = gifNote(clips);
  const title = one ? t("Export as GIF") : t("Export {n} clips as GIF", { n: clips.length });
  const el = $("#confirm");
  el.innerHTML = `<div class="confirm-box gif-dlg" role="dialog" aria-modal="true" aria-labelledby="gif-dlg-h">
    <h3 id="gif-dlg-h">${gifEsc(title)}</h3>
    ${one ? `<div class="gif-clip" title="${gifEsc(clips[0].title)}">${gifEsc(clips[0].title)}</div>` : ""}
    <div class="gif-grid">
      <label for="gif-w">${gifEsc(t("Size"))}</label><select id="gif-w" class="field" data-gif="width">${widths}</select>
      <label for="gif-f">${gifEsc(t("Frames a second"))}</label><select id="gif-f" class="field" data-gif="fps">${rates}</select>
      <label for="gif-l">${gifEsc(t("Size limit"))}</label><select id="gif-l" class="field" data-gif="max_mb">${limits}</select>
    </div>
    ${one ? `<label class="check gif-copy"><input type="checkbox" data-gif="copy"${o.copy ? " checked" : ""}><span>${gifEsc(t("Copy to the clipboard when done"))}</span></label>` : ""}
    <p class="gif-note${note.warn ? " warn" : ""}">${gifEsc(note.line)}</p>
    <p class="gif-hint">${gifEsc(t("Smaller and slower GIFs are smaller files. A GIF over its size limit is made again smaller."))}</p>
    <div class="confirm-row"><button type="button" class="btn" data-gif-c="no">${gifEsc(t("Cancel"))}</button><button type="button" class="btn primary" data-gif-c="yes">${gifEsc(one ? t("Export GIF") : t("Export {n} GIFs", { n: clips.length }))}</button></div>
  </div>`;
  el.hidden = false;
  const read = () => {
    const v = (k) => +el.querySelector(`[data-gif="${k}"]`).value;
    const copy = el.querySelector('[data-gif="copy"]');
    return { width: v("width"), fps: v("fps"), max_mb: v("max_mb"), copy: copy ? copy.checked : o.copy };
  };
  const close = () => { el.hidden = true; el.innerHTML = ""; el.onclick = null; askKey = null; };
  const go = async () => {
    const opts = read();
    close();
    S.settings.gif = { ...(S.settings.gif || {}), ...opts };
    saveSettings();
    try {
      const jobs = (await invoke("gif_export", { ids: clips.map((c) => c.id), opts: { ...opts, copy: one && opts.copy } })) || [];
      for (const j of jobs) gifMine.add(j.id);
      if (typeof sfx === "function") sfx("launch");
      const skipped = clips.length - jobs.length;
      flash(skipped > 0
        ? t("Making {n} GIFs. {k} could not start (already being made).", { n: jobs.length, k: skipped })
        : jobs.length === 1 ? t("Making the GIF. The renders list shows how far it is.") : t("Making {n} GIFs. The renders list shows how far they are.", { n: jobs.length }));
    } catch (e) { fail(e); }
  };
  // Glint's prompts take every key through askKey; "pass" lets it through (Space ticks the box, arrows move in a list).
  askKey = (e) => {
    if (e.code === "Escape") { e.preventDefault(); close(); return; }
    if (e.code === "Enter" || e.code === "NumpadEnter") { e.preventDefault(); go(); return; }
    return "pass";
  };
  el.onclick = (e) => {
    const b = e.target.closest("[data-gif-c]");
    if (b) return b.dataset.gifC === "yes" ? go() : close();
    if (e.target === el) close();
  };
  el.querySelector('[data-gif-c="yes"]').focus();
}

/** The renders list's line for a "gif" job ("" leaves it to jobs.js: failures and cancels). */
function gifJobLine(r) {
  const pct = Math.floor(r.pct);
  const stage = (r.job && r.job.stage) || "";
  switch (r.state) {
    case "queued": return t("Waiting to make a GIF");
    case "running":
      if (stage === "smaller") return t("Making the GIF smaller {pct} %", { pct });
      return pct < 1 ? t("Starting the GIF…") : t("Making a GIF {pct} %", { pct });
    case "done": return stage === "over" ? t("GIF saved, over its size limit") : t("GIF saved");
    default: return "";
  }
}

/** The message when a GIF is done, with Show in folder and (unless it was copied) Copy. */
function gifDoneBar(p) {
  document.getElementById("gif-done")?.remove();
  const size = gifSize(p.size || 0);
  const text = p.over
    ? t("GIF saved ({size}), but it is still over {mb} MB at the smallest size. Trim the clip for a smaller GIF.", { size, mb: p.max_mb })
    : p.copied ? t("GIF saved and copied ({size}). Paste it anywhere.", { size }) : t("GIF saved ({size})", { size });
  const bar = document.createElement("div");
  bar.id = "gif-done";
  bar.className = "flash gif-done" + (p.over ? " bad" : "");
  bar.setAttribute("role", "status");
  bar.innerHTML = `<span>${gifEsc(text)}</span><button type="button" class="btn sm" data-gif-act="reveal">${gifEsc(t("Show in folder"))}</button>${p.copied ? "" : `<button type="button" class="btn sm" data-gif-act="copy">${gifEsc(t("Copy"))}</button>`}`;
  document.body.append(bar);
  const timer = setTimeout(() => bar.remove(), 10000);
  bar.addEventListener("click", async (e) => {
    const b = e.target.closest("[data-gif-act]");
    if (!b) return;
    clearTimeout(timer);
    bar.remove();
    try {
      if (b.dataset.gifAct === "reveal") await invoke("compress_reveal", { path: p.path });
      else { await invoke("compress_copy", { path: p.path }); flash(t("GIF copied. Paste it anywhere.")); }
    } catch (err) { fail(err); }
  });
}

/** A GIF job of this window failed: say why (the result itself comes as "gif-done"). */
function gifWatch(list) {
  for (const j of list || []) {
    if (!gifMine.has(j.id) || j.state === "queued" || j.state === "running") continue;
    gifMine.delete(j.id);
    if (j.state === "error") flash(t("The GIF failed: {message}", { message: t(j.message || "unknown error") }), true);
  }
}

/* ---------- wiring (in the app only) ---------- */
if (typeof document !== "undefined" && typeof window !== "undefined" && window.__TAURI__) {
  document.addEventListener("click", (e) => {
    const b = e.target.closest?.("[data-gifm]");
    if (!b || b.disabled) return;
    closeMenu();
    const ids = (b.dataset.arg || "").split(",").filter(Boolean);
    if (b.dataset.gifm === "open") gifOpen(ids);
  });
  const ev = window.__TAURI__.event;
  if (ev) {
    ev.listen("gif-done", ({ payload: p }) => {
      if (!p || !p.path) return;
      gifMine.delete(p.job_id);
      if (typeof sfx === "function") sfx("success");
      gifDoneBar(p);
    });
    ev.listen("jobs-changed", ({ payload }) => gifWatch(payload));
  }
}

if (typeof module === "object" && module.exports) {
  module.exports = { gifOpts, gifMenuItem, gifNote, gifJobLine, GIF_WIDTHS, GIF_RATES, GIF_LIMITS };
}
