"use strict";
// The bug report button in the top bar and its dialog. The backend does the
// sending (to the developers' Discord); this file only collects the text and
// shows, before anything leaves the PC, exactly what will be sent.
// Loaded after app.js and updater.js, so their globals (S, $, $$, invoke, esc, t, flash, fail) exist.
// All names here start with `bug` / `BUG_`; markup uses `bug-` classes and `data-bug` attributes.
// While a `.bug-dlg` exists the app's own key handlers stand aside; this file's
// capture-phase listener handles Esc and Ctrl+Enter, and the dialog traps Tab itself.

const BUG_ICON = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 8.2V7.5a3 3 0 0 1 6 0v.7"/><path d="M10.2 4.8 9 3.3M13.8 4.8 15 3.3"/><rect x="7.5" y="8.2" width="9" height="12" rx="4.5"/><path d="M12 12v8.2M7.5 13.5H4M16.5 13.5H20M7.8 10.2 5 8.5M16.2 10.2 19 8.5M7.9 17.4 5 19.5M16.1 17.4 19 19.5"/></svg>';
const BUG_CHECK = '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="m7.8 12.3 2.8 2.8 5.6-5.8"/></svg>';
const BUG_MIN = 10;          // the backend refuses shorter reports
const BUG_MAX = 4000;        // ... and longer ones
const BUG_CONTACT_MAX = 100;
const BUG_ISSUES = "https://github.com/swamie/glint/issues";

let bugDraft = { text: "", contact: "", includeLog: true }; // kept between openings until a report is sent
let bugCur = null;       // the open dialog: { root, info, mode: "form" | "off" | "done", onKey, onFocus, downOnBack }
let bugOpening = false;  // bug_report_info is being asked for
let bugSending = false;  // send_bug_report is running (it may outlive the dialog that started it)

const bugReady = (text) => String(text).trim().length >= BUG_MIN;

/** Next index for Tab (or Shift+Tab) among `n` focusable elements, from index `i` (-1 when focus is elsewhere). */
function bugCycle(i, n, back) {
  if (n <= 0) return -1;
  if (i < 0 || i >= n) return back ? n - 1 : 0;
  return (i + (back ? -1 : 1) + n) % n;
}

function bugRender() {
  const slot = $("#bug-slot");
  if (!slot) return;
  let btn = slot.querySelector(".bug-btn");
  if (!btn) {
    slot.innerHTML = `<button type="button" class="btn bug-btn" aria-haspopup="dialog" aria-expanded="false">${BUG_ICON}</button>`;
    btn = slot.firstElementChild;
    btn.addEventListener("click", bugOpen);
  }
  const tip = t("Report a bug");
  if (btn.title !== tip) { btn.title = tip; btn.setAttribute("aria-label", tip); }
  btn.setAttribute("aria-expanded", String(!!bugCur));
}

async function bugOpen() {
  if (bugCur || bugOpening) return;
  bugOpening = true;
  let info;
  try { info = (await invoke("bug_report_info")) || {}; } catch (e) { fail(e); return; } finally { bugOpening = false; }
  if (bugCur) return;
  const root = document.createElement("div");
  root.className = "bug-dlg";
  root.setAttribute("role", "dialog");
  root.setAttribute("aria-modal", "true");
  root.setAttribute("aria-labelledby", "bug-title");
  root.innerHTML = `<div class="bug-box"><div class="bug-head">${BUG_ICON}<h2 id="bug-title">${esc(t("Report a bug"))}</h2></div><div class="bug-main"></div></div>`;
  const dlg = { root, info, mode: info.configured ? "form" : "off", downOnBack: false };

  dlg.onKey = (e) => {
    if (e.isComposing) return; // a Korean (IME) syllable is still being typed
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); bugClose(); }
    else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); e.stopPropagation(); if (dlg.mode === "form") bugSend(); }
  };
  // Focus that wanders out of the dialog (a click elsewhere, a stray focus() call) is brought back.
  dlg.onFocus = (e) => { if (bugCur === dlg && !root.contains(e.target)) bugFocusFirst(dlg); };
  window.addEventListener("keydown", dlg.onKey, true);
  document.addEventListener("focusin", dlg.onFocus, true);

  root.addEventListener("keydown", (e) => {
    if (e.key !== "Tab") return;
    e.preventDefault();
    const list = bugFocusables(root);
    const next = bugCycle(list.indexOf(document.activeElement), list.length, e.shiftKey);
    if (next >= 0) list[next].focus();
  });
  // The dimmed area closes the dialog only when nothing has been typed (and only for a click that also started there).
  root.addEventListener("mousedown", (e) => { dlg.downOnBack = e.target === root; });
  root.addEventListener("click", (e) => {
    if (e.target === root) {
      if (dlg.downOnBack && !(dlg.mode === "form" && bugDraft.text.trim())) bugClose();
      return;
    }
    const b = e.target.closest("[data-bug]");
    if (!b || b.disabled) return;
    if (b.dataset.bug === "send") bugSend();
    else bugClose();
  });
  root.addEventListener("input", (e) => bugTake(dlg, e.target));
  root.addEventListener("change", (e) => bugTake(dlg, e.target));

  // In full screen only the full-screen element is drawn, so the dialog goes inside it (a <video> cannot hold it).
  const fs = document.fullscreenElement;
  (fs && !(fs instanceof HTMLMediaElement) ? fs : document.body).appendChild(root);
  bugCur = dlg;
  bugShow(dlg);
  bugRender();
}

