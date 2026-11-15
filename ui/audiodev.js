"use strict";
// Settings → Audio (0.1.27): what is recorded (desktop audio, microphone, noise
// suppression, the game's own track) and from which device. The devices are
// kept by name in settings.desktop_device / mic_device; "" = the Windows default.
// A chosen device that is not there is recorded from the default instead
// (audio.rs), and this section says so. app.js calls audSectionHtml().

const AUD = { list: null, loading: false, failed: false };

/** Ask the backend for the devices Windows has now, then redraw the Settings page. */
async function audLoad() {
  if (AUD.loading) return;
  AUD.loading = true;
  try {
    const list = await invoke("audio_devices");
    AUD.list = { outputs: (list && list.outputs) || [], inputs: (list && list.inputs) || [] };
    AUD.failed = false;
  } catch (e) {
    AUD.failed = true;
    if (!AUD.list) AUD.list = { outputs: [], inputs: [] };
  } finally {
    AUD.loading = false;
  }
  if (S.page === "settings" && typeof render === "function") render();
}

/**
 * Which listed device a saved name is: the same name, or the same name in
 * other case / with spaces around it (as audio.rs `pick`). -1 = not there.
 */
function audFind(devs, want) {
  const w = String(want || "").trim();
  if (!w) return -1;
  const exact = devs.findIndex((d) => d.name === want);
  if (exact >= 0) return exact;
  return devs.findIndex((d) => String(d.name).trim().toLowerCase() === w.toLowerCase());
}

/** The options of one device dropdown: "Default (name)" first, then every device; a chosen device that is gone is kept as "(not connected)". */
function audOptions(devs, want) {
  const def = devs.find((d) => d.default);
  const opts = [["", def ? t("Default ({name})", { name: def.name }) : t("Windows default")]];
  for (const d of devs) opts.push([d.name, d.name]);
  const i = audFind(devs, want);
  // Until the first list has come in, the chosen name is shown as it is, not as missing.
  const missing = !!AUD.list && !!String(want || "").trim() && i < 0;
  if (i < 0 && String(want || "").trim()) opts.push([want, missing ? t("{name} (not connected)", { name: want }) : want]);
  // The value the dropdown shows: the listed spelling of the chosen device.
  const value = !String(want || "").trim() ? "" : i >= 0 ? devs[i].name : want;
  return { opts, value, missing, def };
}

/** One device dropdown, saved through app.js's data-set handler. Greyed while its toggle is off. */
function audSelect(path, devs, on, label) {
  const { opts, value } = audOptions(devs, getPath(S.settings, path));
  return `<select class="field aud-dev" data-set="${path}" aria-label="${esc(label)}"${on ? "" : " disabled"}>${opts.map(([v, l]) => `<option value="${esc(v)}"${v === value ? " selected" : ""}>${esc(l)}</option>`).join("")}</select>`;
}

/** The hint under a device dropdown: what it does, and a note when the chosen device is not there. */
function audHint(text, devs, want, none) {
  const { missing, def } = audOptions(devs, want);
  let out = text;
  if (AUD.list && !AUD.loading && !devs.length) out += `<br><b>${none}</b>`;
  else if (AUD.list && missing) {
    out += `<br><b class="aud-warn">${def
      ? t("{name} is not connected, so {other} is recorded instead. It is used again as soon as it is back.", { name: esc(want), other: esc(def.name) })
      : t("{name} is not connected, so the Windows default is recorded instead. It is used again as soon as it is back.", { name: esc(want) })}</b>`;
  }
  return out;
}

/* ---------- the mixer (0.1.32): a fader, a meter and an on / off button per source ---------- */

const AUD_DB_MIN = -40, AUD_DB_MAX = 12; // the fader's range (audio.rs GAIN_DB_MIN / GAIN_DB_MAX)
const AUD_FLOOR = -60; // the meter's lowest mark, dB
/** [id, setting that turns the source on, its fader's setting, its name]. */
const audStrips = () => [
  ["desktop", "desktop_audio", "desktop_gain_db", t("Desktop audio")],
  ["mic", "mic", "mic_gain_db", t("Microphone")],
  ["game", "game_audio", "game_gain_db", t("Game audio")],
];
const audDbText = (db) => `${db > 0 ? "+" : ""}${(Math.round(db * 10) / 10).toFixed(1)} dB`;
/** A level in dB as the filled share of the meter (0..1). */
const audFill = (db) => Math.min(1, Math.max(0, (db - AUD_FLOOR) / -AUD_FLOOR));
/** What a meter shows next: the new level, or the old one falling by `fall` dB when the sound got quieter. */
const audFallTo = (shown, level, fall) => Math.max(level, shown - fall, -120);
const AUD_METER = { shown: {}, hold: {}, busy: false, timer: 0 };

