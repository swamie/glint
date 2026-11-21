// The server's scrubbing pass: `node --test test/*.test.mjs` in server/doctor.
import { test } from "node:test";
import assert from "node:assert/strict";
import { cleanBundle, scrubText } from "../src/scrub.js";

const gone = (text, ...leaks) => { for (const l of leaks) assert.ok(!text.includes(l), `${l} in ${text}`); };

test("webhooks, also with an API version", () => {
  const t = scrubText("to https://discord.com/api/webhooks/1/abc and https://canary.discord.com/api/v10/webhooks/2/def?wait=true");
  assert.equal(t, "to <webhook removed> and <webhook removed>");
});

test("profile folders and OneDrive organizations", () => {
  const t = scrubText(String.raw`C:\Users\Vera Example\Videos D:/users/bob/x C:\Users\Public\x E:\OneDrive - Contoso Ltd\Glint ~\AppData`);
  assert.equal(t, String.raw`C:\Users\<user>\Videos D:/users/<user>/x C:\Users\Public\x E:\OneDrive - <org>\Glint ~\AppData`);
});

test("network share hosts, not device paths", () => {
  assert.equal(scrubText(String.raw`saved to \\NAS01\clips\x and "\\vera-pc\share"`), String.raw`saved to \\<host>\clips\x and "\\<host>\share"`);
  const device = String.raw`\\?\C:\Glint \\.\DISPLAY1`;
  assert.equal(scrubText(device), device);
});

test("addresses", () => {
  const t = scrubText("from 192.168.1.20 and fe80::1ff:fe23:4567:890a and 2001:db8::1 mail a.b@example.co.uk or c%40example.org");
  gone(t, "192.168", "fe80", "2001:db8", "example.co", "example.org");
  const keep = "2026-11-18 12:34:56.789 capture: driver 32.0.15.6094 build 10.0.19045.5131 at 127.0.0.1:8080 v1.2.3.4";
  assert.equal(scrubText(keep), keep);
});

test("keys, tokens and queries", () => {
  const t = scrubText([
    "Authorization: Bearer abcdefgh12345678",
    "access_token=ya29.a0AfH6SMBxyzxyz",
    'client_secret: "s3cr3t-value-99"',
    "x-api-key: sk-ant-api03-abcdefgh",
    "jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk",
    "gh ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789",
    "id 0123456789abcdef0123456789abcdef",
    "GET https://glint-share.vera.workers.dev/upload?key=hunter2hunter2",
    "user S-1-5-21-3623811015-3361044348-30300820-1013",
  ].join("\n"));
  gone(t, "abcdefgh1234", "ya29", "s3cr3t", "sk-ant", "eyJhbGci", "ghp_", "0123456789abcdef", "hunter2", "3623811015");
});

test("only known sections, each scrubbed, a bad version dropped", () => {
  const b = cleanBundle({ schema: 1, glint_version: "0.1.48; drop table", description: "mail me@example.com", settings: { share: { url: "<set; removed>" } }, extra: "dropped", log_tail: "x" });
  assert.deepEqual(Object.keys(b).sort(), ["description", "glint_version", "log_tail", "schema", "settings"]);
  assert.equal(b.glint_version, "");
  assert.equal(b.description, "mail <email>");
  assert.equal(cleanBundle({ schema: 2 }), null);
  assert.equal(cleanBundle([1]), null);
});

test("the tool's placeholders pass unchanged", () => {
  const t = String.raw`~\Videos\Glint\<name>.mp4 C:\Users\<user>\x \\<host>\clips <set; removed> <webhook removed> token=<removed>`;
  assert.equal(scrubText(t), t);
});
