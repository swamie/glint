"use strict";
// Upscale for YouTube (0.1.24, backend: upscale.rs). A clip's right-click menu → "Upscale for
// YouTube…" opens a small dialog (target, Sharp / Smooth, quality); each clip becomes a background
// job of kind "upscale" (the renders list shows it) whose result is a new library clip
// "<title> (4K)". The last choices are kept in S.settings.upscale.
// Loaded before app.js: at load time this file only defines things. Names start with `upscale` / `UP_`.

const UP_HEIGHTS = [1080, 1440, 2160];   // as upscale.rs HEIGHTS
const UP_ICON = '<svg viewBox="0 0 24 24"><rect x="3" y="11" width="10" height="10" rx="1.500"/><path d="M14 3h7v7M21 3l-8 8M3 7V4a1 1 0 0 1 1-1h3"/></svg>';
// Rough bits per pixel and frame of each quality, for the size the dialog expects (upscale.rs codec_args).
const UP_BPP = { best: 0.16, high: 0.1, smaller: 0.05 };
let upscaleMenuIds = [];                  // the clips the last menu was about
const upscaleMine = new Set();            // job ids queued from here: their failures are flashed

const upLabel = (h) => (h === 2160 ? "4K" : `${h}p`);
const upOpts = () => {
  const o = (typeof S !== "undefined" && S.settings && S.settings.upscale) || {};
  return {
    height: UP_HEIGHTS.includes(+o.height) ? +o.height : 2160,
    filter: o.filter === "smooth" ? "smooth" : "sharp",
    quality: ["best", "high", "smaller"].includes(o.quality) ? o.quality : "high",
    encoder: typeof o.encoder === "string" && o.encoder ? o.encoder : "auto",
  };
};

/** The "Upscale for YouTube…" item of a clip's right-click menu (one clip or several). */
function upscaleMenuItem(clips) {
  if (!Array.isArray(clips) || !clips.length) return "";
  upscaleMenuIds = clips.map((c) => c.id);
  const tallest = Math.max(...UP_HEIGHTS);
  if (clips.every((c) => +c.height >= tallest)) {
    const why = clips.length === 1 ? t("This clip is already 4K or larger") : t("These clips are already 4K or larger");
    return `<button data-up="open" disabled aria-disabled="true" title="${esc(why)}" style="opacity:.5;cursor:default">${UP_ICON}<span>${esc(t("Upscale for YouTube…"))}</span></button>`;
  }
  return `<button data-up="open" title="${esc(t("Scale the clip up to 1440p or 4K before you upload it, so YouTube gives it more bitrate."))}">${UP_ICON}<span>${esc(t("Upscale for YouTube…"))}</span></button>`;
}

/** Roughly how large the results of `clips` get at `o`, in bytes. */
function upscaleGuess(clips, o) {
  let bytes = 0;
  for (const c of clips) {
    const h = o.height, w = c.width > 0 && c.height > 0 ? Math.round((c.width * h) / c.height / 2) * 2 : Math.round((h * 16) / 9);
    const fps = +c.fps > 0 ? +c.fps : 60;
    const kbps = Math.min(250000, (w * h * fps * (UP_BPP[o.quality] || UP_BPP.high)) / 1000);
    bytes += ((kbps * 1000) / 8) * (+c.duration || 0) + (+c.size || 0) * 0.05; // plus the sound, about as it was
  }
  return bytes;
}

/** A backend refusal or failure in the user's language. */
function upscaleError(e) {
  const msg = String(e?.message ?? e).trim();
  let m = /^This clip is already (\d+)p, as tall as (\S+) or taller/.exec(msg);
  if (m) return t("This clip is already {h}p, as tall as {target} or taller, so upscaling would not help", { h: +m[1], target: m[2] });
  m = /^Could not upscale the clip: (.*)$/.exec(msg);
  if (m) return t("Could not upscale the clip: {reason}", { reason: m[1] });
  // upscale.rs's other plain sentences.
  if (msg === "That clip is already being upscaled") return t("That clip is already being upscaled");
  if (msg === "The clip's video file is missing") return t("The clip's video file is missing");
  if (msg === "This file has no picture to upscale") return t("This file has no picture to upscale");
  if (msg === "Could not read the clip (ffprobe.exe was not found)") return t("Could not read the clip (ffprobe.exe was not found)");
  return t(msg);
}

