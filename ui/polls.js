"use strict";
// Polls from the developer (backend: polls.rs; server: server/polls).
// - A small card at the bottom right when a poll is open, this install has not voted and
//   has not pressed "Not now" for it. It is not a dialog: it never takes the focus and
//   stays clear of the top bar's clip buttons.
// - A button in the top bar (after the heart) opens the list of open and recently closed
//   polls any time; a dot on it while one is unanswered.
// - The setting "Polls from the developer" (Settings → Updates) turns all of it off; then the
//   backend asks nothing and this file shows nothing.
// Poll texts are the developer's own words: always put on the page with textContent and
// never translated. Every other text goes through t().
// Loaded after app.js: its globals (S, $, invoke, row, toggle, goPage, flash) exist.
// All names here start with `polls` / `POLLS_`; markup uses `pl-` classes.

const POLLS_ICON = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 20V11M12 20V5M19 20v-6"/><path d="M3 20h18"/></svg>';
const POLLS_COMMENT_MAX = 300;
const POLLS_HIDDEN_KEY = "glint.polls.hidden";     // ids of polls the user said "Not now" to
const POLLS_INTRO_KEY = "glint.polls.introSeen";   // the card's one-time line about the setting

let pollsAll = [];               // what polls_list / polls-changed gave
let pollsCardId = null;          // the poll the card shows (it stays after a vote, with the results)
let pollsListEl = null;          // the open list from the top bar button, or null
let pollsBusy = new Set();       // polls whose vote is being sent
const pollsPick = {};            // poll id -> ids ticked (several answers)
const pollsComment = {};         // poll id -> comment typed so far
const pollsErr = {};             // poll id -> error shown under it
let pollsIntroNow = false;       // the card that shows the intro line is up

// ---- Pure logic (no DOM; tested with node).

/** Still open at `nowMs`? */
const pollsIsOpen = (p, nowMs = Date.now()) => !!p && !p.closed && !(p.closes && p.closes * 1000 <= nowMs);
/** Open and not voted on by this install. */
const pollsUnanswered = (p, nowMs = Date.now()) => pollsIsOpen(p, nowMs) && !p.voted;

/** "1 day 4 hours", "3 hours", "12 minutes" for a span of `secs` seconds. */
function pollsSpan(secs) {
  const s = Math.max(0, Math.floor(secs));
  const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.max(1, Math.ceil((s % 3600) / 60));
  const parts = [];
  if (d) parts.push(d === 1 ? t("{n} day", { n: d }) : t("{n} days", { n: d }));
  if (h) parts.push(h === 1 ? t("{n} hour", { n: h }) : t("{n} hours", { n: h }));
  if (!d && !h) parts.push(m === 1 ? t("{n} minute", { n: m }) : t("{n} minutes", { n: m }));
  return parts.join(" ");
}

/** When the poll ends, in words: "ends in 1 day 4 hours", "Ended", or "No end date". */
function pollsWhen(p, nowMs = Date.now()) {
  if (!pollsIsOpen(p, nowMs)) return t("Ended");
  if (!p.closes) return t("No end date");
  return t("ends in {time}", { time: pollsSpan(p.closes - nowMs / 1000) });
}

/** Per option: { id, text, n, pct, mine }, the percents of all voters (several answers may add up past 100). */
function pollsBars(p) {
  const total = p?.results?.total || 0;
  const mine = new Set(p?.voted || []);
  return (p?.options || []).map((o) => {
    const n = p?.results?.counts?.[String(o.id)] || 0;
    return { id: o.id, text: o.text, n, pct: total ? Math.round((n / total) * 100) : 0, mine: mine.has(o.id) };
  });
}

