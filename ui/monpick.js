"use strict";
// The monitor picker (0.1.41): wherever Glint asks for a monitor (Settings → Capture, a clip keybind's monitor,
// the keybind that switches the capture monitor), the monitors are drawn to scale in their real arrangement,
// each with its number, resolution and refresh rate, the main one marked, the chosen one highlighted, and
// inside each a small live picture of what is on it (monprev.rs `monitor_preview`, every few seconds while
// a picker is on screen and the window is visible; never otherwise). Click or arrow keys + Enter pick.
//
// monPick(opts) returns the markup. It carries a hidden native <select data-native> with the same data
// attributes the page used before (data-set="monitor" data-num, or data-kb / data-kbf="monitor"), so app.js's
// change handler stores the same value with the same meaning; picking sets the select and sends "change".
//   opts: { value, set?: "monitor", kb?: index, special?: { value: -1, label }, inline?: bool, title?, cls? }
//   inline: the map itself on the page (Settings → Capture); otherwise a compact button that opens the map
//   in a small pop-over (the keybind rows).
// Pure helpers (node-testable): mpLayout, mpLabel. Names here start with `mp` / `MP`; markup uses `mp-` classes.

const MP = { pics: {}, fails: {}, timer: 0, busy: false, ticks: 0, pop: null, popFor: null };
// Light by design: a new picture every few seconds, only while Glint is the window in front, at about
// the size it is shown. Out of focus the picker keeps its last pictures and takes none.
const MP_REFRESH_MS = 3000;
const MP_PIC_WIDTH = 240;

const mpEsc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const mpMons = () => (typeof S !== "undefined" && Array.isArray(S.monitors) ? S.monitors : []);

/**
 * The monitors' rectangles inside a box of boxW x boxH (CSS pixels) with `pad` around, to scale and in their
 * real arrangement (desktop coordinates, negative ones too), centred. Each rect: { index, x, y, w, h } with a
 * `gap` px gutter taken off every side so neighbours never touch.
 */
function mpLayout(mons, boxW, boxH, pad = 6, gap = 2) {
  const list = (mons || []).filter((m) => m && m.width > 0 && m.height > 0);
  if (!list.length) return { scale: 0, rects: [] };
  const x0 = Math.min(...list.map((m) => m.x || 0)), y0 = Math.min(...list.map((m) => m.y || 0));
  const x1 = Math.max(...list.map((m) => (m.x || 0) + m.width)), y1 = Math.max(...list.map((m) => (m.y || 0) + m.height));
  const bw = x1 - x0, bh = y1 - y0;
  const scale = Math.max(0, Math.min((boxW - 2 * pad) / bw, (boxH - 2 * pad) / bh));
  const ox = (boxW - bw * scale) / 2, oy = (boxH - bh * scale) / 2;
  const rects = list.map((m) => {
    const x = ox + ((m.x || 0) - x0) * scale, y = oy + ((m.y || 0) - y0) * scale;
    const w = m.width * scale, h = m.height * scale;
    const g = Math.min(gap, w / 4, h / 4);
    return { index: m.index, x: Math.round((x + g) * 10) / 10, y: Math.round((y + g) * 10) / 10, w: Math.round((w - 2 * g) * 10) / 10, h: Math.round((h - 2 * g) * 10) / 10 };
  });
  return { scale, rects };
}

/** One monitor in words: "Monitor 2 · 1680×1050 · 60 Hz" (+ " · main"). */
function mpLabel(m) {
  const base = t("Monitor {n}", { n: m.index + 1 }) + ` · ${m.width}×${m.height} · ${m.hz} Hz`;
  return m.primary ? `${base} · ${t("main")}` : base;
}

