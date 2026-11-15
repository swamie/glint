"use strict";
/* The buffer calculator on memory.html. The rules are Glint's own (README, Replay buffer):
   size = bitrate x length; RAM + disk keeps the newest 30 s in RAM, and a buffer barely longer
   than that (under about 40 s) stays in RAM; Disk only keeps about 1 s of video and 10 s of audio.
   Audio (desktop + microphone) was measured at about 0.38 MB a second; the encoder at 131 MB
   and Glint at 17 MB (2160 x 1440). */

(function () {
  const root = document.querySelector("[data-calc]");
  if (!root) return;

  const LENGTHS = [30, 60, 120, 300, 600, 1200, 1800, 3600]; // seconds
  const AUDIO = 0.38;        // MB a second
  const RAM_PART = 30;       // seconds, the default
  const FIXED = 131 + 17;    // MB: encoder + Glint
  const rate = root.querySelector('[data-in="rate"]');
  const len = root.querySelector('[data-in="len"]');
  const modes = [...root.querySelectorAll("[data-mode]")];
  let mode = "mixed";

  const lenText = (s) => (s < 60 ? `${s} s` : s < 3600 ? `${s / 60} min` : "1 hour");
  const size = (mb) => (mb >= 1000 ? `${(mb / 1000).toFixed(mb >= 10000 ? 1 : 2)} GB` : `${Math.round(mb)} MB`);

  function draw() {
    const mbps = Number(rate.value);
    const secs = LENGTHS[Number(len.value)] || 600;
    const video = mbps / 8; // MB a second
    const total = secs * (video + AUDIO);
    let ram;
    if (mode === "ram") ram = total;
    else if (mode === "disk") ram = Math.min(total, video * 1 + AUDIO * 10);
    else ram = secs <= RAM_PART + 10 ? total : RAM_PART * (video + AUDIO);
    const disk = Math.max(0, total - ram);

    root.querySelector('[data-out="rate"]').textContent = `${mbps} Mbps`;
    root.querySelector('[data-out="len"]').textContent = lenText(secs);
    root.querySelector('[data-res="ram"]').textContent = size(ram);
    root.querySelector('[data-res="disk"]').textContent = disk > 0 ? size(disk) : "0 MB";
    root.querySelector('[data-res="total"]').textContent = size(ram + FIXED);
    const share = total > 0 ? (ram / total) * 100 : 100;
    root.querySelector('[data-bar="ram"]').style.width = `${Math.max(share, 0.8)}%`;
    root.querySelector('[data-bar="disk"]').style.width = `${Math.max(100 - share, 0)}%`;
    modes.forEach((b) => b.setAttribute("aria-checked", String(b.dataset.mode === mode)));
  }

  rate.addEventListener("input", draw);
  len.addEventListener("input", draw);
  modes.forEach((b) => b.addEventListener("click", () => { mode = b.dataset.mode; draw(); }));
  draw();
})();
