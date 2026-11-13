"use strict";
// The update button in the top bar: an arrow rising out of a tray. Glint itself
// checks GitHub about once a day; a click checks right now. When a newer release
// exists the button fills with the accent colour and says "Update 0.2.0", and a
// click offers to download it and restart.
// Loaded after app.js, so its globals (S, $, invoke, ask, flash, t, ...) exist.

// The arrow is its own path so it can bob inside the tray while a check runs.
const UPD_ICON = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4.5 14.5v3a2.5 2.5 0 0 0 2.5 2.5h10a2.5 2.5 0 0 0 2.5-2.5v-3"/><path class="upd-arrow" d="M12 15V4.5M7.5 8.5 12 4l4.5 4.5"/></svg>';
let updInfo = { current: "", latest: "", available: false, error: "" };
let updBusy = ""; // "" | "checking" | "installing"
let updPct = 0;

/** Which look the button has: "idle" | "checking" | "installing" | "new" | "error". */
const updState = () => updBusy || (updInfo.available ? "new" : updInfo.error ? "error" : "idle");

function updRender() {
  const slot = $("#update-slot");
  if (!slot) return;
  let btn = slot.querySelector(".upd-btn");
  if (!btn) {
    // Made once and then updated in place, so a click is never lost to a redraw.
    slot.innerHTML = `<button type="button" class="btn upd-btn">${UPD_ICON}<span class="upd-label"></span><i class="upd-warn" aria-hidden="true">!</i></button>`;
    btn = slot.firstElementChild;
    btn.addEventListener("click", updClick);
  }
  const st = updState();
  const pct = Math.round(Math.min(100, Math.max(0, updPct)));
  const have = updInfo.current ? t("Installed: Glint {version}", { version: updInfo.current }) : "";
  const tip = st === "installing" ? t("Downloading the update: {pct} %", { pct })
    : st === "checking" ? t("Checking for updates")
    : st === "new" ? t("Glint {version} is available. Click to update", { version: updInfo.latest })
    : st === "error" ? [t("Could not check for updates: {error}", { error: t(updInfo.error) }), t("Click to try again"), have].filter(Boolean).join("\n")
    : [t("Check for updates"), have].filter(Boolean).join("\n");
  const label = st === "installing" ? `${pct} %` : st === "new" ? t("Update {version}", { version: updInfo.latest }) : "";
  btn.title = tip;
  btn.setAttribute("aria-label", tip.split("\n")[0]);
  for (const s of ["checking", "installing", "new", "error"]) btn.classList.toggle(`upd-${s}`, st === s);
  btn.classList.toggle("upd-has-label", !!label);
  const lab = btn.querySelector(".upd-label");
  if (lab.textContent !== label) lab.textContent = label;
  btn.style.setProperty("--upd-pct", `${st === "installing" ? pct : 0}%`);
  // The bug report button sits next to this one; its tooltip follows a language change the same way.
  if (typeof bugRender === "function") bugRender();
}

async function updClick() {
  if (updBusy) return;
  try {
    if (!updInfo.available) {
      // A left click checks by hand.
      updBusy = "checking";
      updRender();
      updInfo = (await invoke("update_check")) || updInfo;
      updBusy = "";
      updRender();
      if (updInfo.error) return flash(t(updInfo.error), true);
      if (!updInfo.available) return flash(t("Glint is up to date (version {version})", { version: updInfo.current }));
    }
    const rendering = typeof jobsList !== "undefined" && jobsList.some((j) => j.state === "queued" || j.state === "running");
    const question = rendering
      ? t("Glint {version} is available. Updating restarts Glint, which stops the renders that are running. Download it and restart now?", { version: updInfo.latest })
      : t("Glint {version} is available (you have {current}). Download it and restart Glint now?", { version: updInfo.latest, current: updInfo.current });
    if (!(await ask(question, t("Update and restart")))) return;
    updBusy = "installing";
    updPct = 0;
    updRender();
    await invoke("update_install"); // Glint restarts itself when this succeeds
  } catch (e) {
    fail(e);
  } finally {
    updBusy = "";
    updRender();
  }
}

(async function updInit() {
  updRender();
  const ev = window.__TAURI__?.event;
  if (ev) {
    ev.listen("update-changed", ({ payload }) => { if (payload) { updInfo = payload; updRender(); } });
    ev.listen("update-progress", ({ payload }) => { updPct = payload?.pct || 0; updRender(); });
  }
  try { updInfo = (await invoke("update_state")) || updInfo; } catch (e) { console.error("Glint: update_state failed", e); }
  updRender();
})();
