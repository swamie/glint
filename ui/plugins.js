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
//   settings   [{ title, cat, render(ctx) }]: sections of the Settings page (cat: one of its categories, or "plugins").
//   actions    { <action id>(ctx) }: run when a keybind with that action fires while the window is open.
//              The actions themselves (id, label, the process they start) are listed in plugin.json,
//              so a keybind also works while the window is closed.
//   topbar(ctx)            markup for the top bar ("" for nothing): a short status such as "STREAMING: 1080p @ 60".
//                          Build it from Glint's own top bar buttons: .btn, joined with .top-group; .top-dot is a small accent dot.
//   onAct(name, el, ctx)   a click on an element with data-pact="name" inside its page or sections.
//   onSave(ctx)            just before the settings are saved: the place to work out derived values.
//   onProc(status, ctx)    one of its processes started or ended.
//
// `ctx` (also `Glint.ctx`): { id, s (its settings), key(name) (the data-set path of one), save(rerender),
// row, toggle, select, number, text, t, esc, invoke, flash, fail, render, chrome (redraw the top bar and sidebar only),
// go(page), glint (Glint's own state),
// proc: { start(name, args, { audio }), stop(name), status(name) } }.

const PLUG = {
  defs: new Map(),   // plugin id -> { info (from plugins_boot), def (what it registered), ctx }
  failed: {},        // plugin id -> why its script did not load
  state: null,       // plugins_state: { installed: [...], restart }
  market: null,      // plugins_market: { plugins: [...], error, cached }
  marketBusy: false,
  busy: {},          // plugin id -> percent of a running install
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
    proc: {
      start: (name, args, opts = {}) => invoke("plugin_proc_start", { plugin: id, name, args, audio: !!opts.audio }),
      stop: (name) => invoke("plugin_proc_stop", { plugin: id, name }),
      status: (name) => invoke("plugin_proc_status", { plugin: id, name }),
    },
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
