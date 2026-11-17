"use strict";
// The GAME / DESKTOP switch in the top bar (0.1.27, capmode.rs). Loaded before app.js: nothing
// here touches app.js's globals (S, invoke, esc, placeMenu, ...) at load time, only when called.
//
// - capRender(): draws or updates the switch in #capmode-slot (renderChrome calls it).
// - capStatusText(status): the status bar's text in game mode ("" = leave app.js's text).
// - A click on the other half switches at once (settings.capture_mode, saved like any setting);
//   the thumb slides under the chosen half (capmode.css). Arrow keys, Home / End work too.
// - 0.1.46: the halves show a game pad and a monitor; a tiny tooltip of the switch's own says "Game" / "Desktop"
//   (and why, while GAME waits or carries the warning mark). What each mode records is in the menu's head.
// - Right-click: a menu in the app's own menu style (app.js placeMenu / closeMenu and its #menu)
//   to turn automatic detection on or off, pick the game by hand among the running programs,
//   or say "This is not a game" (game_menu / game_mark).
// Names here start with `cap` / `CAP_`; markup uses `cm-` classes and `data-cm…` attributes.

// 0.1.46: redrawn to read at 16 px: a game pad with two grips (cross on the left, two buttons on the right) and a monitor on its stand.
const CAP_ICON_GAME = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M7.600 6.500h8.800a4.200 4.200 0 0 1 4.100 3.400l.9 4.900a2.600 2.600 0 0 1-4.500 2.200L15.400 15H8.600l-1.500 2a2.600 2.600 0 0 1-4.500-2.200l.9-4.900a4.200 4.200 0 0 1 4.100-3.400z"/><path d="M8.300 9.300v3.400M6.600 11h3.400M15.300 10h.01M17.400 12h.01"/></svg>';
const CAP_ICON_DESK = '<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="3" y="4" width="18" height="12.500" rx="2"/><path d="M8.500 20.500h7M12 16.500v4"/></svg>';
const CAP_ICON_APP = '<svg viewBox="0 0 24 24"><rect x="3.500" y="4.500" width="17" height="15" rx="2"/><path d="M3.500 8.500h17"/></svg>';

const capEsc = (s) => (typeof esc === "function" ? esc(s) : String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]));
const capIsGame = () => typeof S !== "undefined" && !!S.settings && S.settings.capture_mode === "game";

/** 0.1.41: what GAME mode decided and why (status.capture_why / game_menu's decision, gamedetect.rs Why). "" = nothing to say. */
function capWhyText(why) {
  if (!why || !why.code || why.code === "desktop") return "";
  const game = why.game || t("the game");
  const n = (why.monitor || 0) + 1;
  switch (why.code) {
    case "window": return t("GAME: recording {game}", { game });
    case "minimized": return t("GAME: {game} is minimized; keeping its last picture", { game });
    case "exclusive": return t("GAME: {game} runs in exclusive fullscreen; recording its monitor {n} instead", { game, n });
    case "window_failed": return t("GAME: {game}'s window gave no picture; recording its monitor {n} instead", { game, n });
    case "no_window_capture": return t("GAME: window capture is not available on this PC; recording {game}'s monitor {n}", { game, n });
    case "excluded": return t("GAME: {game} blocks screen capture; nothing can record it", { game });
    case "hidden": return t("GAME: {game}'s window is hidden; paused", { game });
    case "paused_away": return t("GAME: {game} is not in front; its monitor is not recorded meanwhile", { game });
    case "waiting_window": return t("GAME: waiting for {game}'s window", { game });
    case "waiting_pick": return t("GAME: waiting for {game} (your pick)", { game });
    case "auto_off": return t("GAME: detection is off and no game is picked");
    default: return t("GAME: waiting for a game");
  }
}

