"use strict";
// Clip viewer, player and editor. Loaded before app.js: nothing here may touch
// app.js globals (S, $, invoke, ICON, ...) at load time, only inside functions.
// The edit itself lives in an edit model (editmodel.js, em... functions); the
// multi-track timeline is timeline.js (mountTimeline). This file previews the
// model (video, cut skipping, audio levels, added files), runs the side column
// (speed, colour, export) and the editor's keyboard shortcuts.
// Naming rule: no local may be called `t`, it would hide the translation function t().

/* ---------- editor data ---------- */
const ED_SPEEDS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4];
const ED_COLOR_KEYS = ["brightness", "contrast", "saturation", "gamma"];
const ED_NO_COLOR = { brightness: 1, contrast: 1, saturation: 1, gamma: 1 };
// Names are English keys, shown through t().
const ED_PRESETS = [
  { name: "None", ...ED_NO_COLOR },
  { name: "Vivid", brightness: 1, contrast: 1.08, saturation: 1.35, gamma: 1 },
  { name: "Punchy", brightness: 1, contrast: 1.25, saturation: 1.15, gamma: 0.95 },
  { name: "Soft", brightness: 1.02, contrast: 0.88, saturation: 0.9, gamma: 1.08 },
  { name: "Bright", brightness: 1.1, contrast: 1, saturation: 1.05, gamma: 1.15 },
  { name: "Moody", brightness: 0.94, contrast: 1.15, saturation: 0.8, gamma: 0.9 },
  { name: "Black and white", brightness: 1, contrast: 1.12, saturation: 0, gamma: 1 },
];
// key, label, min, max, how the value is shown
const ED_SLIDERS = [
  ["brightness", "Brightness", 0.5, 1.5, "pct"],
  ["contrast", "Contrast", 0.5, 1.5, "pct"],
  ["saturation", "Saturation", 0, 2, "pct"],
  ["gamma", "Gamma", 0.5, 2, "num"],
];
const ED_DRIFT = 0.08;          // seconds a preview audio element may drift from the video before it is re-seeked
const ED_GAIN_STEP = 0.1;       // Up / Down change a segment's level by this much
const ED_ZOOM_STEP = 1.5;       // + / - zoom factor
const ED_THUMBS = 30;           // film strip frames asked from editor_thumbs
const ED_LOAD_TIMEOUT = 15000;  // ms an audio element may take to load
const ED_CHEV = '<svg class="e-chev" viewBox="0 0 24 24"><path d="m7 10 5 5 5-5"/></svg>';

const edFmtVal = (key, x) => (ED_SLIDERS.find((s) => s[0] === key)[4] === "pct" ? `${Math.round(x * 100)}%` : x.toFixed(2));
const edSameColor = (a, b) => ED_COLOR_KEYS.every((k) => Math.abs(a[k] - b[k]) < 0.005);
const edBaseName = (path) => String(path).split(/[\\/]/).pop() || String(path);
/** Used to tell whether an edit changed the model (so a no-op leaves no undo step). */
const edState = (m) => JSON.stringify([m.cuts, m.removed, m.audio, m.speed, m.color]);
/** A text field: keys typed there are never shortcuts. Sliders and check boxes are not text fields. */
const edTextTarget = (node) => !!node?.matches?.('textarea, select, [contenteditable=""], [contenteditable="true"], input:not([type="range"]):not([type="checkbox"]):not([type="radio"]):not([type="button"])');

/** CSS filter for the live colour preview, applied in the same order the export uses:
 *  brightness, contrast, saturation, then gamma (an SVG filter, CSS has none). */
function edFilterCss(col) {
  const f = [];
  if (col.brightness !== 1) f.push(`brightness(${col.brightness})`);
  if (col.contrast !== 1) f.push(`contrast(${col.contrast})`);
  if (col.saturation !== 1) f.push(`saturate(${col.saturation})`);
  if (col.gamma !== 1) f.push("url(#e-gamma)");
  return f.join(" ") || "none";
}

/** The user's saved colour presets, skipping malformed entries. */
function edUserPresets() {
  const list = S.settings?.color_presets;
  return Array.isArray(list) ? list.filter((p) => p && typeof p.name === "string" && ED_COLOR_KEYS.every((k) => Number.isFinite(p[k]))) : [];
}

/** The tracks to edit when editor_audio_tracks fails: same rows, no files (the preview then uses the clip's own audio). */
function edFallbackTracks(c) {
  const n = c.audio_tracks || 0;
  if (n === 3) return [{ index: 0, title: t("Mix"), path: "" }, { index: 1, title: t("Desktop"), path: "" }, { index: 2, title: t("Microphone"), path: "" }];
  return Array.from({ length: n }, (_, i) => ({ index: i, title: n === 1 ? t("Audio") : t("Track {n}", { n: i + 1 }), path: "" }));
}

/** Where the playhead should land for source time `at` while editing: never inside a cut-out part.
 *  Moving forward (or a plain seek) goes to the next kept part, moving back to the end of the previous one.
 *  Past the last kept part it stops on that part's last frame. */
function edLand(m, at, back, frame) {
  const kept = emKept(m);
  if (!kept.length) return at;
  if (back) {
    for (let i = kept.length - 1; i >= 0; i--) if (at >= kept[i].start - 1e-6) return Math.min(at, Math.max(kept[i].start, kept[i].end - frame));
    return kept[0].start;
  }
  const p = emPlayable(m, at);
  const last = kept[kept.length - 1];
  return p ?? Math.max(last.start, last.end - frame);
}