/** The dialog for clips `ids`. Each clip that is not yet as tall as the target becomes one job. */
function upscaleOpen(ids) {
  const clips = (ids || []).map((id) => S.clips.find((c) => c.id === id)).filter(Boolean);
  if (!clips.length) return;
  const o = upOpts();
  const tooTall = (h) => clips.every((c) => +c.height >= h);
  if (tooTall(o.height)) o.height = UP_HEIGHTS.find((h) => !tooTall(h)) || 2160;
  const el = $("#confirm");
  const paint = () => {
    const todo = clips.filter((c) => !(+c.height >= o.height));
    const left = clips.length - todo.length;
    const size = fmtSize(upscaleGuess(todo, o));
    const now = fmtSize(todo.reduce((s, c) => s + (+c.size || 0), 0));
    const heights = UP_HEIGHTS.map((h) => `<button type="button" class="${o.height === h ? "active" : ""}" data-up-h="${h}"${tooTall(h) ? ` disabled title="${esc(t("Already this tall"))}"` : ""}>${esc(upLabel(h))}</button>`).join("");
    const filt = (v, name, line) => `<label class="up-choice${o.filter === v ? " on" : ""}"><input type="radio" name="up-filter" value="${v}"${o.filter === v ? " checked" : ""}><span><b>${esc(name)}</b><span class="small">${esc(line)}</span></span></label>`;
    el.innerHTML = `<div class="confirm-box up-box">
      <p class="up-title">${esc(clips.length === 1 ? t("Upscale for YouTube") : t("Upscale {clips} for YouTube", { clips: plural(clips.length) }))}</p>
      <div class="up-why small">
        <span>${esc(t("YouTube gives larger uploads more bitrate, so a clip uploaded in 1440p or 4K looks clearer there, even when you watch it in 1080p."))}</span>
        <span>${esc(t("Upscaling does not add detail: the picture is the same, only bigger."))}</span>
        <span>${esc(t("Do it last, right before you upload. The file gets much bigger: roughly {size} (now {now}).", { size, now }))}</span>
      </div>
      <div class="up-row"><span class="up-label">${esc(t("Size"))}</span><div class="seg up-seg">${heights}</div></div>
      <div class="up-row up-top"><span class="up-label">${esc(t("Scaling"))}</span><div class="up-choices">
        ${filt("sharp", t("Sharp"), t("Every pixel becomes a block of pixels. Crisp edges, best for games. Recommended."))}
        ${filt("smooth", t("Smooth"), t("Blends neighbouring pixels (Lanczos). Softer, for camera footage."))}
      </div></div>
      <div class="up-row"><span class="up-label">${esc(t("Quality"))}</span>
        <select class="field up-q" data-up-q aria-label="${esc(t("Quality"))}">
          <option value="best"${o.quality === "best" ? " selected" : ""}>${esc(t("Best (largest file)"))}</option>
          <option value="high"${o.quality === "high" ? " selected" : ""}>${esc(t("High (recommended)"))}</option>
          <option value="smaller"${o.quality === "smaller" ? " selected" : ""}>${esc(t("Smaller file"))}</option>
        </select></div>
      ${left ? `<div class="small up-left">${esc(left === 1 ? t("1 clip is already this tall and is left out.") : t("{n} clips are already this tall and are left out.", { n: left }))}</div>` : ""}
      <div class="confirm-row"><button class="btn" data-c="no">${esc(t("Cancel"))}</button><button class="btn primary" data-c="yes"${todo.length ? "" : " disabled"}>${esc(todo.length > 1 ? t("Upscale {clips}", { clips: plural(todo.length) }) : t("Upscale"))}</button></div></div>`;
  };
  const done = (go) => {
    el.hidden = true; el.innerHTML = ""; el.onclick = null; el.onchange = null; askKey = null;
    if (!go) return;
    S.settings.upscale = { ...(S.settings.upscale || {}), ...o };
    saveSettings();
    upscaleStart(clips.filter((c) => !(+c.height >= o.height)).map((c) => c.id), { ...o });
  };
  // "pass" lets other keys through (arrows in the list, Space on a choice).
  askKey = (e) => { if (e.code === "Enter" || e.code === "NumpadEnter") done(true); else if (e.code === "Escape") done(false); else return "pass"; };
  el.onchange = (e) => {
    if (e.target.name === "up-filter") o.filter = e.target.value === "smooth" ? "smooth" : "sharp";
    else if (e.target.matches("[data-up-q]")) o.quality = e.target.value;
    else return;
    paint();
  };
  el.onclick = (e) => {
    const h = e.target.closest("[data-up-h]");
    if (h) { if (!h.disabled) { o.height = +h.dataset.upH; paint(); } return; }
    const b = e.target.closest("[data-c]");
    if (b) { if (!b.disabled) done(b.dataset.c === "yes"); }
    else if (e.target === el) done(false);
  };
  paint();
  el.hidden = false;
}

