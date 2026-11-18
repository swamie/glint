// Glint share links: a Cloudflare Worker that stores clips in R2 and serves a page
// whose tags make Discord show the clip as an inline video embed.
//
// Public routes
//   GET  /c/<id>        the share page (embed tags for Discord, a player for people)
//   GET  /v/<id>.mp4    the video (Range requests, for Discord's player and browsers)
//   GET  /t/<id>.jpg    the poster picture
// Upload API (Authorization: Bearer <UPLOAD_TOKEN>)
//   GET    /api/usage                              { used, limit, clips }
//   POST   /api/uploads                            { title, uploader, game, width, height, duration, size } -> { id, uploadId }
//   PUT    /api/uploads/<id>/parts/<n>?uploadId=   one piece (all but the last the same size, >= 5 MiB) -> { etag }
//   PUT    /api/uploads/<id>/poster?uploadId=      the JPEG poster
//   POST   /api/uploads/<id>/complete?uploadId=    { parts: [{ partNumber, etag }] } -> { id, url }
//   DELETE /api/uploads/<id>?uploadId=             abandon an upload
//   DELETE /api/clips/<id>                         delete a shared clip
//
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
    const id = newId();
    const meta = {
      title: str(body.title, 200) || "Clip",
      uploader: str(body.uploader, 64),
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

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const parts = url.pathname.split("/").filter(Boolean);
    try {
      if (parts[0] === "api") return await api(request, env, url, parts);
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

export { newId, sharePage };