/** The native select the page's change handler reads (hidden; the picker drives it). */
function mpSelect(opts, mons) {
  const attrs = opts.set ? `data-set="${mpEsc(opts.set)}" data-num` : `data-kb="${mpEsc(opts.kb)}" data-kbf="monitor"`;
  const sp = opts.special ? `<option value="${mpEsc(opts.special.value)}"${opts.value === opts.special.value ? " selected" : ""}>${mpEsc(opts.special.label)}</option>` : "";
  return `<select class="mp-src" hidden data-native tabindex="-1" aria-hidden="true" ${attrs}>${sp}${mons.map((m) => `<option value="${m.index}"${opts.value === m.index ? " selected" : ""}>${mpEsc(mpLabel(m))}</option>`).join("")}</select>`;
}

/** The map: every monitor to scale with its picture, number, size and rate. */
function mpMap(mons, value, boxW, boxH, live) {
  const { rects } = mpLayout(mons, boxW, boxH);
  const tip = t("A live picture of each monitor. A game in exclusive fullscreen can look black here.");
  const cells = rects.map((r) => {
    const m = mons.find((x) => x.index === r.index);
    const on = m.index === value;
    const pic = MP.pics[m.index];
    const small = r.w < 92 || r.h < 52;
    return `<button type="button" class="mp-mon${on ? " on" : ""}${m.primary ? " main" : ""}${small ? " small" : ""}" role="radio" aria-checked="${on}" tabindex="${on ? 0 : -1}"
        data-mp-pick="${m.index}" title="${mpEsc(mpLabel(m) + " · " + m.name)}" aria-label="${mpEsc(mpLabel(m))}"
        style="left:${r.x}px;top:${r.y}px;width:${r.w}px;height:${r.h}px">
        ${live ? `<img class="mp-pic" data-mon="${m.index}" alt=""${pic && !MP.fails[m.index] ? ` src="${mpEsc(pic)}"` : " hidden"}>` : ""}
        <span class="mp-num">${m.index + 1}</span>
        ${m.primary ? `<span class="mp-main" title="${mpEsc(t("Main monitor"))}">★</span>` : ""}
        ${small ? "" : `<span class="mp-info">${m.width}×${m.height} · ${m.hz} Hz</span>`}
      </button>`;
  }).join("");
  return `<div class="mp-map"${live ? " data-live" : ""} role="radiogroup" aria-label="${mpEsc(t("Monitor"))}" title="${mpEsc(tip)}" style="width:${boxW}px;height:${boxH}px">${cells}</div>`;
}

/** The picker's markup (see the top of this file). */
function monPick(opts) {
  const mons = mpMons();
  const value = Number.isInteger(opts.value) ? opts.value : -1;
  const sel = mpSelect({ ...opts, value }, mons);
  const special = opts.special ? `<button type="button" class="mp-special${value === opts.special.value ? " on" : ""}" role="radio" aria-checked="${value === opts.special.value}" data-mp-pick="${mpEsc(opts.special.value)}">${mpEsc(opts.special.label)}</button>` : "";
  setTimeout(() => { mpRefocus(); mpWake(); }, 0); // after the page has put the markup in
  if (opts.inline) {
    const cur = mons.find((m) => m.index === value);
    return `<div class="mp mp-inline ${mpEsc(opts.cls || "")}" title="${mpEsc(opts.title || "")}">${sel}${mpMap(mons, value, 300, 150, true)}${special}
      <div class="mp-cap">${mpEsc(cur ? mpLabel(cur) : opts.special && value === opts.special.value ? opts.special.label : "")}</div></div>`;
  }
  // Compact: a button with a tiny map (no pictures) and the choice in words; the full map opens over it.
  const cur = mons.find((m) => m.index === value);
  const label = cur ? t("Monitor {n}", { n: cur.index + 1 }) + ` · ${cur.width}×${cur.height}` : opts.special ? opts.special.label : "";
  const mini = mpLayout(mons, 34, 20, 1, 1).rects.map((r) => `<i class="${r.index === value ? "on" : ""}" style="left:${r.x}px;top:${r.y}px;width:${r.w}px;height:${r.h}px"></i>`).join("");
  const data = mpEsc(JSON.stringify({ value, special: opts.special || null }));
  return `<div class="mp mp-compact ${mpEsc(opts.cls || "")}">${sel}<button type="button" class="field mp-btn" data-mp-open="${data}" aria-haspopup="dialog" title="${mpEsc(opts.title || "")}">
      <span class="mp-mini" aria-hidden="true">${mini}</span><span class="mp-btn-text">${mpEsc(label)}</span>
      <svg class="mp-chev" viewBox="0 0 24 24" aria-hidden="true"><path d="m7 10 5 5 5-5"/></svg></button></div>`;
}

