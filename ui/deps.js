"use strict";
// Settings → Components ("Check and repair"): what Glint needs (ffmpeg, ffprobe, Blur, curl, tar),
// with Install buttons. The same install runs from the "Install ffmpeg" button beside the status
// bar's "ffmpeg.exe was not found" message. Backend: deps.rs (check_deps, install_dep, cancel_dep_install).
// Hooks in app.js: depsSection() in renderSettings, depsErrorExtra(text) beside a capture error.

const DEPS = {
  items: null,      // from check_deps
  checking: false,
  checked: false,   // a check ran since the window opened
  busy: "",         // id being installed
  prog: null,       // last "deps-progress" payload
  error: "",        // last install error (English, translated when shown)
  errorId: "",
  queue: [],        // "Install everything missing"
};

const DEPS_NAMES = { ffmpeg: "ffmpeg", blur: "Blur" };

/** The fixed texts deps.rs sends (notes and errors), listed so the translation checker sees them. */
function depsKnownTexts() {
  return [
    t("GitHub could not be reached. Check your internet connection and try again."),
    t("The download failed. Check your internet connection and try again."),
    t("Cancelled."),
    t("The download could not be unpacked."),
    t("The downloaded ffmpeg does not run on this PC."),
    t("Glint could not put the files in place. Close any program that is using ffmpeg and try again."),
    t("Another install is already running."),
    t("curl.exe or tar.exe is missing. They are part of Windows 10 version 1803 and newer."),
    t("The download was not found on GitHub."),
    t("Blur's installer did not finish. Install it yourself from github.com/f0e/blur."),
    t("Nothing to install for that."),
    t("Older than ffmpeg 8: it has no Windows Graphics Capture, so Glint captures with Desktop Duplication. That works, but Graphics Capture is smoother at high frame rates. Reinstall to get the newest ffmpeg."),
    t("The file is there but does not run."),
    t("Records the replay buffer and saves clips. Required."),
    t("Reads clip lengths and sizes for the library. Installed together with ffmpeg."),
    t("Optional: motion-blur renders of finished clips. Windows may ask for permission while it installs."),
    t("Part of Windows 10 version 1803 and newer. Only needed to install the parts above."),
  ];
}

/** The id an item's Install button installs: ffprobe comes with ffmpeg. */
const depsTarget = (it) => (it.id === "ffprobe" ? "ffmpeg" : it.id);

function depsStateText(state) {
  return { ok: t("Installed"), missing: t("Missing"), old: t("Too old") }[state] || state;
}

/** "Downloading ffmpeg… 34 MB of 98 MB" and a bar. `small`: the status bar's one-line form. */
function depsProgHTML(small) {
  if (!DEPS.busy) return `<span class="deps-prog" hidden></span>`;
  const p = DEPS.prog && DEPS.prog.id === DEPS.busy ? DEPS.prog : { phase: "download", done: 0, total: 0 };
  const name = DEPS_NAMES[DEPS.busy] || DEPS.busy;
  const pct = p.total > 0 ? Math.min(100, (p.done / p.total) * 100) : 0;
  const text = p.phase === "unpack" ? t("Unpacking {name}…", { name })
    : p.phase === "install" ? t("Installing {name}…", { name })
    : p.total > 0 ? t("Downloading {name}… {done} of {total}", { name, done: fmtSize(p.done || 0), total: fmtSize(p.total) })
    : t("Downloading {name}…", { name });
  const width = p.phase === "download" ? pct : 100;
  const cancel = p.phase === "download" ? `<button class="btn sm" data-deps="cancel">${t("Cancel")}</button>` : "";
  return `<span class="deps-prog${small ? " small" : ""}"><span class="deps-bar"><i style="width:${width.toFixed(1)}%"></i></span><span class="deps-prog-text">${esc(text)}</span>${cancel}</span>`;
}

function depsErrorHTML() {
  return DEPS.error ? `<span class="deps-err">${esc(t(DEPS.error))}</span>` : "";
}

function depsItemRow(it) {
  const state = `<b class="deps-state ${esc(it.state)}">${esc(depsStateText(it.state))}</b>`;
  const ver = it.version ? ` · ${esc(t("Version {v}", { v: it.version }))}` : "";
  const path = it.path ? `<br><span class="path" title="${esc(it.path)}">${esc(it.path)}</span>` : "";
  const note = it.note ? `<br>${esc(t(it.note))}` : "";
  const target = depsTarget(it);
  const btn = it.installable
    ? `<button class="btn sm${it.state === "ok" ? "" : " primary"}" data-deps="install" data-id="${esc(target)}"${DEPS.busy ? " disabled" : ""}>${it.state === "ok" ? t("Reinstall") : t("Install")}</button>`
    : "";
  return row(esc(it.name), state + ver + path + note, btn);
}

