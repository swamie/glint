// The server's own pass over an upload, before it is stored or sent anywhere.
// The tool has already taken personal data out (tools/glint-doctor/src/scrub.rs);
// this is the second lock on the same door, for an old or altered copy of the
// tool: only known sections are kept, each is cut to a size, and the patterns
// that must never get through are removed again. The server does not know the
// user's name or the PC's name; only the tool can take those out.

/** The sections the tool sends, and the most characters each may hold as JSON. */
const SECTIONS = {
  schema: 10, tool: 60, created_unix: 20, glint_version: 20, description: 2400,
  glint: 12000, system: 12000, settings: 60000, ffmpeg: 8000, processes: 4000,
  crashes: 16000, data: 16000, log_bytes_total: 20, log_faults: 60000, log_tail: 200000,
};

const RULES = [
  // Discord webhooks, with or without an API version in the path (/api/v10/webhooks/...).
  [/https?:\/\/[a-z0-9.-]*discord(?:app)?\.com\/api\/(?:v\d+\/)?webhooks\/[^\s"'<>]*/gi, "<webhook removed>"],
  // The query of any web address can carry a key: it never gets through.
  [/\b((?:https?|wss?):\/\/[^\s"'<>?#]+)[?#][^\s"'<>]*/gi, "$1"],
  // Someone's profile folder, on any drive.
  [/\b([a-z]:[\\/]+(?:Users|Documents and Settings)[\\/]+)(?!Public\b|Default\b|All Users\b|<user>)[^\\/:*?"<>|\r\n]+/gi, "$1<user>"],
  // A work or school OneDrive folder is named after the organization.
  [/\b(OneDrive - )[^\\/:*?"<>|\r\n]+/gi, "$1<org>"],
  // The host of a network share (\\NAS01\clips), not \\?\ or \\.\ device paths.
  [/(^|[\s"'=(,[])(\\{2,4})[A-Za-z0-9._$-]{2,}(?=\\)/g, "$1$2<host>"],
  // Windows account ids.
  [/\bS-1-5-21(?:-\d+){2,5}\b/g, "<sid>"],
  // E-mail addresses, also URL-encoded (name%40example.com).
  [/[A-Za-z0-9._%+-]+(?:@|%40)[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}/g, "<email>"],
  // "token=...", "access_token: ...", "Authorization: Bearer ..." and the like.
  [/\b([a-z0-9_-]*(?:bearer|token|secret|password|passwd|pwd|api[_-]?key|access[_-]?key|upload[_-]?key|authorization|cookie))\b(["']?\s*[:=]\s*["']?|\s+)(?!<)[A-Za-z0-9._~+/=%-]{8,}/gi, "$1$2<removed>"],
  [/\bsk-ant-[A-Za-z0-9_-]+/g, "<removed>"],
  // JSON web tokens and Discord bot tokens: three long parts with dots between.
  [/\b[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{16,}\b/g, "<token>"],
  [/\b[0-9a-fA-F]{32,}\b/g, "<hex>"],
  // Long random-looking strings (GitHub tokens, API keys of other services).
  [/\b[A-Za-z0-9_-]{40,}\b/g, "<token>"],
];

const IPV4 = /(?<![.\dvV])\b(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\b(?!\.\d)/g;
// Full and "::"-shortened IPv6 addresses. A time of day (12:34:56) has too few groups to match.
const IPV6 = /\b(?:[0-9a-f]{1,4}:){4,7}[0-9a-f]{1,4}\b|\b(?:[0-9a-f]{1,4}:){1,6}:[0-9a-f]{1,4}(?::[0-9a-f]{1,4})*\b/gi;

export function scrubText(text) {
  let t = String(text);
  for (const [re, to] of RULES) t = t.replace(re, to);
  t = t.replace(IPV4, (m, a, b, c, d) => ([a, b, c, d].some((n) => Number(n) > 255) || m === "127.0.0.1" || m === "0.0.0.0" ? m : "<ip>"));
  return t.replace(IPV6, "<ip>");
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
