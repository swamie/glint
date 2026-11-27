// Bass Boost: a clip's sound with more bass, some grit, another speed or pitch, saved as a new clip.
//
// One list of numbers (`plan`) drives two engines that are built to sound the same:
//   - the preview: a Web Audio graph on the page's own <video>, changed live while a slider moves;
//   - the saved clip: an ffmpeg filter chain (`filterFor`), run as the plugin's own process.
// Both use the same biquads (RBJ, as Web Audio and ffmpeg both implement them), the same distortion curve and the
// same ceiling. tools/bass-test.js renders both and compares them.
//
// Speed is the "sped up" kind by default: the pitch moves with it. "Keep the pitch" stretches time instead.
// With a speed other than 1 the picture is encoded again; otherwise it is copied as it is and only the sound changes.
const { ctx, t, esc } = Glint;
const ID = "bass";
const RATE = 48000;
const ICON_B = '<svg viewBox="0 0 24 24"><path d="M4 10v4h3l4 3.500v-11L7 10z"/><path d="M14.500 9.500a3.500 3.500 0 0 1 0 5M17 7a7 7 0 0 1 0 10"/></svg>';

const NEUTRAL = { speed: 1, keep_pitch: false, bass: 0, bass_freq: 90, sub: 0, drive: 0, drive_all: false, treble: 0, volume: 100 };
const PRESETS = [
  ["bass", "Bass boosted", { bass: 8, sub: 5, drive: 25, volume: 120 }],
  ["heavy", "Heavy bass", { bass: 12, bass_freq: 100, sub: 7, drive: 60, volume: 140 }],
  ["fried", "Deep fried", { bass: 14, bass_freq: 110, sub: 8, drive: 90, drive_all: true, treble: 2, volume: 200 }],
  ["sped", "Sped up + bass", { speed: 1.22, bass: 9, sub: 5, drive: 55, treble: 1.5, volume: 130 }],
  ["nightcore", "Nightcore", { speed: 1.28, bass: 4, bass_freq: 115, treble: 2, volume: 115 }],
  ["slowed", "Slowed", { speed: 0.84, bass: 4, bass_freq: 80, treble: -2, volume: 115 }],
  ["clean", "No effects", {}],
];
const presetValues = (id) => ({ ...NEUTRAL, ...((PRESETS.find((p) => p[0] === id) || PRESETS[0])[2]) });
const DEFAULTS = { preset: "bass", ...presetValues("bass") };

// [key, min, max, step]
const RANGES = { speed: [0.5, 2, 0.01], bass: [0, 18, 0.5], bass_freq: [40, 250, 1], sub: [0, 12, 0.5], drive: [0, 100, 1], treble: [-12, 12, 0.5], volume: [50, 300, 5] };
const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
const dbToGain = (db) => Math.pow(10, db / 20);
const num = (x, d = 4) => String(+(+x).toFixed(d));

/** The settings as safe numbers: { speed, keepPitch, bass, bassFreq, sub, drive, driveAll, treble, volume }. */
function fxOf(s) {
  const v = (k) => { const [lo, hi, step] = RANGES[k], x = Number(s?.[k]); return Number.isFinite(x) ? clamp(Math.round(x / step) * step, lo, hi) : NEUTRAL[k]; };
  return { speed: +v("speed").toFixed(2), keepPitch: !!s?.keep_pitch, bass: v("bass"), bassFreq: v("bass_freq"), sub: v("sub"), drive: v("drive"), driveAll: !!s?.drive_all, treble: v("treble"), volume: v("volume") };
}
/** Nothing about the sound's tone or level changes (the speed may). */
const flat = (f) => f.bass === 0 && f.sub === 0 && f.drive === 0 && f.treble === 0 && f.volume === 100;
const neutral = (f) => flat(f) && f.speed === 1;

// The distortion's curve: a fixed soft clip. Drive pushes the sound into it, so more drive is more grit, not just more level.
const SHAPE_K = 3, SHAPE_NORM = Math.tanh(SHAPE_K);
const shape = (u) => Math.tanh(SHAPE_K * clamp(u, -1, 1)) / SHAPE_NORM;
/** RMS out / in of the drive and the curve for a typical bass level: what the distorted part is turned down by. */
function shaperGain(pre) {
  let a = 0, b = 0;
  for (let i = 0; i < 256; i++) { const x = 0.3 * Math.sin((2 * Math.PI * i) / 256), y = shape(x * pre); a += x * x; b += y * y; }
  return Math.sqrt(b / a);
}
// The ceiling after the limiter: untouched up to the knee, then bent towards knee + room, never past it.
const CEIL_KNEE = 0.8, CEIL_ROOM = 0.091;
const ceiling = (x) => { const a = Math.abs(x); return a <= CEIL_KNEE ? x : Math.sign(x) * (CEIL_KNEE + CEIL_ROOM * Math.tanh((a - CEIL_KNEE) / CEIL_ROOM)); };
// Butterworth stages (as plain Q): a 4th-order highpass is two stages, a Linkwitz-Riley split two equal ones.
const Q_B4 = [0.5412, 1.3066], Q_B2 = 0.7071;
const CROSS = 120, RUMBLE = 28, MUD = 250, SUB = 55, SUB_Q = 1.2, TREBLE = 6000;

/** Every number both engines need, from the settings. */
function plan(f) {
  const mix = f.drive / 100, pre = dbToGain(f.drive * 0.24);
  return {
    // Headroom: the sound is turned down by about half of what the boosts add, so they do not just hit the limiter.
    trim: dbToGain(-clamp(0.5 * (f.bass + 0.7 * f.sub + Math.max(0, f.treble)), 0, 12)),
    rumble: f.bass > 0 || f.sub > 0 ? RUMBLE : 10,   // what a boost would only waste headroom on
    mud: -clamp(0.35 * f.bass, 0, 5),                // a big shelf alone sounds muddy: a little out at 250 Hz
    bass: f.bass, bassFreq: f.bassFreq, sub: f.sub, treble: f.treble,
    dist: mix > 0, all: f.driveAll, pre,
    tone: f.driveAll ? 7000 : 3000,                  // the distortion's own harmonics, kept warm
    wet: mix / shaperGain(pre),
    lowClean: f.driveAll ? 1 - mix : 1 - 0.6 * mix,
    highClean: f.driveAll ? 1 - mix : 1,
    volume: f.volume / 100,
  };
}

