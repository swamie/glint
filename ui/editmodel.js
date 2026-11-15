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
//   colors     one color correction per segment: { brightness, contrast, saturation, gamma }, 1 = unchanged
//   blank      one flag per segment: true = the picture is black there while the audio plays on
//   speed      export speed, 0.25..4
//   marks      bookmarks: sorted source times, none closer than EM_MARK_GAP. Only there
//              to find a moment again: not part of undo / redo, emChanged or the export,
//              and cutting a part out leaves the bookmarks inside it alone.
//   undo, redo snapshots (JSON strings) for emUndo / emRedo
//   parts      null, or (a clip made by "Edit together") the clips it is made of, in DISPLAY
//              order: [{ src, len, title }] (src = source second where the part starts).
//              Then every time above is a TIMELINE second: the parts laid end to end in
//              display order; emToSrc / emFromSrc convert. Without parts timeline = source.
//              A part border is always a cut, and a locked one (emCutLocked).
//              A part may be a GAP (0.1.36): { src: -1, len, title: "", gap: true }: an empty stretch of the
//              main video row, black and silent from the clip's own tracks in the result (overlays and
//              rows that play through cuts go on over it). A single clip gets parts once it has a gap.
//              m.duration is then the whole row with its gaps; m.source the length of the source.
//   overlays   things on the picture (ui/overlays.js), bottom to top: { id, kind, from, to, ... }
//              kind: text, rect, ellipse, line, arrow, pen, blur. from / to: timeline seconds it
//              is on screen; positions are fractions (0..1) of the picture.
//              An item (and a canvas piece) may have `keys` and `smooth`: keyframes (ui/keyframes.js).
//   fadeIn, fadeOut  seconds of the RESULT that fade from / to black (picture and sound); 0 = off

const EM_MIN_SEG = 0.1; // shortest segment a split or a drag may leave, seconds
const EM_MAX_GAIN = 2;
const EM_EPS = 1e-6;
const EM_MARK_GAP = 0.05;  // two bookmarks are never closer than this, seconds
const EM_MARK_BACK = 0.3;  // "previous bookmark" skips one the playhead is this close after (like a "previous track" button)
const EM_COLOR_KEYS = ["brightness", "contrast", "saturation", "gamma"];
const emNoColor = () => ({ brightness: 1, contrast: 1, saturation: 1, gamma: 1 });

/**
 * A fresh model. `tracks` is the result of the `editor_audio_tracks` command
 * ([{ index, title, path }], possibly empty). The track titled "Mix" is not
 * shown, since it is the ready-made mix of Desktop and Microphone: a clip with
 * Mix, Desktop, Microphone (and Game) shows Desktop, Microphone (and Game).
 * Without a "Mix" title the older rule applies: a 3-track clip hides track 0.
 * Any other clip shows every track it has.
 */
