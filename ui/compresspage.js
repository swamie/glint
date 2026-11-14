"use strict";
// The Compress page (sidebar → Compress): pick clips or any video file, pick a size,
// see what each will become (compress_plan), start, follow the jobs, use the results.
// Its settings (settings.compress, settings.compress_mb) are on the same page and are
// used by the clip menu's "Compress for Discord" too.
// Loaded before app.js: app.js globals (S, $, row, select, toggle, esc, invoke, ...) are
// used only inside functions. Names start with `cpg` / `CPG`; markup uses `cpg-`
// classes and `data-cpg*` attributes. The pure parts at the top are tested in Node.

/* ---------- pure helpers (Node tests: module.exports at the end) ---------- */

/** "10, 20 50;500" -> [10, 20, 50, 500]: whole numbers in min..max, no repeats, at most 8, in the order typed. */
function cpgParseSizes(text, min = 5, max = 1000) {
  const out = [];
  for (const part of String(text || "").split(/[^0-9.]+/)) {
    if (!part) continue;
    const n = Math.round(+part);
    if (Number.isFinite(n) && n >= min && n <= max && !out.includes(n) && out.length < 8) out.push(n);
  }
  return out;
}

/** The sizes the clip menu lists: the saved ones, cleaned; the defaults when none is left. */
function cpgMenuSizes(settings, fallback = [10, 20, 50, 500]) {
  const list = cpgParseSizes(((settings && settings.compress && settings.compress.sizes) || []).join(","));
  return list.length ? list : fallback.slice();
}

/** "2.9 Mb/s" / "450 kb/s". */
const cpgRate = (kbps) => (kbps >= 1000 ? `${(kbps / 1000).toFixed(1)} Mb/s` : `${Math.round(kbps)} kb/s`);

/** The clips (from `clips`) whose title or game holds every word of `q`, newest first. */
function cpgFilter(clips, q) {
  const words = String(q || "").toLowerCase().split(/\s+/).filter(Boolean);
  return clips
    .filter((c) => !c.draft && words.every((w) => `${c.title} ${c.game}`.toLowerCase().includes(w)))
    .sort((a, b) => (b.created || 0) - (a.created || 0));
}

/** Add `add` (items with a `key`) to `list` without repeats, keeping the order. */
function cpgMerge(list, add) {
  const keys = new Set(list.map((x) => x.key));
  return [...list, ...add.filter((x) => !keys.has(x.key) && keys.add(x.key))];
}

/** The plan request for the items: library clip ids and file paths. */
function cpgPlanArgs(items, mb) {
  return { ids: items.filter((x) => x.id).map((x) => x.id), paths: items.filter((x) => !x.id).map((x) => x.path), mb };
}

/* ---------- state ---------- */

const CPG = {
  items: [],        // what is to be compressed: { key, id ("" for a file), path, title }
  mb: 0,            // the size picked on the page (0 = not yet: the default size)
  plans: {},        // key -> compress_plan row, for `planKey`
  planKey: "",      // what the plans were made for (items, size, settings)
  planBusy: false,
  jobs: {},         // job id -> item key, for the jobs started here
  results: [],      // finished: { job_id, key, title, size, mb, path, clip_id, copied, secs }
  info: null,       // compress_info: size range, presets, what Auto resolves to
  pick: null,       // the clip picker while open: { q, sel: Set of ids }
  bound: false,
};

const cpgMb = () => clamp(Math.round(CPG.mb || +S.settings.compress_mb || 10), CPG.info?.min_mb || 5, CPG.info?.max_mb || 1000);

/** One-time: the events this page follows, and the fixed numbers from the backend. */
function cpgInit() {
  if (CPG.bound) return;
  CPG.bound = true;
  const ev = window.__TAURI__?.event;
  if (ev) {
    ev.listen("jobs-changed", () => setTimeout(cpgJobsChanged, 0)); // after jobs.js has the new list
    ev.listen("clip-compressed", ({ payload }) => cpgCompressed(payload));
  }
  invoke("compress_info").then((info) => { if (info) { CPG.info = info; if (S.page === "compress") cpgRedraw(); } }).catch(() => {});
}

