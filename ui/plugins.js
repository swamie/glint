"use strict";
// Plugins: the marketplace page (Tools → Plugins) and what an installed plugin adds to the window.
// Backend: src-tauri/src/plugins.rs. How to write one: plugins/README.md.
//
// With no plugin installed this file only adds the "Plugins" item to the sidebar. An installed,
// switched-on plugin's script is run once per page load with one argument, `Glint`, and calls
// `Glint.register({ ... })` with what it adds:
//
//   defaults   its settings' first values. They live in S.settings.plugins[<id>] and are saved with Glint's own.
//   lang       { ko: { "English text": "…" }, el: { … } }: its texts in Glint's other languages.
//   pages      [{ id, title, icon, render(ctx), after(ctx, root), badge(ctx) }]: pages under Tools.
//   onDrop(paths, point, ctx)   files were dropped on Glint's window while one of its pages is open
//                          (.plug-dropping is on the page while they hover).
//   settings   [{ title, cat, render(ctx) }]: sections of the Settings page (cat: one of its categories, or "plugins").
//   actions    { <action id>(ctx) }: run when a keybind with that action fires while the window is open.
//              The actions themselves (id, label, the process they start) are listed in plugin.json,
//              so a keybind also works while the window is closed.
//   topbar(ctx)            markup for the top bar ("" for nothing): a short status such as "STREAMING: 1080p @ 60".
//                          Build it from Glint's own top bar buttons: .btn, joined with .top-group; .top-dot is a small accent dot.
//   onAct(name, el, ctx)   a click on an element with data-pact="name" inside its page or sections.
//   onSave(ctx)            just before the settings are saved: the place to work out derived values.
//   onProc(status, ctx)    one of its processes started or ended.
//   onDownload(p, ctx)     one of its downloads moved on: { id, phase, done, total, error }.
//   editor     { title, tab, render(ed, ctx), onOpen(ed, ctx), onSync(ed, what, ctx), onClose(ed, ctx) }: a section in the
//              editor's side pane (tab: "clip", "color" or "overlays"). `ed` is the open editor: see plugEditorHandle below.
//
// `ctx` (also `Glint.ctx`): { id, s (its settings), key(name) (the data-set path of one), save(rerender),
// row, toggle, select, number, text, t, esc, invoke, flash, fail, render, chrome (redraw the top bar and sidebar only),
// go(page), glint (Glint's own state),
// proc: { start(name, args, { audio, quiet, tool }), stop(name), status(name), run(name, args, opts) },
// dl: { state(), get(id), cancel(id), remove(id) }, work: { read(name), clear() }, saveText(name, text),
// editor() (the open editor's handle, or null) }.

const PLUG = {
  defs: new Map(),   // plugin id -> { info (from plugins_boot), def (what it registered), ctx }
  failed: {},        // plugin id -> why its script did not load
  state: null,       // plugins_state: { installed: [...], restart }
  market: null,      // plugins_market: { plugins: [...], error, cached }
  marketBusy: false,
  busy: {},          // plugin id -> percent of a running install
  ed: null,          // the open editor's plugin sections (plugEditorMount)
  ready: false,
};
ICON.plug = '<svg viewBox="0 0 24 24"><path d="M9 3v5M15 3v5M6.500 8h11v4.500a5.500 5.500 0 0 1-11 0zM12 18v3"/></svg>';
ICON.shield = '<svg viewBox="0 0 24 24"><path d="M12 3 5 6v5.500c0 4.300 2.900 7.700 7 9 4.100-1.300 7-4.700 7-9V6z"/><path d="m9 12 2.200 2.200L15.200 10"/></svg>';

const plugPageId = (pluginId, pageId) => `p:${pluginId}:${pageId}`;
const plugSettingsOf = (id) => ((S.settings.plugins ||= {})[id] ||= {});

