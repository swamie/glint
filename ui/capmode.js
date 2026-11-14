"use strict";
// The GAME / DESKTOP switch in the top bar (0.1.27, capmode.rs). Loaded before app.js: nothing
// here touches app.js's globals (S, invoke, esc, placeMenu, ...) at load time, only when called.
//
// - capRender(): draws or updates the switch in #capmode-slot (renderChrome calls it).
// - capStatusText(status): the status bar's text in game mode ("" = leave app.js's text).
// - A click on the other half switches at once (settings.capture_mode, saved like any setting);
//   the thumb slides under the chosen half (capmode.css). Arrow keys, Home / End work too.
// - Right-click: a menu in the app's own menu style (app.js placeMenu / closeMenu and its #menu)
//   to turn automatic detection on or off, pick the game by hand among the running programs,
//   or say "This is not a game" (game_menu / game_mark).
// Names here start with `cap` / `CAP_`; markup uses `cm-` classes and `data-cm…` attributes.

const CAP_ICON_GAME = '<svg viewBox="0 0 24 24"><rect x="2.500" y="7" width="19" height="10" rx="5"/><path d="M7 10.500v3M5.500 12h3M15.500 11h.01M18 13h.01"/></svg>';
const CAP_ICON_DESK = '<svg viewBox="0 0 24 24"><rect x="3" y="4.500" width="18" height="12" rx="2"/><path d="M9 20h6M12 16.500V20"/></svg>';
const CAP_ICON_APP = '<svg viewBox="0 0 24 24"><rect x="3.500" y="4.500" width="17" height="15" rx="2"/><path d="M3.500 8.500h17"/></svg>';

const capEsc = (s) => (typeof esc === "function" ? esc(s) : String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]));
const capIsGame = () => typeof S !== "undefined" && !!S.settings && S.settings.capture_mode === "game";

/** The tooltip of each half. The game half also says what it is doing now. */
function capTips() {
  const st = (typeof S !== "undefined" && S.status) || {};
  let now = "";
  if (capIsGame() && st.recording) {
    if (st.capture_target === "waiting") now = t("Now: waiting for a game");
    else if (st.game && st.capture_target === "window") now = t("Now: recording {game}", { game: st.game });
    else if (st.game) now = t("Now: recording {game} (whole screen)", { game: st.game });
  }
  const game = t("Game: records only the game, its window and its own sound (and your microphone).") + "\n" + t("Right-click to choose the game.") + (now ? "\n" + now : "");
  const desk = t("Desktop: records the whole monitor and all the sound of the PC.") + "\n" + t("Right-click to choose the game.");
  return { game, desk };
}

function capRender() {
  const slot = document.getElementById("capmode-slot");
  if (!slot || typeof S === "undefined" || !S.settings) return;
  let pill = slot.querySelector(".cm-pill");
  if (!pill) {
    slot.innerHTML = `<div class="cm-pill" role="radiogroup">
      <span class="cm-thumb" aria-hidden="true"></span>
      <button type="button" class="cm-half" role="radio" data-cm="game">${CAP_ICON_GAME}<span class="cm-label"></span></button>
      <button type="button" class="cm-half" role="radio" data-cm="desktop">${CAP_ICON_DESK}<span class="cm-label"></span></button>
    </div>`;
    pill = slot.querySelector(".cm-pill");
  }
  const game = capIsGame();
  const tips = capTips();
  pill.classList.toggle("is-game", game);
  // Waiting for a game: the game half breathes a little (capmode.css), so the reason no clip comes is visible.
  pill.classList.toggle("waiting", game && !!S.status?.recording && S.status?.capture_target === "waiting");
  pill.setAttribute("aria-label", t("What to record"));
  for (const b of pill.querySelectorAll(".cm-half")) {
    const on = (b.dataset.cm === "game") === game;
    const label = b.dataset.cm === "game" ? t("GAME") : t("DESKTOP");
    const lab = b.querySelector(".cm-label");
    if (lab.textContent !== label) lab.textContent = label;
    b.setAttribute("aria-checked", String(on));
    b.setAttribute("aria-label", label);
    b.tabIndex = on ? 0 : -1; // one stop for Tab; the arrows move between the halves
    b.dataset.sfx = on ? "none" : b.dataset.cm === "game" ? "on" : "off"; // sfx.js: a switch's sound, none for the half already chosen
    b.title = b.dataset.cm === "game" ? tips.game : tips.desk;
  }
}

/** Switch to `mode` ("game" | "desktop"). The capture follows in the backend (apply_settings). */
function capSet(mode) {
  if (typeof S === "undefined" || !S.settings || (mode !== "game" && mode !== "desktop")) return;
  if (S.settings.capture_mode === mode) return;
  S.settings.capture_mode = mode;
  saveSettings(); // redraws the chrome at once: the thumb slides now, the save follows
}