/** Redraw the page unless the user is typing in one of its fields. */
function cpgRedraw() {
  if (S.page !== "compress") return;
  if (document.activeElement?.matches?.("#view input, #view select, #view textarea")) return;
  render();
}

/* ---------- the page ---------- */

function renderCompressPage() {
  cpgInit();
  const s = S.settings;
  // Settings saved before 0.1.16 have no "compress" part yet: the backend's defaults.
  if (!s.compress || typeof s.compress !== "object") s.compress = { sizes: [10, 20, 50, 500], preset: "veryfast", audio_kbps: 0, max_height: 0, max_fps: 0, audio: "first", dest: "beside", dest_dir: "", copy: true };
  const c = s.compress;
  const mb = cpgMb();
  const sizes = cpgMenuSizes(s);
  const auto = CPG.info?.auto;
  cpgPlanFetch(mb);

  const rows = CPG.items.map((it) => cpgItemHtml(it, mb)).join("");
  const n = CPG.items.length;
  const startable = CPG.items.filter((it) => cpgCanStart(it)).length;
  const custom = !sizes.includes(mb);
  const sizeChips = sizes.map((x) => `<button class="chip${x === mb ? " active" : ""}" data-cpg-mb="${x}">${t("{n} MB", { n: x })}</button>`).join("")
    + `<label class="cpg-custom${custom ? " on" : ""}"><span>${t("Custom")}</span><input class="field" type="number" data-cpg-custom min="${CPG.info?.min_mb || 5}" max="${CPG.info?.max_mb || 1000}" step="1" value="${mb}" aria-label="${esc(t("Custom size in MB"))}"><span>MB</span></label>`;

  const results = CPG.results.slice().reverse().map((r) => {
    const line = t("{size}, under {mb} MB, in {secs}", { size: fmtSize(r.size), mb: r.mb, secs: fmtLen(Math.round(r.secs || 0)) }) + (r.copied ? " · " + t("copied") : "");
    return `<div class="row cpg-res"><div class="txt"><div class="name" title="${esc(r.path)}">${esc(r.title)}</div><div class="hint">${esc(line)}</div></div>
      <div class="ctl"><button class="btn sm" data-cpg-copy="${esc(r.job_id || r.path)}">${ICON.copy}${t("Copy")}</button><button class="btn sm" data-cpg-reveal="${esc(r.job_id || r.path)}">${ICON.folder}${t("Show in folder")}</button>${r.clip_id ? `<button class="btn sm" data-cpg-open="${esc(r.clip_id)}">${t("Open")}</button>` : ""}</div></div>`;
  }).join("");

  const audioAuto = auto ? t("Auto ({kbps} kb/s, {low} kb/s when the size is tight)", { kbps: auto.audio_kbps, low: auto.audio_low_kbps }) : t("Auto");
  const heightAuto = auto ? t("Auto (up to {h}p)", { h: auto.max_height }) : t("Auto");
  const fpsAuto = auto ? t("Auto (up to {fps} fps)", { fps: auto.max_fps }) : t("Auto");
  const presets = (CPG.info?.presets || ["ultrafast", "superfast", "veryfast", "faster", "fast", "medium", "slow"]).map((p) => [p, p === (CPG.info?.default_preset || "veryfast") ? t("{name} (default)", { name: p }) : p]);
  const destRow = c.dest === "folder"
    ? row(t("Folder"), c.dest_dir ? `<span class="path" title="${esc(c.dest_dir)}">${esc(c.dest_dir)}</span>` : t("None chosen yet: results go beside the clip"), `<button class="btn sm" data-cpg-dir>${t("Change")}</button>`)
    : "";

  $("#view").innerHTML = pageHead(t("Compress"), t("Make clips small enough to send, for Discord or anywhere else")) + `
    ${S.ffmpeg ? "" : `<div class="note bad">${t("<b>ffmpeg.exe was not found.</b> Glint needs ffmpeg 8 or newer. Install it or set its path below.")}</div>`}
    <div class="section cpg-clips"><h2>${t("Clips")}</h2>
      <div class="cpg-add"><button class="btn sm primary" data-cpg-pick>${ICON.plus}${t("Add clips from the library")}</button><button class="btn sm" data-cpg-file>${ICON.folder}${t("Choose a video file…")}</button>${n ? `<button class="btn sm cpg-clear" data-cpg-clear>${t("Clear list")}</button>` : ""}</div>
      ${n ? `<div class="cpg-list">${rows}</div>` : `<div class="cpg-empty">${t("Nothing picked yet. Add clips from your library, or any video file. You can also right-click a clip and pick Compress for Discord → More options…")}</div>`}
    </div>
    <div class="section"><h2>${t("Size")}</h2>
      <div class="cpg-sizes">${sizeChips}</div>
      <div class="cpg-start"><span class="hint">${t("Each result is at or under the size. 10 MB: Discord free · 50 MB: Nitro Basic · 500 MB: Nitro")}</span>
        <button class="btn primary" data-cpg-start${startable ? "" : " disabled"}>${ICON.compress}${startable > 1 ? t("Compress {clips}", { clips: plural(startable) }) : t("Compress")}</button></div>
    </div>
    ${results ? `<div class="section"><h2>${t("Results")}</h2>${results}</div>` : ""}
    <div class="section"><h2>${t("Compression settings")}</h2>
      ${row(t("Default size"), t("Picked first on this page and ticked in the clip menu"), `<input class="field" type="number" data-cpg-default min="5" max="1000" step="1" value="${+s.compress_mb || 10}"><span class="cpg-unit">MB</span>`)}
      ${row(t("Sizes in the clip menu"), t("Right-click a clip → Compress for Discord lists these. Up to 8, from 5 to 1000 MB"), `<input class="field" type="text" data-cpg-sizes value="${esc(sizes.join(", "))}" spellcheck="false" style="width:190px">`)}
      ${row(t("Speed"), t("Slower presets give a slightly better picture at the same size, and take longer"), select("compress.preset", presets))}
      ${row(t("Audio bitrate"), t("Taken from the size before the picture gets the rest"), select("compress.audio_kbps", [[0, audioAuto], [64, "64 kb/s"], [96, "96 kb/s"], [128, "128 kb/s"], [160, "160 kb/s"], [192, "192 kb/s"]], true))}
      ${row(t("Highest resolution"), t("Glint picks the size the bitrate can carry, never more than this"), select("compress.max_height", [[0, heightAuto], [1080, "1080p"], [720, "720p"], [480, "480p"], [360, "360p"]], true))}
      ${row(t("Highest frame rate"), t("Fewer frames leave more bits for each one"), select("compress.max_fps", [[0, fpsAuto], [60, "60 fps"], [30, "30 fps"], [24, "24 fps"]], true))}
      ${row(t("Audio tracks"), t("Glint's own clips have everything in their first track. Clips from other apps may keep the game and your voice apart"), select("compress.audio", [["first", t("First track only")], ["all", t("All tracks mixed into one")]]))}
      ${row(t("Save the result"), t("The original is never changed"), select("compress.dest", [["beside", t("Beside the clip")], ["folder", t("In a folder")]]))}
      ${destRow}
      ${row(t("Copy when done"), t("Puts the result on the clipboard, ready to paste into Discord"), toggle("compress.copy"))}
    </div>`;
  cpgBind();
}