function bugClose() {
  const dlg = bugCur;
  if (!dlg) return;
  bugCur = null;
  window.removeEventListener("keydown", dlg.onKey, true);
  document.removeEventListener("focusin", dlg.onFocus, true);
  dlg.root.remove(); // the listeners on the dialog itself go with it
  bugRender();
  const btn = $("#bug-slot .bug-btn");
  if (btn && !document.fullscreenElement) btn.focus({ preventScroll: true });
}

function bugFocusables(root) {
  return $$('button, input, textarea, select, summary, a[href], [tabindex]:not([tabindex="-1"])', root)
    .filter((el) => !el.disabled && el.getClientRects().length > 0); // skips what a closed fold-out or [hidden] hides
}

function bugFocusFirst(dlg) {
  const r = dlg.root;
  const el = dlg.mode === "form" ? $(".bug-text", r) : dlg.mode === "off" ? $(".bug-link", r) : $('[data-bug="close"]', r);
  if (!el) return;
  el.focus({ preventScroll: true });
  if (el.matches(".bug-text")) el.setSelectionRange(el.value.length, el.value.length);
  else if (el.matches(".bug-link")) el.select();
}

/** Draw the dialog's content for its mode. */
function bugShow(dlg) {
  const r = dlg.root, info = dlg.info;
  const main = $(".bug-main", r);
  if (dlg.mode === "off") {
    // A build without a webhook: point at GitHub, as text to copy (Glint does not open a browser from here).
    main.innerHTML = `<p class="bug-sub">${esc(t("Reporting a bug from the app is not set up in this build of Glint. You can report it on GitHub instead:"))}</p>
      <input class="field bug-link" type="text" readonly spellcheck="false" aria-label="${esc(t("Address of the GitHub issues page"))}">
      ${info.system ? `<div class="bug-pre-lab">${esc(t("Details worth adding to the report"))}</div><pre class="bug-pre" tabindex="0">${esc(info.system)}</pre>` : ""}
      <div class="bug-row"><button type="button" class="btn primary" data-bug="close">${esc(t("Close"))}</button></div>`;
    $(".bug-link", r).value = BUG_ISSUES;
  } else if (dlg.mode === "done") {
    main.innerHTML = `<div class="bug-done">${BUG_CHECK}<p>${esc(t("Thanks! Your report was sent to the Glint developers."))}</p></div>
      <div class="bug-row"><button type="button" class="btn primary" data-bug="close">${esc(t("Close"))}</button></div>`;
  } else {
    main.innerHTML = `<p class="bug-sub">${esc(t("This goes to the Glint developers on Discord. Nothing is sent until you press Send."))}</p>
      <label class="bug-lab" for="bug-text">${esc(t("What happened, and what did you expect?"))}</label>
      <textarea id="bug-text" class="field bug-text" rows="7" maxlength="${BUG_MAX}" spellcheck="true" placeholder="${esc(t("For example: I pressed the clip key in a game, but no clip was saved."))}"></textarea>
      <div class="bug-meta"><span class="bug-hint"></span><span class="bug-count" aria-live="polite"></span></div>
      <label class="bug-lab" for="bug-contact">${esc(t("How can we reach you? (optional)"))}</label>
      <input id="bug-contact" class="field bug-contact" type="text" maxlength="${BUG_CONTACT_MAX}" spellcheck="false" autocomplete="off" placeholder="${esc(t("For example your Discord name"))}">
      <label class="check bug-check"><input type="checkbox" class="bug-inc-log"><span>${esc(t("Include the end of Glint's log"))}</span></label>
      <details class="bug-sent"><summary>${esc(t("What will be sent"))}</summary>
        <p class="bug-sent-note">${esc(t("Your description and how to reach you, and:"))}</p>
        <div class="bug-pre-lab">${esc(t("This PC and Glint"))}</div>
        <pre class="bug-pre" tabindex="0">${esc(info.system || "")}</pre>
        <div class="bug-log-part"><div class="bug-pre-lab">${esc(t("The end of Glint's log"))}</div>
        <pre class="bug-pre bug-log" tabindex="0">${esc(info.log || t("(The log is empty.)"))}</pre></div>
      </details>
      <div class="bug-err" role="alert" hidden></div>
      <div class="bug-row"><button type="button" class="btn" data-bug="cancel">${esc(t("Cancel"))}</button><button type="button" class="btn primary bug-send" data-bug="send"></button></div>`;
    // Values are set as properties, so nothing typed is ever parsed as markup.
    $(".bug-text", r).value = bugDraft.text;
    $(".bug-contact", r).value = bugDraft.contact;
    $(".bug-inc-log", r).checked = !!bugDraft.includeLog;
    dlg.sendKey = "";
    bugSync(dlg);
  }
  bugFocusFirst(dlg);
}

