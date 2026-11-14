"use strict";
// Capture page extras (0.1.16):
// - "Apply recommended settings" (rules in the backend: capture2.rs `recommended_settings`),
// - what each "Auto" choice is using right now (`S.auto` from get_state / `auto_info`),
// - a line about Windows' yellow capture border.
// Loaded before app.js: its globals (state, translation, helpers) are used only inside functions.

const CAP2 = { busy: false, seq: 0, timer: 0 };
const CAP2_ENC = { nvenc: "NVIDIA NVENC", amf: "AMD AMF", qsv: "Intel Quick Sync", x264: "x264" };
const CAP2_METHOD = { wgc: "Windows Graphics Capture", dda: "Desktop Duplication" };

/** "16:9", "21:9", ... for a picture size, or "W×H" when it is no common shape. */
function cap2Ratio(w, h) {
  if (!w || !h) return "";
  const r = w / h;
  const named = [["16:9", 16 / 9], ["16:10", 1.6], ["4:3", 4 / 3], ["5:4", 1.25], ["3:2", 1.5], ["32:9", 32 / 9]];
  for (const [name, v] of named) if (Math.abs(r / v - 1) < 0.02) return name;
  if (r > 2.3 && r < 2.42) return "21:9"; // 2560×1080, 3440×1440 and 21:9 proper
  return `${w}×${h}`;
}

/** What "Auto" resolves to for a setting, as words; "" when it is not known yet. */
function cap2AutoName(key) {
  const a = (typeof S !== "undefined" && S.auto) || {};
  if (key === "encoder") return CAP2_ENC[a.encoder] || "";
  if (key === "method") return CAP2_METHOD[a.method] || "";
  if (key === "aspect") {
    const r = cap2Ratio(a.aspect_width, a.aspect_height);
    return !r ? "" : a.widened ? t("widened to {ratio}", { ratio: r }) : t("{ratio}, unchanged", { ratio: r });
  }
  if (key === "bitrate") return a.auto_mbps ? t("up to {mbps} Mbps", { mbps: Math.round(a.auto_mbps) }) : "";
  return "";
}

/** "Auto (NVIDIA NVENC)", or plain "Auto" while it is not known. */
function cap2AutoLabel(key) {
  const name = cap2AutoName(key);
  return name ? t("Auto ({name})", { name }) : t("Auto");
}

/** Write the resolved values into the Capture page's "Auto" options. */
function cap2PatchLabels() {
  const opt = (path, value) => document.querySelector(`#view select[data-set="${path}"] option[value="${value}"]`);
  for (const [path, value] of [["encoder", "auto"], ["method", "auto"], ["aspect", "auto"], ["bitrate", "0"]]) {
    const o = opt(path, value);
    if (o) o.textContent = cap2AutoLabel(path);
  }
  const hint = document.querySelector("#view [data-cap2-border]");
  if (hint) hint.innerHTML = cap2BorderText();
}

/** Called at the end of renderRecording(): labels now, then fresh values once the settings are saved. */
function cap2AfterRender() {
  cap2PatchLabels();
  clearTimeout(CAP2.timer);
  const seq = ++CAP2.seq;
  // saveSettings() waits 200 ms before it saves: ask after that, so the answer is for the new settings.
  CAP2.timer = setTimeout(async () => {
    try {
      const a = await invoke("auto_info");
      if (seq !== CAP2.seq || !a) return;
      S.auto = a;
      cap2PatchLabels();
    } catch (e) {
      console.warn("Glint: auto_info failed", e);
    }
  }, 350);
}

/** The button at the top of the Capture page, with its small print under it. */
function cap2TopHtml() {
  return `<div class="cap2-rec">
      <button class="btn sm" data-cap2="apply"${CAP2.busy ? " disabled" : ""}>${CAP2.busy ? t("Checking this PC…") : t("Apply recommended settings")}</button>
      <div class="cap2-small">${t("Results can be inconsistent. It is best to do your own research and tweak these to your liking.")}</div>
    </div>`;
}

/** The yellow border, in the Capture method row. */
function cap2BorderText() {
  const s = S.settings || {};
  const a = S.auto || {};
  const st = S.status || {};
  const b = a.border || {};
  const where = t("Windows Settings › Privacy & security › Screenshot borders");
  const method = st.recording && st.method ? st.method : s.method === "auto" ? a.method : s.method;
  if (s.method === "auto" && a.method === "dda" && a.method_why === "border") {
    return t("Auto uses Desktop Duplication because {where} does not let desktop apps turn off the yellow border.", { where: esc(where) });
  }
  if (method !== "wgc") return "";
  if (st.recording && st.border === "failed") {
    return `<span class="cap2-warn">${t("Windows did not turn off the yellow capture border this time. In {where}, switch “Let desktop apps turn off the screenshot border” off and on again, or choose Desktop Duplication here.", { where: esc(where) })}</span>`;
  }
  if (b.configurable && !b.can_remove) {
    return `<span class="cap2-warn">${t("{where} does not let desktop apps turn off the yellow border, so Graphics Capture shows it around your screen. Desktop Duplication has no border.", { where: esc(where) })}</span>`;
  }
  return t("Glint asks Windows for no yellow border. If one shows up in a game anyway, switch “Let desktop apps turn off the screenshot border” off and on again in {where}, or choose Desktop Duplication here.", { where: esc(where) });
}