/** Why a program counts as a game, in words (gamedetect.rs Ev codes). */
function capEvText(code) {
  switch (code) {
    case "pick": return t("your pick");
    case "handoff": return t("started from your pick");
    case "marked": return t("marked as a game");
    case "known": return t("known game");
    case "minecraft": return "Minecraft";
    case "seen": return t("played before");
    case "folder": return t("game folder");
    case "fullscreen": return t("fullscreen");
    case "gamewindow": return t("game window");
    case "front": return t("in front for a while");
    case "library:steam": return t("Steam library");
    case "library:epic": return t("Epic library");
    case "library:riot": return t("Riot library");
    case "library:xbox": return t("Xbox app library");
    case "library:gog": return t("GOG library");
    case "library:ubisoft": return t("Ubisoft library");
    case "library:blizzard": return t("Battle.net library");
    case "library:ea": return t("EA app library");
    default: return "";
  }
}
const capEvList = (codes) => (codes || []).map(capEvText).filter(Boolean).join(" · ");
/** Codes that mean GAME is not recording the game's own window right now (the switch shows a mark). */
const CAP_WARN = ["exclusive", "window_failed", "no_window_capture", "excluded", "hidden", "paused_away", "waiting_window", "waiting_pick"];

/** 0.1.46: the reason line under "Game" in the switch's own tooltip, while GAME is waiting or carries the mark ("" otherwise). */
function capReason() {
  const st = (typeof S !== "undefined" && S.status) || {};
  if (!capIsGame() || !st.recording) return "";
  const why = st.capture_why;
  if (why && why.code && CAP_WARN.includes(why.code)) return capWhyText(why);
  if (st.capture_target === "waiting") return why && why.code && why.code !== "window" ? capWhyText(why) : t("Now: waiting for a game");
  return "";
}

/** Sets an attribute only when it differs (the switch is updated on every status poll). */
const capAttr = (el, name, value) => { if (el.getAttribute(name) !== value) el.setAttribute(name, value); };

function capRender() {
  const slot = document.getElementById("capmode-slot");
  if (!slot || typeof S === "undefined" || !S.settings) return;
  let pill = slot.querySelector(".cm-pill");
  if (!pill) {
    // The two halves with the sliding thumb, then a small arrow that opens the menu a right-click opens.
    // 0.1.46: each half shows only its icon; its word is the button's name for screen readers (aria-label) and a
    // tiny tooltip of the switch's own (capTipShow) for the eye. No `title`: the browser's tooltip would come too.
    slot.innerHTML = `<div class="cm-pill">
      <div class="cm-halves" role="radiogroup">
        <span class="cm-thumb" aria-hidden="true"></span>
        <button type="button" class="cm-half" role="radio" data-cm="game">${CAP_ICON_GAME}<span class="cm-label"></span></button>
        <button type="button" class="cm-half" role="radio" data-cm="desktop">${CAP_ICON_DESK}<span class="cm-label"></span></button>
      </div>
      <button type="button" class="cm-more" data-cm-menu data-sfx="tap" aria-haspopup="menu"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="m7 10 5 5 5-5"/></svg></button>
    </div>`;
    pill = slot.querySelector(".cm-pill");
  }
  const more = pill.querySelector(".cm-more");
  if (more) { capAttr(more, "aria-label", t("Choose the game")); capAttr(more, "aria-expanded", String(capMenuOpen())); }
  const game = capIsGame();
  pill.classList.toggle("is-game", game);
  // Waiting for a game: the game half breathes a little (capmode.css), so the reason no clip comes is visible.
  pill.classList.toggle("waiting", game && !!S.status?.recording && S.status?.capture_target === "waiting");
  // Not the game's own window (exclusive fullscreen, refused, paused, waiting for the pick): a mark on GAME, the tooltip says why.
  pill.classList.toggle("cm-warn", game && !!S.status?.recording && CAP_WARN.includes(S.status?.capture_why?.code));
  capAttr(pill.querySelector(".cm-halves") || pill, "aria-label", t("What to record"));
  for (const b of pill.querySelectorAll(".cm-half")) {
    const on = (b.dataset.cm === "game") === game;
    const label = b.dataset.cm === "game" ? t("Game") : t("Desktop");
    const lab = b.querySelector(".cm-label");
    if (lab.textContent !== label) lab.textContent = label; // visually hidden (capmode.css)
    capAttr(b, "aria-checked", String(on));
    capAttr(b, "aria-label", label);
    if (b.tabIndex !== (on ? 0 : -1)) b.tabIndex = on ? 0 : -1; // one stop for Tab; the arrows move between the halves
    const sfx = on ? "none" : b.dataset.cm === "game" ? "on" : "off"; // sfx.js: a switch's sound, none for the half already chosen
    if (b.dataset.sfx !== sfx) b.dataset.sfx = sfx;
    if (b.hasAttribute("title")) b.removeAttribute("title");
  }
  // An open tooltip follows (its reason line comes and goes with the status), in place.
  if (capTip.el && !capTip.el.hidden && capTip.for?.isConnected) capTipFill(capTip.for);
  // The halves are as wide as their icons, so the thumb takes the chosen half's place and width (it
  // slides with a transition, capmode.css). Not measurable (hidden): the CSS fallback of two equal halves.
  const on = pill.querySelector('.cm-half[aria-checked="true"]'), thumb = pill.querySelector(".cm-thumb");
  if (on && thumb && on.offsetWidth > 0) {
    const w = `${on.offsetWidth}px`, tr = `translateX(${on.offsetLeft}px)`;
    if (thumb.style.width !== w) thumb.style.width = w;
    if (thumb.style.transform !== tr) thumb.style.transform = tr;
  }
}

