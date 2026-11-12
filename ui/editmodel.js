"use strict";
// The editor's edit model: the single description of an edit that the timeline
// draws, the player previews and the export applies. Plain data plus pure
// functions; nothing here touches the page.
//
// Every time is in seconds of the SOURCE clip, unless a name says "out"
// (position in the finished clip, after removed parts are cut out; speed is
// not included in "out" times).
//
// Shape of a model `m`:
//   duration   source length
//   cuts       sorted split points, each inside (0, duration). They divide the
//              clip into cuts.length + 1 segments, shared by every row.
//   removed    one flag per segment: true = cut out of the result (video and all audio)
//   audio      rows under the video, top to bottom:
//                { id, kind: "track", index, title, path, volume, muted, gains }
//                { id, kind: "file", path, title, volume, muted, gains,
//                  offset, trimStart, length, fileDuration }
//              gains: one number per segment (1 = unchanged, 0 = silent, up to EM_MAX_GAIN)
//              volume: the row's own level, 0..EM_MAX_GAIN; muted silences the whole row
//              file rows: the file's audio from trimStart, `length` seconds
//              long, placed so it starts at source time `offset`
//   colors     one colour correction per segment: { brightness, contrast, saturation, gamma }, 1 = unchanged
//   blank      one flag per segment: true = the picture is black there while the audio plays on
//   speed      export speed, 0.25..4
//   undo, redo snapshots (JSON strings) for emUndo / emRedo

const EM_MIN_SEG = 0.1; // shortest segment a split or a drag may leave, seconds
const EM_MAX_GAIN = 2;
const EM_EPS = 1e-6;
const EM_COLOR_KEYS = ["brightness", "contrast", "saturation", "gamma"];
const emNoColor = () => ({ brightness: 1, contrast: 1, saturation: 1, gamma: 1 });

/**
 * A fresh model. `tracks` is the result of the `editor_audio_tracks` command
 * ([{ index, title, path }], possibly empty). A 3-track clip shows Desktop and
 * Microphone (tracks 1 and 2), since track 0 is their ready-made mix; any other
 * clip shows every track it has.
 */
function emNew(duration, tracks) {
  const list = tracks || [];
  const shown = list.length === 3 ? list.filter((t) => t.index !== 0) : list;
  return {
    duration: Math.max(0, +duration || 0),
    cuts: [],
    removed: [false],
    audio: shown.map((t) => ({ id: `t${t.index}`, kind: "track", index: t.index, title: t.title || `Track ${t.index + 1}`, path: t.path || "", volume: 1, muted: false, gains: [1] })),
    colors: [emNoColor()],
    blank: [false],
    speed: 1,
    undo: [],
    redo: [],
  };
}

/** Segments in order: [{ i, start, end, removed }]. */
function emSegments(m) {
  const edges = [0, ...m.cuts, m.duration];
  return m.removed.map((removed, i) => ({ i, start: edges[i], end: edges[i + 1], removed }));
}

/** Index of the segment that holds source time `t`. */
function emSegmentAt(m, t) {
  let i = 0;
  while (i < m.cuts.length && t >= m.cuts[i] - EM_EPS) i++;
  return i;
}

/* ---------- undo ---------- */

const emState = (m) => JSON.stringify({ cuts: m.cuts, removed: m.removed, audio: m.audio, speed: m.speed, colors: m.colors, blank: m.blank });
function emRestore(m, json) {
  const s = JSON.parse(json);
  m.cuts = s.cuts;
  m.removed = s.removed;
  m.audio = s.audio;
  m.speed = s.speed;
  m.colors = s.colors;
  m.blank = s.blank;
}

/** Call BEFORE changing the model, so the change can be undone. Clears redo. */
function emSnapshot(m) {
  const now = emState(m);
  if (m.undo[m.undo.length - 1] !== now) m.undo.push(now);
  if (m.undo.length > 100) m.undo.shift();
  m.redo.length = 0;
}
function emUndo(m) {
  if (!m.undo.length) return false;
  m.redo.push(emState(m));
  emRestore(m, m.undo.pop());
  return true;
}
function emRedo(m) {
  if (!m.redo.length) return false;
  m.undo.push(emState(m));
  emRestore(m, m.redo.pop());
  return true;
}

/* ---------- cutting ---------- */

/**
 * Split the segment at `t` in two. Both halves keep the segment's state on
 * every row. Returns the index of the new cut, or -1 if `t` is too close to an
 * existing edge. Does not snapshot.
 */
