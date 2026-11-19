// Glint share links: a Cloudflare Worker that stores clips in R2 and serves a page
// whose tags make Discord show the clip as an inline video embed.
//
// Public routes
//   GET  /c/<id>        the share page (embed tags for Discord, a player for people)
//   GET  /v/<id>.mp4    the video (Range requests, for Discord's player and browsers)
//   GET  /t/<id>.jpg    the poster picture
// Bug reports and feedback from the app (no key; see "Bug reports" below)
//   POST /report?kind=bug|feedback   multipart: payload_json (Discord-style), files[0] (bug: glint-log.txt)
//                                    -> 204, or { error } with 400 / 413 / 429 / 502 / 503
// Upload API (Authorization: Bearer <UPLOAD_TOKEN>)
//   GET    /api/usage                              { used, limit, clips }
//   POST   /api/names                              { name, secret, release? } -> { name }: claim a publisher name
//                                                  (2 to 16 characters, one holder each, compared without case)
//   POST   /api/uploads                            { title, uploader, game, width, height, duration, size } -> { id, uploadId }
//   PUT    /api/uploads/<id>/parts/<n>?uploadId=   one piece (all but the last the same size, >= 5 MiB) -> { etag }
//   PUT    /api/uploads/<id>/poster?uploadId=      the JPEG poster
//   POST   /api/uploads/<id>/complete?uploadId=    { parts: [{ partNumber, etag }] } -> { id, url }
//   DELETE /api/uploads/<id>?uploadId=             abandon an upload
//   DELETE /api/clips/<id>                         delete a shared clip
//
// Publisher names: n/<name, lower case>.json = { name, hash } where hash is the SHA-256 of the
// secret the claiming install made. An upload that carries a name must carry that secret.
// Storage: c/<id>/video.mp4, c/<id>/thumb.jpg, c/<id>/meta.json. A clip whose meta.json
// still says pending (an upload in progress) is not shown.

const ID_LEN = 12;
const ID_CHARS = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const ID_RE = /^[0-9A-Za-z]{12}$/;
const MIN_PART = 5 * 1024 * 1024;

const key = (id, name) => `c/${id}/${name}`;
const NOINDEX = { "X-Robots-Tag": "noindex" };

function json(body, status = 200) {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json", "Cache-Control": "no-store", ...NOINDEX } });
}
const fail = (error, status) => json({ error }, status);

function newId() {
  // Rejection sampling: 62 * 4 = 248, so bytes 248..255 are skipped and every character is equally likely.
  let out = "";
  while (out.length < ID_LEN) {
    for (const b of crypto.getRandomValues(new Uint8Array(32))) {
      if (b < 248 && out.length < ID_LEN) out += ID_CHARS[b % 62];
    }
  }
  return out;
}

async function authorized(request, env) {
  const want = env.UPLOAD_TOKEN || "";
  if (!want) return false;
  const got = (request.headers.get("Authorization") || "").replace(/^Bearer\s+/i, "");
  const enc = new TextEncoder();
  const [a, b] = await Promise.all([crypto.subtle.digest("SHA-256", enc.encode(got)), crypto.subtle.digest("SHA-256", enc.encode(want))]);
  return crypto.subtle.timingSafeEqual(a, b);
}

const limitBytes = (env) => Math.max(1, Number(env.MAX_MB) || 9000) * 1e6;

/** Bytes the bucket holds and how many clips. */
async function usage(env) {
  let used = 0, clips = 0, cursor;
  do {
    const page = await env.BUCKET.list({ prefix: "c/", cursor });
    for (const o of page.objects) {
      used += o.size;
      if (o.key.endsWith("/video.mp4")) clips++;
    }
    cursor = page.truncated ? page.cursor : undefined;
  } while (cursor);
  return { used, clips };
}

async function readMeta(env, id) {
  if (!ID_RE.test(id)) return null;
  const o = await env.BUCKET.get(key(id, "meta.json"));
  if (!o) return null;
  try { return await o.json(); } catch { return null; }
}

