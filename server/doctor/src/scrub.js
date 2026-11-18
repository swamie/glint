// The server's own pass over an upload, before it is stored or sent anywhere.
// The tool has already taken personal data out (tools/glint-doctor/src/scrub.rs);
// this is the second lock on the same door, for an old or altered copy of the
// tool: only known sections are kept, each is cut to a size, and the patterns
// that must never get through are removed again.

/** The sections the tool sends, and the most characters each may hold as JSON. */
const SECTIONS = {
  schema: 10, tool: 60, created_unix: 20, glint_version: 20, description: 2400,
  glint: 12000, system: 12000, settings: 60000, ffmpeg: 8000, processes: 4000,
  crashes: 16000, data: 16000, log_bytes_total: 20, log_faults: 60000, log_tail: 200000,
};

const RULES = [
  [/https?:\/\/[a-z0-9.-]*discord(?:app)?\.com\/api\/webhooks\/[^\s"'<>]*/gi, "<webhook removed>"],
  [/\b([a-z]:[\\/]+Users[\\/]+)(?!Public\b|Default\b|<user>)[^\\/:*?"<>|\r\n]+/gi, "$1<user>"],
  [/[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}/g, "<email>"],
  [/\b(bearer|token|secret|password|passwd|api[_-]?key|upload[_-]?key|authorization)\b(["']?\s*[:=]\s*["']?|\s+)(?!<removed>)[A-Za-z0-9._~+/=-]{8,}/gi, "$1$2<removed>"],
  [/\bsk-ant-[A-Za-z0-9_-]+/g, "<removed>"],
  [/\b[0-9a-fA-F]{32,}\b/g, "<hex>"],
];

const IPV4 = /(?<![.\dvV])\b(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\b(?!\.\d)/g;

export function scrubText(text) {
  let t = String(text);
  for (const [re, to] of RULES) t = t.replace(re, to);
  return t.replace(IPV4, (m, a, b, c, d) => ([a, b, c, d].some((n) => Number(n) > 255) || m === "127.0.0.1" || m === "0.0.0.0" ? m : "<ip>"));
}

function walk(v, depth) {
  if (depth > 12) return null;
  if (typeof v === "string") return scrubText(v);
  if (Array.isArray(v)) return v.slice(0, 500).map((x) => walk(x, depth + 1));
  if (v && typeof v === "object") return Object.fromEntries(Object.entries(v).slice(0, 500).map(([k, x]) => [String(k).slice(0, 80), walk(x, depth + 1)]));
  return typeof v === "number" || typeof v === "boolean" ? v : null;
}

/** Cut a section to `max` characters of JSON. Text keeps its END (the newest log lines). */
function cut(v, max) {
  if (typeof v === "string") return v.length > max ? "[cut]\n" + v.slice(v.length - max) : v;
  const text = JSON.stringify(v);
  return text.length > max ? { cut: text.slice(0, max) } : v;
}

/** The upload with only known sections, scrubbed and cut. Null if it is not a Glint Doctor report. */
export function cleanBundle(raw) {
  if (!raw || typeof raw !== "object" || Array.isArray(raw) || raw.schema !== 1) return null;
  const out = {};
  for (const [name, max] of Object.entries(SECTIONS)) {
    if (raw[name] === undefined || raw[name] === null) continue;
    out[name] = cut(walk(raw[name], 0), max);
  }
  out.glint_version = /^\d{1,3}\.\d{1,3}\.\d{1,4}$/.test(String(out.glint_version || "")) ? String(out.glint_version) : "";
  return out;
}
