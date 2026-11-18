//! Glint Doctor: a support tool that runs once.
//!
//! The developer gives a user a one-time support code. The user runs this
//! program; it reads Glint's log and settings and a few facts about the PC,
//! takes the personal data out (scrub.rs), shows what it is about to send, and
//! after the user agrees posts it with the code to the developer's endpoint
//! (server/doctor). There the code is used up, Claude reads the diagnostics
//! and writes a report for the developer. The user gets "sent, thanks" and,
//! if the developer allows it, a short summary in plain words.
//!
//! One-time: the server refuses a code that was used, which is the real
//! guarantee. Locally the tool also leaves `doctor-used.json` in Glint's data
//! folder and refuses to run again while it is there, and it removes its own
//! exe after a successful run (not with --keep).
//!
//! It is its own program: nothing in src-tauri or ui knows about it, and it
//! changes nothing of Glint's (it only adds the one marker file).
//!
//!   glint-doctor.exe                       ask for the code, then run
//!   glint-doctor-ABCD-EFGH-JKMN.exe        the code is in the file name
//!   glint-doctor.exe --code ABCD-EFGH-JKMN
//!   glint-doctor.exe --dry-run report.txt  collect and write the report to a file; send nothing
//!   --keep                                 do not remove the exe afterwards
mod collect;
mod scrub;
mod send;

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The letters and digits a support code is made of: none that look alike (no 0 O 1 I L).
const CODE_CHARS: &str = "23456789ABCDEFGHJKMNPQRSTUVWXYZ";
/// How long to wait for the summary after the report was taken.
const WAIT_FOR_SUMMARY: Duration = Duration::from_secs(300);

fn say(text: &str) {
    println!("{text}");
    let _ = io::stdout().flush();
}

fn ask(prompt: &str) -> String {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);
    line.trim().to_string()
}

/// "abcd efgh-jkmn" -> Some("ABCD-EFGH-JKMN"); anything that is not twelve code characters -> None.
fn normalize_code(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().filter(|c| c.is_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
    if chars.len() != 12 || !chars.iter().all(|c| CODE_CHARS.contains(*c)) {
        return None;
    }
    Some(chars.chunks(4).map(|g| g.iter().collect::<String>()).collect::<Vec<_>>().join("-"))
}

/// A code in this program's own file name: three groups of four code characters in a row,
/// as in "glint-doctor-ABCD-EFGH-JKMN (1).exe". "GLINT" and "DOCTOR" hold letters no code has.
fn code_in_name(stem: &str) -> Option<String> {
    let upper = stem.to_uppercase();
    let parts: Vec<&str> = upper.split(|c: char| !c.is_alphanumeric()).filter(|p| !p.is_empty()).collect();
    let fits = |p: &str| p.len() == 4 && p.chars().all(|c| CODE_CHARS.contains(c));
    parts.windows(3).find(|w| w.iter().all(|p| fits(p))).map(|w| w.join("-"))
}

/// Text from the server, made safe to print: no control characters (a terminal escape
/// sequence could otherwise rewrite what the window shows), at most 900 characters.
fn printable(text: &str) -> String {
    text.chars().filter(|c| *c == '\n' || !c.is_control()).take(900).collect::<String>().trim().to_string()
}

fn marker() -> PathBuf {
    collect::data_dir().join("doctor-used.json")
}

/// Remove this exe a few seconds after it has ended (a running exe cannot delete itself).
#[cfg(windows)]
fn remove_self() {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Ok(exe) = std::env::current_exe() else { return };
    let _ = std::process::Command::new("cmd.exe")
        .raw_arg(format!("/D /C ping -n 4 127.0.0.1 >nul & del /F /Q \"{}\"", exe.display()))
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(not(windows))]
fn remove_self() {}

struct Args {
    code: Option<String>,
    dry_run: Option<PathBuf>,
    keep: bool,
    yes: bool,
}

fn args() -> Args {
    let mut a = Args { code: None, dry_run: None, keep: false, yes: false };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--code" => a.code = it.next(),
            "--dry-run" => a.dry_run = it.next().map(PathBuf::from),
            "--keep" => a.keep = true,
            "--yes" => a.yes = true,
            _ => {}
        }
    }
    a
}

