"use strict";
// Interface sounds, synthesized with Web Audio (no audio files). Ported from Kite's sfx.js:
// every sound is a short sine "pop" with a falling pitch and an exponential fade, which reads as
// soft rather than clicky. Off by default (settings: ui_sounds, ui_sounds_hover, ui_sounds_volume).
// Loaded after app.js, updater.js and bugreport.js. All names here start with `sfx` / `SFX_`.
//
// Nothing here may throw or weigh on the page: no AudioContext exists until the first sound is
// actually played (with sounds on, or a preview button), and every listener swallows its errors.
//
// Which sound where:
//   wiring (below): buttons and other clickable things -> click, switches -> on / off,
//     dropdowns -> tap, [data-hover] and top-bar buttons -> hover, [data-sfx="name"] -> that sound,
//     [data-sfx="none"] -> silence. sfxGlintKind() picks Glint's own: sidebar pages and filter
//     chips -> tap, a clip card -> open (or tap while selecting), the viewer's favourite button
//     -> star / off, its Close -> close.
//   sfxWatch(): the clip viewer, the right-click menu, the yes/no prompt, the renders list and
//     the dialogs opening -> open, closing -> close (skipped right after a click, which already
//     made its sound); the update becoming available -> star, once per version.
//   app.js: errors (flash(..., true)) -> error, a clip saved / Blur render / compress done ->
//     success, favourite added -> star (removed: off), delete -> close.
//   Exports finishing -> success, from "export-progress" here.
// The editor's timeline (its canvases) makes no sound; only its buttons click.

let sfxCtx = null;
let sfxMaster = null;
const sfxCfg = { enabled: false, hover: false, volume: 0.5 };
let sfxPointerAt = 0; // when the wiring last played a sound for a press (see sfxEventPlay)

function sfxConfigure({ enabled, hover, volume } = {}) {
  sfxCfg.enabled = !!enabled;
  sfxCfg.hover = !!hover;
  sfxCfg.volume = Math.min(1, Math.max(0, Number(volume) || 0));
  if (sfxMaster) sfxMaster.gain.value = sfxCfg.volume;
}

/** The settings as the backend stores them (undefined: the defaults, off / off / 50). */
function sfxFromSettings(s) {
  if (!s) return;
  sfxConfigure({ enabled: s.ui_sounds === true, hover: s.ui_sounds_hover === true, volume: (s.ui_sounds_volume ?? 50) / 100 });
}

function sfxAudio() {
  if (!sfxCtx) {
    const AC = window.AudioContext || window.webkitAudioContext;
    if (!AC) return null;
    sfxCtx = new AC();
    sfxMaster = sfxCtx.createGain();
    sfxMaster.gain.value = sfxCfg.volume;
    // Gentle low-pass keeps every sound round.
    const lp = sfxCtx.createBiquadFilter();
    lp.type = "lowpass";
    lp.frequency.value = 5200;
    sfxMaster.connect(lp).connect(sfxCtx.destination);
  }
  if (sfxCtx.state === "suspended") sfxCtx.resume().catch(() => {});
  return sfxCtx;
}

function sfxTone({ f0, f1 = f0, dur = 0.08, gain = 0.15, type = "sine", delay = 0, attack = 0.004 }) {
  const c = sfxAudio();
  if (!c) return;
  const at = c.currentTime + delay;
  const osc = c.createOscillator();
  const g = c.createGain();
  osc.type = type;
  osc.frequency.setValueAtTime(f0, at);
  osc.frequency.exponentialRampToValueAtTime(Math.max(20, f1), at + dur);
  g.gain.setValueAtTime(0.0001, at);
  g.gain.exponentialRampToValueAtTime(gain, at + attack);
  g.gain.exponentialRampToValueAtTime(0.0001, at + dur);
  osc.connect(g).connect(sfxMaster);
  osc.start(at);
  osc.stop(at + dur + 0.02);
}

function sfxNoise({ dur = 0.03, gain = 0.04, freq = 2400, q = 0.9, delay = 0 }) {
  const c = sfxAudio();
  if (!c) return;
  const at = c.currentTime + delay;
  const len = Math.max(1, Math.floor(c.sampleRate * dur));
  const buf = c.createBuffer(1, len, c.sampleRate);
  const d = buf.getChannelData(0);
  for (let i = 0; i < len; i++) d[i] = (Math.random() * 2 - 1) * (1 - i / len);
  const src = c.createBufferSource();
  src.buffer = buf;
  const bp = c.createBiquadFilter();
  bp.type = "bandpass";
  bp.frequency.value = freq;
  bp.Q.value = q;
  const g = c.createGain();
  g.gain.value = gain;
  src.connect(bp).connect(g).connect(sfxMaster);
  src.start(at);
}

