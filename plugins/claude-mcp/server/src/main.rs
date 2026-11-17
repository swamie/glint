//! glint-mcp: the Glint plugin that lets Claude edit video in Glint's editor, over MCP (stdio).
//!
//!   glint-mcp                         the MCP server (Claude Desktop / Claude Code start it)
//!   glint-mcp grant <folder> [--note "what it holds"] [--write]
//!   glint-mcp revoke <folder>
//!   glint-mcp folders
//!   glint-mcp setup-transcripts [model]   whisper.cpp + a model (default base.en)
//!   glint-mcp install-claude              add this server to Claude Desktop's config
mod folders;
mod glint;
mod media;
mod tools;

use serde_json::{json, Value};
use std::io::{BufRead, Write};

const PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Glint is the person's video clipper and editor on Windows. These tools drive Glint's editor live: the person sees every change on the timeline and can undo it there.\n\
Typical splice: list_clips (or list_folders + list_files, then import_files) -> create_project with the clips -> get_timeline -> listen and look (transcribe, audio_levels, get_frames, contact_sheet) -> assemble_timeline with the ranges to keep, or split / delete_pieces / trim_piece / move_piece -> export_video.\n\
Times: 'timeline' seconds are positions on the edit's video row (V1). 'clip' seconds (from, to, clip_from, clip_to, transcript times) are positions inside one source file. get_timeline maps between them for each piece.\n\
You cannot hear audio directly: use transcribe / transcribe_timeline for speech, audio_levels for loudness and silences, and get_frames / contact_sheet to see the picture.\n\
Folders: only folders the person shared are readable (list_folders shows each with the person's note on what it holds). They are read-only unless marked can_write; copy_file / move_file / save_text work only there. Never delete anything.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(cmd) = args.first() {
        std::process::exit(cli(cmd, &args[1..]));
    }
    serve();
    media::cleanup();
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn cli(cmd: &str, args: &[String]) -> i32 {
    let res = match cmd {
        "grant" => match args.first() {
            Some(p) if !p.starts_with("--") => folders::grant(p, flag(args, "--note").as_deref(), args.iter().any(|a| a == "--write")),
            _ => Err("usage: glint-mcp grant <folder> [--note \"what it holds\"] [--write]".into()),
        },
        "revoke" => match args.first() {
            Some(p) => folders::revoke(p),
            None => Err("usage: glint-mcp revoke <folder>".into()),
        },
        "folders" => Ok(serde_json::to_string_pretty(&folders::list()).unwrap()),
        "setup-transcripts" => media::setup_whisper(args.first().map(String::as_str).unwrap_or("base.en")),
        "install-claude" => install_claude(),
        "--version" | "-V" => Ok(env!("CARGO_PKG_VERSION").to_string()),
        _ => Err(format!(
            "glint-mcp {}: Claude in Glint's editor.\n\nRun with no arguments as an MCP server, or:\n  grant <folder> [--note \"...\"] [--write]\n  revoke <folder>\n  folders\n  setup-transcripts [model]\n  install-claude",
            env!("CARGO_PKG_VERSION")
        )),
    };
    match res {
        Ok(s) => {
            println!("{s}");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn install_claude() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = folders::display(&exe);
    let path = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("Claude").join("claude_desktop_config.json");
    let mut cfg: Value = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    if !cfg["mcpServers"].is_object() {
        cfg["mcpServers"] = json!({});
    }
    cfg["mcpServers"]["glint"] = json!({ "command": exe, "args": [] });
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(format!("Added \"glint\" to {}. Restart Claude Desktop.\nFor Claude Code instead: claude mcp add glint -- \"{exe}\"", path.display()))
}

/* ---------- MCP over stdio: one JSON-RPC message per line ---------- */

fn serve() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                send(&mut out, json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("parse error: {e}") } }));
                continue;
            }
        };
        // A batch (older clients) is answered item by item.
        let list = if let Value::Array(a) = msg { a } else { vec![msg] };
        for m in list {
            if let Some(reply) = handle(&m) {
                send(&mut out, reply);
            }
        }
    }
}

fn send(out: &mut impl Write, v: Value) {
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn handle(m: &Value) -> Option<Value> {
    let id = m.get("id").cloned()?; // a notification: no answer
    let method = m["method"].as_str().unwrap_or("");
    let params = &m["params"];
    let result = match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or("");
            let ver = if PROTOCOLS.contains(&asked) { asked } else { PROTOCOLS[0] };
            Ok(json!({
                "protocolVersion": ver,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "glint", "title": "Glint editor", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools::list() })),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            Ok(match tools::call(name, &args) {
                Ok(content) => json!({ "content": content }),
                Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
            })
        }
        "resources/list" => Ok(json!({ "resources": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        _ => Err(json!({ "code": -32601, "message": format!("unknown method {method}") })),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e }),
    })
}