/* ---------- keyboard shortcuts (edit mode) ---------- */
// Key names shown in the Shortcuts pop-up, by KeyboardEvent.code. Words go through t() when shown.
const ED_KEY_NAMES = { Space: "Space", Delete: "Del", Backspace: "Backspace", ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right", BracketLeft: "[", BracketRight: "]", Comma: ",", Period: ".", Equal: "+", NumpadAdd: "+", Minus: "-", NumpadSubtract: "-", Escape: "Esc", Home: "Home", End: "End" };
const edKeyName = (code) => ED_KEY_NAMES[code] || code.replace(/^Key/, "").replace(/^Digit/, "");
/** One key combination. `codes`: space-separated KeyboardEvent.code values that all count. shift: true, false or null (either). */
const edCombo = (codes, { ctrl = false, shift = false } = {}) => ({ codes: codes.split(" "), ctrl, shift });
// The one table of editor shortcuts: the key handlers match against it and the
// Shortcuts pop-up is built from it. `cmd` is run by the player's command();
// `once` ignores auto-repeat. Keep each group's rows together.
const ED_KEYS = [
  { group: "Playback", what: "Play or pause", cmd: "play", once: true, keys: [edCombo("Space")] },
  { group: "Playback", what: "Back 5 seconds", cmd: "back5", keys: [edCombo("ArrowLeft")] },
  { group: "Playback", what: "Forward 5 seconds", cmd: "fwd5", keys: [edCombo("ArrowRight")] },
  { group: "Playback", what: "Back 1 second", cmd: "back1", keys: [edCombo("ArrowLeft", { shift: true })] },
  { group: "Playback", what: "Forward 1 second", cmd: "fwd1", keys: [edCombo("ArrowRight", { shift: true })] },
  { group: "Playback", what: "Back one frame", cmd: "framePrev", keys: [edCombo("Comma")] },
  { group: "Playback", what: "Forward one frame", cmd: "frameNext", keys: [edCombo("Period")] },
  { group: "Playback", what: "Jump to the previous cut point", cmd: "cutPrev", keys: [edCombo("BracketLeft")] },
  { group: "Playback", what: "Jump to the next cut point", cmd: "cutNext", keys: [edCombo("BracketRight")] },
  { group: "Playback", what: "Go to the start", cmd: "home", keys: [edCombo("Home")] },
  { group: "Playback", what: "Go to the end", cmd: "end", keys: [edCombo("End")] },
  { group: "Playback", what: "Loop on or off", cmd: "loop", once: true, keys: [edCombo("KeyL")] },
  { group: "Playback", what: "Fullscreen", cmd: "full", once: true, keys: [edCombo("KeyF")] },
  { group: "Editing", what: "Split at the playhead", cmd: "split", once: true, keys: [edCombo("KeyS")] },
  { group: "Editing", what: "Cut out the selected segment, or restore it", cmd: "remove", once: true, keys: [edCombo("Delete"), edCombo("Backspace")] },
  { group: "Editing", what: "Mute or unmute the selected audio segment", cmd: "mute", once: true, keys: [edCombo("KeyM")] },
  { group: "Editing", what: "Raise the selected audio segment by 10%", cmd: "louder", keys: [edCombo("ArrowUp")] },
  { group: "Editing", what: "Lower the selected audio segment by 10%", cmd: "quieter", keys: [edCombo("ArrowDown")] },
  { group: "Editing", what: "Cut out everything before the playhead", cmd: "trimStart", once: true, keys: [edCombo("KeyI")] },
  { group: "Editing", what: "Cut out everything from the playhead on", cmd: "trimEnd", once: true, keys: [edCombo("KeyO")] },
  { group: "Editing", what: "Undo", cmd: "undo", keys: [edCombo("KeyZ", { ctrl: true })] },
  { group: "Editing", what: "Redo", cmd: "redo", keys: [edCombo("KeyY", { ctrl: true }), edCombo("KeyZ", { ctrl: true, shift: true })] },
  { group: "Editing", what: "Export", cmd: "export", once: true, keys: [edCombo("KeyE", { ctrl: true })] },
  { group: "Timeline", what: "Zoom in", cmd: "zoomIn", keys: [edCombo("Equal NumpadAdd", { shift: null })] },
  { group: "Timeline", what: "Zoom out", cmd: "zoomOut", keys: [edCombo("Minus NumpadSubtract", { shift: null })] },
  { group: "Timeline", what: "Fit the whole clip", cmd: "fit", once: true, keys: [edCombo("KeyF", { shift: true })] },
  { group: "Other", what: "Close a pop-up, or leave the editor", cmd: "escape", keys: [edCombo("Escape")] },
];

/** The shortcut a key press means in edit mode: { entry, combo }, or null. */
function edKeyFind(e) {
  if (e.altKey || e.metaKey) return null;
  for (const entry of ED_KEYS) {
    const combo = entry.keys.find((k) => k.codes.includes(e.code) && k.ctrl === e.ctrlKey && (k.shift === null || k.shift === e.shiftKey));
    if (combo) return { entry, combo };
  }
  return null;
}
function edComboHtml(k) {
  const parts = [k.ctrl && "Ctrl", k.shift === true && "Shift", edKeyName(k.codes[0])].filter(Boolean);
  return parts.map((p) => `<kbd>${esc(t(p))}</kbd>`).join('<span class="e-kplus">+</span>');
}

/* ---------- pop-ups (speed list, shortcut list) ---------- */
let edPopClose = null; // closes the open pop-up, if any

// The one window key listener of this file. Registered at load time, before
// app.js adds its own capture listener, so it runs first: Escape closes an open
// pop-up (or leaves edit mode) instead of the whole viewer, and the editor's Ctrl
// shortcuts are handled here, since app.js only passes keys without Ctrl to S.player.key.
window.addEventListener("keydown", (e) => {
  if (typeof askKey !== "undefined" && askKey) return; // app.js's prompt is open: it owns the keyboard
  if (edPopClose && e.code === "Escape") { e.preventDefault(); e.stopImmediatePropagation(); edPopClose(); return; }
  if (typeof S === "undefined" || !S.open || !S.player) return;
  S.player.resumeAudio?.();
  if (!S.player.editing || edTextTarget(e.target)) return;
  const hit = edKeyFind(e);
  if (!hit || !(hit.combo.ctrl || hit.entry.cmd === "escape")) return; // the rest arrives through S.player.key
  if (hit.entry.cmd === "escape" && document.fullscreenElement) return; // Escape leaves fullscreen first
  e.preventDefault();
  e.stopImmediatePropagation();
  if (e.repeat && hit.entry.once) return;
  S.player.command(hit.entry.cmd);
}, true);

/** Opens a speed list next to `anchor`, positioned inside `host` (which must be positioned). */
function edOpenPop({ anchor, host, value, above = false, onPick }) {
  edPopClose?.();
  const pop = document.createElement("div");
  pop.className = "e-pop";
  pop.setAttribute("role", "menu");
  pop.innerHTML = ED_SPEEDS.map((s) => `<button type="button" role="menuitemradio" aria-checked="${s === value}" tabindex="-1" data-e-pick="${s}" class="${s === value ? "on" : ""}">${s}x</button>`).join("");
  host.appendChild(pop);
  const hr = host.getBoundingClientRect(), ar = anchor.getBoundingClientRect();
  pop.style.maxHeight = `${Math.max(120, hr.height - 8)}px`;
  const w = pop.offsetWidth, h = pop.offsetHeight;
  const up = above || ar.bottom + h + 8 > window.innerHeight;
  pop.style.left = `${clamp(ar.left + ar.width / 2 - w / 2 - hr.left, 4, Math.max(4, hr.width - w - 4))}px`;
  pop.style.top = `${Math.max(4, up ? ar.top - hr.top - h - 6 : ar.bottom - hr.top + 6)}px`;
  anchor.classList.add("open");
  anchor.setAttribute("aria-expanded", "true");

  const away = (e) => { if (!pop.contains(e.target) && !anchor.contains(e.target)) close(); };
  const scrolled = (e) => { if (!pop.contains(e.target)) close(); };
  function close() {
    if (edPopClose !== close) return;
    edPopClose = null;
    pop.remove();
    anchor.classList.remove("open");
    anchor.setAttribute("aria-expanded", "false");
    document.removeEventListener("pointerdown", away, true);
    document.removeEventListener("scroll", scrolled, true);
    window.removeEventListener("resize", close);
  }
  pop.addEventListener("mousedown", (e) => e.preventDefault()); // keep keyboard focus where it was
  pop.addEventListener("click", (e) => {
    const b = e.target.closest("[data-e-pick]");
    if (!b) return;
    close();
    onPick(+b.dataset.ePick);
  });
  document.addEventListener("pointerdown", away, true);
  document.addEventListener("scroll", scrolled, true);
  window.addEventListener("resize", close);
  edPopClose = close;
}

/** The Shortcuts pop-up over the viewer, built from ED_KEYS. */
function edOpenKeys(host) {
  edPopClose?.();
  const box = document.createElement("div");
  box.className = "e-keys";
  box.setAttribute("role", "dialog");
  box.setAttribute("aria-label", t("Keyboard shortcuts"));
  let rows = "", group = "";
  for (const k of ED_KEYS) {
    if (k.group !== group) { group = k.group; rows += `<div class="e-kgroup">${esc(t(group))}</div>`; }
    rows += `<div class="e-krow"><span class="e-kcap">${k.keys.map(edComboHtml).join('<span class="e-kor"></span>')}</span><span class="e-kwhat">${esc(t(k.what))}</span></div>`;
  }
  box.innerHTML = `<div class="e-keys-box">
      <div class="e-keys-head"><div class="e-title">${esc(t("Keyboard shortcuts"))}</div><button type="button" class="btn sm" data-e-keys-close>${esc(t("Close"))}</button></div>
      <div class="small">${esc(t("These work while the editor is open."))}</div>
      <div class="e-keys-list">${rows}</div>
    </div>`;
  host.appendChild(box);
  function close() {
    if (edPopClose !== close) return;
    edPopClose = null;
    box.remove();
  }
  box.addEventListener("click", (e) => { if (e.target === box || e.target.closest("[data-e-keys-close]")) close(); });
  edPopClose = close;
}

/* ---------- media helpers ---------- */
/** Resolves once `a` can play its first frame; rejects on error, abort or after ED_LOAD_TIMEOUT. */
function edMediaReady(a, label) {
  if (a.readyState >= 2) return Promise.resolve();
  return new Promise((ok, bad) => {
    const done = (fn, arg) => {
      clearTimeout(timer);
      a.removeEventListener("loadeddata", onData);
      a.removeEventListener("error", onError);
      a.removeEventListener("emptied", onGone);
      fn(arg);
    };
    const onData = () => done(ok);
    const onError = () => done(bad, new Error(`Could not load ${label} (${a.error?.message || "media error " + a.error?.code})`));
    const onGone = () => done(bad, new Error(`${label} was unloaded while loading`));
    const timer = setTimeout(() => done(bad, new Error(`${label} took too long to load`)), ED_LOAD_TIMEOUT);
    a.addEventListener("loadeddata", onData);
    a.addEventListener("error", onError);
    a.addEventListener("emptied", onGone);
  });
}
function edKillMedia(a) {
  a.pause();
  a.removeAttribute("src");
  a.load();
  a.remove();
}

/* ---------- viewer ---------- */
function blurPanel(c) {
  const job = S.jobs[c.id];
  const st = job?.state;
  if (st === "running" || st === "queued") {
    const pct = clamp(+job.pct || 0, 0, 100);
    return `<div class="small">${esc(st === "queued" ? t("Waiting to render") : t("Rendering with Blur… {pct}%", { pct: pct.toFixed(0) }))}</div>
      ${st === "running" ? `<div class="progress"><i style="width:${pct}%"></i></div>` : ""}
      <div class="small">${esc(t("It keeps rendering in the background. You can follow it under the Renders button at the top of the window."))}</div>
      <button class="btn wide" data-v="blur-done">${esc(t("Done"))}</button>`;
  }
  // "cancelled", "error", "done" or no job: not rendering.
  const b = S.settings.blur;
  const note = st === "error" ? `<div class="small e-bad">${esc(job.message)}</div>` : "";
  const info = !S.blur ? t("blur-cli.exe not found. Set its path under Blur.")
    : b.interpolate ? t("{fps} fps out, amount {amount}, interpolate {interp}. Change under Blur.", { fps: b.output_fps, amount: b.amount, interp: b.interpolated_fps })
    : t("{fps} fps out, amount {amount}. Change under Blur.", { fps: b.output_fps, amount: b.amount });
  return `${note}<button class="btn wide" data-v="blur" ${S.blur ? "" : "disabled"}>${ICON.blur}<span>${esc(t("Render with Blur"))}</span></button>
    <div class="small">${esc(info)}</div>`;
}

function edTracksText(n) {
  if (!n) return t("No audio");
  if (n === 1) return t("1 audio track");
  if (n === 3) return t("Mix + desktop + mic tracks");
  return t("{n} audio tracks", { n });
}

function openClip(id) {
  const c = S.clips.find((x) => x.id === id);
  if (!c) return;
  if (S.player) { S.player.destroy(); S.player = null; }
  edDropInit();
  S.open = id;
  const m = $("#modal");
  m.hidden = false;
  const gammaFn = (ch) => `<feFunc${ch} type="gamma" amplitude="1" exponent="1" offset="0"/>`;
  const L = (s) => esc(t(s));
  m.innerHTML = `
    <div class="viewer">
      <div class="v-main">
        <div class="player">
          <svg class="e-defs" aria-hidden="true"><filter id="e-gamma" color-interpolation-filters="sRGB"><feComponentTransfer>${gammaFn("R")}${gammaFn("G")}${gammaFn("B")}</feComponentTransfer></filter></svg>
          <video src="${fileSrc(c.path)}" autoplay playsinline></video>
          <div class="p-audio" aria-hidden="true"></div>
          <div class="p-big" hidden>${PICON.play}</div>
          <div class="p-ui">
            <div class="p-bar">
              <div class="p-track"><div class="p-buf"></div><div class="p-prog"></div></div>
              <div class="p-knob"></div>
              <div class="p-tip" hidden></div>
            </div>
            <div class="p-row">
              <button data-p="play" tabindex="-1" aria-label="${L("Play or pause")}">${PICON.pause}</button>
              <button data-p="back" tabindex="-1" aria-label="${L("Back 5 seconds")}">${PICON.back}</button>
              <button data-p="fwd" tabindex="-1" aria-label="${L("Forward 5 seconds")}">${PICON.fwd}</button>
              <span class="p-clock">0:00 / ${fmtClock(c.duration)}</span>
              <span class="sp"></span>
              <button data-p="mute" tabindex="-1" aria-label="${L("Mute")}">${PICON.vol}</button>
              <input class="p-vol" type="range" min="0" max="1" step="0.02" aria-label="${L("Volume")}">
              <button data-p="speed" class="p-text" tabindex="-1" aria-label="${L("Playback speed")}" aria-haspopup="menu" aria-expanded="false">1x</button>
              <button data-p="loop" tabindex="-1" aria-label="${L("Loop")}">${PICON.loop}</button>
              <button data-p="full" tabindex="-1" aria-label="${L("Fullscreen")}">${PICON.full}</button>
            </div>
          </div>
        </div>
        <div class="e-tl"></div>
      </div>
      <div class="v-side">
        <div class="v-pane v-info">
          <input class="v-title" value="${esc(c.title)}" spellcheck="false" aria-label="${L("Clip title")}">
          <div class="v-meta"><b>${esc(c.game)}</b> · ${new Date(c.created).toLocaleString()}<br>${c.width}×${c.height} · ${c.fps} fps · ${fmtDur(c.duration)} · ${fmtSize(c.size)}<br>${esc(edTracksText(c.audio_tracks))}</div>
          <button class="btn primary wide" data-v="copy">${ICON.copy}<span>${L("Copy to clipboard")}</span></button>
          <div class="small">${L("Copies the file. Paste it into Discord or any chat.")}</div>
          <div class="v-sep"></div>
          <button class="btn wide" data-v="edit">${ICON.cut}<span>${L("Edit")}</span></button>
          <div class="small">${L("Trim, cut, speed, colour and audio.")}</div>
          <div id="v-blur">${blurPanel(c)}</div>
          <div class="v-sep"></div>
          <button class="btn wide" data-v="fav">${ICON.star}<span>${L(c.favorite ? "Remove favorite" : "Add to favorites")}</span></button>
          <button class="btn wide" data-v="reveal">${ICON.folder}<span>${L("Show in folder")}</span></button>
          <button class="btn wide danger" data-v="delete">${ICON.trash}<span>${L("Delete")}</span></button>
          <span style="flex:1"></span>
          <button class="btn wide" data-v="close">${L("Close")}</button>
        </div>
        <div class="v-pane v-edit">
          <div class="e-head"><div class="e-h">${L("Edit")}</div><button class="btn sm" data-v="edit-done">${L("Done")}</button></div>
          <section class="e-sec">
            <div class="e-title">${L("Speed")}</div>
            <div class="e-line"><button type="button" class="btn sm e-speed" aria-haspopup="menu" aria-expanded="false" aria-label="${L("Clip speed")}"></button><span class="small e-speed-note"></span></div>
          </section>
          <section class="e-sec e-color"></section>
          <section class="e-sec e-export"></section>
          <section class="e-sec"><button class="btn wide" data-v="shortcuts">${L("Shortcuts")}</button></section>
        </div>
      </div>
      <div class="e-drop" hidden><div class="e-drop-box">${L("Drop audio files to add them as new tracks")}</div></div>
    </div>`;
  const root = $(".viewer", m);
  S.player = mountPlayer(root, c);
  S.player.ed = mountEditor(root, c, S.player);
  $(".v-title", root).onchange = async (e) => {
    try { replaceClip(await invoke("rename_clip", { id, title: e.target.value })); } catch (err) { fail(err); e.target.value = c.title; }
  };
  // Clicking a button keeps keyboard focus off it (so Space still plays / pauses),
  // but first takes it away from any text box or slider in the viewer.
  root.addEventListener("mousedown", (e) => {
    if (!e.target.closest("button")) return;
    if (document.activeElement?.matches?.(".viewer input")) document.activeElement.blur();
    e.preventDefault();
  });
  // Web Audio may only start after a user gesture.
  root.addEventListener("pointerdown", () => S.player?.resumeAudio?.());
}

/* ---------- player: Glint's own transport and the edit preview ---------- */
// `root` is the .viewer element.
function mountPlayer(root, c) {
  const v = $("video", root), box = $(".player", root), bar = $(".p-bar", root);
  const el = (s) => $(s, root);
  const dur = () => (Number.isFinite(v.duration) && v.duration > 0 ? v.duration : c.duration) || 1;
  const clamp = (x, lo, hi) => Math.min(hi, Math.max(lo, x));
  const frame = 1 / (c.fps || 60);
  const P = {
    v, loop: false, editing: false,
    viewSpeed: 1,             // the player's own speed menu (the export speed is model.speed)
    vol: 0.6, muted: false,   // master volume, also applied to the preview audio
    model: null,              // the edit model, made on first entering edit mode, dropped with the player
    tl: null,                 // timeline handle while edit mode is on
    sel: null,                // last selection the timeline reported
    src: new Map(),           // preview audio by model row id: { id, kind, path, title, a, node, gain, state }
    trackState: "idle",       // the clip's own tracks: idle | loading | on | failed | none
    ctx: null,                // AudioContext: null = not made yet, false = not available
    waves: new Map(),         // waveforms by row id, kept to hand over again when the timeline is remounted
    thumbs: null,             // film strip urls
    duration: dur,
  };
  let raf = 0, idle = 0, drag = false, dead = false, driftTimer = 0, modelJob = null, extrasAsked = false;

  try { const x = parseFloat(localStorage.getItem("glint.vol") ?? "0.6"); P.vol = Number.isFinite(x) ? clamp(x, 0, 1) : 0.6; }
  catch (e) { console.warn("Glint: could not read the saved volume", e); }

  const ed = () => (P.editing && P.model ? P.model : null);
  const tl = (fn) => {
    if (!P.tl) return;
    try { fn(P.tl); } catch (e) { console.error("Glint: timeline call failed", e); }
  };
  P.alive = () => !dead;

  /* ----- Web Audio: one context per player, a GainNode per preview element ----- */
  P.audioCtx = () => {
    if (P.ctx === null) {
      const AC = window.AudioContext || window.webkitAudioContext;
      if (!AC) { console.warn("Glint: no Web Audio here; levels above 100% are heard at 100% in the preview."); P.ctx = false; }
      else {
        try { P.ctx = new AC(); }
        catch (e) { console.warn("Glint: could not create an AudioContext; levels above 100% are heard at 100% in the preview.", e); P.ctx = false; }
      }
    }
    P.resumeAudio();
    return P.ctx || null;
  };
  P.resumeAudio = () => {
    if (P.ctx && P.ctx.state === "suspended") P.ctx.resume().catch((e) => console.warn("Glint: could not resume audio (needs a click or key press)", e));
  };
  function route(s) {
    const ctx = P.audioCtx();
    if (!ctx) return;
    let node;
    try { node = ctx.createMediaElementSource(s.a); }
    catch (e) { console.warn(`Glint: ${s.title} plays without Web Audio; levels above 100% are heard at 100%.`, e); return; }
    const gain = ctx.createGain();
    gain.gain.value = 0;
    node.connect(gain);
    gain.connect(ctx.destination);
    s.node = node;
    s.gain = gain;
  }

  /* ----- preview audio elements ----- */
  function newSrc(id, kind, path, title) {
    const s = { id, kind, path, title, a: null, node: null, gain: null, state: "loading", error: null };
    P.src.set(id, s);
    s.ready = loadSrc(s);
    return s;
  }
  async function loadSrc(s) {
    // First with CORS, so Web Audio may read it (needed for levels above 100%).
    // A source that refuses CORS is retried as a plain element (levels then stop at 100%).
    for (const cors of [true, false]) {
      const a = document.createElement("audio");
      a.preload = "auto";
      if (cors) a.crossOrigin = "anonymous";
      a.src = fileSrc(s.path);
      el(".p-audio").appendChild(a);
      s.a = a;
      try { await edMediaReady(a, s.title); }
      catch (e) {
        edKillMedia(a);
        if (s.a === a) s.a = null;
        if (dead || s.state === "dropped") return;
        s.error = e;
        console.warn(`Glint: could not load ${s.title} for the preview${cors ? " with CORS, trying without" : ""}`, e);
        continue;
      }
      if (dead || s.state === "dropped") { edKillMedia(a); s.a = null; return; }
      if (cors) route(s);
      else console.warn(`Glint: ${s.title} plays without Web Audio; levels above 100% are heard at 100%.`);
      a.defaultPlaybackRate = v.playbackRate;
      a.playbackRate = v.playbackRate;
      s.state = "on";
      return;
    }
    s.state = "failed";
  }
  function dropSrc(s) {
    s.state = "dropped";
    s.node?.disconnect();
    s.gain?.disconnect();
    s.node = s.gain = null;
    if (s.a) { const a = s.a; s.a = null; edKillMedia(a); }
  }
  /** Loads the clip's own tracks once. Falls back to the video's own audio if any of them fails. */
  function loadTracks() {
    if (P.trackState !== "idle" || !P.model) return;
    const rows = P.model.audio.filter((r) => r.kind === "track");
    if (!rows.length) { P.trackState = "none"; return; }
    P.trackState = "loading";
    (async () => {
      if (rows.some((r) => !r.path)) throw new Error("the clip's audio tracks were not extracted");
      const list = rows.map((r) => newSrc(r.id, "track", r.path, r.title));
      await Promise.all(list.map((s) => s.ready));
      if (dead) return;
      const bad = list.find((s) => s.state !== "on");
      if (bad) throw bad.error || new Error(`${bad.title} did not load`);
      P.trackState = "on";
      applyRate();
      output(true);
      P.ed?.sync("audio");
    })().catch((e) => {
      if (dead) return;
      console.warn("Glint: separate audio track preview failed, using the clip's own audio instead.", e);
      for (const [id, s] of [...P.src]) if (s.kind === "track") { dropSrc(s); P.src.delete(id); }
      P.trackState = "failed";
      output(true);
      P.ed?.sync("audio");
    });
  }
  /** Gives every file row of the model an audio element, and drops elements whose row is gone (undo). */
  function syncRows() {
    const m = P.model;
    if (!m) return;
    const files = new Map(m.audio.filter((r) => r.kind === "file").map((r) => [r.id, r]));
    for (const [id, s] of [...P.src]) if (s.kind === "file" && !files.has(id)) { dropSrc(s); P.src.delete(id); }
    for (const [id, row] of files) {
      if (P.src.has(id)) continue;
      const s = newSrc(id, "file", row.path, row.title);
      s.ready.then(() => {
        if (dead || P.src.get(id) !== s) return;
        if (s.state === "failed") flash(t("{name} cannot be played in the preview. The export still includes it.", { name: s.title }), true);
        applyRate();
        output(true);
        P.ed?.sync("audio");
      });
      const wave = P.waves.get(id); // a row brought back by redo
      if (wave) tl((h) => h.setWaveform(id, wave));
    }
  }

  /* ----- audio output: the video's own audio, or the preview elements while editing ----- */
  const heard = (s) => P.editing && (s.kind === "file" || P.trackState === "on");
  /** Sets each element's level from the model for the current moment. */
  function levels(now) {
    const m = ed(), master = P.muted ? 0 : P.vol;
    for (const s of P.src.values()) {
      if (s.state !== "on" || !s.a) continue;
      const row = m && heard(s) ? m.audio.find((r) => r.id === s.id) : null;
      const want = row ? emGainAt(m, row, now) * master : 0;
      if (s.gain) { if (Math.abs(s.gain.gain.value - want) > 0.001) s.gain.gain.value = want; }
      else { const vol = clamp(want, 0, 1); if (Math.abs(s.a.volume - vol) > 0.001) s.a.volume = vol; }
    }
  }
  /** Keeps each element at its place for the video's time: tracks at the same time, file rows inside their span. */
  function syncAudio(hard) {
    const m = ed(), now = v.currentTime, run = !v.paused && !v.ended && v.readyState >= 3;
    for (const s of P.src.values()) {
      const a = s.a;
      if (s.state !== "on" || !a) continue;
      const row = m && heard(s) ? m.audio.find((r) => r.id === s.id) : null;
      let want = null;
      if (row) want = s.kind === "file" ? (emRowCovers(row, now) ? row.trimStart + (now - row.offset) : null) : now;
      const end = Number.isFinite(a.duration) ? a.duration : Infinity;
      if (want === null || want >= end - 0.01) { if (!a.paused) a.pause(); continue; }
      if (a.playbackRate !== v.playbackRate) a.playbackRate = v.playbackRate;
      if (hard || (run && a.paused) || (!a.seeking && Math.abs(a.currentTime - want) > ED_DRIFT)) a.currentTime = want;
      if (run && a.paused) a.play().catch((e) => console.warn(`Glint: could not play ${s.title} in the preview`, e));
      else if (!run && !a.paused) a.pause();
    }
    levels(now);
  }
  function drift() {
    const want = P.editing && P.src.size > 0 && !v.paused && !dead;
    if (want && !driftTimer) driftTimer = setInterval(() => syncAudio(false), 300);
    else if (!want && driftTimer) { clearInterval(driftTimer); driftTimer = 0; }
  }
  function output(hard = false) {
    v.muted = (P.editing && P.trackState === "on") || P.muted;
    v.volume = P.vol;
    syncAudio(hard);
    drift();
  }
  P.output = output;

  /* ----- speed and colour preview ----- */
  function applyRate() {
    const r = clamp(P.viewSpeed * (ed() ? P.model.speed : 1), 0.0625, 16);
    v.defaultPlaybackRate = r;
    v.playbackRate = r;
    for (const s of P.src.values()) if (s.a) { s.a.defaultPlaybackRate = r; s.a.playbackRate = r; }
  }
  function applyColor() {
    const col = ed() ? P.model.color : ED_NO_COLOR;
    v.style.filter = edFilterCss(col);
    const exp = String(Math.round((1 / col.gamma) * 10000) / 10000);
    for (const f of $$("#e-gamma feFuncR, #e-gamma feFuncG, #e-gamma feFuncB", root)) f.setAttribute("exponent", exp);
  }
  P.applyColor = applyColor;

  /* ----- the seek bar: the source clip, or the finished clip while editing ----- */
  const barLen = () => { const m = ed(); return m ? emKeptDuration(m) || 1 : dur(); };
  const toBar = (sec) => { const m = ed(); return m ? emSrcToOut(m, sec) : sec; };
  const fromBar = (x) => { const m = ed(); return m ? emOutToSrc(m, x) : x; };
  const barSpeed = () => (ed() ? P.model.speed || 1 : 1);
  const pct = (x) => `${clamp((x / barLen()) * 100, 0, 100)}%`;

  /** While editing and playing: skip cut-out parts, stop (or loop) after the last kept part. */
  function guard(now) {
    const m = ed();
    if (!m || v.paused || drag || v.seeking) return;
    const p = emPlayable(m, now);
    if (p === null) {
      const kept = emKept(m);
      if (!kept.length) return;
      if (P.loop) v.currentTime = kept[0].start;
      else { v.pause(); const last = kept[kept.length - 1]; v.currentTime = Math.max(last.start, last.end - frame); }
    } else if (p > now + 0.001) v.currentTime = p;
  }

  /* ----- drawing ----- */
  function paint() {
    if (dead) return;
    guard(v.currentTime);
    const now = v.currentTime, m = ed();
    el(".p-prog").style.width = pct(toBar(now));
    el(".p-knob").style.left = pct(toBar(now));
    el(".p-clock").textContent = m
      ? `${fmtClock(emSrcToOut(m, now) / barSpeed())} / ${fmtClock(emOutDuration(m))}`
      : `${fmtClock(now)} / ${fmtClock(dur())}`;
    if (v.buffered.length) el(".p-buf").style.width = pct(toBar(v.buffered.end(v.buffered.length - 1)));
    if (P.editing) levels(now);
  }
  const tick = () => {
    if (P.editing) syncAudio(false);
    paint();
    raf = v.paused || dead ? 0 : requestAnimationFrame(tick);
  };
  const wake = () => {
    box.classList.remove("idle");
    clearTimeout(idle);
    idle = setTimeout(() => { if (!v.paused && !drag && !edPopClose) box.classList.add("idle"); }, 2200);
  };
  const volUI = () => {
    el('[data-p="mute"]').innerHTML = P.muted || P.vol === 0 ? PICON.mute : PICON.vol;
    el(".p-vol").value = P.muted ? 0 : P.vol;
  };
  const state = () => {
    if (dead) return;
    el('[data-p="play"]').innerHTML = v.paused ? PICON.play : PICON.pause;
    el(".p-big").hidden = !v.paused || v.currentTime === 0;
    el('[data-p="loop"]').classList.toggle("on", P.loop);
    el('[data-p="speed"]').textContent = `${P.viewSpeed}x`;
    volUI();
    if (!v.paused && !raf) tick();
    wake();
  };
  const setVol = (x, unmute = true) => {
    P.vol = clamp(x, 0, 1);
    if (unmute) P.muted = false;
    try { localStorage.setItem("glint.vol", P.vol); } catch (e) { console.warn("Glint: could not save the volume", e); }
    output();
    volUI();
  };
  const setMuted = (on) => { P.muted = on; output(); volUI(); };
  P.setLoop = (on) => { P.loop = !!on; v.loop = P.loop && !P.editing; state(); };

  P.toggle = () => {
    P.resumeAudio();
    if (!v.paused) { v.pause(); return; }
    const m = ed();
    if (m) {
      const p = emPlayable(m, v.currentTime);
      if (p === null) v.currentTime = emPlayable(m, 0) ?? 0;
      else if (p > v.currentTime + 0.001) v.currentTime = p;
    } else if (v.ended) v.currentTime = 0;
    v.play().catch((e) => console.warn("Glint: play() was refused", e));
  };
  /** Seek to source time `sec`. While editing it never lands inside a cut-out part (see edLand), unless `raw`. */
  P.seek = (sec, back = false, raw = false) => {
    let at = clamp(sec, 0, dur());
    const m = ed();
    if (m && !raw) at = edLand(m, at, back, frame);
    v.currentTime = at;
    paint();
    wake();
  };
  P.time = () => v.currentTime;
  P.fullscreen = () => (document.fullscreenElement ? document.exitFullscreen() : box.requestFullscreen().catch((e) => console.warn("Glint: fullscreen refused", e)));

  /* ----- edit mode ----- */
  /** Resolves once the video's length is known (or failed to load). */
  const metadata = () => (v.readyState >= 1 ? Promise.resolve() : new Promise((ok) => {
    const done = () => { clearTimeout(timer); v.removeEventListener("loadedmetadata", done); v.removeEventListener("error", done); ok(); };
    const timer = setTimeout(done, 5000);
    v.addEventListener("loadedmetadata", done);
    v.addEventListener("error", done);
  }));
  /** The clip's edit model, made once per open clip. */
  function ensureModel() {
    if (P.model) return Promise.resolve(P.model);
    modelJob ||= (async () => {
      await metadata();
      let tracks;
      try {
        tracks = await invoke("editor_audio_tracks", { id: c.id });
        if (!Array.isArray(tracks)) throw new Error("editor_audio_tracks did not return a list");
      } catch (e) {
        console.warn("Glint: editor_audio_tracks failed; the rows can still be edited, the preview uses the clip's own audio.", e);
        tracks = edFallbackTracks(c);
      }
      if (dead) return null;
      P.model = emNew(dur(), tracks);
      return P.model;
    })();
    return modelJob;
  }
  P.whenModel = ensureModel;

  const tlFailHtml = () => `<div class="e-tl-fail">${esc(t("The timeline did not load. Speed, colour, export and the I and O keys still work."))}</div>`;
  function mountTl() {
    const host = el(".e-tl");
    if (P.tl || !host) return;
    host.textContent = "";
    if (typeof mountTimeline !== "function") {
      console.error("Glint: mountTimeline is not defined (timeline.js is missing or failed to load)");
      host.innerHTML = tlFailHtml();
      return;
    }
    try {
      P.tl = mountTimeline(host, {
        model: P.model,
        title: c.title,
        getTime: () => v.currentTime,
        onSeek: (sec) => P.seek(sec),
        onChange: (what) => P.edSync(what),
        onSelect: (sel) => { P.sel = sel || null; },
        onAddAudio: () => edPickAudio(),
      });
    } catch (e) {
      console.error("Glint: mountTimeline failed", e);
      P.tl = null;
      host.innerHTML = tlFailHtml();
      return;
    }
    if (P.thumbs) tl((h) => h.setThumbs(P.thumbs));
    for (const [id, wave] of P.waves) if (P.model.audio.some((r) => r.id === id)) tl((h) => h.setWaveform(id, wave));
  }
  function dropTimeline() {
    const h = P.tl;
    P.tl = null;
    P.sel = null;
    if (h) { try { h.destroy(); } catch (e) { console.error("Glint: timeline destroy failed", e); } }
    const host = el(".e-tl");
    if (host) host.textContent = "";
  }
  /** Waveforms and film strip, fetched once per open clip in the background. */
  function fetchExtras() {
    if (extrasAsked) return;
    extrasAsked = true;
    invoke("editor_waveforms", { id: c.id }).then((list) => {
      if (dead) return;
      if (!Array.isArray(list)) throw new Error("editor_waveforms did not return a list");
      for (const w of list) {
        const id = "t" + w.index, wave = { peaks: Array.isArray(w.peaks) ? w.peaks : [], perSecond: +w.per_second || 0 };
        P.waves.set(id, wave);
        if (P.model?.audio.some((r) => r.id === id)) tl((h) => h.setWaveform(id, wave));
      }
    }).catch((e) => { if (!dead) console.warn("Glint: editor_waveforms failed; the audio rows show no waveform.", e); });
    invoke("editor_thumbs", { id: c.id, count: ED_THUMBS }).then((paths) => {
      if (dead) return;
      if (!Array.isArray(paths)) throw new Error("editor_thumbs did not return a list");
      P.thumbs = paths.map(fileSrc);
      tl((h) => h.setThumbs(P.thumbs));
    }).catch((e) => { if (!dead) console.warn("Glint: editor_thumbs failed; the video row shows no film strip.", e); });
  }

  /** Re-sync the preview and the side column after the model changed (from the timeline, a key or the side column). */
  P.edSync = (what) => {
    if (dead) return;
    syncRows();
    applyRate();
    applyColor();
    output(["load", "undo", "redo", "file"].includes(what));
    paint();
    P.ed?.sync(what);
  };
  /** Change the model: snapshot, run `fn`, then refresh the timeline and the preview.
   *  If `fn` changed nothing, the snapshot is taken back and `failMsg` (if any) is shown. */
  P.edit = (what, fn, failMsg) => {
    const m = P.model;
    if (!m) return false;
    const before = edState(m), undo = m.undo.slice(), redo = m.redo.slice();
    emSnapshot(m);
    fn(m);
    if (edState(m) === before) {
      m.undo = undo;
      m.redo = redo;
      if (failMsg) flash(failMsg);
      return false;
    }
    tl((h) => h.refresh());
    P.edSync(what);
    return true;
  };
  /** Add a file (an editor_audio_info result) as a new row at the playhead. */
  P.addFileRow = (info) => {
    const m = P.model;
    emSnapshot(m);
    const row = emAddFile(m, info, v.currentTime);
    syncRows(); // creates its audio element
    tl((h) => h.refresh());
    const wave = { peaks: Array.isArray(info.peaks) ? info.peaks : [], perSecond: +info.per_second || 0 };
    P.waves.set(row.id, wave);
    tl((h) => h.setWaveform(row.id, wave));
    P.edSync("file");
    return row;
  };

  /** Edit mode: timeline under the video, edit column, and the speed / colour / audio preview.
   *  Returns a promise of the model (null when leaving edit mode or if it could not be made). */
  P.setEdit = (on) => {
    on = !!on;
    if (on !== P.editing) {
      P.editing = on;
      root.classList.toggle("editing", on);
      v.loop = P.loop && !on; // while editing, looping is handled by guard() and "ended"
      if (on) v.pause();
      else { edPopClose?.(); dropTimeline(); }
      applyRate();
      applyColor();
      output(true);
      paint();
      P.onEdit?.(on);
    }
    if (!on) return Promise.resolve(null);
    return ensureModel().then((m) => {
      if (!m || dead || !P.editing) return m;
      mountTl();
      loadTracks();
      fetchExtras();
      P.edSync("load");
      return m;
    }).catch((e) => {
      console.error("Glint: the editor could not start", e);
      fail(e);
      return null;
    });
  };
  P.setTrim = P.setEdit; // app.js opens a clip "in the editor" with setTrim(true)
  P.openKeys = () => edOpenKeys(root);

  /** Runs one editor shortcut (an ED_KEYS cmd). */
  P.command = (cmd) => {
    const now = v.currentTime;
    switch (cmd) {
      case "play": return P.toggle();
      case "back5": return P.seek(now - 5, true);
      case "fwd5": return P.seek(now + 5);
      case "back1": return P.seek(now - 1, true);
      case "fwd1": return P.seek(now + 1);
      case "framePrev": v.pause(); return P.seek(now - frame, true);
      case "frameNext": v.pause(); return P.seek(now + frame);
      case "home": return P.seek(0);
      case "end": return P.seek(dur(), true);
      case "loop": return P.setLoop(!P.loop);
      case "full": return P.fullscreen();
      case "zoomIn": return tl((h) => h.zoom(ED_ZOOM_STEP));
      case "zoomOut": return tl((h) => h.zoom(1 / ED_ZOOM_STEP));
      case "fit": return tl((h) => h.fit());
      case "export": return edExport(false);
      case "escape": if (edPopClose) edPopClose(); else P.setEdit(false); return;
    }
    const m = P.model;
    if (!m) return flash(t("The editor is still loading"));
    switch (cmd) {
      case "cutPrev": {
        const pts = [0, ...m.cuts].filter((x) => x < now - 0.02);
        return P.seek(pts.length ? pts[pts.length - 1] : 0, false, true);
      }
      case "cutNext": {
        const pts = [...m.cuts, m.duration].filter((x) => x > now + 0.02);
        return P.seek(Math.min(pts.length ? pts[0] : m.duration, dur() - frame), false, true);
      }
      case "split": return P.edit("split", (mm) => emSplit(mm, now), t("Too close to a cut point to split here"));
      case "trimStart": return P.edit("cut", (mm) => emTrimStart(mm, now), t("Nothing would be left to keep"));
      case "trimEnd": return P.edit("cut", (mm) => emTrimEnd(mm, now), t("Nothing would be left to keep"));
      case "undo": if (emUndo(m)) { tl((h) => h.refresh()); P.edSync("undo"); } else flash(t("Nothing to undo")); return;
      case "redo": if (emRedo(m)) { tl((h) => h.refresh()); P.edSync("redo"); } else flash(t("Nothing to redo")); return;
    }
    let sel = P.sel;
    if (P.tl) { try { sel = P.tl.selection() ?? null; } catch (e) { console.error("Glint: timeline selection() failed", e); } }
    const seg = sel && Number.isInteger(sel.seg) ? sel.seg : -1;
    if (cmd === "remove") {
      if (seg < 0 || seg >= m.removed.length) return flash(t("Select a segment on the timeline first"));
      const off = !m.removed[seg];
      return P.edit(off ? "cut" : "restore", (mm) => emSetRemoved(mm, seg, off), t("At least one part of the clip has to stay"));
    }
    // mute, louder, quieter: the selected audio segment. With the video row selected they do nothing.
    if (sel?.row === "video") return;
    const row = sel ? m.audio.find((r) => r.id === sel.row) : null;
    if (!row || seg < 0 || seg >= row.gains.length) return flash(t("Select an audio segment on the timeline first"));
    const g = row.gains[seg];
    if (cmd === "mute") return P.edit("mute", (mm) => emSetGain(mm, row.id, seg, g > 0.005 ? 0 : 1));
    const next = clamp(Math.round((g + (cmd === "louder" ? ED_GAIN_STEP : -ED_GAIN_STEP)) * 100) / 100, 0, EM_MAX_GAIN);
    if (P.edit("gain", (mm) => emSetGain(mm, row.id, seg, next))) flash(t("Level {pct}%", { pct: Math.round(next * 100) }));
  };

  P.destroy = () => {
    dead = true;
    edPopClose?.();
    cancelAnimationFrame(raf);
    raf = 0;
    clearTimeout(idle);
    clearInterval(driftTimer);
    driftTimer = 0;
    dropTimeline();
    for (const s of P.src.values()) dropSrc(s);
    P.src.clear();
    if (P.ctx) P.ctx.close().catch((e) => console.warn("Glint: AudioContext close failed", e));
    P.ctx = false;
    P.model = null;
    P.waves.clear();
    P.thumbs = null;
    if (document.fullscreenElement) document.exitFullscreen().catch((e) => console.warn("Glint: exitFullscreen failed", e));
    v.pause();
    v.removeAttribute("src");
    v.load();
  };
  /** Player shortcuts. Returns true when the key was used. In edit mode the keys are ED_KEYS. */
  P.key = (e) => {
    if (P.editing) {
      const hit = edKeyFind(e);
      if (!hit || hit.combo.ctrl) return false;
      if (!(e.repeat && hit.entry.once)) P.command(hit.entry.cmd);
      return true;
    }
    switch (e.code) {
      case "Space": case "KeyK": P.toggle(); break;
      case "ArrowLeft": P.seek(v.currentTime - (e.shiftKey ? 1 : 5)); break;
      case "ArrowRight": P.seek(v.currentTime + (e.shiftKey ? 1 : 5)); break;
      case "Comma": v.pause(); P.seek(v.currentTime - frame); break;
      case "Period": v.pause(); P.seek(v.currentTime + frame); break;
      case "ArrowUp": setVol(P.vol + 0.05); break;
      case "ArrowDown": setVol(P.vol - 0.05, false); break;
      case "KeyM": setMuted(!P.muted); break;
      case "KeyF": P.fullscreen(); break;
      case "KeyL": P.setLoop(!P.loop); break;
      case "Home": P.seek(0); break;
      case "End": P.seek(dur()); break;
      default: return false;
    }
    return true;
  };

  for (const ev of ["play", "pause", "ended", "loadedmetadata"]) v.addEventListener(ev, state);
  for (const ev of ["timeupdate", "progress", "seeked", "loadedmetadata"]) v.addEventListener(ev, paint);
  // Keep the preview audio following the video.
  for (const ev of ["play", "playing", "pause", "waiting", "seeked", "ended", "ratechange"]) {
    v.addEventListener(ev, (e) => {
      if (!P.editing || dead) return;
      syncAudio(e.type === "play" || e.type === "seeked");
      drift();
    });
  }
  // Loop while editing: from the end of the clip back to the first kept part.
  v.addEventListener("ended", () => {
    if (!P.editing || !P.loop || !P.model || dead) return;
    v.currentTime = emPlayable(P.model, 0) ?? 0;
    v.play().catch((e) => console.warn("Glint: play() was refused", e));
  });
  v.addEventListener("click", P.toggle);
  v.addEventListener("dblclick", P.fullscreen);
  box.addEventListener("mousemove", wake);
  box.addEventListener("mousedown", (e) => { if (e.target.closest("button")) e.preventDefault(); }); // keep keyboard focus off the buttons
  el(".p-vol").addEventListener("input", (e) => setVol(+e.target.value));
  el(".p-vol").addEventListener("change", (e) => e.target.blur()); // so Space plays / pauses again
  box.addEventListener("click", (e) => {
    const b = e.target.closest("[data-p]");
    if (!b) return;
    switch (b.dataset.p) {
      case "play": P.toggle(); break;
      case "back": P.seek(v.currentTime - 5, true); break;
      case "fwd": P.seek(v.currentTime + 5); break;
      case "mute": setMuted(!P.muted); break;
      case "speed":
        if (b.classList.contains("open")) { edPopClose?.(); break; }
        edOpenPop({ anchor: b, host: box, value: P.viewSpeed, above: true, onPick: (s) => { P.viewSpeed = s; applyRate(); state(); } });
        wake();
        break;
      case "loop": P.setLoop(!P.loop); break;
      case "full": P.fullscreen(); break;
    }
  });

  // Seek bar: drag to scrub. While editing it shows the finished clip.
  const barAt = (e) => { const r = bar.getBoundingClientRect(); return clamp((e.clientX - r.left) / r.width, 0, 1) * barLen(); };
  bar.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    bar.setPointerCapture(e.pointerId);
    drag = true;
    bar.classList.add("drag");
    P.seek(fromBar(barAt(e)));
  });
  bar.addEventListener("pointermove", (e) => {
    const tip = el(".p-tip"), r = bar.getBoundingClientRect();
    tip.hidden = false;
    tip.textContent = fmtDurMs(barAt(e) / barSpeed());
    tip.style.left = `${clamp(e.clientX - r.left, 24, r.width - 24)}px`;
    if (drag) P.seek(fromBar(barAt(e)));
  });
  bar.addEventListener("pointerleave", () => { if (!drag) el(".p-tip").hidden = true; });
  const dropDrag = () => { drag = false; bar.classList.remove("drag"); el(".p-tip").hidden = true; wake(); };
  bar.addEventListener("pointerup", dropDrag);
  bar.addEventListener("pointercancel", dropDrag);

  output();
  state();
  paint();
  return P;
}