/** Queue the upscales (one job per clip) and say so. */
async function upscaleStart(ids, opts) {
  if (!ids.length) return;
  try {
    const jobs = (await invoke("upscale_clips", { ids, opts })) || [];
    for (const j of jobs) if (j && j.id) upscaleMine.add(j.id);
    // Shown at once; the backend's "jobs-changed" brings the real list right after.
    if (typeof jobsSet === "function" && Array.isArray(jobsList)) jobsSet([...jobsList, ...jobs.filter((j) => j && !jobsList.some((x) => x.id === j.id))]);
    sfx("launch");
    const target = upLabel(opts.height);
    flash(jobs.length === ids.length
      ? t("Upscaling {clips} to {target}. The new clip appears in your library when it is done.", { clips: plural(ids.length), target })
      : t("Upscaling {done} of {clips} to {target}. The others could not be queued.", { done: jobs.length, clips: plural(ids.length), target }));
  } catch (e) {
    flash(upscaleError(e), true);
  }
}

/** The status line of an upscale in the renders list (jobs.js). */
function upscaleJobLine(r) {
  if (!r || r.kind !== "upscale") return "";
  const pct = Math.floor(r.pct);
  switch (r.state) {
    case "queued": return t("Waiting to upscale");
    case "running": return r.job && r.job.stage === "x264" ? t("Upscaling {pct} % (on the processor, slower)", { pct }) : t("Upscaling {pct} %", { pct });
    case "done": return t("Upscaled");
    case "error": return r.message ? t("Failed: {message}", { message: upscaleError(r.message) }) : t("Failed");
    default: return "";
  }
}

/* ---------- wiring (in the app only; node checks load this file without a page) ---------- */
if (typeof document !== "undefined" && typeof window !== "undefined" && window.__TAURI__) {
  document.addEventListener("click", (e) => {
    const b = e.target.closest?.("[data-up]");
    if (!b || b.disabled || !b.closest("#menu")) return;
    closeMenu();
    if (b.dataset.up === "open") upscaleOpen(upscaleMenuIds.slice());
  });
  const ev = window.__TAURI__.event;
  if (ev) {
    // A finished upscale: { clip, size, encoder, secs, job_id, source }.
    ev.listen("clip-upscaled", ({ payload: p }) => {
      if (!p || !p.clip) return;
      upscaleMine.delete(p.job_id);
      const i = S.clips.findIndex((c) => c.id === p.clip.id);
      if (i >= 0) S.clips[i] = p.clip; else S.clips.push(p.clip);
      render();
      sfx("success");
      flash(t("Upscaled to {target} ({size}). The new clip is in your library.", { target: upLabel(+p.clip.height || 0), size: fmtSize(p.size || p.clip.size || 0) }));
    });
    ev.listen("jobs-changed", ({ payload }) => {
      for (const j of Array.isArray(payload) ? payload : []) {
        if (j.kind !== "upscale" || !upscaleMine.has(j.id) || (j.state !== "error" && j.state !== "cancelled")) continue;
        upscaleMine.delete(j.id);
        if (j.state === "error") flash(t("Could not upscale {title}: {reason}", { title: j.title, reason: upscaleError(j.message || "") }), true);
      }
    });
  }
}