/** What a plugin's script works with. */
function plugCtx(info) {
  const id = info.id;
  const key = (name) => `plugins.${id}.${name}`;
  return {
    id,
    get s() { return plugSettingsOf(id); },
    key,
    save: (rerender = false) => saveSettings(rerender),
    row,
    toggle: (name) => toggle(key(name)),
    select: (name, opts, num = false) => select(key(name), opts, num),
    number: (name, min, max, step = 1) => number(key(name), min, max, step),
    /** A text box bound to one of its settings. opts: { placeholder, password, width }. */
    text: (name, opts = {}) => `<input class="field" type="${opts.password ? "password" : "text"}" data-set="${key(name)}" value="${esc(plugSettingsOf(id)[name] ?? "")}" placeholder="${esc(opts.placeholder || "")}" spellcheck="false" autocomplete="off" style="width:${opts.width || 300}px">`,
    t, esc, invoke, flash, fail,
    render: () => render(),
    chrome: () => renderChrome(),
    go: (page) => goPage(page),
    get glint() { return { settings: S.settings, status: S.status, monitors: S.monitors, hardware: S.hardware, version: S.version }; },
    /** The address a local file is shown from (an <img>'s src). */
    fileSrc: (path) => T.core.convertFileSrc(path),
    /** The plugin's own copies of pictures the user gave it: keep(path) -> { path, name, size }, drop(path). */
    files: {
      keep: (path) => invoke("plugin_file_keep", { plugin: id, path }),
      drop: (path) => invoke("plugin_file_drop", { plugin: id, path }),
    },
    proc: {
      /** opts.tool: one of the plugin's downloads with a program (plugin.json "downloads"): that runs instead of ffmpeg. */
      start: (name, args, opts = {}) => invoke("plugin_proc_start", { plugin: id, name, args, audio: !!opts.audio, quiet: !!opts.quiet, tool: opts.tool || null }),
      stop: (name) => invoke("plugin_proc_stop", { plugin: id, name }),
      status: (name) => invoke("plugin_proc_status", { plugin: id, name }),
      /** Start a process that does one job and wait for its end: its last status ({ code, error, log }).
       *  opts.onTick(status) is called about twice a second while it runs (its last lines are in status.log). */
      async run(name, args, opts = {}) {
        await invoke("plugin_proc_start", { plugin: id, name, args, audio: false, quiet: true, tool: opts.tool || null });
        for (;;) {
          await new Promise((r) => setTimeout(r, 400));
          const st = await invoke("plugin_proc_status", { plugin: id, name });
          if (!st.running) return st;
          if (opts.onTick) plugSafe(id, "onTick", () => opts.onTick(st));
        }
      },
    },
    /** The plugin's downloads (plugin.json "downloads"): files it needs but does not carry, checked when they arrive. */
    dl: {
      state: () => invoke("plugin_dl_state", { plugin: id }),
      get: (dl) => invoke("plugin_dl_get", { plugin: id, id: dl }),
      cancel: (dl) => invoke("plugin_dl_cancel", { plugin: id, id: dl }),
      remove: (dl) => invoke("plugin_dl_remove", { plugin: id, id: dl }),
    },
    /** The scratch folder its processes write to ("{work}" in their arguments). */
    work: {
      read: (name) => invoke("plugin_work_read", { plugin: id, name }),
      clear: () => invoke("plugin_work_clear", { plugin: id }),
    },
    /** A save dialog for a text the plugin made (.srt, .vtt, .txt, .csv, .json): the path, or null when cancelled. */
    saveText: (name, text) => invoke("plugin_save_text", { plugin: id, name, text }),
    /** The open editor (plugEditorHandle), or null. */
    editor: () => PLUG.ed?.byId?.[id] || null,
  };
}

/** A plugin's settings always hold at least its defaults (also after the settings were read again). */
function plugEnsureSettings() {
  if (!S.settings) return;
  S.settings.plugins ||= {};
  for (const [id, p] of PLUG.defs) S.settings.plugins[id] = { ...(p.def.defaults || {}), ...(S.settings.plugins[id] || {}) };
}

