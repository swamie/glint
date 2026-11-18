//! The tools Claude sees, and what each one does.
use crate::folders::{self, display};
use crate::glint;
use crate::media;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

type Content = Vec<Value>;

fn text(v: Value) -> Content {
    let s = if let Value::String(s) = &v { s.clone() } else { serde_json::to_string(&v).unwrap_or_default() };
    vec![json!({ "type": "text", "text": s })]
}
fn image(jpg: &[u8]) -> Value {
    json!({ "type": "image", "data": media::b64(jpg), "mimeType": "image/jpeg" })
}

fn s(desc: &str) -> Value { json!({ "type": "string", "description": desc }) }
fn n(desc: &str) -> Value { json!({ "type": "number", "description": desc }) }
fn i(desc: &str) -> Value { json!({ "type": "integer", "description": desc }) }
fn b(desc: &str) -> Value { json!({ "type": "boolean", "description": desc }) }
fn arr(item: Value, desc: &str) -> Value { json!({ "type": "array", "items": item, "description": desc }) }

struct Tool {
    name: &'static str,
    desc: &'static str,
    props: fn() -> Value,
    required: &'static [&'static str],
    edits: bool,
}

const TOOLS: &[Tool] = &[
    // ----- Glint and the library
    Tool { name: "glint_status", desc: "Is Glint running, what is open in the editor, and its timeline.", props: || json!({}), required: &[], edits: false },
    Tool { name: "list_clips", desc: "Clips in Glint's library, newest first: id, title, file path, length, size, game, folder.", props: || json!({ "query": s("Words to match in title, game, folder or path"), "limit": i("At most this many (default 50)") }), required: &[], edits: false },
    Tool { name: "list_projects", desc: "Glint's editor projects and the clips in each.", props: || json!({}), required: &[], edits: false },
    Tool { name: "import_files", desc: "Add video files to Glint's library (they stay where they are unless copy is true). Files must be in a shared folder. Returns the new clip ids.", props: || json!({ "paths": arr(json!({"type":"string"}), "Full file paths"), "copy": b("Copy into Glint's clips folder (default false)") }), required: &["paths"], edits: true },
    Tool { name: "create_project", desc: "Make a new editor project from library clips (laid end to end in this order) and open it in the editor.", props: || json!({ "name": s("Project name"), "clip_ids": arr(json!({"type":"string"}), "Library clip ids, in order") }), required: &["name", "clip_ids"], edits: true },
    Tool { name: "open_in_editor", desc: "Open a library clip or a project in the editor.", props: || json!({ "id": s("Clip or project id") }), required: &["id"], edits: true },
    Tool { name: "close_editor", desc: "Close what is open in the editor (a project keeps its edit).", props: || json!({}), required: &[], edits: true },
    // ----- the timeline
    Tool { name: "get_timeline", desc: "The open edit: sources (clips in it), pieces on the video row V1 in order (index, timeline start/end, which clip and which clip seconds it shows, speed), gaps, audio rows, added audio clips, text and other overlays, fades, result length.", props: || json!({}), required: &[], edits: false },
    Tool { name: "add_clips", desc: "Add library clips to the open edit (at the end, or at timeline second `at`). A single open clip becomes a project.", props: || json!({ "clip_ids": arr(json!({"type":"string"}), "Library clip ids"), "at": n("Timeline second to place them (default: the end)") }), required: &["clip_ids"], edits: true },
    Tool { name: "assemble_timeline", desc: "Best tool for splicing. Replace the whole video row with these clip ranges, in this order, back to back. Each segment names a clip in the open edit (source index or clip_id) and the range of THAT clip's own seconds to keep. One undo step.", props: || json!({ "segments": arr(json!({ "type": "object", "properties": { "source": i("Source index from get_timeline"), "clip_id": s("Or the clip id"), "from": n("Clip second where it starts"), "to": n("Clip second where it ends") }, "required": ["from", "to"] }), "Ranges in play order") }), required: &["segments"], edits: true },
    Tool { name: "split", desc: "Split the video row at these timeline seconds.", props: || json!({ "times": arr(json!({"type":"number"}), "Timeline seconds") }), required: &["times"], edits: true },
    Tool { name: "delete_pieces", desc: "Delete pieces from the video row. What follows closes up, unless leave_gap.", props: || json!({ "pieces": arr(json!({"type":"integer"}), "Piece indexes from get_timeline"), "leave_gap": b("Leave an empty stretch instead of closing up") }), required: &["pieces"], edits: true },
    Tool { name: "trim_piece", desc: "Move a piece's start and/or end to new timeline seconds. Inward trims (what follows closes up, unless leave_gap); outward brings back footage from its clip.", props: || json!({ "piece": i("Piece index"), "start": n("New start (timeline seconds)"), "end": n("New end (timeline seconds)"), "leave_gap": b("Leave an empty stretch where footage was trimmed away") }), required: &["piece"], edits: true },
    Tool { name: "move_piece", desc: "Reorder: put piece `piece` right before piece `to` (or after it).", props: || json!({ "piece": i("Piece to move"), "to": i("Piece to put it beside"), "after": b("After instead of before") }), required: &["piece", "to"], edits: true },
    Tool { name: "close_gaps", desc: "Close every empty stretch on the video row.", props: || json!({}), required: &[], edits: true },
    Tool { name: "set_piece_speed", desc: "Play one piece faster or slower (0.25 to 4, pitch kept).", props: || json!({ "piece": i("Piece index"), "speed": n("0.25 to 4") }), required: &["piece", "speed"], edits: true },
    Tool { name: "set_row_volume", desc: "Level or mute of a clip's own audio row (Desktop, Mic, Game ...).", props: || json!({ "row": s("Row id or title from get_timeline"), "volume": n("0 to 2 (1 = unchanged)"), "muted": b("Mute") }), required: &["row"], edits: true },
    Tool { name: "add_audio", desc: "Put an audio file (voice message, song, sound) on the timeline at timeline second `at`, on a new row or on an existing added row. The file must be in a shared folder.", props: || json!({ "path": s("Audio file path"), "at": n("Timeline second where it starts (default 0)"), "row": s("Added row id to put it on (default: a new row)") }), required: &["path"], edits: true },
    Tool { name: "edit_audio_clip", desc: "Change an added audio clip: move it (at, row), trim (start, end in timeline seconds), split at a timeline second, volume, mute, or remove.", props: || json!({ "id": s("Audio clip id from get_timeline"), "at": n("New start"), "row": s("Row id, or \"new\""), "start": n("Trim start to"), "end": n("Trim end to"), "split": n("Split at"), "volume": n("0 to 2"), "muted": b("Mute"), "remove": b("Remove it") }), required: &["id"], edits: true },
    Tool { name: "add_text", desc: "Put text on the picture from timeline second `from` to `to`. x, y: position as fractions of the picture (0.5, 0.85 = bottom middle).", props: || json!({ "text": s("The words"), "from": n("Start"), "to": n("End"), "x": n("0..1"), "y": n("0..1"), "size": n("Text height, % of the picture (2..20, default 7)"), "color": s("#rrggbb") }), required: &["text"], edits: true },
    Tool { name: "remove_overlay", desc: "Remove a text or other item on the picture.", props: || json!({ "id": s("Overlay id from get_timeline") }), required: &["id"], edits: true },
    Tool { name: "set_fades", desc: "Fade from black at the start and to black at the end (seconds, 0 = off).", props: || json!({ "fade_in": n("Seconds"), "fade_out": n("Seconds") }), required: &[], edits: true },
    Tool { name: "set_color", desc: "Color of some pieces (or all): brightness, contrast, saturation, gamma (1 = unchanged), sharpness 0..1, temperature -1..1, glow 0..1.", props: || json!({ "pieces": arr(json!({"type":"integer"}), "Piece indexes (default all)"), "brightness": n(""), "contrast": n(""), "saturation": n(""), "gamma": n(""), "sharpness": n(""), "temperature": n(""), "glow": n("") }), required: &[], edits: true },
    Tool { name: "undo", desc: "Undo the last edit.", props: || json!({}), required: &[], edits: true },
    Tool { name: "redo", desc: "Redo.", props: || json!({}), required: &[], edits: true },
    Tool { name: "seek", desc: "Move the editor's playhead to a timeline second (to show the person a spot).", props: || json!({ "time": n("Timeline second") }), required: &["time"], edits: true },
    Tool { name: "export_video", desc: "Export the edit as a new video in Glint's library. Waits until it is done (can take minutes) and returns the new file.", props: || json!({ "close_gaps": b("Close gaps first instead of refusing") }), required: &[], edits: true },
    // ----- seeing and hearing
    Tool { name: "media_info", desc: "Length, picture size, frame rate and audio tracks of a file.", props: || json!({ "path": s("File path") }), required: &["path"], edits: false },
    Tool { name: "get_frames", desc: "See the picture: frames of a file at clip seconds, or of the edit at timeline seconds.", props: || json!({ "path": s("File path (with times)"), "times": arr(json!({"type":"number"}), "Seconds in the file"), "timeline_times": arr(json!({"type":"number"}), "Or: timeline seconds of the open edit"), "width": i("Pixels wide (default 640, max 1280)") }), required: &[], edits: false },
    Tool { name: "contact_sheet", desc: "One picture with frames spread over a file (or a range of it), to scan a clip quickly. Returns the time of each frame, left to right, top to bottom.", props: || json!({ "path": s("File path"), "from": n("Start second (default 0)"), "to": n("End second (default the end)"), "count": i("Frames (2..36, default 12)"), "columns": i("Per row (default 4)") }), required: &["path"], edits: false },
    Tool { name: "audio_levels", desc: "Hear the loudness: dB over time and the silent stretches of a file's audio track. Good for finding pauses to cut at, dead air and loud moments.", props: || json!({ "path": s("File path"), "track": i("Audio track (default 0; Glint clips: 0 mix, 1 desktop, 2 mic)"), "from": n("Start second"), "to": n("End second"), "window": n("Seconds per level value"), "silence_db": n("Quieter than this is silence (default -40)"), "min_silence": n("Shortest silence, seconds (default 0.4)") }), required: &["path"], edits: false },
    Tool { name: "transcribe", desc: "Hear the speech: a timed transcript (seconds in the file) of a video or audio file, made on this computer with whisper.cpp. Cached.", props: || json!({ "path": s("File path"), "track": i("Audio track (default 0)"), "from": n("Start second"), "to": n("End second"), "language": s("Language code, e.g. en (default: detect)"), "words": b("One segment per word (for exact cuts)") }), required: &["path"], edits: false },
    Tool { name: "transcribe_timeline", desc: "The transcript of the open edit as it plays now, in timeline seconds, piece by piece.", props: || json!({ "track": i("Audio track of each clip (default 0)"), "language": s("Language code (default: detect)") }), required: &[], edits: false },
    // ----- folders the person shared
    Tool { name: "list_folders", desc: "The folders the person shared with Claude, each with their note on what it holds and whether Claude may write there.", props: || json!({}), required: &[], edits: false },
    Tool { name: "list_files", desc: "Files in a shared folder.", props: || json!({ "folder": s("Folder path"), "recursive": b("Include sub-folders"), "kind": s("media, documents or all (default all)"), "limit": i("At most (default 300)") }), required: &["folder"], edits: false },
    Tool { name: "read_document", desc: "The text of a script or notes file in a shared folder: txt, md, pdf, docx, srt, vtt, csv, json.", props: || json!({ "path": s("File path"), "max_chars": i("At most this many characters (default 60000)") }), required: &["path"], edits: false },
    Tool { name: "copy_file", desc: "Copy a file into a folder Claude may write in (to organize, e.g. name voice messages after script lines). Never overwrites.", props: || json!({ "path": s("Source file"), "to_folder": s("Destination folder (must be writable)"), "name": s("New file name (default: the same)") }), required: &["path", "to_folder"], edits: true },
    Tool { name: "move_file", desc: "Move or rename a file. Both folders must be writable. Never overwrites.", props: || json!({ "path": s("Source file"), "to_folder": s("Destination folder"), "name": s("New file name") }), required: &["path", "to_folder"], edits: true },
    Tool { name: "save_text", desc: "Save a text file (a matching report, notes, subtitles) into a writable folder. Never overwrites.", props: || json!({ "folder": s("Writable folder"), "name": s("File name ending .txt, .md, .csv, .json or .srt"), "text": s("Content") }), required: &["folder", "name", "text"], edits: true },
];

