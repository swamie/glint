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
const CLIP = { duration: 20, width: 640, height: 360, fps: 30, bar: 8, tone: 1000, tracks: [{ title: "Mix", tone: true }, { title: "Desktop", tone: true }, { title: "Microphone", tone: false }], parts: [{ src: 0, len: 10, title: "A" }, { src: 10, len: 10, title: "B" }], song: { file: "song.wav", freq: 400, duration: 6 } };
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

if (missing.length) console.log(`note: not in ui/editmodel.js yet: ${missing.join(", ")} (0.1.36 functions used; scenarios that need them left out)`);
files.forEach((f, i) => fs.writeFileSync(path.join(dir, `s${String(i + 1).padStart(2, "0")}.json`), JSON.stringify(f, null, 2)));
const bad = files.reduce((n, f) => n + f.checks.filter((c) => !c.ok).length, 0);
console.log(`${files.length} scenarios written to ${dir} (${hasNew ? "0.1.37 model" : "0.1.36 functions"}); model checks failing: ${bad}`);