/* ---------- the switch's own tooltip (0.1.46) ----------
   A tiny label under a half ("Game" / "Desktop") or the arrow ("Choose the game"), after about 350 ms of hover or at once
   on keyboard focus. On <body>, fixed, and never takes the pointer or the focus. While GAME waits or carries the
   mark, the game half's tooltip has a second line: the reason. Gone on leave, a press, a click, Esc or a blur. */
const CAP_TIP_MS = 350;
const capTip = { el: null, for: null, timer: 0 };
const capMenuOpen = () => typeof document !== "undefined" && !!document.getElementById("menu")?.classList.contains("cm-menu") && !document.getElementById("menu").hidden;

function capTipTexts(target) {
  if (target.matches(".cm-more")) return [t("Choose the game"), ""];
  const isGame = target.dataset.cm === "game";
  return [isGame ? t("Game") : t("Desktop"), isGame ? capReason() : ""];
}

function capTipFill(target) {
  const [main, why] = capTipTexts(target);
  const el = capTip.el;
  let a = el.firstElementChild, b = a && a.nextElementSibling;
  if (!a) { a = document.createElement("span"); a.className = "cm-tip-main"; el.append(a); }
  if (!b) { b = document.createElement("span"); b.className = "cm-tip-why"; el.append(b); }
  if (a.textContent !== main) a.textContent = main;
  if (b.textContent !== why) b.textContent = why;
  b.hidden = !why;
  el.classList.toggle("cm-tip-warn", !!why);
  capTipPlace(target);
}

function capTipPlace(target) {
  const el = capTip.el, r = target.getBoundingClientRect();
  const w = el.offsetWidth, vw = window.innerWidth;
  el.style.left = `${Math.round(Math.max(6, Math.min(r.left + r.width / 2 - w / 2, vw - w - 6)))}px`;
  el.style.top = `${Math.round(r.bottom + 6)}px`;
}

function capTipShow(target) {
  clearTimeout(capTip.timer);
  capTip.timer = 0;
  if (!target.isConnected) return;
  if (!capTip.el) {
    capTip.el = document.createElement("div");
    capTip.el.className = "cm-tip";
    capTip.el.id = "cm-tip";
    capTip.el.setAttribute("role", "tooltip");
    capTip.el.hidden = true;
    document.body.appendChild(capTip.el);
  }
  if (capTip.for && capTip.for !== target) capTip.for.removeAttribute("aria-describedby");
  capTip.for = target;
  capTip.el.hidden = false;
  capTipFill(target);
  target.setAttribute("aria-describedby", "cm-tip");
}

function capTipHide() {
  clearTimeout(capTip.timer);
  capTip.timer = 0;
  if (capTip.for) capTip.for.removeAttribute("aria-describedby");
  capTip.for = null;
  if (capTip.el) capTip.el.hidden = true;
}