function emNew(duration, tracks) {
  const list = tracks || [];
  const titled = list.length > 1 && list.some((x) => x.title === "Mix");
  const shown = titled ? list.filter((x) => x.title !== "Mix") : list.length === 3 ? list.filter((x) => x.index !== 0) : list;
  return {
    duration: Math.max(0, +duration || 0),
    cuts: [],
    removed: [false],
    audio: shown.map((t) => ({ id: `t${t.index}`, kind: "track", index: t.index, title: t.title || `Track ${t.index + 1}`, path: t.path || "", volume: 1, muted: false, gains: [1] })),
    // The clip's own tracks as the editor first showed them: a row of them can be deleted (emRemoveRow), and
    // this tells which are gone (emTracksGone). Not part of the undo state: it never changes.
    tracks0: shown.map((x) => ({ index: x.index, title: x.title || `Track ${x.index + 1}` })),
    colors: [emNoColor()],
    blank: [false],
    speed: 1,
    marks: [],
    parts: null,
    overlays: [],
    fadeIn: 0,
    fadeOut: 0,
    canvas: null, // null = the clip's own picture; else { width, height, background, pieces } (ui/canvas.js)
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

// The part order is in the snapshot only when there are parts, so a normal clip's snapshots are as before.
// Overlays and fades likewise, only when there are any.
const emHasLayers = (m) => !!((m.overlays && m.overlays.length) || m.fadeIn > 0 || m.fadeOut > 0);
const emState = (m) => JSON.stringify({ cuts: m.cuts, removed: m.removed, audio: m.audio, speed: m.speed, colors: m.colors, blank: m.blank, ...(m.parts ? { parts: m.parts } : {}), ...(emHasLayers(m) ? { ov: m.overlays || [], fi: m.fadeIn || 0, fo: m.fadeOut || 0 } : {}), ...(m.canvas ? { cv: m.canvas } : {}) });
function emRestore(m, json) {
  const s = JSON.parse(json);
  m.overlays = Array.isArray(s.ov) ? s.ov : [];
  m.fadeIn = +s.fi || 0;
  m.fadeOut = +s.fo || 0;
  m.canvas = emCanvasOk(s.cv) ? s.cv : null;
  if (Array.isArray(s.parts) || m.parts) {
    // The bookmarks are not in the snapshot but move with their part: keep each at its place in its part.
    const P0 = emParts(m);
    const pos = (m.marks || []).map((x) => { const p = P0[emPartAt(m, x)]; return p && !p.gap ? { src: p.src, off: x - p.at } : { src: null, off: x }; });
    // A single clip has parts only while it has gaps (0.1.36): a snapshot without them is the clip as it is.
    m.parts = Array.isArray(s.parts) ? s.parts : null;
    m.duration = m.parts ? m.parts.reduce((n, p) => n + p.len, 0) : Number.isFinite(m.source) ? m.source : m.duration;
    const P1 = emParts(m);
    if (m.marks) m.marks = pos.map((q) => { const p = q.src === null ? null : P1.find((r) => !r.gap && r.src === q.src); return p ? Math.min(m.duration, p.at + q.off) : Math.min(m.duration, q.off); }).sort((a, b) => a - b);
  }
  m.cuts = s.cuts;
  m.removed = s.removed;
  m.audio = s.audio;
  m.speed = s.speed;
  m.colors = s.colors;
  m.blank = s.blank;
  emPiecify(m, true); // an undo step from before 0.1.37 (its bookmarks were already moved with the parts above)
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
  // 0.1.37: a split makes two pieces (emSplitAt). Returns the index of the cut between them, or -1.
  if (!emSplitAt(m, t)) return -1;
  return m.cuts.findIndex((c) => Math.abs(c - t) < 1e-6);
}
/** emSplit without the length check: split segment `i` at `t` (the caller has made sure both halves are long enough). */
function emSplitIn(m, i, t) {
  m.cuts.splice(i, 0, t);
  m.removed.splice(i, 0, m.removed[i]);
  m.colors.splice(i, 0, { ...m.colors[i] });
  m.blank.splice(i, 0, m.blank[i]);
  for (const row of m.audio) row.gains.splice(i, 0, row.gains[i]);
  return i;
}

/** Remove cut `c`, merging segments c and c + 1. The merged segment takes the left one's state. A part border (emCutLocked) stays. */
function emJoin(m, c) {
  if (c < 0 || c >= m.cuts.length || emCutLocked(m, c)) return false;
  m.cuts.splice(c, 1);
  m.removed.splice(c + 1, 1);
  m.colors.splice(c + 1, 1);
  m.blank.splice(c + 1, 1);
  for (const row of m.audio) row.gains.splice(c + 1, 1);
  return true;
}

/** Drag cut `c` to `t`, kept between its neighbours. Returns the time used. A part border does not move. */
function emMoveCut(m, c, t) {
  if (emCutLocked(m, c)) return m.cuts[c];
  const lo = (c > 0 ? m.cuts[c - 1] : 0) + EM_MIN_SEG;
  const hi = (c < m.cuts.length - 1 ? m.cuts[c + 1] : m.duration) - EM_MIN_SEG;
  if (hi < lo) return m.cuts[c];
  m.cuts[c] = Math.min(hi, Math.max(lo, t));
  return m.cuts[c];
}

/** Cut segment `i` out of the result, or put it back. The last kept segment cannot be removed. */
function emSetRemoved(m, i, removed) {
  // 0.1.37: cutting out really removes the piece and closes up (emRemovePiece); nothing is ever "put back" here
  // (the footage comes back by pulling a piece's end out, emTrimPiece).
  if (i < 0 || i >= m.removed.length || !removed) return false;
  return emRemovePiece(m, i, false);
}

/** "Start here": everything before `t` is cut out (and the rest moves to the start). */
function emTrimStart(m, t) {
  return emSetTrimStart(m, t, true) !== null;
}

/** "End here": everything from `t` on is cut out. */
function emTrimEnd(m, t) {
  return emSetTrimEnd(m, t, true) !== null;
}

/* ---------- trim brackets ---------- */
// The trim is the leading (and the trailing) run of removed segments: the start bracket sits where
// the first kept segment begins, the end bracket where the last kept one ends. emSetTrimStart /
// emSetTrimEnd move the inner edge of that run to a time:
//   inward   everything the edge passes is cut out and joins the run: kept segments (split at the
//            new edge) and segments already cut out in the middle alike;
//   outward  the run's segments come back in order, each with the state it had (levels, color,
//            hidden video); the segment under the new edge is split, its outer part stays cut out.
// So a part cut out in the middle stays cut out until the edge passes it; once a released drag has
// swallowed it, it is part of the trim and dragging back out gives it back (undo restores it as it was).
// No stray cut points are left: inside the trimmed run, neighbours that look the same are merged,
// and the cut at the old edge is joined when the edge moves outward past it and both sides look the same.

/** The kept span: { start, end } = start of the first kept segment, end of the last one. */
function emTrimBounds(m) {
  const edges = [0, ...m.cuts, m.duration];
  const f = m.removed.indexOf(false), l = m.removed.lastIndexOf(false);
  if (f < 0) return { start: 0, end: m.duration };
  return { start: edges[f], end: edges[l + 1] };
}

/** True if segments `a` and `b` look the same everywhere: cut out or not, color, hidden video, level on every row. */
function emSameSeg(m, a, b) {
  return !!m.removed[a] === !!m.removed[b] && !!m.blank[a] === !!m.blank[b] && emSameColor(m.colors[a], m.colors[b]) &&
    m.audio.every((r) => Math.abs((r.gains[a] ?? 1) - (r.gains[b] ?? 1)) < 1e-9);
}

/** Move the inner edge of the leading removed run to `sec` (see above). Returns the new trim start. */
function emTrimLead(m, sec) {
  const edges = () => [0, ...m.cuts, m.duration];
  let f = m.removed.indexOf(false);
  if (f < 0) return 0; // nothing kept: not a valid model, leave it alone
  const E = edges()[m.removed.lastIndexOf(false) + 1];
  // At least EM_MIN_SEG of the last kept segment stays; no piece shorter than EM_MIN_SEG is made.
  sec = Math.min(E - EM_MIN_SEG, Math.max(0, sec));
  const j = emSegmentAt(m, sec), e0 = edges();
  if (sec - e0[j] < EM_MIN_SEG - EM_EPS) sec = e0[j];
  else if (e0[j + 1] - sec < EM_MIN_SEG - EM_EPS) sec = e0[j + 1];
  const B = e0[f];
  if (Math.abs(sec - B) < EM_EPS) return B;
  if (sec > B) {
    // Inward: split the kept segment under the edge, cut out everything before it.
    let k = emSegmentAt(m, sec);
    if (sec > edges()[k] + EM_EPS && !m.removed[k]) emSplitIn(m, k++, sec);
    for (let i = 0; i < k; i++) m.removed[i] = true;
  } else {
    // Outward: the outer part of the segment under the edge stays cut out, the rest of the run comes back.
    let from = emSegmentAt(m, sec);
    if (sec > edges()[from] + EM_EPS) { emSplitIn(m, from++, sec); f++; }
    for (let i = from; i < f; i++) m.removed[i] = false;
    if (f - 1 >= 0 && emSameSeg(m, f - 1, f)) emJoin(m, f - 1); // the old edge, if nothing tells the two sides apart
  }
  // Merge look-alike neighbours inside the trimmed run (from the right, so the indexes stay valid).
  const f2 = m.removed.indexOf(false);
  for (let c = f2 - 2; c >= 0; c--) if (emSameSeg(m, c, c + 1)) emJoin(m, c);
  return emTrimBounds(m).start;
}

/** The model seen from its end (cuts and per-segment state reversed), for the end bracket; emUnmirror writes it back. */
function emMirror(m) {
  const D = m.duration;
  return { duration: D, _locked: emLockedTimes(m).map((x) => D - x), cuts: m.cuts.map((c) => D - c).reverse(), removed: m.removed.slice().reverse(), colors: m.colors.slice().reverse(), blank: m.blank.slice().reverse(), audio: m.audio.map((r) => ({ gains: r.gains.slice().reverse() })) };
}
function emUnmirror(m, mm, sec) {
  const D = m.duration;
  // D - (D - x) need not be exactly x: put back the exact value of an old cut, or of `sec` for the new one.
  const exact = (x) => m.cuts.find((c) => Math.abs(c - x) < 1e-7) ?? (Math.abs(x - sec) < 1e-7 ? sec : x);
  m.cuts = mm.cuts.map((c) => exact(D - c)).reverse();
  m.removed = mm.removed.slice().reverse();
  m.colors = mm.colors.slice().reverse();
  m.blank = mm.blank.slice().reverse();
  m.audio.forEach((r, i) => { r.gains = mm.audio[i].gains.slice().reverse(); });
}

/**
 * Start bracket: make the kept span begin at source time `sec` (see "trim brackets" above).
 * `sec` is kept inside what is possible (0 .. end of the kept span - EM_MIN_SEG); with `strict`,
 * a time outside that changes nothing and gives null. Returns the new start. Does not snapshot.
 */
function emSetTrimStart(m, sec, strict) {
  // 0.1.37 (pieces): everything before `sec` leaves the row and what is after it moves to the start (the I key).
  // Returns the new start (0), or null when nothing with footage would be left (with `strict`, or always).
  sec = +sec;
  if (!Number.isFinite(sec)) return null;
  emPiecify(m);
  sec = Math.max(0, Math.min(m.duration, sec));
  if (sec <= EM_EPS) return 0;
  const L = emPieceList(m);
  if (!L.some((p) => !p.gap && p.at + p.len > sec + EM_MIN_SEG - EM_EPS)) return strict ? null : 0;
  emSplitAt(m, sec);
  const groups = emGroups(m);
  let acc = 0;
  const order = groups.filter((g) => { const keep = acc + g.part.len / 2 >= sec; acc += g.part.len; return keep; });
  emLayGroups(m, groups, emNormGroups(m, order));
  return 0;
}

/** The O key: everything from `sec` on leaves the row. Returns the new end, or null when nothing with footage would be left. */
function emSetTrimEnd(m, sec, strict) {
  sec = +sec;
  if (!Number.isFinite(sec)) return null;
  emPiecify(m);
  sec = Math.max(0, Math.min(m.duration, sec));
  if (sec >= m.duration - EM_EPS) return m.duration;
  const L = emPieceList(m);
  if (!L.some((p) => !p.gap && p.at < sec - EM_MIN_SEG + EM_EPS)) return strict ? null : m.duration;
  emSplitAt(m, sec);
  const groups = emGroups(m);
  let acc = 0;
  const order = groups.filter((g) => { const keep = acc + g.part.len / 2 < sec; acc += g.part.len; return keep; });
  emLayGroups(m, groups, emNormGroups(m, order));
  return m.duration;
}

/** The segment layout and per-segment state, to put back with emSegPaste (a drag re-applies its change to the state it started from). */
const emSegCopy = (m) => ({ cuts: m.cuts.slice(), removed: m.removed.slice(), colors: m.colors.map((c) => ({ ...c })), blank: m.blank.slice(), gains: m.audio.map((r) => [r.id, r.gains.slice()]) });
function emSegPaste(m, s) {
  m.cuts = s.cuts.slice();
  m.removed = s.removed.slice();
  m.colors = s.colors.map((c) => ({ ...c }));
  m.blank = s.blank.slice();
  const g = new Map(s.gains);
  for (const r of m.audio) r.gains = g.has(r.id) ? g.get(r.id).slice() : m.removed.map(() => 1);
}

/** Back to one whole, kept segment (one per part: the part borders stay). Row levels and files stay; the first segment's color becomes the clip's. */
function emResetCuts(m) {
  const keep = emLockedTimes(m);
  m.cuts = keep.slice();
  m.removed = [false, ...keep.map(() => false)];
  m.colors = m.removed.map(() => ({ ...m.colors[0] }));
  m.blank = m.removed.map(() => false);
  for (const row of m.audio) row.gains = m.removed.map(() => 1);
}

/* ---------- parts ("Edit together") ---------- */
// See `parts` at the top. Every function here is plain identity / empty without parts.

/** Make the parts: `starts` = source second where each part starts (the first is 0), `titles` their names.
 *  Puts a (locked) cut at every border. Does nothing (false) when fewer than two parts result or parts exist. */
function emSetParts(m, starts, titles) {
  if (m.parts) return false;
  const D = m.duration, names = Array.isArray(titles) ? titles : [];
  const list = [];
  (Array.isArray(starts) ? starts : []).forEach((x, k) => {
    x = k === 0 ? 0 : +x;
    if (!Number.isFinite(x)) return;
    const prev = list[list.length - 1];
    if (prev ? x >= prev.x + EM_MIN_SEG && x <= D - EM_MIN_SEG : x === 0) list.push({ x, k });
  });
  if (list.length < 2) return false;
  m.bounds = list.map((p) => p.x); // where each source clip starts: a piece's footage ends there (0.1.37)
  m.parts = list.map((p, j) => ({
    src: p.x,
    len: (j + 1 < list.length ? list[j + 1].x : D) - p.x,
    title: typeof names[p.k] === "string" && names[p.k] ? names[p.k] : `Clip ${p.k + 1}`,
  }));
  for (const at of emLockedTimes(m)) {
    const i = emSegmentAt(m, at), edges = [0, ...m.cuts, D];
    if (Math.abs(edges[i] - at) < 1e-6) m.cuts[i - 1] = at; // a cut is there already: it becomes the border
    else emSplitIn(m, i, at);
  }
  return true;
}

/** The parts in display order: [{ i, at, end, src, len, title }] (at / end: timeline seconds); [] without parts. */
function emParts(m) {
  if (!m.parts) return [];
  let at = 0;
  return m.parts.map((p, i) => { const o = { i, at, end: at + p.len, src: p.src, len: p.len, title: p.title, ...(p.gap ? { gap: true } : {}) }; at += p.len; return o; });
}

/** Timeline seconds of the part borders (where each part after the first starts). */
function emLockedTimes(m) {
  if (m._locked) return m._locked; // the mirror image the end bracket works on (emMirror)
  return emParts(m).slice(1).map((p) => p.at);
}

/** True if cut `c` is a part border: it cannot be removed or dragged. */
function emCutLocked(m, c) {
  const x = m.cuts[c];
  return x !== undefined && emLockedTimes(m).some((L) => Math.abs(L - x) < 1e-6);
}

/** Display index of the part at timeline second `sec` (a border belongs to the part after it), or -1. */
function emPartAt(m, sec) {
  const P = emParts(m);
  if (!P.length || !(sec >= -EM_EPS && sec <= m.duration + EM_EPS)) return -1;
  let i = 0;
  while (i < P.length - 1 && sec >= P[i].end - EM_EPS) i++;
  return i;
}

/** Timeline second -> source second. */
function emToSrc(m, sec) {
  if (!m.parts) return sec;
  const x = Math.min(m.duration, Math.max(0, +sec || 0));
  const P = emParts(m);
  const p = P[emPartAt(m, x)];
  if (p.gap) {
    // A gap has no source: the start of the clip after it (or the end of the one before).
    const next = P.find((q) => !q.gap && q.at >= p.end - EM_EPS), prev = [...P].reverse().find((q) => !q.gap && q.end <= p.at + EM_EPS);
    return next ? next.src : prev ? prev.src + prev.len : 0;
  }
  return p.src + Math.min(p.len, Math.max(0, x - p.at));
}

/** Source second -> timeline second. A source border belongs to the part that starts there. */
function emFromSrc(m, src) {
  if (!m.parts) return src;
  const P = emParts(m).filter((p) => !p.gap);
  if (!P.length) return 0;
  const x = Math.max(0, +src || 0);
  let best = null; // the part with the largest source start at or before x
  for (const p of P) if (p.src <= x + EM_EPS && (!best || p.src > best.src)) best = p;
  best ||= P[0];
  return best.at + Math.min(best.len, Math.max(0, x - best.src));
}

/** The source ranges under timeline range [a, b): [{ tl, src, len }] in timeline order; neighbours that also touch in source are merged.
 *  A gap (0.1.36) is { tl, src: null, len, gap: true }. */
function emSrcSpans(m, a, b) {
  if (!(b > a)) return [];
  if (!m.parts) return [{ tl: a, src: a, len: b - a }];
  const out = [];
  for (const p of emParts(m)) {
    const lo = Math.max(a, p.at), hi = Math.min(b, p.end);
    if (hi - lo <= EM_EPS) continue;
    const last = out[out.length - 1];
    if (p.gap) {
      if (last && last.gap) last.len += hi - lo;
      else out.push({ tl: lo, src: null, len: hi - lo, gap: true });
      continue;
    }
    const s = { tl: lo, src: p.src + (lo - p.at), len: hi - lo };
    if (last && !last.gap && Math.abs(last.src + last.len - s.src) < EM_EPS) last.len += s.len;
    else out.push(s);
  }
  return out;
}

/** Segment indexes inside part `i` (display index). */
function emPartSegs(m, i) {
  const p = emParts(m)[i];
  if (!p) return [];
  return emSegments(m).filter((s) => emPartAt(m, (s.start + s.end) / 2) === i).map((s) => s.i);
}
/** True if every segment of part `i` is cut out. */
const emPartRemoved = (m, i) => { const segs = emPartSegs(m, i); return segs.length > 0 && segs.every((k) => m.removed[k]); };
/** Cut part `i` out (every segment of it), or put it all back. Refused when nothing else would be kept. Does not snapshot. */
function emSetPartRemoved(m, i, removed) {
  const segs = emPartSegs(m, i);
  if (!segs.length) return false;
  if (removed && !m.removed.some((r, k) => !r && !segs.includes(k))) return false;
  for (const k of segs) m.removed[k] = !!removed;
  return true;
}

/**
 * Move the part at display index `from` to display index `to`. Its segments (cuts, cut out,
 * color, hidden video, every row's levels) and bookmarks go with it; file rows stay where they
 * are on the timeline. Returns true if the order changed. Does not snapshot.
 */
function emMovePart(m, from, to) {
  const P = emParts(m), n = P.length;
  if (!n || !Number.isInteger(from) || !Number.isInteger(to) || from < 0 || to < 0 || from >= n || to >= n || from === to) return false;
  const groups = emGroups(m);
  const order = groups.slice();
  order.splice(to, 0, order.splice(from, 1)[0]);
  emLayGroups(m, groups, emNormGroups(m, order));
  return true;
}

/* The main video row as groups (0.1.36): one per part, each with its segments (relative cuts, cut out, color,
 * hidden video, every row's level) and bookmarks. emLayGroups lays a new order of them (old groups, and new gap
 * groups) back onto the model. */
function emGroups(m) {
  const P = emParts(m), segs = emSegments(m);
  const groups = P.map((p) => ({ p, part: m.parts[p.i], cuts: [], segs: [], marks: [] }));
  for (const s of segs) {
    const g = groups[emPartAt(m, (s.start + s.end) / 2)];
    if (g.segs.length) g.cuts.push(s.start - g.p.at);
    g.segs.push({ removed: m.removed[s.i], color: m.colors[s.i], blank: m.blank[s.i], gains: m.audio.map((r) => r.gains[s.i]) });
  }
  for (const x of m.marks || []) { const g = groups[emPartAt(m, x)]; if (g) g.marks.push(x - g.p.at); }
  return groups;
}
/** A new gap group `len` seconds long: one kept segment, neutral, full level. */
const emGapGroup = (m, len) => ({ p: null, part: { src: -1, len, title: "", gap: true }, cuts: [], segs: [{ removed: false, color: emNoColor(), blank: false, gains: m.audio.map(() => 1) }], marks: [] });
/** Tidy an order of groups: gaps next to each other become one, a gap at the very end goes (the time after the
 *  video does that job), slivers of gap go, and a gap is always one kept segment. */
function emNormGroups(m, order) {
  const out = [];
  for (const g of order) {
    if (!g.part.gap) { out.push(g); continue; }
    const last = out[out.length - 1];
    if (last && last.part.gap) out[out.length - 1] = emGapGroup(m, last.part.len + g.part.len);
    else out.push(g.segs.length === 1 && g.cuts.length === 0 && !g.segs[0].removed ? g : emGapGroup(m, g.part.len));
  }
  while (out.length && out[out.length - 1].part.gap) out.pop();
  return out.filter((g) => !g.part.gap || g.part.len >= 0.01);
}
/** Lay `order` (groups of emGroups(m), in any order, and new gap groups) onto the model. An overlay wholly inside
 *  one clip moves with that clip (one across a border or in a gap stays where it is); a canvas piece's keyframes
 *  each go with the part they are in. A single clip without gaps has no parts again. */
function emLayGroups(m, groups, order) {
  const P = groups.map((g) => g.p);
  const newAt = new Map();
  { let a = 0; for (const g of order) { if (g.p) newAt.set(g.p.i, a); a += g.part.len; } }
  for (const o of m.overlays || []) {
    const p = P.find((q) => !q.gap && o.from >= q.at - EM_EPS && o.to <= q.end + EM_EPS);
    if (!p || !newAt.has(p.i)) continue;
    const d = newAt.get(p.i) - p.at;
    o.from += d;
    o.to += d;
    if (Array.isArray(o.keys)) for (const k of o.keys) k.t += d; // its keyframes (keyframes.js) go with it
  }
  for (const pc of m.canvas?.pieces || []) {
    if (!Array.isArray(pc.keys) || !pc.keys.length) continue;
    for (const k of pc.keys) { const p = P[emPartAt(m, k.t)]; if (p && newAt.has(p.i)) k.t += newAt.get(p.i) - p.at; }
    pc.keys.sort((a, b) => a.t - b.t);
  }
  const cuts = [], removed = [], colors = [], blank = [], gains = m.audio.map(() => []), marks = [];
  let at = 0;
  const total = order.reduce((n, g) => n + g.part.len, 0);
  order.forEach((g, k) => {
    if (k > 0) cuts.push(at);
    for (const c of g.cuts) cuts.push(at + c);
    for (const s of g.segs) {
      removed.push(s.removed);
      colors.push(s.color);
      blank.push(s.blank);
      s.gains.forEach((x, r) => gains[r].push(x));
    }
    // A bookmark on the very end of a part that is not last any more would sit on the next part's start: keep it inside its part.
    for (const x of g.marks) marks.push(Math.min(total, at + Math.min(x, g.part.len - (k < order.length - 1 ? EM_MARK_GAP : 0))));
    at += g.part.len;
  });
  // One clip left, from the start of the source, and no gap: back to a plain clip (timeline = source).
  const whole = order.length === 1 && !order[0].part.gap && Math.abs(order[0].part.src) < EM_EPS && Math.abs(order[0].part.len - (Number.isFinite(m.source) ? m.source : order[0].part.len)) < 1e-6;
  m.parts = whole ? null : order.map((g) => g.part);
  m.duration = total;
  Object.assign(m, { cuts, removed, colors, blank });
  m.audio.forEach((r, k) => { r.gains = gains[k]; });
  // Sorted; one at the very end of the clip can land on a border where another starts: keep one of them.
  if (m.marks) m.marks = marks.sort((a, b) => a - b).filter((x, i, l) => i === 0 || x - l[i - 1] >= EM_MARK_GAP - EM_EPS);
}

/** True if the parts are not in source order. */
const emReordered = (m) => !!m.parts && m.parts.some((p, i) => i > 0 && p.src < m.parts[i - 1].src);

/* ---------- gaps and free placement on the main video row (0.1.36) ----------
 * See `parts` at the top. Every function here changes the model only through emGroups / emLayGroups, so the
 * segments, levels, colors, bookmarks, overlays inside a clip and canvas keyframes always go along. None snapshots. */

/** True if the model has a gap. */
const emHasGaps = (m) => !!m.parts && m.parts.some((p) => p.gap);
/** Display index of the gap at timeline second `sec` (its start belongs to it, its end does not), or -1. */
function emGapAt(m, sec) {
  if (!m.parts) return -1;
  for (const p of emParts(m)) if (p.gap && sec >= p.at - EM_EPS && sec < p.end - EM_EPS) return p.i;
  return -1;
}
/** True if segment `i` lies in a gap. */
function emSegIsGap(m, i) {
  if (!m.parts) return false;
  const s = emSegments(m)[i];
  return !!s && emGapAt(m, (s.start + s.end) / 2) >= 0;
}
/** The source's length, remembered before the first change that could hide it (a lifted piece). */
function emKeepSource(m) {
  if (Number.isFinite(m.source)) return m.source;
  m.source = m.parts ? Math.max(0, ...m.parts.filter((p) => !p.gap).map((p) => p.src + p.len)) : m.duration;
  return m.source;
}
/** A single clip becomes one part (named `title`, else the clip's own name m.clipTitle), so it can have gaps. */
function emEnsureParts(m, title) {
  emKeepSource(m);
  const name = typeof title === "string" && title ? title : typeof m.clipTitle === "string" && m.clipTitle ? m.clipTitle : "Clip 1";
  if (!m.parts) m.parts = [{ src: 0, len: m.duration, title: name }];
}

/* ---------- pieces (0.1.37) ----------
 * What lies on the main video row are PIECES: a stretch of the source (src, len) at a place on the timeline, or a
 * gap. A cut really removes (no cut-out stretch stays on the row), a split makes two pieces, each piece moves alone
 * and both its ends can be pulled back out over the footage that was cut away, as far as its source clip goes
 * (emLimits: a single clip's whole length; in a project the library clip it came from, m.bounds).
 * The model keeps one segment per piece: cuts are exactly the borders between parts, nothing is `removed`. A whole,
 * untouched single clip is the one exception: no parts, no cuts. emPiecify turns an older edit (cut-out stretches,
 * cuts inside a clip) into pieces with every time moved to the same moment of the result. */

/** The source seconds where each source clip starts, and the source's end: [0, ..., end]. */
function emClipBounds(m) {
  const end = Number.isFinite(m.source) ? m.source : m.parts ? Math.max(0, ...m.parts.filter((p) => !p.gap).map((p) => p.src + p.len)) : m.duration;
  const list = Array.isArray(m.bounds) && m.bounds.length >= 2 ? m.bounds.filter((x) => Number.isFinite(x) && x > EM_EPS && x < end - EM_EPS) : [];
  return [0, ...list, end];
}
/** The footage limits (source seconds) of a piece that shows source [src, src + len): its source clip's start and end. */
function emLimits(m, src, len) {
  const B = emClipBounds(m), mid = src + Math.max(0, len) / 2;
  let k = 0;
  while (k < B.length - 2 && mid >= B[k + 1]) k++;
  return { lo: B[k], hi: B[k + 1] };
}
/** True if the model is not in pieces form yet (cut-out stretches, or cuts inside a piece). */
function emNeedsPieces(m) {
  if (m.removed.some(Boolean)) return true;
  return m.parts ? m.cuts.length !== m.parts.length - 1 : m.cuts.length > 0;
}
/** The row in order: [{ i, at, len, src, lo, hi, gap, title }] (a gap has src -1, lo = hi = 0). */
function emPieceList(m) {
  if (!m.parts) return [{ i: 0, at: 0, len: m.duration, src: 0, lo: 0, hi: emClipBounds(m)[1] ?? m.duration, gap: false, title: m.clipTitle || "" }];
  return emParts(m).map((p) => (p.gap ? { i: p.i, at: p.at, len: p.len, src: -1, lo: 0, hi: 0, gap: true, title: "" } : { i: p.i, at: p.at, len: p.len, src: p.src, ...emLimits(m, p.src, p.len), gap: false, title: p.title }));
}
/** Index of the piece or gap under timeline second `time` (a border belongs to the piece after it), or -1 after the end. */
function emPieceAt(m, time) {
  if (!(time >= -EM_EPS && time < m.duration - EM_EPS)) return -1;
  return m.parts ? emPartAt(m, time) : 0;
}
/** Split the piece under `time` in two there (both at least EM_MIN_SEG). True if it did. Does not snapshot. */
function emSplitAt(m, time) {
  emPiecify(m);
  const L = emPieceList(m), p = L[emPieceAt(m, time)];
  if (!p || p.gap || time - p.at < EM_MIN_SEG - EM_EPS || p.at + p.len - time < EM_MIN_SEG - EM_EPS) return false;
  emEnsureParts(m);
  return emSplitPartAt(m, time);
}
/** Remove piece `i`: what follows moves left (leaveGap false), or an empty stretch stays (true). A gap removed closes.
 *  Refused when no other piece with footage would be left. Does not snapshot. */
function emRemovePiece(m, i, leaveGap) {
  emPiecify(m);
  const L = emPieceList(m), p = L[i];
  if (!p) return false;
  if (p.gap) return leaveGap ? false : emCloseGap(m, i);
  if (!L.some((q) => !q.gap && q.i !== i)) return false;
  emEnsureParts(m);
  const groups = emGroups(m), order = groups.slice();
  if (leaveGap) order[i] = emGapGroup(m, p.len); else order.splice(i, 1);
  emLayGroups(m, groups, emNormGroups(m, order));
  return true;
}
/**
 * Move one end of piece `i` ("start" | "end") to timeline second `time`. Inward it trims (an empty stretch stays
 * where the footage was, nothing else moves); outward it brings back footage that was cut away, as far as the
 * source clip has any (emLimits) and no further. Pulling out uses an empty stretch beside the piece first, after
 * that what follows moves later; nothing is overwritten. Returns the piece's new { at, len, src }, or null.
 * Does not snapshot.
 */
function emTrimPiece(m, i, side, time) {
  emPiecify(m);
  const L = emPieceList(m), p = L[i];
  if (!p || p.gap || !Number.isFinite(time) || (side !== "start" && side !== "end")) return null;
  emEnsureParts(m);
  const groups = emGroups(m), order = groups.slice(), g = groups[i];
  let part, delta;
  if (side === "end") {
    const len = Math.min(p.hi - p.src, Math.max(EM_MIN_SEG, time - p.at));
    delta = len - p.len;
    part = { ...g.part, len };
  } else {
    const src = Math.max(p.lo, Math.min(p.src + p.len - EM_MIN_SEG, p.src + (time - p.at)));
    delta = p.src - src;
    part = { ...g.part, src, len: p.len + delta };
  }
  if (Math.abs(delta) < 1e-9) return { at: p.at, len: p.len, src: p.src };
  order[i] = { ...g, part };
  const k = side === "end" ? i + 1 : i - 1, nb = order[k], gapLen = nb && nb.part.gap ? nb.part.len : 0;
  if (delta > 0) {
    // Pulled out: the empty stretch beside it first; what is still missing moves the rest along.
    if (gapLen > 0) { const left = gapLen - Math.min(delta, gapLen); if (left < 0.01) order.splice(k, 1); else order[k] = emGapGroup(m, left); }
  } else if (gapLen > 0) order[k] = emGapGroup(m, gapLen - delta); // trimmed: the stretch beside it grows
  else order.splice(side === "end" ? i + 1 : i, 0, emGapGroup(m, -delta)); // ... or appears
  emLayGroups(m, groups, emNormGroups(m, order));
  const j = m.parts ? m.parts.indexOf(part) : 0, q = emPieceList(m)[j < 0 ? 0 : j];
  return q ? { at: q.at, len: q.len, src: q.src } : null;
}
/** What the media code needs to put piece `i` on the picture as a clip item: { off, len, at } (source seconds of the
 *  working file / clip, its length and its place on the timeline), or null for a gap. */
function emPieceToItem(m, i) {
  const p = emPieceList(m)[i];
  return p && !p.gap ? { off: p.src, len: p.len, at: p.at } : null;
}
/**
 * An edit from before 0.1.37 (cut-out stretches lying on the row, cuts inside a clip) as pieces: every kept stretch
 * becomes a piece where it was in the result, cut-out stretches leave the row (their footage can be pulled back out
 * of the piece ends), and every time kept on the old timeline (overlay items and their keyframes, canvas
 * keyframes, file rows, bookmarks) moves to the same moment of the result. The export stays the same. A row that
 * was cut with the video across a cut-out stretch becomes one row per stretch it is heard in. True if it changed.
 */
function emPiecify(m, keepMarks) {
  if (!emNeedsPieces(m)) return false;
  emKeepSource(m);
  const old = { duration: m.duration, cuts: m.cuts.slice(), removed: m.removed.slice() };
  const newT = (x) => emSrcToOut(old, x);
  const segs = emSegments(old);
  // File rows: through rows keep their place in the result; a row cut with the video is split where it was cut.
  const extra = [];
  for (const r of m.audio) {
    if (r.kind !== "file") continue;
    if (r.through) { r.offset = newT(r.offset); continue; }
    const a = r.offset, b = r.offset + r.length, spans = [];
    for (const s of segs) {
      if (s.removed) continue;
      const lo = Math.max(a, s.start), hi = Math.min(b, s.end);
      if (hi - lo <= EM_EPS) continue;
      const last = spans[spans.length - 1];
      if (last && Math.abs(last.hi - lo) < EM_EPS) last.hi = hi; else spans.push({ lo, hi });
    }
    const t0 = r.trimStart;
    if (!spans.length) { r.offset = newT(a); r.length = EM_MIN_SEG; continue; }
    spans.forEach((sp, k) => {
      const row = k === 0 ? r : { ...r, id: `${r.id}s${k}` };
      Object.assign(row, { offset: newT(sp.lo), trimStart: t0 + (sp.lo - a), length: sp.hi - sp.lo });
      if (k > 0) extra.push({ row, from: r });
    });
  }
  for (const o of m.overlays || []) {
    o.from = newT(o.from);
    o.to = newT(o.to);
    if (Array.isArray(o.keys)) for (const k of o.keys) if (k && Number.isFinite(k.t)) k.t = newT(k.t);
  }
  for (const pc of m.canvas?.pieces || []) if (Array.isArray(pc.keys)) for (const k of pc.keys) if (k && Number.isFinite(k.t)) k.t = newT(k.t);
  const marks = keepMarks ? (m.marks || []).slice() : (m.marks || []).map(newT);
  if (!m.parts) m.parts = [{ src: 0, len: m.duration, title: m.clipTitle || "Clip 1" }];
  const groups = emGroups(m), order = [];
  for (const g of groups) {
    const rel = [0, ...g.cuts, g.part.len];
    g.segs.forEach((s, k) => {
      if (s.removed) return;
      const len = rel[k + 1] - rel[k];
      if (g.part.gap) order.push(emGapGroup(m, len));
      else order.push({ p: null, part: { ...g.part, src: g.part.src + rel[k], len }, cuts: [], segs: [{ ...s, removed: false }], marks: [] });
    });
  }
  emLayGroups(m, groups, emNormGroups(m, order));
  for (const x of extra) m.audio.splice(m.audio.indexOf(x.from) + 1 + extra.filter((y) => y.from === x.from).indexOf(x), 0, { ...x.row, gains: x.from.gains.slice() });
  if (m.marks || marks.length) m.marks = marks.map((x) => Math.min(m.duration, x)).sort((a, b) => a - b).filter((x, i, l) => i === 0 || x - l[i - 1] >= EM_MARK_GAP - EM_EPS);
  return true;
}
/** Split the clip part under timeline second `sec` in two there (both keep its title; the cut there becomes a part
 *  border). False on a gap, at a border or outside. */
function emSplitPartAt(m, sec) {
  const k = emPartAt(m, sec), p = emParts(m)[k];
  if (!p || p.gap || sec - p.at < EM_EPS * 10 || p.end - sec < EM_EPS * 10) return false;
  const i = emSegmentAt(m, sec), edges = [0, ...m.cuts, m.duration];
  if (Math.abs(edges[i] - sec) > 1e-6) emSplitIn(m, i, sec);
  const a = m.parts[k], len1 = sec - p.at;
  m.parts.splice(k, 1, { ...a, len: len1 }, { ...a, src: a.src + len1, len: a.len - len1 });
  return true;
}
/**
 * Free placement: put clip part `i` (display index) so it starts at timeline second `at`. Where it was stays empty
 * (a gap). Dropped on a gap or after the end it sits exactly there; when it is longer than the gap, what follows
 * moves later (nothing is overwritten). Dropped on a clip it goes beside it (the nearer side). `title` names the clip of a
 * single clip that gets parts. True if the layout changed.
 */
function emPlacePart(m, i, at, title) {
  at = Math.max(0, +at || 0);
  if (!m.parts) { if (i !== 0) return false; emEnsureParts(m, title); }
  emKeepSource(m);
  const before = JSON.stringify([m.parts, m.cuts]);
  const groups = emGroups(m), g = groups[i];
  if (!g || g.part.gap) return false;
  let order = groups.slice();
  order[i] = emGapGroup(m, g.part.len);
  // Empty stretches next to each other are one (the place it leaves joins the gaps beside it).
  order = order.reduce((out, x) => { const last = out[out.length - 1]; if (last && last.part.gap && x.part.gap) out[out.length - 1] = emGapGroup(m, last.part.len + x.part.len); else out.push(x); return out; }, []);
  let acc = 0, k = 0;
  for (; k < order.length; k++) { if (at < acc + order[k].part.len - EM_EPS) break; acc += order[k].part.len; }
  if (k >= order.length) {
    if (at - acc > EM_EPS) order.push(emGapGroup(m, at - acc));
    order.push(g);
  } else if (order[k].part.gap) {
    const G = order[k], lead = at - acc, rest = Math.max(0, acc + G.part.len - (at + g.part.len));
    order.splice(k, 1, ...(lead > EM_EPS ? [emGapGroup(m, lead)] : []), g, ...(rest > EM_EPS ? [emGapGroup(m, rest)] : []));
  } else order.splice(at >= acc + order[k].part.len / 2 ? k + 1 : k, 0, g); // on a clip: beside it, the nearer side
  emLayGroups(m, groups, emNormGroups(m, order));
  return JSON.stringify([m.parts, m.cuts]) !== before;
}
/** Reorder: clip part `i` goes right before (or `after`) clip part `k`; the place it came from closes. True if it moved. */
function emPutBeside(m, i, k, after) {
  const P = emParts(m);
  if (!P[i] || !P[k] || P[i].gap || P[k].gap || i === k) return false;
  const groups = emGroups(m), order = groups.slice(), g = order.splice(i, 1)[0];
  const j = order.indexOf(groups[k]) + (after ? 1 : 0);
  order.splice(j, 0, g);
  if (order.every((x, n) => x === groups[n])) return false;
  emKeepSource(m);
  emLayGroups(m, groups, emNormGroups(m, order));
  return true;
}
/** Close gap `i` (display index): everything after it moves earlier. */
function emCloseGap(m, i) {
  const P = emParts(m);
  if (!P[i] || !P[i].gap) return false;
  const groups = emGroups(m);
  emLayGroups(m, groups, emNormGroups(m, groups.filter((_, n) => n !== i)));
  return true;
}
/** Close every gap. */
function emCloseGaps(m) {
  if (!emHasGaps(m)) return false;
  const groups = emGroups(m);
  emLayGroups(m, groups, emNormGroups(m, groups.filter((g) => !g.part.gap)));
  return true;
}
/** Make gap `i` `len` seconds long (what follows moves along); under 0.01 s it closes. */
function emSetGapLen(m, i, len) {
  const P = emParts(m);
  if (!P[i] || !P[i].gap || !Number.isFinite(len)) return false;
  if (len < 0.01) return emCloseGap(m, i);
  const groups = emGroups(m), order = groups.slice();
  order[i] = emGapGroup(m, len);
  emLayGroups(m, groups, emNormGroups(m, order));
  return true;
}
/** Add a gap `len` seconds long at timeline second `at` (a clip there is split; at a border it goes between). */
function emInsertGap(m, at, len, title) {
  if (!(len >= 0.01) || !(at >= 0) || at >= m.duration - EM_EPS) return false;
  emEnsureParts(m, title);
  emSplitPartAt(m, at);
  const groups = emGroups(m), order = groups.slice();
  let k = 0, acc = 0;
  while (k < order.length && acc + order[k].part.len <= at + EM_EPS) acc += order[k++].part.len;
  order.splice(k, 0, emGapGroup(m, len));
  emLayGroups(m, groups, emNormGroups(m, order));
  return true;
}
/** "Cut out and leave a gap": segment `i` leaves the row and an empty stretch as long as it stays. Refused on a gap
 *  and when no other kept picture would be left. */
function emLiftSeg(m, i, title) {
  const s = emSegments(m)[i];
  if (!s || emSegIsGap(m, i)) return false;
  if (!m.removed.some((r, k) => k !== i && !r && !emSegIsGap(m, k))) return false;
  emEnsureParts(m, title);
  emSplitPartAt(m, s.start);
  emSplitPartAt(m, s.end);
  const k = emPartAt(m, (s.start + s.end) / 2);
  const groups = emGroups(m), order = groups.slice();
  order[k] = emGapGroup(m, groups[k].part.len);
  emLayGroups(m, groups, emNormGroups(m, order));
  return true;
}

/**
 * Split overlay item `o` at timeline second `at` (the blade): `o` ends there and a copy with id `newId` starts there.
 * A media item's second half starts `off` later; keyframes go to the half they lie in, and each half gets one at
 * the cut with the values there, so nothing jumps. Returns the new item (not yet in m.overlays), or null.
 */
function emSplitOverlay(o, at, newId) {
  if (!o || !(at - o.from >= EM_OV_MIN - EM_EPS) || !(o.to - at >= EM_OV_MIN - EM_EPS)) return null;
  const b = JSON.parse(JSON.stringify(o));
  b.id = newId;
  b.from = at;
  if (Number.isFinite(o.off)) b.off = Math.round((o.off + (at - o.from)) * 1000) / 1000;
  if (Array.isArray(o.keys) && o.keys.length) {
    const cut = emKeyAt(o.keys, at, !!o.smooth);
    o.keys = [...o.keys.filter((k) => k.t < at - 1e-6), cut];
    b.keys = [{ ...cut }, ...b.keys.filter((k) => k.t > at + 1e-6)];
  }
  o.to = at;
  return b;
}
/** A keyframe at `at` with every number of the keys between which it lies (held before the first, after the last). */
function emKeyAt(keys, at, smooth) {
  const L = keys.slice().sort((a, b) => a.t - b.t), n = L.length;
  const pick = (k) => ({ ...k, t: at });
  if (at <= L[0].t) return pick(L[0]);
  if (at >= L[n - 1].t) return pick(L[n - 1]);
  let i = 0;
  while (i < n - 2 && at >= L[i + 1].t) i++;
  const a = L[i], b = L[i + 1], span = b.t - a.t;
  let u = span > 1e-9 ? Math.min(1, Math.max(0, (at - a.t) / span)) : 1;
  if (smooth) u = u * u * (3 - 2 * u);
  const out = { ...a, t: at };
  for (const f of Object.keys(a)) if (f !== "t" && typeof a[f] === "number" && typeof b[f] === "number") out[f] = a[f] + (b[f] - a[f]) * u;
  return out;
}

/* ---------- editor projects: more clips, saved state ---------- */
// A project's draft grows when clips are added: the backend joins [old draft, new clips], so every
// source second of the old draft keeps its place and the new clips start at its end.

/** The new parts emAddParts would append: [{ src, len, title }] (empty if none fit). */
function emNewParts(D, starts, titles, end) {
  const list = [], names = Array.isArray(titles) ? titles : [];
  (Array.isArray(starts) ? starts : []).forEach((x, k) => {
    x = +x;
    if (!Number.isFinite(x)) return;
    const prev = list[list.length - 1];
    // The first new clip starts where the old draft ends; a few ms of difference between the two
    // measurements is snapped away so the parts keep tiling the source.
    if (!prev && Math.abs(x - D) < 0.25) x = D;
    if (prev ? x >= prev.src + EM_MIN_SEG : x >= D - 0.25) { if (x <= end - EM_MIN_SEG) list.push({ src: x, k }); }
  });
  if (!list.length && end - D >= EM_MIN_SEG) list.push({ src: D, k: 0 });
  return list.map((p, j) => ({ src: p.src, len: (j + 1 < list.length ? list[j + 1].src : end) - p.src, title: typeof names[p.k] === "string" && names[p.k] ? names[p.k] : `Clip ${p.k + 1}` }));
}
/** Append `parts` after timeline second D on a model-like object (the model or an undo snapshot). */
function emAppendTo(o, D, parts, firstTitle) {
  if (!Array.isArray(o.parts) || !o.parts.length) o.parts = [{ src: 0, len: D, title: firstTitle }];
  // With gaps (0.1.36) the row is longer than the source: the new clips go after its end.
  let at = o.parts.reduce((n, p) => n + p.len, 0);
  for (const p of parts) {
    o.cuts.push(at);
    o.removed.push(false);
    o.colors.push(emNoColor());
    o.blank.push(false);
    for (const r of o.audio) r.gains.push(1);
    o.parts.push({ src: p.src, len: p.len, title: p.title });
    at += p.len;
  }
  return at;
}

/**
 * The project's draft got longer: clips were added at its end. `starts` = source second where each
 * new clip starts (the first is the old draft's length), `titles` their names, `end` = the new
 * draft's length. The new clips become parts at the END of the display order, each one kept, with
 * neutral color and full level on every row; nothing that was there moves (cuts, order, levels,
 * file rows, bookmarks). A model without parts first becomes one part (its title: `firstTitle`).
 * Undo snapshots get the same new parts, so undo still works across the add. Returns true if
 * parts were added. Does not snapshot.
 */
function emAddParts(m, starts, titles, end, firstTitle) {
  const D = Number.isFinite(m.source) ? m.source : m.duration; // the old draft's length (the row may be longer with gaps)
  end = +end;
  if (!Number.isFinite(end) || end - D < EM_MIN_SEG) return false;
  const parts = emNewParts(D, starts, titles, end);
  if (!parts.length) return false;
  const first = typeof firstTitle === "string" && firstTitle ? firstTitle : "Clip 1";
  const fix = (json) => {
    try { const s = JSON.parse(json); emAppendTo(s, D, parts, first); return JSON.stringify(s); } catch { return null; }
  };
  m.undo = m.undo.map(fix).filter(Boolean);
  m.redo = m.redo.map(fix).filter(Boolean);
  m.bounds = [...(Array.isArray(m.bounds) ? m.bounds : [0]), ...parts.map((p) => p.src)];
  m.duration = emAppendTo(m, D, parts, first);
  if (Number.isFinite(m.source)) m.source = end;
  return true;
}

const EM_STATE_V = 1;
/**
 * The edit as plain data, to save with a project and give back to emStateLoad: cuts, order, cut-out
 * parts, levels, added audio rows, colors, hidden video, speed, bookmarks (timeline seconds), and
 * `joins` (where each source clip after the first starts, as the caller passes them). `undo` keeps
 * the undo history too (only for a reload in the same session; it is not worth saving to disk).
 */
function emStateSave(m, joins, undo) {
  const s = {
    // `duration` is the SOURCE's length (the project's draft); with gaps (0.1.36) the row is the parts' sum.
    v: EM_STATE_V, duration: Number.isFinite(m.source) ? m.source : m.duration, joins: Array.isArray(joins) ? joins.slice() : [],
    cuts: m.cuts, removed: m.removed, colors: m.colors, blank: m.blank, speed: m.speed,
    audio: m.audio.map((r) => (r.kind === "track" ? { id: r.id, kind: "track", index: r.index, title: r.title, volume: r.volume, muted: r.muted, gains: r.gains } : r)),
    parts: m.parts, marks: m.marks || [],
    overlays: m.overlays || [], fadeIn: m.fadeIn || 0, fadeOut: m.fadeOut || 0,
    canvas: m.canvas || null,
    gone: emTracksGone(m).map((x) => x.title), // the clip's own tracks that were deleted, by title
  };
  if (undo) { s.undo = m.undo.slice(); s.redo = m.redo.slice(); }
  return JSON.parse(JSON.stringify(s));
}

/** True if `s` is a state emStateLoad can use (shape and numbers checked; nothing else trusted). */
function emStateOk(s) {
  const num = (x) => typeof x === "number" && Number.isFinite(x);
  if (!s || typeof s !== "object" || s.v !== EM_STATE_V || !num(s.duration) || s.duration <= 0) return false;
  const D = s.duration, n = Array.isArray(s.cuts) ? s.cuts.length + 1 : 0;
  // The row's length: the source's, or with parts their sum (gaps included, 0.1.36).
  const T = Array.isArray(s.parts) && s.parts.length && s.parts.every((p) => p && num(p.len)) ? s.parts.reduce((a, p) => a + p.len, 0) : D;
  if (!n || !s.cuts.every((c, i) => num(c) && c > 0 && c < T && (i === 0 || c > s.cuts[i - 1]))) return false;
  if (!Array.isArray(s.removed) || s.removed.length !== n || !s.removed.some((r) => !r)) return false;
  if (!Array.isArray(s.blank) || s.blank.length !== n) return false;
  if (!Array.isArray(s.colors) || s.colors.length !== n || !s.colors.every((c) => c && typeof c === "object" && EM_COLOR_KEYS.every((k) => num(c[k])))) return false;
  if (!num(s.speed) || s.speed < 0.25 || s.speed > 4) return false;
  if (!Array.isArray(s.joins) || !s.joins.every(num)) return false;
  if (!Array.isArray(s.audio) || !s.audio.every((r) => r && (r.kind === "track" || r.kind === "file") && Array.isArray(r.gains) && r.gains.length === n && r.gains.every(num) && num(r.volume)
    && (r.kind === "track" || (typeof r.path === "string" && r.path && num(r.offset) && num(r.trimStart) && num(r.length) && num(r.fileDuration) && typeof r.id === "string")))) return false;
  if (s.parts != null) {
    if (!Array.isArray(s.parts) || s.parts.length < 1 || !s.parts.every((p) => p && num(p.src) && num(p.len) && p.len > 0 && typeof p.title === "string")) return false;
    // Without gaps or lifted pieces the parts tile the source exactly (as before 0.1.36); else each clip lies in it.
    const clips = s.parts.filter((p) => !p.gap);
    if (!clips.length || !clips.every((p) => p.src >= -1e-3 && p.src + p.len <= D + 1e-3)) return false;
    if (!s.parts.some((p) => p.gap) && Math.abs(T - D) > 1e-3 && !(T < D)) return false;
  }
  // Overlays and fades (0.1.25) are optional: a state saved before them has none.
  if (s.overlays != null && !(Array.isArray(s.overlays) && s.overlays.every(emOverlayOk))) return false;
  if ((s.fadeIn != null && !(num(s.fadeIn) && s.fadeIn >= 0)) || (s.fadeOut != null && !(num(s.fadeOut) && s.fadeOut >= 0))) return false;
  // The canvas (0.1.26) is optional too.
  if (s.canvas != null && !emCanvasOk(s.canvas)) return false;
  return true;
}
/** A canvas as saved or sent: a size, a background and pieces whose eight numbers are finite. */
function emCanvasOk(c) {
  const num = (x) => typeof x === "number" && Number.isFinite(x);
  return !!c && typeof c === "object" && num(c.width) && num(c.height) && c.width >= 16 && c.height >= 16 && c.width <= 4096 && c.height <= 4096 && typeof c.background === "string"
    && Array.isArray(c.pieces) && c.pieces.every((p) => p && typeof p === "object" && ["sx", "sy", "sw", "sh", "x", "y", "w", "h"].every((k) => num(p[k])) && p.sw > 0 && p.sh > 0 && p.w > 0 && p.h > 0);
}
/** The canvas for export_clip: the size, the background and each piece's two boxes. */
function emCanvasOpts(c) {
  const r5 = (x) => Math.round(x * 100000) / 100000;
  return { width: Math.round(c.width), height: Math.round(c.height), background: c.background, pieces: c.pieces.map((p) => ({ sx: r5(p.sx), sy: r5(p.sy), sw: r5(p.sw), sh: r5(p.sh), x: r5(p.x), y: r5(p.y), w: r5(p.w), h: r5(p.h) })) };
}
/** An overlay as saved: an object with a known kind and a finite time range (its own fields are checked by overlays.js). */
const EM_OVERLAY_KINDS = ["text", "rect", "ellipse", "line", "arrow", "pen", "blur", "image", "gif", "video"]; // the last three: ui/media.js (0.1.35)
const emOverlayOk = (o) => !!o && typeof o === "object" && EM_OVERLAY_KINDS.includes(o.kind) && typeof o.id === "string" && Number.isFinite(o.from) && Number.isFinite(o.to);

/**
 * Put saved state `s` (emStateSave) on `m`, a fresh model of the project's draft (emNew with the
 * draft's tracks). The draft may since have grown (the caller then calls emAddParts for the new
 * clips), never shrunk. Track rows are matched by title (the draft's track list can change when a
 * clip with another track joins); a track the state does not know starts at full level. File rows
 * come back as saved. Returns false, and leaves `m` as it was, when `s` cannot be used.
 */
function emStateLoad(m, s) {
  if (typeof s === "string") { try { s = JSON.parse(s); } catch { return false; } }
  if (!emStateOk(s) || s.duration > m.duration + 0.5) return false;
  const n = s.cuts.length + 1;
  const saved = s.audio.filter((r) => r.kind === "track");
  const used = new Set();
  const find = (row) => {
    let k = saved.findIndex((r, i) => !used.has(i) && r.title && r.title === row.title);
    if (k < 0) k = saved.findIndex((r, i) => !used.has(i) && !r.title && r.index === row.index);
    if (k >= 0) used.add(k);
    return k >= 0 ? saved[k] : null;
  };
  const tracks = m.audio.filter((r) => r.kind === "track").map((row) => {
    const old = find(row);
    return { ...row, volume: old ? Math.min(EM_MAX_GAIN, Math.max(0, old.volume)) : 1, muted: old ? !!old.muted : false, gains: old ? old.gains.slice() : Array(n).fill(1) };
  });
  // Tracks deleted in the saved edit stay deleted (they are known by title, like the rows).
  const gone = Array.isArray(s.gone) ? s.gone.filter((x) => typeof x === "string") : [];
  for (let i = tracks.length - 1; i >= 0; i--) if (gone.includes(tracks[i].title)) tracks.splice(i, 1);
  const sameTracks = used.size === saved.length && tracks.length === saved.length;
  const files = s.audio.filter((r) => r.kind === "file").map((r) => ({ ...r, gains: r.gains.slice() }));
  // With gaps or a lifted piece (0.1.36) the parts say how long the row is; `duration` stays the source's.
  const whole = Array.isArray(s.parts) && s.parts.length === 1 && !s.parts[0].gap && Math.abs(s.parts[0].src) < 1e-6 && Math.abs(s.parts[0].len - s.duration) < 1e-3;
  m.parts = Array.isArray(s.parts) && s.parts.length >= 1 && !whole ? s.parts.map((p) => (p.gap ? { src: -1, len: p.len, title: "", gap: true } : { src: p.src, len: p.len, title: p.title })) : null;
  m.duration = m.parts ? m.parts.reduce((a, p) => a + p.len, 0) : s.duration;
  if (m.parts && (m.parts.some((p) => p.gap) || Math.abs(m.duration - s.duration) > 1e-6)) m.source = s.duration;
  else delete m.source;
  m.cuts = s.cuts.slice();
  m.removed = s.removed.map((r) => !!r);
  m.colors = s.colors.map((c) => ({ ...emNoColor(), ...c }));
  m.blank = s.blank.map((b) => !!b);
  m.speed = s.speed;
  m.audio = [...tracks, ...files];
  m.overlays = Array.isArray(s.overlays) ? JSON.parse(JSON.stringify(s.overlays)) : [];
  m.fadeIn = +s.fadeIn || 0;
  m.fadeOut = +s.fadeOut || 0;
  m.canvas = s.canvas ? JSON.parse(JSON.stringify(s.canvas)) : null;
  emSetMarks(m, s.marks);
  // The undo history only when the rows are the same ones (their paths are refreshed).
  const keep = (list) => (sameTracks && Array.isArray(list) ? list.filter((x) => typeof x === "string").map((json) => {
    try {
      const o = JSON.parse(json);
      o.audio = o.audio.map((r) => { if (r.kind !== "track") return r; const now = tracks.find((x) => x.title === r.title && (r.title || x.index === r.index)); return now ? { ...r, id: now.id, index: now.index, path: now.path } : r; });
      return JSON.stringify(o);
    } catch { return null; }
  }).filter(Boolean) : []);
  m.undo = keep(s.undo);
  m.redo = keep(s.redo);
  // 0.1.37: where each of the project's clips starts in its working file (a piece's footage ends there), and an
  // edit saved before pieces made into pieces (cut-out stretches leave the row, every time keeps its moment).
  if (Array.isArray(s.joins) && s.joins.length) m.bounds = [0, ...s.joins.filter((x) => x > EM_EPS && x < s.duration - EM_EPS)];
  emPiecify(m);
  return true;
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
  // After the video (0.1.35): always kept, after everything kept.
  if (t > m.duration) out += t - m.duration;
  return out;
}

/** Position in the finished clip (speed not applied) -> source time. Past everything kept: the time after the video. */
function emOutToSrc(m, out) {
  const kept = emKept(m);
  for (const r of kept) {
    const len = r.end - r.start;
    if (out <= len + EM_EPS) return r.start + Math.max(0, out);
    out -= len;
  }
  if (out > EM_EPS) return m.duration + out;
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

/* ---------- picture: color and hiding, per segment ---------- */

const emSameColor = (a, b = emNoColor()) => EM_COLOR_KEYS.every((k) => Math.abs((a?.[k] ?? 1) - (b?.[k] ?? 1)) < 0.005);

/** The color correction in force at source time `t`. */
const emColorAt = (m, t) => m.colors[emSegmentAt(m, t)] || emNoColor();
/** True if the picture is hidden (black) at source time `t`. */
const emBlankAt = (m, t) => !!m.blank[emSegmentAt(m, t)];

/**
 * Change the color correction of some segments. `segs`: segment indexes, or
 * null for every segment (the whole clip). `change`: any of the four color
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

/* ---------- bookmarks ---------- */
// m.marks is created on demand, so a model made before bookmarks existed works too.

/** Add a bookmark at source time `sec` (kept inside the clip). Returns its index, or -1 if one is already within EM_MARK_GAP. */
function emAddMark(m, sec) {
  if (!Number.isFinite(+sec)) return -1;
  const at = Math.min(m.duration, Math.max(0, +sec));
  const marks = (m.marks ||= []);
  if (marks.some((x) => Math.abs(x - at) < EM_MARK_GAP - EM_EPS)) return -1;
  let i = 0;
  while (i < marks.length && marks[i] < at) i++;
  marks.splice(i, 0, at);
  return i;
}
function emRemoveMark(m, i) {
  if (!m.marks || !(i >= 0 && i < m.marks.length)) return false;
  m.marks.splice(i, 1);
  return true;
}
/** Replace every bookmark with `list` (e.g. loaded from storage): sorted, inside the clip, too-close ones dropped. */
function emSetMarks(m, list) {
  m.marks = [];
  const num = (x) => (typeof x === "number" ? x : typeof x === "string" ? parseFloat(x) : NaN); // not Number(): null would be 0
  const ok = (Array.isArray(list) ? list : []).map(num).filter((x) => Number.isFinite(x) && x >= 0 && x <= m.duration + EM_EPS).sort((a, b) => a - b);
  for (const x of ok) if (!m.marks.length || x - m.marks[m.marks.length - 1] >= EM_MARK_GAP - EM_EPS) m.marks.push(Math.min(x, m.duration));
  return m.marks.length;
}
/** Index of the bookmark nearest `sec` within `tol` seconds, or -1. */
function emMarkNear(m, sec, tol) {
  let best = -1, dist = tol;
  (m.marks || []).forEach((x, i) => { const d = Math.abs(x - sec); if (d <= dist) { dist = d; best = i; } });
  return best;
}
/** Time of the first bookmark after `sec` (one the playhead sits on does not count), or null. */
function emNextMark(m, sec) {
  for (const x of m.marks || []) if (x > sec + 0.01) return x;
  return null;
}
/** Time of the last bookmark before `sec`, skipping one the playhead is on or less than EM_MARK_BACK after; or null. */
function emPrevMark(m, sec) {
  const list = m.marks || [];
  for (let i = list.length - 1; i >= 0; i--) if (list[i] < sec - EM_MARK_BACK) return list[i];
  return null;
}

/* ---------- audio ---------- */

const emRow = (m, id) => m.audio.find((r) => r.id === id) || null;

/* An added file "plays through" cuts (row.through, 0.1.34; every file added since): it starts at timeline time
 * `offset` and then plays `length` seconds of the RESULT without a break, whatever is cut out of the video under
 * it. On the timeline it therefore pauses under a cut-out part and ends later (emRowEnd). A row without the flag
 * (older projects, or switched by hand) is laid on the timeline and cut with the video, as before. */
/** Where a row's block ends on the timeline. */
const emRowEnd = (m, row) => (row.kind !== "file" ? m.duration : row.through ? emOutToSrc(m, emSrcToOut(m, row.offset) + row.length) : row.offset + row.length);
/** The time inside the file that file row `row` plays at timeline time `t`. */
const emRowFileAt = (m, row, t) => row.trimStart + (row.through ? emSrcToOut(m, t) - emSrcToOut(m, row.offset) : t - row.offset);
/** True if file row `row` has sound at source time `t`. Track rows always do. (`m` is needed for a row that plays through cuts.) */
const emRowCovers = (row, t, m) => row.kind !== "file" || (t >= row.offset - EM_EPS && (row.through && m ? emRowFileAt(m, row, t) - row.trimStart < row.length - EM_EPS : t < row.offset + row.length));
/* ---------- the end of the timeline (0.1.35) ----------
 * The result ends where its last thing ends: the video (m.duration), an overlay item's `to`, or a file row's end.
 * Time between m.duration and emEnd(m) is "after the video": black, no sound from the clip's own tracks; overlay
 * items and (through) file rows go on there. In the result it comes after everything kept, divided by the speed. */
/** Timeline seconds where the result ends (never before the video's end). */
function emEnd(m) {
  let end = m.duration || 0;
  for (const o of m.overlays || []) if (Number.isFinite(o?.to) && o.to > end) end = o.to;
  for (const r of m.audio || []) if (r.kind === "file") { const e = emRowEnd(m, r); if (e > end) end = e; }
  return end;
}
/** Timeline seconds after the video's end that the result runs on (0 = it ends with the video). */
const emTail = (m) => Math.max(0, emEnd(m) - (m.duration || 0));
/** Length of the finished clip with the time after the video (output seconds); emOutDuration without anything after it. */
const emResultDuration = (m) => emSrcToOut(m, emEnd(m)) / (m.speed || 1);
/** Switch a file row between playing through cuts and being cut with the video; its block keeps its place on the timeline. */
function emSetThrough(m, id, on) {
  const row = emRow(m, id);
  if (!row || row.kind !== "file" || !!row.through === !!on) return false;
  const end = emRowEnd(m, row);
  row.through = !!on;
  row.length = Math.max(EM_MIN_SEG, on ? emSrcToOut(m, end) - emSrcToOut(m, row.offset) : end - row.offset);
  return true;
}

/* The "Game" track (only the game's own sound) is already part of "Desktop"
 * (everything the PC played). Played or mixed together, the game would be
 * twice as loud. So the result is built from Desktop + Microphone as before,
 * and Game is used INSTEAD of Desktop: it is heard only in the segments where
 * Desktop is silent (muted, level 0, or another row soloed). Mute Desktop and
 * keep Game to keep the game and drop voice chat and music. */

/** The clip's own track row with this title ("Desktop", "Game"), or null. */
const emTrackRow = (m, title) => m.audio.find((r) => r.kind === "track" && r.title === title) || null;

/** True if `row` is the Game track and is left out in segment `i` because Desktop is heard there. */
function emGameHidden(m, row, i) {
  if (!row || row.kind !== "track" || row.title !== "Game") return false;
  // `gameApart` (set by editor.js from the waveforms): this clip's desktop sound is silent, so the game
  // played on another output device and is only on the Game row. It is then heard beside Desktop.
  if (m.gameApart) return false;
  const d = emTrackRow(m, "Desktop");
  return !!d && !d.muted && d.volume > 0.005 && (d.gains[i] ?? 1) > 0.005;
}

/** What the timeline says about a row, in English (translate with t()): "" for most rows. */
function emRowHint(m, row) {
  if (row && row.kind === "file") return row.through ? "Plays on through every cut in the video" : "Cut together with the video";
  if (!row || row.kind !== "track") return "";
  if (m.gameApart && row.title === "Game") return "Only the game's sound. The desktop sound of this clip does not hold it";
  if (m.gameApart && row.title === "Desktop") return "What the recorded output device played. The game played on another device and is on its own row";
  if (row.title === "Game" && emTrackRow(m, "Desktop")) return "Only the game's sound. Desktop already includes it, so Game is heard only where Desktop is muted";
  if (row.title === "Desktop" && emTrackRow(m, "Game")) return "Everything the PC played, the game included. Mute it and keep Game on to drop voice chat and music";
  return "";
}

/** How loud `row` is at source time `t`: 0 = silent, 1 = as recorded. */
function emGainAt(m, row, t) {
  if (row.muted || !emRowCovers(row, t, m)) return 0;
  const i = emSegmentAt(m, t);
  if (emGameHidden(m, row, i)) return 0;
  return row.volume * row.gains[i];
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
  // At the playhead, also after the video (0.1.35): there it is as long as the file (the result gets longer).
  const offset = Math.max(0, +at || 0);
  const room = emKeptDuration(m) - emSrcToOut(m, offset);
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
    // It plays through cuts (see emRowEnd): as long as the result has room for from where it starts.
    through: true,
    length: Math.max(EM_MIN_SEG, room >= EM_MIN_SEG ? Math.min(info.duration, room) : info.duration),
    fileDuration: info.duration,
  };
  m.audio.push(row);
  return row;
}

/** Remove an added file row. The clip's own tracks cannot be removed (mute them instead). */
function emRemoveRow(m, id) {
  // Any row: an added file, or one of the clip's own tracks (0.1.33). A deleted track is not in the export.
  const at = m.audio.findIndex((r) => r.id === id);
  if (at < 0) return false;
  m.audio.splice(at, 1);
  return true;
}
/** The clip's own tracks whose row was deleted: [{ index, title }]. */
const emTracksGone = (m) => (m.tracks0 || []).filter((x) => !m.audio.some((r) => r.kind === "track" && r.index === x.index));

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
  // A row that plays through cuts may start and run on after the video (0.1.35: the result gets longer, emEnd);
  // a row cut with the video stays inside it (there is no video after the end to cut it with).
  offset = row.through ? Math.max(0, offset) : Math.min(Math.max(0, offset), Math.max(0, m.duration - EM_MIN_SEG));
  if (!row.through) length = Math.max(EM_MIN_SEG, Math.min(length, m.duration - offset));
  Object.assign(row, { offset, trimStart, length });
  return true;
}

/**
 * Slip a file row (0.1.35): the sound under the block moves by `delta` seconds of the file (trimStart), the block
 * stays where it is and keeps its length; kept inside the file. True if it moved.
 */
function emSlipFile(m, id, delta) {
  const row = emRow(m, id);
  if (!row || row.kind !== "file" || !Number.isFinite(delta)) return false;
  const hi = Math.max(0, row.fileDuration - row.length);
  const next = Math.round(Math.min(hi, Math.max(0, row.trimStart + delta)) * 1000) / 1000;
  if (Math.abs(next - row.trimStart) < 1e-9) return false;
  row.trimStart = next;
  return true;
}

/* ---------- overlay items on the timeline (0.1.35) ----------
 * The items are ui/overlays.js's (and media.js's); the timeline moves and trims them with these. Times are
 * timeline seconds; an item may lie partly or wholly after the video. `off` (media: the seconds into the media
 * shown at `from`) and `mlen` (the media's length; with `loop` it repeats) are only there for media items. */
const EM_OV_MIN = 0.1;
/** Move item `o` by `dt` seconds (not before 0): from, to and every keyframe. Returns the seconds it moved. */
function emMoveOverlay(o, dt) {
  if (!o || !Number.isFinite(dt)) return 0;
  const d = Math.max(dt, -o.from);
  if (Math.abs(d) < 1e-9) return 0;
  o.from += d;
  o.to += d;
  if (Array.isArray(o.keys)) for (const k of o.keys) if (k && Number.isFinite(k.t)) k.t += d;
  return d;
}
/** The longest item `o` may be from its start: its media's rest unless it loops; Infinity for other items. */
const emOvMaxLen = (o) => (Number.isFinite(o?.mlen) && !o.loop ? Math.max(EM_OV_MIN, o.mlen - (Number.isFinite(o.off) ? o.off : 0)) : Infinity);
/** Trim the start ("left") or the end ("right") of item `o` to timeline second `at` (at least EM_OV_MIN long).
 *  A media item's start trim moves `off` with it (never under 0), its end stops at the media's end. True if it changed. */
function emTrimOverlay(o, side, at) {
  if (!o || !Number.isFinite(at)) return false;
  const f0 = o.from, t0 = o.to, off = Number.isFinite(o.off) ? o.off : null;
  if (side === "left") {
    let from = Math.max(0, Math.min(at, o.to - EM_OV_MIN));
    if (off !== null && off + (from - f0) < 0) from = f0 - off; // the media has nothing before its start
    o.from = from;
    if (off !== null) o.off = Math.max(0, Math.round((off + (from - f0)) * 1000) / 1000);
  } else if (side === "right") {
    o.to = Math.min(Math.max(at, o.from + EM_OV_MIN), o.from + emOvMaxLen(o));
  } else return false;
  return Math.abs(o.from - f0) > 1e-9 || Math.abs(o.to - t0) > 1e-9;
}

/* ---------- export ---------- */

/** True if no audio row differs from the clip as recorded (so the export can keep the original tracks). */
const emAudioUntouched = (m) => !emTracksGone(m).length && m.audio.every((r) => r.kind === "track" && !r.muted && Math.abs(r.volume - 1) < 0.005 && r.gains.every((g, i) => m.removed[i] || Math.abs(g - 1) < 0.005));
/** True if the edit is nothing but one kept range (one in the source too): the export can cut it without re-encoding. */
const emIsPlainCut = (m) => !emHasLayers(m) && !m.canvas && emKept(m).length === 1 &&(!m.parts || emSrcSpans(m, emKept(m)[0].start, emKept(m)[0].end).length === 1) && Math.abs(m.speed - 1) < 0.005 && emPieces(m).every((p) => !p.blank && emSameColor(p.color)) && emAudioUntouched(m);
/** True if the result would differ from the source at all. */
const emChanged = (m) => !(emIsPlainCut(m) && m.removed.every((r) => !r));

const emRound = (x) => Math.round(x * 1000) / 1000;

/**
 * The `opts` for the `export_clip` command.
 *   segments  the kept source ranges, in order: { start, end }, plus
 *             `color` when the kept parts do not all share one color
 *             correction (it then replaces the top-level `color` for that
 *             range) and `blank: true` where the picture is hidden (black)
 *   color     the color correction of the whole result when every kept part
 *             shares it; neutral when the ranges carry their own
 *   audio     [] = keep the clip's audio tracks as recorded. Otherwise one
 *             entry per row that is heard, mixed into ONE track:
 *             { source: "track" | "file", index, path, offset, trim_start,
 *               length, volume, regions: [{ start, end, gain }] }
 *             `regions` lists the source ranges whose level differs from the
 *             row's volume (gain multiplies it; 0 = silent).
 *   ordered   only with parts in another order than the source: true, and
 *             `segments` are in display order (any source order). A file row
 *             that spans parts then comes as one entry per source range.
 * Every time sent is SOURCE time (emSrcSpans); without parts that is the model's time.
 */
function emToOpts(m) {
  const segs = emSegments(m);
  const pieces = emPieces(m);
  // A piece that is all gap (0.1.36) has no picture: its color does not count (the result's color applies over it).
  const pics = m.parts ? pieces.filter((p) => emSrcSpans(m, p.start, p.end).some((x) => !x.gap)) : pieces;
  const uniform = pics.every((p) => emSameColor(p.color, pics[0].color));
  // Source ranges [{ start, end }] under timeline range [a, b); without parts exactly [a, b].
  // A gap (0.1.36) is { gap: length }; srcOf leaves it out, srcGapOf keeps it.
  const srcGapOf = (a, b) => (m.parts ? emSrcSpans(m, a, b).map((x) => (x.gap ? { gap: x.len } : { start: x.src, end: x.src + x.len })) : [{ start: a, end: b }]);
  const srcOf = (a, b) => srcGapOf(a, b).filter((x) => !x.gap);
  // A Game row counts as silent where Desktop is heard (see emGameHidden); left out if that is everywhere.
  const gainOf = (r, i) => (emGameHidden(m, r, i) ? 0 : r.gains[i]);
  const regionsOf = (r) => segs.filter((s) => !s.removed && Math.abs(gainOf(r, s.i) - 1) >= 0.005)
    .flatMap((s) => srcOf(s.start, s.end).map((x) => ({ start: emRound(x.start), end: emRound(x.end), gain: emRound(gainOf(r, s.i)) })));
  const entries = (r) => {
    const e = {
      source: r.kind,
      index: r.kind === "track" ? r.index : 0,
      path: r.kind === "file" ? r.path : "",
      offset: r.kind === "file" ? emRound(r.offset) : 0,
      trim_start: r.kind === "file" ? emRound(r.trimStart) : 0,
      length: r.kind === "file" ? emRound(r.length) : 0,
      volume: emRound(r.volume),
      regions: regionsOf(r),
    };
    if (r.kind === "file" && r.through) {
      // Plays on through every cut: placed on the result's own timeline (kept seconds, before the speed), and its
      // levels likewise. The backend mixes it in after the cuts (`through`).
      const k0 = emSrcToOut(m, r.offset);
      // It may run on after the video (into `tail`, 0.1.35).
      return [{ ...e, through: true, offset: emRound(k0), length: emRound(Math.max(0, r.length)),
        regions: segs.filter((s) => !s.removed && Math.abs(gainOf(r, s.i) - 1) >= 0.005).map((s) => ({ start: emRound(emSrcToOut(m, s.start)), end: emRound(emSrcToOut(m, s.end)), gain: emRound(gainOf(r, s.i)) })) }];
    }
    if (r.kind !== "file" || !m.parts) return [e];
    // A row cut with the video is silent over a gap, as the clip's own tracks are.
    const spans = emSrcSpans(m, r.offset, r.offset + r.length).filter((sp) => !sp.gap);
    if (spans.length <= 1) return [{ ...e, offset: emRound(spans[0] ? spans[0].src : emToSrc(m, r.offset)) }];
    // One entry per source range it covers, each with the regions inside that range.
    return spans.map((sp) => ({
      ...e,
      offset: emRound(sp.src),
      trim_start: emRound(r.trimStart + (sp.tl - r.offset)),
      length: emRound(sp.len),
      regions: e.regions.filter((g) => g.end > sp.src + EM_EPS && g.start < sp.src + sp.len - EM_EPS)
        .map((g) => ({ ...g, start: emRound(Math.max(g.start, sp.src)), end: emRound(Math.min(g.end, sp.src + sp.len)) })),
    }));
  };
  const audio = emAudioUntouched(m)
    ? []
    : m.audio.filter((r) => !r.muted && r.volume > 0.005 && segs.some((s) => !s.removed && gainOf(r, s.i) > 0.005)).flatMap(entries);
  const segments = [];
  for (const p of pieces) for (const x of srcGapOf(p.start, p.end)) {
    if (x.gap) {
      // Black and silent here (0.1.36); gaps in a row become one.
      const last = segments[segments.length - 1];
      if (last && last.gap !== undefined) last.gap = emRound(last.gap + x.gap);
      else segments.push({ gap: emRound(x.gap) });
    } else segments.push({ start: emRound(x.start), end: emRound(x.end), ...(uniform ? {} : { color: { ...p.color } }), ...(p.blank ? { blank: true } : {}) });
  }
  const ranges = segments.filter((s) => s.gap === undefined);
  const ordered = segments.length !== ranges.length || ranges.some((s, i) => i > 0 && s.start < ranges[i - 1].end - 0.0005);
  // After the video (0.1.35): OUTPUT seconds of black after everything kept, where overlays and through rows go on.
  const tail = emRound(emTail(m) / (m.speed || 1));
  return {
    ...(tail > 0 ? { tail } : {}),
    start: 0,
    end: 0,
    speed: m.speed,
    color: uniform && pics.length ? { ...pics[0].color } : emNoColor(),
    // Nothing is heard (every row muted or deleted): the older per-track list with one silent track says "no sound"
    // (an empty `audio` alone would mean "keep the clip's tracks as recorded").
    tracks: !audio.length && !emAudioUntouched(m) ? [{ index: 0, volume: 0 }] : [],
    segments,
    ...(ordered ? { ordered: true } : {}),
    audio,
    ...emLayerOpts(m),
    ...(m.canvas ? { canvas: emCanvasOpts(m.canvas) } : {}),
    // Pieces with keyframes: one track per piece, [] for one that stands still (keyframes.js).
    ...(m.canvas && m.canvas.pieces.some((p) => Array.isArray(p.keys) && p.keys.length) && typeof kfPieceTrack === "function"
      ? { canvas_tracks: m.canvas.pieces.map((p) => (Array.isArray(p.keys) && p.keys.length ? kfPieceTrack(m, p) : [])) } : {}),
  };
}

/**
 * Timeline range [from, to] -> the OUTPUT seconds it covers in the finished clip (cut-out parts
 * gone, speed applied), as the export's overlays, blurs and fades count them. null when nothing
 * of it is kept.
 */
function emOutRange(m, from, to) {
  const sp = m.speed || 1, E = emEnd(m); // after the video too (0.1.35)
  const a = emSrcToOut(m, Math.max(0, Math.min(E, +from || 0))) / sp;
  const b = emSrcToOut(m, Math.max(0, Math.min(E, +to || 0))) / sp;
  return b - a > 0.001 ? { from: emRound(a), to: emRound(b) } : null;
}
/** The export's blur boxes and fades (the overlay PNGs are added by ui/overlays.js). Keys only when there are any. */
function emLayerOpts(m) {
  const out = {}, r4 = (x) => Math.round(x * 10000) / 10000, total = emOutDuration(m);
  const blurs = (m.overlays || []).filter((o) => o.kind === "blur").map((o) => {
    const r = emOutRange(m, o.from, o.to);
    // A box with keyframes moves along its track (output seconds; keyframes.js).
    const track = r && Array.isArray(o.keys) && o.keys.length && typeof kfOvTrack === "function" ? kfOvTrack(m, o) : null;
    return r && { x: r4(o.x), y: r4(o.y), w: r4(o.w), h: r4(o.h), from: r.from, to: r.to, strength: r4(o.strength), ...(track && track.length ? { track } : {}),
      ...(o.round > 0 ? { round: r4(o.round) } : {}), ...(o.soft > 0 ? { soft: r4(o.soft) } : {}) }; // round / soft (0.1.35, ui/media.js)
  }).filter(Boolean);
  if (blurs.length) out.blurs = blurs;
  if (m.fadeIn > 0) out.fade_in = emRound(Math.min(m.fadeIn, total / 2));
  if (m.fadeOut > 0) out.fade_out = emRound(Math.min(m.fadeOut, total / 2));
  return out;
}

// Lets the functions be tested under Node; ignored in the app.
if (typeof module !== "undefined" && module.exports) {
  module.exports = { EM_MIN_SEG, EM_MAX_GAIN, EM_EPS, emNew, emSegments, emSegmentAt, emSnapshot, emUndo, emRedo, emSplit, emJoin, emMoveCut, emSetRemoved, emTrimStart, emTrimEnd, emTrimBounds, emSameSeg, emSetTrimStart, emSetTrimEnd, emSegCopy, emSegPaste, emResetCuts, emKept, emKeptDuration, emOutDuration, emSrcToOut, emOutToSrc, emPlayable, EM_COLOR_KEYS, emNoColor, emSameColor, emColorAt, emBlankAt, emSetColor, emSetBlank, emPieces, EM_MARK_GAP, EM_MARK_BACK, emAddMark, emRemoveMark, emSetMarks, emMarkNear, emNextMark, emPrevMark, emRow, emRowCovers, emTrackRow, emGameHidden, emRowHint, emGainAt, emSetGain, emIsSolo, emSolo, emAddFile, emRemoveRow, emPlaceFile, emAudioUntouched, emIsPlainCut, emChanged, emToOpts,
    emSetParts, emParts, emPartAt, emToSrc, emFromSrc, emSrcSpans, emMovePart, emCutLocked, emLockedTimes, emPartSegs, emPartRemoved, emSetPartRemoved, emReordered, emState, emRestore,
    emAddParts, emNewParts, emStateSave, emStateOk, emStateLoad, EM_STATE_V,
    emPieceList, emPieceAt, emSplitAt, emRemovePiece, emTrimPiece, emPieceToItem, emPiecify, emNeedsPieces, emClipBounds, emLimits,
    emHasGaps, emGapAt, emSegIsGap, emKeepSource, emEnsureParts, emSplitPartAt, emPlacePart, emPutBeside, emCloseGap, emCloseGaps, emSetGapLen, emInsertGap, emLiftSeg, emSplitOverlay, emKeyAt, emGroups, emNormGroups, emLayGroups,
    emRowEnd, emRowFileAt, emEnd, emTail, emResultDuration, emSlipFile, emMoveOverlay, emTrimOverlay, emOvMaxLen, EM_OV_MIN, emSetThrough, emTracksGone, emRemoveRow, emHasLayers, emOutRange, emLayerOpts, emOverlayOk, EM_OVERLAY_KINDS, emCanvasOk, emCanvasOpts };
}
