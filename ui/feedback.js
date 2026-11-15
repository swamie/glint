"use strict";
// The heart button in the top bar (next to the bug button) and its feedback
// dialog: five stars, a few words, an optional contact, and a plain preview of
// exactly what will be sent. The backend (bugreport.rs send_feedback) posts it
// to the same Discord webhook as bug reports, pinging no one.
// Loaded BEFORE app.js: nothing here runs at load. bugRender (bugreport.js)
// calls fbRender, so the button appears with the bug button and its tooltip
// follows a language change the same way.
// The dialog's root carries `bug-dlg` too, so the app's key handlers stand
// aside for it exactly as for the bug dialog, and it shares that dialog's look.
// All names here start with `fb` / `FB_`; markup uses `fb-` classes and `data-fb` attributes.

const FB_ICON = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 20s-7-4.4-7-10a4 4 0 0 1 7-2.6A4 4 0 0 1 19 10c0 5.6-7 10-7 10z"/></svg>';
const FB_STAR = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m12 3.3 2.7 5.5 6 .9-4.4 4.2 1 6-5.3-2.8-5.3 2.8 1-6-4.4-4.2 6-.9z"/></svg>';
const FB_MAX = 4000;          // the backend refuses longer texts
const FB_CONTACT_MAX = 100;
const FB_STARS = 5;

let fbDraft = { stars: 0, text: "", contact: "" }; // kept between openings until feedback is sent
let fbCur = null;       // the open dialog: { root, info, mode: "form" | "off" | "done", hover, onKey, onFocus, downOnBack }
let fbOpening = false;  // feedback_info is being asked for
let fbSending = false;  // send_feedback is running (it may outlive the dialog that started it)

// ---- Pure logic (no DOM; tested with node).

/** Whole stars 0..5 from anything. */
const fbClampStars = (n) => Math.max(0, Math.min(FB_STARS, Math.round(Number(n) || 0)));

/** Can this be sent? Stars, or some text (or both), and the text within the limit. */
const fbReady = (stars, text) => {
  const len = String(text ?? "").trim().length;
  return (fbClampStars(stars) > 0 || len > 0) && len <= FB_MAX;
};

/** The stars as they are sent: "★★★★☆ 4 / 5", or "No stars given". */
function fbStarsText(stars) {
  const n = fbClampStars(stars);
  return n ? "★".repeat(n) + "☆".repeat(FB_STARS - n) + ` ${n} / ${FB_STARS}` : "No stars given";
}

/**
 * Exactly what the post carries, as plain lines (English, as Discord receives it).
 * Mirrors feedback_payload in bugreport.rs: the content line, the text, then the fields.
 */
function fbPreview(draft, info) {
  const version = info?.version || "";
  const lang = info?.language || "en";
  const text = String(draft?.text ?? "").trim();
  const contact = String(draft?.contact ?? "").trim();
  const lines = [`Feedback · Glint ${version}`, ""];
  if (text) lines.push(text, "");
  lines.push(`Stars: ${fbStarsText(draft?.stars)}`);
  if (contact) lines.push(`Contact: ${contact}`);
  lines.push(`Glint: ${version} · language ${lang}`);
  return lines.join("\n");
}

/** The star count after a key on the star row, or null when the key is not for it. */
function fbStarKey(cur, key) {
  const n = fbClampStars(cur);
  switch (key) {
    case "ArrowRight": case "ArrowUp": return Math.min(FB_STARS, n + 1);
    case "ArrowLeft": case "ArrowDown": return Math.max(0, n - 1);
    case "Home": return 1;
    case "End": return FB_STARS;
    case "Delete": case "Backspace": case "0": return 0;
    default: return /^[1-5]$/.test(key) ? +key : null;
  }
}

/** A click on star `n`: that many stars, or none when it was already the count. */
const fbStarClick = (cur, n) => (fbClampStars(cur) === fbClampStars(n) ? 0 : fbClampStars(n));