const writeMeta = (env, id, meta) => env.BUCKET.put(key(id, "meta.json"), JSON.stringify(meta), { httpMetadata: { contentType: "application/json" } });

const str = (v, max) => String(v ?? "").replace(/[\u0000-\u001f]/g, " ").trim().slice(0, max);
const int = (v, lo, hi, dflt) => { const n = Math.round(Number(v)); return Number.isFinite(n) && n >= lo && n <= hi ? n : dflt; };

async function removeClip(env, id) {
  await env.BUCKET.delete([key(id, "video.mp4"), key(id, "thumb.jpg"), key(id, "meta.json")]);
}

/* ---------- publisher names ---------- */

const NAME_RE = /^[\p{L}\p{N} _.\-]{2,16}$/u;
const NAME_RULE = "A publisher name is 2 to 16 characters: letters, numbers, spaces, dots, dashes or underscores.";
/** The name as shown (trimmed, single spaces), or "" when it breaks the rule. */
function cleanName(v) {
  const name = String(v ?? "").normalize("NFKC").replace(/\s+/g, " ").trim();
  return [...name].length >= 2 && [...name].length <= 16 && NAME_RE.test(name) ? name : "";
}
const nameKey = (name) => `n/${encodeURIComponent(name.toLowerCase())}.json`;
async function sha256Hex(text) {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, "0")).join("");
}
/** True when `name` is claimed and `secret` is its holder's. */
async function holdsName(env, name, secret) {
  if (!name || !secret) return false;
  const o = await env.BUCKET.get(nameKey(name));
  if (!o) return false;
  let rec;
  try { rec = await o.json(); } catch { return false; }
  return rec.hash === (await sha256Hex(secret));
}

async function claimName(request, env) {
  let body;
  try { body = await request.json(); } catch { return fail("The request was not JSON", 400); }
  const name = cleanName(body.name);
  const secret = String(body.secret || "");
  if (!name) return fail(NAME_RULE, 400);
  if (secret.length < 32 || secret.length > 128) return fail("The name's secret is missing. Update Glint.", 400);
  const hash = await sha256Hex(secret);
  const k = nameKey(name);
  const cur = await env.BUCKET.get(k);
  if (cur) {
    let rec = {};
    try { rec = await cur.json(); } catch { /* unreadable: treated as taken */ }
    if (rec.hash !== hash) return fail(`The name ${name} is taken. Pick another one.`, 409);
  } else {
    // onlyIf etagDoesNotMatch "*": written only if nobody claimed it in between.
    const put = await env.BUCKET.put(k, JSON.stringify({ name, hash, created: Date.now() }), { httpMetadata: { contentType: "application/json" }, onlyIf: { etagDoesNotMatch: "*" } });
    if (!put) return fail(`The name ${name} is taken. Pick another one.`, 409);
  }
  // Give back the name this install held before, so someone else can take it.
  const old = cleanName(body.release);
  if (old && old.toLowerCase() !== name.toLowerCase() && (await holdsName(env, old, secret))) await env.BUCKET.delete(nameKey(old));
  return json({ name });
}

/* ---------- upload API ---------- */

