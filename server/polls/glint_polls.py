#!/usr/bin/env python3
"""Glint polls: a small server where the developer asks Glint's users questions.

Python 3 standard library only. It listens on 127.0.0.1:8787; Caddy in front
of it does HTTPS and passes requests on (see setup.sh).

Public API (what Glint calls, with curl):
  GET  /v1/polls?install=<32 hex>   open polls and polls closed less than 3 days ago
  POST /v1/vote {poll, options, install, comment?}
  GET  /v1/health                   {"ok": true}

Admin API (every call needs "Authorization: Bearer <admin key>"):
  GET    /v1/admin/polls              every poll, with live counts and comments
  POST   /v1/admin/polls              {question, options, multiple, allow_comment, hours}
  POST   /v1/admin/polls/<id>/close
  POST   /v1/admin/polls/<id>/reopen  {hours?}
  DELETE /v1/admin/polls/<id>

GET /admin is the admin web page (one self-contained HTML page).

What is stored about a voter: the random install number Glint made, the
options picked, the comment and the time. No IP address is stored; the
abuse limits count per IP in memory only and forget it.
"""

import argparse
import hmac
import ipaddress
import json
import os
import re
import sqlite3
import sys
import threading
import time
from collections import OrderedDict
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

MAX_BODY = 4096
MAX_QUESTION = 200
MAX_OPTION = 80
MAX_COMMENT = 300
MIN_OPTIONS, MAX_OPTIONS = 2, 8
RECENT_SECS = 3 * 86400          # closed polls stay visible this long
INSTALL_RE = re.compile(r"^[0-9a-f]{32}$")


def ip_bucket(ip):
    """What the per-IP limits count: an IPv4 address as it is, an IPv6 address by its /64
    (one home or server gets a whole /64), anything unparsable in one shared bucket."""
    try:
        addr = ipaddress.ip_address(ip.strip())
    except ValueError:
        return "unparsable"
    if addr.version == 6:
        if addr.ipv4_mapped:
            return str(addr.ipv4_mapped)
        return str(ipaddress.ip_network(f"{addr}/64", strict=False))
    return str(addr)


class Table:
    """A dict with a hard size cap: when full, the oldest entry (least recently set) goes first."""

    def __init__(self, cap):
        self.cap = cap
        self.data = OrderedDict()

    def get(self, key, default=None):
        return self.data.get(key, default)

    def set(self, key, value):
        self.data[key] = value
        self.data.move_to_end(key)
        while len(self.data) > self.cap:
            self.data.popitem(last=False)

    def drop_where(self, stale):
        for k in [k for k, v in self.data.items() if stale(k, v)]:
            del self.data[k]

    def __len__(self):
        return len(self.data)


