"use strict";
// End-to-end scenarios, half 1 of 2 (0.1.37). Plays edits through the UI's real edit model
// (ui/editmodel.js) and writes, per scenario, what the model sends to the export (`emToOpts`)
// and what the result must be: its length, which source second is on screen at given output
// times (or black), and where the clip's tone (and an added song) is heard or silent.
// Half 2 is `glint.exe --e2e-test <report.txt>` with GLINT_E2E_DIR=<folder>: it makes the
// synthetic clip described in clip.json, exports every scenario and checks it. Glint never starts
// this script; tools/e2e.ps1 runs the two one after the other.
//
// Usage: node tools/e2e-scenarios.js <folder> [editmodel-0.1.36.js]
//   The second file (git show 1b36be8:ui/editmodel.js) is the 0.1.36 model for the "old state" scenario.
const fs = require("fs");
const path = require("path");

const dir = process.argv[2];
if (!dir) { console.error("usage: node tools/e2e-scenarios.js <folder> [editmodel-0.1.36.js]"); process.exit(2); }
fs.mkdirSync(dir, { recursive: true });
for (const f of fs.readdirSync(dir)) if (/^(s\d+.*|clip)\.json$/.test(f)) fs.unlinkSync(path.join(dir, f));
// E2E_MODEL=<file> plays the scenarios on another copy of the model (an older release); default: the app's own.
const em = require(process.env.E2E_MODEL ? path.resolve(process.env.E2E_MODEL) : path.join(__dirname, "..", "ui", "editmodel.js"));
const oldPath = process.argv[3];
const old = oldPath && fs.existsSync(oldPath) ? require(path.resolve(oldPath)) : null;

// ---- The synthetic clip, written down once: the Rust half reads clip.json and makes exactly this.
// A white bar 8 px wide moves across a black 640x360 picture: at source second s its left edge is at
// x = trunc(s * (width - 8) / duration). The tracks are titled as a Glint clip's; Mix and Desktop are a `tone` Hz sine,
// the microphone is silent (so Mix = Desktop + Microphone holds the tone once, as a real clip's mix does). A project is the same
// file seen as two library clips joined: A = 0..10, B = 10..20.
const CLIP = { duration: 20, width: 640, height: 360, fps: 30, bar: 8, tone: 1000, tracks: [{ title: "Mix", tone: true }, { title: "Desktop", tone: true }, { title: "Microphone", tone: false }], parts: [{ src: 0, len: 10, title: "A" }, { src: 10, len: 10, title: "B" }], song: { file: "song.wav", freq: 400, duration: 6 },
  // 0.1.38: separate clip files for projects. Each draws its bar only in its own third of the picture
  // (`band`), has its own tone; S3 is smaller and has one track only.
  sources: [
    { name: "S1", duration: 8, width: 640, height: 360, fps: 30, bar: 8, band: 0, tone: 1000, tracks: [{ title: "Mix", tone: true }, { title: "Desktop", tone: true }, { title: "Microphone", tone: false }] },
    { name: "S2", duration: 6, width: 640, height: 360, fps: 30, bar: 8, band: 1, tone: 1500, tracks: [{ title: "Mix", tone: true }, { title: "Desktop", tone: true }, { title: "Microphone", tone: false }] },
    { name: "S3", duration: 5, width: 480, height: 270, fps: 30, bar: 6, band: 2, tone: 700, tracks: [{ title: "Mix", tone: true }] },
    // Tracks in another order and no Mix: Microphone first (its own tone), then a silent Desktop.
    { name: "S4", duration: 6, width: 640, height: 360, fps: 30, bar: 8, band: 1, tone: 1800, tracks: [{ title: "Microphone", tone: true }, { title: "Desktop", tone: false }] },
  ] };
fs.writeFileSync(path.join(dir, "clip.json"), JSON.stringify(CLIP, null, 2));