pub fn list() -> Value {
    json!(TOOLS
        .iter()
        .map(|t| json!({
            "name": t.name,
            "description": t.desc,
            "inputSchema": { "type": "object", "properties": (t.props)(), "required": t.required },
            "annotations": { "readOnlyHint": !t.edits, "destructiveHint": false, "openWorldHint": false },
        }))
        .collect::<Vec<_>>())
}

/* ---------- arguments ---------- */

fn st<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    a[k].as_str().filter(|s| !s.is_empty()).ok_or_else(|| format!("missing \"{k}\""))
}
fn f(a: &Value, k: &str) -> Option<f64> {
    a[k].as_f64()
}
/// Only the keys the plugin script knows, without nulls.
fn pick(a: &Value, keys: &[&str]) -> Value {
    let mut o = serde_json::Map::new();
    for k in keys {
        if let Some(v) = a.get(*k).filter(|v| !v.is_null()) {
            o.insert(k.to_string(), v.clone());
        }
    }
    Value::Object(o)
}

/// A file Claude named: in a shared folder or Glint's clips folder, or a file of Glint's library / the open edit.
fn file(path: &str) -> Result<PathBuf, String> {
    match folders::readable(path) {
        Ok(p) => Ok(p),
        Err(e) => {
            if !Path::new(path).is_file() {
                return Err(e);
            }
            let name = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let lib = glint::editor("clips", json!({ "query": name, "limit": 500 })).unwrap_or(Value::Null);
            let known = lib["clips"].as_array().is_some_and(|l| l.iter().any(|c| c["path"].as_str().is_some_and(|p| p.eq_ignore_ascii_case(path))));
            let open = glint::editor("state", json!({})).ok().is_some_and(|v| {
                v["timeline"]["sources"].as_array().is_some_and(|l| l.iter().any(|s| s["path"].as_str().is_some_and(|p| p.eq_ignore_ascii_case(path))))
                    || v["timeline"]["audio_clips"].as_array().is_some_and(|l| l.iter().any(|s| s["path"].as_str().is_some_and(|p| p.eq_ignore_ascii_case(path))))
            });
            if known || open { Ok(PathBuf::from(path)) } else { Err(e) }
        }
    }
}