function plugLoad(info) {
  const ctx = plugCtx(info);
  let def = null;
  const api = { register: (d) => { def = d || {}; }, ctx, version: S.version || "", t, esc, invoke };
  if (info.css) {
    const st = document.createElement("style");
    st.dataset.plugin = info.id;
    st.textContent = info.css;
    document.head.appendChild(st);
  }
  if (!info.js) return; // a plugin with a program of its own and no script (the local API)
  try {
    new Function("Glint", info.js)(api);
  } catch (e) {
    console.error(`Glint plugin ${info.id} failed to load`, e);
    PLUG.failed[info.id] = String(e?.message ?? e);
    return;
  }
  if (!def) return; // a script that only listens (the Claude plugin's bridge)
  // Its own texts in Glint's other languages. A text Glint already translates keeps Glint's wording.
  for (const [code, dict] of Object.entries(def.lang || {})) {
    const lang = window.GLINT_LANGS[code];
    if (lang) glintAddLanguage(code, lang.name, Object.fromEntries(Object.entries(dict).filter(([k]) => !(k in lang.dict))));
  }
  for (const pg of def.pages || []) ICON[plugPageId(info.id, pg.id)] = pg.icon || ICON.plug;
  PLUG.defs.set(info.id, { info, def, ctx });
}

/** Run a plugin's own code without letting a mistake in it break the page. */
function plugSafe(id, what, fn, fallback = "") {
  try { return fn(); } catch (e) { console.error(`Glint plugin ${id}: ${what} failed`, e); return fallback; }
}

/* ---------- what the rest of the window asks (app.js) ---------- */

/** The plugins' pages as sidebar items, then "Plugins" itself. */
function plugNavHtml(navItem) {
  let html = "";
  for (const [id, p] of PLUG.defs) {
    for (const pg of p.def.pages || []) html += navItem(plugPageId(id, pg.id), esc(t(pg.title)), plugPageId(id, pg.id), pg.badge ? plugSafe(id, "badge", () => esc(pg.badge(p.ctx) || "")) : 0);
  }
  const updates = plugUpdates().length;
  return html + navItem("plugins", t("Plugins"), "plug", updates || 0);
}

/** The top bar's plugin slot: what each plugin's `topbar` gives, patched in place (it is redrawn every second). */
function plugTopbarRender() {
  const el = $("#plugin-slot");
  if (!el) return;
  // Each plugin's wrapper stays in place and only its inside is redrawn, so an animation on the wrapper
  // (its fade-in, a pulsing outline) is not restarted every time the plugin's text changes.
  const seen = new Set();
  for (const [id, p] of PLUG.defs) {
    const part = p.def.topbar ? plugSafe(id, "topbar", () => p.def.topbar(p.ctx) || "") : "";
    if (!part) continue;
    seen.add(id);
    let w = [...el.children].find((x) => x.dataset.plugin === id);
    if (!w) { w = document.createElement("span"); w.className = "plug-top"; w.dataset.plugin = id; el.appendChild(w); }
    if (w._html !== part) { w._html = part; w.innerHTML = part; }
  }
  for (const w of [...el.children]) if (!seen.has(w.dataset.plugin)) w.remove();
}

/** Files are over the window, left it, or were dropped: the open page's plugin gets them, with where (in the page's own pixels). */
function plugDrop(ev, payload) {
  const host = $("#view .plug-page");
  const p = host && PLUG.defs.get(host.dataset.plugin);
  if (!p?.def.onDrop) return;
  host.classList.toggle("plug-dropping", ev.endsWith("drag-enter"));
  if (!ev.endsWith("drag-drop") || !payload.paths?.length) return;
  const k = window.devicePixelRatio || 1, at = payload.position || { x: 0, y: 0 };
  plugSafe(host.dataset.plugin, "onDrop", () => p.def.onDrop(payload.paths, { x: at.x / k, y: at.y / k }, p.ctx));
}

