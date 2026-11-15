"use strict";
/* Glint's site: three small things.
   1. A picture's motion runs only while the picture is on screen.
   2. "Pause motion" stops every picture (and is remembered).
   3. The newest version number is read from GitHub and shown beside the eyebrow. */

(function () {
  const root = document.documentElement;
  const stages = [...document.querySelectorAll(".stage")];

  // 1. On screen = live. Without IntersectionObserver every picture simply runs.
  if ("IntersectionObserver" in window) {
    const io = new IntersectionObserver((entries) => {
      for (const e of entries) e.target.classList.toggle("is-live", e.isIntersecting);
    }, { threshold: 0.2 });
    stages.forEach((s) => io.observe(s));
  } else stages.forEach((s) => s.classList.add("is-live"));

  // 2. The pause switch.
  const btn = document.querySelector("[data-motion-toggle]");
  const setMotion = (on) => {
    root.dataset.motion = on ? "on" : "off";
    if (btn) { btn.setAttribute("aria-pressed", String(!on)); btn.textContent = on ? "Pause motion" : "Play motion"; }
  };
  let wanted = true;
  try { wanted = localStorage.getItem("glint.site.motion") !== "off"; } catch { /* private window: not remembered */ }
  setMotion(wanted);
  btn?.addEventListener("click", () => {
    const on = root.dataset.motion === "off";
    setMotion(on);
    try { localStorage.setItem("glint.site.motion", on ? "on" : "off"); } catch { /* not remembered */ }
  });

  // 3. The latest release, if GitHub answers. The page is complete without it.
  const ver = document.querySelector("[data-version]");
  if (ver && "fetch" in window) {
    fetch("https://api.github.com/repos/swamie/glint/releases/latest", { headers: { Accept: "application/vnd.github+json" } })
      .then((r) => (r.ok ? r.json() : null))
      .then((rel) => {
        const tag = rel && typeof rel.tag_name === "string" ? rel.tag_name.replace(/^v/, "") : "";
        if (!/^\d+\.\d+\.\d+$/.test(tag)) return;
        ver.textContent = `v${tag}`;
        ver.hidden = false;
      })
      .catch(() => { /* offline, or rate limited: no number */ });
  }
})();