// A song to add as an audio file: `song.freq` Hz, 48 kHz mono 16-bit WAV.
{
  const n = CLIP.song.duration * 48000, data = Buffer.alloc(44 + n * 2);
  data.write("RIFF", 0); data.writeUInt32LE(36 + n * 2, 4); data.write("WAVEfmt ", 8); data.writeUInt32LE(16, 16);
  data.writeUInt16LE(1, 20); data.writeUInt16LE(1, 22); data.writeUInt32LE(48000, 24); data.writeUInt32LE(96000, 28);
  data.writeUInt16LE(2, 32); data.writeUInt16LE(16, 34); data.write("data", 36); data.writeUInt32LE(n * 2, 40);
  for (let i = 0; i < n; i++) data.writeInt16LE(Math.round(Math.sin((i / 48000) * CLIP.song.freq * 2 * Math.PI) * 0.3 * 32767), 44 + i * 2);
  fs.writeFileSync(path.join(dir, CLIP.song.file), data);
}
const songPath = path.resolve(dir, CLIP.song.file);

const TRACKS = CLIP.tracks.map((x, index) => ({ index, title: x.title, path: "" }));
const single = (E = em) => E.emNew(CLIP.duration, TRACKS);
const project = (E = em) => { const m = E.emNew(CLIP.duration, TRACKS); E.emSetParts(m, CLIP.parts.map((p) => p.src), CLIP.parts.map((p) => p.title)); return m; };

// ---- B's 0.1.37 names when they are there, else the 0.1.36 functions that do the same.
const NEW = ["emPieceList", "emPieceAt", "emSplitAt", "emRemovePiece", "emTrimPiece", "emPieceToItem"];
const hasNew = NEW.every((n) => typeof em[n] === "function");
const missing = NEW.filter((n) => typeof em[n] !== "function");
const ops = {
  split(m, t) { return hasNew ? em.emSplitAt(m, t) : m.parts ? em.emSplitPartAt(m, t) : em.emSplit(m, t) >= 0; },
  /** Take the piece under timeline time `t` out: close up, or leave a gap. */
  remove(m, t, leaveGap) {
    if (hasNew) return em.emRemovePiece(m, em.emPieceAt(m, t), leaveGap);
    const i = em.emSegmentAt(m, m.parts ? em.emToSrc(m, t) : t);
    return leaveGap ? em.emLiftSeg(m, i, "Clip") : em.emSetRemoved(m, i, true);
  },
  /** Move the piece under timeline time `t` so it starts at `at` (0.1.36's free placement). */
  /** Split a clip piece in two at `t` (a part border, so each half can move alone). */
  splitPiece(m, t) {
    if (hasNew) return em.emSplitAt(m, t);
    if (!m.parts) em.emEnsureParts(m, "Clip");
    return em.emSplitPartAt(m, t);
  },
  /** Reorder: the piece under `t` goes right after the piece under `u`; its old place closes. */
  after(m, t, u) {
    const at = (x) => (hasNew ? em.emPieceAt(m, x) : em.emParts(m).findIndex((p) => x >= p.at - 1e-6 && x < p.end - 1e-6));
    return em.emPutBeside(m, at(t), at(u), true);
  },
  place(m, t, at) {
    if (hasNew) return em.emPlacePart(m, em.emPieceAt(m, t), at, "Clip");
    if (!m.parts) em.emEnsureParts(m, "Clip");
    const i = em.emParts(m).findIndex((p) => t >= p.at - 1e-6 && t < p.end - 1e-6);
    return em.emPlacePart(m, i, at, "Clip");
  },
};

