// Glint Doctor: the developer's end of the one-time support tool (tools/glint-doctor).
// The Anthropic API key lives here, as a secret, and nowhere else. The tool
// only ever holds a one-time support code.
//
// For the tool (no key; the support code is the credential)
//   POST /v1/submit            Authorization: Bearer <code>, body = the diagnostics JSON
//                              -> 202 { ticket }   the code is now used up
//                              -> 401 unknown code, 403 used or withdrawn, 410 expired, 413 too large, 429 slow down
//   GET  /v1/status/<ticket>   -> { status: "working" | "done" | "failed", user_summary? }
// For the developer (Authorization: Bearer <ADMIN_TOKEN>)
//   POST   /admin/codes                 { label, hours?, count? } -> { codes: [{ code, expires }] }
//   GET    /admin/codes                 every code's label, state and expiry (never the code itself)
//   DELETE /admin/codes/<last 4>        withdraw an unused code
//   GET    /admin/reports               the list
//   GET    /admin/reports/<ticket>      the report as JSON; ?format=md as Markdown; ?include=bundle adds the diagnostics
//   POST   /admin/reports/<ticket>/retry   analyse the stored diagnostics again
//   DELETE /admin/reports/<ticket>
//
// Storage and the analysis itself: src/store.js. The Claude call: src/analyze.js.
import { cleanBundle } from "./scrub.js";
import { markdown } from "./deliver.js";

export { DoctorStore } from "./store.js";

/** A support code: twelve of these, shown as ABCD-EFGH-JKMN. No 0 O 1 I L. The tool knows the same set. */
const CODE_CHARS = "23456789ABCDEFGHJKMNPQRSTUVWXYZ";
const CODE_RE = /^[2-9A-HJKMNP-Z]{12}$/;
const TICKET_RE = /^[0-9a-z]{24}$/;

const NOINDEX = { "X-Robots-Tag": "noindex" };

function json(body, status = 200) {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json", "Cache-Control": "no-store", ...NOINDEX } });
}
const fail = (error, status) => json({ error }, status);

const bearer = (request) => (request.headers.get("Authorization") || "").replace(/^Bearer\s+/i, "").trim();

async function sha256(text) {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** What is stored for a code: never the code. */
const codeHash = (code) => sha256(`glint-doctor:code:${code}`);

function newCode() {
  // Rejection sampling: 31 * 8 = 248, so bytes 248..255 are skipped and every character is equally likely.
  let out = "";
  while (out.length < 12) for (const b of crypto.getRandomValues(new Uint8Array(32))) if (b < 248 && out.length < 12) out += CODE_CHARS[b % 31];
  return out;
}
const pretty = (code) => code.match(/.{4}/g).join("-");

async function isAdmin(request, env) {
  const want = env.ADMIN_TOKEN || "";
  if (!want) return false;
  const enc = new TextEncoder();
  const [a, b] = await Promise.all([crypto.subtle.digest("SHA-256", enc.encode(bearer(request))), crypto.subtle.digest("SHA-256", enc.encode(want))]);
  return crypto.subtle.timingSafeEqual(a, b);
}

/** The body as text, or null once it passes `max` bytes (the rest is not read). */
async function readCapped(request, max) {
  if (Number(request.headers.get("Content-Length") || 0) > max) return null;
  if (!request.body) return "";
  const reader = request.body.getReader();
  const chunks = [];
  let size = 0;
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > max) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  const all = new Uint8Array(size);
  let at = 0;
  for (const c of chunks) { all.set(c, at); at += c.byteLength; }
  return new TextDecoder().decode(all);
}

const store = (env) => env.STORE.get(env.STORE.idFromName("main"));

/* ---------- the tool's two requests ---------- */