function cap2BorderHint() {
  return `<br><span data-cap2-border>${cap2BorderText()}</span>`;
}

/* ---------- Apply recommended settings ---------- */

const CAP2_NAMES = { encoder: "Encoder", method: "Capture method", aspect: "Aspect ratio", height: "Resolution", fps: "Frame rate", bitrate: "Bitrate", quality: "Quality", buffer: "Keep the buffer in" };

/** A value as the Capture page's dropdown shows it. */
function cap2ValueText(key, value) {
  const o = document.querySelector(`#view select[data-set="${key}"] option[value="${CSS.escape(String(value))}"]`);
  if (o) return o.textContent.trim();
  if (key === "quality") return { low: t("Low"), medium: t("Medium"), high: t("High"), ultra: t("Ultra") }[value] || String(value);
  if (key === "buffer") return { mixed: t("RAM + disk"), ram: t("RAM only"), disk: t("Disk only") }[value] || String(value);
  if (key === "fps") return `${value} fps`;
  if (key === "bitrate") return value ? `${value} Mbps` : cap2AutoLabel("bitrate");
  if (key === "encoder" || key === "method") return value === "auto" ? cap2AutoLabel(key) : (key === "encoder" ? CAP2_ENC : CAP2_METHOD)[value] || String(value);
  return String(value);
}

/** The reason for one change, translated, with names in place of setting values. */
function cap2Why(item) {
  const vars = { ...(item.vars || {}) };
  if (vars.encoder) vars.encoder = CAP2_ENC[vars.encoder] || vars.encoder;
  return t(item.why, vars);
}

/** A prompt like ask(), with a list in it. Resolves to true to go ahead. */
function cap2Confirm(html, okLabel) {
  return new Promise((resolve) => {
    const el = $("#confirm");
    el.innerHTML = `<div class="confirm-box cap2-box">${html}<div class="confirm-row"><button class="btn" data-c="no">${t("Cancel")}</button><button class="btn primary" data-c="yes">${esc(okLabel)}</button></div></div>`;
    el.hidden = false;
    const done = (yes) => { el.hidden = true; el.innerHTML = ""; el.onclick = null; askKey = null; resolve(yes); };
    askKey = (e) => { if (e.code === "Enter" || e.code === "NumpadEnter") done(true); else if (e.code === "Escape") done(false); };
    el.onclick = (e) => { const b = e.target.closest("[data-c]"); if (b) done(b.dataset.c === "yes"); else if (e.target === el) done(false); };
  });
}

/** The changes `rec` asks for against the current settings. */
function cap2Changes(rec, settings) {
  return (rec.items || []).filter((it) => {
    const cur = getPath(settings, it.key);
    // The quality is not used while a fixed bitrate stays fixed; with Auto it is.
    return String(cur) !== String(it.value);
  });
}

async function cap2Apply() {
  if (CAP2.busy) return;
  CAP2.busy = true;
  cap2Redraw();
  let rec = null;
  try {
    rec = await invoke("recommended_settings");
  } catch (e) {
    fail(e);
  } finally {
    CAP2.busy = false;
    cap2Redraw();
  }
  if (!rec) return;
  const changes = cap2Changes(rec, S.settings);
  const i = rec.input || {};
  const facts = [
    i.gpu ? t("Graphics card: {gpu}", { gpu: esc(i.gpu) }) : t("No graphics card with a video encoder was found"),
    t("Screen: {w}×{h} at {hz} Hz", { w: i.width, h: i.height, hz: i.hz }),
    t("RAM: {gb} GB · {n} processor threads", { gb: Math.round(i.ram_gb || 0), n: i.threads }),
    i.build ? t("Windows build {build}", { build: i.build }) : "",
  ].filter(Boolean).join("<br>");
  if (!changes.length) {
    flash(t("Your capture settings already match the recommended ones."));
    return;
  }
  const list = changes.map((c) => `<li><b>${t(CAP2_NAMES[c.key] || c.key)}</b>: ${esc(cap2ValueText(c.key, getPath(S.settings, c.key)))} → <b>${esc(cap2ValueText(c.key, c.value))}</b><div class="cap2-why">${esc(cap2Why(c))}</div></li>`).join("");
  const html = `<p>${t("These settings will change:")}</p><ul class="cap2-list">${list}</ul><p class="cap2-facts">${facts}</p><p class="cap2-small">${t("Results can be inconsistent. It is best to do your own research and tweak these to your liking.")}</p>`;
  if (!(await cap2Confirm(html, t("Apply")))) return;
  for (const c of changes) setPath(S.settings, c.key, c.value);
  saveSettings(true);
  flash(t("Recommended settings applied"));
}

/** Show the button's busy state without a full re-render. */
function cap2Redraw() {
  const box = document.querySelector("#view .cap2-rec");
  if (box) box.outerHTML = cap2TopHtml();
}

document.addEventListener("click", (e) => {
  const b = e.target.closest && e.target.closest("[data-cap2]");
  if (!b || b.disabled) return;
  if (b.dataset.cap2 === "apply") cap2Apply();
});

// Node tests load this file directly.
if (typeof module !== "undefined") module.exports = { cap2Ratio, cap2AutoName, cap2AutoLabel, cap2Changes, cap2Why, cap2BorderText };