/** The backend's error in the user's language. */
function pollsErrText(msg) {
  const m = String(msg?.message ?? msg ?? "");
  const known = {
    "Could not reach the polls server. Try again later.": t("Could not reach the polls server. Try again later."),
    "This poll has ended.": t("This poll has ended."),
    "Too many votes from this network today. Try again later.": t("Too many votes from this network today. Try again later."),
    "Polls are off in Settings.": t("Polls are off in Settings."),
    "Pick an answer first.": t("Pick an answer first."),
    "A comment can be at most 300 characters.": t("A comment can be at most 300 characters."),
  };
  return known[m] || t("The vote was not accepted.");
}

// ---- Remembered in this window's storage (a convenience only: losing it shows a card again).

function pollsLoadHidden() {
  try { return new Set(JSON.parse(localStorage.getItem(POLLS_HIDDEN_KEY) || "[]")); } catch { return new Set(); }
}
function pollsHide(id) {
  const ids = pollsLoadHidden();
  ids.add(id);
  try { localStorage.setItem(POLLS_HIDDEN_KEY, JSON.stringify([...ids].slice(-200))); } catch {}
}
function pollsIntroSeen() {
  try { return localStorage.getItem(POLLS_INTRO_KEY) === "1"; } catch { return true; }
}
function pollsSetIntroSeen() {
  try { localStorage.setItem(POLLS_INTRO_KEY, "1"); } catch {}
}

const pollsOn = () => S.settings?.polls !== false;

// ---- The setting (app.js puts this row under Settings → Updates).

function pollsSettingRow() {
  return row(t("Polls from the developer"), t("Glint asks the developer's server for open polls. A vote sends your answer and a random number made for this install, nothing else."), toggle("polls"));
}

// ---- Building a poll's body (shared by the card and the list).

function pollsEl(tag, cls, text) {
  const el = document.createElement(tag);
  if (cls) el.className = cls;
  if (text !== undefined) el.textContent = text;
  return el;
}

/** The answers to pick from, or the results once voted (or ended). */
function pollsBody(p) {
  const box = pollsEl("div", "pl-body");
  box.dataset.poll = p.id;
  box.append(pollsEl("div", "pl-q", p.question));
  const busy = pollsBusy.has(p.id);
  if (p.results || !pollsIsOpen(p)) {
    const list = pollsEl("div", "pl-bars");
    for (const b of pollsBars(p)) {
      const bar = pollsEl("div", "pl-bar" + (b.mine ? " mine" : ""));
      const fill = pollsEl("i", "pl-fill");
      fill.style.width = `${b.pct}%`;
      const name = pollsEl("span", "pl-name", b.text);
      if (b.mine) name.title = t("Your answer");
      bar.append(fill, name, pollsEl("span", "pl-pct", `${b.pct} %`));
      list.append(bar);
    }
    box.append(list);
    const total = p.results?.total || 0;
    box.append(pollsEl("div", "pl-foot", [total === 1 ? t("1 vote") : t("{n} votes", { n: total }), pollsWhen(p)].join(" · ")));
    return box;
  }
  const opts = pollsEl("div", "pl-opts" + (p.multiple ? " multi" : ""));
  if (p.multiple) {
    const picked = pollsPick[p.id] || (pollsPick[p.id] = new Set());
    for (const o of p.options) {
      const lab = pollsEl("label", "pl-tick");
      const box2 = pollsEl("input");
      box2.type = "checkbox";
      box2.dataset.plTick = o.id;
      box2.checked = picked.has(o.id);
      box2.disabled = busy;
      lab.append(box2, pollsEl("span", "", o.text));
      opts.append(lab);
    }
  } else {
    for (const o of p.options) {
      const btn = pollsEl("button", "btn pl-opt", o.text);
      btn.type = "button";
      btn.dataset.plVote = o.id;
      btn.disabled = busy;
      opts.append(btn);
    }
  }
  box.append(opts);
  if (p.allow_comment) {
    const c = pollsEl("input", "field pl-comment");
    c.type = "text";
    c.maxLength = POLLS_COMMENT_MAX;
    c.placeholder = t("Add a comment (optional)");
    c.setAttribute("aria-label", t("Add a comment (optional)"));
    c.value = pollsComment[p.id] || "";
    c.readOnly = busy;
    c.spellcheck = true;
    box.append(c);
  }
  if (p.multiple) {
    const go = pollsEl("button", "btn primary sm pl-send", busy ? t("Sending") : t("Vote"));
    go.type = "button";
    go.dataset.plSend = "1";
    go.disabled = busy || !(pollsPick[p.id]?.size);
    box.append(go);
  }
  const err = pollsEl("div", "pl-err", pollsErr[p.id] || "");
  err.setAttribute("role", "alert");
  err.hidden = !pollsErr[p.id];
  box.append(err);
  box.append(pollsEl("div", "pl-foot", pollsWhen(p)));
  return box;
}