/* ---------- calls ---------- */

pub fn call(name: &str, a: &Value) -> Result<Content, String> {
    let ed = |m: &str, keys: &[&str]| glint::editor(m, pick(a, keys)).map(text);
    match name {
        "glint_status" => {
            let hello = glint::call("glint.hello", json!({}), 5)?;
            let state = glint::editor("state", json!({})).unwrap_or(Value::Null);
            let (wc, wm) = media::whisper_paths();
            let stt = wc.is_file() && wm.is_file();
            Ok(text(json!({ "glint": hello, "editor": state, "shared_folders": folders::list(), "speech_to_text_ready": stt })))
        }
        "list_clips" => ed("clips", &["query", "limit"]),
        "list_projects" => ed("projects", &[]),
        "import_files" => {
            let paths: Vec<String> = a["paths"].as_array().ok_or("missing \"paths\"")?.iter().filter_map(|p| p.as_str().map(str::to_string)).collect();
            for p in &paths {
                folders::readable(p)?;
            }
            glint::call("claude.import", json!({ "paths": paths, "copy": a["copy"].as_bool().unwrap_or(false) }), 600).map(text)
        }
        "create_project" => ed("create_project", &["name", "clip_ids"]),
        "open_in_editor" => ed("open", &["id"]),
        "close_editor" => ed("close", &[]),
        "get_timeline" => ed("timeline", &[]),
        "add_clips" => ed("add_clips", &["clip_ids", "at"]),
        "assemble_timeline" => ed("assemble", &["segments"]),
        "split" => ed("split", &["times"]),
        "delete_pieces" => ed("delete", &["pieces", "leave_gap"]),
        "trim_piece" => ed("trim", &["piece", "start", "end", "leave_gap"]),
        "move_piece" => ed("move", &["piece", "to", "after"]),
        "close_gaps" => ed("close_gaps", &[]),
        "set_piece_speed" => ed("speed", &["piece", "speed"]),
        "set_row_volume" => ed("volume", &["row", "volume", "muted"]),
        "add_audio" => {
            let p = file(st(a, "path")?)?;
            let mut args = pick(a, &["at", "row"]);
            args["path"] = json!(display(&p));
            glint::editor("add_audio", args).map(text)
        }
        "edit_audio_clip" => ed("audio_clip", &["id", "at", "row", "start", "end", "split", "volume", "muted", "remove"]),
        "add_text" => ed("add_text", &["text", "from", "to", "x", "y", "size", "color", "outline"]),
        "remove_overlay" => ed("remove_overlay", &["id"]),
        "set_fades" => ed("fade", &["fade_in", "fade_out"]),
        "set_color" => ed("color", &["pieces", "brightness", "contrast", "saturation", "gamma", "sharpness", "temperature", "glow"]),
        "undo" => ed("undo", &[]),
        "redo" => ed("redo", &[]),
        "seek" => ed("seek", &["time"]),
        "export_video" => glint::call("claude.export", pick(a, &["close_gaps"]), 3600).map(text),

        "media_info" => media::info(&file(st(a, "path")?)?).map(text),
        "get_frames" => frames(a),
        "contact_sheet" => {
            let p = file(st(a, "path")?)?;
            let from = f(a, "from").unwrap_or(0.0);
            let to = f(a, "to").unwrap_or_else(|| media::duration(&p));
            if !(to > from) {
                return Err("\"to\" must be after \"from\" (or the file's length is unknown)".into());
            }
            let (jpg, times) = media::sheet(&p, from, to, a["count"].as_u64().unwrap_or(12) as usize, a["columns"].as_u64().unwrap_or(4) as usize)?;
            Ok(vec![json!({ "type": "text", "text": format!("Frames of {} at seconds {:?}", display(&p), times) }), image(&jpg)])
        }
        "audio_levels" => {
            let p = file(st(a, "path")?)?;
            media::levels(&p, a["track"].as_u64().unwrap_or(0) as u32, f(a, "from").unwrap_or(0.0), f(a, "to"), f(a, "window"), f(a, "silence_db").unwrap_or(-40.0), f(a, "min_silence").unwrap_or(0.4)).map(text)
        }
        "transcribe" => {
            let p = file(st(a, "path")?)?;
            media::transcribe(&p, a["track"].as_u64().unwrap_or(0) as u32, f(a, "from").unwrap_or(0.0), f(a, "to"), a["language"].as_str().unwrap_or(""), a["words"].as_bool().unwrap_or(false)).map(text)
        }
        "transcribe_timeline" => transcribe_timeline(a),

        "list_folders" => Ok(text(json!({ "folders": folders::list(), "how_to_share": "The person shares a folder with: glint-mcp.exe grant \"<folder>\" --note \"what it holds\" (add --write to let Claude add files there)." }))),
        "list_files" => folders::files(st(a, "folder")?, a["recursive"].as_bool().unwrap_or(false), a["kind"].as_str().unwrap_or("all"), a["limit"].as_u64().unwrap_or(300) as usize).map(text),
        "read_document" => folders::read_document(st(a, "path")?, a["max_chars"].as_u64().unwrap_or(60_000) as usize).map(text),
        "copy_file" => folders::copy(st(a, "path")?, st(a, "to_folder")?, a["name"].as_str(), false).map(text),
        "move_file" => folders::copy(st(a, "path")?, st(a, "to_folder")?, a["name"].as_str(), true).map(text),
        "save_text" => folders::save_text(st(a, "folder")?, st(a, "name")?, st(a, "text")?).map(text),
        _ => Err(format!("unknown tool {name}")),
    }
}