/** Draw a page of the sidebar that is not one of Glint's own. False when it is not ours either. */
function plugRenderPage(page) {
  if (page === "plugins") { renderPluginsPage(); return true; }
  const m = /^p:([^:]+):(.+)$/.exec(page || "");
  const p = m && PLUG.defs.get(m[1]);
  const pg = p && (p.def.pages || []).find((x) => x.id === m[2]);
  if (!pg) return false;
  $("#view").innerHTML = `<div class="plug-page" data-plugin="${esc(m[1])}">${plugSafe(m[1], "render", () => pg.render(p.ctx), `<div class="note bad">${esc(t("This plugin's page could not be drawn."))}</div>`)}</div>`;
  if (pg.after) plugSafe(m[1], "after", () => pg.after(p.ctx, $("#view .plug-page")));
  return true;
}

/** The plugins' sections of the Settings page. */
function plugSettingsHtml() {
  let html = "";
  for (const [id, p] of PLUG.defs) {
    for (const sec of p.def.settings || []) {
      html += `<div class="section" data-cat="${esc(sec.cat || "plugins")}" data-plugin="${esc(id)}"><h2>${esc(t(sec.title))}</h2>${plugSafe(id, "settings", () => sec.render(p.ctx))}</div>`;
    }
  }
  return html;
}
/** The "Plugins" category of the Settings page, while a plugin has a section in it. */
const plugSetCats = () => ([...PLUG.defs.values()].some((p) => (p.def.settings || []).some((s) => !s.cat || s.cat === "plugins")) ? [["plugins", t("Plugins")]] : []);

/** Keybind actions: [value, label] of every loaded plugin's, and the label of one that is not loaded. */
const plugKbActions = () => [...PLUG.defs.values()].flatMap((p) => (p.info.actions || []).map((a) => [`plugin:${p.info.id}:${a.id}`, t(a.label || a.id)]));
function plugKbMissing(action) {
  if (!String(action).startsWith("plugin:") || plugKbActions().some(([v]) => v === action)) return null;
  const id = action.split(":")[1] || "";
  const name = (PLUG.state?.installed || []).find((x) => x.id === id)?.name || id;
  return [action, t("{name} (plugin is off or not installed)", { name })];
}

function plugBeforeSave() {
  for (const [id, p] of PLUG.defs) if (p.def.onSave) plugSafe(id, "onSave", () => p.def.onSave(p.ctx));
}

/* ---------- the editor ---------- */

/**
 * What a plugin's `editor` works with: the editor that is open on clip `c` (player `P`, ui/editor.js).
 *   clip                    { id, title, path, width, height, duration, virtual }
 *   model                   the edit model (ui/editmodel.js), to read. Change it only inside change().
 *   editing                 the editor's tools are showing
 *   time(), seek(sec)       the playhead, in timeline seconds
 *   change(fn)              one undo step: fn(model) changes the model; the timeline and the picture follow. False if nothing changed.
 *   redraw()                draw the plugin's section again
 *   footage()               what the main row shows, in timeline order: [{ at, len, speed, path, from, tracks, missing }]:
 *                           `len` timeline seconds from `at` show the file `path` from its second `from`, at `speed`;
 *                           tracks: that file's audio streams [{ index, title }]. Gaps are left out.
 *   removedAt(sec)          true when timeline second `sec` is cut out of the result
 *   outRange(from, to)      a timeline range in seconds of the finished clip ({ from, to }), or null when none of it is kept
 *   picture()               { width, height } of the result's picture
 *   textWidth(text, size)   a line's width as a fraction of the picture's width (`size`: its height as a fraction of the picture's)
 *   texts(tag)              the text items on the picture that carry `tag`, in time order
 *   select(id)              select an item on the picture (null: none)
 * A text item: { id, kind: "text", from, to, x, y, text, color, size, outline, align: "center" (x is its middle), tag }.
 */