/** The backend's error, in plain words the user can act on (translated). */
function fbErrText(msg) {
  const m = String(msg ?? "");
  if (m.startsWith("Could not reach Discord")) return t("Could not reach Discord. Check the internet connection and try again.");
  if (/HTTP 429/.test(m)) return t("Discord is busy right now. Try again in a minute.");
  if (m.startsWith("Wait a moment")) return t("Wait a moment before sending more feedback.");
  if (m.startsWith("Give some stars")) return t("Give some stars or write a few words before sending.");
  if (m.startsWith("Pick from one")) return t("Pick from one to five stars.");
  if (m.startsWith("Feedback is not set up")) return t("Feedback is not set up in this copy of Glint.");
  return t(m);
}

// ---- The button.

function fbRender() {
  const slot = document.getElementById("fb-slot");
  if (!slot) return;
  let btn = slot.querySelector(".fb-btn");
  if (!btn) {
    slot.innerHTML = `<button type="button" class="btn bug-btn fb-btn" aria-haspopup="dialog" aria-expanded="false">${FB_ICON}</button>`;
    btn = slot.firstElementChild;
    btn.addEventListener("click", fbOpen);
  }
  const tip = t("Send feedback");
  if (btn.title !== tip) { btn.title = tip; btn.setAttribute("aria-label", tip); }
  btn.setAttribute("aria-expanded", String(!!fbCur));
}

// ---- The dialog.

async function fbOpen() {
  if (fbCur || fbOpening || document.querySelector(".bug-dlg")) return;
  fbOpening = true;
  let info;
  try { info = (await invoke("feedback_info")) || {}; } catch (e) { fail(e); return; } finally { fbOpening = false; }
  if (fbCur || document.querySelector(".bug-dlg")) return;
  const root = document.createElement("div");
  root.className = "bug-dlg fb-dlg";
  root.setAttribute("role", "dialog");
  root.setAttribute("aria-modal", "true");
  root.setAttribute("aria-labelledby", "fb-title");
  root.innerHTML = `<div class="bug-box"><div class="bug-head fb-head">${FB_ICON}<h2 id="fb-title">${esc(t("How do you like Glint?"))}</h2></div><div class="bug-main"></div></div>`;
  const dlg = { root, info, mode: info.configured ? "form" : "off", hover: 0, downOnBack: false };

  dlg.onKey = (e) => {
    if (e.isComposing) return; // a Korean (IME) syllable is still being typed
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); fbClose(); }
    else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); e.stopPropagation(); if (dlg.mode === "form") fbSend(); }
  };
  dlg.onFocus = (e) => { if (fbCur === dlg && !root.contains(e.target)) fbFocusFirst(dlg); };
  window.addEventListener("keydown", dlg.onKey, true);
  document.addEventListener("focusin", dlg.onFocus, true);

  root.addEventListener("keydown", (e) => {
    const star = e.target.closest?.(".fb-star");
    if (star && dlg.mode === "form" && !fbSending && !e.ctrlKey && !e.altKey && !e.metaKey) {
      const next = fbStarKey(fbDraft.stars, e.key);
      if (next !== null) { e.preventDefault(); fbSetStars(dlg, next, true); return; }
      if (e.key === " " || e.key === "Enter") { e.preventDefault(); fbSetStars(dlg, fbStarClick(fbDraft.stars, +star.dataset.n), true); return; }
    }
    if (e.key !== "Tab") return;
    e.preventDefault();
    const list = bugFocusables(root);
    const next = bugCycle(list.indexOf(document.activeElement), list.length, e.shiftKey);
    if (next >= 0) list[next].focus();
  });
  root.addEventListener("mousedown", (e) => { dlg.downOnBack = e.target === root; });
  root.addEventListener("click", (e) => {
    if (e.target === root) {
      if (dlg.downOnBack && !(dlg.mode === "form" && (fbDraft.text.trim() || fbDraft.stars))) fbClose();
      return;
    }
    const star = e.target.closest(".fb-star");
    if (star) { if (dlg.mode === "form" && !fbSending) fbSetStars(dlg, fbStarClick(fbDraft.stars, +star.dataset.n), true); return; }
    const b = e.target.closest("[data-fb]");
    if (!b || b.disabled) return;
    if (b.dataset.fb === "send") fbSend();
    else fbClose();
  });
  // Pointing at a star lights the stars up to it (a preview of the click).
  root.addEventListener("pointerover", (e) => {
    const star = e.target.closest?.(".fb-star");
    const n = star && !fbSending ? +star.dataset.n : 0;
    if (n !== dlg.hover) { dlg.hover = n; fbPaintStars(dlg); }
  });
  root.addEventListener("pointerleave", () => { if (dlg.hover) { dlg.hover = 0; fbPaintStars(dlg); } });
  root.addEventListener("input", (e) => fbTake(dlg, e.target));
  root.addEventListener("change", (e) => fbTake(dlg, e.target));

  // In full screen only the full-screen element is drawn, so the dialog goes inside it (a <video> cannot hold it).
  const fs = document.fullscreenElement;
  (fs && !(fs instanceof HTMLMediaElement) ? fs : document.body).appendChild(root);
  fbCur = dlg;
  fbShow(dlg);
  fbRender();
}

