// Checks the Subtitles plugin's own arithmetic (plugins/official/subtitles/subtitles.js) under node:
//   node tools/subtitles-test.js
// The script is run as Glint runs it (one argument, `Glint`), with a stand-in that only keeps what it registers.
const fs = require("fs");
const path = require("path");
const assert = require("assert");

let def = null;
const Glint = { register: (d) => { def = d; }, ctx: { s: {} }, t: (s) => s, esc: (s) => String(s) };
new Function("Glint", "navigator", "document", fs.readFileSync(path.join(__dirname, "..", "plugins", "official", "subtitles", "subtitles.js"), "utf8"))(Glint, { hardwareConcurrency: 8 }, {});
const P = def._pure;
let n = 0;
const check = (name, fn) => { fn(); n++; console.log(`ok  ${name}`); };

check("times", () => {
  assert.strictEqual(P.fmtTime(62.54), "1:02.5");
  assert.strictEqual(P.srtTime(3723.0415), "01:02:03,042");
  assert.strictEqual(P.srtTime(0), "00:00:00,000");
});

check("an .srt file", () => {
  assert.strictEqual(P.srtText([{ from: 0.6, to: 2.5, text: "Hello" }, { from: 3, to: 4.25, text: "Two\nlines" }]),
    "1\n00:00:00,600 --> 00:00:02,500\nHello\n\n2\n00:00:03,000 --> 00:00:04,250\nTwo\nlines\n");
});

check("silence and sounds are not lines", () => {
  for (const x of ["[BLANK_AUDIO]", " (music) ", "♪", "*laughs*", "...", ""]) assert.strictEqual(P.cleanText(x), "", x);
  assert.strictEqual(P.cleanText("  Nice   shot. "), "Nice shot.");
  assert.strictEqual(P.cleanText("안녕하세요"), "안녕하세요");
  assert.strictEqual(P.cleanText("(wait) what"), "(wait) what");
});

check("whisper's JSON", () => {
  const json = JSON.stringify({ transcription: [{ offsets: { from: 500, to: 1500 }, text: " Hi there" }, { offsets: { from: 1500, to: 2000 }, text: " [BLANK_AUDIO]" }, { offsets: { from: 3000, to: 3000 }, text: "x" }] });
  assert.deepStrictEqual(P.parseWhisper(json, 10), [{ a: 10.5, b: 11.5, text: "Hi there" }]);
  assert.deepStrictEqual(P.parseWhisper("{}", 0), []);
});

check("whisper's progress", () => {
  assert.strictEqual(P.progressOf(["x", "whisper_print_progress_callback: progress =  40%", "y"]), 40);
  assert.strictEqual(P.progressOf(["nothing"]), null);
});

check("the files of the footage", () => {
  const foot = [{ at: 0, len: 4, speed: 1, path: "a.mp4", from: 10, tracks: [] }, { at: 4, len: 2, speed: 2, path: "b.mp4", from: 0, tracks: [{ index: 0, title: "Mix" }] }, { at: 6, len: 3, speed: 1, path: "a.mp4", from: 2, tracks: [{ index: 1, title: "Desktop" }] }];
  assert.deepStrictEqual(P.filesOf(foot), [{ path: "a.mp4", lo: 2, hi: 14, tracks: [{ index: 1, title: "Desktop" }] }, { path: "b.mp4", lo: 0, hi: 4, tracks: [{ index: 0, title: "Mix" }] }]);
});

check("which sound is listened to", () => {
  const glint = [{ index: 0, title: "Mix" }, { index: 1, title: "Desktop" }, { index: 2, title: "Microphone" }];
  assert.strictEqual(P.pickStream(glint, "all"), 0);
  assert.strictEqual(P.pickStream(glint, "mic"), 2);
  assert.strictEqual(P.pickStream(glint, "game"), 1);
  assert.strictEqual(P.pickStream([...glint, { index: 3, title: "Game" }], "game"), 3);
  assert.strictEqual(P.pickStream([{ index: 0, title: "Audio" }], "mic"), 0);
  assert.strictEqual(P.pickStream([], "all"), 0);
});

