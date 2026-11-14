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
//   marks      bookmarks: sorted source times, none closer than EM_MARK_GAP. Only there
//              to find a moment again: not part of undo / redo, emChanged or the export,
//              and cutting a part out leaves the bookmarks inside it alone.
//   undo, redo snapshots (JSON strings) for emUndo / emRedo
//   parts      null, or (a clip made by "Edit together") the clips it is made of, in DISPLAY
//              order: [{ src, len, title }] (src = source second where the part starts).
//              Then every time above is a TIMELINE second: the parts laid end to end in
//              display order; emToSrc / emFromSrc convert. Without parts timeline = source.
//              A part border is always a cut, and a locked one (emCutLocked).

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
    colors: [emNoColor()],
    blank: [false],
    speed: 1,
    marks: [],
    parts: null,
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
const emState = (m) => JSON.stringify({ cuts: m.cuts, removed: m.removed, audio: m.audio, speed: m.speed, colors: m.colors, blank: m.blank, ...(m.parts ? { parts: m.parts } : {}) });
function emRestore(m, json) {
  const s = JSON.parse(json);
  if (m.parts && Array.isArray(s.parts)) {
    // The bookmarks are not in the snapshot but move with their part: keep each at its place in its part.
    const P0 = emParts(m);
    const pos = (m.marks || []).map((x) => { const p = P0[emPartAt(m, x)]; return p ? { src: p.src, off: x - p.at } : { src: null, off: x }; });
    m.parts = s.parts;
    const P1 = emParts(m);
    if (m.marks) m.marks = pos.map((q) => { const p = P1.find((r) => r.src === q.src); return p ? Math.min(m.duration, p.at + q.off) : q.off; }).sort((a, b) => a - b);
  }
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
  return emSplitIn(m, i, t);
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

/* ---------- trim brackets ---------- */
// The trim is the leading (and the trailing) run of removed segments: the start bracket sits where
// the first kept segment begins, the end bracket where the last kept one ends. emSetTrimStart /
// emSetTrimEnd move the inner edge of that run to a time:
//   inward   everything the edge passes is cut out and joins the run: kept segments (split at the
//            new edge) and segments already cut out in the middle alike;
//   outward  the run's segments come back in order, each with the state it had (levels, colour,
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

/** True if segments `a` and `b` look the same everywhere: cut out or not, colour, hidden video, level on every row. */
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
  sec = +sec;
  if (!Number.isFinite(sec)) return null;
  if (strict && !(sec >= -EM_EPS && sec <= emTrimBounds(m).end - EM_MIN_SEG + EM_EPS)) return null;
  return emTrimLead(m, sec);
}

/** End bracket: make the kept span end at `sec`; the mirror image of emSetTrimStart. Returns the new end. */
function emSetTrimEnd(m, sec, strict) {
  sec = +sec;
  if (!Number.isFinite(sec)) return null;
  if (strict && !(sec <= m.duration + EM_EPS && sec >= emTrimBounds(m).start + EM_MIN_SEG - EM_EPS)) return null;
  const mm = emMirror(m);
  emTrimLead(mm, m.duration - sec);
  emUnmirror(m, mm, sec);
  return emTrimBounds(m).end;
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

/** Back to one whole, kept segment (one per part: the part borders stay). Row levels and files stay; the first segment's colour becomes the clip's. */
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
  return m.parts.map((p, i) => { const o = { i, at, end: at + p.len, src: p.src, len: p.len, title: p.title }; at += p.len; return o; });
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
  const p = emParts(m)[emPartAt(m, x)];
  return p.src + Math.min(p.len, Math.max(0, x - p.at));
}

/** Source second -> timeline second. A source border belongs to the part that starts there. */
function emFromSrc(m, src) {
  if (!m.parts) return src;
  const P = emParts(m);
  const x = Math.min(m.duration, Math.max(0, +src || 0));
  let best = null; // the part with the largest source start at or before x
  for (const p of P) if (p.src <= x + EM_EPS && (!best || p.src > best.src)) best = p;
  best ||= P[0];
  return best.at + Math.min(best.len, Math.max(0, x - best.src));
}