/** The status bar in game mode: waiting, or the whole screen as a fallback. "" = app.js's own text. */
function capStatusText(st) {
  if (!st || st.capture_mode !== "game" || !st.recording || st.error) return "";
  if (st.capture_target === "waiting") {
    const hint = S.settings && !S.settings.game_auto && !S.settings.game_pick
      ? t("Automatic detection is off: right-click GAME to pick the game")
      : t("Glint records the game as soon as one starts");
    return `<span class="cm-wait-dot" aria-hidden="true"></span><b>${capEsc(t("Waiting for a game"))}</b> · ${capEsc(hint)}`;
  }
  if (st.capture_target === "screen" || st.capture_target === "screen_failed") {
    const why = st.capture_target === "screen" ? t("window capture is not available here") : t("its window could not be captured");
    const info = `${st.width}×${st.height} @ ${st.fps} fps`;
    return `<b>${capEsc(t("Clipping"))}</b> ${capEsc(st.game || "")} · ${capEsc(t("whole screen"))} (${capEsc(why)}) · ${info}`;
  }
  return "";
}

/* ---------- the right-click menu ---------- */
let capMenuData = null;

const capItem = (act, label, icon, arg = "", cls = "", extra = "") =>
  `<button type="button" data-capm="${act}" data-arg="${capEsc(arg)}" class="${cls}">${icon || "<svg></svg>"}<span>${capEsc(label)}</span>${extra}</button>`;

async function capOpenMenu(x, y) {
  let data;
  try { data = await invoke("game_menu"); } catch (e) { return typeof fail === "function" ? fail(e) : console.warn("Glint: game_menu failed", e); }
  if (!data) return;
  capMenuData = data;
  if (typeof menuIds !== "undefined") menuIds = [];
  const cur = data.current;
  const head = cur ? t("Game: {game}", { game: cur.name }) : t("No game found");
  const rows = (data.programs || []).map((p) => {
    const tag = p.current && !p.picked ? `<i class="cm-tag">${capEsc(t("detected"))}</i>` : "";
    const label = p.title && p.title !== p.name ? `${p.name} — ${p.title}` : p.name;
    return capItem(p.picked ? "unpick" : "pick", label, p.game ? CAP_ICON_GAME : CAP_ICON_APP, p.exe, `cm-prog${p.picked ? " on" : ""}${p.not_game ? " cm-not" : ""}`, tag);
  }).join("");
  let html = `<div class="menu-head" title="${capEsc(head)}">${capEsc(head)}</div>`
    + capItem(data.auto ? "auto-off" : "auto-on", t("Detect the game automatically"), ICON.refresh, "", data.auto ? "on" : "")
    + "<hr>"
    + `<div class="cm-sub">${capEsc(t("Pick the game by hand"))}</div>`
    + (rows ? `<div class="cm-list">${rows}</div>` : `<div class="menu-hint">${capEsc(t("No program with a window is running."))}</div>`);
  if (cur && !cur.picked) html += "<hr>" + capItem("not-game", t("This is not a game"), ICON.x, cur.exe, "danger");
  if (data.pick) html += (cur && !cur.picked ? "" : "<hr>") + capItem("unpick", t("Stop using the game I picked"), ICON.x, data.pick);
  if (data.mode !== "game") html += `<div class="menu-hint">${capEsc(t("Desktop mode uses this to name clips. Switch to GAME to record only the game."))}</div>`;
  $("#menu").innerHTML = html;
  placeMenu(x, y);
  $("#menu").classList.add("cm-menu");
}

async function capMark(action, exe) {
  try {
    const saved = await invoke("game_mark", { action, exe });
    if (saved && typeof S !== "undefined") S.settings = saved;
    if (action === "not-game") flash(t("Marked as not a game. Glint will not record it again."));
    else if (action === "pick") flash(t("Glint records this program as the game from now on."));
    if (typeof pollStatus === "function") await pollStatus();
    else if (typeof renderChrome === "function") renderChrome();
  } catch (e) { if (typeof fail === "function") fail(e); }
}

/* ---------- wiring ---------- */
if (typeof document !== "undefined") {
  document.addEventListener("click", (e) => {
    const half = e.target.closest?.("#capmode-slot [data-cm]");
    if (half) return capSet(half.dataset.cm);
    const item = e.target.closest?.("#menu [data-capm]");
    if (!item) return;
    if (typeof closeMenu === "function") closeMenu();
    capMark(item.dataset.capm, item.dataset.arg || "");
  });
  document.addEventListener("keydown", (e) => {
    const half = e.target.closest?.("#capmode-slot [data-cm]");
    if (!half) return;
    const to = { ArrowLeft: "game", ArrowUp: "game", Home: "game", ArrowRight: "desktop", ArrowDown: "desktop", End: "desktop" }[e.key];
    if (!to) return;
    e.preventDefault();
    capSet(to);
    document.querySelector(`#capmode-slot [data-cm="${to}"]`)?.focus();
  });
  // Registered before app.js's own right-click handler (this file loads first), which would close the menu again.
  document.addEventListener("contextmenu", (e) => {
    if (!e.target.closest?.("#capmode-slot")) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    capOpenMenu(e.clientX, e.clientY);
  });
}