/** Hover: after a short pause; a move to the other half shows its tooltip at once while one is showing. */
function capTipLater(target) {
  if (capTip.for === target && !capTip.el.hidden) return;
  clearTimeout(capTip.timer);
  if (capTip.el && !capTip.el.hidden) return capTipShow(target);
  capTip.timer = setTimeout(() => capTipShow(target), CAP_TIP_MS);
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
  // 0.1.41: the decision's own words whenever it is not simply the game's window.
  const why = st.capture_why;
  if (why && why.code && why.code !== "window") {
    const hint = why.code === "auto_off" ? t("Automatic detection is off: right-click GAME to pick the game")
      : why.code === "waiting" ? t("Glint records the game as soon as one starts")
      : why.code === "waiting_pick" ? t("Right-click GAME to change the pick")
      : "";
    const dot = st.capture_target === "waiting" ? '<span class="cm-wait-dot" aria-hidden="true"></span>' : "";
    const info = st.capture_target === "screen" || st.capture_target === "screen_failed" ? ` · ${st.width}×${st.height} @ ${st.fps} fps` : "";
    return `${dot}<b>${capEsc(capWhyText(why))}</b>${hint ? ` · ${capEsc(hint)}` : ""}${info}`;
  }
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
  // Each program: its name and window title, why it counts as a game (0.1.41), the one recorded now
  // marked, and for a game a small "never" button beside it.
  const rows = (data.programs || []).map((p) => {
    const tag = p.current ? `<i class="cm-tag">${capEsc(data.mode === "game" ? t("recording") : t("detected"))}</i>` : "";
    const label = p.title && p.title !== p.name ? `${p.name} — ${p.title}` : p.name;
    const why = capEvList(p.why);
    const sub = why ? `<small class="cm-why">${capEsc(why)}</small>` : "";
    const never = p.game && !p.not_game && !p.picked ? `<button type="button" class="cm-never" data-capm="not-game" data-arg="${capEsc(p.exe)}" title="${capEsc(t("Never treat this as a game"))}" aria-label="${capEsc(t("Never treat this as a game"))}">${ICON.x}</button>` : "";
    return `<div class="cm-row">${capItem(p.picked ? "unpick" : "pick", label, p.game ? CAP_ICON_GAME : CAP_ICON_APP, p.path || p.exe, `cm-prog${p.picked ? " on" : ""}${p.not_game ? " cm-not" : ""}${p.current ? " cm-cur" : ""}`, tag + sub)}${never}</div>`;
  }).join("");
  const whyNow = data.mode === "game" ? capWhyText(data.decision) : "";
  const whyEv = data.mode === "game" ? capEvList(data.decision?.evidence) : "";
  // 0.1.46: what each mode records (the halves' long tooltips until 0.1.45), the chosen one first.
  const modeGame = t("Game: records only the game, its window and its own sound (and your microphone).");
  const modeDesk = t("Desktop: records the whole monitor and all the sound of the PC.");
  const modes = (data.mode === "game" ? [modeGame, modeDesk] : [modeDesk, modeGame]).map((s, i) => `<span class="${i ? "" : "cm-mode-on"}">${capEsc(s)}</span>`).join("");
  let html = `<div class="menu-head" title="${capEsc(head)}">${capEsc(head)}</div>`
    + `<div class="menu-hint cm-modes">${modes}</div>`
    + (whyNow ?`<div class="menu-hint cm-whyline"><b>${capEsc(t("Why?"))}</b> ${capEsc(whyNow)}${whyEv ? ` (${capEsc(whyEv)})` : ""}</div>` : "")
    + capItem(data.auto ? "auto-off" : "auto-on", t("Detect the game automatically"), ICON.refresh, "", data.auto ? "on" : "")
    + "<hr>"
    + `<div class="cm-sub">${capEsc(t("Pick the game by hand"))}</div>`
    + (rows ? `<div class="cm-list">${rows}</div>` : `<div class="menu-hint">${capEsc(t("No program with a window is running."))}</div>`)
    + capItem("choose", t("Choose a program…"), ICON.folder || "", "", "cm-choose");
  if (cur && !cur.picked) html += "<hr>" + capItem("not-game", t("This is not a game"), ICON.x, cur.exe, "danger");
  if (data.pick) html += (cur && !cur.picked ? "" : "<hr>") + capItem("unpick", t("Stop using the game I picked"), ICON.x, data.pick);
  if (data.mode !== "game") html += `<div class="menu-hint">${capEsc(t("Desktop mode uses this to name clips. Switch to GAME to record only the game."))}</div>`;
  $("#menu").innerHTML = html;
  placeMenu(x, y);
  const m = $("#menu");
  m.classList.add("cm-menu");
  // 0.1.46: placed again with its own class on (narrower, so taller), and kept inside a small window: taller than the
  // window, it gets the window's height less a margin and scrolls inside.
  const room = window.innerHeight - 16;
  let r = m.getBoundingClientRect();
  if (r.height > room) { m.style.maxHeight = `${room}px`; m.style.overflowY = "auto"; r = m.getBoundingClientRect(); }
  m.style.top = `${Math.max(8, Math.min(y, window.innerHeight - r.height - 8))}px`;
  m.style.left = `${Math.max(8, Math.min(x, window.innerWidth - r.width - 8))}px`;
  capRender(); // the arrow's aria-expanded
}