// ---- Expectations from a layout written by hand: [{ src, len } | { gap }], the speed, the tail (OUTPUT s).
function expect(layout, { speed = 1, tail = 0, song = null, kind = "" } = {}) {
  const frames = [], tones = [];
  let tl = 0;
  const out = (x) => +(x / speed).toFixed(4);
  for (const it of layout) {
    const len = it.gap !== undefined ? it.gap : it.len;
    // Frames: 0.2 s inside each end and the middle (timeline seconds), as output times.
    for (const d of len > 0.6 ? [0.2, len / 2, len - 0.2] : [len / 2]) {
      frames.push(it.gap !== undefined ? { t: out(tl + d), black: true } : { t: out(tl + d), src: +(it.src + d).toFixed(4) });
    }
    if (len > 0.5) tones.push({ freq: CLIP.tone, from: out(tl + 0.15), to: out(tl + len - 0.15), heard: it.gap === undefined });
    tl += len;
  }
  const video = tl / speed;
  if (tail > 0) {
    frames.push({ t: +(video + tail / 2).toFixed(4), black: true });
    if (tail > 0.5) tones.push({ freq: CLIP.tone, from: +(video + 0.15).toFixed(4), to: +(video + tail - 0.15).toFixed(4), heard: false });
  }
  if (song) tones.push({ freq: CLIP.song.freq, from: song.from, to: song.to, heard: true });
  return { duration: +(video + tail).toFixed(4), kind, frames, tones };
}

const files = [];
function scenario(name, build, layoutOf, opts = {}) {
  const checks = [];
  const check = (what, ok, detail = "") => checks.push({ name: what, ok: !!ok, detail: String(detail) });
  let m = null, err = null;
  try { m = build(check); } catch (e) { err = e; }
  if (!m) {
    check("the edit can be made with the model", false, err ? err.stack || err : "the scenario built no model");
    files.push({ name, checks, opts: null, expect: null });
    return;
  }
  const want = expect(layoutOf, opts);
  check("emToOpts gives segments", Array.isArray(em.emToOpts(m).segments), JSON.stringify(em.emToOpts(m)));
  if (hasNew) {
    // B's own description of the row must agree with the layout written here.
    const row = em.emPieceList(m).map((p) => (p.gap ? { gap: +p.len.toFixed(3) } : { src: +p.src.toFixed(3), len: +p.len.toFixed(3) }));
    const hand = layoutOf.map((it) => (it.gap !== undefined ? { gap: +it.gap.toFixed(3) } : { src: +it.src.toFixed(3), len: +it.len.toFixed(3) }));
    check("emPieceList is the layout of the scenario", JSON.stringify(row) === JSON.stringify(hand), `${JSON.stringify(row)} vs ${JSON.stringify(hand)}`);
  }
  files.push({ name, checks, opts: em.emToOpts(m), expect: want, model: hasNew ? "0.1.37" : "0.1.36 functions" });
}