fn frames(a: &Value) -> Result<Content, String> {
    let width = a["width"].as_u64().unwrap_or(640).clamp(64, 1280) as u32;
    let mut out = Vec::new();
    let mut list: Vec<(PathBuf, f64, String)> = Vec::new();
    if let Some(tt) = a["timeline_times"].as_array().filter(|x| !x.is_empty()) {
        let res = glint::editor("resolve", json!({ "times": tt }))?;
        for r in res.as_array().cloned().unwrap_or_default() {
            let t = r["time"].as_f64().unwrap_or(0.0);
            if r["gap"] == json!(true) {
                out.push(json!({ "type": "text", "text": format!("timeline {t:.2} s: a gap (black)") }));
                continue;
            }
            let p = r["path"].as_str().unwrap_or("").to_string();
            let ct = r["clip_time"].as_f64().unwrap_or(0.0);
            list.push((PathBuf::from(&p), ct, format!("timeline {t:.2} s = {} at {ct:.2} s", Path::new(&p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())));
        }
    } else {
        let p = file(st(a, "path")?)?;
        let times = a["times"].as_array().ok_or("give \"times\" with \"path\", or \"timeline_times\"")?;
        for t in times.iter().filter_map(Value::as_f64) {
            list.push((p.clone(), t, format!("{} at {t:.2} s", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())));
        }
    }
    if list.len() > 12 {
        return Err("At most 12 frames at once (use contact_sheet for an overview)".into());
    }
    for (p, t, label) in list {
        match media::frame(&p, t, width) {
            Ok(jpg) => {
                out.push(json!({ "type": "text", "text": label }));
                out.push(image(&jpg));
            }
            Err(e) => out.push(json!({ "type": "text", "text": format!("{label}: {e}") })),
        }
    }
    Ok(out)
}

fn transcribe_timeline(a: &Value) -> Result<Content, String> {
    let tl = glint::editor("timeline", json!({}))?;
    let sources = tl["sources"].as_array().cloned().unwrap_or_default();
    let track = a["track"].as_u64().unwrap_or(0) as u32;
    let lang = a["language"].as_str().unwrap_or("");
    let mut lines = Vec::new();
    let mut cache: std::collections::HashMap<usize, Value> = Default::default();
    for p in tl["pieces"].as_array().cloned().unwrap_or_default() {
        if p["gap"] == json!(true) {
            continue;
        }
        let k = p["source"].as_u64().unwrap_or(0) as usize;
        let path = sources.get(k).and_then(|s| s["path"].as_str()).unwrap_or("");
        if path.is_empty() {
            continue;
        }
        if !cache.contains_key(&k) {
            cache.insert(k, media::transcribe(Path::new(path), track, 0.0, None, lang, false)?);
        }
        let (cf, ct) = (p["clip_from"].as_f64().unwrap_or(0.0), p["clip_to"].as_f64().unwrap_or(0.0));
        let start = p["start"].as_f64().unwrap_or(0.0);
        let speed = p["speed"].as_f64().unwrap_or(1.0);
        for seg in cache[&k]["segments"].as_array().cloned().unwrap_or_default() {
            let (a0, b0) = (seg["start"].as_f64().unwrap_or(0.0), seg["end"].as_f64().unwrap_or(0.0));
            if b0 <= cf || a0 >= ct {
                continue;
            }
            let map = |x: f64| ((start + (x.clamp(cf, ct) - cf) / speed) * 100.0).round() / 100.0;
            lines.push(json!({ "piece": p["index"], "start": map(a0), "end": map(b0), "text": seg["text"], "cut_off": a0 < cf || b0 > ct }));
        }
    }
    Ok(text(json!({ "timeline_length": tl["timeline_length"], "segments": lines })))
}