async function api(request, env, url, parts) {
  if (!(await authorized(request, env))) return fail("The upload key was not accepted. Check it in Glint's Settings → Share links.", 401);
  const m = request.method;
  const uploadId = url.searchParams.get("uploadId") || "";

  if (parts[1] === "usage" && m === "GET") {
    const u = await usage(env);
    return json({ ...u, limit: limitBytes(env) });
  }

  if (parts[1] === "clips" && parts.length === 3 && m === "DELETE") {
    const id = parts[2];
    if (!ID_RE.test(id)) return fail("No such clip", 404);
    const meta = await readMeta(env, id);
    await removeClip(env, id);
    return meta ? json({ ok: true }) : fail("No such clip", 404);
  }

  if (parts[1] === "names" && parts.length === 2 && m === "POST") return claimName(request, env);

  if (parts[1] !== "uploads") return fail("Not found", 404);

  // POST /api/uploads: start one.
  if (parts.length === 2 && m === "POST") {
    let body;
    try { body = await request.json(); } catch { return fail("The request was not JSON", 400); }
    const size = int(body.size, 1, 50e9, 0);
    if (!size) return fail("The upload's size is missing", 400);
    const { used } = await usage(env);
    const limit = limitBytes(env);
    if (used + size > limit) {
      const mb = (n) => Math.round(n / 1e6);
      return fail(`Not enough room on the share server: ${mb(used)} of ${mb(limit)} MB used, this clip needs ${mb(size)} MB. Delete old share links or raise MAX_MB.`, 413);
    }
    // A name on the embed must be the one this install claimed (POST /api/names).
    const uploader = str(body.uploader, 64);
    if (uploader && !(await holdsName(env, cleanName(uploader), String(body.nameSecret || "")))) {
      return fail("Choose a publisher name first: this one is not yours on this server. In Glint, Settings → Share links → Publisher name.", 403);
    }
    const id = newId();
    const meta = {
      title: str(body.title, 200) || "Clip",
      uploader: cleanName(uploader),
      game: str(body.game, 80),
      width: int(body.width, 16, 8192, 1920),
      height: int(body.height, 16, 8192, 1080),
      duration: Math.max(0, Number(body.duration) || 0),
      size,
      created: Date.now(),
      pending: true,
    };
    const mp = await env.BUCKET.createMultipartUpload(key(id, "video.mp4"), { httpMetadata: { contentType: "video/mp4", cacheControl: "public, max-age=31536000, immutable" } });
    meta.uploadId = mp.uploadId;
    await writeMeta(env, id, meta);
    return json({ id, uploadId: mp.uploadId });
  }

  const id = parts[2];
  const meta = await readMeta(env, id);
  if (!meta || !meta.pending || meta.uploadId !== uploadId) return fail("No such upload in progress", 404);
  const mp = env.BUCKET.resumeMultipartUpload(key(id, "video.mp4"), uploadId);

  // PUT /api/uploads/<id>/parts/<n>
  if (parts[3] === "parts" && parts.length === 5 && m === "PUT") {
    const n = int(parts[4], 1, 10000, 0);
    if (!n) return fail("Bad piece number", 400);
    const len = Number(request.headers.get("Content-Length") || 0);
    if (!len || !request.body) return fail("The piece is empty or has no length", 411);
    if (len > 100e6) return fail("A piece is too large", 413);
    const part = await mp.uploadPart(n, request.body);
    return json({ etag: part.etag, partNumber: part.partNumber });
  }

  // PUT /api/uploads/<id>/poster
  if (parts[3] === "poster" && parts.length === 4 && m === "PUT") {
    const len = Number(request.headers.get("Content-Length") || 0);
    if (!len || len > 5e6) return fail("The poster must be a JPEG under 5 MB", 413);
    await env.BUCKET.put(key(id, "thumb.jpg"), request.body, { httpMetadata: { contentType: "image/jpeg", cacheControl: "public, max-age=31536000, immutable" } });
    return json({ ok: true });
  }

  // POST /api/uploads/<id>/complete
  if (parts[3] === "complete" && parts.length === 4 && m === "POST") {
    let body;
    try { body = await request.json(); } catch { return fail("The request was not JSON", 400); }
    const list = (Array.isArray(body.parts) ? body.parts : []).map((p) => ({ partNumber: int(p.partNumber, 1, 10000, 0), etag: String(p.etag || "") })).filter((p) => p.partNumber && p.etag);
    if (!list.length) return fail("No pieces were listed", 400);
    let obj;
    try { obj = await mp.complete(list); } catch (e) { return fail(`The upload could not be finished: ${e.message || e}`, 400); }
    const { used } = await usage(env);
    if (used > limitBytes(env)) {
      await removeClip(env, id);
      return fail("Not enough room on the share server for this clip", 413);
    }
    delete meta.pending;
    delete meta.uploadId;
    meta.size = obj.size;
    await writeMeta(env, id, meta);
    return json({ id, url: `${publicBase(env, url)}/c/${id}` });
  }

  // DELETE /api/uploads/<id>: give up.
  if (parts.length === 3 && m === "DELETE") {
    try { await mp.abort(); } catch { /* already gone */ }
    await removeClip(env, id);
    return json({ ok: true });
  }
  return fail("Not found", 404);
}

