//! Taking personal data out of everything before it leaves the PC.
//!
//! The first rule is Glint's own (`sanitize_with` is copied from
//! src-tauri/src/bugreport.rs): every occurrence of the user's profile folder
//! becomes `~`. On top of that, in this order:
//!   - Discord webhook addresses are removed whole;
//!   - other people's profile folders (`C:\Users\<someone>`) lose the name;
//!   - the names of clips, pictures and sound files are replaced by `<name>`;
//!     the folders they are in and the file type stay;
//!   - web addresses keep only a known host (GitHub, Discord, ...), never the
//!     query; any other host is hidden;
//!   - e-mail addresses, IP addresses, the PC's name, network share hosts;
//!   - the Windows user name as a word of its own becomes `~`;
//!   - anything that looks like a key or token.
//!
//! Settings get one more pass (`Scrubber::settings`): values are dropped by
//! the NAME of the setting (tokens, keys, webhooks, share-link fields) and the
//! large or personal ones (custom CSS, presets, folder colors) are reduced to
//! a count.
//!
//! This is best effort on free text. The server runs a second pass
//! (server/doctor/src/scrub.js) before anything is stored or sent on.
use regex::{Captures, Regex};
use serde_json::Value;

/// Names that are too ordinary to replace as a bare word: a Windows user
/// called "Glint" must not turn every "glint" in the log into "~".
const ORDINARY: [&str; 34] = [
    "glint", "user", "users", "admin", "administrator", "owner", "guest", "home", "pc", "game", "games", "gaming", "video", "videos", "clip", "clips", "windows", "desktop", "public", "default", "test", "error", "audio", "export", "capture", "monitor",
    "nvidia", "intel", "amd", "steam", "obs", "blur", "ffmpeg", "system",
];

/// Hosts that may stay in a web address: Glint's own downloads and services.
const KNOWN_HOSTS: [&str; 9] = ["github.com", "api.github.com", "raw.githubusercontent.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com", "discord.com", "www.gyan.dev", "127.0.0.1", "localhost"];

/// Settings that are large or personal and tell nothing about a fault: only how many there are is sent.
const COUNT_ONLY: [&str; 12] = ["custom_css", "theme_presets", "theme_colors", "theme_order", "folder_colors", "color_presets", "canvas_presets", "rules", "clip_name", "card_tags", "editor_keys", "css"];

/// A word in a setting's name that marks its value as a credential or as personal.
const SECRET_WORDS: [&str; 17] = ["token", "secret", "password", "passwd", "pass", "webhook", "hook", "credential", "credentials", "cookie", "auth", "apikey", "uploader", "username", "email", "contact", "license"];

pub struct Scrubber {
    profile: String,
    user: Option<Regex>,
    host: Option<Regex>,
    webhook: Regex,
    other_profile: Regex,
    media: Regex,
    url: Regex,
    email: Regex,
    ipv4: Regex,
    ipv6: Regex,
    unc: Regex,
    labelled: Regex,
    long_hex: Regex,
    long_token: Regex,
}

