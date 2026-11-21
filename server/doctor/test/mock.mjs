#!/usr/bin/env node
// Stand-ins for the three services the Worker talks to, for test/smoke.sh:
//   POST /v1/messages                 the Claude Messages API (streams one fixed report; from the second
//                                     call on, after a mid-answer `fallback` block, as a declined request
//                                     that the fallback model finished)
//   GET  /raw/<ref>/src-tauri/src/*   raw.githubusercontent.com (tag v9.9.9 exists; any other tag does not)
//   POST /discord                     a Discord webhook
//   GET  /seen                        what arrived, for the test to check
// Nothing here calls the real API, so the smoke test costs nothing.
import { createServer } from "node:http";

const PORT = Number(process.argv[2]) || 8798;
const seen = { claude: [], discord: [], raw: [] };

const report = {
  user_summary: "Glint could not start recording because the graphics driver refused the encoder. Updating the graphics driver should help. See https://evil.example/x for more.",
  developer_summary: "NVENC session open fails; the fallback to x264 never runs.",
  severity: "broken_feature",
  findings: [{ title: "NVENC open failure is not retried on x264", category: "glint_bug", confidence: "medium", evidence: ["2026-11-18 10:00:01.000 capture 1: ffmpeg did not start (exit 0x1)"], explanation: "The restart loop keeps the same encoder." }],
  patches: [{ finding: "NVENC open failure is not retried on x264", file: "src-tauri/src/recorder.rs", function: "run_capture", change: "After two failed starts, switch the encoder to x264.", code: "", source_seen: false, risk: "Low.", verify: "Run with an old driver." }],
  workarounds: ["Set Settings, Encoder to x264."],
  missing_information: [],
};

createServer((req, res) => {
  let body = [];
  req.on("data", (c) => body.push(c));
  req.on("end", () => {
    body = Buffer.concat(body).toString("utf8");
    const url = new URL(req.url, "http://x");
    if (req.method === "POST" && url.pathname === "/v1/messages") {
      const sent = JSON.parse(body);
      seen.claude.push({ key: req.headers["x-api-key"], version: req.headers["anthropic-version"], beta: req.headers["anthropic-beta"], model: sent.model, max_tokens: sent.max_tokens, thinking: sent.thinking?.type, effort: sent.output_config?.effort, format: sent.output_config?.format?.type, schema: !!sent.output_config?.format?.schema, fallbacks: sent.fallbacks, stream: sent.stream, sampling: ["temperature", "top_p", "top_k"].some((k) => k in sent), system_chars: sent.system.length, prompt: sent.messages[0].content });
      const fallback = seen.claude.length > 1;
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      const send = (type, data) => res.write(`event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`);
      send("message_start", { message: { id: "msg_test", model: fallback ? "claude-opus-4-8" : sent.model, usage: { input_tokens: 1234 } } });
      send("content_block_start", { index: 0, content_block: { type: "thinking", thinking: "" } });
      send("content_block_stop", { index: 0 });
      let at = 1;
      if (fallback) {
        // Part of an answer, then the switch to the fallback model, which answers in full.
        send("content_block_start", { index: at, content_block: { type: "text", text: "" } });
        send("content_block_delta", { index: at, delta: { type: "text_delta", text: '{"user_summary":"half an ans' } });
        send("content_block_stop", { index: at++ });
        send("content_block_start", { index: at, content_block: { type: "fallback", from: { model: sent.model }, to: { model: "claude-opus-4-8" } } });
        send("content_block_stop", { index: at++ });
      }
      send("content_block_start", { index: at, content_block: { type: "text", text: "" } });
      const text = JSON.stringify(report);
      for (let i = 0; i < text.length; i += 37) send("content_block_delta", { index: at, delta: { type: "text_delta", text: text.slice(i, i + 37) } });
      send("content_block_stop", { index: at });
      send("message_delta", { delta: { stop_reason: "end_turn" }, usage: { output_tokens: 567 } });
      send("message_stop", {});
      return res.end();
    }
    if (req.method === "GET" && url.pathname.startsWith("/raw/")) {
      seen.raw.push(url.pathname);
      const [, , ref, , , file] = url.pathname.split("/");
      if (ref !== "v9.9.9" && ref !== "main") return res.writeHead(404).end("404");
      res.writeHead(200, { "Content-Type": "text/plain" });
      return res.end(`//! ${file} at ${ref}\nfn run_capture() {\n    util::log(&format!("capture {mon}: ffmpeg did not start (exit {})", 1));\n}\n`);
    }
    if (req.method === "POST" && url.pathname === "/discord") {
      seen.discord.push(body);
      return res.writeHead(204).end();
    }
    if (url.pathname === "/seen") {
      res.writeHead(200, { "Content-Type": "application/json" });
      return res.end(JSON.stringify(seen));
    }
    res.writeHead(404).end();
  });
}).listen(PORT, "127.0.0.1", () => console.log(`mock services on http://127.0.0.1:${PORT}`));