/* ---------- editor side column: speed, colour, export ---------- */
function mountEditor(root, c, P) {
  const pane = $(".v-edit", root);
  const speedBtn = $(".e-speed", pane);
  let lastExport = "", colorDrag = false;
  const model = () => P.model;
  const color = () => P.model?.color ?? ED_NO_COLOR;

  const presetList = () => [...ED_PRESETS.map((p, i) => ["b" + i, p]), ...edUserPresets().map((p, i) => ["u" + i, p])];

  function renderSpeed() {
    const m = model();
    speedBtn.disabled = !m;
    speedBtn.innerHTML = `<span>${m ? m.speed : 1}x</span>${ED_CHEV}`;
    $(".e-speed-note", pane).textContent = m ? t("Length {len}", { len: fmtDurMs(emOutDuration(m)) }) : t("Loading…");
  }

  function renderColor() {
    const user = edUserPresets(), col = color(), off = model() ? "" : "disabled";
    $(".e-color", pane).innerHTML = `
      <div class="e-title">${esc(t("Colour"))} <span class="e-tag" hidden>${esc(t("Custom"))}</span></div>
      <div class="e-chips">
        ${ED_PRESETS.map((p, i) => `<button type="button" class="e-chip" data-e-chip="b${i}" data-e-preset="b${i}" ${off}>${esc(t(p.name))}</button>`).join("")}
        ${user.map((p, i) => `<span class="e-chip e-user" data-e-chip="u${i}"><button type="button" data-e-preset="u${i}" ${off}>${esc(p.name)}</button><button type="button" class="e-x" data-e-delpreset="${esc(p.name)}" aria-label="${esc(t("Delete preset {name}", { name: p.name }))}" title="${esc(t("Delete preset"))}">${ICON.x}</button></span>`).join("")}
      </div>
      <div class="e-sliders">
        ${ED_SLIDERS.map(([k, label, min, max]) => `<label class="e-slider"><span>${esc(t(label))}</span><input type="range" min="${min}" max="${max}" step="0.01" value="${col[k]}" data-e-color="${k}" ${off}><output data-e-val="${k}">${edFmtVal(k, col[k])}</output></label>`).join("")}
      </div>
      <button type="button" class="btn sm e-save" data-e-act="preset-save" ${off}>${esc(t("Save as preset"))}</button>`;
    markPreset();
  }
  function syncSliders() {
    const col = color();
    for (const k of ED_COLOR_KEYS) {
      const input = $(`[data-e-color="${k}"]`, pane);
      if (!input) continue;
      input.value = col[k];
      input.disabled = !model();
      $(`[data-e-val="${k}"]`, pane).textContent = edFmtVal(k, col[k]);
    }
  }
  function markPreset() {
    const col = color();
    const hit = presetList().find(([, p]) => edSameColor(p, col))?.[0];
    for (const x of $$("[data-e-chip]", pane)) x.classList.toggle("on", x.dataset.eChip === hit);
    const tag = $(".e-tag", pane);
    if (tag) tag.hidden = !!hit;
  }
  async function savePreset() {
    const m = model();
    if (!m) return;
    const name = await askText(t("Name this colour preset"), "", t("Save preset"));
    if (!name) return;
    const low = name.toLowerCase();
    if (ED_PRESETS.some((p) => p.name.toLowerCase() === low || t(p.name).toLowerCase() === low)) return flash(t("A built-in preset already has that name"), true);
    if (!Array.isArray(S.settings.color_presets)) S.settings.color_presets = [];
    const list = S.settings.color_presets;
    const p = { name, ...Object.fromEntries(ED_COLOR_KEYS.map((k) => [k, Math.round(m.color[k] * 1000) / 1000])) };
    const i = list.findIndex((x) => x?.name?.toLowerCase() === low);
    if (i >= 0) list[i] = p; else list.push(p);
    saveSettings();
    renderColor();
    flash(t('Saved preset "{name}"', { name }));
  }
  async function deletePreset(name) {
    if (!(await ask(t('Delete the colour preset "{name}"?', { name }), t("Delete")))) return;
    const list = S.settings.color_presets;
    const i = Array.isArray(list) ? list.findIndex((x) => x?.name === name) : -1;
    if (i < 0) return;
    list.splice(i, 1);
    saveSettings();
    renderColor();
  }

  /** Notes about what the preview cannot play as exported. */
  function audioNotes() {
    const m = model(), notes = [];
    if (!m) return notes;
    if (P.trackState === "loading") notes.push(t("Loading the audio tracks…"));
    if (P.trackState === "failed") notes.push(t("The separate tracks cannot be previewed, so you hear the clip's own audio. The export still uses your levels."));
    const srcs = [...P.src.values()];
    if (srcs.some((s) => s.kind === "file" && s.state === "failed")) notes.push(t("Some added audio cannot be played in the preview. The export still includes it."));
    const boosted = (s) => { const row = m.audio.find((r) => r.id === s.id); return row && (row.volume > 1.005 || row.gains.some((g) => g > 1.005)); };
    if (srcs.some((s) => s.state === "on" && !s.gain && boosted(s))) notes.push(t("The preview plays levels above 100% at 100%. The export uses the real levels."));
    return notes;
  }

  function renderExport() {
    const m = model(), job = S.exports?.[c.id];
    const changed = !!m && emChanged(m);
    const lines = [];
    if (!m) lines.push(t("Loading the editor…"));
    else if (!changed) lines.push(t("No changes yet."));
    else {
      lines.push(t("New clip, {len} long.", { len: fmtDurMs(emOutDuration(m)) }));
      lines.push(emIsPlainCut(m) ? t("Quick cut, no quality loss.") : t("Re-encodes the video."));
      if (emToOpts(m).audio.length && m.audio.length > 1) lines.push(t("The audio is mixed into one track."));
    }
    const html = `<div class="e-title">${esc(t("Export"))}</div>
      <div class="small">${lines.map(esc).join(" ")}</div>
      ${audioNotes().map((n) => `<div class="small e-note">${esc(n)}</div>`).join("")}
      ${job ? `<div class="e-prog"><div class="small e-prog-txt">${esc(t("Exporting… {pct}%", { pct: job.pct.toFixed(0) }))}</div><div class="progress"><i style="width:${job.pct}%"></i></div></div>` : ""}
      <button class="btn primary wide" data-v="export" ${job || !changed ? "disabled" : ""}><span>${esc(t("Export"))}</span></button>
      <button class="btn wide" data-v="export-blur" ${job || !S.blur || !m ? "disabled" : ""}>${ICON.blur}<span>${esc(changed ? t("Export and render with Blur") : t("Render with Blur"))}</span></button>
      ${S.blur ? "" : `<div class="small">${esc(t("blur-cli.exe not found. Set its path under Blur."))}</div>`}`;
    if (html === lastExport) return;
    lastExport = html;
    $(".e-export", pane).innerHTML = html;
  }
  function paintProgress() {
    const job = S.exports?.[c.id];
    const bar = $(".e-prog i", pane), txt = $(".e-prog-txt", pane);
    if (!job || !bar || !txt) return renderExport();
    bar.style.width = `${job.pct}%`;
    txt.textContent = t("Exporting… {pct}%", { pct: job.pct.toFixed(0) });
  }

  pane.addEventListener("input", (e) => {
    const d = e.target.dataset, m = model();
    if (!("eColor" in d) || !m) return;
    if (!colorDrag) { emSnapshot(m); colorDrag = true; } // one undo step per drag
    m.color[d.eColor] = +e.target.value;
    P.applyColor();
    $(`[data-e-val="${d.eColor}"]`, pane).textContent = edFmtVal(d.eColor, m.color[d.eColor]);
    markPreset();
    renderExport();
  });
  // Sliders are inputs, and app.js skips player keys while an input has focus.
  pane.addEventListener("change", (e) => { if (e.target.type === "range") { colorDrag = false; e.target.blur(); } });
  pane.addEventListener("click", async (e) => {
    const btn = e.target.closest("[data-e-preset],[data-e-delpreset],[data-e-act],.e-speed");
    if (!btn) return;
    const d = btn.dataset;
    try {
      if ("ePreset" in d) {
        const p = presetList().find(([k]) => k === d.ePreset)?.[1];
        if (p) P.edit("color", (m) => { for (const k of ED_COLOR_KEYS) m.color[k] = p[k]; });
      } else if ("eDelpreset" in d) await deletePreset(d.eDelpreset);
      else if (d.eAct === "preset-save") await savePreset();
      else if (btn === speedBtn && model()) {
        if (speedBtn.classList.contains("open")) return edPopClose?.();
        edOpenPop({ anchor: speedBtn, host: root, value: model().speed, onPick: (s) => P.edit("speed", (m) => { m.speed = s; }) });
      }
    } catch (err) { fail(err); }
  });

  P.onEdit = (on) => { if (on) renderExport(); };

  /** Called by P.edSync after any change of the model or the preview audio. */
  function sync(what) {
    renderSpeed();
    if (what === "load") renderColor(); // enables the controls once the model exists
    else { syncSliders(); markPreset(); }
    renderExport();
  }

  renderSpeed();
  renderColor();
  renderExport();
  return { renderExport, paintProgress, sync };
}