function plugEditorHandle(root, c, P, redraw) {
  const pic = () => {
    const cs = P.cv?.size?.(), v = root.querySelector(".player video");
    return cs ? { width: cs.width, height: cs.height } : { width: (c.virtual ? c.width : v?.videoWidth) || c.width || 1920, height: (c.virtual ? c.height : v?.videoHeight) || c.height || 1080 };
  };
  return {
    clip: { id: c.id, title: c.title, path: c.path || "", width: c.width, height: c.height, duration: c.duration, virtual: !!c.virtual },
    get model() { return P.model; },
    get editing() { return !!P.editing; },
    time: () => P.time(),
    seek: (sec) => P.seek(sec, false, true),
    change: (fn) => P.edit("overlay", fn),
    redraw,
    footage() {
      const m = P.model;
      if (!m) return [];
      const VS = P.sources?.();
      const own = (m.tracksAll && m.tracksAll.length ? m.tracksAll : m.audio.filter((r) => r.kind === "track")).map((x) => ({ index: x.index, title: x.title || "" }));
      return emSrcSpans(m, 0, m.duration).filter((sp) => !sp.gap).map((sp) => {
        const speed = sp.speed || 1;
        if (!VS) return { at: sp.tl, len: sp.len, speed, path: c.path || "", from: sp.src, tracks: own, missing: !c.path };
        const k = emSourceAt(VS, sp.src + 1e-4), src = VS[k] || {};
        return { at: sp.tl, len: sp.len, speed, path: src.path || "", from: Math.max(0, sp.src - (src.start || 0)), tracks: (src.tracksAll || src.tracks || []).map((x) => ({ index: x.index, title: x.title || "" })), missing: !!src.missing || !src.path };
      });
    },
    removedAt(sec) { const m = P.model; return !!m && !!m.removed[emSegmentAt(m, sec)]; },
    outRange(from, to) { const m = P.model; return m ? emOutRange(m, from, to) : null; },
    picture: pic,
    textWidth(text, size) { const { width, height } = pic(); return ovMeasure(String(text), size * height) / width; },
    texts(tag) { return (P.model?.overlays || []).filter((o) => o.kind === "text" && o.tag === tag).sort((a, b) => a.from - b.from); },
    select: (id) => P.ov?.select?.(id),
  };
}

/** The editor opened (ui/editor.js, openClip): every plugin with an `editor` gets its section. Returns { sync(what), destroy() } or null. */
function plugEditorMount(root, c, P) {
  const pane = root.querySelector(".v-edit"), before = pane?.querySelector(".e-export");
  const list = [...PLUG.defs].filter(([, p]) => p.def.editor);
  if (!pane || !list.length) return null;
  const mine = { handle: null, dead: false };
  const parts = list.map(([id, p]) => {
    const sec = document.createElement("section");
    sec.className = "e-sec e-plug";
    sec.dataset.plugin = id;
    // Its tab of the inspector (workspace.js): "clip" (the default), "color" or "overlays", where it comes first.
    const tab = ["color", "overlays"].includes(p.def.editor.tab) ? p.def.editor.tab : "clip";
    sec.dataset.tab = tab;
    pane.insertBefore(sec, (tab === "overlays" && pane.querySelector(":scope > .e-ov")) || before);
    const draw = () => {
      if (mine.dead) return;
      // A text box being typed in keeps its place: the section is not redrawn under the caret.
      if (sec.contains(document.activeElement) && document.activeElement.matches("input[type=text], textarea")) { sec._late = true; return; }
      sec._late = false;
      const top = sec.querySelector("[data-keep-scroll]")?.scrollTop || 0;
      sec.innerHTML = `<div class="e-title">${esc(t(p.def.editor.title || p.info.name))}</div>` + plugSafe(id, "editor", () => p.def.editor.render(ed, p.ctx) || "", "");
      const sc = sec.querySelector("[data-keep-scroll]");
      if (sc) sc.scrollTop = top;
    };
    const ed = plugEditorHandle(root, c, P, draw);
    sec.addEventListener("focusout", () => setTimeout(() => { if (sec._late) draw(); }, 0));
    return { id, p, sec, ed, draw };
  });
  mine.handle = parts[0]?.ed || null;
  mine.byId = Object.fromEntries(parts.map((x) => [x.id, x.ed]));
  PLUG.ed = mine;
  for (const x of parts) { x.draw(); if (x.p.def.editor.onOpen) plugSafe(x.id, "editor.onOpen", () => x.p.def.editor.onOpen(x.ed, x.p.ctx)); }
  return {
    /** After any change of the model (P.edSync), and when the editor's tools are shown or hidden ("edit"). */
    sync(what) { for (const x of parts) { if (x.p.def.editor.onSync) plugSafe(x.id, "editor.onSync", () => x.p.def.editor.onSync(x.ed, what, x.p.ctx)); x.draw(); } },
    destroy() {
      for (const x of parts) { if (x.p.def.editor.onClose) plugSafe(x.id, "editor.onClose", () => x.p.def.editor.onClose(x.ed, x.p.ctx)); x.sec.remove(); }
      mine.dead = true;
      if (PLUG.ed === mine) PLUG.ed = null;
    },
  };
}

