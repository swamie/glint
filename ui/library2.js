"use strict";
// 0.1.16, the Library page: the game and folder chip rows fold to one line, and
// a trail beside the title ("All clips › <folder> › <game>") while a filter is on.
// Loaded before app.js; nothing here touches app.js's globals at load time.
//
// A row is a `.chiprow` holding every chip. Chips that may be left out carry
// `data-fit` (in order: fullest first), the toggle chip `data-fit-more`; every
// other child (the "All" chip, "New folder", "Rename", "Delete folder") always
// stays. Folded (no `.open`), CSS clips the row to one line and libFitRow hides
// chips from the end until the rest really is one line, measured in the DOM, so
// it holds at any window width. Measured after each render and on resize only:
// scrolling the grid never changes it.

/** How many chips sit on the first line of `row`, and whether every shown child is on it. */
function libLineInfo(row) {
  const kids = [...row.children].filter((el) => !el.hidden && !el.classList.contains("sp") && el.offsetWidth > 0);
  if (!kids.length) return { first: 0, one: true };
  const top = Math.min(...kids.map((el) => el.offsetTop));
  // Rows of chips of slightly different heights (game chips 30 px, buttons 28 px) are centred: allow a few px.
  const first = kids.filter((el) => el.offsetTop - top < 8).length;
  return { first, one: first === kids.length };
}

/** "{n} more" / "Show fewer" on a row's toggle chip. */
function libMoreText(more, open, n) {
  const span = more.querySelector("span") || more;
  span.textContent = open ? t("Show fewer") : t("{n} more", { n });
}

/**
 * Fit one row. Folded: the fullest chips that fit on one line with the toggle
 * chip and the fixed buttons, the active chip always among them. Unfolded: every
 * chip, and "Show fewer" only when folding would hide something.
 */
function libFitRow(row) {
  const open = row.classList.contains("open");
  const chips = [...row.querySelectorAll("[data-fit]")];
  const more = row.querySelector("[data-fit-more]");
  row.classList.remove("tight");
  for (const c of chips) c.hidden = false;
  if (!more) return;
  more.hidden = true;
  const all = libLineInfo(row).one;
  if (open) {
    libMoreText(more, true, 0);
    more.hidden = all;
    return;
  }
  if (all) return;
  more.hidden = false;
  const active = chips.find((c) => c.classList.contains("active"));
  // First guess from widths, measured once (chips do not wrap inside, so their width does not depend on the line).
  libMoreText(more, false, chips.length);
  const gap = parseFloat(getComputedStyle(row).columnGap) || 6;
  const room = row.clientWidth;
  // The spacer (`.sp`) only takes what is left over, so it is not counted.
  const fixed = [...row.children].filter((el) => !el.hasAttribute("data-fit") && !el.classList.contains("sp") && el.offsetWidth > 0);
  let used = fixed.reduce((s, el) => s + el.offsetWidth + gap, 0);
  if (active) used += active.offsetWidth + gap;
  const keep = new Set(active ? [active] : []);
  for (const c of chips) {
    if (c === active) continue;
    const w = c.offsetWidth + gap;
    if (used + w > room) break; // fullest first: stop at the first that does not fit, so the order stays
    used += w;
    keep.add(c);
  }
  const apply = () => {
    let n = 0;
    for (const c of chips) { c.hidden = !keep.has(c); if (c.hidden) n++; }
    libMoreText(more, false, n);
    return n;
  };
  apply();
  // Then the real layout decides: drop the last shown chip (never the active one) until it is one line.
  for (let guard = chips.length; guard > 0 && !libLineInfo(row).one; guard--) {
    const last = [...keep].filter((c) => c !== active).pop();
    if (!last) break;
    keep.delete(last);
    apply();
  }
  // Too narrow even for the buttons alone: let them wrap rather than cut them off.
  if (!libLineInfo(row).one) row.classList.add("tight");
}

/** Fit every chip row on the page (app.js calls this after drawing the Library). */
function libFitRows() {
  for (const row of document.querySelectorAll("#view .chiprow")) libFitRow(row);
}

// Re-measure when the window (or the sidebar, which changes the room) changes size; once per frame.
(() => {
  let queued = false;
  const again = () => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => { queued = false; libFitRows(); });
  };
  window.addEventListener("resize", again);
  if (typeof ResizeObserver === "function") {
    let last = 0;
    const ro = new ResizeObserver((entries) => {
      const w = Math.round(entries[0]?.contentRect.width || 0);
      if (w !== last) { last = w; again(); }
    });
    const watch = () => { const v = document.getElementById("view"); if (v) ro.observe(v); else setTimeout(watch, 200); };
    watch();
  }
})();

/**
 * The trail beside the Library's title while a folder or game filter is on:
 * All clips › <folder> › <game>. Each step goes back to that level; the last one is
 * where you are. `folder`: null = all, "" = the main folder. `game`: "all" = none.
 */
function libTrailHtml(folder, game, mainLabel) {
  if (folder === null && (game === "all" || !game)) return "";
  const steps = [{ step: "all", label: t("All clips") }];
  if (folder !== null) steps.push({ step: "folder", label: folder === "" ? mainLabel : folder });
  if (game && game !== "all") steps.push({ step: "game", label: game });
  const sep = `<span class="lib-trail-sep" aria-hidden="true">›</span>`;
  return `<nav class="lib-trail" aria-label="${esc(t("Where you are"))}">${steps.map((s, i) => {
    const here = i === steps.length - 1;
    return `<button class="lib-trail-step${here ? " here" : ""}" data-act="trail" data-step="${s.step}"${here ? ' aria-current="location"' : ""} title="${esc(s.label)}">${esc(s.label)}</button>`;
  }).join(sep)}</nav>`;
}
