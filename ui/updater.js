"use strict";
// The update button in the top bar: a small up arrow. Glint itself checks
// GitHub about once a day; a click checks right now. When a newer release
// exists the arrow lights up, and a click offers to download it and restart.
// Loaded after app.js, so its globals (S, $, invoke, ask, flash, t, ...) exist.

const UPD_ARROW = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 14 6-6 6 6"/></svg>';
let updInfo = { current: "", latest: "", available: false, error: "" };
let updBusy = ""; // "" | "checking" | "installing"
let updPct = 0;

function updRender() {
  const slot = $("#update-slot");
  if (!slot) return;
  let btn = slot.querySelector(".upd-btn");
  if (!btn) {
    slot.innerHTML = `<button type="button" class="upd-btn">${UPD_ARROW}<i class="upd-dot" aria-hidden="true"></i></button>`;
    btn = slot.firstElementChild;
    btn.addEventListener("click", updClick);
  }
  const tip = updBusy === "installing" ? t("Downloading the update: {pct} %", { pct: Math.round(updPct) })
    : updBusy === "checking" ? t("Checking for updates")
    : updInfo.available ? t("Glint {version} is available. Click to update", { version: updInfo.latest })
    : t("Check for updates");
  btn.title = tip;
  btn.setAttribute("aria-label", tip);
  btn.classList.toggle("upd-new", !!updInfo.available && !updBusy);
  btn.classList.toggle("upd-busy", !!updBusy);
  btn.style.setProperty("--upd-pct", `${updBusy === "installing" ? updPct : 0}%`);
}

async function updClick() {
  if (updBusy) return;
  try {
    if (!updInfo.available) {
      // A left click on the arrow checks by hand.
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