const publicBase = (env, url) => (env.PUBLIC_BASE || url.origin).replace(/\/+$/, "");

/* ---------- serving ---------- */

/** An R2 object as a response, honouring Range and If-None-Match (GET and HEAD). */
async function serveObject(request, env, k) {
  const head = request.method === "HEAD";
  const obj = head ? await env.BUCKET.head(k) : await env.BUCKET.get(k, { range: request.headers, onlyIf: request.headers });
  if (!obj) return new Response("Not found", { status: 404, headers: NOINDEX });
  const h = new Headers(NOINDEX);
  obj.writeHttpMetadata(h);
  h.set("ETag", obj.httpEtag);
  h.set("Accept-Ranges", "bytes");
  h.set("Access-Control-Allow-Origin", "*");
  if (head) {
    h.set("Content-Length", String(obj.size));
    return new Response(null, { status: 200, headers: h });
  }
  if (!("body" in obj) || !obj.body) {
    // A precondition matched (If-None-Match): nothing changed.
    return new Response(null, { status: 304, headers: h });
  }
  const r = obj.range;
  if (r && request.headers.has("Range")) {
    let start, len;
    if (typeof r.suffix === "number") { len = Math.min(r.suffix, obj.size); start = obj.size - len; }
    else { start = Number(r.offset) || 0; len = typeof r.length === "number" ? r.length : obj.size - start; }
    if (start >= obj.size) {
      h.set("Content-Range", `bytes */${obj.size}`);
      return new Response(null, { status: 416, headers: h });
    }
    h.set("Content-Range", `bytes ${start}-${start + len - 1}/${obj.size}`);
    h.set("Content-Length", String(len));
    return new Response(obj.body, { status: 206, headers: h });
  }
  h.set("Content-Length", String(obj.size));
  return new Response(obj.body, { status: 200, headers: h });
}

const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

