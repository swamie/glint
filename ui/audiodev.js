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

/** The whole "Audio" section of the Settings page. */
function audSectionHtml() {
  const s = S.settings;
  const st = S.status || {};
  if (!AUD.list && !AUD.loading) setTimeout(audLoad, 0);
  const outs = (AUD.list && AUD.list.outputs) || [];
  const ins = (AUD.list && AUD.list.inputs) || [];
  const gameNote = s.game_audio && st.game_audio === "unavailable" ? `<br><b>${t("Not available on this version of Windows.")}</b>`
    : s.game_audio && st.game_audio === "failed" ? `<br><b>${t("The current game's sound could not be captured.")}</b>` : "";
  return `<div class="section aud-section"><h2>${t("Audio")}</h2>
      ${row(t("Desktop audio"), t("Everything you hear on the output device chosen below"), toggle("desktop_audio"))}
      ${row(t("Desktop audio device"), audHint(t("The output whose sound is recorded. Pick the headphones or speakers your game plays on"), outs, s.desktop_device, t("No output device was found.")), audSelect("desktop_device", outs, s.desktop_audio, t("Desktop audio device")))}
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