/* ---------- the saved clip: ffmpeg ---------- */

/** `atempo` stages for a speed, each within 0.5 .. 2. */
function atempo(speed) {
  const out = [];
  let left = speed;
  while (left > 2 + 1e-9) { out.push("atempo=2"); left /= 2; }
  while (left < 0.5 - 1e-9) { out.push("atempo=0.5"); left /= 0.5; }
  out.push(`atempo=${num(left, 6)}`);
  return out.join(",");
}

/** The sound's filter chain (one input, one output): the value of ffmpeg's -af. */
function filterFor(f) {
  const p = plan(f), out = [`aresample=${RATE}`];
  // Speed first, as in the preview, where the player's rate comes before the graph.
  if (f.speed !== 1) out.push(f.keepPitch ? atempo(f.speed) : `asetrate=${Math.round(RATE * f.speed)},aresample=${RATE}`);
  if (flat(f)) return out.join(",");
  out.push(`volume=${num(p.trim, 5)}`);
  for (const q of Q_B4) out.push(`highpass=f=${p.rumble}:t=q:w=${q}`);
  if (p.mud) out.push(`equalizer=f=${MUD}:t=q:w=1:g=${num(p.mud)}`);
  if (p.bass) out.push(`bass=g=${num(p.bass)}:f=${p.bassFreq}:t=q:w=0.5`);
  if (p.sub) out.push(`equalizer=f=${SUB}:t=q:w=${SUB_Q}:g=${num(p.sub)}`);
  if (p.treble) out.push(`treble=g=${num(p.treble)}:f=${TREBLE}:t=q:w=0.5`);
  let chain = out.join(",");
  if (p.dist) {
    const lp = `lowpass=f=${CROSS}:t=q:w=${Q_B2},lowpass=f=${CROSS}:t=q:w=${Q_B2}`, hp = `highpass=f=${CROSS}:t=q:w=${Q_B2},highpass=f=${CROSS}:t=q:w=${Q_B2}`;
    const dist = `highpass=f=35:t=q:w=${Q_B2},volume=${num(p.pre, 5)},aeval=exprs='tanh(${SHAPE_K}*clip(val(ch)\\,-1\\,1))/${num(SHAPE_NORM, 7)}':c=same,highpass=f=18:t=q:w=${Q_B2},lowpass=f=${p.tone}:t=q:w=${Q_B2},volume=${num(p.wet, 5)}[bw]`;
    chain += p.all
      ? `,asplit=3[b1][b2][b3];[b1]${lp},volume=${num(p.lowClean, 5)}[bl];[b2]${hp},volume=${num(p.highClean, 5)}[bh];[b3]${dist}`
      : `,asplit=2[b1][b2];[b1]${lp},asplit=2[b3][b4];[b3]volume=${num(p.lowClean, 5)}[bl];[b2]${hp}[bh];[b4]${dist}`;
    chain += ";[bl][bh][bw]amix=inputs=3:normalize=0:duration=longest";
  }
  const k = CEIL_KNEE, r = CEIL_ROOM;
  return `${chain},volume=${num(p.volume)},alimiter=limit=0.85:attack=5:release=150:level=disabled:latency=1,` +
    `aeval=exprs='if(lte(abs(val(ch))\\,${k})\\,val(ch)\\,sgn(val(ch))*(${k}+${r}*tanh((abs(val(ch))-${k})/${r})))':c=same`;
}

const extOf = (path) => { const m = /\.(mp4|mkv|mov)$/i.exec(path || ""); return m ? m[1].toLowerCase() : "mp4"; };

/** ffmpeg's arguments for one clip: every sound track through the chain, the picture copied unless the speed changed.
 *  `titles`: the tracks' names ("Mix", "Microphone"), which an encoded track would otherwise lose. */
function argsFor(clip, f, out, titles = []) {
  const a = ["-hide_banner", "-nostdin", "-nostats", "-progress", "pipe:2", "-y", "-i", clip.path, "-map", "0:v:0", "-map", "0:a", "-map_metadata", "0"];
  if (f.speed === 1) a.push("-c:v", "copy");
  else {
    a.push("-vf", `setpts=PTS/${num(f.speed, 2)}`);
    if (clip.fps > 0) a.push("-r", num(Math.min(clip.fps, 240), 3));
    a.push("-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p");
  }
  a.push("-af", filterFor(f), "-c:a", "aac", "-b:a", "192k");
  titles.forEach((title, i) => { if (title) a.push(`-metadata:s:a:${i}`, `title=${title}`); if (titles.length > 1) a.push(`-disposition:a:${i}`, i ? "0" : "default"); });
  if (extOf(out) !== "mkv") a.push("-movflags", "+faststart");
  a.push(out);
  return a;
}

/** The word in the new clip's name. */
const labelOf = (f) => (f.bass > 0 || f.sub > 0 ? t("bass boosted") : f.speed > 1 ? t("sped up") : f.speed < 1 ? t("slowed") : t("boosted"));

/* ---------- the preview: Web Audio ---------- */

const st = {
  clipId: null,     // the clip on the page
  video: null,      // its <video>, kept over redraws
  failed: "",       // why the preview has no sound effects
  original: false,  // hear the clip as it is
  ac: null, n: null, routed: null,
  queue: [], now: null, cancel: false, note: "",
};

function curveOf(fn) {
  const size = 4097, out = new Float32Array(size);
  for (let i = 0; i < size; i++) out[i] = fn((i / (size - 1)) * 2 - 1);
  return out;
}