function fmtDur(s) {
  s = Math.round(s || 0);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

function sharePage(env, url, id, meta) {
  const base = publicBase(env, url);
  const page = `${base}/c/${id}`;
  const video = `${base}/v/${id}.mp4`;
  const thumb = `${base}/t/${id}.jpg`;
  const color = /^#[0-9a-fA-F]{6}$/.test(env.EMBED_COLOR || "") ? env.EMBED_COLOR : "#3B82F6";
  const line = [meta.uploader, meta.game].filter(Boolean).join(" · ");
  const w = meta.width || 1920, h = meta.height || 1080;
  const pw = Math.min(1280, w), ph = Math.round((h * pw) / w / 2) * 2;
  const lineTag = !line ? "" : env.EMBED_LINE === "site_name"
    ? `<meta property="og:site_name" content="${esc(line)}">`
    : `<meta property="og:description" content="${esc(line)}">\n<meta name="twitter:description" content="${esc(line)}">`;
  // The embed tags come first in <head>: Discord reads only the start of a page.
  return `<!doctype html>
<html lang="en"><head>
<meta charset="utf-8">
<meta name="robots" content="noindex">
<meta name="theme-color" content="${color}">
<meta property="og:type" content="video.other">
<meta property="og:url" content="${esc(page)}">
<meta property="og:title" content="${esc(meta.title)}">
${lineTag}
<meta property="og:image" content="${esc(thumb)}">
<meta property="og:image:type" content="image/jpeg">
<meta property="og:image:width" content="${pw}">
<meta property="og:image:height" content="${ph}">
<meta property="og:video" content="${esc(video)}">
<meta property="og:video:url" content="${esc(video)}">
<meta property="og:video:secure_url" content="${esc(video)}">
<meta property="og:video:type" content="video/mp4">
<meta property="og:video:width" content="${w}">
<meta property="og:video:height" content="${h}">
<meta name="twitter:card" content="player">
<meta name="twitter:title" content="${esc(meta.title)}">
<meta name="twitter:image" content="${esc(thumb)}">
<meta name="twitter:player:width" content="${w}">
<meta name="twitter:player:height" content="${h}">
<meta name="twitter:player:stream" content="${esc(video)}">
<meta name="twitter:player:stream:content_type" content="video/mp4">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${esc(meta.title)}</title>
<style>
  :root { --accent: ${color}; --bg: #0e1015; --panel: #171a21; --text: #e8eaf0; --dim: #9aa1b2; }
  * { box-sizing: border-box; }
  body { margin: 0; min-height: 100vh; background: var(--bg); color: var(--text); font: 15px/1.45 system-ui, -apple-system, "Segoe UI", sans-serif; display: flex; align-items: center; justify-content: center; padding: 24px 16px; }
  main { width: 100%; max-width: 1100px; background: var(--panel); border-left: 4px solid var(--accent); border-radius: 10px; padding: 18px 18px 20px; }
  h1 { font-size: 20px; margin: 0 0 4px; overflow-wrap: anywhere; }
  .line { color: var(--dim); margin: 0 0 14px; }
  video { display: block; width: 100%; max-height: 75vh; background: #000; border-radius: 8px; }
  .row { display: flex; flex-wrap: wrap; gap: 10px; margin-top: 14px; align-items: center; }
  a.btn { color: var(--text); text-decoration: none; background: #232733; padding: 8px 14px; border-radius: 8px; font-weight: 600; }
  a.btn:hover { background: var(--accent); }
  .meta { color: var(--dim); font-size: 13px; margin-left: auto; }
</style>
</head><body>
<main>
  <h1>${esc(meta.title)}</h1>
  ${line ? `<p class="line">${esc(line)}</p>` : ""}
  <video controls playsinline preload="metadata" poster="${esc(thumb)}" width="${w}" height="${h}"><source src="${esc(video)}" type="video/mp4"></video>
  <div class="row">
    <a class="btn" href="${esc(video)}" download="${esc(meta.title.replace(/[\\/:*?"<>|]/g, " ").trim() || "clip")}.mp4">Download</a>
    <span class="meta">${esc(fmtDur(meta.duration))} · shared with Glint</span>
  </div>
</main>
</body></html>`;
}

// ---- Bug reports and feedback.
// Glint posts here and the Worker passes the report on to a Discord webhook. The webhook lives only
// in the Worker's secrets (BUG_WEBHOOK_URL, optional FEEDBACK_WEBHOOK_URL), so it is not in the app
// and cannot be pulled out of it. BUG_PING_IDS (optional, comma-separated Discord user ids) are
// mentioned on bug reports. The Worker rebuilds the Discord message from the parts it allows, so a
// caller cannot ping anyone, add links or images, or post anything but one plain embed.
// Limits: per IP one report a REPORT_GAP_S (30 s) and REPORT_PER_IP_DAY (10) a day; REPORT_PER_DAY (300)
// a day in all. Counts are kept in R2 under r/<day>/, a few bytes each.

const REPORT_MAX_BODY = 300 * 1024;
const REPORT_MAX_LOG = 256 * 1024;
const WEBHOOK_RE = /^https:\/\/(?:canary\.|ptb\.)?discord(?:app)?\.com\/api\/webhooks\/\d+\/[\w-]+$/;

const rstr = (v, max) => {
  const s = typeof v === "string" ? v : "";
  return s.length > max ? s.slice(0, max - 1) + "…" : s;
};

/** One clean Discord payload from what the app sent. Only text, a color and plain fields survive. */
function cleanPayload(raw, kind, pings) {
  const e = (raw && Array.isArray(raw.embeds) && raw.embeds[0]) || {};
  const embed = {};
  const title = rstr(e.title, 256).trim();
  if (title) embed.title = title;
  const desc = rstr(e.description, 4096).trim();
  if (desc) embed.description = desc;
  if (Number.isInteger(e.color) && e.color >= 0 && e.color <= 0xffffff) embed.color = e.color;
  const fields = (Array.isArray(e.fields) ? e.fields : []).slice(0, 6)
    .map((f) => ({ name: rstr(f && f.name, 256).trim(), value: rstr(f && f.value, 1024).trim(), inline: !!(f && f.inline) }))
    .filter((f) => f.name && f.value);
  if (fields.length) embed.fields = fields;
  if (!embed.description && !fields.length) return null;
  // The first line ("Bug report · Glint 0.1.52"), without anything that could look like a mention.
  const head = rstr(raw && raw.content, 200).split("\n").pop().replace(/[@<>]/g, "").trim() || (kind === "bug" ? "Bug report" : "Feedback");
  const content = kind === "bug" && pings.length ? `${pings.map((id) => `<@${id}>`).join(" ")}\n${head}` : head;
  return { content, embeds: [embed], allowed_mentions: kind === "bug" ? { parse: [], users: pings } : { parse: [] } };
}

async function sha(text) {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(d)].slice(0, 12).map((b) => b.toString(16).padStart(2, "0")).join("");
}