function emSplit(m, t) {
  const i = emSegmentAt(m, t);
  const seg = emSegments(m)[i];
  if (t - seg.start < EM_MIN_SEG || seg.end - t < EM_MIN_SEG) return -1;
  m.cuts.splice(i, 0, t);
  m.removed.splice(i, 0, m.removed[i]);
  m.colors.splice(i, 0, { ...m.colors[i] });
  m.blank.splice(i, 0, m.blank[i]);
  for (const row of m.audio) row.gains.splice(i, 0, row.gains[i]);
  return i;
}

/** Remove cut `c`, merging segments c and c + 1. The merged segment takes the left one's state. */
function emJoin(m, c) {
  if (c < 0 || c >= m.cuts.length) return false;
  m.cuts.splice(c, 1);
  m.removed.splice(c + 1, 1);
  m.colors.splice(c + 1, 1);
  m.blank.splice(c + 1, 1);
  for (const row of m.audio) row.gains.splice(c + 1, 1);
  return true;
}

/** Drag cut `c` to `t`, kept between its neighbours. Returns the time used. */
function emMoveCut(m, c, t) {
  const lo = (c > 0 ? m.cuts[c - 1] : 0) + EM_MIN_SEG;
  const hi = (c < m.cuts.length - 1 ? m.cuts[c + 1] : m.duration) - EM_MIN_SEG;
  if (hi < lo) return m.cuts[c];
  m.cuts[c] = Math.min(hi, Math.max(lo, t));
  return m.cuts[c];
}

/** Cut segment `i` out of the result, or put it back. The last kept segment cannot be removed. */
function emSetRemoved(m, i, removed) {
  if (i < 0 || i >= m.removed.length) return false;
  if (removed && m.removed.filter((r) => !r).length <= 1 && !m.removed[i]) return false;
  m.removed[i] = !!removed;
  return true;
}

/** "Start here": everything before `t` is cut out. */
function emTrimStart(m, t) {
  emSplit(m, t);
  const first = emSegmentAt(m, t);
  if (m.removed.slice(first).every((r) => r)) return false;
  for (let i = 0; i < first; i++) m.removed[i] = true;
  return true;
}

/** "End here": everything from `t` on is cut out. */
function emTrimEnd(m, t) {
  emSplit(m, t);
  const first = emSegmentAt(m, t);
  if (first === 0 || m.removed.slice(0, first).every((r) => r)) return false;
  for (let i = first; i < m.removed.length; i++) m.removed[i] = true;
  return true;
}

/** Back to one whole, kept segment. Row levels and files stay; the first segment's colour becomes the clip's. */
function emResetCuts(m) {
  m.cuts = [];
  m.removed = [false];
  m.colors = [{ ...m.colors[0] }];
  m.blank = [false];
  for (const row of m.audio) row.gains = [1];
}

/* ---------- time mapping ---------- */

/** The kept parts as merged source ranges: [{ start, end }]. */
function emKept(m) {
  const out = [];
  for (const s of emSegments(m)) {
    if (s.removed) continue;
    const last = out[out.length - 1];
    if (last && Math.abs(last.end - s.start) < EM_EPS) last.end = s.end;
    else out.push({ start: s.start, end: s.end });
  }
  return out;
}

/** Length of the finished clip before speed is applied. */
const emKeptDuration = (m) => emKept(m).reduce((n, r) => n + (r.end - r.start), 0);
/** Length of the finished clip. */
const emOutDuration = (m) => emKeptDuration(m) / (m.speed || 1);

/** Source time -> position in the finished clip (speed not applied). A time inside a removed part maps to where that part was cut. */
function emSrcToOut(m, t) {
  let out = 0;
  for (const r of emKept(m)) {
    if (t <= r.start) break;
    out += Math.min(t, r.end) - r.start;
  }
  return out;
}

/** Position in the finished clip (speed not applied) -> source time. */
function emOutToSrc(m, out) {
  const kept = emKept(m);
  for (const r of kept) {
    const len = r.end - r.start;
    if (out <= len + EM_EPS) return r.start + Math.max(0, out);
    out -= len;
  }
  return kept.length ? kept[kept.length - 1].end : 0;
}

/**
 * For playback: where the player should be when it is at source time `t`.
 * Returns `t` if that moment is kept, the start of the next kept part if it is
 * cut out, or null when nothing kept follows (the preview has reached its end).
 */
function emPlayable(m, t) {
  for (const r of emKept(m)) {
    if (t < r.start - EM_EPS) return r.start;
    if (t < r.end - EM_EPS) return t;
  }
  return null;
}

/* ---------- picture: colour and hiding, per segment ---------- */

const emSameColor = (a, b = emNoColor()) => EM_COLOR_KEYS.every((k) => Math.abs((a?.[k] ?? 1) - (b?.[k] ?? 1)) < 0.005);