/** One picked clip or file: picture, name, what it becomes (or why not), and its job. */
function cpgItemHtml(it, mb) {
  const clip = it.id ? S.clips.find((c) => c.id === it.id) : null;
  const p = CPG.plans[it.key];
  const job = cpgJobOf(it.key);
  let what;
  if (!p) what = CPG.planBusy ? t("Working out the size…") : "";
  else if (p.error) what = `<span class="cpg-bad">${esc(t(p.error))}</span>`;
  else if (p.small) what = t("Already under {mb} MB ({size}): nothing to do", { mb, size: fmtSize(p.size) });
  else {
    const from = `${p.src_w}×${p.src_h} · ${p.src_fps || "?"} fps`;
    const to = `${p.width}×${p.height} · ${p.fps} fps`;
    const audio = p.audio_kbps ? (p.audio_mix > 1 ? t("audio {rate}, {n} tracks mixed", { rate: cpgRate(p.audio_kbps), n: p.audio_mix }) : t("audio {rate}", { rate: cpgRate(p.audio_kbps) })) : t("no audio");
    what = `${esc(fmtSize(p.size))} · ${esc(from)} → <b>${esc(to)}</b> · ${esc(t("video {rate}", { rate: cpgRate(p.video_kbps) }))} · ${esc(audio)}`;
  }
  let state = "";
  if (job) {
    const pct = Math.floor(+job.pct || 0);
    state = job.state === "running" ? `<div class="cpg-state">${t("Compressing {pct} %", { pct })}</div><div class="progress"><i style="width:${pct}%"></i></div>`
      : job.state === "queued" ? `<div class="cpg-state">${t("Waiting")}</div>`
      : job.state === "done" ? `<div class="cpg-state ok">${t("Finished")}</div>`
      : job.state === "error" ? `<div class="cpg-state cpg-bad" title="${esc(job.message || "")}">${esc(job.message ? t("Failed: {message}", { message: t(job.message) }) : t("Failed"))}</div>`
      : `<div class="cpg-state">${t("Cancelled")}</div>`;
  }
  const busy = job && (job.state === "queued" || job.state === "running");
  const pic = clip && clip.thumb ? `<img src="${esc(fileSrc(clip.thumb))}" alt="" loading="lazy">` : `<span class="cpg-noimg">${ICON.library}</span>`;
  return `<div class="cpg-item" data-cpg-key="${esc(it.key)}" data-cpg-state="${job ? esc(job.state) : ""}"><div class="cpg-pic">${pic}</div>
    <div class="cpg-main"><div class="cpg-title" title="${esc(it.path || it.title)}">${esc(it.title)}${it.id ? "" : ` <span class="cpg-tag">${t("File")}</span>`}</div><div class="cpg-plan">${what}</div><div class="cpg-job">${state}</div></div>
    ${busy ? `<button class="btn sm" data-cpg-cancel="${esc(job.id)}">${t("Cancel")}</button>` : `<button class="cpg-x" data-cpg-remove="${esc(it.key)}" title="${esc(t("Remove from the list"))}" aria-label="${esc(t("Remove from the list"))}">${ICON.x}</button>`}</div>`;
}