/** The source ranges under timeline range [a, b): [{ tl, src, len }] in timeline order; neighbours that also touch in source are merged. */
function emSrcSpans(m, a, b) {
  if (!(b > a)) return [];
  if (!m.parts) return [{ tl: a, src: a, len: b - a }];
  const out = [];
  for (const p of emParts(m)) {
    const lo = Math.max(a, p.at), hi = Math.min(b, p.end);
    if (hi - lo <= EM_EPS) continue;
    const s = { tl: lo, src: p.src + (lo - p.at), len: hi - lo };
    const last = out[out.length - 1];
    if (last && Math.abs(last.src + last.len - s.src) < EM_EPS) last.len += s.len;
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
 * colour, hidden video, every row's levels) and bookmarks go with it; file rows stay where they
 * are on the timeline. Returns true if the order changed. Does not snapshot.
 */
function emMovePart(m, from, to) {
  const P = emParts(m), n = P.length;
  if (!n || !Number.isInteger(from) || !Number.isInteger(to) || from < 0 || to < 0 || from >= n || to >= n || from === to) return false;
  const segs = emSegments(m);
  const groups = P.map((p) => ({ p, cuts: [], segs: [], marks: [] }));
  for (const s of segs) {
    const g = groups[emPartAt(m, (s.start + s.end) / 2)];
    if (g.segs.length) g.cuts.push(s.start - g.p.at);
    g.segs.push({ removed: m.removed[s.i], color: m.colors[s.i], blank: m.blank[s.i], gains: m.audio.map((r) => r.gains[s.i]) });
  }
  for (const x of m.marks || []) { const g = groups[emPartAt(m, x)]; if (g) g.marks.push(x - g.p.at); }
  const order = groups.slice();
  order.splice(to, 0, order.splice(from, 1)[0]);
  const cuts = [], removed = [], colors = [], blank = [], gains = m.audio.map(() => []), marks = [];
  let at = 0;
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
    for (const x of g.marks) marks.push(Math.min(m.duration, at + Math.min(x, g.p.len - (k < order.length - 1 ? EM_MARK_GAP : 0))));
    at += g.p.len;
  });
  m.parts = order.map((g) => m.parts[g.p.i]);
  Object.assign(m, { cuts, removed, colors, blank });
  m.audio.forEach((r, k) => { r.gains = gains[k]; });
  // Sorted; one at the very end of the clip can land on a border where another starts: keep one of them.
  if (m.marks) m.marks = marks.sort((a, b) => a - b).filter((x, i, l) => i === 0 || x - l[i - 1] >= EM_MARK_GAP - EM_EPS);
  return true;
}

/** True if the parts are not in source order. */
const emReordered = (m) => !!m.parts && m.parts.some((p, i) => i > 0 && p.src < m.parts[i - 1].src);

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
  let at = D;
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
 * neutral colour and full level on every row; nothing that was there moves (cuts, order, levels,
 * file rows, bookmarks). A model without parts first becomes one part (its title: `firstTitle`).
 * Undo snapshots get the same new parts, so undo still works across the add. Returns true if
 * parts were added. Does not snapshot.
 */
function emAddParts(m, starts, titles, end, firstTitle) {
  const D = m.duration;
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
  m.duration = emAppendTo(m, D, parts, first);
  return true;
}

const EM_STATE_V = 1;
/**
 * The edit as plain data, to save with a project and give back to emStateLoad: cuts, order, cut-out
 * parts, levels, added audio rows, colours, hidden video, speed, bookmarks (timeline seconds), and
 * `joins` (where each source clip after the first starts, as the caller passes them). `undo` keeps
 * the undo history too (only for a reload in the same session; it is not worth saving to disk).
 */
