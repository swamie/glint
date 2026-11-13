//! Build script: Tauri's own build step, plus the bug-report destination.
//!
//! The Discord webhook that bug reports go to is a credential and the
//! repository is public, so it is not in the source. It is read from
//! `bug-webhook.txt` beside this file (git-ignored): line 1 is the webhook
//! URL, line 2 the Discord user ids to ping, separated by commas. Both are
//! handed to the compiler XOR-ed with `KEY` and hex-encoded, so the address
//! does not sit in the program as plain text (`bugreport::reveal` undoes it).
//! Without the file the build still works and bug reports say "not set up".

/// Must match `KEY` in src/bugreport.rs.
const KEY: &[u8] = b"glint:bug-report:v1/k9#Qz";

fn hide(text: &str) -> String {
    text.bytes().enumerate().map(|(i, b)| format!("{:02x}", b ^ KEY[i % KEY.len()] ^ (i as u8).wrapping_mul(31))).collect()
}

fn main() {
    println!("cargo:rerun-if-changed=bug-webhook.txt");
    println!("cargo:rerun-if-changed=build.rs");
    let text = std::fs::read_to_string("bug-webhook.txt").unwrap_or_default();
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
    let url = lines.next().unwrap_or("").to_string();
    let ids: Vec<&str> = lines.next().unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())).collect();
    println!("cargo:rustc-env=GLINT_BUG_HOOK={}", hide(&url));
    println!("cargo:rustc-env=GLINT_BUG_PINGS={}", hide(&ids.join(",")));
    tauri_build::build()
}