/* ---------- the marketplace page ---------- */

const plugInstalled = (id) => (PLUG.state?.installed || []).find((x) => x.id === id) || null;
const plugVer = (v) => String(v || "").split(".").map((n) => parseInt(n, 10) || 0);
const plugNewer = (a, b) => { const x = plugVer(a), y = plugVer(b); for (let i = 0; i < Math.max(x.length, y.length); i++) { if ((x[i] || 0) !== (y[i] || 0)) return (x[i] || 0) > (y[i] || 0); } return false; };
/** Marketplace entries that are newer than the copy installed. */
const plugUpdates = () => (PLUG.market?.plugins || []).filter((m) => { const i = plugInstalled(m.id); return i && i.official && plugNewer(m.version, i.version); });

async function plugRefresh(market = false) {
  try { PLUG.state = await invoke("plugins_state"); } catch (e) { console.warn("Glint: plugins_state failed", e); }
  if (market && !PLUG.marketBusy) {
    PLUG.marketBusy = true;
    if (S.page === "plugins") render();
    try { PLUG.market = await invoke("plugins_market"); } catch (e) { PLUG.market = { plugins: PLUG.market?.plugins || [], error: String(e?.message ?? e), cached: true }; }
    PLUG.marketBusy = false;
  }
  if (S.page === "plugins") render(); else renderChrome();
}

