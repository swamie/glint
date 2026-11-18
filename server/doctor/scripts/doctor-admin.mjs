#!/usr/bin/env node
// The developer's side of Glint Doctor, from a terminal. Needs Node 18 or newer, nothing else.
//
//   node scripts/doctor-admin.mjs mint "who it is for" [--hours 72] [--count 1]
//   node scripts/doctor-admin.mjs codes
//   node scripts/doctor-admin.mjs revoke <last 4 characters of the code>
//   node scripts/doctor-admin.mjs reports
//   node scripts/doctor-admin.mjs show <ticket> [--json] [--bundle]
//   node scripts/doctor-admin.mjs retry <ticket>
//   node scripts/doctor-admin.mjs delete <ticket>
//
// The Worker's address and the admin key come from the environment
// (DOCTOR_URL, DOCTOR_ADMIN_TOKEN) or from a git-ignored file `.doctor-admin`
// in server/doctor with those two lines:
//   DOCTOR_URL=https://glint-doctor.<you>.workers.dev
//   DOCTOR_ADMIN_TOKEN=<the ADMIN_TOKEN secret>
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const config = { ...process.env };
try {
  const file = readFileSync(fileURLToPath(new URL("../.doctor-admin", import.meta.url)), "utf8");
  for (const line of file.split(/\r?\n/)) {
    const m = line.match(/^\s*([A-Z_]+)\s*=\s*(.*?)\s*$/);
    if (m && !process.env[m[1]]) config[m[1]] = m[2];
  }
} catch {}

const BASE = (config.DOCTOR_URL || "").replace(/\/$/, "");
const TOKEN = config.DOCTOR_ADMIN_TOKEN || "";
const [cmd, ...rest] = process.argv.slice(2);
const flag = (name) => { const i = rest.indexOf(`--${name}`); return i >= 0 ? rest[i + 1] ?? true : undefined; };
const has = (name) => rest.includes(`--${name}`);
const words = rest.filter((a, i) => !a.startsWith("--") && !(rest[i - 1] || "").match(/^--(hours|count)$/));

function die(msg) { console.error(msg); process.exit(1); }
if (!cmd || cmd === "help" || cmd === "--help") die(readFileSync(fileURLToPath(import.meta.url), "utf8").split("\n").filter((l) => l.startsWith("//")).map((l) => l.slice(3)).join("\n"));
if (!BASE || !TOKEN) die("Set DOCTOR_URL and DOCTOR_ADMIN_TOKEN (or put them in server/doctor/.doctor-admin).");

async function api(method, path, body) {
  const res = await fetch(BASE + path, { method, headers: { Authorization: `Bearer ${TOKEN}`, ...(body ? { "Content-Type": "application/json" } : {}) }, body: body ? JSON.stringify(body) : undefined });
  const text = await res.text();
  if (!res.ok) {
    let msg = text;
    try { msg = JSON.parse(text).error; } catch {}
    die(`${method} ${path}: HTTP ${res.status}: ${msg}`);
  }
  return (res.headers.get("Content-Type") || "").includes("json") ? JSON.parse(text) : text;
}

const when = (ms) => (ms ? new Date(ms).toISOString().replace("T", " ").slice(0, 16) : "");

switch (cmd) {
  case "mint": {
    const label = words.join(" ").trim();
    if (!label) die('Say who the code is for: mint "a Discord user, black clips on AMD"');
    const { codes } = await api("POST", "/admin/codes", { label, hours: Number(flag("hours")) || undefined, count: Number(flag("count")) || undefined });
    for (const c of codes) {
      console.log(`${c.code}   good until ${c.expires.replace("T", " ").slice(0, 16)} UTC   for: ${c.label}`);
      console.log(`  send the user glint-doctor.exe renamed to:  glint-doctor-${c.code}.exe`);
    }
    console.log("A code is shown once, here. The server keeps only its hash.");
    break;
  }
  case "codes": {
    const { codes } = await api("GET", "/admin/codes");
    for (const c of codes) console.log(`…${c.hint}  ${c.state.padEnd(8)} made ${when(c.created)}  expires ${when(c.expires)}  ${c.ticket ? `ticket ${c.ticket}  ` : ""}${c.label}`);
    if (!codes.length) console.log("No codes.");
    break;
  }
  case "revoke": {
    if (!words[0]) die("revoke <last 4 characters of the code>");
    console.log(await api("DELETE", `/admin/codes/${encodeURIComponent(words[0].slice(-4))}`));
    break;
  }
  case "reports": {
    const { reports } = await api("GET", "/admin/reports");
    for (const r of reports) console.log(`${r.ticket}  ${r.status.padEnd(9)} ${when(r.created)}  Glint ${r.glint_version || "?"}  ${r.label}${r.error ? `  (${r.error.slice(0, 80)})` : ""}`);
    if (!reports.length) console.log("No reports.");
    break;
  }
  case "show": {
    if (!words[0]) die("show <ticket> [--json] [--bundle]");
    const query = has("json") || has("bundle") ? (has("bundle") ? "?include=bundle" : "") : "?format=md";
    const out = await api("GET", `/admin/reports/${words[0]}${query}`);
    console.log(typeof out === "string" ? out : JSON.stringify(out, null, 2));
    break;
  }
  case "retry":
    if (!words[0]) die("retry <ticket>");
    console.log(await api("POST", `/admin/reports/${words[0]}/retry`));
    break;
  case "delete":
    if (!words[0]) die("delete <ticket>");
    console.log(await api("DELETE", `/admin/reports/${words[0]}`));
    break;
  default:
    die(`Unknown command "${cmd}". Run with no arguments for the list.`);
}