/** Clicks, ticks and typing inside a poll body (the card and the list share these). */
function pollsWire(root) {
  root.addEventListener("click", (e) => {
    const body = e.target.closest?.(".pl-body");
    const p = body && pollsAll.find((x) => String(x.id) === body.dataset.poll);
    if (!p) return;
    const opt = e.target.closest("[data-pl-vote]");
    if (opt && !opt.disabled) return pollsVote(p, [Number(opt.dataset.plVote)]);
    const send = e.target.closest("[data-pl-send]");
    if (send && !send.disabled) return pollsVote(p, [...(pollsPick[p.id] || [])]);
  });
  root.addEventListener("change", (e) => {
    const tick = e.target.closest?.("[data-pl-tick]");
    if (!tick) return;
    const id = e.target.closest(".pl-body")?.dataset.poll;
    const set = pollsPick[id] || (pollsPick[id] = new Set());
    const opt = Number(tick.dataset.plTick);
    if (tick.checked) set.add(opt); else set.delete(opt);
    const send = e.target.closest(".pl-body").querySelector("[data-pl-send]");
    if (send) send.disabled = !set.size;
  });
  root.addEventListener("input", (e) => {
    if (!e.target.matches?.(".pl-comment")) return;
    pollsComment[e.target.closest(".pl-body").dataset.poll] = e.target.value;
  });
  // Typing in the comment box must not reach the app's shortcuts (Space, Delete, letters).
  root.addEventListener("keydown", (e) => { if (e.target.matches?.(".pl-comment")) e.stopPropagation(); });
}

async function pollsVote(p, options) {
  if (pollsBusy.has(p.id) || !options.length) return;
  pollsBusy.add(p.id);
  delete pollsErr[p.id];
  pollsRender();
  try {
    const comment = String(pollsComment[p.id] || "").trim().slice(0, POLLS_COMMENT_MAX);
    const fresh = await invoke("polls_vote", { poll: p.id, options, comment });
    if (fresh) pollsTake(pollsAll.map((x) => (x.id === fresh.id ? fresh : x)));
    delete pollsComment[p.id];
    delete pollsPick[p.id];
  } catch (e) {
    pollsErr[p.id] = pollsErrText(e);
  } finally {
    pollsBusy.delete(p.id);
    pollsRender();
  }
}

// ---- The top bar button and its list.

function pollsRenderButton() {
  const slot = document.getElementById("polls-slot");
  if (!slot) return;
  let btn = slot.querySelector(".pl-btn");
  if (!btn) {
    slot.innerHTML = `<button type="button" class="btn bug-btn pl-btn" aria-haspopup="true" aria-expanded="false">${POLLS_ICON}<i class="pl-dot" aria-hidden="true"></i></button>`;
    btn = slot.firstElementChild;
    btn.addEventListener("click", () => (pollsListEl ? pollsCloseList() : pollsOpenList()));
  }
  slot.hidden = !pollsOn();
  const waiting = pollsAll.some((p) => pollsUnanswered(p));
  const tip = waiting ? t("Polls from the developer: one is waiting for your answer") : t("Polls from the developer");
  if (btn.title !== tip) { btn.title = tip; btn.setAttribute("aria-label", tip); }
  btn.classList.toggle("pl-waiting", waiting);
  btn.setAttribute("aria-expanded", String(!!pollsListEl));
}

