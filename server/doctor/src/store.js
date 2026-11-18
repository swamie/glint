// All of Glint Doctor's state: one SQLite-backed Durable Object.
//
// Why a Durable Object: "a support code works once" needs a test-and-set that
// two requests cannot both win. A Durable Object runs one piece of JavaScript
// at a time and its SQLite writes inside one turn commit together, so `redeem`
// below (mark the code used, store the upload) is atomic. Workers KV cannot do
// that: two readers can both see "unused". The same object also has an alarm,
// which is how the analysis runs after the user has already been answered.
//
// Tables
//   codes    one row per support code. Only the SHA-256 of a code is kept.
//   reports  one row per accepted upload: the diagnostics, then Claude's report.
//   tries    wrong codes by client address, to slow guessing.
import { DurableObject } from "cloudflare:workers";
import { analyze, Permanent } from "./analyze.js";
import { postToDiscord } from "./deliver.js";

const HOUR = 3600 * 1000;
/** Wrong codes one address may send in an hour. */
const MAX_TRIES = 10;
/** Times an analysis is tried before it is marked failed. */
const MAX_ATTEMPTS = 3;

const TICKET_CHARS = "0123456789abcdefghijklmnopqrstuvwxyz";
function newTicket() {
  // 24 characters of 36: about 124 bits. A ticket is the only thing that reads a status.
  let out = "";
  while (out.length < 24) for (const b of crypto.getRandomValues(new Uint8Array(32))) if (b < 252 && out.length < 24) out += TICKET_CHARS[b % 36];
  return out;
}

const num = (v, dflt) => { const n = Number(v); return Number.isFinite(n) && n > 0 ? n : dflt; };