const S = CLIP.duration;
// 1. Split twice and take the middle out, closing up.
scenario("split and remove, closed up (single clip)", (check) => {
  const m = single();
  check("split at 4", ops.split(m, 4)); check("split at 8", ops.split(m, 8));
  check("remove 4..8, close up", ops.remove(m, 6, false));
  return m;
}, [{ src: 0, len: 4 }, { src: 8, len: 12 }]);
scenario("split and remove, closed up (project)", (check) => {
  const m = project();
  check("split at 3", ops.split(m, 3)); check("split at 6", ops.split(m, 6));
  check("remove 3..6, close up", ops.remove(m, 4.5, false));
  return m;
}, [{ src: 0, len: 3 }, { src: 6, len: 4 }, { src: 10, len: 10 }]);
// 2. Remove, leaving a gap.
scenario("remove leaving a gap (single clip)", (check) => {
  const m = single();
  check("split at 5", ops.split(m, 5)); check("split at 7", ops.split(m, 7));
  check("remove 5..7, a gap stays", ops.remove(m, 6, true));
  return m;
}, [{ src: 0, len: 5 }, { gap: 2 }, { src: 7, len: 13 }]);
scenario("remove leaving a gap (project)", (check) => {
  const m = project();
  check("split at 12", ops.split(m, 12)); check("split at 15", ops.split(m, 15));
  check("remove 12..15, a gap stays", ops.remove(m, 13, true));
  return m;
}, [{ src: 0, len: 10 }, { src: 10, len: 2 }, { gap: 3 }, { src: 15, len: 5 }]);
// 3. Trim both ends, then pull the end all the way out again: as the untrimmed clip from its new start.
scenario("trim both ends, restore the end fully (single clip)", (check) => {
  const m = single();
  if (hasNew) {
    check("trim the end to 15", em.emTrimPiece(m, 0, "end", 15));
    check("trim the start to 2 (the piece stays where it is: a gap before it)", em.emTrimPiece(m, 0, "start", 2));
    const left = em.emPieceList(m);
    check("then the row is a 2 s gap and the piece", left.length === 2 && left[0].gap && Math.abs(left[1].at - 2) < 1e-6, JSON.stringify(left));
    check("move it back to 0", em.emPlacePart(m, em.emPieceAt(m, 3), 0, "Clip"));
    const r = em.emTrimPiece(m, em.emPieceAt(m, 1), "end", 40);
    check("pulled out to the clip's end", r && Math.abs(r.src + r.len - S) < 1e-6, JSON.stringify(r));
  } else {
    check("trim the end to 15", Math.abs(em.emSetTrimEnd(m, 15) - 15) < 1e-6);
    check("trim the start to 2", Math.abs(em.emSetTrimStart(m, 2) - 2) < 1e-6);
    check("restore the end fully", Math.abs(em.emSetTrimEnd(m, S) - S) < 1e-6);
  }
  return m;
}, [{ src: 2, len: 18 }]);
// 4. Pulling an end out stops where the footage stops.
scenario("restore stops at the footage limit (single clip)", (check) => {
  const m = single();
  if (hasNew) {
    check("trim the end to 12", em.emTrimPiece(m, 0, "end", 12));
    const r = em.emTrimPiece(m, 0, "end", 30);
    check("stopped at the clip's end", r && Math.abs(r.src + r.len - S) < 1e-6, JSON.stringify(r));
  } else {
    check("trim the end to 12", Math.abs(em.emSetTrimEnd(m, 12) - 12) < 1e-6);
    const end = em.emSetTrimEnd(m, 30);
    check("stopped at the clip's end", Math.abs(end - S) < 1e-6, end);
  }
  return m;
}, [{ src: 0, len: 20 }]);
if (hasNew) {
  scenario("restore stops at the library clip's end, not its neighbour's (project)", (check) => {
    const m = project();
    check("trim A's end to 7", em.emTrimPiece(m, 0, "end", 7));
    const r = em.emTrimPiece(m, 0, "end", 14);
    check("A stopped at 10 (its own footage)", r && Math.abs(r.src + r.len - 10) < 1e-6, JSON.stringify(r));
    return m;
  }, [{ src: 0, len: 10 }, { src: 10, len: 10 }]);
}
// 5. A piece moved after another.
scenario("a piece moved after another (project)", (check) => {
  const m = project();
  check("A goes after B", ops.after(m, 2, 15));
  return m;
}, [{ src: 10, len: 10 }, { src: 0, len: 10 }]);
scenario("a piece moved after another (single clip)", (check) => {
  const m = single();
  check("split at 6", ops.splitPiece(m, 6));
  check("0..6 goes after 6..20", ops.after(m, 2, 17));
  return m;
}, [{ src: 6, len: 14 }, { src: 0, len: 6 }]);
// 6. A piece moved into free space: exactly there, a gap where it was.
scenario("a piece moved into free space (single clip)", (check) => {
  const m = single();
  check("split at 10", ops.splitPiece(m, 10));
  check("10..20 goes to 23", ops.place(m, 12, 23));
  return m;
}, [{ src: 0, len: 10 }, { gap: 13 }, { src: 10, len: 10 }]);
scenario("a piece moved into free space (project)", (check) => {
  const m = project();
  check("B goes to 12", ops.place(m, 15, 12));
  return m;
}, [{ src: 0, len: 10 }, { gap: 2 }, { src: 10, len: 10 }]);
// 7. The same footage twice (an end pulled over footage another piece shows).
if (hasNew) {
  scenario("the same footage on the timeline twice (single clip)", (check) => {
    const m = single();
    check("split at 8", em.emSplitAt(m, 8));
    const r = em.emTrimPiece(m, 0, "end", 12);
    check("the first piece pulled out to 12, over 8..12 of the second", r && Math.abs(r.len - 12) < 1e-6, JSON.stringify(r));
    return m;
  }, [{ src: 0, len: 12 }, { src: 8, len: 12 }]);
}
// 8. Speed 2x.
scenario("pieces at 2x speed (single clip)", (check) => {
  const m = single();
  check("split at 4", ops.split(m, 4)); check("split at 8", ops.split(m, 8));
  check("remove 4..8", ops.remove(m, 6, false));
  m.speed = 2;
  return m;
}, [{ src: 0, len: 4 }, { src: 8, len: 12 }], { speed: 2 });
// 9. Pieces and a tail: a song that plays through runs 3 s past the video.
scenario("pieces and a tail (single clip)", (check) => {
  const m = single();
  check("split at 4", ops.split(m, 4)); check("split at 12", ops.split(m, 12));
  check("remove 4..12", ops.remove(m, 8, false));
  // At 9 s of the result: with 0.1.36's functions the cut-out stretch 4..12 still lies on the timeline, so that is 17.
  const row = em.emAddFile(m, { path: songPath, title: "song", duration: CLIP.song.duration }, hasNew ? 9 : 17);
  check("the song is a row that plays through", row && row.through === true, JSON.stringify(row));
  // Its block's right edge dragged out to the song's end, past the video (as timeline.js does).
  em.emPlaceFile(m, row.id, { length: CLIP.song.duration });
  check("the result ends 3 s after the video", Math.abs(em.emTail(m) - 3) < 1e-6, em.emTail(m));
  return m;
}, [{ src: 0, len: 4 }, { src: 12, len: 8 }], { tail: 3, song: { from: 9.2, to: 14.8 } });
// 10. An edit saved by 0.1.36 (cut-out stretches, a gap) loads and exports as 0.1.36 sent it.
scenario("an old 0.1.36 state with cut-out stretches", (check) => {
  if (!old) { check("the 0.1.36 model is given (git show 1b36be8:ui/editmodel.js)", false, oldPath || "no file named"); return null; }
  const o = project(old);
  old.emSplit(o, 2); old.emSplit(o, 4); old.emSetRemoved(o, old.emSegmentAt(o, 3), true);
  old.emSplit(o, 14); old.emSplit(o, 16); old.emSetRemoved(o, old.emSegmentAt(o, 15), true);
  const saved = JSON.parse(JSON.stringify(old.emStateSave(o, [10])));
  const sent = JSON.stringify(old.emToOpts(o));
  const m = project();
  check("the saved state loads", em.emStateLoad(m, saved));
  check("it sends what 0.1.36 sent", JSON.stringify(em.emToOpts(m)) === sent, `${JSON.stringify(em.emToOpts(m))}\n vs 0.1.36 ${sent}`);
  return m;
}, [{ src: 0, len: 2 }, { src: 4, len: 6 }, { src: 10, len: 4 }, { src: 16, len: 4 }]);
// 11. A single clip trimmed to one piece is the lossless cut.
scenario("a single clip trimmed to one piece is the lossless cut", (check) => {
  const m = single();
  if (hasNew) { check("trim the end to 14", em.emTrimPiece(m, 0, "end", 14)); }
  else check("trim the end to 14", Math.abs(em.emSetTrimEnd(m, 14) - 14) < 1e-6);
  check("emIsPlainCut", em.emIsPlainCut(m));
  return m;
}, [{ src: 0, len: 14 }], { kind: "trim" });