/* ---------- adding audio files ---------- */
let edDropSub = null; // Promise of the drag-and-drop subscriptions, made once
function edDropInit() {
  if (edDropSub || !window.__TAURI__?.event?.listen) return;
  const listen = window.__TAURI__.event.listen;
  const open = () => !!(S.open && S.player);
  const mark = (on) => { const o = $("#modal .e-drop"); if (o && o.hidden === on) o.hidden = !on; };
  edDropSub = Promise.all([
    listen("tauri://drag-enter", () => mark(open())),
    listen("tauri://drag-over", () => mark(open())),
    listen("tauri://drag-leave", () => mark(false)),
    listen("tauri://drag-drop", ({ payload }) => {
      mark(false);
      const paths = Array.isArray(payload?.paths) ? payload.paths.filter((p) => typeof p === "string" && p) : [];
      if (open() && paths.length) edAddAudio(paths);
    }),
  ]).catch((e) => { edDropSub = null; console.error("Glint: could not subscribe to file drops", e); });
}

/** Adds each file as a new audio row of the open clip, entering edit mode first if needed. */
async function edAddAudio(paths) {
  const P = S.player;
  if (!P || !S.open) return;
  try {
    const m = await (P.editing ? P.whenModel() : P.setEdit(true));
    if (!m || S.player !== P || !P.alive()) return;
    const added = [], errors = [];
    for (const path of paths) {
      const name = edBaseName(path);
      let info;
      try { info = await invoke("editor_audio_info", { path }); }
      catch (e) {
        console.warn("Glint: editor_audio_info refused", path, e);
        errors.push(t("Could not add {name}: {error}", { name, error: String(e?.message ?? e) }));
        continue;
      }
      if (S.player !== P || !P.alive() || P.model !== m) return; // the clip was closed or changed meanwhile
      if (!info || !(+info.duration > 0)) { errors.push(t("{name} has no audio.", { name })); continue; }
      added.push(P.addFileRow(info).title);
    }
    if (errors.length) flash(errors.length > 1 ? `${errors[0]} ${t("{n} more files could not be added.", { n: errors.length - 1 })}` : errors[0], true);
    else if (added.length === 1) flash(t("Added {name} as a new audio track", { name: added[0] }));
    else if (added.length > 1) flash(t("Added {n} audio tracks", { n: added.length }));
  } catch (e) {
    console.error("Glint: adding audio failed", e);
    fail(e);
  }
}