/** The colour correction in force at source time `t`. */
const emColorAt = (m, t) => m.colors[emSegmentAt(m, t)] || emNoColor();
/** True if the picture is hidden (black) at source time `t`. */
const emBlankAt = (m, t) => !!m.blank[emSegmentAt(m, t)];

/**
 * Change the colour correction of some segments. `segs`: segment indexes, or
 * null for every segment (the whole clip). `change`: any of the four colour
 * keys; keys left out keep their value.
 */
function emSetColor(m, segs, change) {
  const list = segs == null ? m.colors.map((_, i) => i) : segs;
  let any = false;
  for (const i of list) {
    if (i < 0 || i >= m.colors.length) continue;
    for (const k of EM_COLOR_KEYS) if (k in change && Number.isFinite(+change[k])) m.colors[i][k] = +change[k];
    any = true;
  }
  return any;
}

/** Hide the picture of segment `i` (black, the audio plays on), or show it again. */
function emSetBlank(m, i, on) {
  if (i < 0 || i >= m.blank.length) return false;
  m.blank[i] = !!on;
  return true;
}

/**
 * The kept parts as the export needs them: like emKept, but neighbours are
 * joined only when they look the same. [{ start, end, color, blank }]
 */
function emPieces(m) {
  const out = [];
  for (const s of emSegments(m)) {
    if (s.removed) continue;
    const last = out[out.length - 1];
    if (last && Math.abs(last.end - s.start) < EM_EPS && last.blank === !!m.blank[s.i] && emSameColor(last.color, m.colors[s.i])) last.end = s.end;
    else out.push({ start: s.start, end: s.end, color: { ...m.colors[s.i] }, blank: !!m.blank[s.i] });
  }
  return out;
}

/* ---------- audio ---------- */

const emRow = (m, id) => m.audio.find((r) => r.id === id) || null;

/** True if file row `row` has sound at source time `t`. Track rows always do. */
const emRowCovers = (row, t) => row.kind !== "file" || (t >= row.offset - EM_EPS && t < row.offset + row.length);

/** How loud `row` is at source time `t`: 0 = silent, 1 = as recorded. */
function emGainAt(m, row, t) {
  if (row.muted || !emRowCovers(row, t)) return 0;
  return row.volume * row.gains[emSegmentAt(m, t)];
}

/** Set the level of segment `i` on a row (0 mutes it). */
function emSetGain(m, id, i, gain) {
  const row = emRow(m, id);
  if (!row || i < 0 || i >= row.gains.length) return false;
  row.gains[i] = Math.min(EM_MAX_GAIN, Math.max(0, +gain || 0));
  return true;
}

/** True if row `id` is the only one heard in segment `i` (and at least one other row exists). */
function emIsSolo(m, id, i) {
  const row = emRow(m, id);
  return !!row && m.audio.length > 1 && row.gains[i] > 0 && m.audio.every((r) => r === row || r.gains[i] === 0);
}

/**
 * Solo: silence every other row in segment `i`, so only row `id` is heard
 * there. On a segment that is already soloed this way, the others come back
 * at their recorded level.
 */
function emSolo(m, id, i) {
  const row = emRow(m, id);
  if (!row || i < 0 || i >= row.gains.length || m.audio.length < 2) return false;
  const undo = emIsSolo(m, id, i);
  for (const r of m.audio) if (r !== row) r.gains[i] = undo ? 1 : 0;
  if (!undo && row.gains[i] === 0) row.gains[i] = 1;
  return true;
}

/**
 * Add a dropped or picked audio file as a new row. `info` is the result of the
 * `editor_audio_info` command ({ path, title, duration }). It starts at source
 * time `at` and is as long as fits. Returns the new row.
 */
function emAddFile(m, info, at = 0) {
  const n = m.audio.filter((r) => r.kind === "file").length + 1;
  const offset = Math.min(Math.max(0, at), Math.max(0, m.duration - EM_MIN_SEG));
  const row = {
    id: `f${Date.now().toString(36)}${n}`,
    kind: "file",
    path: info.path,
    title: info.title || `Audio ${n}`,
    volume: 1,
    muted: false,
    gains: m.removed.map(() => 1),
    offset,
    trimStart: 0,
    length: Math.max(EM_MIN_SEG, Math.min(info.duration, m.duration - offset)),
    fileDuration: info.duration,
  };
  m.audio.push(row);
  return row;
}

/** Remove an added file row. The clip's own tracks cannot be removed (mute them instead). */
function emRemoveRow(m, id) {
  const at = m.audio.findIndex((r) => r.id === id && r.kind === "file");
  if (at < 0) return false;
  m.audio.splice(at, 1);
  return true;
}

