// From a stored upload to a report: pick the Glint source files the faults
// point at, fetch them from GitHub at the user's version, and ask Claude once.
//
// What one run can cost is bounded here: the diagnostics and the source are
// each cut to a fixed number of characters before they go into the prompt
// (roughly 125,000 input tokens at the very most), and the answer to
// MAX_OUTPUT_TOKENS.
import { MODULES, REPORT_SCHEMA, SYSTEM } from "./prompt.js";

const API_VERSION = "2026-10-21";
const FALLBACK_BETA = "server-side-fallback-2026-07-01";
const MAX_OUTPUT_TOKENS = 32000;

/** Characters of each diagnostics section that reach the prompt. Logs keep their end. */
const PROMPT_CUT = { description: 2400, glint: 8000, system: 8000, settings: 24000, ffmpeg: 6000, processes: 3000, crashes: 10000, data: 8000, log_faults: 24000, log_tail: 70000 };
/** Source: how many files, and characters per file and in all. */
const MAX_FILES = 6;
const FILE_CHARS = 60000;
const SOURCE_CHARS = 220000;
/** Lines kept on each side of a matching line when a large file is excerpted. */
const AROUND = 45;

/** An error that trying again will not fix (a bad request, a bad key, a refusal). */
export class Permanent extends Error {}

/* ---------- which source files ---------- */

/** The message of a log line, without its "2026-11-18 12:34:56.789 " stamp. */
const message = (line) => line.replace(/^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d(?:\.\d+)?\s+/, "");

/** Files to attach, the most implicated first: scored by how many fault lines carry their log prefixes or words. */
export function pickFiles(bundle) {
  const faults = String(bundle.log_faults || "").toLowerCase();
  const rest = `${bundle.description || ""}\n${JSON.stringify(bundle.crashes || "")}`.toLowerCase();
  const lines = faults.split("\n").map(message);
  const scores = {};
  for (const [file, m] of Object.entries(MODULES)) {
    let score = 0;
    for (const p of m.log || []) score += 3 * lines.filter((l) => l.startsWith(p)).length;
    for (const w of m.words || []) score += lines.filter((l) => l.includes(w)).length + (rest.includes(w) ? 1 : 0);
    // A file named outright ("recorder.rs:1338") is wanted.
    if (faults.includes(file) || rest.includes(file)) score += 5;
    if (score > 0) scores[file] = score;
  }
  const picked = Object.entries(scores).sort((a, b) => b[1] - a[1]).map(([f]) => f).slice(0, MAX_FILES);
  // Nothing pointed anywhere: the capture path is where most trouble starts.
  return picked.length ? picked : ["recorder.rs", "core.rs"];
}