/** Choose `value` in the picker around `el`: the select changes and tells the page (which saves and redraws). */
function mpChoose(root, value) {
  const sel = root && root.querySelector("select.mp-src");
  if (!sel) return;
  if (String(sel.value) !== String(value)) {
    // The page redraws on the change: the keyboard focus comes back to the chosen monitor (`mpRefocus`).
    MP.refocus = { key: sel.dataset.set ? "set:" + sel.dataset.set : "kb:" + sel.dataset.kb, value: String(value), inPop: !!MP.popFor };
    sel.value = String(value);
    sel.dispatchEvent(new Event("input", { bubbles: true }));
    sel.dispatchEvent(new Event("change", { bubbles: true }));
  }
}

/** After a pick redrew the page: focus the chosen monitor again (or the compact button it came from). */
function mpRefocus() {
  const r = MP.refocus;
  if (!r) return;
  MP.refocus = null;
  const sel = [...document.querySelectorAll("select.mp-src")].find((s) => (s.dataset.set ? "set:" + s.dataset.set : "kb:" + s.dataset.kb) === r.key);
  const root = sel && sel.closest(".mp");
  if (!root) return;
  const target = r.inPop ? root.querySelector(".mp-btn") : root.querySelector(`[data-mp-pick="${CSS.escape(r.value)}"]`);
  target?.focus({ preventScroll: true });
}

/* ---------- the pop-over of a compact picker ---------- */
function mpClosePop() {
  if (MP.pop) { MP.pop.remove(); MP.pop = null; }
  if (MP.popFor) { MP.popFor.setAttribute("aria-expanded", "false"); MP.popFor.focus?.(); MP.popFor = null; }
}

function mpOpenPop(btn) {
  mpClosePop();
  let o = {};
  try { o = JSON.parse(btn.dataset.mpOpen || "{}"); } catch { o = {}; }
  const mons = mpMons();
  const pop = document.createElement("div");
  pop.className = "mp-pop";
  pop.setAttribute("role", "dialog");
  pop.setAttribute("aria-label", t("Choose a monitor"));
  const special = o.special ? `<button type="button" class="mp-special${o.value === o.special.value ? " on" : ""}" data-mp-pick="${mpEsc(o.special.value)}">${mpEsc(o.special.label)}</button>` : "";
  pop.innerHTML = `<div class="mp-pop-head">${mpEsc(t("Choose a monitor"))}</div>${mpMap(mons, o.value, 340, 170, true)}${special}`;
  document.body.appendChild(pop);
  pop.__mpRoot = btn.closest(".mp");
  MP.pop = pop;
  MP.popFor = btn;
  btn.setAttribute("aria-expanded", "true");
  const r = btn.getBoundingClientRect(), pw = pop.offsetWidth, ph = pop.offsetHeight;
  const left = Math.max(8, Math.min(window.innerWidth - pw - 8, r.left + r.width / 2 - pw / 2));
  const below = r.bottom + 6 + ph < window.innerHeight - 8;
  pop.style.left = `${left}px`;
  pop.style.top = `${below ? r.bottom + 6 : Math.max(8, r.top - ph - 6)}px`;
  (pop.querySelector(".mp-mon.on, .mp-special.on") || pop.querySelector("[data-mp-pick]"))?.focus();
  mpWake();
}