function plugCard(m, inst) {
  const id = m ? m.id : inst.id;
  const src = m || inst;
  const official = !!m || !!inst?.official;
  const pct = PLUG.busy[id];
  const update = m && inst && inst.official && plugNewer(m.version, inst.version);
  const problem = inst?.problem || (m?.too_new ? t("Needs Glint {version} or newer", { version: m.min_glint }) : "");
  const icon = m?.icon ? `<svg viewBox="0 0 24 24"><path d="${esc(m.icon)}"/></svg>` : ICON[Object.keys(ICON).find((k) => k.startsWith(`p:${id}:`))] || ICON.plug;
  const by = official
    ? `<span class="plug-by official" title="${esc(t("Published by the people who make Glint, and checked against the marketplace's checksum when it is installed"))}">${ICON.shield}${esc(src.author || t("Glint developers"))} · ${t("Official")}</span>`
    : `<span class="plug-by" title="${esc(t("This plugin was put in the plugins folder by hand. Glint has not checked it."))}">${esc(src.author || t("Unknown author"))} · ${t("Not from the marketplace")}</span>`;
  let actions;
  if (pct !== undefined) actions = `<div class="plug-bar"><i style="width:${Math.round(pct)}%"></i></div><span class="small muted">${t("Installing…")}</span>`;
  else if (!inst) actions = `<button class="btn primary" data-pm="install" data-id="${esc(id)}"${m.too_new ? " disabled" : ""}>${ICON.plus}${t("Install")}</button>`;
  else {
    actions = `${update ? `<button class="btn primary sm" data-pm="install" data-id="${esc(id)}">${t("Update to {version}", { version: esc(m.version) })}</button>` : ""}
      <button class="btn sm danger" data-pm="remove" data-id="${esc(id)}">${ICON.trash}${t("Remove")}</button>
      <label class="switch" title="${esc(inst.enabled ? t("On. Switch off to keep it installed without loading it") : t("Off"))}"><input type="checkbox" data-pm-toggle="${esc(id)}" ${inst.enabled ? "checked" : ""} aria-label="${esc(t("Switch {name} on or off", { name: src.name }))}"><i></i></label>`;
  }
  return `<article class="plug-card${inst && !inst.enabled ? " off" : ""}">
    <div class="plug-icon">${icon}</div>
    <div class="plug-main">
      <h3>${esc(t(src.name))}${m?.tag ? `<span class="plug-tag">${esc(t(m.tag))}</span>` : ""}${inst ? `<span class="plug-tag on">${inst.enabled ? t("Installed") : t("Installed, off")}</span>` : ""}</h3>
      ${by}
      <p>${esc(t(src.description || ""))}</p>
      ${problem ? `<p class="plug-problem">${ICON.warn}${esc(problem)}</p>` : ""}
      ${PLUG.failed[id] ? `<p class="plug-problem">${ICON.warn}${esc(t("Its script did not load: {error}", { error: PLUG.failed[id] }))}</p>` : ""}
      <div class="plug-meta">${t("Version {version}", { version: esc(inst ? inst.version : m.version) })}${m?.size ? ` · ${fmtSize(m.size)}` : ""}</div>
    </div>
    <div class="plug-actions">${actions}</div>
  </article>`;
}

function renderPluginsPage() {
  if (!PLUG.market && !PLUG.marketBusy) plugRefresh(true); // the list is asked for when the page is first opened, never before
  const market = PLUG.market?.plugins || [];
  const installed = PLUG.state?.installed || [];
  const byHand = installed.filter((i) => !market.some((m) => m.id === i.id));
  const head = pageHead(t("Plugins"), t("Extra tools you add to Glint. Install only the ones you use")).replace(/<\/div>$/, () => `<div class="head-actions"><button class="btn" data-pm="refresh"${PLUG.marketBusy ? " disabled" : ""}>${ICON.refresh}${t("Refresh")}</button></div></div>`);
  $("#view").innerHTML = head + `
    ${PLUG.state?.restart ? `<div class="note plug-restart"><span><b>${t("Restart Glint to finish.")}</b> ${t("Plugins you installed, removed or switched take effect when Glint starts again. Clipping carries on as it is.")}</span><button class="btn primary sm" data-pm="restart">${ICON.refresh}${t("Restart Glint")}</button></div>` : ""}
    ${PLUG.market?.error ? `<div class="note bad"><b>${t("The marketplace could not be reached.")}</b> ${esc(t(PLUG.market.error))}${market.length ? " " + t("Showing the list as it was last time.") : ""}</div>` : ""}
    <div class="plug-label">${t("Marketplace")}</div>
    ${market.length ? `<div class="plug-grid">${market.map((m) => plugCard(m, plugInstalled(m.id))).join("")}</div>`
      : `<div class="plug-empty">${PLUG.marketBusy || !PLUG.market ? t("Loading the marketplace…") : PLUG.market.error ? t("Check the internet connection, then press Refresh.") : t("No plugins are published yet.")}</div>`}
    ${byHand.length ? `<div class="plug-label">${t("Installed by hand")}</div><div class="plug-grid">${byHand.map((i) => plugCard(null, i)).join("")}</div>` : ""}
    <p class="plug-foot">${t("Official plugins are made by Glint's developers. A plugin is downloaded from Glint's own page on GitHub and checked before it is installed. Nothing about you or this PC is sent.")}</p>`;
}