/** Phrases of three or more plain words from the fault lines: what to look for in a source file's string literals. */
function phrases(bundle) {
  const out = new Set();
  for (const line of String(bundle.log_faults || "").split("\n")) {
    for (const run of message(line).split(/[^A-Za-z' ]+/)) {
      const words = run.trim().split(/\s+/).filter(Boolean);
      for (let i = 0; i + 3 <= words.length && out.size < 400; i++) out.add(words.slice(i, i + 3).join(" "));
    }
  }
  return [...out];
}

const numbered = (lines, from, to) => lines.slice(from, to).map((l, i) => `${String(from + i + 1).padStart(5)}  ${l}`).join("\n");

/** A large file cut down: its functions (with line numbers) and the regions around lines that hold a fault's words. */
export function excerpt(text, needles, max) {
  const lines = text.split("\n");
  const outline = lines.map((l, i) => (/^\s*(pub(\([a-z]+\))? )?(async )?(unsafe )?fn \w+/.test(l) ? `${String(i + 1).padStart(5)}  ${l.trim().replace(/\s*\{$/, "")}` : null)).filter(Boolean).join("\n").slice(0, max / 4);
  const hits = [];
  lines.forEach((l, i) => { if (l.includes('"') && needles.some((n) => l.includes(n))) hits.push(i); });
  const regions = [];
  for (const h of hits) {
    const from = Math.max(0, h - AROUND), to = Math.min(lines.length, h + AROUND + 1);
    const last = regions[regions.length - 1];
    if (last && from <= last[1]) last[1] = to; else regions.push([from, to]);
  }
  let body = "", left = max - outline.length;
  for (const [from, to] of regions) {
    const part = numbered(lines, from, to);
    if (part.length > left) break;
    body += `\n\n// lines ${from + 1} to ${to}\n${part}`;
    left -= part.length;
  }
  return `// ${lines.length} lines in all. Functions:\n${outline}${body || "\n\n// (no line of this file matched the faults; only the function list is shown)"}`;
}

/** Fetch the picked files at `v<version>`, else at main. -> { ref, files: [{ path, excerpt, text }] } */
export async function fetchSources(env, bundle, files) {
  const base = (env.SOURCE_BASE_URL || `https://raw.githubusercontent.com/${env.REPO || "swamie/glint"}`).replace(/\/$/, "");
  const headers = { "User-Agent": "glint-doctor" };
  if (env.GITHUB_TOKEN) headers.Authorization = `Bearer ${env.GITHUB_TOKEN}`;
  const get = async (ref, file) => {
    try {
      const r = await fetch(`${base}/${ref}/src-tauri/src/${file}`, { headers });
      return r.ok ? await r.text() : null;
    } catch { return null; }
  };
  // One probe decides the ref: the version's tag if it exists, else main.
  let ref = "main";
  if (bundle.glint_version && (await get(`v${bundle.glint_version}`, "main.rs")) !== null) ref = `v${bundle.glint_version}`;
  const needles = phrases(bundle);
  const out = [];
  let left = SOURCE_CHARS;
  for (const file of files) {
    const text = await get(ref, file);
    if (text === null) continue;
    const max = Math.min(FILE_CHARS, left);
    if (max < 4000) break;
    const whole = numbered(text.split("\n"), 0, undefined);
    const shown = whole.length <= max ? { excerpt: false, text: whole } : { excerpt: true, text: excerpt(text, needles, max) };
    out.push({ path: `src-tauri/src/${file}`, ...shown });
    left -= shown.text.length;
  }
  return { ref, files: out };
}

/* ---------- the prompt ---------- */

const tail = (s, max) => (s.length > max ? "[earlier lines cut]\n" + s.slice(s.length - max) : s);

/** The user message: the diagnostics, then the source, then the request. */
export function buildPrompt(bundle, sources) {
  const section = (name) => {
    const v = bundle[name];
    if (v === undefined || v === "") return "";
    const text = typeof v === "string" ? v : JSON.stringify(v, null, 1);
    return `<${name}>\n${tail(text, PROMPT_CUT[name])}\n</${name}>\n`;
  };
  const diagnostics = Object.keys(PROMPT_CUT).map(section).join("");
  const source = sources.files.map((f) => `<source path="${f.path}" ref="${sources.ref}" excerpt="${f.excerpt}">\n${f.text}\n</source>`).join("\n");
  const versionNote = sources.ref === "main" ? `No tag v${bundle.glint_version || "?"} was found, so the source below is the main branch and may be newer than what the user runs.` : `The source below is tag ${sources.ref}, the version the user runs.`;
  return `<diagnostics glint_version="${bundle.glint_version || "unknown"}" collected_by="${bundle.tool || "glint-doctor"}">\n${diagnostics}</diagnostics>\n\n${versionNote}\n${source || "(No source file could be fetched; write patches by intent with source_seen false.)"}\n\nFind what is wrong with Glint on this PC and write the report: the summary for the user, the findings with evidence and confidence, and exact patch instructions for the developer.`;
}

/* ---------- Claude ---------- */

/** One Messages API call, streamed. -> { text, stop_reason, model, usage } */
export async function callClaude(env, system, prompt) {
  const fallbacks = (env.FALLBACKS || "on") !== "off";
  const body = {
    model: env.MODEL || "claude-opus-5-5",
    max_tokens: MAX_OUTPUT_TOKENS,
    stream: true,
    thinking: { type: "adaptive" },
    output_config: { effort: env.EFFORT || "high", format: { type: "json_schema", schema: REPORT_SCHEMA } },
    system,
    messages: [{ role: "user", content: prompt }],
  };
  const headers = { "content-type": "application/json", "x-api-key": env.ANTHROPIC_API_KEY, "anthropic-version": API_VERSION };
  if (fallbacks) {
    body.fallbacks = "default";
    headers["anthropic-beta"] = FALLBACK_BETA;
  }
  const res = await fetch(`${(env.ANTHROPIC_BASE_URL || "https://api.anthropic.com").replace(/\/$/, "")}/v1/messages`, { method: "POST", headers, body: JSON.stringify(body) });
  if (!res.ok) {
    const detail = (await res.text()).slice(0, 500);
    const err = `Claude API answered HTTP ${res.status}: ${detail}`;
    // 408, 409, 429 and 5xx (529 = overloaded) are worth another try; the rest are not.
    throw [408, 409, 429].includes(res.status) || res.status >= 500 ? new Error(err) : new Permanent(err);
  }
  const result = { text: "", stop_reason: null, stop_details: null, model: body.model, usage: { input_tokens: 0, output_tokens: 0 } };
  const onEvent = (data) => {
    let e;
    try { e = JSON.parse(data); } catch { return; }
    if (e.type === "message_start") {
      result.model = e.message?.model || result.model;
      result.usage.input_tokens = e.message?.usage?.input_tokens || 0;
    } else if (e.type === "content_block_delta" && e.delta?.type === "text_delta") {
      // After a mid-answer fallback the next model continues the same text, so it is one string either way.
      result.text += e.delta.text;
    } else if (e.type === "message_delta") {
      result.stop_reason = e.delta?.stop_reason ?? result.stop_reason;
      result.stop_details = e.delta?.stop_details ?? result.stop_details;
      result.usage.output_tokens = e.usage?.output_tokens ?? result.usage.output_tokens;
    } else if (e.type === "error") {
      throw new Error(`Claude API stream error: ${e.error?.type || ""} ${e.error?.message || ""}`.trim());
    }
  };
  const reader = res.body.pipeThrough(new TextDecoderStream()).getReader();
  let buffer = "";
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    buffer += value;
    let end;
    while ((end = buffer.search(/\r?\n\r?\n/)) >= 0) {
      const event = buffer.slice(0, end);
      buffer = buffer.slice(end).replace(/^\r?\n\r?\n/, "");
      for (const line of event.split(/\r?\n/)) if (line.startsWith("data:")) onEvent(line.slice(5).trim());
    }
  }
  return result;
}

/** The whole analysis of one upload. -> { report, meta } */
export async function analyze(env, bundle) {
  if (!env.ANTHROPIC_API_KEY) throw new Permanent("ANTHROPIC_API_KEY is not set on the Worker");
  const picked = pickFiles(bundle);
  const sources = await fetchSources(env, bundle, picked);
  const answer = await callClaude(env, SYSTEM, buildPrompt(bundle, sources));
  const meta = { model: answer.model, input_tokens: answer.usage.input_tokens, output_tokens: answer.usage.output_tokens, source_ref: sources.ref, source_files: sources.files.map((f) => f.path + (f.excerpt ? " (excerpt)" : "")) };
  if (answer.stop_reason === "refusal") throw new Permanent(`Claude declined the request (${answer.stop_details?.category || "no category"}). The diagnostics are stored; read them with ?include=bundle.`);
  if (answer.stop_reason === "max_tokens") throw new Error("Claude's answer was cut off at the output limit");
  let report;
  try { report = JSON.parse(answer.text); } catch { throw new Error(`Claude's answer was not the report JSON (stop reason ${answer.stop_reason})`); }
  return { report, meta };
}