/** The newest job started here for an item, from the renders list (jobs.js). */
function cpgJobOf(key) {
  const list = typeof jobsList !== "undefined" ? jobsList : [];
  let best = null;
  for (const j of list) if (CPG.jobs[j.id] === key && (!best || (+j.queued || 0) >= (+best.queued || 0))) best = j;
  return best;
}

const cpgCanStart = (it) => {
  const p = CPG.plans[it.key];
  const j = cpgJobOf(it.key);
  return !!p && !p.error && !p.small && !(j && (j.state === "queued" || j.state === "running"));
};

/** Ask the backend what every item becomes, once per change of items, size or settings. */
function cpgPlanFetch(mb) {
  const s = S.settings;
  const key = JSON.stringify([CPG.items.map((x) => x.key), mb, s.compress || {}, s.ffmpeg_path || ""]);
  if (key === CPG.planKey) return;
  CPG.planKey = key;
  if (!CPG.items.length) { CPG.plans = {}; CPG.planBusy = false; return; }
  CPG.planBusy = true;
  invoke("compress_plan", cpgPlanArgs(CPG.items, mb))
    .then((rows) => {
      if (CPG.planKey !== key) return;
      CPG.plans = Object.fromEntries((rows || []).map((r) => [r.key, r]));
    })
    .catch((e) => { if (CPG.planKey === key) { CPG.plans = Object.fromEntries(CPG.items.map((it) => [it.key, { key: it.key, error: String(e?.message ?? e) }])); } })
    .finally(() => { if (CPG.planKey === key) { CPG.planBusy = false; cpgRedraw(); } });
}

/** "More options…" in the clip menu: open this page with those clips. */
function compressOpenPage(ids) {
  const add = ids.map((id) => S.clips.find((c) => c.id === id)).filter(Boolean).map((c) => ({ key: c.id, id: c.id, path: c.path, title: c.title }));
  CPG.items = cpgMerge(CPG.items, add);
  if (typeof endCapture === "function") endCapture();
  S.page = "compress";
  S.sel.clear();
  $("#view").scrollTop = 0;
  render();
}

/** The sizes for the clip menu (app.js openClipMenu). */
const compressMenuSizes = () => cpgMenuSizes(S.settings, typeof COMPRESS_MB !== "undefined" ? COMPRESS_MB : [10, 20, 50, 500]);