function emStateSave(m, joins, undo) {
  const s = {
    v: EM_STATE_V, duration: m.duration, joins: Array.isArray(joins) ? joins.slice() : [],
    cuts: m.cuts, removed: m.removed, colors: m.colors, blank: m.blank, speed: m.speed,
    audio: m.audio.map((r) => (r.kind === "track" ? { id: r.id, kind: "track", index: r.index, title: r.title, volume: r.volume, muted: r.muted, gains: r.gains } : r)),
    parts: m.parts, marks: m.marks || [],
  };
  if (undo) { s.undo = m.undo.slice(); s.redo = m.redo.slice(); }
  return JSON.parse(JSON.stringify(s));
}

/** True if `s` is a state emStateLoad can use (shape and numbers checked; nothing else trusted). */
function emStateOk(s) {
  const num = (x) => typeof x === "number" && Number.isFinite(x);
  if (!s || typeof s !== "object" || s.v !== EM_STATE_V || !num(s.duration) || s.duration <= 0) return false;
  const D = s.duration, n = Array.isArray(s.cuts) ? s.cuts.length + 1 : 0;
  if (!n || !s.cuts.every((c, i) => num(c) && c > 0 && c < D && (i === 0 || c > s.cuts[i - 1]))) return false;
  if (!Array.isArray(s.removed) || s.removed.length !== n || !s.removed.some((r) => !r)) return false;
  if (!Array.isArray(s.blank) || s.blank.length !== n) return false;
  if (!Array.isArray(s.colors) || s.colors.length !== n || !s.colors.every((c) => c && typeof c === "object" && EM_COLOR_KEYS.every((k) => num(c[k])))) return false;
  if (!num(s.speed) || s.speed < 0.25 || s.speed > 4) return false;
  if (!Array.isArray(s.joins) || !s.joins.every(num)) return false;
  if (!Array.isArray(s.audio) || !s.audio.every((r) => r && (r.kind === "track" || r.kind === "file") && Array.isArray(r.gains) && r.gains.length === n && r.gains.every(num) && num(r.volume)
    && (r.kind === "track" || (typeof r.path === "string" && r.path && num(r.offset) && num(r.trimStart) && num(r.length) && num(r.fileDuration) && typeof r.id === "string")))) return false;
  if (s.parts != null) {
    if (!Array.isArray(s.parts) || s.parts.length < 1 || !s.parts.every((p) => p && num(p.src) && num(p.len) && p.len > 0 && typeof p.title === "string")) return false;
    if (Math.abs(s.parts.reduce((a, p) => a + p.len, 0) - D) > 1e-3) return false;
  }
  return true;
}

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
  const sameTracks = used.size === saved.length && tracks.length === saved.length;
  const files = s.audio.filter((r) => r.kind === "file").map((r) => ({ ...r, gains: r.gains.slice() }));
  m.duration = s.duration;
  m.cuts = s.cuts.slice();
  m.removed = s.removed.map((r) => !!r);
  m.colors = s.colors.map((c) => ({ ...emNoColor(), ...c }));
  m.blank = s.blank.map((b) => !!b);
  m.speed = s.speed;
  m.audio = [...tracks, ...files];
  m.parts = Array.isArray(s.parts) && s.parts.length > 1 ? s.parts.map((p) => ({ src: p.src, len: p.len, title: p.title })) : null;
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

/** True if file row `row` has sound at source time `t`. Track rows always do. */
const emRowCovers = (row, t) => row.kind !== "file" || (t >= row.offset - EM_EPS && t < row.offset + row.length);

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
  const d = emTrackRow(m, "Desktop");
  return !!d && !d.muted && d.volume > 0.005 && (d.gains[i] ?? 1) > 0.005;
}

/** What the timeline says about a row, in English (translate with t()): "" for most rows. */
function emRowHint(m, row) {
  if (!row || row.kind !== "track") return "";
  if (row.title === "Game" && emTrackRow(m, "Desktop")) return "Only the game's sound. Desktop already includes it, so Game is heard only where Desktop is muted";
  if (row.title === "Desktop" && emTrackRow(m, "Game")) return "Everything the PC played, the game included. Mute it and keep Game on to drop voice chat and music";
  return "";
}