class Limits:
    """The abuse limits. Counted in memory only, per IP bucket (`ip_bucket`); nothing here is
    written to disk. Every table has a hard cap (`cap` entries, oldest dropped first), and
    entries whose time window has passed are dropped once a minute."""

    def __init__(self, per_minute=60, votes_per_day=10, admin_fails=5, lockout_secs=600, cap=20000):
        self.per_minute = per_minute
        self.votes_per_day = votes_per_day
        self.admin_fails = admin_fails
        self.lockout_secs = lockout_secs
        self.lock = threading.Lock()
        self.requests = Table(cap)   # bucket -> [minute start, count]
        self.votes = Table(cap)      # (bucket, poll, day) -> count
        self.fails = Table(cap)      # bucket -> [fail times]
        self.locked = Table(cap)     # bucket -> locked until
        self.swept = time.time()

    def _sweep(self, now):
        if now - self.swept < 60:
            return
        self.swept = now
        day = int(now // 86400)
        self.requests.drop_where(lambda k, v: now - v[0] >= 60)
        self.votes.drop_where(lambda k, v: k[2] < day)
        self.fails.drop_where(lambda k, v: all(now - x >= self.lockout_secs for x in v))
        self.locked.drop_where(lambda k, v: v <= now)

    def request(self, ip):
        """False when this IP made too many requests in the last minute."""
        key = ip_bucket(ip)
        now = time.time()
        with self.lock:
            self._sweep(now)
            slot = self.requests.get(key)
            if not slot or now - slot[0] >= 60:
                self.requests.set(key, [now, 1])
                return True
            slot[1] += 1
            return slot[1] <= self.per_minute

    def vote_allowed(self, ip, poll):
        """False when this IP already has its votes for this poll today. Counts nothing."""
        key = (ip_bucket(ip), poll, int(time.time() // 86400))
        with self.lock:
            return self.votes.get(key, 0) < self.votes_per_day

    def vote_counted(self, ip, poll):
        """A vote the store accepted: one more for this IP, poll and day."""
        key = (ip_bucket(ip), poll, int(time.time() // 86400))
        with self.lock:
            self.votes.set(key, self.votes.get(key, 0) + 1)

    def admin_locked(self, ip):
        with self.lock:
            return self.locked.get(ip_bucket(ip), 0) > time.time()

    def admin_failed(self, ip):
        key = ip_bucket(ip)
        now = time.time()
        with self.lock:
            recent = [x for x in self.fails.get(key, []) if now - x < self.lockout_secs] + [now]
            self.fails.set(key, recent)
            if len(recent) >= self.admin_fails:
                self.locked.set(key, now + self.lockout_secs)
                self.fails.set(key, [])

class Store:
    """The SQLite database: polls, their options and the votes. One connection, one lock."""

    def __init__(self, path):
        self.lock = threading.Lock()
        self.db = sqlite3.connect(path, check_same_thread=False)
        self.db.row_factory = sqlite3.Row
        with self.lock:
            self.db.execute("PRAGMA journal_mode=WAL")
            self.db.execute("PRAGMA foreign_keys=ON")
            self.db.executescript(
                """
                CREATE TABLE IF NOT EXISTS polls (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    question TEXT NOT NULL,
                    multiple INTEGER NOT NULL DEFAULT 0,
                    allow_comment INTEGER NOT NULL DEFAULT 0,
                    opens INTEGER NOT NULL,
                    closes INTEGER,
                    closed_at INTEGER
                );
                CREATE TABLE IF NOT EXISTS options (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    poll INTEGER NOT NULL REFERENCES polls(id) ON DELETE CASCADE,
                    pos INTEGER NOT NULL,
                    text TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS votes (
                    poll INTEGER NOT NULL REFERENCES polls(id) ON DELETE CASCADE,
                    install TEXT NOT NULL,
                    options TEXT NOT NULL,
                    comment TEXT NOT NULL DEFAULT '',
                    time INTEGER NOT NULL,
                    PRIMARY KEY (poll, install)
                );
                """
            )
            self.db.commit()

    # ---- reading

    @staticmethod
    def ended_at(row, now):
        """When the poll ended (unix seconds), or None while it is open."""
        if row["closed_at"] is not None:
            return row["closed_at"]
        if row["closes"] is not None and row["closes"] <= now:
            return row["closes"]
        return None

    def _options(self, poll_id):
        return [dict(id=r["id"], text=r["text"]) for r in self.db.execute("SELECT id, text FROM options WHERE poll=? ORDER BY pos", (poll_id,))]

    def _results(self, poll_id, option_ids):
        counts = {str(o): 0 for o in option_ids}
        total = 0
        for r in self.db.execute("SELECT options FROM votes WHERE poll=?", (poll_id,)):
            total += 1
            for o in json.loads(r["options"]):
                if str(o) in counts:
                    counts[str(o)] += 1
        return {"total": total, "counts": counts}

    def _public(self, row, install, now):
        ended = self.ended_at(row, now)
        options = self._options(row["id"])
        mine = self.db.execute("SELECT options FROM votes WHERE poll=? AND install=?", (row["id"], install)).fetchone()
        voted = json.loads(mine["options"]) if mine else None
        show = voted is not None or ended is not None   # nobody is steered before voting
        return {
            "id": row["id"],
            "question": row["question"],
            "options": options,
            "multiple": bool(row["multiple"]),
            "allow_comment": bool(row["allow_comment"]),
            "opens": row["opens"],
            "closes": ended if ended is not None else row["closes"],
            "closed": ended is not None,
            "voted": voted,
            "results": self._results(row["id"], [o["id"] for o in options]) if show else None,
        }

    def public_list(self, install):
        now = int(time.time())
        with self.lock:
            out = []
            for row in self.db.execute("SELECT * FROM polls ORDER BY id DESC"):
                ended = self.ended_at(row, now)
                if ended is None or now - ended < RECENT_SECS:
                    out.append(self._public(row, install, now))
            return out

    def admin_list(self):
        now = int(time.time())
        with self.lock:
            out = []
            for row in self.db.execute("SELECT * FROM polls ORDER BY id DESC"):
                p = self._public(row, "", now)
                p["results"] = self._results(row["id"], [o["id"] for o in p["options"]])
                p["comments"] = [
                    {"text": r["comment"], "time": r["time"]}
                    for r in self.db.execute("SELECT comment, time FROM votes WHERE poll=? AND comment != '' ORDER BY time DESC", (row["id"],))
                ]
                del p["voted"]
                out.append(p)
            return out

    # ---- writing

    def vote(self, poll_id, install, options, comment):
        """Store a vote (replacing this install's earlier one). Returns (status, answer)."""
        now = int(time.time())
        with self.lock:
            row = self.db.execute("SELECT * FROM polls WHERE id=?", (poll_id,)).fetchone()
            if row is None:
                return 404, {"error": "There is no such poll."}
            if self.ended_at(row, now) is not None:
                return 409, {"error": "This poll has ended."}
            valid = {o["id"] for o in self._options(poll_id)}
            if not options or any(o not in valid for o in options) or len(set(options)) != len(options):
                return 400, {"error": "An option is not part of this poll."}
            if not row["multiple"] and len(options) != 1:
                return 400, {"error": "This poll takes one answer."}
            if comment and not row["allow_comment"]:
                return 400, {"error": "This poll takes no comments."}
            self.db.execute(
                "INSERT INTO votes (poll, install, options, comment, time) VALUES (?, ?, ?, ?, ?) "
                "ON CONFLICT(poll, install) DO UPDATE SET options=excluded.options, comment=excluded.comment, time=excluded.time",
                (poll_id, install, json.dumps(sorted(options)), comment, now),
            )
            self.db.commit()
            return 200, self._public(row, install, now)

    def create(self, question, options, multiple, allow_comment, hours):
        now = int(time.time())
        closes = now + int(hours * 3600) if hours else None
        with self.lock:
            cur = self.db.execute("INSERT INTO polls (question, multiple, allow_comment, opens, closes) VALUES (?, ?, ?, ?, ?)", (question, int(multiple), int(allow_comment), now, closes))
            poll_id = cur.lastrowid
            for pos, text in enumerate(options):
                self.db.execute("INSERT INTO options (poll, pos, text) VALUES (?, ?, ?)", (poll_id, pos, text))
            self.db.commit()
            return poll_id

    def close(self, poll_id):
        now = int(time.time())
        with self.lock:
            n = self.db.execute("UPDATE polls SET closed_at=? WHERE id=? AND closed_at IS NULL", (now, poll_id)).rowcount
            self.db.commit()
            return n > 0 or self._exists(poll_id)

    def reopen(self, poll_id, hours):
        now = int(time.time())
        with self.lock:
            if not self._exists(poll_id):
                return False
            row = self.db.execute("SELECT closes FROM polls WHERE id=?", (poll_id,)).fetchone()
            closes = row["closes"]
            if hours:
                closes = now + int(hours * 3600)
            elif closes is not None and closes <= now:
                closes = None   # its end time had passed: open until closed by hand
            self.db.execute("UPDATE polls SET closed_at=NULL, closes=? WHERE id=?", (closes, poll_id))
            self.db.commit()
            return True

    def delete(self, poll_id):
        with self.lock:
            n = self.db.execute("DELETE FROM polls WHERE id=?", (poll_id,)).rowcount
            self.db.commit()
            return n > 0

    def _exists(self, poll_id):
        return self.db.execute("SELECT 1 FROM polls WHERE id=?", (poll_id,)).fetchone() is not None


def clean_text(value, limit):
    """A one-line text: control characters become spaces, outer spaces go. None when too long or not text."""
    if not isinstance(value, str):
        return None
    text = "".join(" " if (ord(c) < 32 or ord(c) == 127) else c for c in value).strip()
    return text if len(text) <= limit else None


MAX_DEPTH = 4   # a vote or a new poll is never nested deeper than {"options": [...]}


def nesting(raw):
    """How deep `[` / `{` nest in a JSON text (strings skipped), without parsing it."""
    depth = deepest = 0
    in_str = esc = False
    for ch in raw:
        if in_str:
            if esc:
                esc = False
            elif ch == "\\":
                esc = True
            elif ch == '"':
                in_str = False
        elif ch == '"':
            in_str = True
        elif ch in "[{":
            depth += 1
            deepest = max(deepest, depth)
        elif ch in "]}":
            depth -= 1
    return deepest


def no_constant(name):
    raise ValueError(f"{name} is not allowed")


def whole_hours(value):
    """`hours` as given: a whole number from 0 (never ends) to 8760, or None when it is not."""
    if value is None:
        return 0
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= 24 * 365:
        return None
    return value


class Handler(BaseHTTPRequestHandler):
    server_version = "GlintPolls"
    sys_version = ""
    protocol_version = "HTTP/1.1"
    # A client that sends its headers or body slowly (or never) is dropped after this many
    # seconds of silence, so it cannot hold a thread for ever.
    timeout = 15

    # The server logs nothing about requests: no addresses, no install numbers, no keys.
    def log_message(self, fmt, *args):
        pass

    # ---- plumbing

    def client_ip(self):
        peer = self.client_address[0]
        # Caddy, on the same machine, sets X-Forwarded-For; its last entry is the address Caddy saw.
        if peer in ("127.0.0.1", "::1"):
            fwd = self.headers.get("X-Forwarded-For", "")
            last = fwd.split(",")[-1].strip()
            if last:
                return last[:64]
        return peer

    def send(self, status, payload, ctype="application/json; charset=utf-8"):
        body = payload if isinstance(payload, bytes) else json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        if not getattr(self, "consumed", False) and self.headers.get("Content-Length", "0").strip() not in ("", "0"):
            # The request's body was not read: the connection cannot carry another request.
            self.close_connection = True
        if self.close_connection:
            self.send_header("Connection", "close")
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        if ctype.startswith("text/html"):
            self.send_header("Content-Security-Policy", "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")
            self.send_header("X-Frame-Options", "DENY")
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def error(self, status, message):
        self.send(status, {"error": message})

    def body(self):
        """The JSON body as a dict, or None after an error answer was sent."""
        try:
            length = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            length = -1
        if length < 0 or length > MAX_BODY:
            self.close_connection = True
            self.error(413, "The request is too large.")
            return None
        try:
            raw = self.rfile.read(length) if length else b""
        except TimeoutError:
            self.close_connection = True   # the body never came: drop the connection, no answer
            return None
        self.consumed = True
        if len(raw) < length:
            self.close_connection = True
            return None
        try:
            text = raw.decode("utf-8")
            if nesting(text) > MAX_DEPTH:
                raise ValueError("nested too deep")
            # NaN and Infinity are not JSON; Python would accept them.
            data = json.loads(text, parse_constant=no_constant) if text else {}
        except (ValueError, UnicodeDecodeError, RecursionError):
            self.error(400, "The request is not valid JSON.")
            return None
        if not isinstance(data, dict):
            self.error(400, "The request is not a JSON object.")
            return None
        return data

    def handle_any(self, method):
        self.consumed = False
        try:
            if "Transfer-Encoding" in self.headers:
                # Only bodies with a Content-Length are read (Glint and Caddy send one).
                self.close_connection = True
                return self.error(411, "A Content-Length is required.")
            ip = self.client_ip()
            if not self.server.limits.request(ip):
                self.close_connection = True
                return self.error(429, "Too many requests. Try again in a minute.")
            url = urlsplit(self.path)
            path = url.path.rstrip("/") or "/"
            if path.startswith("/v1/admin"):
                return self.admin(method, path, ip)
            if method == "GET" and path == "/v1/polls":
                return self.list_polls(url.query)
            if method == "POST" and path == "/v1/vote":
                return self.vote(ip)
            if method == "GET" and path == "/v1/health":
                return self.send(200, {"ok": True})
            if method == "GET" and path == "/admin":
                return self.send(200, ADMIN_PAGE.encode("utf-8"), "text/html; charset=utf-8")
            self.error(404, "Not found.")
        except Exception as e:  # never leak a traceback to a client
            print(f"glint-polls: internal error: {type(e).__name__}", file=sys.stderr)
            try:
                self.error(500, "Internal error.")
            except Exception:
                pass

    def do_GET(self):
        self.handle_any("GET")

    def do_POST(self):
        self.handle_any("POST")

    def do_DELETE(self):
        self.handle_any("DELETE")

    # ---- public

    def list_polls(self, query):
        install = (parse_qs(query).get("install") or [""])[0]
        if not INSTALL_RE.match(install):
            return self.error(400, "The install number must be 32 lower-case hex digits.")
        self.send(200, {"polls": self.server.store.public_list(install)})

    def vote(self, ip):
        data = self.body()
        if data is None:
            return
        install = data.get("install")
        if not isinstance(install, str) or not INSTALL_RE.match(install):
            return self.error(400, "The install number must be 32 lower-case hex digits.")
        poll = data.get("poll")
        options = data.get("options")
        if not isinstance(poll, int) or isinstance(poll, bool) or not 0 < poll < 2**53:
            return self.error(400, "The poll is missing.")
        if not isinstance(options, list) or not all(isinstance(o, int) and not isinstance(o, bool) and 0 < o < 2**53 for o in options) or len(options) > MAX_OPTIONS:
            return self.error(400, "The options must be a list of option ids.")
        comment = data.get("comment") or ""
        comment = clean_text(comment, MAX_COMMENT)
        if comment is None:
            return self.error(400, f"A comment can be at most {MAX_COMMENT} characters.")
        if not self.server.limits.vote_allowed(ip, poll):
            return self.error(429, "Too many votes from this network today.")
        status, answer = self.server.store.vote(poll, install, options, comment)
        if status == 200:
            self.server.limits.vote_counted(ip, poll)   # only a vote the store took costs one
        self.send(status, answer)

    # ---- admin

    def admin(self, method, path, ip):
        limits = self.server.limits
        if limits.admin_locked(ip):
            return self.error(429, "Too many wrong keys. Try again in 10 minutes.")
        auth = self.headers.get("Authorization", "")
        given = auth[7:].strip() if auth.startswith("Bearer ") else ""
        if not hmac.compare_digest(given.encode("utf-8"), self.server.admin_key.encode("utf-8")):
            limits.admin_failed(ip)
            return self.error(401, "Wrong admin key.")
        store = self.server.store
        if path == "/v1/admin/polls":
            if method == "GET":
                return self.send(200, {"polls": store.admin_list()})
            if method == "POST":
                return self.create(store)
            return self.error(405, "Not allowed.")
        m = re.fullmatch(r"/v1/admin/polls/(\d{1,12})(?:/(close|reopen))?", path)
        if not m:
            return self.error(404, "Not found.")
        poll_id, action = int(m.group(1)), m.group(2)
        if method == "DELETE" and action is None:
            return self.send(200, {"ok": True}) if store.delete(poll_id) else self.error(404, "There is no such poll.")
        if method == "POST" and action == "close":
            return self.send(200, {"ok": True}) if store.close(poll_id) else self.error(404, "There is no such poll.")
        if method == "POST" and action == "reopen":
            data = self.body()
            if data is None:
                return
            hours = whole_hours(data.get("hours"))
            if hours is None:
                return self.error(400, "Hours must be a whole number from 0 (never ends) to 8760.")
            return self.send(200, {"ok": True}) if store.reopen(poll_id, hours) else self.error(404, "There is no such poll.")
        self.error(405, "Not allowed.")

    def create(self, store):
        data = self.body()
        if data is None:
            return
        question = clean_text(data.get("question"), MAX_QUESTION)
        if not question:
            return self.error(400, f"The question must be 1 to {MAX_QUESTION} characters.")
        raw = data.get("options")
        if not isinstance(raw, list) or not MIN_OPTIONS <= len(raw) <= MAX_OPTIONS:
            return self.error(400, f"A poll needs {MIN_OPTIONS} to {MAX_OPTIONS} answers.")
        options = [clean_text(o, MAX_OPTION) for o in raw]
        if any(not o for o in options):
            return self.error(400, f"Every answer must be 1 to {MAX_OPTION} characters.")
        if len({o.lower() for o in options}) != len(options):
            return self.error(400, "Two answers are the same.")
        hours = whole_hours(data.get("hours"))
        if hours is None:
            return self.error(400, "Hours must be a whole number from 0 (never ends) to 8760.")
        poll_id = store.create(question, options, bool(data.get("multiple")), bool(data.get("allow_comment")), hours)
        self.send(200, {"ok": True, "id": poll_id})


class PollServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, address, store, admin_key, limits):
        super().__init__(address, Handler)
        self.store = store
        self.admin_key = admin_key
        self.limits = limits


def make_server(host, port, db_path, admin_key, limits=None):
    if not admin_key or len(admin_key) < 16:
        raise ValueError("the admin key is missing or too short")
    return PollServer((host, port), Store(db_path), admin_key, limits or Limits())


def main():
    ap = argparse.ArgumentParser(description="Glint polls server")
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=8787)
    ap.add_argument("--db", default="/var/lib/glint-polls/polls.db")
    ap.add_argument("--key-file", default="/etc/glint-polls/admin.key")
    args = ap.parse_args()
    with open(args.key_file, encoding="utf-8") as f:
        key = f.read().strip()
    os.makedirs(os.path.dirname(os.path.abspath(args.db)), exist_ok=True)
    server = make_server(args.host, args.port, args.db, key)
    print(f"glint-polls: listening on {args.host}:{args.port}", file=sys.stderr)
    server.serve_forever()


# ---------------------------------------------------------------------------
# The admin page. Self-contained: no external scripts, fonts or pictures.
# Everything a voter typed is put on the page with textContent, never as HTML.
ADMIN_PAGE = r"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<title>Glint polls</title>
<style>
:root { color-scheme: light dark; --bg: #f6f6f8; --card: #fff; --text: #17171c; --muted: #6a6a75; --line: #dedee5; --accent: #6d5cff; --bar: #e8e5ff; --danger: #d23b4f; }
@media (prefers-color-scheme: dark) { :root { --bg: #111116; --card: #1b1b22; --text: #ececf1; --muted: #9a9aa6; --line: #2e2e38; --accent: #8f82ff; --bar: #2c2850; --danger: #ff6b7d; } }
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--text); font: 15px/1.45 system-ui, -apple-system, "Segoe UI", sans-serif; }
main { max-width: 760px; margin: 0 auto; padding: 20px 16px 60px; }
h1 { font-size: 22px; margin: 4px 0 16px; }
h2 { font-size: 16px; margin: 0 0 12px; }
.card { background: var(--card); border: 1px solid var(--line); border-radius: 12px; padding: 16px; margin: 0 0 14px; }
label { display: block; font-size: 13px; color: var(--muted); margin: 10px 0 4px; }
input[type=text], input[type=password], input[type=number] { width: 100%; padding: 9px 10px; border-radius: 8px; border: 1px solid var(--line); background: var(--bg); color: var(--text); font: inherit; }
.row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
.row > input[type=text] { flex: 1; min-width: 0; }
.check { display: flex; gap: 8px; align-items: center; color: var(--text); font-size: 14px; margin: 8px 0; }
button { font: inherit; padding: 8px 14px; border-radius: 8px; border: 1px solid var(--line); background: var(--card); color: var(--text); cursor: pointer; }
button.primary { background: var(--accent); border-color: var(--accent); color: #fff; }
button.danger { color: var(--danger); }
button:disabled { opacity: .5; cursor: default; }
.muted { color: var(--muted); font-size: 13px; }
.err { color: var(--danger); font-size: 14px; min-height: 1em; margin: 8px 0 0; }
.poll-head { display: flex; justify-content: space-between; gap: 10px; align-items: flex-start; }
.q { font-weight: 600; font-size: 16px; word-break: break-word; }
.tag { font-size: 12px; padding: 2px 8px; border-radius: 99px; border: 1px solid var(--line); color: var(--muted); white-space: nowrap; }
.tag.open { color: var(--accent); border-color: var(--accent); }
.bar { position: relative; margin: 8px 0; padding: 6px 10px; border-radius: 8px; overflow: hidden; border: 1px solid var(--line); }
.bar i { position: absolute; inset: 0 auto 0 0; background: var(--bar); z-index: 0; transition: width .4s; }
.bar span { position: relative; z-index: 1; display: flex; justify-content: space-between; gap: 10px; word-break: break-word; }
.actions { display: flex; gap: 8px; flex-wrap: wrap; margin-top: 10px; }
details { margin-top: 10px; }
.comment { border-top: 1px solid var(--line); padding: 8px 0; white-space: pre-wrap; word-break: break-word; }
.hidden { display: none; }
</style>
</head>
<body>
<main>
  <h1>Glint polls</h1>

  <section class="card" id="login">
    <h2>Admin key</h2>
    <p class="muted">The key setup.sh printed. It is kept in this browser tab only, until the tab is closed.</p>
    <div class="row"><input type="password" id="key" autocomplete="off" spellcheck="false"><button class="primary" id="key-go">Open</button></div>
    <p class="err" id="login-err"></p>
  </section>

  <div id="admin" class="hidden">
    <section class="card">
      <h2>New poll</h2>
      <label for="q">Question</label>
      <input type="text" id="q" maxlength="200">
      <label>Answers (2 to 8)</label>
      <div id="opts"></div>
      <button id="add-opt" type="button">Add an answer</button>
      <label class="check"><input type="checkbox" id="multiple"> More than one answer may be picked</label>
      <label class="check"><input type="checkbox" id="comment"> Voters may add a comment</label>
      <label for="hours">Ends after (hours, 0 = never)</label>
      <input type="number" id="hours" min="0" max="8760" step="1" value="48">
      <div class="actions"><button class="primary" id="create">Create poll</button><button id="logout" type="button">Forget the key</button></div>
      <p class="err" id="create-err"></p>
    </section>
    <p class="muted" id="status"></p>
    <div id="polls"></div>
  </div>
</main>
<script>
"use strict";
const $ = (id) => document.getElementById(id);
const KEY = "glint-polls-key";
let key = "";
try { key = sessionStorage.getItem(KEY) || ""; } catch (e) {}
let timer = null;

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

async function api(method, path, body) {
  const res = await fetch(path, {
    method,
    headers: Object.assign({ "Authorization": "Bearer " + key }, body ? { "Content-Type": "application/json" } : {}),
    body: body ? JSON.stringify(body) : undefined,
    cache: "no-store",
  });
  let data = {};
  try { data = await res.json(); } catch (e) {}
  if (!res.ok) {
    const err = new Error(data.error || ("HTTP " + res.status));
    err.status = res.status;
    throw err;
  }
  return data;
}

function addOption(value) {
  const box = $("opts");
  if (box.children.length >= 8) return;
  const row = el("div", "row");
  const inp = el("input");
  inp.type = "text"; inp.maxLength = 80; inp.value = value || "";
  inp.placeholder = "Answer " + (box.children.length + 1);
  const rm = el("button", "", "Remove");
  rm.type = "button";
  rm.onclick = () => { if (box.children.length > 2) row.remove(); };
  row.append(inp, rm);
  box.append(row);
}

function resetForm() {
  $("q").value = "";
  $("opts").textContent = "";
  addOption(); addOption();
  $("multiple").checked = false;
  $("comment").checked = false;
  $("hours").value = "48";
}

function when(secs) {
  return new Date(secs * 1000).toLocaleString();
}

function render(polls) {
  const box = $("polls");
  box.textContent = "";
  if (!polls.length) box.append(el("p", "muted", "No polls yet."));
  for (const p of polls) {
    const card = el("section", "card");
    const head = el("div", "poll-head");
    head.append(el("div", "q", p.question), el("span", "tag" + (p.closed ? "" : " open"), p.closed ? "Closed" : "Open"));
    card.append(head);
    const info = [p.multiple ? "several answers" : "one answer", p.allow_comment ? "comments allowed" : "no comments", "opened " + when(p.opens)];
    info.push(p.closes ? (p.closed ? "ended " : "ends ") + when(p.closes) : "no end time");
    card.append(el("p", "muted", info.join(" · ")));
    const total = p.results ? p.results.total : 0;
    for (const o of p.options) {
      const n = p.results ? (p.results.counts[String(o.id)] || 0) : 0;
      const pct = total ? Math.round((n / total) * 100) : 0;
      const bar = el("div", "bar");
      const fill = el("i");
      fill.style.width = pct + "%";
      const label = el("span");
      label.append(el("b", "", o.text), el("span", "", n + " · " + pct + " %"));
      bar.append(fill, label);
      card.append(bar);
    }
    card.append(el("p", "muted", total + (total === 1 ? " vote" : " votes")));
    const actions = el("div", "actions");
    const toggle = el("button", "", p.closed ? "Reopen" : "Close now");
    toggle.onclick = async () => {
      if (!p.closed && !confirm("Close this poll now? Glint users will see the results.")) return;
      try { await api("POST", "/v1/admin/polls/" + p.id + (p.closed ? "/reopen" : "/close"), p.closed ? {} : undefined); refresh(); } catch (e) { $("status").textContent = e.message; }
    };
    const del = el("button", "danger", "Delete");
    del.onclick = async () => {
      if (!confirm("Delete this poll and all its votes? This cannot be undone.")) return;
      try { await api("DELETE", "/v1/admin/polls/" + p.id); refresh(); } catch (e) { $("status").textContent = e.message; }
    };
    actions.append(toggle, del);
    card.append(actions);
    if (p.comments && p.comments.length) {
      const d = el("details");
      d.append(el("summary", "", p.comments.length + (p.comments.length === 1 ? " comment" : " comments")));
      for (const c of p.comments) {
        const row = el("div", "comment");
        row.append(el("div", "muted", when(c.time)), el("div", "", c.text));
        d.append(row);
      }
      if (openComments.has(p.id)) d.open = true;
      d.addEventListener("toggle", () => { if (d.open) openComments.add(p.id); else openComments.delete(p.id); });
      card.append(d);
    }
    box.append(card);
  }
}
const openComments = new Set();

async function refresh() {
  try {
    const data = await api("GET", "/v1/admin/polls");
    render(data.polls || []);
    $("status").textContent = "Updated " + new Date().toLocaleTimeString() + " · refreshes every 5 s";
  } catch (e) {
    if (e.status === 401 || e.status === 429) return logout(e.message);
    $("status").textContent = "Could not refresh: " + e.message;
  }
}

function showAdmin() {
  $("login").classList.add("hidden");
  $("admin").classList.remove("hidden");
  refresh();
  clearInterval(timer);
  timer = setInterval(refresh, 5000);
}

function logout(msg) {
  key = "";
  try { sessionStorage.removeItem(KEY); } catch (e) {}
  clearInterval(timer);
  $("admin").classList.add("hidden");
  $("login").classList.remove("hidden");
  $("login-err").textContent = msg || "";
}

$("key-go").onclick = async () => {
  key = $("key").value.trim();
  $("key").value = "";
  if (!key) return;
  try {
    await api("GET", "/v1/admin/polls");
    try { sessionStorage.setItem(KEY, key); } catch (e) {}
    $("login-err").textContent = "";
    showAdmin();
  } catch (e) {
    key = "";
    $("login-err").textContent = e.message;
  }
};
$("key").addEventListener("keydown", (e) => { if (e.key === "Enter") $("key-go").click(); });
$("add-opt").onclick = () => addOption();
$("logout").onclick = () => logout("");
$("create").onclick = async () => {
  const options = [...$("opts").querySelectorAll("input")].map((i) => i.value.trim()).filter(Boolean);
  const body = { question: $("q").value.trim(), options, multiple: $("multiple").checked, allow_comment: $("comment").checked, hours: Number($("hours").value) || 0 };
  $("create-err").textContent = "";
  try { await api("POST", "/v1/admin/polls", body); resetForm(); refresh(); } catch (e) { $("create-err").textContent = e.message; }
};

resetForm();
if (key) showAdmin();
</script>
</body>
</html>
"""

if __name__ == "__main__":
    main()