fn run(a: &Args) -> Result<bool, String> {
    say("Glint Doctor");
    say("A one-time check of Glint on this PC, sent to Glint's developer.");
    say("");

    let scrubber = scrub::Scrubber::from_env();

    if let Some(path) = &a.dry_run {
        say("Collecting (nothing will be sent)...");
        let bundle = collect::bundle(&scrubber, "(dry run)");
        fs::write(path, collect::preview(&bundle)).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
        say(&format!("Written to {}", path.display()));
        return Ok(false);
    }

    if marker().is_file() {
        return Err("This tool has already been used on this PC. Each support code works once; ask the developer for a new one if you still need help.".into());
    }
    if send::endpoint().contains("CHANGE-ME") {
        return Err("This copy of Glint Doctor was built without the developer's address.".into());
    }

    // 1. The code: from the command line, from this file's name, or typed.
    let from_name = std::env::current_exe().ok().and_then(|e| e.file_stem().map(|s| s.to_string_lossy().into_owned())).and_then(|s| code_in_name(&s));
    let mut code = a.code.as_deref().and_then(normalize_code).or(from_name);
    while code.is_none() {
        let typed = ask("Type the support code the developer gave you (like ABCD-EFGH-JKMN), or just press Enter to quit: ");
        if typed.is_empty() {
            return Ok(false);
        }
        code = normalize_code(&typed);
        if code.is_none() {
            say("That is not a support code: it has twelve letters and digits. Check it and try again.");
        }
    }
    let code = code.unwrap();

    // 2. A few words from the user.
    say("");
    say("In a sentence or two, what goes wrong? (Press Enter to skip.)");
    let description = if a.yes { String::new() } else { ask("> ") };

    // 3. Collect and scrub.
    say("");
    say("Reading Glint's log and settings and checking this PC. This takes a few seconds...");
    let bundle = collect::bundle(&scrubber, &description);
    let size = serde_json::to_string(&bundle).map(|s| s.len()).unwrap_or(0);

    // 4. Show it, and ask.
    let work = send::Work::new()?;
    say("");
    say("Ready to send to the developer:");
    say("  - Glint's version, settings (no keys, tokens or addresses) and the end of its log");
    say("  - Windows version, graphics card and driver, CPU, memory, free disk space");
    say("  - which ffmpeg Glint uses, whether other recording software is running");
    say("  - crashes of Glint that Windows recorded");
    say("Your Windows user name, the PC's name, clip names, e-mail and network addresses are taken out first.");
    say(&format!("Size: {} KB. The report is read by an AI model (Claude) and by the developer.", size / 1024 + 1));
    if !a.yes {
        loop {
            say("");
            let answer = ask("Press Enter to send, V to look at everything first, Q to quit without sending: ").to_lowercase();
            match answer.as_str() {
                "" | "y" | "yes" | "s" | "send" => break,
                "v" | "view" => {
                    let file = work.0.join("glint-doctor-report.txt");
                    match fs::write(&file, collect::preview(&bundle)) {
                        Ok(()) => {
                            say("Opening it in Notepad. Close Notepad to come back here.");
                            if std::process::Command::new("notepad.exe").arg(&file).status().is_err() {
                                say(&format!("Notepad did not open. The file is here: {}", file.display()));
                            }
                        }
                        Err(e) => say(&format!("Could not write the preview: {e}")),
                    }
                }
                "q" | "quit" | "n" | "no" => {
                    say("Nothing was sent. Your code is still good.");
                    return Ok(false);
                }
                _ => {}
            }
        }
    }

    // 5. Send. The server uses the code up the moment it accepts the report.
    say("");
    say("Sending...");
    let ticket = send::submit(&work, &code, &bundle)?;
    let note = serde_json::json!({ "used_unix": collect::now_secs(), "ticket": ticket, "note": "Glint Doctor ran once on this PC. This file only stops it from running again; it is safe to delete." });
    let _ = fs::create_dir_all(collect::data_dir());
    let _ = fs::write(marker(), serde_json::to_string_pretty(&note).unwrap_or_default());
    say("Sent. Thank you! The developer has your report.");
    say("");

    // 6. Optionally, the short summary.
    say("Waiting a few minutes for a short summary. You can close this window now if you like; nothing more is needed from you.");
    let started = Instant::now();
    let mut summary = None;
    while started.elapsed() < WAIT_FOR_SUMMARY {
        std::thread::sleep(Duration::from_secs(6));
        print!(".");
        let _ = io::stdout().flush();
        let Some(st) = send::status(&work, &ticket) else { continue };
        match st["status"].as_str().unwrap_or("") {
            "done" => {
                summary = Some(st["user_summary"].as_str().unwrap_or("").to_string());
                break;
            }
            "failed" => {
                summary = Some(String::new());
                break;
            }
            _ => {}
        }
    }
    say("");
    match summary.as_deref() {
        Some(text) if !printable(text).is_empty() => {
            say("");
            say("What was found:");
            say(&printable(text));
        }
        _ => say("The developer will take it from here and get back to you."),
    }
    Ok(true)
}

fn main() {
    let a = args();
    let result = run(&a);
    let sent = match result {
        Ok(sent) => sent,
        Err(e) => {
            say("");
            say(&e);
            false
        }
    };
    if !a.yes && a.dry_run.is_none() {
        say("");
        ask("Press Enter to close.");
    }
    if sent && !a.keep {
        remove_self();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes() {
        assert_eq!(normalize_code(" abcd efgh-jkmn ").as_deref(), Some("ABCD-EFGH-JKMN"));
        assert_eq!(normalize_code("ABCD-EFGH-JKM"), None);
        assert_eq!(normalize_code("ABCD-EFGH-JKL0"), None);
        assert_eq!(code_in_name("glint-doctor-abcd-efgh-jkmn (1)").as_deref(), Some("ABCD-EFGH-JKMN"));
        assert_eq!(code_in_name("glint-doctor"), None);
        assert_eq!(code_in_name("glint-doctor-2345-6789"), None);
    }

    #[test]
    fn server_text_is_printed_plainly() {
        assert_eq!(printable("Update the driver.\x1b[2J\x1b]0;owned\x07 Then restart.\r\n"), "Update the driver.[2J]0;owned Then restart.");
        assert_eq!(printable(&"a".repeat(2000)).len(), 900);
        assert!(send::is_ticket("0123456789abcdefghijklmn"));
        assert!(!send::is_ticket("../admin/reports/abcdefgh"));
    }
}