/** The mixer block at the top of the Audio section. */
function audMixerHtml() {
  const s = S.settings, st = S.status || {};
  const gameMode = s.capture_mode === "game";
  const marks = [0, -10, -20, -30, -40, -50, -60];
  const strip = ([id, onKey, gainKey, name]) => {
    // In GAME mode the game's own sound is always recorded, and the desktop's sound has its own
    // switch there (game_desktop_audio, off to begin with): the game's picture with everything you hear.
    const forced = gameMode && id === "game", unused = false;
    if (gameMode && id === "desktop") onKey = "game_desktop_audio";
    const on = forced ? true : !!s[onKey];
    const db = Math.min(AUD_DB_MAX, Math.max(AUD_DB_MIN, +s[gainKey] || 0));
    const muteTip = forced ? t("Always recorded in GAME mode")
      : gameMode && id === "desktop" ? (on ? t("Recorded with the game. Click to record only the game's sound") : t("Not recorded in GAME mode. Click to record everything you hear as well"))
        : on ? t("Recorded. Click to turn it off") : t("Not recorded. Click to turn it on");
    return `<div class="aud-strip${on ? "" : " off"}" data-aud-strip="${id}">
        <div class="aud-name" title="${esc(name)}">${esc(name)}</div>
        <div class="aud-db">${audDbText(db)}</div>
        <div class="aud-body">
          <input type="range" class="aud-fader" data-native min="${AUD_DB_MIN}" max="${AUD_DB_MAX}" step="0.5" value="${db}" data-aud-gain="${gainKey}" aria-label="${esc(t("{name} volume", { name }))}" title="${esc(t("Double-click for 0 dB"))}">
          <div class="aud-meter" aria-hidden="true"><div class="aud-bar"><i></i><u></u></div><div class="aud-bar"><i></i><u></u></div></div>
          <div class="aud-scale" aria-hidden="true">${marks.map((m) => `<span style="top:${(1 - audFill(m)) * 100}%">${m}</span>`).join("")}</div>
        </div>
        <button type="button" class="aud-mute" data-aud-mute="${onKey}" aria-pressed="${on}"${unused || forced ? " disabled" : ""} title="${esc(muteTip)}" aria-label="${esc(muteTip)}">${on ? PICON.vol : PICON.mute}</button>
      </div>`;
  };
  const note = st.recording ? t("The faders set how loud each source is in your clips, from now on. The meters show what is being recorded")
    : t("The meters move while the replay buffer is on");
  return `<div class="aud-mixer-row"><div class="aud-mixer">${audStrips().map(strip).join("")}</div><div class="small aud-mixer-note">${esc(note)}</div></div>`;
}