async function capMark(action, exe) {
  try {
    if (action === "choose") {
      // "Choose a program…": a file dialog for the game's .exe (capmode.rs game_pick_exe). Cancelled: nothing changes.
      const chosen = await invoke("game_pick_exe");
      if (!chosen) return;
      if (typeof S !== "undefined") S.settings = chosen;
      flash(t("Glint records this program as the game from now on."));
      if (typeof pollStatus === "function") await pollStatus();
      else if (typeof renderChrome === "function") renderChrome();
      return;
    }
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
    if (half) { capTipHide(); return capSet(half.dataset.cm); }
    // The arrow: the same menu as a right-click, under the switch. A click while that menu is open closes it
    // (app.js menuToggle stops this click before it gets here).
    const more = e.target.closest?.("#capmode-slot [data-cm-menu]");
    if (more) { capTipHide(); const r = more.closest(".cm-pill").getBoundingClientRect(); e.stopPropagation(); return capOpenMenu(r.left, r.bottom + 6); }
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
    capTipHide();
    capOpenMenu(e.clientX, e.clientY);
  });
  // The tooltip (0.1.46): hover after a pause, keyboard focus at once; anything the user does hides it.
  const capTipTarget = (n) => n?.closest?.("#capmode-slot .cm-half, #capmode-slot .cm-more") || null;
  document.addEventListener("pointerover", (e) => {
    if (e.pointerType === "touch") return;
    const el = capTipTarget(e.target);
    if (el) capTipLater(el);
  });
  document.addEventListener("pointerout", (e) => {
    const el = capTipTarget(e.target);
    if (!el || capTipTarget(e.relatedTarget) === el) return;
    if (capTipTarget(e.relatedTarget)) return; // onto the other half or the arrow: its own pointerover follows
    if (capTip.for === el && document.activeElement === el && el.matches(":focus-visible")) return; // the keyboard's still
    capTipHide();
  });
  document.addEventListener("pointerdown", () => { if (capTip.timer || capTip.for) capTipHide(); }, true);
  document.addEventListener("focusin", (e) => {
    const el = capTipTarget(e.target);
    if (el && el.matches(":focus-visible")) capTipShow(el);
  });
  document.addEventListener("focusout", (e) => { if (capTip.for && capTipTarget(e.target) === capTip.for) capTipHide(); });
  window.addEventListener("keydown", (e) => { if (e.key === "Escape" && (capTip.for || capTip.timer)) capTipHide(); }, true);
  window.addEventListener("blur", capTipHide);
  window.addEventListener("scroll", () => { if (capTip.for) capTipHide(); }, true);
  window.addEventListener("resize", () => { if (capTip.for) capTipHide(); });
}
