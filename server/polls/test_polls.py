"""Tests for glint_polls.py: python -m unittest -v test_polls (in this folder).

Each test starts the server on a free port of 127.0.0.1 with a temporary
database and a random admin key made for the run. Nothing else is contacted.
"""

import http.client
import json
import os
import secrets
import socket
import sqlite3
import tempfile
import threading
import time
import unittest

import glint_polls as gp


class Server:
    def __init__(self, limits=None):
        self.dir = tempfile.TemporaryDirectory()
        self.db = os.path.join(self.dir.name, "polls.db")
        self.key = secrets.token_urlsafe(32)
        self.httpd = gp.make_server("127.0.0.1", 0, self.db, self.key, limits)
        self.port = self.httpd.server_address[1]
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()
        self.conn = None

    def stop(self):
        if self.conn:
            self.conn.close()
        self.httpd.shutdown()
        self.httpd.server_close()
        self.httpd.store.db.close()
        self.dir.cleanup()

    def raw(self, method, path, body=None, headers=None):
        """(status, headers, bytes) over one kept-alive connection."""
        for attempt in range(2):
            if self.conn is None:
                self.conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
            try:
                self.conn.request(method, path, body=body, headers=headers or {})
                res = self.conn.getresponse()
                data = res.read()
                if res.getheader("Connection", "").lower() == "close" or res.will_close:
                    self.conn.close()
                    self.conn = None
                return res.status, res, data
            except (http.client.RemoteDisconnected, ConnectionError, http.client.CannotSendRequest):
                self.conn.close()
                self.conn = None
                if attempt:
                    raise

    def call(self, method, path, body=None, ip="198.51.100.1", key=None):
        headers = {"X-Forwarded-For": ip}
        raw = None
        if body is not None:
            raw = body if isinstance(body, bytes) else json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        if key is not None:
            headers["Authorization"] = "Bearer " + key
        status, _, data = self.raw(method, path, raw, headers)
        return status, (json.loads(data) if data else None)

    def admin(self, method, path, body=None, ip="198.51.100.200"):
        return self.call(method, path, body, ip=ip, key=self.key)

    def create(self, question="Which mode next?", options=("Ranked", "Casual"), multiple=False, allow_comment=False, hours=48):
        status, ans = self.admin("POST", "/v1/admin/polls", {"question": question, "options": list(options), "multiple": multiple, "allow_comment": allow_comment, "hours": hours})
        assert status == 200, ans
        return ans["id"]

    def poll(self, install, poll_id, ip="198.51.100.1"):
        status, ans = self.call("GET", f"/v1/polls?install={install}", ip=ip)
        assert status == 200, ans
        return next((p for p in ans["polls"] if p["id"] == poll_id), None)

    def vote(self, install, poll_id, options, comment=None, ip="198.51.100.1"):
        body = {"poll": poll_id, "options": options, "install": install}
        if comment is not None:
            body["comment"] = comment
        return self.call("POST", "/v1/vote", body, ip=ip)


def install():
    return secrets.token_hex(16)


