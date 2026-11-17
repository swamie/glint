// Glint plugin "claude-mcp": the part that runs inside Glint's window.
//
// Glint loads this file only when the plugin is installed (src-tauri/src/plugins.rs). It answers the
// "plugin-rpc" requests that glint-mcp.exe sends through Glint's local API, and makes every edit with the
// editor's own model functions (ui/editmodel.js) through P.edit, the same path the timeline's buttons take:
// each change is one undo step, the project saves itself, and the person sees it happen on the timeline.
//
// Times: "timeline" seconds are positions on V1 of the open edit. "clip" seconds are positions inside one
// source clip's own file (what a transcript or a frame grab of that file uses).
/* global S, invoke, openClip, closeClip, render, flash, edExport, edReplaceOn, edSetReplace, projById, projOpen, projUpsert,
   projRefreshUI, projAddTo, projKeepClip, emPieceList, emPieceAt, emSplitAt, emRemovePiece, emTrimPiece, emPutBeside,
   emSetPieceSpeed, emGroups, emNormGroups, emLayGroups, emPiecify, emEnsureParts, emKeepSource, emNoColor, emUndo, emRedo,
   emResultDuration, emGapStats, emCloseGaps, emSourceAt, emProjToFile, emFootLen, emPartSp, EM_MIN_SEG, EM_EPS,
   emMoveClip, emTrimClip, emCloseGap, emSetGapLen, emSplitClip, emRemoveClip, emClipList, emSetColor, emSegmentAt */
