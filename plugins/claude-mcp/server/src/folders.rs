//! Which folders Claude may read (and which it may also write into), with the person's note on each.
//! Kept in `config.json` beside glint-mcp.exe. Claude cannot grant itself a folder: only the person can,
//! with `glint-mcp.exe grant <folder> --note "..."` (or by editing config.json).
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub fn plugin_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("GLINT_MCP_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."))
}

fn config_path() -> PathBuf {
    plugin_dir().join("config.json")
}

pub fn load() -> Value {
    fs::read_to_string(config_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}))
}

pub fn save(cfg: &Value) -> Result<(), String> {
    fs::write(config_path(), serde_json::to_string_pretty(cfg).unwrap()).map_err(|e| format!("could not write {}: {e}", config_path().display()))
}

#[derive(Clone, Debug)]
pub struct Grant {
    pub path: PathBuf,
    pub note: String,
    pub write: bool,
}

pub fn grants() -> Vec<Grant> {
    let cfg = load();
    cfg["folders"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    Some(Grant { path: PathBuf::from(f["path"].as_str()?), note: f["note"].as_str().unwrap_or("").to_string(), write: f["write"].as_bool().unwrap_or(false) })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Add or update a folder grant (the command line, never a tool).
pub fn grant(path: &str, note: Option<&str>, write: bool) -> Result<String, String> {
    let p = fs::canonicalize(path).map_err(|e| format!("{path}: {e}"))?;
    if !p.is_dir() {
        return Err(format!("{path} is not a folder"));
    }
    let shown = display(&p);
    let mut cfg = load();
    let mut list: Vec<Value> = cfg["folders"].as_array().cloned().unwrap_or_default();
    list.retain(|f| f["path"].as_str().map(|s| !same(Path::new(s), &p)).unwrap_or(true));
    let old_note = grants().into_iter().find(|g| same(&g.path, &p)).map(|g| g.note).unwrap_or_default();
    list.push(json!({ "path": shown, "note": note.map(str::to_string).unwrap_or(old_note), "write": write }));
    cfg["folders"] = Value::Array(list);
    save(&cfg)?;
    Ok(format!("Claude can now {} {shown}", if write { "read and add files in" } else { "read" }))
}

pub fn revoke(path: &str) -> Result<String, String> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
    let mut cfg = load();
    let mut list: Vec<Value> = cfg["folders"].as_array().cloned().unwrap_or_default();
    let n = list.len();
    list.retain(|f| f["path"].as_str().map(|s| !same(Path::new(s), &target) && s != path).unwrap_or(true));
    if list.len() == n {
        return Err(format!("{path} was not shared"));
    }
    cfg["folders"] = Value::Array(list);
    save(&cfg)?;
    Ok(format!("Claude can no longer see {path}"))
}

/// A path without Windows' `\\?\` prefix.
pub fn display(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

fn canon(p: &Path) -> Option<PathBuf> {
    fs::canonicalize(p).ok()
}

fn same(a: &Path, b: &Path) -> bool {
    match (canon(a), canon(b)) {
        (Some(x), Some(y)) => x == y,
        _ => display(a).eq_ignore_ascii_case(&display(b)),
    }
}

fn inside(p: &Path, dir: &Path) -> bool {
    match (canon(p), canon(dir)) {
        (Some(x), Some(d)) => x.starts_with(d),
        _ => false,
    }
}

/// `path` may be read: it is in a shared folder, or in Glint's clips folder.
pub fn readable(path: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(path);
    if !p.exists() {
        return Err(format!("{path} does not exist"));
    }
    let clips = crate::glint::clips_dir();
    if (!clips.is_empty() && inside(&p, Path::new(&clips))) || grants().iter().any(|g| inside(&p, &g.path)) {
        return Ok(p);
    }
    Err(format!("{path} is not in a folder shared with Claude. The person can share it with: glint-mcp.exe grant \"<folder>\" --note \"what it holds\""))
}

/// Folder `dir` may be written into (a grant with write on).
pub fn writable_dir(dir: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(dir);
    if grants().iter().any(|g| g.write && (inside(&p, &g.path) || same(&p, &g.path))) {
        return Ok(p);
    }
    Err(format!("{dir} is not a folder Claude may write into. Shared folders are read-only unless the person shares them with --write."))
}

pub fn list() -> Value {
    json!(grants()
        .iter()
        .map(|g| json!({ "path": display(&g.path), "note": g.note, "can_write": g.write, "exists": g.path.is_dir() }))
        .collect::<Vec<_>>())
}

const MEDIA: &[&str] = &["mp4", "mkv", "mov", "webm", "avi", "m4v", "flv", "ts", "wmv", "mp3", "wav", "m4a", "aac", "ogg", "opus", "flac", "wma", "amr", "3gp", "weba"];

/// Files under `dir` (which must be readable), newest structure first by name. `recursive`, `kinds`: "media",
/// "documents" or "all".
pub fn files(dir: &str, recursive: bool, kinds: &str, limit: usize) -> Result<Value, String> {
    let root = readable(dir)?;
    if !root.is_dir() {
        return Err(format!("{dir} is not a folder"));
    }
    let docs = ["txt", "md", "pdf", "docx", "srt", "vtt", "csv", "json", "rtf"];
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    let mut total = 0usize;
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                if recursive {
                    stack.push(p);
                }
                continue;
            }
            let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            let keep = match kinds {
                "media" => MEDIA.contains(&ext.as_str()),
                "documents" => docs.contains(&ext.as_str()),
                _ => true,
            };
            if !keep {
                continue;
            }
            total += 1;
            if out.len() < limit {
                let modified = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
                out.push(json!({ "path": display(&p), "name": e.file_name().to_string_lossy(), "size": md.len(), "modified_unix": modified }));
            }
        }
    }
    Ok(json!({ "folder": display(&root), "total": total, "files": out }))
}