class PollsTest(unittest.TestCase):
    def setUp(self):
        self.s = Server()

    def tearDown(self):
        self.s.stop()

    def opt_ids(self, poll_id):
        return [o["id"] for o in self.s.poll(install(), poll_id)["options"]]

    # ---- the public API

    def test_health(self):
        self.assertEqual(self.s.call("GET", "/v1/health"), (200, {"ok": True}))

    def test_install_must_be_32_hex(self):
        pid = self.s.create()
        a = self.opt_ids(pid)[0]
        for bad in ["", "abc", "A" * 32, "g" * 32, "0" * 31, "0" * 33]:
            self.assertEqual(self.s.call("GET", f"/v1/polls?install={bad}")[0], 400, bad)
            self.assertEqual(self.s.vote(bad, pid, [a])[0], 400, bad)

    def test_list_shape_and_results_hidden_before_voting(self):
        pid = self.s.create(options=("A", "B", "C"))
        me = install()
        p = self.s.poll(me, pid)
        self.assertEqual(p["question"], "Which mode next?")
        self.assertEqual([o["text"] for o in p["options"]], ["A", "B", "C"])
        self.assertFalse(p["multiple"])
        self.assertFalse(p["closed"])
        self.assertIsNone(p["voted"])
        self.assertIsNone(p["results"], "nobody is steered before voting")
        self.assertGreater(p["closes"], time.time())
        b = p["options"][1]["id"]
        status, ans = self.s.vote(me, pid, [b])
        self.assertEqual(status, 200)
        self.assertEqual(ans["voted"], [b])
        self.assertEqual(ans["results"]["total"], 1)
        self.assertEqual(ans["results"]["counts"][str(b)], 1)
        self.assertEqual(self.s.poll(me, pid)["voted"], [b])
        self.assertIsNone(self.s.poll(install(), pid)["results"], "another install still sees no results")

    def test_one_vote_per_install_a_second_replaces_the_first(self):
        pid = self.s.create()
        a, b = self.opt_ids(pid)
        me = install()
        self.s.vote(me, pid, [a])
        status, ans = self.s.vote(me, pid, [b])
        self.assertEqual(status, 200)
        self.assertEqual(ans["results"]["total"], 1)
        self.assertEqual(ans["results"]["counts"], {str(a): 0, str(b): 1})
        con = sqlite3.connect(self.s.db)
        self.assertEqual(con.execute("SELECT COUNT(*) FROM votes WHERE poll=?", (pid,)).fetchone()[0], 1)
        con.close()

    def test_closed_poll_refuses_votes_and_shows_results(self):
        pid = self.s.create()
        a = self.opt_ids(pid)[0]
        self.s.vote(install(), pid, [a])
        self.assertEqual(self.s.admin("POST", f"/v1/admin/polls/{pid}/close")[0], 200)
        status, ans = self.s.vote(install(), pid, [a])
        self.assertEqual(status, 409)
        self.assertIn("ended", ans["error"])
        p = self.s.poll(install(), pid)
        self.assertTrue(p["closed"])
        self.assertEqual(p["results"]["total"], 1, "a closed poll shows its results to everyone")

    def test_poll_ends_by_its_time_and_old_ones_disappear(self):
        pid = self.s.create(hours=1)
        a = self.opt_ids(pid)[0]
        con = sqlite3.connect(self.s.db)
        con.execute("UPDATE polls SET closes=? WHERE id=?", (int(time.time()) - 5, pid))
        con.commit()
        self.assertEqual(self.s.vote(install(), pid, [a])[0], 409)
        self.assertTrue(self.s.poll(install(), pid)["closed"])
        con.execute("UPDATE polls SET closes=? WHERE id=?", (int(time.time()) - 4 * 86400, pid))
        con.commit()
        con.close()
        self.assertIsNone(self.s.poll(install(), pid), "closed more than 3 days ago: not listed")
        self.assertIn(pid, [p["id"] for p in self.s.admin("GET", "/v1/admin/polls")[1]["polls"]])

    def test_poll_without_end(self):
        pid = self.s.create(hours=0)
        p = self.s.poll(install(), pid)
        self.assertIsNone(p["closes"])
        self.assertFalse(p["closed"])

    def test_option_of_another_poll_is_refused(self):
        p1, p2 = self.s.create(), self.s.create(question="Other?")
        other = self.opt_ids(p2)[0]
        self.assertEqual(self.s.vote(install(), p1, [other])[0], 400)
        self.assertEqual(self.s.vote(install(), p1, [])[0], 400)
        self.assertEqual(self.s.vote(install(), p1, ["1"])[0], 400)
        self.assertEqual(self.s.vote(install(), 999999, [other])[0], 404)

    def test_single_choice_takes_one_multiple_takes_several(self):
        single = self.s.create()
        a, b = self.opt_ids(single)
        self.assertEqual(self.s.vote(install(), single, [a, b])[0], 400)
        multi = self.s.create(options=("X", "Y", "Z"), multiple=True)
        x, y, z = self.opt_ids(multi)
        self.assertEqual(self.s.vote(install(), multi, [x, x])[0], 400, "the same option twice")
        status, ans = self.s.vote(install(), multi, [x, z])
        self.assertEqual(status, 200)
        self.assertEqual(ans["results"]["counts"], {str(x): 1, str(y): 0, str(z): 1})
        self.assertEqual(ans["results"]["total"], 1)

    def test_comments(self):
        no = self.s.create()
        a = self.opt_ids(no)[0]
        self.assertEqual(self.s.vote(install(), no, [a], "hello")[0], 400, "no comments on this poll")
        yes = self.s.create(allow_comment=True)
        b = self.opt_ids(yes)[0]
        self.assertEqual(self.s.vote(install(), yes, [b], "x" * 301)[0], 400)
        self.assertEqual(self.s.vote(install(), yes, [b], "x" * 300)[0], 200)
        script = '<script>alert("hi")</script> & <b>bold</b>'
        self.assertEqual(self.s.vote(install(), yes, [b], script)[0], 200)
        polls = self.s.admin("GET", "/v1/admin/polls")[1]["polls"]
        comments = [c["text"] for c in next(p for p in polls if p["id"] == yes)["comments"]]
        self.assertIn(script, comments, "stored and returned as the plain text it was")
        con = sqlite3.connect(self.s.db)
        self.assertIn(script, [r[0] for r in con.execute("SELECT comment FROM votes")])
        con.close()
        # The admin page puts voters' texts on the page with textContent only.
        self.assertNotIn("innerHTML", gp.ADMIN_PAGE)
        self.assertNotIn("insertAdjacentHTML", gp.ADMIN_PAGE)
        self.assertNotIn("document.write", gp.ADMIN_PAGE)

    def test_size_limits(self):
        pid = self.s.create(allow_comment=True)
        a = self.opt_ids(pid)[0]
        big = json.dumps({"poll": pid, "options": [a], "install": install(), "comment": "x" * 5000}).encode()
        self.assertEqual(self.s.call("POST", "/v1/vote", big)[0], 413)
        self.assertEqual(self.s.call("POST", "/v1/vote", b"not json")[0], 400)
        q = lambda **k: self.s.admin("POST", "/v1/admin/polls", {"question": "Q?", "options": ["a", "b"], **k})[0]
        self.assertEqual(q(question="x" * 201), 400)
        self.assertEqual(q(question="x" * 200), 200)
        self.assertEqual(q(question=""), 400)
        self.assertEqual(q(options=["a"]), 400)
        self.assertEqual(q(options=[str(i) for i in range(9)]), 400)
        self.assertEqual(q(options=[str(i) for i in range(8)]), 200)
        self.assertEqual(q(options=["a", "x" * 81]), 400)
        self.assertEqual(q(options=["a", "x" * 80]), 200)
        self.assertEqual(q(options=["Same", "same"]), 400)
        self.assertEqual(q(hours=-1), 400)

    def test_page_and_unknown_paths(self):
        status, res, body = self.s.raw("GET", "/admin")
        self.assertEqual(status, 200)
        self.assertIn("text/html", res.getheader("Content-Type"))
        self.assertIn("default-src 'none'", res.getheader("Content-Security-Policy"))
        page = body.decode()
        self.assertIn("sessionStorage", page)
        for external in ["http://", "https://", "<link", "src="]:
            self.assertNotIn(external, page, "self-contained")
        self.assertEqual(self.s.call("GET", "/nope")[0], 404)

    # ---- the admin API

    def test_admin_needs_the_key(self):
        self.assertEqual(self.s.call("GET", "/v1/admin/polls", ip="203.0.113.9")[0], 401)
        status, ans = self.s.call("GET", "/v1/admin/polls", ip="203.0.113.9", key="wrong")
        self.assertEqual(status, 401)
        self.assertNotIn(self.s.key, json.dumps(ans))
        self.assertEqual(self.s.call("POST", "/v1/admin/polls", {"question": "Q", "options": ["a", "b"]}, ip="203.0.113.9")[0], 401)

    def test_admin_create_close_reopen_delete(self):
        pid = self.s.create(allow_comment=True)
        a, b = self.opt_ids(pid)
        self.s.vote(install(), pid, [a], "nice")
        self.s.vote(install(), pid, [b])
        p = next(p for p in self.s.admin("GET", "/v1/admin/polls")[1]["polls"] if p["id"] == pid)
        self.assertEqual(p["results"]["total"], 2, "the admin sees live counts")
        self.assertEqual([c["text"] for c in p["comments"]], ["nice"])
        self.assertNotIn("install", json.dumps(p["comments"]))
        self.assertEqual(self.s.admin("POST", f"/v1/admin/polls/{pid}/close")[0], 200)
        self.assertTrue(self.s.poll(install(), pid)["closed"])
        self.assertEqual(self.s.admin("POST", f"/v1/admin/polls/{pid}/reopen", {"hours": 2})[0], 200)
        p = self.s.poll(install(), pid)
        self.assertFalse(p["closed"])
        self.assertAlmostEqual(p["closes"], time.time() + 7200, delta=10)
        self.assertEqual(self.s.vote(install(), pid, [a])[0], 200)
        self.assertEqual(self.s.admin("DELETE", f"/v1/admin/polls/{pid}")[0], 200)
        self.assertIsNone(self.s.poll(install(), pid))
        self.assertEqual(self.s.admin("DELETE", f"/v1/admin/polls/{pid}")[0], 404)
        con = sqlite3.connect(self.s.db)
        self.assertEqual(con.execute("SELECT COUNT(*) FROM votes").fetchone()[0], 0, "its votes went with it")
        con.close()

    def test_wrong_keys_lock_the_ip_out(self):
        bad_ip, good_ip = "203.0.113.50", "203.0.113.51"
        for _ in range(5):
            self.assertEqual(self.s.call("GET", "/v1/admin/polls", ip=bad_ip, key="guess-" + secrets.token_hex(4))[0], 401)
        self.assertEqual(self.s.call("GET", "/v1/admin/polls", ip=bad_ip, key=self.s.key)[0], 429, "locked out, even with the right key")
        self.assertEqual(self.s.call("GET", "/v1/admin/polls", ip=good_ip, key=self.s.key)[0], 200, "another IP is not")
        self.assertEqual(self.s.call("GET", f"/v1/polls?install={install()}", ip=bad_ip)[0], 200, "the public API still answers")

    def test_database_holds_no_ip(self):
        pid = self.s.create(allow_comment=True)
        a = self.opt_ids(pid)[0]
        self.s.vote(install(), pid, [a], "hi", ip="192.0.2.77")
        self.s.httpd.store.db.execute("PRAGMA wal_checkpoint(FULL)")
        con = sqlite3.connect(self.s.db)
        cols = [r[1] for r in con.execute("PRAGMA table_info(votes)")]
        con.close()
        self.assertEqual(sorted(cols), ["comment", "install", "options", "poll", "time"])
        for name in os.listdir(self.s.dir.name):
            with open(os.path.join(self.s.dir.name, name), "rb") as f:
                self.assertNotIn(b"192.0.2.77", f.read(), name)

    # ---- counting

    def test_a_thousand_votes_are_counted_right(self):
        pid = self.s.create(options=("A", "B", "C", "D"), multiple=True)
        ids = self.opt_ids(pid)
        want = {str(i): 0 for i in ids}
        for n in range(1000):
            pick = [ids[n % 4]] + ([ids[(n + 1) % 4]] if n % 3 == 0 else [])
            for o in pick:
                want[str(o)] += 1
            status, _ = self.s.vote(install(), pid, pick, ip=f"10.{n // 250}.{n % 250}.1")
            self.assertEqual(status, 200)
        p = next(p for p in self.s.admin("GET", "/v1/admin/polls")[1]["polls"] if p["id"] == pid)
        self.assertEqual(p["results"]["total"], 1000)
        self.assertEqual(p["results"]["counts"], want)