/** The timeline's "Add audio": a file picker, then the same path as a drop. */
async function edPickAudio() {
  try {
    const path = await invoke("pick_audio_file");
    if (path) await edAddAudio([path]);
  } catch (e) {
    console.error("Glint: pick_audio_file failed", e);
    fail(e);
  }
}

/* ---------- export ---------- */
let edListening = null; // Promise of the "export-progress" subscription, made once
function edListen() {
  edListening ||= window.__TAURI__.event.listen("export-progress", ({ payload: p }) => {
    const job = S.exports?.[p?.id];
    if (!job) return;
    job.pct = clamp(+p.pct || 0, 0, 100);
    if (S.open === p.id && S.player?.ed) S.player.ed.paintProgress();
  });
  return edListening;
}

/** Exports the open clip's edit as a new clip, then opens it (and starts a Blur render if asked). */
async function edExport(withBlur) {
  const id = S.open, P = S.player;
  if (!id || !P) return;
  if (!S.exports) S.exports = {};
  if (S.exports[id]) return;
  const m = P.model;
  if (!m) return flash(t("The editor is still loading"));
  if (!emChanged(m)) {
    if (!withBlur) return flash(t("Nothing to export yet"));
    P.setEdit(false); // show the Blur panel
    return viewerAction("blur");
  }
  const opts = emToOpts(m);
  try { await edListen(); }
  catch (e) { edListening = null; console.error("Glint: could not subscribe to export-progress; exporting without a progress bar.", e); }
  S.exports[id] = { pct: 0 };
  P.ed?.renderExport();
  console.info("Glint: export_clip", id, opts);
  let meta;
  try {
    meta = await invoke("export_clip", { id, opts });
  } catch (e) {
    delete S.exports[id];
    if (S.open === id) S.player?.ed?.renderExport();
    console.error("Glint: export_clip failed", id, opts, e);
    return fail(e);
  }
  delete S.exports[id];
  const stillOpen = S.open === id;
  replaceClip(meta);
  if (stillOpen) openClip(meta.id); // the source clip is still (or again) open: show the result
  flash(withBlur ? t("Exported. Rendering with Blur…") : stillOpen ? t("Exported as a new clip") : t("Export finished: {title}", { title: meta.title }));
  if (!withBlur) return;
  const show = () => { if (S.open === meta.id) $("#v-blur").innerHTML = blurPanel(meta); else render(); };
  S.jobs[meta.id] = { pct: 0, state: "running", message: "" };
  show();
  try { await invoke("blur_clip", { id: meta.id }); }
  catch (e) { S.jobs[meta.id] = { pct: 0, state: "error", message: String(e) }; show(); fail(e); }
}