/** The graph, once: in -> ... -> out. `ac` may be an OfflineAudioContext (the test). */
function buildGraph(ac) {
  const bq = (type, freq, q) => { const x = ac.createBiquadFilter(); x.type = type; x.frequency.value = freq; if (q !== undefined) x.Q.value = type === "peaking" ? q : 20 * Math.log10(q); return x; };
  const gain = (v = 1) => { const g = ac.createGain(); g.gain.value = v; return g; };
  const chain = (...nodes) => { for (let i = 0; i < nodes.length - 1; i++) nodes[i].connect(nodes[i + 1]); return nodes[nodes.length - 1]; };
  const n = { in: gain(), trim: gain(), cut1: bq("highpass", 10, Q_B4[0]), cut2: bq("highpass", 10, Q_B4[1]), mud: bq("peaking", MUD, 1), bass: bq("lowshelf", 90), sub: bq("peaking", SUB, SUB_Q), treble: bq("highshelf", TREBLE) };
  const eq = chain(n.in, n.trim, n.cut1, n.cut2, n.mud, n.bass, n.sub, n.treble);
  // A Linkwitz-Riley split: low + high sum back flat.
  const low = chain(eq, bq("lowpass", CROSS, Q_B2), bq("lowpass", CROSS, Q_B2)), high = chain(eq, bq("highpass", CROSS, Q_B2), bq("highpass", CROSS, Q_B2));
  n.sum = gain(); n.lowClean = gain(); n.highClean = gain();
  chain(low, n.lowClean, n.sum);
  chain(high, n.highClean, n.sum);
  n.selLow = gain(); n.selAll = gain(0); n.pre = gain(); n.wet = gain(0);
  // Not oversampled: Chromium's oversampler delays the sound, and the distorted part is mixed with the clean one,
  // so a delay would change the sum (and the saved clip, which has none, would sound different).
  n.shaper = ac.createWaveShaper(); n.shaper.curve = curveOf(shape);
  n.tone = bq("lowpass", 3000, Q_B2);
  const dIn = bq("highpass", 35, Q_B2);
  low.connect(n.selLow); eq.connect(n.selAll);
  n.selLow.connect(dIn); n.selAll.connect(dIn);
  chain(dIn, n.pre, n.shaper, bq("highpass", 18, Q_B2), n.tone, n.wet, n.sum);
  n.master = gain();
  // The limiter, then the ceiling. The compressor adds make-up gain of its own (x1.1475 at these settings, measured
  // in Chromium), taken back out after it, so quiet sound passes at its own level. With it a loud bass tone is held
  // at 0.81 to 0.89, around the 0.85 the saved clip's limiter holds.
  n.lim = ac.createDynamicsCompressor();
  for (const [k, v] of Object.entries({ threshold: -3, knee: 3, ratio: 20, attack: 0.001, release: 0.15 })) n.lim[k].value = v;
  n.ceil = ac.createWaveShaper(); n.ceil.curve = curveOf(ceiling);
  n.out = gain();
  chain(n.sum, n.master, n.lim, gain(1 / 1.1475), n.ceil, n.out);
  return n;
}

/** Put the settings on the graph. `now`: at once (the first time, and the test); else over a few milliseconds, so nothing clicks. */
function setGraph(ac, n, f, now) {
  const p = plan(f);
  const set = (param, v) => { if (!Number.isFinite(v)) return; if (now) param.value = v; else param.setTargetAtTime(v, ac.currentTime, 0.02); };
  set(n.trim.gain, p.trim);
  set(n.cut1.frequency, p.rumble); set(n.cut2.frequency, p.rumble);
  set(n.mud.gain, p.mud);
  set(n.bass.frequency, p.bassFreq); set(n.bass.gain, p.bass);
  set(n.sub.gain, p.sub);
  set(n.treble.gain, p.treble);
  set(n.pre.gain, p.pre);
  set(n.tone.frequency, p.tone);
  set(n.selLow.gain, p.all ? 0 : 1); set(n.selAll.gain, p.all ? 1 : 0);
  set(n.lowClean.gain, p.lowClean); set(n.highClean.gain, p.highClean);
  set(n.wet.gain, p.dist ? p.wet : 0);
  set(n.master.gain, p.volume);
}

const heardOriginal = (f) => st.original || flat(f);

/** The video's rate and its sound's path follow the settings. */
function applyPreview() {
  const v = st.video, f = fxOf(ctx.s);
  if (!v) return;
  const rate = st.original ? 1 : f.speed;
  v.preservesPitch = st.original || f.keepPitch;
  if (v.defaultPlaybackRate !== rate) v.defaultPlaybackRate = rate;
  if (v.playbackRate !== rate) v.playbackRate = rate;
  if (!st.ac || !v._bbSrc) return;
  setGraph(st.ac, st.n, f, false);
  const to = heardOriginal(f) ? "dry" : "fx";
  if (st.routed === to && v._bbSrc === st.routedSrc) return;
  v._bbSrc.disconnect();
  v._bbSrc.connect(to === "fx" ? st.n.in : st.ac.destination);
  st.routed = to; st.routedSrc = v._bbSrc;
}

/** The video started playing: its sound goes through the graph from here on. */
function hook(v) {
  if (st.failed) return;
  try {
    if (!st.ac) {
      const AC = window.AudioContext || window.webkitAudioContext;
      st.ac = new AC({ latencyHint: "playback" });
      st.n = buildGraph(st.ac);
      st.n.out.connect(st.ac.destination);
      setGraph(st.ac, st.n, fxOf(ctx.s), true);
    }
    if (st.ac.state !== "running") st.ac.resume().catch(() => {});
    if (!v._bbSrc) { v._bbSrc = st.ac.createMediaElementSource(v); st.routed = null; }
  } catch (e) {
    console.warn("Glint Bass Boost: no Web Audio for the preview", e);
    st.failed = t("The preview cannot change the sound on this PC. The saved clip is still boosted.");
    ctx.render();
    return;
  }
  applyPreview();
}

