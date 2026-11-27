// Subtitles: what is said in a clip, written on the picture. An official Glint plugin
// (plugins/official/subtitles in Glint's repository).
//
// What it does: adds a "Subtitles" section to the editor. "Make subtitles" writes the sound of the
// clips on the timeline to a WAV with Glint's ffmpeg, finds where somebody speaks (Silero VAD),
// has a speech model read just those parts, and puts what was said on the picture as short lines,
// each an ordinary text item (centred, marked with the tag "subtitles"). So the lines are on the
// timeline, can be dragged, retimed and retyped like any text, are saved with a project, are undone
// with Ctrl+Z, and are part of the exported clip without anything new in the export. "Save .srt"
// writes them as a subtitle file, timed for the finished clip.
//
// 0.2.0, the rework. Before, whisper.cpp read the whole mixed sound in one go: slow on a processor,
// lines that came early or stayed through silence, and invented words over game sound and silence.
// Now:
//   - the speech engine is sherpa-onnx with NVIDIA's Parakeet (English and 24 more European
//     languages) or Whisper large-v3-turbo (Korean, Japanese and the rest), picked by the
//     language. Parakeet reads a minute of speech in a few seconds on a processor;
//   - only the stretches the voice detector marks are read, each on its own, so silence and game
//     sound give no lines;
//   - every word comes with its own time (Parakeet's own; with Whisper, shared out over each
//     short stretch), and the lines are built from the words: a line appears with its first
//     word, ends with its last, and breaks at pauses and sentence ends;
//   - the microphone track is listened to first (a clip of Glint's has one), the whole mix only
//     when nobody speaks there.
//
// Everything runs on this PC. The engine and the models are not part of the plugin: they are its
// downloads (plugin.json "downloads"), fetched the first time subtitles are made and checked
// against the SHA-256 written there. Nothing is uploaded.
const { ctx, t, esc } = Glint;
const TAG = "subtitles";
// The speech models. `rtf`: seconds of work for a second of speech on a middling processor, for the bar.
// `join`: stretches of speech this close are read as one (Whisper gives no word times, so its stretches stay short).
const ENGINES = {
  parakeet: { dl: "model-parakeet", dir: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8", rtf: 0.12, join: 1 },
  whisper: { dl: "model-whisper", dir: "sherpa-onnx-whisper-turbo", rtf: 0.6, join: 0.2 },
};
// The languages, each with its own name and the model that reads it. "auto": Parakeet tells its 25 apart by itself.
const LANGS = [
  ["auto", "", "parakeet"], ["en", "English", "parakeet"], ["ko", "한국어", "whisper"], ["el", "Ελληνικά", "parakeet"], ["es", "Español", "parakeet"], ["fr", "Français", "parakeet"],
  ["de", "Deutsch", "parakeet"], ["pt", "Português", "parakeet"], ["it", "Italiano", "parakeet"], ["nl", "Nederlands", "parakeet"], ["pl", "Polski", "parakeet"],
  ["ru", "Русский", "parakeet"], ["uk", "Українська", "parakeet"], ["sv", "Svenska", "parakeet"], ["da", "Dansk", "parakeet"], ["fi", "Suomi", "parakeet"],
  ["cs", "Čeština", "parakeet"], ["sk", "Slovenčina", "parakeet"], ["ro", "Română", "parakeet"], ["hu", "Magyar", "parakeet"], ["bg", "Български", "parakeet"],
  ["hr", "Hrvatski", "parakeet"], ["sl", "Slovenščina", "parakeet"], ["et", "Eesti", "parakeet"], ["lv", "Latviešu", "parakeet"], ["lt", "Lietuvių", "parakeet"],
  ["ja", "日本語", "whisper"], ["zh", "中文", "whisper"], ["yue", "粵語", "whisper"], ["tr", "Türkçe", "whisper"], ["no", "Norsk", "whisper"], ["hi", "हिन्दी", "whisper"],
  ["ar", "العربية", "whisper"], ["he", "עברית", "whisper"], ["fa", "فارسی", "whisper"], ["id", "Bahasa Indonesia", "whisper"], ["ms", "Bahasa Melayu", "whisper"], ["vi", "Tiếng Việt", "whisper"], ["th", "ไทย", "whisper"],
];
// How much a line holds. `max`: letters (a wide one, as in 한국어 or 日本語, counts as two); `gap`: a pause this long ends the line;
// `linger`: how long a line stays after its last word when nothing follows.
const LENGTHS = {
  short: { max: 18, words: 4, gap: 0.3, linger: 0.2 },
  medium: { max: 34, words: 99, gap: 0.45, linger: 0.3 },
  long: { max: 68, words: 99, gap: 0.6, linger: 0.4 },
};
const COLORS = ["#ffffff", "#ffe14d", "#7df9ff", "#7dff9b", "#ff8fb1", "#111111"];
const LINE_H = 1.2;      // a text item's line height, times its size (as Glint draws it)
const MAX_W = 0.9;       // the widest a line may be, as a fraction of the picture
const EDGE = 0.07;       // how far from the top or bottom edge the lines sit
const STYLE_KEYS = ["size", "place", "color", "outline"];
const BATCH = 80;        // the most stretches of speech one run of the engine is given
const OLD_DOWNLOADS = ["whisper", "model-base", "model-small", "model-turbo"]; // 0.1.0's: whisper.cpp and its models
const CANCELLED = {};

const st = {
  run: null,   // while subtitles are being made: { label, pct (null = unknown), cancel, dl, proc }
  dls: null,   // the plugin's downloads: [{ id, name, size, installed }]
  error: "",
};

/* ---------- pure helpers (tested under Node: tools/subtitles-test.js) ---------- */

const clamp = (x, lo, hi) => Math.min(hi, Math.max(lo, x));
const r2 = (x) => Math.round(x * 100) / 100;
const r3 = (x) => Math.round(x * 1000) / 1000;
const mb = (bytes) => `${Math.max(1, Math.round(bytes / 1048576))} MB`;
const pad4 = (n) => String(n).padStart(4, "0");

/** "1:02.5" for 62.5 seconds. */
function fmtTime(sec) {
  const s = Math.max(0, Math.round((+sec || 0) * 10) / 10), mins = Math.floor(s / 60), rest = s - mins * 60;
  return `${mins}:${rest < 10 ? "0" : ""}${rest.toFixed(1)}`;
}

/** "00:01:02,500" for 62.5 seconds (an .srt file's times). */
function srtTime(sec) {
  const ms = Math.max(0, Math.round((+sec || 0) * 1000));
  const p = (n, w = 2) => String(n).padStart(w, "0");
  return `${p(Math.floor(ms / 3600000))}:${p(Math.floor(ms / 60000) % 60)}:${p(Math.floor(ms / 1000) % 60)},${p(ms % 1000, 3)}`;
}

/** An .srt file of `cues` ([{ from, to, text }], seconds of the finished clip). */
function srtText(cues) {
  return cues.map((c, i) => `${i + 1}\n${srtTime(c.from)} --> ${srtTime(c.to)}\n${String(c.text).split("\n").map((l) => l.trim()).filter(Boolean).join("\n")}\n`).join("\n");
}

/** The files the timeline's footage comes from, each with the stretch of it that is used: [{ path, lo, hi, tracks }]. */
function filesOf(foot) {
  const out = [];
  for (const f of foot) {
    const end = f.from + f.len * (f.speed || 1);
    const have = out.find((x) => x.path === f.path);
    if (have) { have.lo = Math.min(have.lo, f.from); have.hi = Math.max(have.hi, end); if (!have.tracks.length) have.tracks = f.tracks || []; }
    else out.push({ path: f.path, lo: f.from, hi: end, tracks: f.tracks || [] });
  }
  return out;
}

/** Whether a file has a microphone track of its own (a clip of Glint's recorded with one). */
const hasMic = (tracks) => (tracks || []).some((x) => /^mic/i.test(String(x.title || "")));

/** The audio stream of a file to listen to. Glint's clips name their tracks: "Mix" (everything), "Desktop", "Microphone", "Game". */
function pickStream(tracks, listen) {
  const by = (re) => (tracks || []).find((x) => re.test(String(x.title || "")));
  const all = by(/^mix$/i);
  const hit = listen === "mic" ? by(/^mic/i) : listen === "game" ? by(/^game/i) || by(/^desktop/i) : null;
  return (hit || all || { index: 0 }).index; // stream 0 is the mix of a clip of Glint's, and the only sound of any other file
}

/** The voice detector's messages as the stretches with speech: [{ a, b }] in seconds of the WAV ("3.046 -- 6.828" lines). */
function parseVad(log) {
  const out = [];
  for (const line of String(log || "").split(/\r?\n/)) {
    const m = /^\s*(\d+(?:\.\d+)?)\s+--\s+(\d+(?:\.\d+)?)\s*$/.exec(line);
    if (m && +m[2] > +m[1]) out.push({ a: +m[1], b: +m[2] });
  }
  return out.sort((x, y) => x.a - y.a);
}

/**
 * The stretches of a WAV `dur` seconds long that the speech model is given, from the detector's `segs`: each
 * with a little sound around it, neighbours a short pause apart joined (a model reads "Nice." better with what
 * was said just before), none longer than the model reads reliably. In order, never overlapping, on 10 ms.
 */
function planChunks(segs, dur, o = {}) {
  const pad = o.pad ?? 0.15, join = o.join ?? 1, max = o.max ?? 14, hard = o.hard ?? 18;
  const out = [];
  for (const s of [...segs].sort((x, y) => x.a - y.a)) {
    const last = out[out.length - 1];
    const a = Math.max(r2(clamp(s.a - pad, 0, dur)), last ? last.b : 0), b = r2(clamp(s.b + pad, 0, dur));
    if (b - a < 0.05) continue;
    if (last && a - last.b < join && b - last.a <= max) last.b = b;
    else out.push({ a, b });
  }
  // One the detector could not break (somebody talking on and on over music): cut into even parts.
  return out.flatMap((c) => {
    if (c.b - c.a <= hard) return [c];
    const n = Math.ceil((c.b - c.a) / max);
    return Array.from({ length: n }, (_, i) => ({ a: r2(c.a + ((c.b - c.a) * i) / n), b: r2(c.a + ((c.b - c.a) * (i + 1)) / n) }));
  });
}

/**
 * How ffmpeg cuts the WAV for `chunks`: `cuts` are the seconds its segment muxer splits at, and `piece[i]` is the
 * number of the file chunk i lands in (the pieces between two chunks are silence, written and never read).
 */
function cutPlan(chunks, dur) {
  const cuts = [];
  for (const c of chunks) for (const x of [c.a, c.b]) if (x > 0.005 && x < dur - 0.005 && !cuts.some((y) => Math.abs(y - x) < 0.005)) cuts.push(x);
  cuts.sort((x, y) => x - y);
  return { cuts, piece: chunks.map((c) => cuts.filter((x) => x <= c.a + 0.005).length) };
}

const HAS_WORD = /[\p{L}\p{N}]/u;
const PUNCT = /^[\p{P}]+$/u;
const NO_SPACES = /[฀-໿ក-៿぀-ヿ㐀-鿿豈-﫿]/; // Thai, Lao, Khmer, kana, han: written without spaces
const WIDE = /[ᄀ-ᇿ⺀-鿿가-힯豈-﫿＀-￯]/g;
/** A text's length in letters, a wide one counting as two. */
const widthOf = (text) => String(text).length + (String(text).match(WIDE) || []).length;

/** The engine's messages on stdout as its results, one for each file it was given, in order. */
function parseResults(text) {
  const out = [];
  for (const line of String(text || "").split(/\r?\n/)) {
    if (!line.startsWith("{")) continue;
    try { const v = JSON.parse(line); if (v && typeof v === "object") out.push(v); } catch { out.push({}); }
  }
  return out;
}

/** How sure the model was of a result, 0..1 (the mean of its tokens' probabilities), or null when it does not say. */
function confidence(res) {
  const lp = Array.isArray(res?.ys_log_probs) ? res.ys_log_probs.filter((x) => Number.isFinite(x)) : [];
  return lp.length ? lp.reduce((n, x) => n + Math.exp(x), 0) / lp.length : null;
}

/** `text` as words spread over `dur` seconds by their length: for a model that gives no times (Whisper here). */
function spread(text, dur) {
  const clean = String(text || "").replace(/\ufffd/g, "").replace(/\s+/g, " ").trim();
  if (!HAS_WORD.test(clean)) return [];
  let parts = clean.split(" ").map((w) => ({ text: w, glue: false }));
  // Thai, Japanese and the like have no spaces to break at: the browser knows where their words end.
  if (NO_SPACES.test(clean) && typeof Intl !== "undefined" && Intl.Segmenter) {
    parts = [];
    let space = true;
    for (const s of new Intl.Segmenter(undefined, { granularity: "word" }).segment(clean)) {
      if (!s.segment.trim()) { space = true; continue; }
      const last = parts[parts.length - 1];
      if (last && !space && !s.isWordLike) last.text += s.segment; // a full stop or comma stays on its word
      else parts.push({ text: s.segment, glue: !space });
      space = false;
    }
  }
  parts = parts.filter((p) => p.text);
  const total = parts.reduce((n, p) => n + widthOf(p.text) + 1, 0), a0 = Math.min(0.05, dur / 4), span = Math.max(0.1, dur - a0 - Math.min(0.15, dur / 4));
  let at = a0;
  return parts.map((p, i) => {
    const len = (span * (widthOf(p.text) + 1)) / total, w = { text: p.text, a: r3(at), b: r3(at + len * 0.92), glue: !!p.glue && i > 0 };
    at += len;
    return w;
  });
}

/**
 * One result of the engine as words with their times in seconds of its WAV (`dur` long): [{ text, a, b, glue }].
 * The models give word pieces (" A", "sk", " not", ","): a piece that starts with a space starts a word, punctuation
 * joins the word before it. `glue`: written right after the word before it (Japanese and Chinese have no spaces).
 */
function wordsOf(res, dur) {
  const toks = Array.isArray(res?.tokens) ? res.tokens.map(String) : [], ts = Array.isArray(res?.timestamps) ? res.timestamps : [], du = Array.isArray(res?.durations) ? res.durations : [];
  if (!toks.length || ts.length !== toks.length || toks.some((x) => x.includes("�"))) return spread(res?.text, dur);
  const out = [];
  let space = true, before = "";
  for (let i = 0; i < toks.length; i++) {
    const text = toks[i].trim(), lead = space || /^\s/.test(toks[i]);
    if (!text) { space = true; continue; }
    space = false;
    const a = clamp(+ts[i] || 0, 0, dur), last = out[out.length - 1];
    if (!HAS_WORD.test(text)) {
      if (!PUNCT.test(text)) continue; // a note, a smiley: not something that was said
      if (last && !lead) last.text += text;
      else before += text; // an opening quote or "¿": on the word that follows
      continue;
    }
    const end = +du[i] > 0 ? a + +du[i] : null; // null: the model gives no lengths, see below
    if (last && !lead && !NO_SPACES.test(text) && !last.atom) { last.text += text; last.e = a; last.b = end; continue; }
    out.push({ text: before + text, a, e: a, b: end, glue: !lead && out.length > 0, atom: NO_SPACES.test(text) });
    before = "";
  }
  // A word ends where the model says, never after the next one starts; without a length, shortly after its last piece.
  return out.map((w, i) => {
    const next = i + 1 < out.length ? out[i + 1].a : dur;
    const b = clamp(w.b === null ? w.e + 0.3 : w.b, w.a + 0.02, Math.max(w.a + 0.02, next));
    return { text: w.text, a: r3(w.a), b: r3(b), glue: w.glue };
  });
}

/**
 * Words heard in the files (`byPath`: path -> [{ text, a, b, glue }], seconds of the file) as words on the timeline,
 * in order: [{ text, a, b, glue, brk }]. `foot` is the editor's footage (which stretch of which file each part of the
 * timeline shows, and how fast). A word in a part that is cut out of the result (`removedAt(sec)`) is left out.
 * `brk`: the footage jumps before this word (a cut, another clip), so a line must not run across it.
 */
function wordsOnTimeline(foot, byPath, removedAt) {
  const out = [];
  let prev = null;
  for (const f of [...foot].sort((x, y) => x.at - y.at)) {
    const sp = f.speed || 1, end = f.from + f.len * sp;
    let jump = !(prev && prev.path === f.path && Math.abs(prev.from + prev.len * (prev.speed || 1) - f.from) < 0.05 && Math.abs(prev.at + prev.len - f.at) < 0.05);
    for (const w of byPath[f.path] || []) {
      const mid = (w.a + w.b) / 2;
      if (mid < f.from || mid >= end) continue;
      const a = f.at + (Math.max(w.a, f.from) - f.from) / sp, b = f.at + (Math.min(w.b, end) - f.from) / sp;
      if (removedAt && removedAt((a + b) / 2)) { jump = true; continue; }
      out.push({ text: w.text, a, b, glue: !!w.glue && !jump, brk: jump && out.length > 0 });
      jump = false;
    }
    prev = f;
  }
  return out;
}

const joined = (words) => words.map((w, i) => (i && !w.glue ? " " : "") + w.text).join("");
const SENTENCE_END = /[.!?…。！？]["')\]»”’]*$/;
const CLAUSE_END = /[,;:、，；：]["')\]»”’]*$/;

/**
 * Words on the timeline as subtitle lines: [{ from, to, text }], never overlapping. What is said in one breath is a
 * phrase: it ends at a pause, where the footage jumps, and at the end of a sentence (two very short sentences said
 * straight after each other stay together: "Nice. Go."). A phrase too long for a line (`o`: one of LENGTHS) is cut
 * into the fewest lines that hold it, as even as they can be, at a comma when one is near, so no line is left with
 * a single stray word. A line comes up with its first word and goes with its last, staying a moment longer when
 * nothing follows.
 */
function captions(words, o) {
  const phrases = [];
  let cur = [];
  for (const w of words) {
    const prev = cur[cur.length - 1];
    if (prev && (w.brk || w.a - prev.b > o.gap || (SENTENCE_END.test(prev.text) && !(widthOf(joined(cur)) < o.max * 0.35 && w.a - prev.b < 0.25)))) { phrases.push(cur); cur = []; }
    cur.push(w);
  }
  if (cur.length) phrases.push(cur);
  const groups = phrases.flatMap((p) => splitPhrase(p, o));
  const out = groups.map((g) => ({ from: g[0].a, to: g[g.length - 1].b, text: joined(g) }));
  for (let i = 0; i < out.length; i++) {
    const c = out[i], next = out[i + 1], room = next ? next.from : Infinity;
    c.from = r3(Math.max(i ? out[i - 1].to : 0, c.from - 0.05));
    // Speech that goes straight on: the line stays until the next one. Otherwise it lingers, and is up long enough to read.
    const want = next && next.from - c.to < 0.3 ? next.from : Math.max(c.to + o.linger, c.from + 0.6);
    c.to = r3(Math.max(c.from + 0.1, Math.min(want, room)));
  }
  return out;
}

/** One phrase as the lines it needs: lists of its words. */
function splitPhrase(p, o) {
  // cum[k]: the width of the first k words as one line.
  const cum = [0];
  for (let k = 1; k <= p.length; k++) cum.push(widthOf(joined(p.slice(0, k))));
  const total = cum[p.length];
  for (let n = Math.max(1, Math.ceil(total / o.max), Math.ceil(p.length / o.words)); ; n++) {
    if (n >= p.length) return p.map((w) => [w]);
    const cuts = [0];
    for (let j = 1; j < n; j++) {
      // The break nearest an even share; a comma pulls it a little.
      const target = (total * j) / n, lo = cuts[j - 1] + 1, hi = p.length - (n - j);
      let best = lo, cost = Infinity;
      for (let k = lo; k <= hi; k++) {
        const x = Math.abs(cum[k] - target) - (CLAUSE_END.test(p[k - 1].text) ? o.max * 0.2 : 0);
        if (x < cost) { cost = x; best = k; }
      }
      cuts.push(best);
    }
    cuts.push(p.length);
    const lines = cuts.slice(1).map((k, j) => p.slice(cuts[j], k));
    if (lines.every((l) => widthOf(joined(l)) <= o.max * 1.15 && l.length <= o.words) || n + 1 >= p.length) return n === 1 ? [p] : lines;
  }
}

/** `text` on as few lines as fit (`fits(line)`), at most three, the first two as even as they can be. */
function wrap(text, fits) {
  const words = String(text).replace(/\s+/g, " ").trim().split(" ").filter(Boolean);
  const whole = words.join(" ");
  if (words.length < 2 || fits(whole)) return [whole];
  // Two lines: the break that leaves them most alike in length.
  let best = null;
  for (let i = 1; i < words.length; i++) {
    const a = words.slice(0, i).join(" "), b = words.slice(i).join(" ");
    if (!fits(a) || !fits(b)) continue;
    const d = Math.abs(a.length - b.length);
    if (!best || d < best.d) best = { d, lines: [a, b] };
  }
  if (best) return best.lines;
  // Longer: fill line after line; what is left stays on the third.
  const lines = [];
  let cur = "";
  for (const w of words) {
    const next = cur ? `${cur} ${w}` : w;
    if (cur && !fits(next) && lines.length < 2) { lines.push(cur); cur = w; } else cur = next;
  }
  lines.push(cur);
  return lines;
}

/** Where a subtitle of `n` lines sits: the `y` (fraction of the picture) of its top. */
function placeY(n, size, place) {
  const h = n * size * LINE_H;
  return r3(place === "top" ? EDGE : place === "middle" ? 0.5 - h / 2 : 1 - EDGE - h);
}

/** The style in the plugin's settings, made safe. */
function styleOf(s) {
  return {
    size: clamp(Math.round(+s.size || 5), 2, 12) / 100,
    place: ["bottom", "middle", "top"].includes(s.place) ? s.place : "bottom",
    color: /^#[0-9a-f]{6}$/i.test(s.color || "") ? s.color : "#ffffff",
    outline: s.outline !== false,
  };
}

/** A text item of the picture for one cue. `width(line, size)`: a line's width as a fraction of the picture's. */
function itemOf(cue, style, width, id) {
  const lines = wrap(cue.text, (l) => width(l, style.size) <= MAX_W);
  return { id, kind: "text", from: cue.from, to: cue.to, x: 0.5, y: placeY(lines.length, style.size, style.place), text: lines.join("\n"), color: style.color, size: style.size, outline: style.outline, align: "center", tag: TAG };
}

/** The engine's arguments for a model and a language (the files follow them). */
function engineArgs(engine, lang, nThreads) {
  const E = ENGINES[engine], d = `{dl:${E.dl}}\\${E.dir}\\`, n = `--num-threads=${nThreads}`;
  if (engine === "whisper") return [`--tokens=${d}turbo-tokens.txt`, `--whisper-encoder=${d}turbo-encoder.int8.onnx`, `--whisper-decoder=${d}turbo-decoder.int8.onnx`, `--whisper-language=${lang}`, "--whisper-task=transcribe", "--whisper-tail-paddings=300", n];
  return [`--tokens=${d}tokens.txt`, `--encoder=${d}encoder.int8.onnx`, `--decoder=${d}decoder.int8.onnx`, `--joiner=${d}joiner.int8.onnx`, "--model-type=nemo_transducer", n];
}

const newId = () => "o" + Date.now().toString(36) + Math.random().toString(36).slice(2, 7);
const plain = (text) => String(text || "").replace(/\s*\n\s*/g, " ").trim();

/* ---------- making the subtitles ---------- */

/** The language: the one chosen, else Glint's own when Parakeet does not read it (Korean), else Parakeet's own guess. */
function langOf(c) {
  if (LANGS.some(([k]) => k === c.s.language)) return c.s.language;
  const ui = String((typeof document !== "undefined" && document.documentElement?.lang) || "").slice(0, 2).toLowerCase();
  return LANGS.some(([k, , e]) => k === ui && e !== "parakeet") ? ui : "auto";
}
const engineOf = (lang) => (LANGS.find(([k]) => k === lang) || LANGS[0])[2];
const hearOf = (c) => (["auto", "mic", "game", "all"].includes(c.s.hear) ? c.s.hear : "auto");
const lengthOf = (c) => (LENGTHS[c.s.length] ? c.s.length : "medium");
/** Half the processor, so Glint keeps clipping and a game keeps running meanwhile. */
const threads = () => clamp(Math.floor((navigator.hardwareConcurrency || 4) / 2), 2, 8);
/** What "Make subtitles" still has to download: [{ id, name, size }]. */
const missing = (c) => { const need = ["sherpa", "vad", ENGINES[engineOf(langOf(c))].dl]; return (st.dls || []).filter((d) => need.includes(d.id) && !d.installed); };

async function refreshDls(c) {
  try { st.dls = await c.dl.state(); } catch (e) { console.warn("Glint subtitles: the downloads could not be listed", e); }
}

/**
 * The words said in `files` ([{ path, lo, hi, tracks }]) on the sound `listen` picks: path -> [{ text, a, b, glue }]
 * in seconds of the file. `step(change)` shows where the work is, `check()` throws once it was cancelled.
 */
async function hear(c, files, listen, lang, step, check) {
  const engine = engineOf(lang), byPath = {};
  await c.work.clear();
  for (let i = 0; i < files.length; i++) {
    const f = files[i], dur = Math.max(0.1, f.hi - f.lo), wav = `{work}\\a${i}.wav`;
    const part = (x) => ((i + x) / files.length) * 100;
    const fail = (r, what) => { check(); if (r.code !== 0) throw r.error || what; };
    // 1. Its sound as a WAV the engine reads: one channel, 16 kHz.
    step({ proc: "sound", pct: part(0), label: t("Reading the sound…") });
    fail(await c.proc.run("sound", ["-v", "error", "-y", "-ss", f.lo.toFixed(3), "-t", dur.toFixed(3), "-i", f.path,
      "-map", `0:a:${pickStream(f.tracks, listen)}`, "-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", wav]), t("The clip's sound could not be read."));
    // 2. Where somebody speaks.
    step({ proc: "voice", pct: part(0.05), label: t("Finding the voices…") });
    fail(await c.proc.run("voice", ["--silero-vad-model={dl:vad}", "--silero-vad-threshold=0.5", "--silero-vad-min-silence-duration=0.4", "--silero-vad-min-speech-duration=0.2",
      "--silero-vad-max-speech-duration=12", wav, `{work}\\a${i}v.wav`], { tool: "sherpa", program: "sherpa-onnx-vad.exe", out: `a${i}.vad` }), t("The speech could not be read."));
    const chunks = planChunks(parseVad(await c.work.read(`a${i}.vad.log`)), dur, { join: ENGINES[engine].join });
    if (!chunks.length) continue;
    // 3. Each stretch of speech as a file of its own (10 ms frames, so the cuts land where they are asked for).
    const { cuts, piece } = cutPlan(chunks, dur);
    if (cuts.length) {
      step({ proc: "cut", pct: part(0.1) });
      fail(await c.proc.run("cut", ["-v", "error", "-y", "-i", wav, "-af", "asetnsamples=n=160", "-c:a", "pcm_s16le", "-f", "segment", "-segment_times", cuts.map((x) => x.toFixed(2)).join(","), `{work}\\a${i}_%04d.wav`]), t("The clip's sound could not be read."));
    }
    // 4. The speech model reads them, a batch at a time (it is loaded once for each batch).
    const words = [], spoken = chunks.reduce((n, k) => n + k.b - k.a, 0);
    let done = 0;
    for (let from = 0; from < chunks.length; from += BATCH) {
      const batch = chunks.slice(from, from + BATCH), secs = batch.reduce((n, k) => n + k.b - k.a, 0), started = Date.now();
      const guess = 4 + secs * ENGINES[engine].rtf, base = done / spoken, share = secs / spoken;
      const label = files.length > 1 ? t("Listening… (clip {n} of {total})", { n: i + 1, total: files.length }) : t("Listening…");
      step({ proc: "speech", pct: part(0.12 + 0.88 * base), label });
      const names = batch.map((_, j) => (cuts.length ? `{work}\\a${i}_${pad4(piece[from + j])}.wav` : wav));
      fail(await c.proc.run("speech", [...engineArgs(engine, lang, threads()), ...names], {
        tool: "sherpa", out: `a${i}_${from}.json`,
        onTick: () => step({ pct: part(0.12 + 0.88 * (base + share * Math.min(0.97, (Date.now() - started) / 1000 / guess))) }),
      }), t("The speech could not be read."));
      const results = parseResults(await c.work.read(`a${i}_${from}.json`));
      if (results.length !== batch.length) throw t("The speech could not be read.");
      results.forEach((res, j) => {
        const k = batch[j], sure = confidence(res);
        if (sure !== null && sure < 0.35) return; // the model was guessing: noise, not words
        const said = wordsOf(res, k.b - k.a), after = chunks[from + j + 1];
        // A stretch cut off where the talking went on: the full stop the model put at its end is not one.
        if (said.length && after && after.a - k.b < 0.3) said[said.length - 1].text = said[said.length - 1].text.replace(/([^.])\.$/, "$1");
        for (const w of said) words.push({ text: w.text, a: f.lo + k.a + w.a, b: f.lo + k.a + w.b, glue: w.glue });
      });
      done += secs;
    }
    if (words.length) byPath[f.path] = words;
  }
  return byPath;
}

async function make(ed, c) {
  if (st.run || !ed.model) return;
  const run = (st.run = { label: t("Getting ready…"), pct: null, cancel: false, dl: "", proc: "" });
  st.error = "";
  const show = (change) => { if (!run.cancel) Object.assign(run, change); ed.redraw(); };
  const check = () => { if (run.cancel) throw CANCELLED; };
  show({});
  try {
    // 1. The speech engine, the voice detector and the language's model, the first time.
    await refreshDls(c);
    const lang = langOf(c);
    for (const d of missing(c)) {
      show({ dl: d.id, pct: 0, label: d.id.startsWith("model-") ? t("Downloading the speech model…") : t("Downloading the speech engine…") });
      st.dls = await c.dl.get(d.id);
      show({ dl: "" });
      check();
    }
    // 2. What is said in each file on the timeline: on the microphone's own track first, on everything if nobody speaks there.
    const foot = ed.footage().filter((f) => !f.missing && f.path);
    const files = filesOf(foot);
    if (!files.length) throw t("There is no clip on the timeline to listen to.");
    const how = hearOf(c);
    let byPath = await hear(c, files, how === "auto" ? "mic" : how, lang, show, check);
    if (!Object.keys(byPath).length && how === "auto" && files.some((f) => hasMic(f.tracks))) byPath = await hear(c, files, "all", lang, show, check);
    // 3. Onto the picture, in place of the subtitles that were there: one undo step.
    const cues = captions(wordsOnTimeline(foot, byPath, (sec) => ed.removedAt(sec)), LENGTHS[lengthOf(c)]);
    if (!cues.length) throw how === "mic" || how === "game" ? t("No speech was found there. Try another sound under \"Listen to\".") : t("No speech was found in this clip.");
    const style = styleOf(c.s);
    ed.change((m) => {
      m.overlays = (m.overlays || []).filter((o) => !(o.kind === "text" && o.tag === TAG));
      for (const cue of cues) m.overlays.push(itemOf(cue, style, ed.textWidth, newId()));
    });
    c.flash(t("{n} subtitle lines added", { n: cues.length }));
  } catch (e) {
    if (e !== CANCELLED && !run.cancel) st.error = String(e?.message ?? e);
  }
  st.run = null;
  c.work.clear().catch(() => {});
  await refreshDls(c);
  ed.redraw();
}

function cancel(c) {
  const run = st.run;
  if (!run) return;
  run.cancel = true;
  run.label = t("Stopping…");
  run.pct = null;
  if (run.dl) c.dl.cancel(run.dl).catch(() => {});
  if (run.proc) c.proc.stop(run.proc).catch(() => {});
  c.editor()?.redraw();
}

/* ---------- changing them ---------- */

/** The style controls changed: every line takes the new look (and its place for it). */
function restyle(ed, c) {
  const style = styleOf(c.s);
  ed.change((m) => {
    for (const o of m.overlays || []) {
      if (o.kind !== "text" || o.tag !== TAG) continue;
      const it = itemOf({ from: o.from, to: o.to, text: plain(o.text) }, style, ed.textWidth, o.id);
      Object.assign(o, { x: it.x, y: it.y, text: it.text, color: it.color, size: it.size, outline: it.outline, align: "center" });
    }
  });
}

/** A line was retyped in the list. Emptied: the line goes. */
function retype(ed, id, value) {
  const text = String(value || "").replace(/\s+/g, " ").trim();
  ed.change((m) => {
    const o = (m.overlays || []).find((x) => x.id === id);
    if (!o) return;
    if (!text) { m.overlays = m.overlays.filter((x) => x.id !== id); return; }
    const before = String(o.text).split("\n").length;
    const lines = wrap(text, (l) => ed.textWidth(l, o.size) <= MAX_W);
    o.text = lines.join("\n");
    // Its bottom edge stays where it is (its middle, for a centred one) when the number of lines changes.
    const grew = (lines.length - before) * o.size * LINE_H, place = styleOf(ctx.s).place;
    if (grew && place !== "top") o.y = r3(o.y - (place === "middle" ? grew / 2 : grew));
  });
}

async function saveSrt(ed, c) {
  const cues = ed.texts(TAG).map((o) => { const r = ed.outRange(o.from, o.to); return r && plain(o.text) ? { from: r.from, to: r.to, text: plain(o.text) } : null; }).filter(Boolean);
  if (!cues.length) return c.flash(t("There are no subtitles to save."), true);
  try {
    const name = `${String(ed.clip.title || "subtitles").replace(/[\\/:*?"<>|]+/g, " ").trim() || "subtitles"}.srt`;
    const path = await c.saveText(name, srtText(cues));
    if (path) c.flash(t("Saved {name}", { name: String(path).split(/[\\/]/).pop() }));
  } catch (e) { c.fail(e); }
}

/* ---------- the section ---------- */

const X_ICON = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18"/></svg>';
const sel = (key, value, opts) => `<select class="field" data-sub-set="${key}">${opts.map(([v, label]) => `<option value="${esc(v)}"${String(v) === String(value) ? " selected" : ""}>${esc(label)}</option>`).join("")}</select>`;

function render(ed, c) {
  if (!ed.model) return "";
  const lines = ed.texts(TAG), s = c.s, style = styleOf(s), now = ed.time();
  let html = "";
  if (st.run) {
    const pct = st.run.pct;
    html += `<div class="sub-prog" role="status">
      <div class="sub-prog-top"><b>${esc(st.run.label)}</b>${pct === null ? "" : `<span>${Math.round(pct)}%</span>`}</div>
      <div class="progress${pct === null ? " wait" : ""}"><i style="width:${pct === null ? 30 : Math.round(pct)}%"></i></div>
      <div class="sub-btns"><button type="button" class="btn sm" data-pact="cancel"${st.run.cancel ? " disabled" : ""}>${esc(t("Cancel"))}</button></div>
    </div>`;
  } else {
    const need = missing(c), size = need.reduce((n, d) => n + d.size, 0);
    html += `<label class="sub-row"><span>${esc(t("Spoken in"))}</span>${sel("language", langOf(c), LANGS.map(([k, name]) => [k, k === "auto" ? t("English or another European language") : name]))}</label>
      <label class="sub-row"><span>${esc(t("Listen to"))}</span>${sel("hear", hearOf(c), [["auto", t("My microphone first")], ["mic", t("My microphone")], ["game", t("The game or desktop")], ["all", t("All the sound")]])}</label>
      <label class="sub-row"><span>${esc(t("Lines"))}</span>${sel("length", lengthOf(c), [["short", t("A few words at a time")], ["medium", t("One short line")], ["long", t("Whole sentences")]])}</label>
      <div class="sub-btns"><button type="button" class="btn sm primary" data-pact="make">${esc(lines.length ? t("Make them again") : t("Make subtitles"))}</button></div>
      <div class="small">${esc(need.length
        ? t("The first time, this downloads {size} (the speech engine and its model for this language). After that it works without internet. The sound never leaves this PC.", { size: mb(size) })
        : lines.length ? t("Making them again replaces every line below.") : t("Made on this PC. The sound never leaves it."))}</div>`;
  }
  if (st.error) html += `<div class="sub-err" role="alert">${esc(st.error)}</div>`;
  if (lines.length) {
    html += `<div class="sub-list" data-keep-scroll role="list">${lines.map((o) => `<div class="sub-line${now >= o.from && now <= o.to ? " now" : ""}" role="listitem">
        <button type="button" class="sub-at" data-pact="seek" data-id="${esc(o.id)}" title="${esc(t("Go to this line"))}">${fmtTime(o.from)}</button>
        <input type="text" class="sub-text" data-sub-line="${esc(o.id)}" value="${esc(plain(o.text))}" spellcheck="false" aria-label="${esc(t("Subtitle text"))}">
        <button type="button" class="sub-x" data-pact="del" data-id="${esc(o.id)}" title="${esc(t("Delete this line"))}" aria-label="${esc(t("Delete this line"))}">${X_ICON}</button>
      </div>`).join("")}</div>
      <div class="sub-style">
        <div class="e-sliders"><label class="e-slider"><span>${esc(t("Size"))}</span><input type="range" min="2" max="12" step="1" value="${Math.round(style.size * 100)}" data-sub-set="size"><output>${Math.round(style.size * 100)}%</output></label></div>
        <label class="sub-row"><span>${esc(t("Place"))}</span>${sel("place", style.place, [["bottom", t("Bottom")], ["middle", t("Middle")], ["top", t("Top")]])}</label>
        <div class="sub-row"><span>${esc(t("Color"))}</span><div class="sub-sw">${COLORS.map((col) => `<button type="button" class="${col === style.color.toLowerCase() ? "on" : ""}" data-pact="color" data-color="${col}" style="--sw:${col}" aria-label="${col}" title="${col}"></button>`).join("")}</div></div>
        <label class="check sub-row"><input type="checkbox" data-sub-set="outline"${style.outline ? " checked" : ""}><span>${esc(t("Dark outline"))}</span></label>
      </div>
      <div class="sub-btns">
        <button type="button" class="btn sm" data-pact="srt">${esc(t("Save .srt file"))}</button>
        <button type="button" class="btn sm danger" data-pact="clear">${esc(t("Remove all"))}</button>
      </div>
      <div class="small">${esc(t("The lines are part of the exported clip. Each is also a text on the picture: move or retime it there or on the timeline."))}</div>`;
  }
  return html;
}

const mySection = (el) => el?.closest?.('.e-plug[data-plugin="subtitles"]');

/** A select, check box or slider of the section changed: the setting is saved; a style control restyles the lines. */
function onChange(e) {
  const el = e.target, ed = ctx.editor();
  if (!mySection(el) || !ed) return;
  if (el.dataset.subLine) return retype(ed, el.dataset.subLine, el.value);
  const key = el.dataset.subSet;
  if (!key) return;
  ctx.s[key] = el.type === "checkbox" ? el.checked : el.type === "range" ? +el.value : el.value;
  ctx.save();
  el.blur?.();
  if (STYLE_KEYS.includes(key) && ed.texts(TAG).length) restyle(ed, ctx);
  if (key === "language") refreshDls(ctx).then(() => ed.redraw());
  ed.redraw();
}

Glint.register({
  defaults: { language: "", hear: "auto", length: "medium", size: 5, place: "bottom", color: "#ffffff", outline: true },
  editor: {
    title: "Subtitles",
    tab: "overlays",
    render,
    onOpen: (ed, c) => { st.error = ""; refreshDls(c).then(() => ed.redraw()); },
    onClose: (ed, c) => cancel(c),
  },
  // Glint 0.1.66 and newer list these files under Settings → Components → Plugin components (Glint.hostDownloads).
  settings: Glint.hostDownloads ? [] : [{
    title: "Subtitles",
    render: (c) => {
      const list = (st.dls || []).filter((d) => d.installed);
      return c.row(t("Downloaded files"), t("The speech engine and the models the Subtitles plugin fetched. Removing one frees its space; it is downloaded again when it is needed."), "")
        + (list.length ? list.map((d) => `<div class="sub-dl"><div>${esc(t(d.name))} <span>· ${mb(d.size)}</span></div><button type="button" class="btn sm danger" data-pact="dl-remove" data-id="${esc(d.id)}">${esc(t("Remove"))}</button></div>`).join("")
          : `<div class="small">${esc(t("Nothing is downloaded yet."))}</div>`);
    },
  }],
  onAct: (name, el, c) => {
    if (name === "dl-remove") return c.dl.remove(el.dataset.id).then((list) => { st.dls = list; c.render(); }).catch(c.fail);
    const ed = c.editor();
    if (!ed) return;
    // A line being typed is taken first (a click on a button does not take the focus away from it).
    if (mySection(document.activeElement) && document.activeElement !== el) document.activeElement.blur();
    const id = el.dataset.id;
    switch (name) {
      case "make": return void make(ed, c);
      case "cancel": return cancel(c);
      case "srt": return void saveSrt(ed, c);
      case "clear": return void ed.change((m) => { m.overlays = (m.overlays || []).filter((o) => !(o.kind === "text" && o.tag === TAG)); });
      case "del": return void ed.change((m) => { m.overlays = (m.overlays || []).filter((o) => o.id !== id); });
      case "seek": { const o = ed.texts(TAG).find((x) => x.id === id); if (o) { ed.seek(o.from + 0.01); ed.redraw(); } return; }
      case "color": c.s.color = el.dataset.color; c.save(); restyle(ed, c); return ed.redraw();
    }
  },
  onDownload: (d) => {
    if (d.phase === "removed") return void refreshDls(ctx).then(() => ctx.editor()?.redraw());
    const run = st.run;
    if (!run || run.dl !== d.id || run.cancel) return;
    if (d.phase === "download") run.pct = d.total > 0 ? clamp((d.done / d.total) * 100, 0, 100) : null;
    else if (d.phase === "check") { run.pct = null; run.label = t("Checking the download…"); }
    else if (d.phase === "unpack") { run.pct = d.total > 0 && d.done > 0 ? clamp((d.done / d.total) * 100, 0, 100) : null; run.label = t("Unpacking…"); }
    ctx.editor()?.redraw();
  },
  start: (c) => {
    document.addEventListener("change", onChange);
    // The slider's number follows it while it is dragged; the lines change when it is let go.
    document.addEventListener("input", (e) => { const el = e.target; if (el.type === "range" && el.dataset.subSet && mySection(el)) { const out = el.parentElement.querySelector("output"); if (out) out.textContent = `${el.value}%`; } });
    // Enter in a line's box takes the text, as leaving the box does.
    document.addEventListener("keydown", (e) => { if (e.key === "Enter" && e.target.dataset?.subLine) e.target.blur(); });
    // What 0.1.0 downloaded (whisper.cpp and its models, up to 900 MB) is of no use any more.
    for (const id of OLD_DOWNLOADS) c.dl.remove(id).catch(() => {});
    refreshDls(c);
  },
  // For tools/subtitles-test.js and tools/subtitles-e2e.js.
  _pure: { fmtTime, srtTime, srtText, filesOf, hasMic, pickStream, parseVad, planChunks, cutPlan, parseResults, confidence, spread, wordsOf, wordsOnTimeline, captions, splitPhrase, widthOf, wrap, placeY, styleOf, itemOf, engineArgs, hear, LENGTHS, LANGS, ENGINES },
  lang: {
    ko: {
      "Turns what is said in a clip into subtitles, on this PC. Adds a Subtitles section to the editor: make them with one click, fix the words, style them, and they are part of the exported clip. Can also save an .srt file.": "클립에서 말한 내용을 이 PC에서 자막으로 만들어요. 편집기에 자막 섹션이 생겨요. 클릭 한 번으로 만들고, 글자를 고치고, 모양을 바꾸면 내보낸 클립에 그대로 들어가요. .srt 파일로 저장할 수도 있어요.",
      "Subtitles": "자막",
      "Speech engine": "음성 엔진",
      "Voice detector": "목소리 감지기",
      "Speech model (English and European languages)": "음성 모델 (영어와 유럽 언어)",
      "Speech model (Korean, Japanese and other languages)": "음성 모델 (한국어, 일본어와 그 밖의 언어)",
      "Reading the sound…": "소리를 읽는 중…",
      "The clip's sound could not be read.": "클립의 소리를 읽지 못했어요.",
      "Finding the voices…": "목소리를 찾는 중…",
      "The speech could not be read.": "말소리를 읽지 못했어요.",
      "Listening… (clip {n} of {total})": "듣는 중… (클립 {n}/{total})",
      "Listening…": "듣는 중…",
      "Getting ready…": "준비 중…",
      "Downloading the speech model…": "음성 모델 다운로드 중…",
      "Downloading the speech engine…": "음성 엔진 다운로드 중…",
      "There is no clip on the timeline to listen to.": "타임라인에 들을 클립이 없어요.",
      "No speech was found there. Try another sound under \"Listen to\".": "거기에서 말소리를 찾지 못했어요. \"들을 소리\"에서 다른 소리를 골라 보세요.",
      "No speech was found in this clip.": "이 클립에서 말소리를 찾지 못했어요.",
      "{n} subtitle lines added": "자막 {n}줄을 추가했어요",
      "Stopping…": "멈추는 중…",
      "There are no subtitles to save.": "저장할 자막이 없어요.",
      "Saved {name}": "{name} 저장됨",
      "Spoken in": "말한 언어",
      "English or another European language": "영어 또는 다른 유럽 언어",
      "Listen to": "들을 소리",
      "My microphone first": "내 마이크 먼저",
      "My microphone": "내 마이크",
      "The game or desktop": "게임 또는 데스크톱",
      "All the sound": "모든 소리",
      "Lines": "줄",
      "A few words at a time": "한 번에 몇 단어씩",
      "One short line": "짧은 한 줄",
      "Whole sentences": "문장 전체",
      "Make them again": "다시 만들기",
      "Make subtitles": "자막 만들기",
      "The first time, this downloads {size} (the speech engine and its model for this language). After that it works without internet. The sound never leaves this PC.": "처음에는 {size}를 다운로드해요 (음성 엔진과 이 언어의 모델). 그다음부터는 인터넷 없이 작동해요. 소리는 이 PC 밖으로 나가지 않아요.",
      "Making them again replaces every line below.": "다시 만들면 아래의 모든 줄이 바뀌어요.",
      "Made on this PC. The sound never leaves it.": "이 PC에서 만들어요. 소리는 밖으로 나가지 않아요.",
      "Go to this line": "이 줄로 이동",
      "Subtitle text": "자막 글자",
      "Delete this line": "이 줄 삭제",
      "Place": "위치",
      "Bottom": "아래",
      "Middle": "가운데",
      "Top": "위",
      "Save .srt file": ".srt 파일 저장",
      "Remove all": "모두 지우기",
      "The lines are part of the exported clip. Each is also a text on the picture: move or retime it there or on the timeline.": "자막은 내보낸 클립에 들어가요. 각 줄은 화면 위의 텍스트이기도 해서, 화면이나 타임라인에서 옮기거나 시간을 바꿀 수 있어요.",
      "Downloaded files": "다운로드한 파일",
      "The speech engine and the models the Subtitles plugin fetched. Removing one frees its space; it is downloaded again when it is needed.": "자막 플러그인이 받은 음성 엔진과 모델이에요. 지우면 공간이 비워지고, 필요할 때 다시 다운로드돼요.",
      "Nothing is downloaded yet.": "아직 다운로드한 것이 없어요.",
      "Checking the download…": "다운로드 확인 중…",
      "Unpacking…": "압축 푸는 중…",
    },
    el: {
      "Turns what is said in a clip into subtitles, on this PC. Adds a Subtitles section to the editor: make them with one click, fix the words, style them, and they are part of the exported clip. Can also save an .srt file.": "Μετατρέπει όσα λέγονται σε ένα κλιπ σε υπότιτλους, σε αυτό το PC. Προσθέτει την ενότητα Υπότιτλοι στον επεξεργαστή: φτιάξτε τους με ένα κλικ, διορθώστε τις λέξεις, αλλάξτε την εμφάνισή τους, και μπαίνουν στο κλιπ που εξάγετε. Μπορεί επίσης να αποθηκεύσει αρχείο .srt.",
      "Subtitles": "Υπότιτλοι",
      "Speech engine": "Μηχανή ομιλίας",
      "Voice detector": "Ανιχνευτής φωνής",
      "Speech model (English and European languages)": "Μοντέλο ομιλίας (αγγλικά και ευρωπαϊκές γλώσσες)",
      "Speech model (Korean, Japanese and other languages)": "Μοντέλο ομιλίας (κορεατικά, ιαπωνικά και άλλες γλώσσες)",
      "Reading the sound…": "Ανάγνωση του ήχου…",
      "The clip's sound could not be read.": "Ο ήχος του κλιπ δεν μπόρεσε να διαβαστεί.",
      "Finding the voices…": "Εντοπισμός των φωνών…",
      "The speech could not be read.": "Η ομιλία δεν μπόρεσε να διαβαστεί.",
      "Listening… (clip {n} of {total})": "Ακρόαση… (κλιπ {n} από {total})",
      "Listening…": "Ακρόαση…",
      "Getting ready…": "Προετοιμασία…",
      "Downloading the speech model…": "Λήψη του μοντέλου ομιλίας…",
      "Downloading the speech engine…": "Λήψη της μηχανής ομιλίας…",
      "There is no clip on the timeline to listen to.": "Δεν υπάρχει κλιπ στη γραμμή χρόνου για ακρόαση.",
      "No speech was found there. Try another sound under \"Listen to\".": "Δεν βρέθηκε ομιλία εκεί. Δοκιμάστε άλλον ήχο στο «Ακρόαση από».",
      "No speech was found in this clip.": "Δεν βρέθηκε ομιλία σε αυτό το κλιπ.",
      "{n} subtitle lines added": "Προστέθηκαν {n} γραμμές υποτίτλων",
      "Stopping…": "Διακοπή…",
      "There are no subtitles to save.": "Δεν υπάρχουν υπότιτλοι για αποθήκευση.",
      "Saved {name}": "Αποθηκεύτηκε το {name}",
      "Spoken in": "Γλώσσα ομιλίας",
      "English or another European language": "Αγγλικά ή άλλη ευρωπαϊκή γλώσσα",
      "Listen to": "Ακρόαση από",
      "My microphone first": "Πρώτα το μικρόφωνό μου",
      "My microphone": "Το μικρόφωνό μου",
      "The game or desktop": "Το παιχνίδι ή η επιφάνεια εργασίας",
      "All the sound": "Όλος ο ήχος",
      "Lines": "Γραμμές",
      "A few words at a time": "Λίγες λέξεις κάθε φορά",
      "One short line": "Μία σύντομη γραμμή",
      "Whole sentences": "Ολόκληρες προτάσεις",
      "Make them again": "Δημιουργία ξανά",
      "Make subtitles": "Δημιουργία υποτίτλων",
      "The first time, this downloads {size} (the speech engine and its model for this language). After that it works without internet. The sound never leaves this PC.": "Την πρώτη φορά γίνεται λήψη {size} (η μηχανή ομιλίας και το μοντέλο της για αυτή τη γλώσσα). Μετά λειτουργεί χωρίς ίντερνετ. Ο ήχος δεν φεύγει ποτέ από αυτό το PC.",
      "Making them again replaces every line below.": "Η νέα δημιουργία αντικαθιστά όλες τις παρακάτω γραμμές.",
      "Made on this PC. The sound never leaves it.": "Δημιουργούνται σε αυτό το PC. Ο ήχος δεν φεύγει ποτέ από αυτό.",
      "Go to this line": "Μετάβαση σε αυτή τη γραμμή",
      "Subtitle text": "Κείμενο υποτίτλου",
      "Delete this line": "Διαγραφή αυτής της γραμμής",
      "Place": "Θέση",
      "Bottom": "Κάτω",
      "Middle": "Μέση",
      "Top": "Πάνω",
      "Save .srt file": "Αποθήκευση αρχείου .srt",
      "Remove all": "Αφαίρεση όλων",
      "The lines are part of the exported clip. Each is also a text on the picture: move or retime it there or on the timeline.": "Οι γραμμές μπαίνουν στο κλιπ που εξάγετε. Καθεμία είναι και κείμενο πάνω στην εικόνα: μετακινήστε την ή αλλάξτε τον χρόνο της εκεί ή στη γραμμή χρόνου.",
      "Downloaded files": "Ληφθέντα αρχεία",
      "The speech engine and the models the Subtitles plugin fetched. Removing one frees its space; it is downloaded again when it is needed.": "Η μηχανή ομιλίας και τα μοντέλα που κατέβασε το πρόσθετο Υπότιτλοι. Η αφαίρεση ελευθερώνει χώρο· γίνεται ξανά λήψη όταν χρειαστεί.",
      "Nothing is downloaded yet.": "Δεν έχει γίνει ακόμη καμία λήψη.",
      "Checking the download…": "Έλεγχος της λήψης…",
      "Unpacking…": "Αποσυμπίεση…",
    },
  },
});
