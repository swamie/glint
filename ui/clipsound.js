"use strict";
/*
 * Settings → Clip sound, for a custom file (0.1.31): the part of the file that is played.
 * A bar shows the file's loudness over time; its two ends are dragged to cut silence (or anything
 * else) before and after the sound. The part is `settings.sound_start` and `settings.sound_end`
 * in seconds (end 0 = to the file's end); src-tauri/src/sound.rs cuts and plays it, at most
 * CS_MAX seconds. app.js puts csTrimRow() into the section and calls csAfterRender() after it
 * drew the page.
 */

const CS_MAX = 15; // seconds: the longest clip sound the backend plays (sound.rs CUSTOM_MAX_SECS)
const CS_MIN = 0.05; // seconds: the shortest part
const csInfo = { file: "", duration: 0, peaks: [], state: "" }; // state: "" | "loading" | "ready" | "failed"
let csDrag = null; // { end: "start" | "end" | "move", x0, a0, b0 }

const csClamp = (x, lo, hi) => Math.min(hi, Math.max(lo, x));
const csFmt = (sec) => `${(Math.round(sec * 100) / 100).toFixed(2)}`;

/** The chosen part [a, b] in seconds for a file `D` seconds long (D 0 = not known yet). */
function csSpan(start, end, D) {
  const top = D > 0 ? D : 3600;
  const a = csClamp(+start || 0, 0, Math.max(0, top - CS_MIN));
  let b = +end || 0;
  if (!(b > a + CS_MIN / 2)) b = top; // 0 (or nothing sensible) = to the end
  b = csClamp(b, a + CS_MIN, Math.min(top, a + CS_MAX));
  return { a, b };
}
/** A drag of one end (or of the whole part) by `dt` seconds from the part [a0, b0]. */
function csDragged(end, a0, b0, dt, D) {
  if (end === "move") { const len = b0 - a0, a = csClamp(a0 + dt, 0, Math.max(0, D - len)); return { a, b: a + len }; }
  if (end === "start") { const a = csClamp(a0 + dt, Math.max(0, b0 - CS_MAX), b0 - CS_MIN); return { a, b: b0 }; }
  return { a: a0, b: csClamp(b0 + dt, a0 + CS_MIN, Math.min(D, a0 + CS_MAX)) };
}

/** The row for the Clip sound section (app.js's row()). */
function csTrimRow() {
  const hint = t("Drag the ends to cut silence before or after the sound. At most {n} seconds are played", { n: CS_MAX });
  const ctl = `<div class="cs-trim" data-cs-trim><canvas class="cs-wave"></canvas><div class="cs-sel"><i data-cs="start"></i><i data-cs="end"></i></div></div>`
    + `<span class="cs-times"></span><button class="btn sm" data-cs-reset>${esc(t("Whole file"))}</button>`;
  return row(t("Part to play"), hint, ctl);
}

/** After the Settings page was drawn: read the file's loudness once, then draw the bar. */
function csAfterRender() {
  const el = document.querySelector(".cs-trim");
  if (!el || !S.settings) return;
  const file = S.settings.sound_file || "";
  if (csInfo.file !== file) {
    Object.assign(csInfo, { file, duration: 0, peaks: [], state: "loading" });
    invoke("editor_audio_info", { path: file }).then((info) => {
      if (csInfo.file !== file) return;
      Object.assign(csInfo, { duration: +info?.duration || 0, peaks: Array.isArray(info?.peaks) ? info.peaks : [], state: +info?.duration > 0 ? "ready" : "failed" });
      csPaint();
    }).catch((e) => {
      if (csInfo.file !== file) return;
      csInfo.state = "failed";
      console.warn("Glint: the sound file could not be read", e);
      csPaint();
    });
  }
  csPaint();
}