class LimitsTest(unittest.TestCase):
    def setUp(self):
        self.s = Server()

    def tearDown(self):
        self.s.stop()

    def test_sixty_requests_a_minute_per_ip(self):
        me = install()
        for _ in range(60):
            self.assertEqual(self.s.call("GET", f"/v1/polls?install={me}", ip="192.0.2.1")[0], 200)
        self.assertEqual(self.s.call("GET", f"/v1/polls?install={me}", ip="192.0.2.1")[0], 429)
        self.assertEqual(self.s.call("GET", f"/v1/polls?install={me}", ip="192.0.2.2")[0], 200, "another IP is not affected")

    def test_ten_votes_a_day_per_poll_per_ip(self):
        pid = self.s.create()
        a = [o["id"] for o in self.s.poll(install(), pid, ip="192.0.2.9")["options"]][0]
        for _ in range(10):
            self.assertEqual(self.s.vote(install(), pid, [a], ip="192.0.2.3")[0], 200)
        self.assertEqual(self.s.vote(install(), pid, [a], ip="192.0.2.3")[0], 429)
        self.assertEqual(self.s.vote(install(), pid, [a], ip="192.0.2.4")[0], 200)
        other = self.s.create(question="Another?")
        b = [o["id"] for o in self.s.poll(install(), other, ip="192.0.2.9")["options"]][0]
        self.assertEqual(self.s.vote(install(), other, [b], ip="192.0.2.3")[0], 200, "counted per poll")