/** Move the meters: called on a timer while a mixer is on the page. */
async function audMeterTick() {
  const mixer = document.querySelector(".aud-mixer");
  if (!mixer || document.hidden || AUD_METER.busy) return;
  AUD_METER.busy = true;
  let lv = null;
  try { lv = await invoke("audio_levels"); } catch { /* the meters rest */ }
  AUD_METER.busy = false;
  if (!lv) return;
  const now = performance.now();
  for (const el of mixer.querySelectorAll("[data-aud-strip]")) {
    const id = el.dataset.audStrip, src = lv[id] || {}, off = el.classList.contains("off");
    [src.left, src.right].forEach((raw, ch) => {
      const key = `${id}${ch}`, level = off || !Number.isFinite(raw) ? -120 : raw;
      const shown = audFallTo(AUD_METER.shown[key] ?? -120, level, 2.2); // falls about 30 dB a second
      AUD_METER.shown[key] = shown;
      // The loudest point stays marked for a second, then falls.
      const hold = AUD_METER.hold[key];
      if (!hold || shown >= hold.db || now - hold.at > 1000) AUD_METER.hold[key] = { db: hold && shown < hold.db ? Math.max(shown, hold.db - 3) : shown, at: hold && shown < hold.db ? hold.at : now };
      const bar = el.querySelectorAll(".aud-bar")[ch];
      if (!bar) return;
      bar.firstElementChild.style.height = `${(1 - audFill(shown)) * 100}%`;
      const line = bar.lastElementChild, h = AUD_METER.hold[key].db;
      line.style.bottom = `${audFill(h) * 100}%`;
      line.style.opacity = h > AUD_FLOOR ? "1" : "0";
    });
  }
}
if (typeof document !== "undefined") {
  AUD_METER.timer = setInterval(audMeterTick, 70);
  document.addEventListener("input", (e) => {
    const f = e.target.closest?.("[data-aud-gain]");
    if (!f || !S.settings) return;
    S.settings[f.dataset.audGain] = +f.value;
    const label = f.closest(".aud-strip")?.querySelector(".aud-db");
    if (label) label.textContent = audDbText(+f.value);
    saveSettings(); // no redraw of the page: the fader is being dragged
  });
  document.addEventListener("dblclick", (e) => {
    const f = e.target.closest?.("[data-aud-gain]");
    if (!f || !S.settings) return;
    f.value = "0";
    f.dispatchEvent(new Event("input", { bubbles: true }));
  });
  document.addEventListener("click", (e) => {
    const b = e.target.closest?.("[data-aud-mute]");
    if (!b || b.disabled || !S.settings) return;
    S.settings[b.dataset.audMute] = !S.settings[b.dataset.audMute];
    saveSettings(true);
  });
}

/** The whole "Audio" section of the Settings page. */
function audSectionHtml() {
  const s = S.settings;
  const st = S.status || {};
  if (!AUD.list && !AUD.loading) setTimeout(audLoad, 0);
  const outs = (AUD.list && AUD.list.outputs) || [];
  const ins = (AUD.list && AUD.list.inputs) || [];
  const gameMode = s.capture_mode === "game";
  const gameNote = s.game_audio && st.game_audio === "unavailable" ? `<br><b>${t("Not available on this version of Windows.")}</b>`
    : s.game_audio && st.game_audio === "failed" ? `<br><b>${t("The current game's sound could not be captured.")}</b>` : "";
  return `<div class="section aud-section"><h2>${t("Audio")}</h2>
      ${audMixerHtml()}
      ${gameMode
        ? row(t("Desktop audio in GAME mode"), t("Records everything you hear on the output device chosen below, as well as the game: the game's window with all the sound. Off: only the game's own sound and your microphone"), toggle("game_desktop_audio"))
        : row(t("Desktop audio"), t("Everything you hear on the output device chosen below"), toggle("desktop_audio"))}
      ${row(t("Desktop audio device"), audHint(t("The output whose sound is recorded. Pick the headphones or speakers your game plays on"), outs, s.desktop_device, t("No output device was found.")), audSelect("desktop_device", outs, gameMode ? s.game_desktop_audio : s.desktop_audio, t("Desktop audio device")))}
      ${row(t("Microphone"), t("Your voice, saved as its own track"), toggle("mic"))}
      ${row(t("Microphone device"), audHint(t("The microphone that is recorded"), ins, s.mic_device, t("No microphone was found.")), audSelect("mic_device", ins, s.mic, t("Microphone device")))}
      ${row(t("Microphone noise suppression"), t("Cuts background noise such as fans and hum from your voice (RNNoise). Applied when a clip is saved, so it costs nothing while clipping"), toggle("mic_denoise"))}
      ${row(t("Separate game audio track"), t("Adds a track with only the game's sound, so you can keep the game and drop everything else (voice chat, music) when editing. Needs Windows 10 version 2004 or newer.") + gameNote, toggle("game_audio"))}
      ${row(t("Device lists"), AUD.failed ? `<b>${t("The devices could not be listed.")}</b>` : t("Plugged in a headset or a microphone? Refresh the lists"), `<button class="btn sm" data-aud="refresh"${AUD.loading ? " disabled" : ""}>${ICON.refresh}${t("Refresh")}</button>`)}
    </div>`;
}

document.addEventListener("click", (e) => {
  const b = e.target.closest?.("[data-aud]");
  if (!b || b.disabled) return;
  if (b.dataset.aud === "refresh") {
    b.disabled = true;
    audLoad().then(() => { if (!AUD.failed && typeof flash === "function") flash(t("Device lists refreshed")); });
  }
});