async function readCount(env, k) {
  const o = await env.BUCKET.get(k);
  if (!o) return { n: 0, last: 0 };
  try { return await o.json(); } catch { return { n: 0, last: 0 }; }
}

/** null when the report may go, else the refusal. Not exact under bursts (R2 has no atomic add); close enough. */
async function reportLimit(env, ip) {
  const now = Date.now();
  const day = new Date(now).toISOString().slice(0, 10);
  const num = (v, d) => (v === undefined || v === "" || !Number.isFinite(Number(v)) ? d : Number(v));
  const gap = num(env.REPORT_GAP_S, 30) * 1000;
  const perIp = num(env.REPORT_PER_IP_DAY, 10);
  const perDay = num(env.REPORT_PER_DAY, 300);
  const ipKey = `r/${day}/${await sha(`${ip}|${day}`)}.json`, allKey = `r/${day}/_all.json`;
  const [mine, all] = await Promise.all([readCount(env, ipKey), readCount(env, allKey)]);
  if (now - mine.last < gap) return fail("Wait a moment before sending another report.", 429);
  if (mine.n >= perIp) return fail("That is a lot of reports from this connection today. Try again tomorrow.", 429);
  if (all.n >= perDay) return fail("Too many reports today. Try again tomorrow.", 429);
  const put = (k, v) => env.BUCKET.put(k, JSON.stringify(v), { httpMetadata: { contentType: "application/json" } });
  await Promise.all([put(ipKey, { n: mine.n + 1, last: now }), put(allKey, { n: all.n + 1, last: now })]);
  return null;
}