/**
 * Move or trim a file row. Any of { offset, trimStart, length } may be given;
 * the result is kept inside the file and inside the clip.
 */
function emPlaceFile(m, id, change) {
  const row = emRow(m, id);
  if (!row || row.kind !== "file") return false;
  let { offset, trimStart, length } = { ...row, ...change };
  trimStart = Math.min(Math.max(0, trimStart), Math.max(0, row.fileDuration - EM_MIN_SEG));
  length = Math.min(Math.max(EM_MIN_SEG, length), row.fileDuration - trimStart);
  offset = Math.min(Math.max(0, offset), Math.max(0, m.duration - EM_MIN_SEG));
  length = Math.max(EM_MIN_SEG, Math.min(length, m.duration - offset));
  Object.assign(row, { offset, trimStart, length });
  return true;
}

/* ---------- export ---------- */

/** True if no audio row differs from the clip as recorded (so the export can keep the original tracks). */
const emAudioUntouched = (m) => m.audio.every((r) => r.kind === "track" && !r.muted && Math.abs(r.volume - 1) < 0.005 && r.gains.every((g, i) => m.removed[i] || Math.abs(g - 1) < 0.005));
/** True if the edit is nothing but one kept range: the export can cut it without re-encoding. */
const emIsPlainCut = (m) => emKept(m).length === 1 && Math.abs(m.speed - 1) < 0.005 && emPieces(m).every((p) => !p.blank && emSameColor(p.color)) && emAudioUntouched(m);
/** True if the result would differ from the source at all. */
const emChanged = (m) => !(emIsPlainCut(m) && m.removed.every((r) => !r));

const emRound = (x) => Math.round(x * 1000) / 1000;

/**
 * The `opts` for the `export_clip` command.
 *   segments  the kept source ranges, in order: { start, end }, plus
 *             `color` when the kept parts do not all share one colour
 *             correction (it then replaces the top-level `color` for that
 *             range) and `blank: true` where the picture is hidden (black)
 *   color     the colour correction of the whole result when every kept part
 *             shares it; neutral when the ranges carry their own
 *   audio     [] = keep the clip's audio tracks as recorded. Otherwise one
 *             entry per row that is heard, mixed into ONE track:
 *             { source: "track" | "file", index, path, offset, trim_start,
 *               length, volume, regions: [{ start, end, gain }] }
 *             `regions` lists the source ranges whose level differs from the
 *             row's volume (gain multiplies it; 0 = silent).
 */
function emToOpts(m) {
  const segs = emSegments(m);
  const pieces = emPieces(m);
  const uniform = pieces.every((p) => emSameColor(p.color, pieces[0].color));
  const audio = emAudioUntouched(m)
    ? []
    : m.audio
        .filter((r) => !r.muted && r.volume > 0.005)
        .map((r) => ({
          source: r.kind,
          index: r.kind === "track" ? r.index : 0,
          path: r.kind === "file" ? r.path : "",
          offset: r.kind === "file" ? emRound(r.offset) : 0,
          trim_start: r.kind === "file" ? emRound(r.trimStart) : 0,
          length: r.kind === "file" ? emRound(r.length) : 0,
          volume: emRound(r.volume),
          regions: segs.filter((s) => !s.removed && Math.abs(r.gains[s.i] - 1) >= 0.005).map((s) => ({ start: emRound(s.start), end: emRound(s.end), gain: emRound(r.gains[s.i]) })),
        }));
  return {
    start: 0,
    end: 0,
    speed: m.speed,
    color: uniform && pieces.length ? { ...pieces[0].color } : emNoColor(),
    tracks: [],
    segments: pieces.map((p) => ({ start: emRound(p.start), end: emRound(p.end), ...(uniform ? {} : { color: { ...p.color } }), ...(p.blank ? { blank: true } : {}) })),
    audio,
  };
}

// Lets the functions be tested under Node; ignored in the app.
if (typeof module !== "undefined" && module.exports) {
  module.exports = { EM_MIN_SEG, EM_MAX_GAIN, emNew, emSegments, emSegmentAt, emSnapshot, emUndo, emRedo, emSplit, emJoin, emMoveCut, emSetRemoved, emTrimStart, emTrimEnd, emResetCuts, emKept, emKeptDuration, emOutDuration, emSrcToOut, emOutToSrc, emPlayable, EM_COLOR_KEYS, emNoColor, emSameColor, emColorAt, emBlankAt, emSetColor, emSetBlank, emPieces, emRow, emRowCovers, emGainAt, emSetGain, emIsSolo, emSolo, emAddFile, emRemoveRow, emPlaceFile, emAudioUntouched, emIsPlainCut, emChanged, emToOpts };
}