async function cpgStart() {
  const mb = cpgMb();
  const errors = [];
  let n = 0;
  for (const it of CPG.items.filter(cpgCanStart)) {
    try {
      const job = it.id ? await invoke("compress_clip", { id: it.id, mb }) : await invoke("compress_file", { path: it.path, mb });
      if (job && job.id) {
        CPG.jobs[job.id] = it.key;
        n++;
        if (typeof jobsSet === "function" && !jobsList.some((j) => j.id === job.id)) jobsSet([...jobsList, job]);
      }
    } catch (e) { errors.push(`${it.title}: ${t(String(e?.message ?? e))}`); }
  }
  render();
  if (errors.length) flash(errors.length === 1 ? errors[0] : t("{n} of the clips were not compressed: {reason}", { n: errors.length, reason: errors[0] }), true);
  else if (n) flash(n === 1 ? t("Compressing…") : t("Compressing {clips}…", { clips: plural(n) }));
}

function cpgJobsChanged() {
  if (S.page !== "compress") return;
  // Progress alone: update the bars in place. A state change: redraw.
  for (const it of CPG.items) {
    const j = cpgJobOf(it.key);
    const el = document.querySelector(`.cpg-item[data-cpg-key="${CSS.escape(it.key)}"]`);
    if (!j || !el) continue;
    const bar = el.querySelector(".cpg-job .progress i");
    if (j.state === "running" && bar) {
      const pct = Math.floor(+j.pct || 0);
      bar.style.width = `${pct}%`;
      el.querySelector(".cpg-state").textContent = t("Compressing {pct} %", { pct });
    } else if (el.dataset.cpgState !== j.state) {
      el.dataset.cpgState = j.state;
      return cpgRedraw();
    }
  }
}

/** "clip-compressed": { clip|null, mb, size, copied, secs, path, job_id, source }. Only jobs started here are listed. */
function cpgCompressed(p) {
  if (!p) return;
  const key = CPG.jobs[p.job_id] ?? (CPG.items.some((x) => x.key === p.source) ? p.source : null);
  if (key == null) return;
  const it = CPG.items.find((x) => x.key === key);
  CPG.results.push({ job_id: p.job_id || "", key, title: p.clip ? p.clip.title : String(p.path || "").split(/[\\/]/).pop(), size: p.size, mb: p.mb, path: p.path || (p.clip && p.clip.path) || "", clip_id: p.clip ? p.clip.id : "", copied: !!p.copied, secs: p.secs, from: it ? it.title : "" });
  // A file from outside the library gets no library flash (app.js shows one only for library clips).
  if (!p.clip) flash(p.copied ? t("Compressed to {size} and copied. Paste it into Discord.", { size: fmtSize(p.size) }) : t("Compressed to {size}.", { size: fmtSize(p.size) }));
  cpgRedraw();
}

const cpgResult = (k) => CPG.results.find((r) => (r.job_id || r.path) === k);

/* ---------- events ---------- */

function cpgBind() {
  const view = $("#view");
  if (view.cpgBound) return;
  view.cpgBound = true;
  view.addEventListener("click", cpgClick);
  view.addEventListener("change", cpgChange);
}