async function pollsOpenList() {
  if (pollsListEl || !pollsOn()) return;
  const el = pollsEl("div", "pl-list");
  el.setAttribute("role", "region");
  el.setAttribute("aria-label", t("Polls from the developer"));
  pollsWire(el);
  document.body.append(el);
  pollsListEl = el;
  pollsRender();
  // A click here asks the server again (at most once a minute; the backend decides).
  try { pollsTake(await invoke("polls_list", { refresh: true })); } catch (e) { console.error("Glint: polls_list failed", e); }
}

function pollsCloseList() {
  if (!pollsListEl) return;
  pollsListEl.remove();
  pollsListEl = null;
  pollsRenderButton();
}

function pollsRenderList() {
  const el = pollsListEl;
  if (!el) return;
  const btn = document.querySelector("#polls-slot .pl-btn");
  const r = btn?.getBoundingClientRect();
  if (r) {
    el.style.top = `${Math.round(r.bottom + 6)}px`;
    el.style.left = `${Math.round(Math.max(8, Math.min(r.left - 8, window.innerWidth - el.offsetWidth - 8)))}px`;
  }
  el.textContent = "";
  const head = pollsEl("div", "pl-head");
  head.append(pollsEl("span", "pl-title", t("Polls from the developer")));
  const x = pollsEl("button", "pl-x");
  x.type = "button";
  x.title = t("Close");
  x.setAttribute("aria-label", t("Close"));
  x.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18"/></svg>';
  x.addEventListener("click", pollsCloseList);
  head.append(x);
  el.append(head);
  const scroll = pollsEl("div", "pl-scroll");
  const open = pollsAll.filter((p) => pollsIsOpen(p));
  const ended = pollsAll.filter((p) => !pollsIsOpen(p));
  if (!pollsAll.length) scroll.append(pollsEl("p", "pl-empty", t("No polls right now.")));
  for (const p of [...open, ...ended]) scroll.append(pollsBody(p));
  el.append(scroll);
}

// ---- The card.

/** The poll the card should show: the one it already shows, else the newest unanswered one not put off. */
/** The clip viewer or the editor workspace is open (both live in #modal): the card would cover their buttons. */
function pollsCovered() {
  const m = document.getElementById("modal");
  return !!(m && !m.hidden && m.querySelector(":scope > .viewer"));
}

function pollsCardPick() {
  // Not over the editor's timeline or an open clip, and not over Settings (where its own switch is).
  if (!pollsOn() || S.page === "editor" || S.page === "settings" || pollsCovered()) return null;
  const cur = pollsAll.find((p) => p.id === pollsCardId);
  if (cur) return cur;
  const hidden = pollsLoadHidden();
  return pollsAll.find((p) => pollsUnanswered(p) && !hidden.has(p.id)) || null;
}