function fbClose() {
  const dlg = fbCur;
  if (!dlg) return;
  fbCur = null;
  window.removeEventListener("keydown", dlg.onKey, true);
  document.removeEventListener("focusin", dlg.onFocus, true);
  dlg.root.remove();
  fbRender();
  const btn = document.querySelector("#fb-slot .fb-btn");
  if (btn && !document.fullscreenElement) btn.focus({ preventScroll: true });
}

function fbFocusFirst(dlg) {
  const r = dlg.root;
  let el;
  if (dlg.mode === "form") el = r.querySelector(".fb-star[tabindex='0']") || r.querySelector(".fb-text");
  else el = r.querySelector('[data-fb="close"]');
  el?.focus({ preventScroll: true });
}

function fbShow(dlg) {
  const r = dlg.root, info = dlg.info;
  const main = r.querySelector(".bug-main");
  if (dlg.mode === "off") {
    main.innerHTML = `<p class="bug-sub">${esc(t("Sending feedback from the app is not set up in this build of Glint."))}</p>
      <div class="bug-row"><button type="button" class="btn primary" data-fb="close">${esc(t("Close"))}</button></div>`;
  } else if (dlg.mode === "done") {
    main.innerHTML = `<div class="bug-done fb-done">${FB_ICON}<p>${esc(t("Thanks for your feedback!"))}</p></div>
      <div class="bug-row"><button type="button" class="btn primary" data-fb="close">${esc(t("Close"))}</button></div>`;
  } else {
    const stars = Array.from({ length: FB_STARS }, (_, i) => `<button type="button" class="fb-star" role="radio" data-n="${i + 1}" aria-label="${esc(t("{n} of 5 stars", { n: i + 1 }))}">${FB_STAR}</button>`).join("");
    main.innerHTML = `<p class="bug-sub">${esc(t("This goes to the Glint developers on Discord. Nothing is sent until you press Send."))}</p>
      <div class="fb-rate"><div class="fb-stars" role="radiogroup" aria-label="${esc(t("Stars"))}">${stars}</div><span class="fb-score" aria-live="polite"></span></div>
      <label class="bug-lab" for="fb-text">${esc(t("What should be better, what do you like?"))}</label>
      <textarea id="fb-text" class="field bug-text fb-text" rows="5" maxlength="${FB_MAX}" spellcheck="true"></textarea>
      <div class="bug-meta"><span class="bug-hint fb-hint"></span><span class="bug-count fb-count"></span></div>
      <label class="bug-lab" for="fb-contact">${esc(t("How can we reach you? (Discord name)"))}</label>
      <input id="fb-contact" class="field bug-contact fb-contact" type="text" maxlength="${FB_CONTACT_MAX}" spellcheck="false" autocomplete="off" placeholder="${esc(t("Optional"))}">
      <details class="bug-sent fb-sent" open><summary>${esc(t("What will be sent"))}</summary>
        <pre class="bug-pre fb-preview" tabindex="0"></pre>
      </details>
      <div class="bug-err" role="alert" hidden></div>
      <div class="bug-row"><button type="button" class="btn" data-fb="cancel">${esc(t("Cancel"))}</button><button type="button" class="btn primary bug-send fb-send" data-fb="send"></button></div>`;
    // Values are set as properties, so nothing typed is ever parsed as markup.
    r.querySelector(".fb-text").value = fbDraft.text;
    r.querySelector(".fb-contact").value = fbDraft.contact;
    dlg.sendKey = "";
    fbSync(dlg);
  }
  fbFocusFirst(dlg);
}