fn word_regex(word: &str) -> Option<Regex> {
    let w = word.trim();
    if w.chars().count() < 3 || ORDINARY.contains(&w.to_lowercase().as_str()) {
        return None;
    }
    Regex::new(&format!("(?i){}", regex::escape(w))).ok()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Replace every match of `re` that stands as a word of its own.
fn replace_word(text: &str, re: &Regex, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for m in re.find_iter(text) {
        let before = text[..m.start()].chars().next_back();
        let after = text[m.end()..].chars().next();
        if before.is_some_and(is_word) || after.is_some_and(is_word) {
            continue;
        }
        out.push_str(&text[last..m.start()]);
        out.push_str(with);
        last = m.end();
    }
    out.push_str(&text[last..]);
    out
}

impl Scrubber {
    /// From this PC: `USERPROFILE`, `USERNAME`, `COMPUTERNAME`.
    pub fn from_env() -> Self {
        let var = |n: &str| std::env::var(n).unwrap_or_default();
        Self::new(&var("USERPROFILE"), &var("USERNAME"), &var("COMPUTERNAME"))
    }

    pub fn new(profile: &str, user: &str, host: &str) -> Self {
        let re = |p: &str| Regex::new(p).expect("a fixed pattern");
        Scrubber {
            profile: profile.to_string(),
            user: word_regex(user),
            host: word_regex(host),
            webhook: re(r#"(?i)https?://[a-z0-9.-]*discord(?:app)?\.com/api/webhooks/[^\s"'<>]*"#),
            other_profile: re(r#"(?i)\b([a-z]:[\\/]+(?:Users|Documents and Settings)[\\/]+)([^\\/:*?"<>|\r\n]+)"#),
            media: re(r"(?i)\.(mp4|mkv|mov|webm|avi|m4v|gif|png|jpe?g|webp|bmp|wav|mp3|m4a|aac|flac|ogg|opus)\b"),
            url: re(r#"(?i)\b(https?|wss?|ftp)://([^\s"'<>/?#]+)([^\s"'<>]*)"#),
            email: re(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}"),
            ipv4: re(r"\b(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\b"),
            ipv6: re(r"(?i)\b(?:[0-9a-f]{1,4}:){4,7}[0-9a-f]{1,4}\b|\b(?:[0-9a-f]{1,4}:){1,6}:[0-9a-f]{1,4}(?::[0-9a-f]{1,4})*\b"),
            unc: re(r#"(^|[\s"'=(,\[])(\\{2,4})([A-Za-z0-9._$-]{2,})(\\)"#),
            labelled: re(r#"(?i)\b(bearer|token|secret|password|passwd|api[_-]?key|upload[_-]?key|authorization)\b(["']?\s*[:=]\s*["']?|\s+)([A-Za-z0-9._~+/=-]{8,})"#),
            long_hex: re(r"\b[0-9a-fA-F]{32,}\b"),
            long_token: re(r"\b[A-Za-z0-9_-]{40,}\b"),
        }
    }

    /// Free text: a log, an error message, a path.
    pub fn text(&self, text: &str) -> String {
        let t = self.webhook.replace_all(text, "<webhook removed>").into_owned();
        let t = sanitize_with(&t, &self.profile);
        let t = self.other_profile.replace_all(&t, |c: &Captures| {
            let name = c[2].trim();
            if ["public", "default", "all users", "default user"].contains(&name.to_lowercase().as_str()) || name == "<user>" {
                c[0].to_string()
            } else {
                format!("{}<user>", &c[1])
            }
        });
        let t = self.hide_media_names(&t);
        let t = self.url.replace_all(&t, |c: &Captures| {
            // Never the "user:password@" part, never the query.
            let host = c[2].rsplit('@').next().unwrap_or("").to_lowercase();
            let name = host.split(':').next().unwrap_or("");
            if KNOWN_HOSTS.contains(&name) {
                let path = c[3].split(['?', '#']).next().unwrap_or("");
                format!("{}://{}{}", &c[1], host, path)
            } else {
                format!("{}://<host>", &c[1])
            }
        });
        let t = self.email.replace_all(&t, "<email>");
        let t = self.ipv4.replace_all(&t, |c: &Captures| {
            let m = c.get(0).unwrap();
            let octets = (1..=4).all(|i| c[i].parse::<u32>().is_ok_and(|n| n <= 255));
            // "1.2.3.4.5" and "v1.2.3.4" are version numbers.
            let before = t[..m.start()].chars().next_back();
            let longer = before == Some('.') || before.is_some_and(|b| b == 'v' || b == 'V') || t[m.end()..].starts_with('.') && t[m.end() + 1..].chars().next().is_some_and(|d| d.is_ascii_digit());
            if !octets || longer || ["127.0.0.1", "0.0.0.0"].contains(&m.as_str()) {
                m.as_str().to_string()
            } else {
                "<ip>".to_string()
            }
        });
        let t = self.ipv6.replace_all(&t, "<ip>");
        let t = self.unc.replace_all(&t, |c: &Captures| format!("{}{}<host>{}", &c[1], &c[2], &c[4]));
        let mut t = t.into_owned();
        if let Some(host) = &self.host {
            t = replace_word(&t, host, "<pc>");
        }
        if let Some(user) = &self.user {
            t = replace_word(&t, user, "~");
        }
        let t = self.labelled.replace_all(&t, |c: &Captures| format!("{}{}<removed>", &c[1], &c[2]));
        let t = self.long_hex.replace_all(&t, "<hex>");
        self.long_token.replace_all(&t, "<token>").into_owned()
    }

    /// `...\Valorant\Bob rages 2026-11-18.mp4` becomes `...\Valorant\<name>.mp4`.
    /// The name runs back from the file type to the nearest `\`, `/`, quote or
    /// colon (none of them can be in a Windows file name), so a bare name after
    /// "saved: " goes whole, at the cost of a few words of the message. Without
    /// one of those on the line it is the last word only.
    fn hide_media_names(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for m in self.media.find_iter(text) {
            let mut start = None;
            let mut word_start = last;
            let mut seen_space = false;
            for (i, c) in text[last..m.start()].char_indices().rev() {
                let at = last + i;
                if c == '\n' || c == '\r' {
                    break;
                }
                if matches!(c, '\\' | '/' | '"' | '\'' | ':' | '<' | '>' | '|' | '*' | '?') {
                    start = Some(at + c.len_utf8());
                    break;
                }
                if c.is_whitespace() && !seen_space {
                    seen_space = true;
                    word_start = at + c.len_utf8();
                }
            }
            let start = start.unwrap_or(word_start);
            let stem = &text[start..m.start()];
            let lead = stem.len() - stem.trim_start().len();
            if stem.trim().is_empty() {
                continue;
            }
            out.push_str(&text[last..start + lead]);
            out.push_str("<name>");
            last = m.start();
        }
        out.push_str(&text[last..]);
        out
    }

    /// Every string in a JSON value, through `text`.
    pub fn json(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.text(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.json(x)).collect()),
            Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), self.json(x))).collect()),
            other => other.clone(),
        }
    }

    /// Glint's settings.json: values dropped by the setting's name, the rest through `text`.
    pub fn settings(&self, v: &Value) -> Value {
        match v {
            Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), self.setting(k, x))).collect()),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.settings(x)).collect()),
            Value::String(s) => Value::String(self.setting_text(s)),
            other => other.clone(),
        }
    }

    fn setting_text(&self, s: &str) -> String {
        let l = s.trim_start().to_lowercase();
        if l.starts_with("http://") || l.starts_with("https://") {
            return "<an address is set; removed>".into();
        }
        self.text(s)
    }

    fn setting(&self, key: &str, v: &Value) -> Value {
        let k = key.to_lowercase();
        let words: Vec<&str> = k.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
        let secret = words.iter().any(|w| SECRET_WORDS.contains(w)) || k.ends_with("_key") || k.starts_with("key_") || k.starts_with("share");
        if secret {
            let empty = match v {
                Value::Null => true,
                Value::String(s) => s.trim().is_empty(),
                Value::Array(a) => a.is_empty(),
                Value::Object(o) => o.is_empty(),
                Value::Bool(_) | Value::Number(_) => return v.clone(),
            };
            return Value::String(if empty { String::new() } else { "<set; removed>".into() });
        }
        if COUNT_ONLY.contains(&k.as_str()) {
            return Value::String(match v {
                Value::String(s) => format!("<{} characters>", s.chars().count()),
                Value::Array(a) => format!("<{} items>", a.len()),
                Value::Object(o) => format!("<{} items>", o.len()),
                _ => "<set>".into(),
            });
        }
        self.settings(v)
    }
}