// ---- 0.1.38: projects that reference separate clip files (no joined working file). The sources are
// in clip.json too; the model works on "project time" (the clips end to end, joins = their starts) and
// the Rust half exports through projexport.rs. A layout item is { k (source in the project), s (seconds
// into it), len } or { gap }.
const SOURCES = CLIP.sources;
const byName = (n) => SOURCES.find((x) => x.name === n);
// The model as the editor builds it for such a project (ui/editor.js `virtualModel` / `titleRows`): no tracks of
// its own, one track row per title across the clips (each clip's shown tracks), its parts, m.bounds, m.trackTitles.
function projectModel(names) {
  const srcs = names.map(byName);
  const starts = [];
  let acc = 0;
  for (const x of srcs) { starts.push(acc); acc += x.duration; }
  const m = em.emNew(acc, []);
  m.virtual = true;
  const titles = em.emTrackTitles(srcs.map((x) => em.emShownTracks(x.tracks.map((t, index) => ({ index, title: t.title })))));
  const n = m.removed.length;
  m.audio = [...titles.map((title, k) => ({ id: `t${k}`, kind: "track", index: k, title, path: "", volume: 1, muted: false, gains: Array(n).fill(1) })), ...m.audio.filter((r) => r.kind !== "track")];
  m.tracks0 = titles.map((title, k) => ({ index: k, title }));
  m.trackTitles = titles.slice();
  if (srcs.length > 1) em.emSetParts(m, starts, names);
  m.bounds = starts.slice();
  return { m, starts, srcs };
}
function projectExpect(names, layout, { speed = 1, tail = 0, song = null, kind = "", joined = null, error = "", tones: given = null } = {}) {
  if (error) return { error, frames: [], tones: [], duration: 0, kind: "", joined };
  const srcs = names.map(byName);
  const frames = [], tones = [];
  let tl = 0;
  const out = (x) => +(x / speed).toFixed(4);
  for (const it of layout) {
    const len = it.gap !== undefined ? it.gap : it.len;
    for (const d of len > 0.6 ? [0.2, len / 2, len - 0.2] : [len / 2]) {
      frames.push(it.gap !== undefined ? { t: out(tl + d), black: true } : { t: out(tl + d), source: srcs[it.k].name, src: +(it.s + d).toFixed(4) });
    }
    if (len > 0.5) tones.push(it.gap !== undefined
      ? { freq: srcs[0].tone, from: out(tl + 0.15), to: out(tl + len - 0.15), heard: false }
      : { freq: srcs[it.k].tone, from: out(tl + 0.15), to: out(tl + len - 0.15), heard: true });
    tl += len;
  }
  const video = tl / speed;
  if (tail > 0) frames.push({ t: +(video + tail / 2).toFixed(4), black: true });
  if (song) tones.push({ freq: CLIP.song.freq, from: song.from, to: song.to, heard: true });
  return { duration: +(video + tail).toFixed(4), kind, frames, tones: given || tones, joined };
}
function projectScenario(name, names, build, layout, opts = {}, missingNames = []) {
  const checks = [];
  const check = (what, ok, detail = "") => checks.push({ name: what, ok: !!ok, detail: String(detail) });
  let pm = null, err = null;
  try { pm = projectModel(names); if (!build(pm, check)) pm = null; } catch (e) { err = e; pm = null; }
  if (!pm) {
    check("the edit can be made with the model", false, err ? err.stack || err : "the scenario built no edit");
    files.push({ name, checks, opts: null, expect: null, project: { sources: names, missing: missingNames } });
    return;
  }
  if (hasNew && !opts.error) {
    const row = em.emPieceList(pm.m).map((p) => (p.gap ? { gap: +p.len.toFixed(3) } : { src: +p.src.toFixed(3), len: +p.len.toFixed(3) }));
    const hand = layout.map((it) => (it.gap !== undefined ? { gap: +it.gap.toFixed(3) } : { src: +(pm.starts[it.k] + it.s).toFixed(3), len: +it.len.toFixed(3) }));
    check("emPieceList is the layout of the scenario (project time)", JSON.stringify(row) === JSON.stringify(hand), `${JSON.stringify(row)} vs ${JSON.stringify(hand)}`);
  }
  files.push({ name, checks, opts: em.emToOpts(pm.m), expect: projectExpect(names, layout, opts), project: { sources: names, missing: missingNames }, model: hasNew ? "0.1.37+" : "0.1.36 functions" });
}
if (hasNew) {
  const piece = (m, t) => em.emPieceAt(m, t);
  projectScenario("project: pieces of three sources reordered", ["S1", "S2", "S3"], ({ m }, check) => {
    check("S1 goes after S3", em.emPutBeside(m, piece(m, 1), piece(m, 15), true));
    return true;
  }, [{ k: 1, s: 0, len: 6 }, { k: 2, s: 0, len: 5 }, { k: 0, s: 0, len: 8 }], { joined: true });
  projectScenario("project: a piece of source 2 between pieces of source 1", ["S1", "S2"], ({ m }, check) => {
    check("split S1 at 4", em.emSplitAt(m, 4));
    check("S2 goes after the first half of S1", em.emPutBeside(m, piece(m, 10), piece(m, 2), true));
    return true;
  }, [{ k: 0, s: 0, len: 4 }, { k: 1, s: 0, len: 6 }, { k: 0, s: 4, len: 4 }], { joined: true });
  projectScenario("project: trim and restore up to a source's own end and no further", ["S1", "S2"], ({ m }, check) => {
    check("trim S1's end to 5", em.emTrimPiece(m, 0, "end", 5));
    const r = em.emTrimPiece(m, 0, "end", 12);
    check("pulled out, it stops at S1's own end (8)", r && Math.abs(r.src + r.len - 8) < 1e-6, JSON.stringify(r));
    return true;
  }, [{ k: 0, s: 0, len: 8 }, { k: 1, s: 0, len: 6 }], { joined: true });
  projectScenario("project: a gap", ["S1", "S2"], ({ m }, check) => {
    check("split at 3", em.emSplitAt(m, 3)); check("split at 5", em.emSplitAt(m, 5));
    check("3..5 out, a gap stays", em.emRemovePiece(m, piece(m, 4), true));
    return true;
  }, [{ k: 0, s: 0, len: 3 }, { gap: 2 }, { k: 0, s: 5, len: 3 }, { k: 1, s: 0, len: 6 }], { joined: true });
  projectScenario("project: the same source twice", ["S1", "S2", "S1"], () => true,
    [{ k: 0, s: 0, len: 8 }, { k: 1, s: 0, len: 6 }, { k: 2, s: 0, len: 8 }], { joined: true });
  projectScenario("project: speed 2x", ["S1", "S2"], ({ m }, check) => {
    check("split at 4", em.emSplitAt(m, 4));
    check("0..4 out, closed up", em.emRemovePiece(m, piece(m, 2), false));
    m.speed = 2;
    return true;
  }, [{ k: 0, s: 4, len: 4 }, { k: 1, s: 0, len: 6 }], { speed: 2, joined: true });
  projectScenario("project: a tail with a song", ["S1", "S2"], ({ m }, check) => {
    const row = em.emAddFile(m, { path: songPath, title: "song", duration: CLIP.song.duration }, 10);
    check("the song plays through", row && row.through === true, JSON.stringify(row));
    em.emPlaceFile(m, row.id, { length: CLIP.song.duration });
    check("the result ends 2 s after the video", Math.abs(em.emTail(m) - 2) < 1e-6, em.emTail(m));
    return true;
  }, [{ k: 0, s: 0, len: 8 }, { k: 1, s: 0, len: 6 }], { tail: 2, song: { from: 10.2, to: 15.8 }, joined: true });
  projectScenario("project: all ranges in one source (its own file, no join)", ["S1", "S2"], ({ m }, check) => {
    check("S2 out, closed up", em.emRemovePiece(m, piece(m, 10), false));
    check("split at 3", em.emSplitAt(m, 3)); check("split at 5", em.emSplitAt(m, 5));
    check("3..5 out, closed up", em.emRemovePiece(m, piece(m, 4), false));
    return true;
  }, [{ k: 0, s: 0, len: 3 }, { k: 0, s: 5, len: 3 }], { joined: false });
  projectScenario("project: one trimmed piece of one source is the lossless cut of its file", ["S1", "S2"], ({ m }, check) => {
    check("S2 out, closed up", em.emRemovePiece(m, piece(m, 10), false));
    check("trim S1's end to 6", em.emTrimPiece(m, 0, "end", 6));
    return true;
  }, [{ k: 0, s: 0, len: 6 }], { kind: "trim", joined: false });
  // Rows by title (0.1.38): S4 has Microphone and Desktop in that order, and no Mix. With only the Desktop row
  // heard, S1 sounds (its Desktop) and S4 is silent (its Desktop is); with only the Microphone row, the other way.
  const solo = (title) => ({ m }, check) => {
    check("the rows are Desktop, Microphone", JSON.stringify(m.trackTitles) === JSON.stringify(["Desktop", "Microphone"]), JSON.stringify(m.trackTitles));
    for (const r of m.audio) if (r.kind === "track" && r.title !== title) r.muted = true;
    check("sent with the titles", JSON.stringify(em.emToOpts(m).track_titles) === JSON.stringify(["Desktop", "Microphone"]), JSON.stringify(em.emToOpts(m).track_titles));
    return true;
  };
  projectScenario("project: rows by title, Desktop alone (the second clip has its tracks in another order, no Mix)", ["S1", "S4"], solo("Desktop"),
    [{ k: 0, s: 0, len: 8 }, { k: 1, s: 0, len: 6 }], { joined: true, tones: [{ freq: 1000, from: 0.15, to: 7.85, heard: true }, { freq: 1800, from: 0.15, to: 7.85, heard: false }, { freq: 1800, from: 8.15, to: 13.85, heard: false }, { freq: 1000, from: 8.15, to: 13.85, heard: false }] });
  projectScenario("project: rows by title, Microphone alone", ["S1", "S4"], solo("Microphone"),
    [{ k: 0, s: 0, len: 8 }, { k: 1, s: 0, len: 6 }], { joined: true, tones: [{ freq: 1000, from: 0.15, to: 7.85, heard: false }, { freq: 1800, from: 8.15, to: 13.85, heard: true }] });
  projectScenario("project: rows by title, one clip's own Microphone track first in its file (direct)", ["S4", "S1"], ({ m }, check) => {
    check("S1 out, closed up", em.emRemovePiece(m, piece(m, 8), false));
    for (const r of m.audio) if (r.kind === "track" && r.title !== "Microphone") r.muted = true;
    return true;
  }, [{ k: 0, s: 0, len: 6 }], { joined: false, tones: [{ freq: 1800, from: 0.15, to: 5.85, heard: true }] });
  // Options the model never sends (its pieces stay inside one clip): one range across the border of S1 and S2.
  // The export splits it at the border; the result is the end of S1 then the start of S2.
  files.push({ name: "project: a range across two clips is split at their border", checks: [], opts: { segments: [{ start: 6, end: 10 }], ordered: true, speed: 1 },
    expect: projectExpect(["S1", "S2"], [{ k: 0, s: 6, len: 2 }, { k: 1, s: 0, len: 2 }], { joined: true }), project: { sources: ["S1", "S2"], missing: [] }, model: "options by hand" });
  projectScenario("project: a missing source is a clear error, nothing written", ["S1", "S2"], () => true,
    [], { error: "\"S2\"", joined: false }, ["S2"]);
  projectScenario("project: a failed export after the join leaves no working file", ["S1", "S2"], ({ m }) => {
    em.emAddFile(m, { path: path.resolve(dir, "no-such-song.wav"), title: "gone", duration: 3 }, 2);
    return true;
  }, [], { error: "no-such-song.wav", joined: true });
}
if (missing.length) console.log(`note: not in ui/editmodel.js yet: ${missing.join(", ")} (0.1.36 functions used; scenarios that need them left out)`);
files.forEach((f, i) => fs.writeFileSync(path.join(dir, `s${String(i + 1).padStart(2, "0")}.json`), JSON.stringify(f, null, 2)));
const bad = files.reduce((n, f) => n + f.checks.filter((c) => !c.ok).length, 0);
console.log(`${files.length} scenarios written to ${dir} (${hasNew ? "0.1.37 model" : "0.1.36 functions"}); model checks failing: ${bad}`);