function fbSetStars(dlg, n, focus) {
  fbDraft.stars = fbClampStars(n);
  fbSync(dlg);
  if (focus) {
    // The roving focus follows the choice (the first star when there is none).
    const el = dlg.root.querySelector(`.fb-star[data-n="${Math.max(1, fbDraft.stars)}"]`);
    el?.focus({ preventScroll: true });
  }
}

/** Light the stars: the hovered count while pointing, else the chosen one. */
function fbPaintStars(dlg) {
  const lit = dlg.hover || fbDraft.stars;
  for (const el of dlg.root.querySelectorAll(".fb-star")) {
    const n = +el.dataset.n;
    el.classList.toggle("on", n <= lit);
    el.classList.toggle("hover", !!dlg.hover);
    el.setAttribute("aria-checked", String(n === fbDraft.stars));
    el.tabIndex = n === Math.max(1, fbDraft.stars) ? 0 : -1;
    el.disabled = fbSending;
  }
  const score = dlg.root.querySelector(".fb-score");
  if (score) score.textContent = fbDraft.stars ? `${fbDraft.stars} / ${FB_STARS}` : t("No stars yet");
}

function fbTake(dlg, el) {
  if (dlg.mode !== "form" || !el) return;
  if (el.matches(".fb-text")) fbDraft.text = el.value;
  else if (el.matches(".fb-contact")) fbDraft.contact = el.value;
  else return;
  fbSync(dlg);
}

/** Bring the stars, count, hint, preview and Send button up to date. */
function fbSync(dlg) {
  if (dlg.mode !== "form") return;
  const r = dlg.root, n = fbDraft.text.length, ready = fbReady(fbDraft.stars, fbDraft.text);
  fbPaintStars(dlg);
  const count = r.querySelector(".fb-count");
  count.textContent = `${n} / ${FB_MAX}`;
  count.classList.toggle("bug-near", n > FB_MAX - 200);
  r.querySelector(".fb-hint").textContent = ready ? "" : t("Pick some stars or write a few words");
  r.querySelector(".fb-preview").textContent = fbPreview(fbDraft, dlg.info);
  for (const el of r.querySelectorAll(".fb-text, .fb-contact")) el.readOnly = fbSending;
  const send = r.querySelector(".fb-send");
  send.disabled = fbSending || !ready;
  const key = fbSending ? "sending" : "send";
  if (dlg.sendKey !== key) {
    dlg.sendKey = key;
    send.innerHTML = fbSending ? `<i class="bug-spin" aria-hidden="true"></i>${esc(t("Sending"))}` : esc(t("Send"));
  }
}

function fbShowErr(dlg, msg) {
  const el = dlg.root.querySelector(".bug-err");
  if (!el) return;
  el.textContent = msg;
  el.hidden = !msg;
}

async function fbSend() {
  const dlg = fbCur;
  if (!dlg || dlg.mode !== "form" || fbSending || !fbReady(fbDraft.stars, fbDraft.text)) return;
  fbSending = true;
  fbShowErr(dlg, "");
  fbSync(dlg);
  try {
    await invoke("send_feedback", { stars: fbClampStars(fbDraft.stars), text: fbDraft.text.trim(), contact: fbDraft.contact.trim() });
    fbDraft = { stars: 0, text: "", contact: "" };
    fbSending = false;
    if (fbCur) { fbCur.mode = "done"; fbShow(fbCur); }
    else flash(t("Thanks for your feedback!"));
  } catch (e) {
    fbSending = false;
    const msg = fbErrText(e?.message ?? e);
    if (fbCur && fbCur.mode === "form") {
      fbShowErr(fbCur, msg);
      fbSync(fbCur);
      if (!fbCur.root.contains(document.activeElement)) fbCur.root.querySelector(".fb-send").focus({ preventScroll: true });
    } else flash(msg, true);
  } finally {
    fbSending = false;
    if (fbCur && fbCur.mode === "form") fbSync(fbCur);
  }
}
