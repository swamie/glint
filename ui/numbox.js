"use strict";
/*
 * Number boxes (0.1.27). Every <input type="number"> has the app's own look (style.css): the
 * browser's step arrows are hidden and two small arrows are drawn at the right edge. This file
 * makes those arrows work: a press on the upper half steps up, on the lower half down, and
 * holding repeats. The box tells its listeners with the usual "input" and "change" events.
 * `data-native` on an input (or the color picker's small fields, which have no arrows) opts out.
 */

const NB_ZONE = 20; // px at the right edge that are the arrows
const nbHasArrows = (el) => el instanceof HTMLInputElement && el.type === "number" && !el.disabled && !el.readOnly && !el.hasAttribute("data-native") && !el.closest(".cpk");
/** +1 / -1 when the point is on the upper / lower arrow of `rect` (the box), else 0. */
function nbDir(rect, x, y) {
  if (x < rect.right - NB_ZONE || x > rect.right || y < rect.top || y > rect.bottom) return 0;
  return y < rect.top + rect.height / 2 ? 1 : -1;
}
/** One step of `el` in direction `dir`, kept inside its min and max; true when the value changed. */
function nbStep(el, dir) {
  const before = el.value;
  try {
    if (el.value === "") el.value = el.min !== "" ? el.min : "0"; // an empty box starts at its lowest value
    else if (dir > 0) el.stepUp();
    else el.stepDown();
  } catch { return false; } // a step the box cannot take (step="any")
  if (el.value === before) return false;
  el.dispatchEvent(new Event("input", { bubbles: true }));
  return true;
}

if (typeof document !== "undefined") {
  let held = null; // { el, dir, timer, changed }
  const release = () => {
    if (!held) return;
    clearTimeout(held.timer);
    clearInterval(held.timer);
    const h = held;
    held = null;
    if (h.changed) h.el.dispatchEvent(new Event("change", { bubbles: true }));
  };
  document.addEventListener("pointerdown", (e) => {
    const el = e.target;
    if (e.button !== 0 || !nbHasArrows(el)) return;
    const dir = nbDir(el.getBoundingClientRect(), e.clientX, e.clientY);
    if (!dir) return;
    e.preventDefault(); // the caret stays where it is; the press is the arrow's
    el.focus();
    release();
    held = { el, dir, changed: nbStep(el, dir), timer: 0 };
    held.timer = setTimeout(() => {
      if (!held) return;
      held.timer = setInterval(() => { if (held && nbStep(held.el, held.dir)) held.changed = true; }, 60);
    }, 380);
  }, true);
  for (const ev of ["pointerup", "pointercancel", "blur"]) window.addEventListener(ev, release, true);
  // The pointer shows that the arrows can be pressed.
  document.addEventListener("pointermove", (e) => {
    const el = e.target;
    if (!nbHasArrows(el)) return;
    const on = nbDir(el.getBoundingClientRect(), e.clientX, e.clientY) !== 0;
    if (el.classList.contains("nb-arrow") !== on) el.classList.toggle("nb-arrow", on);
  }, true);
}

// Lets the pure functions be tested under Node; ignored in the app.
if (typeof module !== "undefined" && module.exports) module.exports = { NB_ZONE, nbDir };