function closeClip() {
  const m = $("#modal");
  if (S.player) { S.player.destroy(); S.player = null; }
  m.hidden = true;
  m.innerHTML = "";
  S.open = null;
  render();
}

function replaceClip(meta) {
  const i = S.clips.findIndex((c) => c.id === meta.id);
  if (i >= 0) S.clips[i] = meta; else S.clips.push(meta);
  render();
}

async function viewerAction(act) {
  const id = S.open;
  const c = S.clips.find((x) => x.id === id);
  const m = $("#modal");
  if (!c) return closeClip();
  try {
    if (act === "close" || act === "blur-done") closeClip();
    else if (act === "copy") { await invoke("copy_clip", { id }); flash(t("Clip copied. Paste it anywhere.")); }
    else if (act === "reveal") await invoke("reveal_clip", { id });
    else if (act === "fav") { const meta = await invoke("set_favorite", { id, favorite: !c.favorite }); replaceClip(meta); $('[data-v="fav"] span', m).textContent = meta.favorite ? t("Remove favorite") : t("Add to favorites"); }
    else if (act === "delete") {
      if (!(await ask(t('Move "{title}" to the Recycle Bin?', { title: c.title }), t("Move to Recycle Bin")))) return;
      closeClip();
      await invoke("delete_clip", { id });
      S.clips = S.clips.filter((x) => x.id !== id);
      render();
      flash(t("Clip moved to the Recycle Bin"));
    }
    else if (act === "edit" || act === "trim") S.player.setEdit(!S.player.editing);
    else if (act === "edit-done" || act === "trim-cancel") S.player.setEdit(false);
    else if (act === "shortcuts") S.player.openKeys();
    else if (act === "export") await edExport(false);
    else if (act === "export-blur") await edExport(true);
    else if (act === "blur") { S.jobs[id] = { pct: 0, state: "running", message: "" }; $("#v-blur").innerHTML = blurPanel(c); await invoke("blur_clip", { id }); }
  } catch (e) {
    if (act === "blur") { S.jobs[id] = { pct: 0, state: "error", message: String(e) }; $("#v-blur").innerHTML = blurPanel(c); }
    fail(e);
  }
}