async function submit(request, env) {
  // 1. A code of the right shape, before anything is read or stored. A wrong shape never reaches storage.
  const code = bearer(request).toUpperCase().replace(/[^0-9A-Z]/g, "");
  if (!CODE_RE.test(code)) return fail("A support code is needed. It looks like ABCD-EFGH-JKMN.", 401);
  // 2. A body that is small enough and is a Glint Doctor report.
  const max = Math.max(1, Number(env.MAX_UPLOAD_KB) || 400) * 1024;
  const text = await readCapped(request, max);
  if (text === null) return fail("The report is too large.", 413);
  let raw;
  try { raw = JSON.parse(text); } catch { return fail("The report was not JSON.", 400); }
  const bundle = cleanBundle(raw);
  if (!bundle) return fail("That is not a Glint Doctor report.", 400);
  // 3. Use the code up and store the report, in one step. Only after this can anything reach Claude.
  const ip = await sha256(`glint-doctor:ip:${request.headers.get("CF-Connecting-IP") || "unknown"}`);
  const result = await store(env).redeem({ hash: await codeHash(code), ip, bundle: JSON.stringify(bundle), glintVersion: bundle.glint_version });
  if (!result.ok) return fail(result.error, result.status);
  return json({ ticket: result.ticket, status: "working" }, 202);
}

async function status(env, ticket) {
  if (!TICKET_RE.test(ticket)) return fail("Not found", 404);
  const s = await store(env).status(ticket);
  return s ? json(s) : fail("Not found", 404);
}

/* ---------- the developer's requests ---------- */

async function admin(request, env, url, parts) {
  if (!(await isAdmin(request, env))) return fail("The admin key was not accepted.", 401);
  const m = request.method;
  const db = store(env);

  if (parts[1] === "codes") {
    if (parts.length === 2 && m === "POST") {
      let body = {};
      try { body = await request.json(); } catch {}
      const label = String(body.label ?? "").replace(/[\u0000-\u001f]/g, " ").trim().slice(0, 100);
      if (!label) return fail("Give the code a label: who it is for.", 400);
      const count = Math.min(Math.max(Math.round(Number(body.count)) || 1, 1), 20);
      const codes = [];
      for (let i = 0; i < count; i++) {
        const code = newCode();
        const made = await db.mint({ hash: await codeHash(code), hint: code.slice(-4), label, hours: Number(body.hours) || 0 });
        codes.push({ code: pretty(code), label, expires: new Date(made.expires).toISOString() });
      }
      return json({ codes });
    }
    if (parts.length === 2 && m === "GET") return json({ codes: await db.listCodes() });
    if (parts.length === 3 && m === "DELETE") {
      const n = await db.revoke(parts[2]);
      return n ? json({ ok: true, revoked: n }) : fail("No unused code ends in that.", 404);
    }
    return fail("Not found", 404);
  }

  if (parts[1] === "reports") {
    if (parts.length === 2 && m === "GET") return json({ reports: await db.listReports() });
    const ticket = parts[2] || "";
    if (!TICKET_RE.test(ticket)) return fail("No such report", 404);
    if (parts.length === 3 && m === "GET") {
      const row = await db.getReport(ticket, url.searchParams.get("include") === "bundle");
      if (!row) return fail("No such report", 404);
      if (url.searchParams.get("format") === "md") return new Response(markdown(row), { headers: { "Content-Type": "text/markdown; charset=utf-8", "Cache-Control": "no-store", ...NOINDEX } });
      return json(row);
    }
    if (parts.length === 3 && m === "DELETE") return (await db.deleteReport(ticket)) ? json({ ok: true }) : fail("No such report", 404);
    if (parts.length === 4 && parts[3] === "retry" && m === "POST") return (await db.retry(ticket)) ? json({ ok: true, status: "working" }, 202) : fail("No such report", 404);
  }
  return fail("Not found", 404);
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const parts = url.pathname.split("/").filter(Boolean);
    try {
      if (parts[0] === "v1" && parts[1] === "submit" && parts.length === 2) return request.method === "POST" ? await submit(request, env) : fail("Method not allowed", 405);
      if (parts[0] === "v1" && parts[1] === "status" && parts.length === 3 && request.method === "GET") return await status(env, parts[2]);
      if (parts[0] === "admin") return await admin(request, env, url, parts);
      return fail("Not found", 404);
    } catch (e) {
      console.error(e);
      return fail("Something went wrong on the server.", 500);
    }
  },
};