export class DoctorStore extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.sql = ctx.storage.sql;
    this.sql.exec(`
      CREATE TABLE IF NOT EXISTS codes (
        hash TEXT PRIMARY KEY,
        hint TEXT NOT NULL,
        label TEXT NOT NULL,
        created INTEGER NOT NULL,
        expires INTEGER NOT NULL,
        used INTEGER,
        ticket TEXT,
        revoked INTEGER NOT NULL DEFAULT 0
      );
      CREATE TABLE IF NOT EXISTS reports (
        ticket TEXT PRIMARY KEY,
        label TEXT NOT NULL,
        created INTEGER NOT NULL,
        status TEXT NOT NULL,
        attempts INTEGER NOT NULL DEFAULT 0,
        next_try INTEGER NOT NULL DEFAULT 0,
        finished INTEGER,
        glint_version TEXT NOT NULL DEFAULT '',
        bundle TEXT NOT NULL,
        report TEXT,
        meta TEXT,
        error TEXT,
        model TEXT,
        input_tokens INTEGER,
        output_tokens INTEGER,
        delivered INTEGER NOT NULL DEFAULT 0
      );
      CREATE TABLE IF NOT EXISTS tries (ip TEXT NOT NULL, at INTEGER NOT NULL);
    `);
  }

  one(query, ...args) { return this.sql.exec(query, ...args).toArray()[0] || null; }

  /* ---------- codes ---------- */

  /** A new code. The Worker made it and hashed it; only the hash arrives here. */
  mint({ hash, hint, label, hours }) {
    const now = Date.now();
    const expires = now + Math.min(num(hours, num(this.env.CODE_HOURS, 72)), 24 * 30) * HOUR;
    this.sql.exec("INSERT INTO codes (hash, hint, label, created, expires) VALUES (?, ?, ?, ?, ?)", hash, hint, String(label || "").slice(0, 100), now, expires);
    return { hint, label, expires };
  }

  listCodes() {
    const now = Date.now();
    return this.sql.exec("SELECT hint, label, created, expires, used, ticket, revoked FROM codes ORDER BY created DESC LIMIT 200").toArray()
      .map((c) => ({ ...c, state: c.used ? "used" : c.revoked ? "revoked" : c.expires <= now ? "expired" : "open" }));
  }

  /** Revoke the open codes whose hint (their last four characters) is `hint`. */
  revoke(hint) {
    return this.sql.exec("UPDATE codes SET revoked = 1 WHERE hint = ? AND used IS NULL AND revoked = 0 RETURNING hint", String(hint).toUpperCase()).toArray().length;
  }

  /**
   * Use a code up and take the upload, or refuse. There is no `await` in here: the checks,
   * the UPDATE that marks the code used and the INSERT of the upload are one atomic step.
   */
  redeem({ hash, ip, bundle, glintVersion }) {
    const now = Date.now();
    this.sql.exec("DELETE FROM tries WHERE at < ?", now - HOUR);
    if (this.one("SELECT COUNT(*) AS n FROM tries WHERE ip = ?", ip).n >= MAX_TRIES) return { ok: false, status: 429, error: "Too many wrong codes from this address. Wait an hour and try again." };
    // A ceiling on a day's spend, checked before the code is touched so a refused upload does not use it up.
    if (this.one("SELECT COUNT(*) AS n FROM reports WHERE created > ?", now - 24 * HOUR).n >= num(this.env.MAX_RUNS_PER_DAY, 20)) return { ok: false, status: 429, error: "The developer's support service is busy today. Your code has not been used up; try again tomorrow." };

    const ticket = newTicket();
    const won = this.one("UPDATE codes SET used = ?, ticket = ? WHERE hash = ? AND used IS NULL AND revoked = 0 AND expires > ? RETURNING label", now, ticket, hash, now);
    if (!won) {
      const code = this.one("SELECT used, revoked, expires FROM codes WHERE hash = ?", hash);
      if (!code) {
        this.sql.exec("INSERT INTO tries (ip, at) VALUES (?, ?)", ip, now);
        return { ok: false, status: 401, error: "That support code is not known. Check it for typing mistakes." };
      }
      if (code.used) return { ok: false, status: 403, error: "That support code has already been used. Each code works once; ask the developer for a new one." };
      if (code.revoked) return { ok: false, status: 403, error: "That support code was withdrawn. Ask the developer for a new one." };
      return { ok: false, status: 410, error: "That support code has expired. Ask the developer for a new one." };
    }
    this.sql.exec("INSERT INTO reports (ticket, label, created, status, glint_version, bundle) VALUES (?, ?, ?, 'queued', ?, ?)", ticket, won.label, now, glintVersion, bundle);
    this.ctx.storage.setAlarm(now);
    return { ok: true, ticket };
  }

  /* ---------- reports ---------- */

  /** What the user's tool may know: how far the report is, and the short summary if that is switched on. */
  status(ticket) {
    const row = this.one("SELECT status, report FROM reports WHERE ticket = ?", ticket);
    if (!row) return null;
    const waiting = row.status === "queued" || row.status === "analyzing";
    const out = { status: waiting ? "working" : row.status };
    if (row.status === "done" && (this.env.USER_SUMMARY || "on") !== "off") {
      // Plain text only: no address may reach the user through the model, whatever a log line said.
      let summary = "";
      try { summary = String(JSON.parse(row.report).user_summary || ""); } catch {}
      out.user_summary = summary
        .replace(/\b(?:https?|ftp|file):\/\/\S+|\bwww\.\S+/gi, "")
        .replace(/[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}/g, "")
        .replace(/[`*_#<>\[\]]/g, "")
        .replace(/[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/g, " ")
        .replace(/[ \t]{2,}/g, " ")
        .trim()
        .slice(0, 900);
    }
    return out;
  }

  listReports() {
    return this.sql.exec("SELECT ticket, label, created, status, attempts, finished, glint_version, error, model, input_tokens, output_tokens, delivered FROM reports ORDER BY created DESC LIMIT 200").toArray();
  }

  getReport(ticket, withBundle) {
    const row = this.one("SELECT * FROM reports WHERE ticket = ?", ticket);
    if (!row) return null;
    const parse = (t) => { try { return t ? JSON.parse(t) : null; } catch { return null; } };
    const out = { ...row, report: parse(row.report), meta: parse(row.meta), bundle: withBundle ? parse(row.bundle) : undefined };
    return out;
  }

  deleteReport(ticket) {
    return this.sql.exec("DELETE FROM reports WHERE ticket = ? RETURNING ticket", ticket).toArray().length > 0;
  }

  /** Run the analysis of a stored upload again (after a failure, or after changing the prompt). */
  retry(ticket) {
    const ok = this.sql.exec("UPDATE reports SET status = 'queued', attempts = 0, next_try = 0, error = NULL WHERE ticket = ? RETURNING ticket", ticket).toArray().length > 0;
    if (ok) this.ctx.storage.setAlarm(Date.now());
    return ok;
  }

  /* ---------- the work, after the user has been answered ---------- */

  /**
   * Runs when `redeem` or `retry` asked for it, when a failed attempt is due again, and once a
   * day to clear old reports. One upload per run. Alarms of one object never overlap, so a row
   * still marked 'analyzing' here was left by a run that was cut short, and is simply taken again.
   * A report waiting to be tried again (`next_try`) does not hold up a newer one.
   */
  async alarm() {
    const now = Date.now();
    this.sql.exec("DELETE FROM reports WHERE created < ?", now - num(this.env.RETAIN_DAYS, 30) * 24 * HOUR);
    this.sql.exec("DELETE FROM codes WHERE expires < ? AND created < ?", now - 30 * 24 * HOUR, now - num(this.env.RETAIN_DAYS, 30) * 24 * HOUR);

    const job = this.one("SELECT ticket, bundle, attempts FROM reports WHERE status IN ('queued', 'analyzing') AND next_try <= ? ORDER BY created LIMIT 1", now);
    if (job) {
      const attempts = job.attempts + 1;
      this.sql.exec("UPDATE reports SET status = 'analyzing', attempts = ? WHERE ticket = ?", attempts, job.ticket);
      let finished = true;
      try {
        const { report, meta } = await analyze(this.env, JSON.parse(job.bundle));
        this.sql.exec("UPDATE reports SET status = 'done', finished = ?, report = ?, meta = ?, error = NULL, model = ?, input_tokens = ?, output_tokens = ? WHERE ticket = ?", Date.now(), JSON.stringify(report), JSON.stringify(meta), meta.model, meta.input_tokens, meta.output_tokens, job.ticket);
      } catch (e) {
        const error = String(e?.message || e).slice(0, 1000);
        if (e instanceof Permanent || attempts >= MAX_ATTEMPTS) {
          this.sql.exec("UPDATE reports SET status = 'failed', finished = ?, error = ? WHERE ticket = ?", Date.now(), error, job.ticket);
        } else {
          finished = false;
          this.sql.exec("UPDATE reports SET status = 'queued', next_try = ?, error = ? WHERE ticket = ?", Date.now() + 60 * 1000 * attempts, error, job.ticket);
        }
      }
      if (finished) {
        let delivered = false;
        try { delivered = await postToDiscord(this.env, this.getReport(job.ticket, false)); } catch (e) { console.error(e); }
        this.sql.exec("UPDATE reports SET delivered = ? WHERE ticket = ?", delivered ? 1 : 0, job.ticket);
      }
    }
    // The next run: soon for a report that is due, at its time for one that waits, else tomorrow
    // to delete what has grown old, as long as anything is stored.
    const next = this.one("SELECT MIN(next_try) AS at FROM reports WHERE status IN ('queued', 'analyzing')").at;
    if (next !== null && next !== undefined) await this.ctx.storage.setAlarm(Math.max(next, Date.now() + 1000));
    else if (this.one("SELECT (SELECT COUNT(*) FROM reports) + (SELECT COUNT(*) FROM codes) AS n").n > 0) await this.ctx.storage.setAlarm(Date.now() + 24 * HOUR);
  }
}
