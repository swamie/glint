// Checks the usage tally and the published-clip cache with a fake R2 bucket: node test/usage.test.mjs
import worker from "../src/index.js";
import { timingSafeEqual } from "node:crypto";

// Workers have this on crypto.subtle; node has it elsewhere.
crypto.subtle.timingSafeEqual ??= (a, b) => timingSafeEqual(new Uint8Array(a), new Uint8Array(b));

const store = new Map(); // key -> { body: string|Uint8Array, size }
let lists = 0, metaGets = 0;
const size = (v) => (typeof v === "string" ? new TextEncoder().encode(v).length : v.length);
const BUCKET = {
  async get(k) {
    if (k.endsWith("/meta.json")) metaGets++;
    const o = store.get(k);
    if (!o) return null;
    return { json: async () => JSON.parse(o.body), body: o.body, size: o.size, httpEtag: '"e"', writeHttpMetadata() {} };
  },
  async head(k) { const o = store.get(k); return o ? { size: o.size, httpEtag: '"e"', writeHttpMetadata() {} } : null; },
  async put(k, v) { const body = typeof v === "string" ? v : new Uint8Array(await new Response(v).arrayBuffer()); store.set(k, { body, size: size(body) }); },
  async delete(k) { for (const x of Array.isArray(k) ? k : [k]) store.delete(x); },
  async list({ prefix }) { lists++; return { objects: [...store].filter(([k]) => k.startsWith(prefix)).map(([key, o]) => ({ key, size: o.size })), truncated: false }; },
  async createMultipartUpload(k) { return { uploadId: "up1" }; },
  resumeMultipartUpload(k) {
    return {
      async uploadPart(n, body) { const b = new Uint8Array(await new Response(body).arrayBuffer()); store.set(`part:${k}`, { body: b, size: b.length }); return { etag: "p1", partNumber: n }; },
      async complete() { const p = store.get(`part:${k}`); store.delete(`part:${k}`); store.set(k, p); return { size: p.size }; },
      async abort() {},
    };
  },
};
const env = { BUCKET, UPLOAD_TOKEN: "key" };
let failed = 0;
const check = (name, ok, more = "") => { console.log(`${ok ? "PASS" : "FAIL"} ${name}${ok ? "" : " " + more}`); if (!ok) failed++; };
const call = (method, path, body, headers = {}) => worker.fetch(new Request(`https://clips.example${path}`, { method, body, headers: { Authorization: "Bearer key", ...headers }, duplex: body ? "half" : undefined }), env);
const usage = async () => (await call("GET", "/api/usage")).json();

let u = await usage();
check("an empty bucket holds nothing", u.used === 0 && u.clips === 0, JSON.stringify(u));
lists = 0;
await usage(); await usage();
check("asking again does not list the bucket", lists === 0, `lists ${lists}`);

const video = new Uint8Array(5000).fill(7);
const start = await (await call("POST", "/api/uploads", JSON.stringify({ size: video.length, title: "t", width: 1920, height: 1080, duration: 3 }), { "Content-Type": "application/json" })).json();
check("an upload starts", !!start.id && start.uploadId === "up1", JSON.stringify(start));
const q = `?uploadId=${start.uploadId}`;
const part = await (await call("PUT", `/api/uploads/${start.id}/parts/1${q}`, video, { "Content-Length": String(video.length) })).json();
const pending = await worker.fetch(new Request(`https://clips.example/v/${start.id}.mp4`), env);
check("a clip still uploading is not served", pending.status === 404, String(pending.status));
const done = await call("POST", `/api/uploads/${start.id}/complete${q}`, JSON.stringify({ parts: [part] }), { "Content-Type": "application/json" });
check("the upload completes", done.status === 200, `${done.status} ${await done.clone().text()}`);
u = await usage();
check("usage counts the new clip", u.clips === 1 && u.used >= video.length, JSON.stringify(u));

metaGets = 0;
for (let i = 0; i < 5; i++) {
  const r = await worker.fetch(new Request(`https://clips.example/v/${start.id}.mp4`, { headers: { Range: "bytes=0-99" } }), env);
  if (i === 0) check("the video is served", r.status === 200 || r.status === 206, String(r.status));
}
check("five requests for the video read its meta once", metaGets === 1, `meta reads ${metaGets}`);

const del = await call("DELETE", `/api/clips/${start.id}`);
check("the clip is deleted", del.status === 200, String(del.status));
const gone = await worker.fetch(new Request(`https://clips.example/v/${start.id}.mp4`), env);
check("a deleted clip is not served", gone.status === 404, String(gone.status));
u = await usage();
check("usage is empty again", u.used === 0 && u.clips === 0, JSON.stringify(u));

console.log(failed ? `${failed} failed` : "all passed");
process.exit(failed ? 1 : 0);