/** Copy what the user changed into the draft (kept if the dialog is closed without sending). */
function bugTake(dlg, el) {
  if (dlg.mode !== "form" || !el) return;
  if (el.matches(".bug-text")) bugDraft.text = el.value;
  else if (el.matches(".bug-contact")) bugDraft.contact = el.value;
  else if (el.matches(".bug-inc-log")) bugDraft.includeLog = el.checked;
  else return;
  bugSync(dlg);
}

/** Bring the form's count, hint, Send button and log preview up to date. */
function bugSync(dlg) {
  if (dlg.mode !== "form") return;
  const r = dlg.root, n = bugDraft.text.length, ready = bugReady(bugDraft.text);
  const count = $(".bug-count", r);
  count.textContent = `${n} / ${BUG_MAX}`;
  count.classList.toggle("bug-near", n > BUG_MAX - 200);
  $(".bug-hint", r).textContent = ready ? "" : t("At least {n} characters", { n: BUG_MIN });
  $(".bug-log-part", r).hidden = !bugDraft.includeLog;
  for (const el of $$(".bug-text, .bug-contact", r)) el.readOnly = bugSending;
  $(".bug-inc-log", r).disabled = bugSending;
  const send = $(".bug-send", r);
  send.disabled = bugSending || !ready;
  const key = bugSending ? "sending" : "send";
  if (dlg.sendKey !== key) { // rebuilt only when it changes, so a hover or click is not disturbed
    dlg.sendKey = key;
    send.innerHTML = bugSending ? `<i class="bug-spin" aria-hidden="true"></i>${esc(t("Sending"))}` : esc(t("Send"));
  }
}

function bugShowErr(dlg, msg) {
  const el = $(".bug-err", dlg.root);
  if (!el) return;
  el.textContent = msg;
  el.hidden = !msg;
}

async function bugSend() {
  const dlg = bugCur;
  if (!dlg || dlg.mode !== "form" || bugSending || !bugReady(bugDraft.text)) return;
  bugSending = true;
  bugShowErr(dlg, "");
  bugSync(dlg);
  try {
    await invoke("send_bug_report", { text: bugDraft.text.trim(), contact: bugDraft.contact.trim(), includeLog: !!bugDraft.includeLog });
    bugDraft = { text: "", contact: "", includeLog: true };
    bugSending = false;
    // The dialog may have been closed (or closed and opened again) while this ran.
    if (bugCur) { bugCur.mode = "done"; bugShow(bugCur); }
    else flash(t("Thanks! Your report was sent to the Glint developers."));
  } catch (e) {
    bugSending = false;
    const msg = t(String(e?.message ?? e));
    if (bugCur && bugCur.mode === "form") {
      bugShowErr(bugCur, msg);
      bugSync(bugCur);
      // Disabling Send while it ran dropped the focus to the page; put it back in the dialog.
      if (!bugCur.root.contains(document.activeElement)) $(".bug-send", bugCur.root).focus({ preventScroll: true });
    } else flash(msg, true);
  } finally {
    bugSending = false;
    if (bugCur && bugCur.mode === "form") bugSync(bugCur);
  }
}

bugRender();