/* ---------- live pictures ---------- */
/** The maps that are on screen now (connected, laid out, the window visible). */
function mpVisibleMaps() {
  if (typeof document === "undefined" || document.visibilityState !== "visible") return [];
  return [...document.querySelectorAll(".mp-map[data-live]")].filter((m) => m.isConnected && m.offsetParent !== null);
}

function mpWake() {
  if (MP.timer || !mpVisibleMaps().length) return;
  MP.timer = setTimeout(mpTick, 0);
}

async function mpTick() {
  MP.timer = 0;
  const maps = mpVisibleMaps();
  if (!maps.length || typeof invoke !== "function") return; // nothing on screen: no picture is taken, the loop stops
  if (!MP.busy && (MP.ticks === 0 || document.hasFocus())) {
    MP.busy = true;
    MP.ticks++;
    const wanted = [...new Set(maps.flatMap((m) => [...m.querySelectorAll(".mp-pic")].map((i) => +i.dataset.mon)))];
    for (const i of wanted) {
      // A monitor that could not be grabbed shows its number and size only; it is tried again now and then.
      if (MP.fails[i] && MP.ticks % 5) continue;
      try {
        MP.pics[i] = await invoke("monitor_preview", { index: i, width: MP_PIC_WIDTH });
        MP.fails[i] = false;
      } catch {
        MP.fails[i] = true;
      }
      for (const img of document.querySelectorAll(`.mp-pic[data-mon="${i}"]`)) {
        if (MP.fails[i] || !MP.pics[i]) { img.hidden = true; continue; }
        if (img.getAttribute("src") !== MP.pics[i]) img.setAttribute("src", MP.pics[i]);
        img.hidden = false;
      }
    }
    MP.busy = false;
  }
  if (mpVisibleMaps().length) MP.timer = setTimeout(mpTick, MP_REFRESH_MS);
}

/* ---------- wiring ---------- */
if (typeof window !== "undefined") window.addEventListener("focus", () => { if (MP.timer) { clearTimeout(MP.timer); MP.timer = 0; } mpWake(); });
if (typeof document !== "undefined") {
  document.addEventListener("click", (e) => {
    const open = e.target.closest?.("[data-mp-open]");
    if (open) { e.preventDefault(); return MP.popFor === open ? mpClosePop() : mpOpenPop(open); }
    const pick = e.target.closest?.("[data-mp-pick]");
    if (pick) {
      const inPop = pick.closest(".mp-pop");
      const root = inPop ? inPop.__mpRoot : pick.closest(".mp");
      const v = Number(pick.dataset.mpPick);
      mpChoose(root, v);
      if (inPop) mpClosePop();
      return;
    }
    if (MP.pop && !e.target.closest?.(".mp-pop")) mpClosePop();
  });
  document.addEventListener("keydown", (e) => {
    if (MP.pop && e.key === "Escape") { e.preventDefault(); e.stopPropagation(); return mpClosePop(); }
    const mon = e.target.closest?.(".mp-mon, .mp-special");
    if (!mon) return;
    const scope = mon.closest(".mp-pop") || mon.closest(".mp");
    const items = [...scope.querySelectorAll(".mp-mon, .mp-special")];
    const at = items.indexOf(mon);
    const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
    if (step) {
      e.preventDefault();
      const next = items[(at + step + items.length) % items.length];
      for (const it of items) it.tabIndex = it === next ? 0 : -1;
      next.focus();
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      (e.key === "Home" ? items[0] : items[items.length - 1]).focus();
    }
  }, true);
  document.addEventListener("visibilitychange", () => { if (document.visibilityState === "visible") mpWake(); });
  window.addEventListener("resize", () => { if (MP.pop) mpClosePop(); });
}

if (typeof module !== "undefined") module.exports = { mpLayout };