async function cpgClick(e) {
  if (S.page !== "compress") return;
  const el = e.target.closest("[data-cpg-pick],[data-cpg-file],[data-cpg-clear],[data-cpg-mb],[data-cpg-start],[data-cpg-remove],[data-cpg-cancel],[data-cpg-copy],[data-cpg-reveal],[data-cpg-open],[data-cpg-dir]");
  if (!el || el.disabled) return;
  const d = el.dataset;
  try {
    if ("cpgPick" in d) return cpgPickOpen();
    if ("cpgFile" in d) {
      const files = (await invoke("pick_compress_files")) || [];
      if (!files.length) return;
      const byPath = new Map(S.clips.map((c) => [String(c.path).toLowerCase(), c]));
      // A file that is a library clip is added as that clip (its result joins the library).
      CPG.items = cpgMerge(CPG.items, files.map((f) => {
        const c = byPath.get(String(f).toLowerCase());
        return c ? { key: c.id, id: c.id, path: c.path, title: c.title } : { key: f, id: "", path: f, title: String(f).split(/[\\/]/).pop().replace(/\.[^.]+$/, "") };
      }));
      return render();
    }
    if ("cpgClear" in d) { CPG.items = CPG.items.filter((it) => { const j = cpgJobOf(it.key); return j && (j.state === "queued" || j.state === "running"); }); return render(); }
    if (d.cpgMb) { CPG.mb = +d.cpgMb; return render(); }
    if ("cpgStart" in d) return await cpgStart();
    if (d.cpgRemove) { CPG.items = CPG.items.filter((x) => x.key !== d.cpgRemove); return render(); }
    if (d.cpgCancel) { el.disabled = true; await invoke("cancel_job", { id: d.cpgCancel }); return; }
    if (d.cpgCopy) {
      const r = cpgResult(d.cpgCopy);
      if (!r) return;
      await (r.clip_id ? invoke("copy_clip", { id: r.clip_id }) : invoke("compress_copy", { path: r.path }));
      return flash(t("Clip copied. Paste it anywhere."));
    }
    if (d.cpgReveal) {
      const r = cpgResult(d.cpgReveal);
      if (r) await (r.clip_id ? invoke("reveal_clip", { id: r.clip_id }) : invoke("compress_reveal", { path: r.path }));
      return;
    }
    if (d.cpgOpen) {
      if (!S.clips.some((c) => c.id === d.cpgOpen)) await refreshClips();
      if (S.clips.some((c) => c.id === d.cpgOpen)) return openClip(d.cpgOpen);
      return flash(t("That clip is no longer in the library"), true);
    }
    if ("cpgDir" in d) {
      const dir = await invoke("pick_folder");
      if (dir) { S.settings.compress.dest_dir = dir; saveSettings(true); }
    }
  } catch (err) { fail(err); }
}

function cpgChange(e) {
  if (S.page !== "compress") return;
  const el = e.target;
  const lo = CPG.info?.min_mb || 5, hi = CPG.info?.max_mb || 1000;
  if ("cpgCustom" in el.dataset) {
    const n = Math.round(+el.value);
    if (!Number.isFinite(n) || n < lo || n > hi) { flash(t("Pick a size from {min} to {max} MB", { min: lo, max: hi }), true); return render(); }
    CPG.mb = n;
    el.blur();
    return render();
  }
  if ("cpgDefault" in el.dataset) {
    const n = Math.round(+el.value);
    if (!Number.isFinite(n) || n < lo || n > hi) { flash(t("Pick a size from {min} to {max} MB", { min: lo, max: hi }), true); return render(); }
    S.settings.compress_mb = n;
    el.blur();
    return saveSettings(true);
  }
  if ("cpgSizes" in el.dataset) {
    const list = cpgParseSizes(el.value, lo, hi);
    if (!list.length) { flash(t("Type one or more sizes from {min} to {max} MB, such as 10, 50, 500", { min: lo, max: hi }), true); return render(); }
    S.settings.compress.sizes = list;
    el.blur();
    return saveSettings(true);
  }
}

/* ---------- the clip picker ---------- */

function cpgPickOpen() {
  CPG.pick = { q: "", sel: new Set() };
  const el = document.createElement("div");
  el.className = "cpg-pick";
  el.innerHTML = `<div class="cpg-pick-box" role="dialog" aria-label="${esc(t("Add clips"))}">
      <div class="cpg-pick-head"><b>${t("Add clips")}</b>
        <label class="search set-search cpg-pick-q"><svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="7"/><path d="m20 20-3.500-3.500"/></svg><input type="text" placeholder="${esc(t("Search clips and games"))}" spellcheck="false" autocomplete="off" aria-label="${esc(t("Search clips and games"))}"></label>
        <button class="btn sm" data-cpp-all>${t("Select all shown")}</button></div>
      <div class="cpg-pick-list" role="listbox" aria-multiselectable="true"></div>
      <div class="cpg-pick-foot"><span class="cpg-pick-n"></span><button class="btn" data-cpp-cancel>${t("Cancel")}</button><button class="btn primary" data-cpp-add>${t("Add")}</button></div>
    </div>`;
  document.body.appendChild(el);
  CPG.pickEl = el;
  el.addEventListener("click", cpgPickClick);
  el.addEventListener("change", (e) => {
    const box = e.target.closest("[data-cpp-id]");
    if (!box) return;
    if (box.checked) CPG.pick.sel.add(box.dataset.cppId); else CPG.pick.sel.delete(box.dataset.cppId);
    cpgPickFoot();
  });
  const q = el.querySelector(".cpg-pick-q input");
  q.addEventListener("input", () => { CPG.pick.q = q.value; cpgPickList(); });
  cpgPickList();
  q.focus();
}