function videoFor(clip) {
  if (st.video && st.video._bbClip === clip.id) return st.video;
  if (st.video) { try { st.video.pause(); st.video._bbSrc?.disconnect(); st.video.removeAttribute("src"); st.video.load(); } catch { /* it is gone anyway */ } }
  const v = document.createElement("video");
  v._bbClip = clip.id;
  v.className = "bb-video";
  v.controls = true;
  v.preload = "metadata";
  v.crossOrigin = "anonymous"; // Web Audio may only read a file that is loaded this way
  v.src = ctx.fileSrc(clip.path);
  v.addEventListener("play", () => hook(v));
  // A file that refuses to load this way is shown without it: it plays, with its own sound.
  v.addEventListener("error", () => {
    if (!v.crossOrigin || st.video !== v) return;
    v.removeAttribute("crossorigin");
    v.src = ctx.fileSrc(clip.path);
    st.failed = t("The preview cannot change the sound of this clip. The saved clip is still boosted.");
    ctx.render();
  }, { once: true });
  st.video = v;
  st.failed = "";
  st.routed = null;
  return v;
}

/* ---------- saving ---------- */

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const IN_WORK = /plugin-files[\\/]bass[\\/]work[\\/]/i;
const pageOpen = () => !!document.querySelector(`.plug-page[data-plugin="${ID}"]`);
const redraw = () => { if (pageOpen()) ctx.render(); else ctx.chrome(); };
const refreshLibrary = () => (typeof refreshClips === "function" ? Promise.resolve(refreshClips()).catch(() => {}) : Promise.resolve());

/** Where ffmpeg is in the result, in percent, from its last messages. */
function pctOf(log, total) {
  for (let i = (log || []).length - 1; i >= 0; i--) {
    const m = /out_time_(us|ms)=(\d+)/.exec(log[i]);
    if (m) return total > 0 ? clamp((+m[2] / 1e6 / total) * 100, 0, 100) : 0;
  }
  return 0;
}

function reasonOf(res) {
  const text = [res.error, ...(res.log || [])].join("\n");
  if (/matches no streams/i.test(text)) return t("This clip has no sound.");
  if (/Unknown encoder 'libx264'/i.test(text)) return t("This ffmpeg cannot encode the picture again, which another speed needs. Set the speed to 1.00x or install the full ffmpeg.");
  if (/No such filter|Option .* not found|Error (initializing|applying)/i.test(text)) return t("This ffmpeg is too old for Bass Boost. Update it in Settings.");
  return res.error || (res.log || []).filter((l) => !/^[a-z_]+=/.test(l)).slice(-1)[0] || t("ffmpeg stopped with code {code}", { code: res.code });
}