function sfxBell(f, delay = 0, gain = 0.11) {
  sfxTone({ f0: f, dur: 0.42, gain, delay, attack: 0.006 });
  sfxTone({ f0: f * 2, dur: 0.22, gain: gain * 0.28, delay, attack: 0.006 });
}

// The same recipes as Kite.
const SFX_SOUNDS = {
  click() { sfxTone({ f0: 740, f1: 400, dur: 0.075, gain: 0.17 }); sfxNoise({ dur: 0.018, gain: 0.035, freq: 2600 }); },
  tap() { sfxTone({ f0: 560, f1: 380, dur: 0.07, gain: 0.13, type: "triangle" }); },
  hover() { sfxTone({ f0: 1280, f1: 1120, dur: 0.028, gain: 0.022 }); },
  on() { sfxTone({ f0: 500, f1: 620, dur: 0.07, gain: 0.13 }); sfxTone({ f0: 760, f1: 940, dur: 0.1, gain: 0.11, delay: 0.06 }); },
  off() { sfxTone({ f0: 760, f1: 620, dur: 0.07, gain: 0.12 }); sfxTone({ f0: 500, f1: 390, dur: 0.1, gain: 0.1, delay: 0.06 }); },
  open() { sfxTone({ f0: 330, f1: 600, dur: 0.16, gain: 0.085 }); sfxNoise({ dur: 0.09, gain: 0.018, freq: 1500, q: 0.5 }); },
  close() { sfxTone({ f0: 540, f1: 300, dur: 0.14, gain: 0.075 }); },
  star() { sfxBell(1174.66, 0, 0.09); sfxBell(1567.98, 0.07, 0.08); },
  launch() { [523.25, 659.25, 783.99, 1046.5].forEach((f, i) => sfxBell(f, i * 0.08, 0.1)); },
  success() { sfxBell(880, 0, 0.09); sfxBell(1318.51, 0.1, 0.08); },
  error() { sfxTone({ f0: 230, f1: 170, dur: 0.16, gain: 0.16, type: "triangle" }); sfxTone({ f0: 190, f1: 140, dur: 0.2, gain: 0.14, type: "triangle", delay: 0.13 }); },
};
const SFX_NAMES = Object.keys(SFX_SOUNDS);

/** Play a sound by name. `force` ignores the on/off settings (the preview buttons in Settings). */
function sfxPlay(name, force = false) {
  if (!force && (!sfxCfg.enabled || (name === "hover" && !sfxCfg.hover))) return;
  if (force && sfxMaster) sfxMaster.gain.value = sfxCfg.volume;
  try { SFX_SOUNDS[name]?.(); } catch { /* a sound is never worth an error */ }
}

/**
 * A sound for something that happened (a menu or dialog opening or closing). Skipped when the
 * press that caused it made its own sound a moment ago, so one click is one sound.
 */
function sfxEventPlay(name) {
  if (performance.now() - sfxPointerAt < 600) return;
  sfxPlay(name);
}

/* ---------- wiring ---------- */
const SFX_CLICKABLE = 'button, [role="option"], [role="tab"], [role="button"], a[href], summary, [data-sfx], #view .card';
const SFX_HOVER = "[data-hover], .topbar .btn";
const sfxIsToggle = (el) => !!el.matches?.('input[type="checkbox"], input[type="radio"]');

/** Glint's own choices for a press on `el` (the clickable element found), or undefined for the plain rule. */
function sfxGlintKind(el, e) {
  if (el.closest("[data-win]")) return null; // minimize / maximize / close the window
  if (el.matches("[data-page], [data-tab], [data-folder], [data-size]")) return "tap";
  if (el.matches("#view .card")) {
    if (e.target.closest(".ctitle") && !e.ctrlKey && !e.shiftKey) return null; // the name: a double click renames
    const sel = typeof S !== "undefined" && S.sel && S.sel.size;
    return sel || e.ctrlKey || e.shiftKey || e.target.closest(".check") ? "tap" : "open";
  }
  if (el.matches('[data-v="close"]')) return "close";
  if (el.matches('[data-v="fav"]')) {
    const c = typeof S !== "undefined" && S.clips.find((x) => x.id === S.open);
    return c && c.favorite ? "off" : "star";
  }
  return undefined;
}

function sfxKindFor(target, e) {
  if (!target?.closest) return null;
  // Switches and checkboxes get their own on / off sound from the change event.
  if (sfxIsToggle(target) || target.closest("label")?.querySelector('input[type="checkbox"], input[type="radio"]')) return null;
  if (target.closest("canvas")) return null; // the editor's timeline: drags make no sound
  const el = target.closest(SFX_CLICKABLE);
  if (!el || el.disabled || el.getAttribute("aria-disabled") === "true") return null;
  const marked = el.closest("[data-sfx]")?.dataset.sfx;
  if (marked) return marked === "none" ? null : marked;
  const own = sfxGlintKind(el, e);
  return own === undefined ? "click" : own;
}