/// Copy or move `src` (readable) into `dest_dir` (writable) as `name` (default: the same name). Never overwrites.
pub fn copy(src: &str, dest_dir: &str, name: Option<&str>, mv: bool) -> Result<Value, String> {
    let s = readable(src)?;
    if mv {
        // Moving takes the file out of its folder: that folder must be writable too.
        writable_dir(&display(s.parent().unwrap_or(Path::new(""))))?;
    }
    let d = writable_dir(dest_dir)?;
    fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let file = name.map(str::to_string).unwrap_or_else(|| s.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    if file.is_empty() || file.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return Err(format!("{file:?} is not a usable file name"));
    }
    let to = d.join(&file);
    if to.exists() {
        return Err(format!("{} already exists; pick another name", display(&to)));
    }
    if mv {
        fs::rename(&s, &to).or_else(|_| fs::copy(&s, &to).and_then(|_| fs::remove_file(&s))).map_err(|e| e.to_string())?;
    } else {
        fs::copy(&s, &to).map_err(|e| e.to_string())?;
    }
    Ok(json!({ "from": display(&s), "to": display(&to), "moved": mv }))
}

pub fn save_text(dir: &str, name: &str, text: &str) -> Result<Value, String> {
    let d = writable_dir(dir)?;
    let ok_ext = [".txt", ".md", ".csv", ".json", ".srt"].iter().any(|x| name.to_lowercase().ends_with(x));
    if !ok_ext || name.contains(['/', '\\', ':']) {
        return Err("The name must be a plain file name ending in .txt, .md, .csv, .json or .srt".into());
    }
    let to = d.join(name);
    if to.exists() {
        return Err(format!("{} already exists; pick another name", display(&to)));
    }
    fs::write(&to, text).map_err(|e| e.to_string())?;
    Ok(json!({ "saved": display(&to) }))
}

/// The text of a script or notes file: txt, md, srt, vtt, csv, json, pdf, docx.
pub fn read_document(path: &str, max_chars: usize) -> Result<Value, String> {
    let p = readable(path)?;
    let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
    let text = match ext.as_str() {
        "pdf" => pdf_extract::extract_text(&p).map_err(|e| format!("could not read the PDF: {e}"))?,
        "docx" => docx_text(&p)?,
        _ => {
            let bytes = fs::read(&p).map_err(|e| e.to_string())?;
            String::from_utf8_lossy(&bytes).into_owned()
        }
    };
    let total = text.chars().count();
    let cut: String = text.chars().take(max_chars).collect();
    Ok(json!({ "path": display(&p), "characters": total, "truncated": total > max_chars, "text": cut }))
}

fn docx_text(p: &Path) -> Result<String, String> {
    use std::io::Read;
    let f = fs::File::open(p).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("not a Word file: {e}"))?;
    let mut xml = String::new();
    z.by_name("word/document.xml").map_err(|e| format!("not a Word file: {e}"))?.read_to_string(&mut xml).map_err(|e| e.to_string())?;
    // Paragraph ends become new lines; every tag goes; the five XML entities are undone.
    let xml = xml.replace("</w:p>", "\n").replace("<w:tab/>", "\t").replace("<w:br/>", "\n");
    let mut out = String::with_capacity(xml.len() / 3);
    let mut in_tag = false;
    for c in xml.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    Ok(out.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&"))
}