function pollsRenderCard() {
  const p = pollsCardPick();
  let card = document.getElementById("polls-card");
  if (!p || pollsListEl) {
    if (card) card.remove();
    return;
  }
  if (pollsCardId !== p.id) {
    pollsCardId = p.id;
    pollsIntroNow = !pollsIntroSeen();
    pollsSetIntroSeen();
  }
  if (!card) {
    card = pollsEl("div", "pl-card");
    card.id = "polls-card";
    card.setAttribute("role", "region");
    pollsWire(card);
    card.addEventListener("click", (e) => {
      const act = e.target.closest?.("[data-pl-card]")?.dataset.plCard;
      if (act === "later") { pollsHide(pollsCardId); pollsCardId = -1; pollsRender(); }
      else if (act === "close") { pollsHide(pollsCardId); pollsCardId = -1; pollsRender(); }
      else if (act === "setting") { goPage("settings", "program"); pollsShowSetting(); }
    });
    document.body.append(card);
  }
  card.setAttribute("aria-label", t("Poll from the developer"));
  // Keep what has the focus (a comment being typed) if the card is drawn again.
  const typing = card.contains(document.activeElement) && document.activeElement.matches(".pl-comment");
  const caret = typing ? document.activeElement.selectionStart : 0;
  card.textContent = "";
  const head = pollsEl("div", "pl-head");
  head.insertAdjacentHTML("beforeend", POLLS_ICON);
  head.append(pollsEl("span", "pl-title", t("Poll from the developer")));
  card.append(head);
  if (pollsIntroNow) {
    const intro = pollsEl("p", "pl-intro", t("Glint asks the developer's server for open polls. A vote sends your answer and a random number made for this install, nothing else."));
    const link = pollsEl("button", "pl-link", t("Change this in Settings"));
    link.type = "button";
    link.dataset.plCard = "setting";
    intro.append(" ", link);
    card.append(intro);
  }
  card.append(pollsBody(p));
  const row2 = pollsEl("div", "pl-actions");
  const voted = !!p.voted || !pollsIsOpen(p);
  const btn = pollsEl("button", "btn sm", voted ? t("Close") : t("Not now"));
  btn.type = "button";
  btn.dataset.plCard = voted ? "close" : "later";
  row2.append(btn);
  card.append(row2);
  if (typing) {
    const c = card.querySelector(".pl-comment");
    if (c) { c.focus({ preventScroll: true }); c.setSelectionRange(caret, caret); }
  }
}

/** After the intro's link: bring the setting into view and mark it for a moment. */
function pollsShowSetting() {
  requestAnimationFrame(() => {
    const rowEl = document.querySelector('#view [data-set="polls"]')?.closest(".row");
    if (!rowEl) return;
    rowEl.scrollIntoView({ block: "center" });
    rowEl.classList.add("pl-spot");
    setTimeout(() => rowEl.classList.remove("pl-spot"), 1800);
  });
}

// ---- State in, everything out.

function pollsTake(list) {
  pollsAll = Array.isArray(list) ? list : [];
  pollsRender();
}

function pollsRender() {
  if (!pollsOn()) {
    pollsAll = [];
    pollsCloseList();
  }
  pollsRenderButton();
  pollsRenderList();
  pollsRenderCard();
}

(function pollsInit() {
  const ev = window.__TAURI__?.event;
  if (ev) ev.listen("polls-changed", ({ payload }) => pollsTake(payload));
  // Wait for the settings (app.js loads them), then ask what the backend already knows (no network).
  let page = null, lang = null, on = null, covered = null;
  const tick = async () => {
    if (!S.settings) return;
    const nowOn = pollsOn();
    if (on !== nowOn) {
      const first = on === null;
      on = nowOn;
      if (nowOn) {
        try { pollsTake(await invoke("polls_list", first ? {} : { refresh: true })); } catch (e) { console.error("Glint: polls_list failed", e); }
      } else pollsRender();
    }
    // The card hides in the editor, and every text follows a language change.
    const cov = pollsCovered();
    if (page !== S.page || lang !== document.documentElement.lang || covered !== cov) {
      covered = cov;
      page = S.page;
      lang = document.documentElement.lang;
      pollsRender();
    }
  };
  setInterval(tick, 700);
  tick();
  // Outside the list, or Escape: close it (nothing typed there is lost: comments are kept).
  document.addEventListener("pointerdown", (e) => {
    if (pollsListEl && !pollsListEl.contains(e.target) && !e.target.closest?.("#polls-slot")) pollsCloseList();
  }, true);
  window.addEventListener("keydown", (e) => { if (e.key === "Escape" && pollsListEl && !e.isComposing) { e.stopPropagation(); pollsCloseList(); } }, true);
  window.addEventListener("resize", () => { if (pollsListEl) pollsRenderList(); });
})();