/** Draw the loudness, place the chosen part and write its times. */
function csPaint() {
  const el = document.querySelector(".cs-trim");
  if (!el || !S.settings) return;
  const times = el.parentElement.querySelector(".cs-times"), sel = el.querySelector(".cs-sel"), cv = el.querySelector(".cs-wave");
  const ready = csInfo.state === "ready", D = csInfo.duration;
  el.classList.toggle("off", !ready);
  if (!ready) {
    sel.hidden = true;
    if (times) times.textContent = csInfo.state === "failed" ? t("This file could not be read") : t("Reading the file…");
    return;
  }
  const { a, b } = csSpan(S.settings.sound_start, S.settings.sound_end, D);
  sel.hidden = false;
  sel.style.left = `${(a / D) * 100}%`;
  sel.style.width = `${((b - a) / D) * 100}%`;
  if (times) times.textContent = `${csFmt(a)} – ${csFmt(b)} ${t("s")}`;
  // The loudness: one bar per pixel column, mirrored about the middle.
  const r = el.getBoundingClientRect(), dpr = window.devicePixelRatio || 1;
  const W = Math.max(1, Math.round(r.width * dpr)), H = Math.max(1, Math.round(r.height * dpr));
  if (cv.width !== W || cv.height !== H) { cv.width = W; cv.height = H; }
  const ctx = cv.getContext("2d"), peaks = csInfo.peaks;
  ctx.clearRect(0, 0, W, H);
  if (!peaks.length) return;
  const css = getComputedStyle(el), dim = css.getPropertyValue("--cs-dim").trim() || "#556", lit = css.getPropertyValue("--cs-lit").trim() || "#9cf";
  const x0 = (a / D) * W, x1 = (b / D) * W;
  for (let x = 0; x < W; x++) {
    const i0 = Math.floor((x / W) * peaks.length), i1 = Math.max(i0 + 1, Math.floor(((x + 1) / W) * peaks.length));
    let p = 0;
    for (let i = i0; i < i1 && i < peaks.length; i++) p = Math.max(p, peaks[i]);
    const h = Math.max(1, Math.round(Math.min(1, p) * (H - 2)));
    ctx.fillStyle = x >= x0 && x < x1 ? lit : dim;
    ctx.fillRect(x, (H - h) / 2, 1, h);
  }
}

if (typeof document !== "undefined") {
  document.addEventListener("pointerdown", (e) => {
    const bar = e.target.closest?.("[data-cs-trim]");
    if (!bar || e.button !== 0 || csInfo.state !== "ready" || !S.settings) return;
    e.preventDefault();
    const D = csInfo.duration, r = bar.getBoundingClientRect();
    let { a, b } = csSpan(S.settings.sound_start, S.settings.sound_end, D);
    let end = e.target.closest("[data-cs]")?.dataset.cs || (e.target.closest(".cs-sel") ? "move" : "");
    if (!end) {
      // A press outside the part: the nearer end jumps there and is dragged on.
      const at = csClamp(((e.clientX - r.left) / r.width) * D, 0, D);
      end = Math.abs(at - a) <= Math.abs(at - b) ? "start" : "end";
      ({ a, b } = csDragged(end, a, b, at - (end === "start" ? a : b), D));
      S.settings.sound_start = a;
      S.settings.sound_end = b;
      csPaint();
    }
    csDrag = { end, x0: e.clientX, a0: a, b0: b, w: r.width, moved: !e.target.closest(".cs-sel") };
    bar.classList.add("dragging");
  }, true);
  window.addEventListener("pointermove", (e) => {
    if (!csDrag || !S.settings) return;
    const D = csInfo.duration, dt = ((e.clientX - csDrag.x0) / (csDrag.w || 1)) * D;
    const { a, b } = csDragged(csDrag.end, csDrag.a0, csDrag.b0, dt, D);
    if (Math.abs(e.clientX - csDrag.x0) > 1) csDrag.moved = true;
    S.settings.sound_start = Math.round(a * 1000) / 1000;
    S.settings.sound_end = Math.round(b * 1000) / 1000;
    csPaint();
  }, true);
  const csEnd = () => {
    if (!csDrag) return;
    const moved = csDrag.moved;
    csDrag = null;
    document.querySelector(".cs-trim.dragging")?.classList.remove("dragging");
    if (!moved) return;
    // The whole file is saved as "to the end" (0), so a longer file picked later is not cut short.
    if (S.settings.sound_end >= csInfo.duration - 0.005) S.settings.sound_end = 0;
    saveSettings();
    setTimeout(() => invoke("preview_sound"), 350); // hear the part that was chosen
  };
  window.addEventListener("pointerup", csEnd, true);
  window.addEventListener("pointercancel", csEnd, true);
  document.addEventListener("click", (e) => {
    if (!e.target.closest?.("[data-cs-reset]") || !S.settings) return;
    S.settings.sound_start = 0;
    S.settings.sound_end = 0;
    saveSettings();
    csPaint();
    setTimeout(() => invoke("preview_sound"), 350);
  });
}

// Lets the pure functions be tested under Node; ignored in the app.
if (typeof module !== "undefined" && module.exports) module.exports = { CS_MAX, CS_MIN, csSpan, csDragged };