/** How loud `row` is at source time `t`: 0 = silent, 1 = as recorded. */
function emGainAt(m, row, t) {
  if (row.muted || !emRowCovers(row, t)) return 0;
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
/** True if the edit is nothing but one kept range (one in the source too): the export can cut it without re-encoding. */
const emIsPlainCut = (m) => emKept(m).length === 1 && (!m.parts || emSrcSpans(m, emKept(m)[0].start, emKept(m)[0].end).length === 1) && Math.abs(m.speed - 1) < 0.005 && emPieces(m).every((p) => !p.blank && emSameColor(p.color)) && emAudioUntouched(m);
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
 *   ordered   only with parts in another order than the source: true, and
 *             `segments` are in display order (any source order). A file row
 *             that spans parts then comes as one entry per source range.
 * Every time sent is SOURCE time (emSrcSpans); without parts that is the model's time.
 */
function emToOpts(m) {
  const segs = emSegments(m);
  const pieces = emPieces(m);
  const uniform = pieces.every((p) => emSameColor(p.color, pieces[0].color));
  // Source ranges [{ start, end }] under timeline range [a, b); without parts exactly [a, b].
  const srcOf = (a, b) => (m.parts ? emSrcSpans(m, a, b).map((x) => ({ start: x.src, end: x.src + x.len })) : [{ start: a, end: b }]);
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
    if (r.kind !== "file" || !m.parts) return [e];
    const spans = emSrcSpans(m, r.offset, r.offset + r.length);
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
  const segments = pieces.flatMap((p) => srcOf(p.start, p.end).map((x) => ({ start: emRound(x.start), end: emRound(x.end), ...(uniform ? {} : { color: { ...p.color } }), ...(p.blank ? { blank: true } : {}) })));
  const ordered = segments.some((s, i) => i > 0 && s.start < segments[i - 1].end - 0.0005);
  return {
    start: 0,
    end: 0,
    speed: m.speed,
    color: uniform && pieces.length ? { ...pieces[0].color } : emNoColor(),
    tracks: [],
    segments,
    ...(ordered ? { ordered: true } : {}),
    audio,
  };
}

// Lets the functions be tested under Node; ignored in the app.
if (typeof module !== "undefined" && module.exports) {
  module.exports = { EM_MIN_SEG, EM_MAX_GAIN, EM_EPS, emNew, emSegments, emSegmentAt, emSnapshot, emUndo, emRedo, emSplit, emJoin, emMoveCut, emSetRemoved, emTrimStart, emTrimEnd, emTrimBounds, emSameSeg, emSetTrimStart, emSetTrimEnd, emSegCopy, emSegPaste, emResetCuts, emKept, emKeptDuration, emOutDuration, emSrcToOut, emOutToSrc, emPlayable, EM_COLOR_KEYS, emNoColor, emSameColor, emColorAt, emBlankAt, emSetColor, emSetBlank, emPieces, EM_MARK_GAP, EM_MARK_BACK, emAddMark, emRemoveMark, emSetMarks, emMarkNear, emNextMark, emPrevMark, emRow, emRowCovers, emTrackRow, emGameHidden, emRowHint, emGainAt, emSetGain, emIsSolo, emSolo, emAddFile, emRemoveRow, emPlaceFile, emAudioUntouched, emIsPlainCut, emChanged, emToOpts,
    emSetParts, emParts, emPartAt, emToSrc, emFromSrc, emSrcSpans, emMovePart, emCutLocked, emLockedTimes, emPartSegs, emPartRemoved, emSetPartRemoved, emReordered, emState, emRestore,
    emAddParts, emNewParts, emStateSave, emStateOk, emStateLoad, EM_STATE_V };
}