class HardeningTest(unittest.TestCase):
    """The second review's findings: each test here failed before its fix."""

    def setUp(self):
        self.s = Server()

    def tearDown(self):
        self.s.stop()

    def raw_socket(self):
        sock = socket.create_connection(("127.0.0.1", self.s.port), timeout=30)
        return sock

    def test_clients_that_never_send_the_body_are_dropped(self):
        base = threading.active_count()
        socks = []
        for _ in range(20):
            sock = self.raw_socket()
            sock.sendall(b"POST /v1/vote HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n")
            socks.append(sock)
        time.sleep(1)
        self.assertGreaterEqual(threading.active_count(), base + 20, "one thread per waiting client")
        start = time.time()
        for sock in socks:
            sock.settimeout(max(0.1, 22 - (time.time() - start)))
            try:
                data = sock.recv(4096)
            except socket.timeout:
                data = None
            self.assertIsNotNone(data, "the server closed the connection by itself")
            sock.close()
        waited = time.time() - start
        deadline = time.time() + 5
        while threading.active_count() > base and time.time() < deadline:
            time.sleep(0.1)
        self.assertLessEqual(threading.active_count(), base, "every thread ended")
        self.assertLess(waited, 22)

    def test_ipv6_counts_by_its_64(self):
        me = install()
        for n in range(60):
            self.assertEqual(self.s.call("GET", f"/v1/polls?install={me}", ip=f"2001:db8:1:2::{n:x}")[0], 200)
        self.assertEqual(self.s.call("GET", f"/v1/polls?install={me}", ip="2001:db8:1:2:ffff::1")[0], 429, "the same /64")
        self.assertEqual(self.s.call("GET", f"/v1/polls?install={me}", ip="2001:db8:1:3::1")[0], 200, "another /64")
        self.assertEqual(gp.ip_bucket("::ffff:192.0.2.5"), "192.0.2.5")
        self.assertEqual(gp.ip_bucket("192.0.2.5"), "192.0.2.5")
        self.assertEqual(gp.ip_bucket("not an ip"), gp.ip_bucket("also junk"), "unparsable: one shared bucket")

    def test_refused_votes_cost_nothing(self):
        pid = self.s.create()
        closed = self.s.create(question="Closed?")
        self.s.admin("POST", f"/v1/admin/polls/{closed}/close")
        a = [o["id"] for o in self.s.poll(install(), pid, ip="192.0.2.99")["options"]][0]
        ip = "192.0.2.40"
        for _ in range(4):
            self.assertEqual(self.s.vote(install(), pid, [999999], ip=ip)[0], 400)    # not an option of it
        for _ in range(3):
            self.assertEqual(self.s.vote(install(), pid, [a, a], ip=ip)[0], 400)      # the same twice
        for _ in range(3):
            self.assertEqual(self.s.vote(install(), pid, [a], comment="no comments here", ip=ip)[0], 400)
        for _ in range(10):
            self.assertIn(self.s.vote(install(), closed, [a], ip=ip)[0], (400, 409))
            self.assertEqual(self.s.vote(install(), 424242, [a], ip=ip)[0], 404)
        self.assertEqual(self.s.vote(install(), pid, [a], ip=ip)[0], 200, "invalid votes did not use up the quota")

    def test_limit_tables_stay_under_their_cap(self):
        lim = gp.Limits(cap=20000)
        for n in range(100_000):
            ip = f"10.{n >> 16 & 255}.{n >> 8 & 255}.{n & 255}"
            lim.request(ip)
            lim.vote_counted(ip, 1)
            lim.admin_failed(ip)
        for table in (lim.requests, lim.votes, lim.fails, lim.locked):
            self.assertLessEqual(len(table), 20000)
        # The newest are kept: the last IP is still counted.
        self.assertEqual(lim.requests.get(gp.ip_bucket("10.1.134.159"))[1], 1)

    def test_transfer_encoding_is_refused(self):
        sock = self.raw_socket()
        sock.sendall(b"POST /v1/vote HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\n\r\n5\r\n{\"a\":\r\n")
        data = b""
        while True:
            chunk = sock.recv(4096)
            if not chunk:
                break
            data += chunk
        sock.close()
        self.assertTrue(data.startswith(b"HTTP/1.1 411"), data[:40])
        self.assertIn(b"Connection: close", data)

    def test_odd_numbers_and_deep_json_are_400(self):
        def admin_raw(text):
            return self.s.call("POST", "/v1/admin/polls", text.encode(), key=self.s.key)[0]
        base = '{"question": "Q?", "options": ["a", "b"], "hours": %s}'
        for bad in ["NaN", "Infinity", "-Infinity", "1.5", "1e400", "true", '"5"']:
            self.assertEqual(admin_raw(base % bad), 400, bad)
        self.assertEqual(admin_raw(base % "48"), 200)
        pid = self.s.create()
        self.assertEqual(self.s.call("POST", f"/v1/admin/polls/{pid}/reopen", b'{"hours": NaN}', key=self.s.key)[0], 400)
        a = self.opt_id(pid)
        body = '{"poll": %d, "options": [%d], "install": "%s", "x": %s}'
        self.assertEqual(self.s.call("POST", "/v1/vote", (body % (pid, a, install(), "NaN")).encode())[0], 400)
        deep = "[" * 2000 + "]" * 2000
        self.assertEqual(self.s.call("POST", "/v1/vote", deep.encode())[0], 400)
        self.assertEqual(self.s.call("POST", "/v1/vote", ('{"a":' * 6 + "1" + "}" * 6).encode())[0], 400)
        self.assertEqual(self.s.call("GET", "/v1/health")[0], 200, "still up")

    def opt_id(self, pid):
        return self.s.poll(install(), pid)["options"][0]["id"]


if __name__ == "__main__":
    unittest.main()