"use strict";
(() => {
  if (window.__glintClaudeMcp) return;
  window.__glintClaudeMcp = true;
  const PREFIX = "claude.";
  const T = window.__TAURI__;
  if (!T || !T.event || !T.core) return;
  const call = (cmd, args) => T.core.invoke(cmd, args);
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const r3 = (x) => Math.round(x * 1000) / 1000;

  async function until(test, ms, what) {
    const t0 = Date.now();
    for (;;) {
      const v = test();
      if (v) return v;
      if (Date.now() - t0 > ms) throw new Error(`Timed out waiting for ${what}`);
      await sleep(100);
    }
  }
  const appReady = () => until(() => typeof S !== "undefined" && Array.isArray(S.clips), 20000, "Glint to load its library");

  /* ---------- what is open ---------- */

  function player() {
    const P = typeof S !== "undefined" ? S.player : null;
    return P && (!P.alive || P.alive()) ? P : null;
  }
  async function editor() {
    const P = player();
    if (!P) throw new Error("Nothing is open in the editor. Use open_in_editor or create_project first.");
    if (!P.editing) await P.setEdit(true);
    const m = await until(() => P.model, 30000, "the editor to load");
    return { P, m };
  }
  /** The open edit's source clips: [{ k, clip, title, path, start, duration }] (start = where it begins in the model's source time). */
  function sources(P, m) {
    const VS = typeof P.sources === "function" ? P.sources() : null;
    if (VS && VS.length) return VS.map((s, k) => ({ k, clip: s.clip || "", title: s.title || `Clip ${k + 1}`, path: s.path || "", start: +s.start || 0, duration: +s.duration || 0, missing: !!s.missing }));
    const c = P.clipMeta || {};
    const len = Number.isFinite(m.source) ? m.source : c.duration || m.duration;
    return [{ k: 0, clip: c.id || S.open || "", title: c.title || "Clip", path: c.path || "", start: 0, duration: len, missing: false }];
  }
  function srcIndex(list, p) {
    if (list.length === 1) return 0;
    return emSourceAt(list, p.src + emFootLen(p) / 2);
  }
  function describe(P, m) {
    const list = sources(P, m);
    const pieces = emPieceList(m).map((p) => {
      if (p.gap) return { index: p.i, gap: true, start: r3(p.at), end: r3(p.at + p.len), length: r3(p.len) };
      const k = srcIndex(list, p), s = list[k];
      return {
        index: p.i, start: r3(p.at), end: r3(p.at + p.len), length: r3(p.len),
        source: k, clip_id: s.clip, clip_title: s.title,
        clip_from: r3(p.src - s.start), clip_to: r3(p.src - s.start + p.flen),
        ...(p.speed !== 1 ? { speed: p.speed } : {}),
      };
    });
    return {
      open: { id: S.open, title: P.clipMeta?.title || "", project: !!(typeof projById === "function" && projById(S.open)) },
      timeline_length: r3(m.duration),
      result_length: r3(emResultDuration(m)),
      overall_speed: m.speed || 1,
      sources: list.map((s) => ({ index: s.k, clip_id: s.clip, title: s.title, path: s.path, length: r3(s.duration), ...(s.missing ? { missing: true } : {}) })),
      pieces,
      audio_rows: (m.audio || []).filter((r) => r.kind === "track").map((r) => ({ id: r.id, title: r.title, volume: r.volume, muted: !!r.muted }))
        .concat((m.lanes || []).map((L) => ({ id: L.id, title: L.title, added: true, volume: L.volume ?? 1, muted: !!L.muted }))),
      audio_clips: emClipList(m).map((c) => { const r = m.audio.find((x) => x.id === c.id) || {}; return { id: c.id, row: c.lane, title: c.title, path: c.path, start: r3(c.at), end: r3(c.at + c.len), file_from: r3(r.trimStart || 0), volume: r.volume, muted: !!r.muted }; }),
      overlays: (m.overlays || []).map((o) => ({ id: o.id, kind: o.kind, from: r3(o.from), to: r3(o.to), ...(o.kind === "text" ? { text: o.text } : {}) })),
      fade_in: m.fadeIn || 0,
      fade_out: m.fadeOut || 0,
      can_undo: m.undo.length > 0,
      can_redo: m.redo.length > 0,
    };
  }
  /** Run one change as one undo step; throws `why` when it changed nothing. */
  function change(P, what, fn, why) {
    let out;
    const ok = P.edit(what, (m) => { out = fn(m); });
    if (!ok) throw new Error(why || "That changed nothing");
    P.tl?.select?.(null);
    return out;
  }
  const num = (x, name) => { const v = +x; if (!Number.isFinite(v)) throw new Error(`${name} must be a number of seconds`); return v; };
  function pieceIdx(m, i) {
    const L = emPieceList(m), k = Math.trunc(+i);
    if (!(k >= 0 && k < L.length)) throw new Error(`There is no piece ${i}. The timeline has pieces 0 to ${L.length - 1}.`);
    return k;
  }

  /* ---------- library ---------- */

  const clipOut = (c) => ({ id: c.id, title: c.title, path: c.path, length: r3(c.duration || 0), width: c.width, height: c.height, fps: c.fps, game: c.game || "", folder: c.folder || "", created: c.created ? new Date(c.created).toISOString() : "", audio_tracks: c.audio_tracks || 0 });

  const methods = {
    async state() {
      await appReady();
      const P = player();
      return {
        open: S.open || null,
        editing: !!P?.editing,
        timeline: P?.model ? describe(P, P.model) : null,
      };
    },

    async tools() {
      const r = await call("check_deps");
      const items = Array.isArray(r?.items) ? r.items : [];
      const path = (id) => items.find((x) => x.id === id)?.path || "";
      return { ffmpeg: path("ffmpeg"), ffprobe: path("ffprobe") };
    },

    async clips({ query = "", limit = 50 } = {}) {
      await appReady();
      const q = String(query).toLowerCase();
      const list = S.clips.filter((c) => !c.draft && (!q || `${c.title} ${c.game} ${c.folder} ${c.path}`.toLowerCase().includes(q)));
      list.sort((a, b) => (b.created || 0) - (a.created || 0));
      return { total: list.length, clips: list.slice(0, Math.max(1, Math.min(500, +limit || 50))).map(clipOut) };
    },

    async projects() {
      await appReady();
      return (S.projects || []).map((p) => ({ id: p.id, name: p.name, clips: (p.clips || []).map((c) => ({ id: c.id, title: c.title, length: r3(c.duration || 0) })) }));
    },

    async import({ paths, copy = false }) {
      await appReady();
      const list = (Array.isArray(paths) ? paths : [paths]).filter((p) => typeof p === "string" && p);
      if (!list.length) throw new Error("Give at least one file path");
      const res = await call("import_files", { paths: list, copy: !!copy, folder: null });
      const out = [];
      for (const d of Array.isArray(res) ? res : []) {
        if (d?.clip?.id) {
          if (typeof projKeepClip === "function") projKeepClip(d.clip);
          else if (!S.clips.some((c) => c.id === d.clip.id)) S.clips.push(d.clip);
          out.push(clipOut(d.clip));
        }
      }
      if (typeof render === "function") render();
      return { imported: out, asked: list.length };
    },

    async create_project({ name, clip_ids }) {
      await appReady();
      const ids = (Array.isArray(clip_ids) ? clip_ids : []).filter(Boolean);
      const p = await call("create_project", { name: String(name || "Claude edit") });
      if (!p?.id) throw new Error("Glint did not make the project");
      if (ids.length) {
        const res = await call("project_add_clips", { id: p.id, ids });
        if (res?.project) projUpsert(res.project); else projUpsert(p);
      } else projUpsert(p);
      if (typeof projRefreshUI === "function") projRefreshUI();
      if (ids.length) return methods.open({ id: p.id });
      return { project: p.id, note: "The project is empty. Add clips with add_clips after opening it." };
    },

    async open({ id }) {
      await appReady();
      if (!id) throw new Error("Give a clip or project id");
      const proj = typeof projById === "function" ? projById(id) : null;
      if (proj) projOpen(id);
      else if (S.clips.some((c) => c.id === id)) openClip(id);
      else throw new Error(`No clip or project has the id ${id}`);
      const P = await until(() => S.open === id && player(), 30000, "the editor to open");
      await P.setEdit(true);
      await until(() => P.model, 30000, "the editor to load");
      return describe(P, P.model);
    },

    async close() {
      if (typeof S !== "undefined" && S.open) closeClip();
      return { closed: true };
    },

    async add_clips({ clip_ids, at }) {
      const { P } = await editor();
      const ids = (Array.isArray(clip_ids) ? clip_ids : [clip_ids]).filter(Boolean);
      if (!ids.length) throw new Error("Give at least one clip id");
      const id = S.open, before = (P.sources?.() || []).length;
      await projAddTo(id, ids, Number.isFinite(+at) && at !== null && at !== undefined ? +at : undefined);
      // A single clip becomes a new project (it opens again); a project grows in place.
      const N = await until(() => { const Q = player(); return Q && Q.model && ((Q.sources?.() || []).length > before || Q !== P) ? Q : null; }, 60000, "the clips to be added");
      if (!N.editing) await N.setEdit(true);
      return describe(N, N.model);
    },

    async timeline() {
      const { P, m } = await editor();
      return describe(P, m);
    },

    async split({ times }) {
      const { P } = await editor();
      const list = (Array.isArray(times) ? times : [times]).map((x) => num(x, "time")).sort((a, b) => b - a);
      change(P, "split", (m) => { let any = false; for (const at of list) any = emSplitAt(m, at) || any; return any; }, "No split was made (a time must be inside a piece and at least 0.1 s from its ends)");
      return describe(P, P.model);
    },

    async delete({ pieces, leave_gap = false }) {
      const { P, m } = await editor();
      const idx = [...new Set((Array.isArray(pieces) ? pieces : [pieces]).map((i) => pieceIdx(m, i)))].sort((a, b) => b - a);
      // Indexes shift as pieces go: remove from the last one back, by their start times.
      const L = emPieceList(m), ats = idx.map((i) => L[i].at + L[i].len / 2);
      change(P, "order", (mm) => { let any = false; for (const at of ats) any = emRemovePiece(mm, emPieceAt(mm, at), !!leave_gap) || any; return any; }, "Nothing was deleted (the last piece with video cannot be deleted)");
      return describe(P, P.model);
    },

    async trim({ piece, start, end, leave_gap = false }) {
      const { P, m } = await editor();
      const i = pieceIdx(m, piece);
      if (start === undefined && end === undefined) throw new Error("Give a new start, a new end, or both (timeline seconds)");
      if (emPieceList(m)[i].gap) throw new Error(`Piece ${i} is a gap; use close_gaps`);
      const gapLen = (L, j) => (L[j] && L[j].gap ? L[j].len : 0);
      // Glint leaves an empty stretch where a trim took footage away; unless leave_gap, the stretch closes up again.
      const restore = (mm, j, old) => { const now = gapLen(emPieceList(mm), j); if (now > old + 1e-6) { if (old < 0.01) emCloseGap(mm, j); else emSetGapLen(mm, j, old); } };
      change(P, "order", (mm) => {
        let k = i, any = false;
        if (end !== undefined && end !== null) {
          const old = gapLen(emPieceList(mm), k + 1);
          any = !!emTrimPiece(mm, k, "end", num(end, "end")) || any; // the piece keeps its index (a gap goes after it)
          if (!leave_gap) restore(mm, k + 1, old);
        }
        if (start !== undefined && start !== null) {
          const old = gapLen(emPieceList(mm), k - 1);
          any = !!emTrimPiece(mm, k, "start", num(start, "start")) || any;
          if (emPieceList(mm)[k]?.gap) k += 1; // a new gap went in before it
          if (!leave_gap) restore(mm, k - 1, old);
        }
        return any;
      }, "The trim changed nothing");
      return describe(P, P.model);
    },

    async move({ piece, to, after = false }) {
      const { P, m } = await editor();
      const i = pieceIdx(m, piece), k = pieceIdx(m, to);
      change(P, "order", (mm) => emPutBeside(mm, i, k, !!after), "The piece did not move (gaps cannot be moved this way; close them first)");
      return describe(P, P.model);
    },

    async close_gaps() {
      const { P } = await editor();
      change(P, "gap", (m) => emCloseGaps(m), "There are no gaps");
      return describe(P, P.model);
    },

    async speed({ piece, speed }) {
      const { P, m } = await editor();
      const i = pieceIdx(m, piece);
      change(P, "order", (mm) => emSetPieceSpeed(mm, i, num(speed, "speed")), "The speed did not change (0.25 to 4)");
      return describe(P, P.model);
    },

    /** Lay V1 out anew from a list of clip ranges: [{ source | clip_id, from, to }] in clip seconds. One undo step. */
    async assemble({ segments }) {
      const { P, m } = await editor();
      const list = sources(P, m);
      const segs = (Array.isArray(segments) ? segments : []).map((x, n) => {
        let k = Number.isInteger(x.source) ? x.source : list.findIndex((s) => s.clip === x.clip_id);
        if (!(k >= 0 && k < list.length)) throw new Error(`Segment ${n}: that clip is not in the open edit. Add it with add_clips first.`);
        const s = list[k], from = Math.max(0, num(x.from, "from")), to = Math.min(s.duration, num(x.to, "to"));
        if (!(to - from >= EM_MIN_SEG)) throw new Error(`Segment ${n}: "to" must be after "from" (inside 0 to ${r3(s.duration)} s of ${s.title})`);
        return { k, from, to, title: s.title };
      });
      if (!segs.length) throw new Error("Give at least one segment");
      change(P, "order", (mm) => {
        emPiecify(mm);
        emEnsureParts(mm);
        emKeepSource(mm);
        const groups = emGroups(mm);
        const order = segs.map((x) => ({
          p: null,
          part: { src: list[x.k].start + x.from, len: x.to - x.from, title: x.title },
          cuts: [],
          segs: [{ removed: false, color: emNoColor(), blank: false, gains: mm.audio.map(() => 1) }],
          marks: [],
        }));
        emLayGroups(mm, groups, emNormGroups(mm, order));
        return true;
      }, "The timeline already looks like that");
      return describe(P, P.model);
    },

    async volume({ row, volume, muted }) {
      const { P, m } = await editor();
      const r = (m.audio || []).find((x) => x.id === row || x.title === row);
      if (!r) throw new Error(`No audio row ${row}. Rows: ${(m.audio || []).map((x) => `${x.id} (${x.title})`).join(", ")}`);
      change(P, "audio", (mm) => {
        const rr = mm.audio.find((x) => x.id === r.id);
        if (volume !== undefined && volume !== null) rr.volume = Math.max(0, Math.min(2, num(volume, "volume")));
        if (muted !== undefined && muted !== null) rr.muted = !!muted;
      }, "The row already had that level");
      return describe(P, P.model);
    },

    async undo() {
      const { P, m } = await editor();
      if (!emUndo(m)) throw new Error("Nothing to undo");
      P.refreshTimeline?.();
      P.edSync?.("undo");
      return describe(P, P.model);
    },

    async redo() {
      const { P, m } = await editor();
      if (!emRedo(m)) throw new Error("Nothing to redo");
      P.refreshTimeline?.();
      P.edSync?.("redo");
      return describe(P, P.model);
    },

    async seek({ time }) {
      const { P } = await editor();
      P.seek(num(time, "time"), false, true);
      return { time: num(time, "time") };
    },

    /** Timeline seconds -> which file and which second of it is seen there. */
    async resolve({ times }) {
      const { P, m } = await editor();
      const list = sources(P, m), L = emPieceList(m);
      return (Array.isArray(times) ? times : [times]).map((x) => {
        const t = num(x, "time"), i = emPieceAt(m, Math.min(Math.max(0, t), Math.max(0, m.duration - 1e-3)));
        const p = L[i];
        if (!p || p.gap) return { time: t, gap: true };
        const s = list[srcIndex(list, p)], src = p.src + Math.max(0, t - p.at) * (p.speed || 1);
        return { time: t, piece: i, clip_id: s.clip, path: s.path, clip_time: r3(src - s.start) };
      });
    },

    /* ----- audio files (songs, voice messages, sound effects) ----- */

    async add_audio({ path, at, row }) {
      const { P } = await editor();
      if (!path) throw new Error("Give the audio file's path");
      const info = await call("editor_audio_info", { path: String(path) });
      if (!info || !(+info.duration > 0)) throw new Error(`${path} has no audio`);
      const r = P.addFileRow(info, Number.isFinite(+at) && at !== null && at !== undefined ? Math.max(0, +at) : 0);
      if (row && emMoveClip(P.model, r.id, { lane: row })) { P.refreshTimeline(); P.edSync("file"); }
      return { added: r.id, length: r3(info.duration), timeline: describe(P, P.model) };
    },

    async audio_clip({ id, at, row, start, end, split, volume, muted, remove = false }) {
      const { P, m } = await editor();
      if (!(m.audio || []).some((r) => r.id === id && r.kind === "file")) throw new Error(`No audio clip ${id}. See audio_clips in the timeline.`);
      change(P, "file", (mm) => {
        if (remove) return emRemoveClip(mm, id);
        if (at !== undefined || row !== undefined) emMoveClip(mm, id, { ...(at !== undefined ? { at: num(at, "at") } : {}), ...(row !== undefined ? { lane: row } : {}) });
        if (start !== undefined) emTrimClip(mm, id, "start", num(start, "start"));
        if (end !== undefined) emTrimClip(mm, id, "end", num(end, "end"));
        if (split !== undefined) emSplitClip(mm, id, num(split, "split"));
        const r = mm.audio.find((x) => x.id === id);
        if (r && volume !== undefined) r.volume = Math.max(0, Math.min(2, num(volume, "volume")));
        if (r && muted !== undefined) r.muted = !!muted;
      }, "That changed nothing");
      return describe(P, P.model);
    },

    /* ----- text, fades, color ----- */

    async add_text({ text, from, to, x = 0.5, y = 0.85, size = 7, color = "#ffffff", outline = true }) {
      const { P, m } = await editor();
      if (!text) throw new Error("Give the text");
      const a = from === undefined ? 0 : num(from, "from"), b = to === undefined ? m.duration : num(to, "to");
      if (!(b > a)) throw new Error('"to" must be after "from"');
      const id = "o" + Date.now().toString(36) + Math.random().toString(36).slice(2, 7);
      const item = { id, kind: "text", from: a, to: b, x: Math.min(1, Math.max(0, +x)), y: Math.min(1, Math.max(0, +y)), text: String(text), color: /^#[0-9a-f]{6}$/i.test(color) ? color : "#ffffff", size: Math.min(20, Math.max(2, +size || 7)) / 100, outline: !!outline };
      change(P, "overlay", (mm) => { mm.overlays = [...(mm.overlays || []), item]; });
      return { added: id, timeline: describe(P, P.model) };
    },

    async remove_overlay({ id }) {
      const { P } = await editor();
      change(P, "overlay", (mm) => { const n = (mm.overlays || []).length; mm.overlays = (mm.overlays || []).filter((o) => o.id !== id); return mm.overlays.length !== n; }, `No overlay ${id}`);
      return describe(P, P.model);
    },

    async fade({ fade_in, fade_out }) {
      const { P } = await editor();
      change(P, "fade", (mm) => {
        if (fade_in !== undefined) mm.fadeIn = Math.max(0, Math.min(10, num(fade_in, "fade_in")));
        if (fade_out !== undefined) mm.fadeOut = Math.max(0, Math.min(10, num(fade_out, "fade_out")));
      }, "The fades were already like that");
      return describe(P, P.model);
    },

    async color({ pieces, ...values }) {
      const { P, m } = await editor();
      let segs = null;
      if (Array.isArray(pieces) && pieces.length) {
        const L = emPieceList(m);
        segs = pieces.map((i) => { const p = L[pieceIdx(m, i)]; return emSegmentAt(m, p.at + p.len / 2); });
      }
      change(P, "color", (mm) => emSetColor(mm, segs, values), "The color did not change");
      return describe(P, P.model);
    },

    async export({ close_gaps = false } = {}) {
      const { P, m } = await editor();
      if (emGapStats(m).count) {
        if (!close_gaps) throw new Error("The timeline has gaps (black, silent stretches). Close them with close_gaps, or export with close_gaps true.");
        change(P, "gap", (mm) => emCloseGaps(mm));
      }
      const id = S.open, before = new Set(S.clips.map((c) => c.id));
      // "Replace the original" would ask the person in a dialog: off for this export, then as it was.
      const rep = typeof edReplaceOn === "function" && edReplaceOn();
      if (rep) edSetReplace(false);
      try { await edExport(false); } finally { if (rep) edSetReplace(true); }
      const made = S.clips.find((c) => !before.has(c.id) && !c.draft);
      if (!made) {
        const why = S.exportErr?.[id];
        throw new Error(`The export did not finish${why ? `: ${why.message || why}` : ". Look at Glint's Export section for the reason."}`);
      }
      return { exported: clipOut(made) };
    },
  };

  T.event.listen("plugin-rpc", async ({ payload }) => {
    const { rid, method, params } = payload || {};
    if (typeof method !== "string" || !method.startsWith(PREFIX)) return; // another plugin's
    const fn = methods[method.slice(PREFIX.length)];
    let ok = true, value;
    try {
      if (!fn) throw new Error(`Unknown method ${method}`);
      value = await fn(params && typeof params === "object" ? params : {});
    } catch (e) {
      ok = false;
      value = String(e?.message ?? e);
    }
    try { await call("plugin_reply", { rid, ok, value: value === undefined ? null : value }); }
    catch (e) { console.error("Glint plugin claude-mcp: could not answer", method, e); }
  }).then(() => call("plugin_ready").catch(() => {}));

  window.__glintClaudeMcpMethods = methods; // for the browser preview's tests only
})();