/// Replace every occurrence of the user's profile folder with `~`. Letter case
/// is ignored and `\`, `/` and doubled separators (as in JSON) all match.
/// Copied from src-tauri/src/bugreport.rs, so both tools hide the same thing.
pub fn sanitize_with(text: &str, profile: &str) -> String {
    let pat: Vec<char> = profile.trim_end_matches(['\\', '/']).chars().collect();
    if pat.len() < 3 {
        return text.to_string();
    }
    let is_sep = |c: char| c == '\\' || c == '/';
    let same = |a: char, b: char| a == b || a.to_lowercase().eq(b.to_lowercase());
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        // Try to match the profile path at i.
        let (mut j, mut k) = (i, 0);
        while k < pat.len() && j < chars.len() {
            if is_sep(pat[k]) {
                if !is_sep(chars[j]) {
                    break;
                }
                while j < chars.len() && is_sep(chars[j]) {
                    j += 1;
                }
            } else if same(pat[k], chars[j]) {
                j += 1;
            } else {
                break;
            }
            k += 1;
        }
        // Only a whole folder name: C:\Users\v must not eat the start of C:\Users\vera.
        let ends = chars.get(j).map_or(true, |c| !(c.is_alphanumeric() || *c == '_' || *c == '-'));
        if k == pat.len() && ends {
            out.push('~');
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn s() -> Scrubber {
        Scrubber::new(r"C:\Users\Vera Example", "Vera Example", "VERA-PC")
    }

    #[test]
    fn profile_and_user() {
        let t = s().text(r#"saved "C:\\Users\\Vera Example\\Videos\\Glint\\x" for c:/users/vera example/AppData and D:\Users\bob\x; owner Vera Example; host VERA-PC"#);
        assert!(!t.to_lowercase().contains("vera"), "{t}");
        assert!(!t.contains("bob"), "{t}");
        assert!(t.contains(r"D:\Users\<user>\x"), "{t}");
        assert!(t.contains(r#""~\\Videos\\Glint\\x""#), "{t}");
    }

    #[test]
    fn an_ordinary_user_name_is_left_alone() {
        let t = Scrubber::new(r"C:\Users\Glint", "Glint", "PC").text(r"Glint 0.1.48 at C:\Users\Glint\AppData");
        assert_eq!(t, r"Glint 0.1.48 at ~\AppData");
    }

    #[test]
    fn clip_names() {
        let sc = s();
        assert_eq!(sc.text(r"export: wrote ~\Videos\Glint\Valorant\Bob rages at me 2026-11-18.mp4 (12 MB)"), r"export: wrote ~\Videos\Glint\Valorant\<name>.mp4 (12 MB)");
        assert_eq!(sc.text(r#"clip saved: "My wedding.mkv""#), r#"clip saved: "<name>.mkv""#);
        // A bare name with no folder: everything back to the colon goes, rather than risk leaving half a name.
        assert_eq!(sc.text("thumbnail: could not read secret plan.PNG: denied"), "thumbnail: <name>.PNG: denied");
        assert_eq!(sc.text("could not read secret plan.PNG"), "could not read secret <name>.PNG");
        assert_eq!(sc.text("no media here, version 1.mp40"), "no media here, version 1.mp40");
        assert_eq!(sc.text(r"D:\a b\c d.mp4 and D:\e\f.jpg"), r"D:\a b\<name>.mp4 and D:\e\<name>.jpg");
    }

    #[test]
    fn network_things() {
        let t = s().text("posted to https://discord.com/api/webhooks/123/abcDEF then GET https://api.github.com/repos/swamie/glint/releases/latest?x=1 and https://my.nas.example/share?k=v from 192.168.1.20 or fe80::1ff:fe23:4567:890a mail a.b@example.co.uk; ffmpeg 7.1.0.0.1, listen 127.0.0.1:8080");
        assert!(t.contains("<webhook removed>") && !t.contains("abcDEF"), "{t}");
        assert!(t.contains("https://api.github.com/repos/swamie/glint/releases/latest ") && !t.contains("x=1"), "{t}");
        assert!(t.contains("https://<host> ") && !t.contains("nas"), "{t}");
        assert!(!t.contains("192.168") && !t.contains("fe80") && !t.contains("example.co"), "{t}");
        assert!(t.contains("7.1.0.0.1") && t.contains("127.0.0.1:8080"), "{t}");
    }

    #[test]
    fn times_and_versions_are_not_addresses() {
        let line = "2026-11-18 12:34:56.789 capture: 2560x1440 at 60 fps, driver 32.0.15.6094, build 10.0.19045.5131, std::fs::read";
        assert_eq!(s().text(line), line);
    }

    #[test]
    fn secrets_in_text() {
        let t = s().text(r#"Authorization: Bearer abcdefgh12345678 token="zzzzzzzzzzzz" id 0123456789abcdef0123456789abcdef0 \\NAS01\clips\x"#);
        assert!(!t.contains("abcdefgh12345678") && !t.contains("zzzzzzzz") && !t.contains("0123456789abcdef"), "{t}");
        assert!(t.contains(r"\\<host>\clips"), "{t}");
    }

    #[test]
    fn settings_by_name() {
        let v = s().settings(&json!({
            "clips_dir": r"C:\Users\Vera Example\Videos\Glint",
            "share_url": "https://glint-share.vera.workers.dev", "share_key": "k", "share_name": "Vera", "share_enabled": true,
            "upload_token": "abc", "bug_webhook": "", "custom_css": "body{}", "theme_presets": [1, 2],
            "keybinds": [{ "key": "F9", "action": "clip", "seconds": 30 }],
            "ffmpeg_path": "", "encoder": "auto", "polls_address": "https://polls.example.org",
        }));
        assert_eq!(v["clips_dir"], r"~\Videos\Glint");
        assert_eq!(v["share_url"], "<set; removed>");
        assert_eq!(v["share_key"], "<set; removed>");
        assert_eq!(v["share_name"], "<set; removed>");
        assert_eq!(v["share_enabled"], true);
        assert_eq!(v["upload_token"], "<set; removed>");
        assert_eq!(v["bug_webhook"], "");
        assert_eq!(v["custom_css"], "<6 characters>");
        assert_eq!(v["theme_presets"], "<2 items>");
        assert_eq!(v["keybinds"][0]["key"], "F9");
        assert_eq!(v["encoder"], "auto");
        assert_eq!(v["polls_address"], "<an address is set; removed>");
    }
}