check("lines on the timeline: plain, cut, reordered, sped up, removed", () => {
  const segs = { "a.mp4": [{ a: 1, b: 3, text: "one" }, { a: 5, b: 7, text: "two" }, { a: 9.8, b: 12, text: "three" }] };
  // The clip as it is.
  assert.deepStrictEqual(P.cuesOnTimeline([{ at: 0, len: 20, speed: 1, path: "a.mp4", from: 0 }], segs), [{ from: 1, to: 3, text: "one" }, { from: 5, to: 7, text: "two" }, { from: 9.8, to: 12, text: "three" }]);
  // Two pieces in the other order: 4..10 first, then 0..4. "three" is mostly outside the first piece (0.2 s of 2.2 s): left out.
  assert.deepStrictEqual(P.cuesOnTimeline([{ at: 0, len: 6, speed: 1, path: "a.mp4", from: 4 }, { at: 6, len: 4, speed: 1, path: "a.mp4", from: 0 }], segs), [{ from: 1, to: 3, text: "two" }, { from: 7, to: 9, text: "one" }]);
  // At double speed the lines are half as long.
  assert.deepStrictEqual(P.cuesOnTimeline([{ at: 2, len: 4, speed: 2, path: "a.mp4", from: 0 }], segs), [{ from: 2.5, to: 3.5, text: "one" }, { from: 4.5, to: 5.5, text: "two" }]);
  // A line in a part that is cut out is left out.
  assert.deepStrictEqual(P.cuesOnTimeline([{ at: 0, len: 20, speed: 1, path: "a.mp4", from: 0 }], segs, (sec) => sec > 4 && sec < 8).map((c) => c.text), ["one", "three"]);
  // Lines never overlap.
  const over = P.cuesOnTimeline([{ at: 0, len: 9, speed: 1, path: "b", from: 0 }], { b: [{ a: 0, b: 3, text: "x" }, { a: 2, b: 4, text: "y" }] });
  assert.deepStrictEqual(over, [{ from: 0, to: 2, text: "x" }, { from: 2, to: 4, text: "y" }]);
});

check("wrapping", () => {
  const fits = (n) => (l) => l.length <= n;
  assert.deepStrictEqual(P.wrap("short one", fits(20)), ["short one"]);
  assert.deepStrictEqual(P.wrap("aaaa bbbb cccc dddd", fits(12)), ["aaaa bbbb", "cccc dddd"]);
  assert.deepStrictEqual(P.wrap("aaaa bbbb cccc dddd eeee ffff gggg", fits(10)), ["aaaa bbbb", "cccc dddd", "eeee ffff gggg"]);
  assert.deepStrictEqual(P.wrap("unbreakableword", fits(4)), ["unbreakableword"]);
});

check("a line's item on the picture", () => {
  const style = P.styleOf({ size: 5, place: "bottom", color: "#FFE14D", outline: false });
  const it = P.itemOf({ from: 1, to: 2, text: "aaaa bbbb cccc dddd" }, style, (l, size) => l.length * size, "o1");
  assert.deepStrictEqual(it, { id: "o1", kind: "text", from: 1, to: 2, x: 0.5, y: 0.81, text: "aaaa bbbb\ncccc dddd", color: "#FFE14D", size: 0.05, outline: false, align: "center", tag: "subtitles" });
  assert.strictEqual(P.placeY(1, 0.05, "top"), 0.07);
  assert.strictEqual(P.placeY(2, 0.05, "middle"), 0.44);
  assert.deepStrictEqual(P.styleOf({}), { size: 0.05, place: "bottom", color: "#ffffff", outline: true });
  assert.strictEqual(P.styleOf({ size: 99 }).size, 0.12);
});

// The manifest's downloads are what Glint will fetch and check: each needs an https address, a size and a SHA-256.
check("the manifest's downloads", () => {
  const m = JSON.parse(fs.readFileSync(path.join(__dirname, "..", "plugins", "official", "subtitles", "plugin.json"), "utf8"));
  assert.ok(m.downloads.length >= 2);
  for (const d of m.downloads) assert.ok(/^[a-z0-9-]+$/.test(d.id) && d.url.startsWith("https://") && /^[0-9a-f]{64}$/.test(d.sha256) && d.size > 0, d.id);
  assert.ok(m.downloads.find((d) => d.id === "whisper" && d.unzip && d.run === "whisper-cli.exe"));
});

console.log(`${n} checks passed`);