async function report(request, env, url) {
  if (request.method !== "POST") return fail("Use POST.", 405);
  const kind = url.searchParams.get("kind") === "feedback" ? "feedback" : "bug";
  const hook = String((kind === "feedback" && env.FEEDBACK_WEBHOOK_URL) || env.BUG_WEBHOOK_URL || "").trim();
  if (!WEBHOOK_RE.test(hook)) return fail("Bug reports are not set up on the server yet.", 503);
  const length = Number(request.headers.get("Content-Length") || 0);
  if (length > REPORT_MAX_BODY) return fail("The report is too big.", 413);
  if (!(request.headers.get("Content-Type") || "").toLowerCase().startsWith("multipart/form-data")) return fail("Send the report as multipart/form-data.", 400);
  const body = await request.arrayBuffer();
  if (body.byteLength > REPORT_MAX_BODY) return fail("The report is too big.", 413);
  let form;
  try { form = await new Response(body, { headers: { "Content-Type": request.headers.get("Content-Type") } }).formData(); } catch { return fail("The report could not be read.", 400); }
  let raw;
  try { raw = JSON.parse(await (async (p) => (typeof p === "string" ? p : p ? await p.text() : ""))(form.get("payload_json"))); } catch { return fail("The report could not be read.", 400); }
  const pings = String(env.BUG_PING_IDS || "").split(",").map((s) => s.trim()).filter((s) => /^\d{5,25}$/.test(s));
  const payload = cleanPayload(raw, kind, pings);
  if (!payload) return fail("The report is empty.", 400);
  let log = null;
  const f = kind === "bug" ? form.get("files[0]") : null;
  if (f) {
    const text = typeof f === "string" ? f : await f.text();
    if (text.length > REPORT_MAX_LOG) return fail("The log is too big.", 413);
    log = text;
  }

  const ip = request.headers.get("CF-Connecting-IP") || "unknown";
  const refused = await reportLimit(env, ip);
  if (refused) return refused;

  const out = new FormData();
  out.append("payload_json", JSON.stringify(payload));
  if (log !== null) out.append("files[0]", new Blob([log], { type: "text/plain" }), "glint-log.txt");
  let res;
  try { res = await fetch(hook, { method: "POST", body: out }); } catch { return fail("Could not reach Discord. Try again in a minute.", 502); }
  if (res.ok) return new Response(null, { status: 204, headers: NOINDEX });
  if (res.status === 429) return fail("Discord is busy right now. Try again in a minute.", 429);
  // Discord's own reason is passed on (it never holds the webhook address).
  let why = "";
  try { why = rstr((await res.json()).message, 120); } catch {}
  return fail(`Discord did not accept the ${kind === "bug" ? "report" : "feedback"} (HTTP ${res.status}${why ? `: ${why}` : ""}).`, 502);
}

export default {
  async fetch(request, env) {
    // The bucket's binding may be named BUCKET (wrangler.toml) or CLIPS (set up in the dashboard).
    if (!env.BUCKET && env.CLIPS) env = { ...env, BUCKET: env.CLIPS };
    if (!env.BUCKET) return fail("The Worker has no R2 bucket bound. Bind one named BUCKET or CLIPS.", 500);
    const url = new URL(request.url);
    const parts = url.pathname.split("/").filter(Boolean);
    try {
      if (parts[0] === "api") return await api(request, env, url, parts);
      if (parts.length === 1 && parts[0] === "report") return await report(request, env, url);
      if (request.method !== "GET" && request.method !== "HEAD") return new Response("Method not allowed", { status: 405, headers: NOINDEX });

      if (parts.length === 2 && (parts[0] === "v" || parts[0] === "t")) {
        const m = /^([0-9A-Za-z]{12})\.(mp4|jpg)$/.exec(parts[1]);
        if (!m || (parts[0] === "v") !== (m[2] === "mp4")) return new Response("Not found", { status: 404, headers: NOINDEX });
        const meta = await readMeta(env, m[1]);
        if (!meta || meta.pending) return new Response("Not found", { status: 404, headers: NOINDEX });
        return await serveObject(request, env, key(m[1], parts[0] === "v" ? "video.mp4" : "thumb.jpg"));
      }

      if (parts.length === 2 && parts[0] === "c") {
        const meta = await readMeta(env, parts[1]);
        if (!meta || meta.pending) return new Response("This clip was deleted or never existed.", { status: 404, headers: { "Content-Type": "text/plain; charset=utf-8", ...NOINDEX } });
        return new Response(request.method === "HEAD" ? null : sharePage(env, url, parts[1], meta), {
          headers: { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "public, max-age=300", ...NOINDEX },
        });
      }

      if (parts.length === 0) return new Response("Glint share links.", { headers: { "Content-Type": "text/plain; charset=utf-8", ...NOINDEX } });
      return new Response("Not found", { status: 404, headers: NOINDEX });
    } catch (e) {
      return fail(`The share server hit an error: ${e && e.message ? e.message : e}`, 500);
    }
  },
};