/** One clip through ffmpeg into the plugin's work folder, then into the library as a new clip beside the original. */
async function boostOne(clip, f, c) {
  const name = `bass-${Date.now()}.${extOf(clip.path)}`;
  const total = clip.duration / f.speed;
  // The tracks' names (the editor and joins tell a clip's tracks apart by them).
  const tracks = await c.invoke("editor_audio_tracks", { id: clip.id }).catch(() => null);
  if (Array.isArray(tracks) && !tracks.length) throw t("This clip has no sound.");
  const titles = (tracks || []).slice().sort((x, y) => x.index - y.index).map((x) => String(x.title || ""));
  const res = await c.proc.run("boost", argsFor(clip, f, `{work}/${name}`, titles), { out: "run.txt", onTick: (s) => {
    if (!st.now) return;
    st.now.pct = pctOf(s.log, total);
    const bar = document.querySelector(".bb-busy .progress i");
    if (bar) bar.style.width = `${Math.round(st.now.pct)}%`;
  } });
  if (st.cancel) throw "cancelled";
  if (res.code !== 0) throw reasonOf(res);
  // Only ffmpeg knows where the work folder is: its own words say where it wrote.
  const log = await c.work.read("run.txt.log");
  const at = log.split(/\r?\n/).map((l) => /^Output #0, [^']*'(.+)':\s*$/.exec(l)).find(Boolean);
  if (!at) throw t("The boosted clip was made, but Glint could not find it.");
  const [got] = await c.invoke("import_files", { paths: [at[1]], copy: true, folder: clip.folder || "" });
  if (!got?.clip) throw got?.error || t("The boosted clip could not be added to the library.");
  const id = got.clip.id;
  // The library copies it into the clips folder in the background; its name and the cleanup wait for that.
  await refreshLibrary();
  let copied = false;
  for (let i = 0; i < 480 && !copied; i++) {
    const k = c.clips.get(id);
    copied = !!k && !!k.path && !IN_WORK.test(k.path) && k.path !== at[1];
    if (!copied) { await sleep(250); if (i % 8 === 7) await refreshLibrary(); }
  }
  await c.invoke("rename_clip", { id, title: `${clip.title} (${labelOf(f)})` }).catch((e) => console.warn("Glint Bass Boost: the new clip keeps its working name", e));
  if (clip.game) await c.invoke("set_game", { ids: [id], game: clip.game }).catch(() => {});
  if (copied) await c.work.clear().catch(() => {});
  await refreshLibrary();
  return id;
}

async function runQueue(c) {
  if (st.now) return;
  st.cancel = false;
  st.note = "";
  // What an earlier run left behind, unless a clip of the library still plays from there (a copy that failed).
  if (!c.clips.list().some((k) => IN_WORK.test(k.path))) await c.work.clear().catch(() => {});
  let done = 0, last = "";
  while (st.queue.length && !st.cancel) {
    const job = st.queue.shift();
    st.now = { title: job.clip.title, pct: 0, encode: job.f.speed !== 1 };
    redraw();
    try { await boostOne(job.clip, job.f, c); done++; last = job.clip.title; }
    catch (e) {
      if (e === "cancelled" || st.cancel) break;
      console.error("Glint Bass Boost:", e);
      st.note = t("{title}: {reason}", { title: job.clip.title, reason: String(e?.message ?? e) });
    }
  }
  const cancelled = st.cancel;
  st.now = null; st.queue = []; st.cancel = false;
  redraw();
  if (cancelled) c.flash(t("Bass boost cancelled"));
  else if (done === 1) c.flash(t("{title} is in the library, boosted", { title: last }));
  else if (done > 1) c.flash(t("{n} boosted clips are in the library", { n: done }));
  else if (st.note) c.flash(st.note, true);
}

function enqueue(clips, c) {
  const f = fxOf(c.s);
  if (neutral(f)) return c.flash(t("Nothing is changed yet. Pick a preset or move a slider first."), true);
  const list = clips.filter((k) => k && k.path && !k.draft && !k.cloud_only);
  if (!list.length) return;
  st.queue.push(...list.map((clip) => ({ clip, f })));
  redraw();
  runQueue(c).catch(c.fail);
}

/* ---------- the page ---------- */

const semis = (v) => { const x = 12 * Math.log2(v); return `${x >= 0 ? "+" : ""}${x.toFixed(1)} st`; };
const dB = (v) => `${v > 0 ? "+" : ""}${num(v, 1)} dB`;
const FMT = {
  speed: (v, f) => `${v.toFixed(2)}x${f.keepPitch || v === 1 ? "" : `  (${semis(v)})`}`,
  bass: dB, sub: dB, treble: dB,
  bass_freq: (v) => `${v} Hz`,
  drive: (v) => `${v}%`,
  volume: (v) => `${v}%`,
};
const fmt = (key, s) => FMT[key](fxOf(s)[{ bass_freq: "bassFreq" }[key] || key], fxOf(s));

function slider(key, s) {
  const [lo, hi, step] = RANGES[key];
  return `<div class="bb-range"><input type="range" min="${lo}" max="${hi}" step="${step}" value="${fxOf(s)[{ bass_freq: "bassFreq" }[key] || key]}" data-bb="${key}"><output>${esc(fmt(key, s))}</output></div>`;
}
const check = (key, s) => `<label class="switch"><input type="checkbox" data-bb="${key}" ${s[key] ? "checked" : ""}><i></i></label>`;
const presetName = (s) => (PRESETS.some((p) => p[0] === s.preset) ? s.preset : "custom");

function renderPage(c) {
  const s = c.s, f = fxOf(s), clips = c.clips.list().filter((k) => !k.draft && !k.cloud_only && k.path);
  const clip = clips.find((k) => k.id === st.clipId) || null;
  if (!clip) st.clipId = null;
  const busy = st.now ? `<div class="note bb-busy"><div class="bb-busy-main"><b>${esc(t("Boosting {title}…", { title: st.now.title }))}</b>
      <span>${esc(st.queue.length ? t("{n} more waiting", { n: st.queue.length }) : st.now.encode ? t("The speed changed, so the picture is encoded again. This takes a little longer.") : t("The picture is copied as it is, only the sound changes."))}</span>
      <div class="progress"><i style="width:${Math.round(st.now.pct)}%"></i></div></div>
      <button type="button" class="btn sm" data-pact="cancel">${esc(t("Cancel"))}</button></div>` : "";
  const opts = [`<option value=""${clip ? "" : " selected"}>${esc(t("Pick a clip"))}</option>`, ...clips.map((k) => `<option value="${esc(k.id)}"${clip && k.id === clip.id ? " selected" : ""}>${esc(k.title)}</option>`)].join("");
  const names = [...PRESETS.map(([id, name]) => [id, t(name)]), ...(presetName(s) === "custom" ? [["custom", t("Custom")]] : [])];
  return `<div class="page-head"><div><h1>${esc(t("Bass Boost"))}</h1><div class="sub">${esc(t("More bass, a different speed or pitch, saved as a new clip"))}</div></div></div>
    <div class="section"><h2>${esc(t("Clip"))}</h2>
      ${c.row(t("Clip"), t("Or right-click a clip in the library and pick Bass boost"), `<select class="field bb-clip" data-bb-clip>${opts}</select>`)}
      ${clip ? `<div class="bb-stage"><div class="bb-slot"></div>
        <div class="bb-stage-row"><div class="bb-ab"><button type="button" class="btn sm${st.original ? "" : " primary"}" data-pact="ab-fx">${esc(t("Boosted"))}</button><button type="button" class="btn sm${st.original ? " primary" : ""}" data-pact="ab-dry">${esc(t("Original"))}</button></div>
        <span class="bb-hint">${esc(st.failed || t("Press play and move the sliders: you hear the change at once."))}</span></div></div>` : ""}
    </div>
    <div class="section"><h2>${esc(t("Sound"))}</h2>
      ${c.row(t("Preset"), "", `<select class="field" data-bb-preset>${names.map(([v, l]) => `<option value="${esc(v)}"${v === presetName(s) ? " selected" : ""}>${esc(l)}</option>`).join("")}</select>`)}
      ${c.row(t("Bass boost"), t("How much louder the low end is"), slider("bass", s))}
      ${c.row(t("Bass range"), t("Everything below this is boosted"), slider("bass_freq", s))}
      ${c.row(t("Sub punch"), t("An extra push around 55 Hz, the part you feel"), slider("sub", s))}
      ${c.row(t("Distortion"), t("Grit on the bass, which also makes it heard on small speakers"), slider("drive", s))}
      ${c.row(t("Distort everything"), t("Off keeps voices and game sound clean"), check("drive_all", s))}
      ${c.row(t("Treble"), "", slider("treble", s))}
      ${c.row(t("Volume"), t("A limiter keeps it from clipping"), slider("volume", s))}
    </div>
    <div class="section"><h2>${esc(t("Speed"))}</h2>
      ${c.row(t("Speed"), t("The pitch moves with the speed, as in sped up and slowed edits"), slider("speed", s))}
      ${c.row(t("Keep the pitch"), t("Changes the speed only, voices stay as they are"), check("keep_pitch", s))}
    </div>
    <div class="section"><h2>${esc(t("Save"))}</h2>
      ${busy}${st.note ? `<div class="note bad">${esc(st.note)}</div>` : ""}
      ${c.row(t("Save as a new clip"), f.speed !== 1 ? t("The original stays as it is. With another speed the picture is encoded again.") : t("The original stays as it is. The picture is copied, so this is quick."),
        `<button type="button" class="btn primary" data-pact="save" ${clip && !st.now && !neutral(f) ? "" : "disabled"}>${ICON_B}<span>${esc(t("Save boosted clip"))}</span></button>`)}
      <p class="bb-foot">${esc(t("Double-click a slider to put it back. To boost several clips at once, select them in the library and right-click."))}</p>
    </div>`;
}

/** After a redraw: the clip's video goes back into its place (the same element, so it keeps playing). */
function afterPage(c, host) {
  const slot = host?.querySelector(".bb-slot");
  const clip = st.clipId && c.clips.get(st.clipId);
  if (!slot || !clip) return;
  slot.appendChild(videoFor(clip));
  applyPreview();
}

const mine = (el) => el?.closest?.(`.plug-page[data-plugin="${ID}"]`);

function setValue(key, value) {
  ctx.s[key] = value;
  ctx.s.preset = "custom";
}

function onInput(e) {
  const el = e.target, key = el.dataset?.bb;
  if (!key || el.type !== "range" || !mine(el)) return;
  setValue(key, +el.value);
  const out = el.parentElement.querySelector("output");
  if (out) out.textContent = fmt(key, ctx.s);
  applyPreview();
}

function onChange(e) {
  const el = e.target;
  if (!mine(el)) return;
  if (el.matches("[data-bb-clip]")) { st.clipId = el.value || null; st.original = false; return ctx.render(); }
  if (el.matches("[data-bb-preset]")) {
    if (el.value !== "custom") Object.assign(ctx.s, presetValues(el.value), { preset: el.value });
    ctx.save();
    return ctx.render();
  }
  const key = el.dataset?.bb;
  if (!key) return;
  if (el.type === "checkbox") setValue(key, el.checked);
  ctx.save();
  ctx.render(); // the preset's name, the hints and the Save button follow
}

function onDouble(e) {
  const el = e.target.closest?.(".bb-range")?.querySelector("input");
  if (!el || !mine(el)) return;
  const key = el.dataset.bb, was = presetName(ctx.s);
  ctx.s[key] = presetValues(was === "custom" ? "clean" : was)[key];
  ctx.save();
  ctx.render();
}

Glint.register({
  defaults: { ...DEFAULTS },
  pages: [{
    id: "bass", title: "Bass Boost", icon: ICON_B,
    render: renderPage,
    after: afterPage,
    badge: () => (st.now ? String(st.queue.length + 1) : ""),
  }],
  clipMenu: (clips) => {
    const list = clips.filter((k) => !k.draft && !k.cloud_only);
    if (!list.length) return [];
    return [{ id: "boost", icon: ICON_B, label: list.length > 1 ? t("Bass boost {n} clips", { n: list.length }) : t("Bass boost…") }];
  },
  onClipMenu: (id, clips, c) => {
    if (id !== "boost") return;
    const list = clips.filter((k) => !k.draft && !k.cloud_only);
    if (list.length > 1) return enqueue(list, c);
    if (!list[0]) return;
    st.clipId = list[0].id;
    st.original = false;
    c.go(`p:${ID}:bass`);
  },
  onAct: (name, el, c) => {
    switch (name) {
      case "save": { const clip = st.clipId && c.clips.get(st.clipId); return clip ? enqueue([clip], c) : undefined; }
      case "cancel": st.cancel = true; st.queue = []; return void c.proc.stop("boost");
      case "ab-fx": case "ab-dry": st.original = name === "ab-dry"; applyPreview(); return c.render();
    }
  },
  busy: () => !!st.now,
  start: () => {
    document.addEventListener("input", onInput);
    document.addEventListener("change", onChange);
    document.addEventListener("dblclick", onDouble);
  },
  // For tools/bass-test.js.
  _pure: { NEUTRAL, PRESETS, presetValues, fxOf, flat, neutral, plan, shape, ceiling, shaperGain, atempo, filterFor, argsFor, extOf, pctOf, buildGraph, setGraph },
  lang: {
    ko: {
      "Adds more bass to a clip's sound, with distortion, treble and volume to taste, and can speed it up or slow it down with the pitch following. You hear the change while you move the sliders, then save it as a new clip.": "클립 소리에 저음을 더하고, 왜곡과 고음, 음량을 취향대로 조절해요. 음높이가 함께 변하는 빠르게 또는 느리게도 할 수 있어요. 슬라이더를 움직이는 동안 바로 들어 보고 새 클립으로 저장해요.",
      "Bass Boost": "베이스 부스트",
      "More bass, a different speed or pitch, saved as a new clip": "저음을 더하고 속도나 음높이를 바꿔 새 클립으로 저장해요",
      "Bass boosted": "베이스 부스트", "Heavy bass": "묵직한 베이스", "Deep fried": "딥 프라이드", "Sped up + bass": "빠르게 + 베이스", "Nightcore": "나이트코어", "Slowed": "느리게", "No effects": "효과 없음", "Custom": "사용자 설정",
      "bass boosted": "베이스 부스트", "sped up": "빠르게", "slowed": "느리게", "boosted": "부스트",
      "Clip": "클립", "Pick a clip": "클립 선택",
      "Or right-click a clip in the library and pick Bass boost": "또는 라이브러리에서 클립을 오른쪽 클릭하고 베이스 부스트를 골라요",
      "Boosted": "부스트", "Original": "원본",
      "Press play and move the sliders: you hear the change at once.": "재생을 누르고 슬라이더를 움직이면 바로 달라진 소리가 들려요.",
      "The preview cannot change the sound on this PC. The saved clip is still boosted.": "이 PC에서는 미리 듣기에 효과를 넣을 수 없어요. 저장한 클립에는 효과가 들어가요.",
      "The preview cannot change the sound of this clip. The saved clip is still boosted.": "이 클립은 미리 듣기에 효과를 넣을 수 없어요. 저장한 클립에는 효과가 들어가요.",
      "Sound": "소리", "Preset": "프리셋",
      "Bass boost": "베이스 부스트", "How much louder the low end is": "저음이 얼마나 커지는지",
      "Bass range": "베이스 범위", "Everything below this is boosted": "이 주파수 아래가 모두 커져요",
      "Sub punch": "서브 펀치", "An extra push around 55 Hz, the part you feel": "몸으로 느끼는 55 Hz 부근을 더 밀어 줘요",
      "Distortion": "왜곡", "Grit on the bass, which also makes it heard on small speakers": "저음에 거친 질감을 더해 작은 스피커에서도 들리게 해요",
      "Distort everything": "전체 왜곡", "Off keeps voices and game sound clean": "끄면 목소리와 게임 소리는 깨끗하게 남아요",
      "Treble": "고음", "Volume": "음량", "A limiter keeps it from clipping": "리미터가 소리가 깨지지 않게 잡아 줘요",
      "Speed": "속도", "The pitch moves with the speed, as in sped up and slowed edits": "스페드 업이나 슬로우드 편집처럼 음높이가 속도를 따라가요",
      "Keep the pitch": "음높이 유지", "Changes the speed only, voices stay as they are": "속도만 바꾸고 목소리는 그대로 둬요",
      "Save": "저장", "Save as a new clip": "새 클립으로 저장",
      "The original stays as it is. With another speed the picture is encoded again.": "원본은 그대로 남아요. 속도를 바꾸면 화면을 다시 인코딩해요.",
      "The original stays as it is. The picture is copied, so this is quick.": "원본은 그대로 남아요. 화면은 복사만 해서 금방 끝나요.",
      "Save boosted clip": "부스트한 클립 저장",
      "Double-click a slider to put it back. To boost several clips at once, select them in the library and right-click.": "슬라이더를 두 번 클릭하면 원래 값으로 돌아가요. 여러 클립을 한꺼번에 부스트하려면 라이브러리에서 선택한 뒤 오른쪽 클릭해요.",
      "Boosting {title}…": "{title} 부스트하는 중…", "{n} more waiting": "{n}개 대기 중",
      "The speed changed, so the picture is encoded again. This takes a little longer.": "속도가 바뀌어서 화면을 다시 인코딩해요. 조금 더 걸려요.",
      "The picture is copied as it is, only the sound changes.": "화면은 그대로 복사하고 소리만 바꿔요.",
      "Cancel": "취소",
      "Bass boost {n} clips": "클립 {n}개 베이스 부스트", "Bass boost…": "베이스 부스트…",
      "Nothing is changed yet. Pick a preset or move a slider first.": "아직 바뀐 게 없어요. 먼저 프리셋을 고르거나 슬라이더를 움직여요.",
      "Bass boost cancelled": "베이스 부스트를 취소했어요",
      "{title} is in the library, boosted": "부스트한 {title}이(가) 라이브러리에 있어요",
      "{n} boosted clips are in the library": "부스트한 클립 {n}개가 라이브러리에 있어요",
      "{title}: {reason}": "{title}: {reason}",
      "This clip has no sound.": "이 클립에는 소리가 없어요.",
      "This ffmpeg cannot encode the picture again, which another speed needs. Set the speed to 1.00x or install the full ffmpeg.": "이 ffmpeg는 속도를 바꿀 때 필요한 화면 재인코딩을 할 수 없어요. 속도를 1.00x로 두거나 전체 버전 ffmpeg를 설치해요.",
      "This ffmpeg is too old for Bass Boost. Update it in Settings.": "이 ffmpeg는 베이스 부스트를 쓰기에 너무 오래됐어요. 설정에서 업데이트해요.",
      "ffmpeg stopped with code {code}": "ffmpeg가 코드 {code}(으)로 멈췄어요",
      "The boosted clip was made, but Glint could not find it.": "부스트한 클립을 만들었지만 Glint가 찾지 못했어요.",
      "The boosted clip could not be added to the library.": "부스트한 클립을 라이브러리에 넣지 못했어요.",
    },
    el: {
      "Adds more bass to a clip's sound, with distortion, treble and volume to taste, and can speed it up or slow it down with the pitch following. You hear the change while you move the sliders, then save it as a new clip.": "Προσθέτει περισσότερα μπάσα στον ήχο ενός κλιπ, με παραμόρφωση, πρίμα και ένταση όπως σου αρέσει, και μπορεί να το επιταχύνει ή να το επιβραδύνει με τον τόνο να ακολουθεί. Ακούς την αλλαγή όσο κουνάς τα ρυθμιστικά και μετά το αποθηκεύεις ως νέο κλιπ.",
      "Bass Boost": "Ενίσχυση μπάσων",
      "More bass, a different speed or pitch, saved as a new clip": "Περισσότερα μπάσα, άλλη ταχύτητα ή τόνος, αποθηκευμένα ως νέο κλιπ",
      "Bass boosted": "Ενισχυμένα μπάσα", "Heavy bass": "Βαριά μπάσα", "Deep fried": "Deep fried", "Sped up + bass": "Επιτάχυνση + μπάσα", "Nightcore": "Nightcore", "Slowed": "Αργό", "No effects": "Χωρίς εφέ", "Custom": "Προσαρμοσμένο",
      "bass boosted": "ενισχυμένα μπάσα", "sped up": "γρήγορο", "slowed": "αργό", "boosted": "ενισχυμένο",
      "Clip": "Κλιπ", "Pick a clip": "Διάλεξε κλιπ",
      "Or right-click a clip in the library and pick Bass boost": "Ή κάνε δεξί κλικ σε ένα κλιπ στη βιβλιοθήκη και διάλεξε Ενίσχυση μπάσων",
      "Boosted": "Ενισχυμένο", "Original": "Αρχικό",
      "Press play and move the sliders: you hear the change at once.": "Πάτα αναπαραγωγή και κούνα τα ρυθμιστικά: ακούς την αλλαγή αμέσως.",
      "The preview cannot change the sound on this PC. The saved clip is still boosted.": "Η προεπισκόπηση δεν μπορεί να αλλάξει τον ήχο σε αυτό το PC. Το αποθηκευμένο κλιπ ενισχύεται κανονικά.",
      "The preview cannot change the sound of this clip. The saved clip is still boosted.": "Η προεπισκόπηση δεν μπορεί να αλλάξει τον ήχο αυτού του κλιπ. Το αποθηκευμένο κλιπ ενισχύεται κανονικά.",
      "Sound": "Ήχος", "Preset": "Προεπιλογή",
      "Bass boost": "Ενίσχυση μπάσων", "How much louder the low end is": "Πόσο πιο δυνατά ακούγονται τα χαμηλά",
      "Bass range": "Εύρος μπάσων", "Everything below this is boosted": "Ενισχύεται ό,τι βρίσκεται κάτω από αυτό",
      "Sub punch": "Ώθηση sub", "An extra push around 55 Hz, the part you feel": "Επιπλέον ώθηση γύρω στα 55 Hz, το κομμάτι που νιώθεις",
      "Distortion": "Παραμόρφωση", "Grit on the bass, which also makes it heard on small speakers": "Τραχύτητα στα μπάσα, που τα κάνει να ακούγονται και σε μικρά ηχεία",
      "Distort everything": "Παραμόρφωση σε όλα", "Off keeps voices and game sound clean": "Όταν είναι κλειστό, οι φωνές και ο ήχος του παιχνιδιού μένουν καθαρά",
      "Treble": "Πρίμα", "Volume": "Ένταση", "A limiter keeps it from clipping": "Ένας περιοριστής δεν το αφήνει να ψαλιδιστεί",
      "Speed": "Ταχύτητα", "The pitch moves with the speed, as in sped up and slowed edits": "Ο τόνος ακολουθεί την ταχύτητα, όπως στα sped up και slowed βίντεο",
      "Keep the pitch": "Διατήρηση τόνου", "Changes the speed only, voices stay as they are": "Αλλάζει μόνο την ταχύτητα, οι φωνές μένουν όπως είναι",
      "Save": "Αποθήκευση", "Save as a new clip": "Αποθήκευση ως νέο κλιπ",
      "The original stays as it is. With another speed the picture is encoded again.": "Το αρχικό μένει όπως είναι. Με άλλη ταχύτητα η εικόνα κωδικοποιείται ξανά.",
      "The original stays as it is. The picture is copied, so this is quick.": "Το αρχικό μένει όπως είναι. Η εικόνα αντιγράφεται, οπότε γίνεται γρήγορα.",
      "Save boosted clip": "Αποθήκευση ενισχυμένου κλιπ",
      "Double-click a slider to put it back. To boost several clips at once, select them in the library and right-click.": "Κάνε διπλό κλικ σε ένα ρυθμιστικό για να το επαναφέρεις. Για να ενισχύσεις πολλά κλιπ μαζί, επίλεξέ τα στη βιβλιοθήκη και κάνε δεξί κλικ.",
      "Boosting {title}…": "Ενίσχυση του {title}…", "{n} more waiting": "{n} ακόμη σε αναμονή",
      "The speed changed, so the picture is encoded again. This takes a little longer.": "Η ταχύτητα άλλαξε, οπότε η εικόνα κωδικοποιείται ξανά. Παίρνει λίγο περισσότερο.",
      "The picture is copied as it is, only the sound changes.": "Η εικόνα αντιγράφεται όπως είναι, αλλάζει μόνο ο ήχος.",
      "Cancel": "Άκυρο",
      "Bass boost {n} clips": "Ενίσχυση μπάσων σε {n} κλιπ", "Bass boost…": "Ενίσχυση μπάσων…",
      "Nothing is changed yet. Pick a preset or move a slider first.": "Δεν έχει αλλάξει τίποτα ακόμη. Διάλεξε πρώτα μια προεπιλογή ή κούνα ένα ρυθμιστικό.",
      "Bass boost cancelled": "Η ενίσχυση μπάσων ακυρώθηκε",
      "{title} is in the library, boosted": "Το {title} είναι στη βιβλιοθήκη, ενισχυμένο",
      "{n} boosted clips are in the library": "{n} ενισχυμένα κλιπ είναι στη βιβλιοθήκη",
      "{title}: {reason}": "{title}: {reason}",
      "This clip has no sound.": "Αυτό το κλιπ δεν έχει ήχο.",
      "This ffmpeg cannot encode the picture again, which another speed needs. Set the speed to 1.00x or install the full ffmpeg.": "Αυτό το ffmpeg δεν μπορεί να κωδικοποιήσει ξανά την εικόνα, κάτι που χρειάζεται η άλλη ταχύτητα. Βάλε την ταχύτητα στο 1.00x ή εγκατάστησε το πλήρες ffmpeg.",
      "This ffmpeg is too old for Bass Boost. Update it in Settings.": "Αυτό το ffmpeg είναι πολύ παλιό για την Ενίσχυση μπάσων. Ενημέρωσέ το στις Ρυθμίσεις.",
      "ffmpeg stopped with code {code}": "Το ffmpeg σταμάτησε με κωδικό {code}",
      "The boosted clip was made, but Glint could not find it.": "Το ενισχυμένο κλιπ δημιουργήθηκε, αλλά το Glint δεν το βρήκε.",
      "The boosted clip could not be added to the library.": "Το ενισχυμένο κλιπ δεν μπόρεσε να προστεθεί στη βιβλιοθήκη.",
    },
  },
});