/** The "Components" section of Settings. Its first showing starts a check. */
function depsSection() {
  if (!DEPS.checked && !DEPS.checking) setTimeout(depsCheck, 0);
  const items = DEPS.items || [];
  const missing = items.filter((i) => i.installable && i.state === "missing");
  const head = row(
    t("Check and repair"),
    DEPS.checking ? t("Checking…") : !DEPS.items ? "" : missing.length ? t("Missing: {list}", { list: [...new Set(missing.map((i) => i.name))].join(", ") }) : t("Everything Glint needs is installed."),
    `<button class="btn sm" data-deps="check"${DEPS.checking || DEPS.busy ? " disabled" : ""}>${t("Check")}</button>` +
      (missing.length ? `<button class="btn sm primary" data-deps="all"${DEPS.busy ? " disabled" : ""}>${t("Install everything missing")}</button>` : ""),
  );
  return `<div class="section deps" id="deps-sec"><h2>${t("Components")}</h2>
    ${head}
    <div class="deps-live">${depsProgHTML(false)}${depsErrorHTML()}</div>
    ${items.map(depsItemRow).join("")}
  </div>`;
}

/** Shown after a capture error: an "Install ffmpeg" button when ffmpeg is missing (or its progress). */
function depsErrorExtra(text) {
  if (!/ffmpeg\.exe was not found/i.test(String(text || ""))) return "";
  const body = DEPS.busy ? depsProgHTML(true) : `<button class="btn sm primary" data-deps="install" data-id="ffmpeg">${t("Install ffmpeg")}</button>`;
  return ` <span class="deps-sb">${body}${depsErrorHTML()}</span>`;
}

/** Redraw the Components section in place (keeps the settings search applied). */
function depsPaint() {
  const sec = document.getElementById("deps-sec");
  if (sec) {
    sec.outerHTML = depsSection();
    if (typeof applySettingsFilter === "function") applySettingsFilter();
  }
  depsPaintProgress();
}

/** Only the progress bars and error lines (cheap: runs on every progress event). */
function depsPaintProgress() {
  for (const el of document.querySelectorAll(".deps-live")) el.innerHTML = depsProgHTML(false) + depsErrorHTML();
  for (const el of document.querySelectorAll(".deps-sb")) el.innerHTML = (DEPS.busy ? depsProgHTML(true) : `<button class="btn sm primary" data-deps="install" data-id="ffmpeg">${t("Install ffmpeg")}</button>`) + depsErrorHTML();
}

async function depsCheck() {
  if (DEPS.checking) return;
  DEPS.checking = true;
  depsPaint();
  try {
    const r = await invoke("check_deps");
    DEPS.items = r.items || [];
    if (r.installing && !DEPS.busy) DEPS.busy = r.installing; // started before the window was reopened
  } catch (e) {
    DEPS.error = String(e);
  } finally {
    DEPS.checking = false;
    DEPS.checked = true;
    depsPaint();
  }
}

async function depsInstall(id) {
  if (DEPS.busy) return;
  DEPS.busy = id;
  DEPS.error = "";
  DEPS.prog = { id, phase: "download", done: 0, total: 0 };
  depsPaint();
  let ok = false;
  try {
    DEPS.items = (await invoke("install_dep", { id })) || DEPS.items;
    ok = true;
  } catch (e) {
    DEPS.error = String(e);
    DEPS.errorId = id;
    DEPS.queue = [];
  }
  DEPS.busy = "";
  DEPS.prog = null;
  if (ok) {
    if (typeof flash === "function") flash(t("{name} is installed.", { name: DEPS_NAMES[id] || id }));
    // ffmpeg's path and the capture's state changed: read them again (this also redraws the page).
    if (typeof refreshState === "function") { try { await refreshState(); } catch {} }
  } else if (typeof sfx === "function") sfx("error");
  depsPaint();
  const next = DEPS.queue.shift();
  if (next) return depsInstall(next);
}

function depsInstallMissing() {
  const ids = [...new Set((DEPS.items || []).filter((i) => i.installable && i.state === "missing").map(depsTarget))];
  if (!ids.length) return;
  DEPS.queue = ids.slice(1);
  depsInstall(ids[0]);
}

function depsAct(el) {
  const act = el.dataset.deps;
  if (act === "check") { DEPS.error = ""; return depsCheck(); }
  if (act === "install") return depsInstall(el.dataset.id);
  if (act === "all") return depsInstallMissing();
  if (act === "cancel") { DEPS.queue = []; return invoke("cancel_dep_install"); }
}

// The status bar is redrawn every second, which can swallow a click between press and release:
// its buttons act on the press. Everywhere else, on the click.
document.addEventListener("pointerdown", (e) => {
  const el = e.target.closest?.("#sb-state [data-deps]");
  if (el && e.button === 0 && !el.disabled) { e.preventDefault(); depsAct(el); }
});
document.addEventListener("click", (e) => {
  const el = e.target.closest?.("[data-deps]");
  if (el && !el.closest("#sb-state") && !el.disabled) depsAct(el);
});

(function depsListen() {
  const T = window.__TAURI__;
  if (!T || !T.event) return;
  T.event.listen("deps-progress", ({ payload: p } = {}) => {
    if (!p) return;
    if (p.phase === "done" || p.phase === "error") return; // install_dep's answer handles the end
    if (!DEPS.busy) DEPS.busy = p.id;
    DEPS.prog = p;
    depsPaintProgress();
  });
})();