async function plugInstall(id) {
  if (PLUG.busy[id] !== undefined) return;
  const m = (PLUG.market?.plugins || []).find((x) => x.id === id);
  PLUG.busy[id] = 1;
  render();
  try {
    PLUG.state = await invoke("plugin_install", { id });
    sfx("success");
    flash(t("{name} is installed. Restart Glint to start using it.", { name: m?.name || id }));
  } catch (e) { fail(e); }
  delete PLUG.busy[id];
  await plugRefresh();
}

document.addEventListener("click", async (e) => {
  const pm = e.target.closest("[data-pm]");
  if (pm) {
    const id = pm.dataset.id || "";
    try {
      switch (pm.dataset.pm) {
        case "refresh": return await plugRefresh(true);
        case "install": return await plugInstall(id);
        case "remove": {
          const name = plugInstalled(id)?.name || id;
          if (!(await ask(t("Remove {name}? Its settings are kept in case you install it again.", { name }), t("Remove")))) return;
          PLUG.state = await invoke("plugin_uninstall", { id });
          return render();
        }
        case "restart":
          try { localStorage.setItem("glint.pageAfterRestart", "plugins"); } catch {}
          return await invoke("plugins_restart");
      }
    } catch (err) { fail(err); }
    return;
  }
  // A click inside a plugin's own page or settings section: its onAct.
  const act = e.target.closest("[data-pact]");
  const host = act?.closest("[data-plugin]");
  const p = host && PLUG.defs.get(host.dataset.plugin);
  if (p?.def.onAct) plugSafe(p.info.id, "onAct", () => p.def.onAct(act.dataset.pact, act, p.ctx));
});

document.addEventListener("change", async (e) => {
  const id = e.target.dataset?.pmToggle;
  if (!id) return;
  try { PLUG.state = await invoke("plugin_set_enabled", { id, on: e.target.checked }); } catch (err) { fail(err); }
  render();
});

/* ---------- start ---------- */

(async function plugBoot() {
  let list = [];
  try { list = (await invoke("plugins_boot")) || []; } catch (e) { console.warn("Glint: plugins_boot failed", e); }
  for (let i = 0; i < 400 && !S.settings; i++) await new Promise((r) => setTimeout(r, 25)); // app.js reads the settings first
  for (const info of list) plugLoad(info);
  plugEnsureSettings();
  PLUG.ready = true;
  T.event.listen("plugin-progress", ({ payload: p }) => {
    if (!p || PLUG.busy[p.id] === undefined) return;
    PLUG.busy[p.id] = p.pct;
    if (S.page === "plugins") render();
  });
  // A keybind with a plugin's action fired (the backend has already started or stopped its process, if it names one).
  T.event.listen("plugin-action", ({ payload: a }) => {
    const p = a && PLUG.defs.get(a.plugin);
    const fn = p?.def.actions?.[a.action];
    if (fn) plugSafe(a.plugin, "action", () => fn(p.ctx));
  });
  // Files dragged from Explorer onto Glint's window go to the plugin whose page is open (its onDrop).
  for (const ev of ["tauri://drag-enter", "tauri://drag-leave", "tauri://drag-drop"]) T.event.listen(ev, ({ payload }) => plugDrop(ev, payload || {}));
  T.event.listen("plugin-download", ({ payload: d }) => {
    const p = d && PLUG.defs.get(d.plugin);
    if (p?.def.onDownload) plugSafe(d.plugin, "onDownload", () => p.def.onDownload(d, p.ctx));
  });
  T.event.listen("plugin-proc", ({ payload: st }) => {
    const p = st && PLUG.defs.get(st.plugin);
    if (p?.def.onProc) plugSafe(st.plugin, "onProc", () => p.def.onProc(st, p.ctx));
  });
  for (const [id, p] of PLUG.defs) if (p.def.start) plugSafe(id, "start", () => p.def.start(p.ctx));
  let after = "";
  try { after = localStorage.getItem("glint.pageAfterRestart") || ""; localStorage.removeItem("glint.pageAfterRestart"); } catch {}
  await plugRefresh();
  if (after) goPage(after); else render();
})();