function sfxInstall() {
  let lastHover = null;
  let lastHoverAt = 0;
  const pressed = (kind) => { sfxPointerAt = performance.now(); sfxPlay(kind); };
  const down = (e) => {
    try {
      if (e.button !== 0 || !sfxCfg.enabled) return;
      const kind = sfxKindFor(e.target, e);
      if (kind) pressed(kind);
    } catch {}
  };
  // Keyboard activation (Enter / Space) fires click with detail 0 and no pointerdown.
  const click = (e) => {
    try {
      if (e.detail !== 0 || !sfxCfg.enabled) return;
      const kind = sfxKindFor(e.target, e);
      if (kind) pressed(kind);
    } catch {}
  };
  const change = (e) => {
    try {
      const el = e.target;
      // The interface sounds switch itself: heard both ways, although the setting changes after this runs.
      if (el.dataset?.set === "ui_sounds") { sfxPointerAt = performance.now(); return sfxPlay(el.checked ? "on" : "off", true); }
      if (!sfxCfg.enabled || el.closest?.("[data-sfx=none]")) return;
      if (sfxIsToggle(el)) pressed(el.checked ? "on" : "off");
      else if (el.tagName === "SELECT") pressed("tap");
    } catch {}
  };
  const over = (e) => {
    try {
      if (!sfxCfg.enabled || !sfxCfg.hover) return;
      const el = e.target.closest?.(SFX_HOVER);
      if (!el || el === lastHover) return;
      lastHover = el;
      const now = performance.now();
      if (now - lastHoverAt < 45) return;
      lastHoverAt = now;
      sfxPlay("hover");
    } catch {}
  };
  const out = (e) => { if (lastHover && !lastHover.contains(e.relatedTarget)) lastHover = null; };
  document.addEventListener("pointerdown", down, true);
  document.addEventListener("click", click, true);
  document.addEventListener("change", change, true);
  document.addEventListener("pointerover", over, true);
  document.addEventListener("pointerout", out, true);
}

/** Opening and closing sounds for the viewer, the menu, the prompts, the renders list and the dialogs. */
function sfxWatch() {
  // Elements shown and hidden with the `hidden` attribute.
  const shown = (el) => !el.hidden;
  const onHidden = new MutationObserver((records) => {
    for (const r of records) {
      const was = r.oldValue === null; // no hidden attribute before: it was showing
      const now = shown(r.target);
      if (was !== now) sfxEventPlay(now ? "open" : "close");
    }
  });
  const watchHidden = (el) => { if (el && !el.sfxWatched) { el.sfxWatched = true; onHidden.observe(el, { attributes: true, attributeFilter: ["hidden"], attributeOldValue: true }); } };
  for (const id of ["modal", "menu", "confirm"]) watchHidden(document.getElementById(id));
  // Dialogs added to and removed from the page, and the renders list (made on first open, then hidden and shown).
  const DLG = ".bug-dlg, .wn-dlg";
  new MutationObserver((records) => {
    for (const r of records) {
      for (const n of r.addedNodes) {
        if (n.nodeType !== 1) continue;
        if (n.matches(DLG)) sfxEventPlay("open");
        else if (n.matches(".jobs-pop")) { watchHidden(n); if (!n.hidden) sfxEventPlay("open"); }
      }
      for (const n of r.removedNodes) if (n.nodeType === 1 && n.matches(DLG)) sfxEventPlay("close");
    }
  }).observe(document.body, { childList: true });
  // The update button turning into "Update x.y.z": a star, once per version (remembered across starts).
  const slot = document.getElementById("update-slot");
  if (slot) {
    const check = () => {
      // updInfo is updater.js's; the button's look tells that it is showing.
      const ver = slot.querySelector(".upd-btn.upd-new") && typeof updInfo !== "undefined" && updInfo.available ? String(updInfo.latest || "") : "";
      if (!ver) return;
      let heard = "";
      try { heard = localStorage.getItem("glint.sfxUpdate") || ""; } catch {}
      if (heard === ver) return;
      try { localStorage.setItem("glint.sfxUpdate", ver); } catch {}
      sfxPlay("star");
    };
    new MutationObserver(check).observe(slot, { subtree: true, attributes: true, attributeFilter: ["class"], childList: true, characterData: true });
  }
  // An editor export reaching 100 %: success, once per export.
  const exportsDone = new Set();
  window.__TAURI__?.event?.listen("export-progress", ({ payload: p } = {}) => {
    if (!p || p.id == null) return;
    if (+p.pct >= 100) { if (!exportsDone.has(p.id)) { exportsDone.add(p.id); sfxPlay("success"); } }
    else exportsDone.delete(p.id);
  })?.catch?.(() => {});
}

try {
  sfxInstall();
  sfxWatch();
  if (typeof S !== "undefined") sfxFromSettings(S.settings);
} catch (e) { console.error("Glint: interface sounds could not start", e); }
