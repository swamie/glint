// Checks the /report route with a fake R2 bucket and a fake Discord: node test/report.test.mjs
import worker from "../src/index.js";

const store = new Map();
const BUCKET = {
  async get(k) { return store.has(k) ? { json: async () => JSON.parse(store.get(k)) } : null; },
  async put(k, v) { store.set(k, v); },
  async list() { return { objects: [], truncated: false }; },
};
const sent = [];
let discordStatus = 204;
globalThis.fetch = async (url, init) => {
  const form = init.body;
  sent.push({ url, payload: JSON.parse(form.get("payload_json")), log: form.get("files[0]") ? await form.get("files[0]").text() : null });
  return new Response(discordStatus === 204 ? null : JSON.stringify({ message: "Invalid Form Body" }), { status: discordStatus });
};
const HOOK = "https://discord.com/api/webhooks/123456789/abc-DEF_ghi";
const env = { BUCKET, BUG_WEBHOOK_URL: HOOK, BUG_PING_IDS: "111111111111, nope ,222222222222", REPORT_GAP_S: "0" };

let failed = 0;
const check = (name, ok) => { console.log(`${ok ? "PASS" : "FAIL"} ${name}`); if (!ok) failed++; };
function req(kind, payload, { log, ip = "1.1.1.1", method = "POST", raw } = {}) {
  const form = new FormData();
  form.append("payload_json", raw ?? JSON.stringify(payload));
  if (log !== undefined) form.append("files[0]", new Blob([log], { type: "text/plain" }), "glint-log.txt");
  return new Request(`https://clips.example/report${kind ? `?kind=${kind}` : ""}`, { method, body: method === "POST" ? form : undefined, headers: { "CF-Connecting-IP": ip } });
}
const bug = {
  content: "Bug report · Glint 0.1.52",
  allowed_mentions: { parse: ["everyone"] },
  embeds: [{ description: "@everyone the clip had no sound", color: 0xf5b83d, image: { url: "https://evil" }, fields: [{ name: "System", value: "```\nGlint 0.1.52\n```" }] }, { description: "second embed" }],
};

let r = await worker.fetch(req("bug", bug, { log: "line 1\nline 2" }), env);
check("bug report accepted (204)", r.status === 204 && sent.length === 1);
let p = sent[0].payload;
check("posted to the secret webhook", sent[0].url === HOOK);
check("content pings the valid ids only", p.content === "<@111111111111> <@222222222222>\nBug report · Glint 0.1.52");
check("allowed_mentions forced to the ids", JSON.stringify(p.allowed_mentions) === JSON.stringify({ parse: [], users: ["111111111111", "222222222222"] }));
check("one embed, no image", p.embeds.length === 1 && !p.embeds[0].image);
check("description and fields kept", p.embeds[0].description.includes("no sound") && p.embeds[0].fields[0].name === "System");
check("log attached", sent[0].log === "line 1\nline 2");

const fb = { content: "<@999> Feedback · Glint 0.1.52", embeds: [{ title: "Feedback", fields: [{ name: "Stars", value: "★★★★☆ 4 / 5" }] }] };
r = await worker.fetch(req("feedback", fb, { log: "x", ip: "2.2.2.2" }), env);
p = sent.at(-1).payload;
check("feedback accepted", r.status === 204 && sent.length === 2);
check("feedback pings no one", p.allowed_mentions.parse.length === 0 && !p.allowed_mentions.users && !p.content.includes("<@") && p.content === "999 Feedback · Glint 0.1.52");
check("feedback carries no file", sent.at(-1).log === null);

r = await worker.fetch(req("bug", { embeds: [{}] }, { ip: "3.3.3.3" }), env);
check("empty report refused (400)", r.status === 400);
r = await worker.fetch(req("bug", null, { raw: "not json", ip: "3.3.3.3" }), env);
check("bad JSON refused (400)", r.status === 400);
r = await worker.fetch(req("bug", bug, { log: "x".repeat(257 * 1024), ip: "3.3.3.3" }), env);
check("big log refused (413)", r.status === 413);
r = await worker.fetch(new Request("https://clips.example/report", { method: "GET" }), env);
check("GET refused (405)", r.status === 405);

// Per-IP gap.
const gapEnv = { ...env, REPORT_GAP_S: "30" };
r = await worker.fetch(req("bug", bug, { ip: "4.4.4.4" }), gapEnv);
const r2 = await worker.fetch(req("bug", bug, { ip: "4.4.4.4" }), gapEnv);
check("second report from one IP at once refused (429)", r.status === 204 && r2.status === 429);
// Per-IP daily cap.
const capEnv = { ...env, REPORT_PER_IP_DAY: "2" };
const codes = [];
for (let i = 0; i < 3; i++) codes.push((await worker.fetch(req("bug", bug, { ip: "5.5.5.5" }), capEnv)).status);
check("per-IP daily cap", codes.join() === "204,204,429");

// Discord says no.
discordStatus = 400;
r = await worker.fetch(req("bug", bug, { ip: "6.6.6.6" }), env);
const err = await r.json();
check("Discord 400 is a 502 with its reason, without the address", r.status === 502 && err.error.includes("HTTP 400") && !err.error.includes("webhooks"));
discordStatus = 204;

r = await worker.fetch(req("bug", bug, { ip: "7.7.7.7" }), { BUCKET });
check("no webhook secret: 503", r.status === 503);
r = await worker.fetch(req("bug", bug, { ip: "7.7.7.8" }), { BUCKET, BUG_WEBHOOK_URL: "https://example.com/x" });
check("a webhook that is not Discord's is not used", r.status === 503);

console.log(failed ? `${failed} failed` : "all passed");
process.exit(failed ? 1 : 0);