function cpgPickList() {
  const el = CPG.pickEl;
  if (!el) return;
  const have = new Set(CPG.items.map((x) => x.key));
  const list = cpgFilter(S.clips, CPG.pick.q);
  el.querySelector(".cpg-pick-list").innerHTML = list.length ? list.map((c) => {
    const added = have.has(c.id);
    const pic = c.thumb ? `<img src="${esc(fileSrc(c.thumb))}" alt="" loading="lazy">` : `<span class="cpg-noimg">${ICON.library}</span>`;
    return `<label class="cpg-pick-row${added ? " added" : ""}"><input type="checkbox" data-cpp-id="${esc(c.id)}"${added || CPG.pick.sel.has(c.id) ? " checked" : ""}${added ? " disabled" : ""}>
      <span class="cpg-pic">${pic}</span><span class="cpg-pick-txt"><span class="cpg-title">${esc(c.title)}</span><span class="cpg-plan">${esc(c.game || "")} · ${fmtDur(c.duration || 0)} · ${fmtSize(c.size || 0)}${added ? " · " + esc(t("Added")) : ""}</span></span></label>`;
  }).join("") : `<div class="cpg-empty">${esc(CPG.pick.q ? t("No clip matches “{text}”", { text: CPG.pick.q.trim() }) : t("No clips yet"))}</div>`;
  cpgPickFoot();
}

function cpgPickFoot() {
  const el = CPG.pickEl;
  const n = CPG.pick.sel.size;
  el.querySelector(".cpg-pick-n").textContent = n ? t("{n} selected", { n }) : "";
  const add = el.querySelector("[data-cpp-add]");
  add.disabled = !n;
  add.textContent = n > 1 ? t("Add {clips}", { clips: plural(n) }) : t("Add");
}

function cpgPickClick(e) {
  if (e.target === CPG.pickEl) return cpgPickClose(false); // outside the box
  if (e.target.closest("[data-cpp-cancel]")) return cpgPickClose(false);
  if (e.target.closest("[data-cpp-add]")) return cpgPickClose(true);
  if (e.target.closest("[data-cpp-all]")) {
    const have = new Set(CPG.items.map((x) => x.key));
    for (const c of cpgFilter(S.clips, CPG.pick.q)) if (!have.has(c.id)) CPG.pick.sel.add(c.id);
    cpgPickList();
  }
}

function cpgPickClose(add) {
  const el = CPG.pickEl;
  if (!el) return;
  const ids = add ? [...CPG.pick.sel] : [];
  el.remove();
  CPG.pickEl = null;
  CPG.pick = null;
  if (ids.length) {
    // In library order (newest first), as the list showed them.
    const order = cpgFilter(S.clips, "").map((c) => c.id).filter((id) => ids.includes(id));
    CPG.items = cpgMerge(CPG.items, order.map((id) => S.clips.find((c) => c.id === id)).map((c) => ({ key: c.id, id: c.id, path: c.path, title: c.title })));
  }
  render();
  $("[data-cpg-pick]")?.focus();
}

// Registered before app.js's listeners (this file loads first): while the picker is open the keyboard is its own.
if (typeof window !== "undefined" && window.addEventListener) {
  window.addEventListener("keydown", (e) => {
    if (!CPG.pickEl) return;
    if (e.key === "Escape") { e.preventDefault(); cpgPickClose(false); }
    else if (e.key === "Enter" && CPG.pick.sel.size && !e.target.closest?.("button")) { e.preventDefault(); cpgPickClose(true); }
    e.stopImmediatePropagation();
  }, true);
}

if (typeof module !== "undefined" && module.exports) module.exports = { cpgParseSizes, cpgMenuSizes, cpgRate, cpgFilter, cpgMerge, cpgPlanArgs };
