//! GlintSetup.exe: the one-time installer.
//!
//! It downloads everything from GitHub releases and puts it in place:
//!   - Glint itself          github.com/swamie/glint          (latest release)
//!   - ffmpeg and ffprobe    github.com/GyanD/codexffmpeg     (latest release, "essentials" build)
//!   - Blur (optional)       github.com/f0e/blur              (v2.23, the version Glint's settings match)
//! and, only if this PC lacks it, Microsoft's WebView2 runtime from microsoft.com
//! (Windows 11 has it built in).
//!
//! After this, Glint updates itself from its GitHub releases (see update.rs).
//! No extra libraries: it drives curl.exe, tar.exe and powershell.exe, which
//! ship with Windows 10 and 11.
//!
//! Options (all optional; with none it asks a few questions):
//!   --dir <folder>     install here instead of %LOCALAPPDATA%\Programs\Glint
//!   --yes              ask nothing, take the default for every question
//!   --no-ffmpeg        do not fetch ffmpeg        --force-ffmpeg  fetch it even if one is on PATH
//!   --no-blur          do not fetch Blur
//!   --no-shortcuts     no Start menu or desktop shortcut, no entry under "Installed apps"
//!   --no-launch        do not start Glint at the end
//!   --uninstall        remove Glint (clips and settings are kept unless you say otherwise)
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const REPO: &str = "swamie/glint";
const ASSET: &str = "Glint-windows-x64.zip";
const FFMPEG_REPO: &str = "GyanD/codexffmpeg";
const BLUR_REPO: &str = "f0e/blur";
const BLUR_TAG: &str = "v2.23";
const BLUR_ASSET: &str = "blur-Windows-Installer-x64.exe";
const WEBVIEW2_URL: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Glint";

struct Opts {
    dir: PathBuf,
    yes: bool,
    ffmpeg: bool,
    force_ffmpeg: bool,
    blur: bool,
    shortcuts: bool,
    launch: bool,
}

fn env_dir(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_default())
}

fn say(text: &str) {
    println!("{text}");
    let _ = std::io::stdout().flush();
}

/// Ask a yes/no question. Enter takes the default.
fn ask(o: &Opts, question: &str, default: bool) -> bool {
    if o.yes {
        return default;
    }
    print!("{question} [{}] ", if default { "Y/n" } else { "y/N" });
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return default;
    }
    match line.trim().to_lowercase().as_str() {
        "" => default,
        s => s.starts_with('y'),
    }
}

fn pause(o: &Opts) {
    if !o.yes {
        print!("\nPress Enter to close this window.");
        let _ = std::io::stdout().flush();
        let _ = std::io::stdin().read_line(&mut String::new());
    }
}

/// GET a GitHub API address and parse the JSON.
fn api(path: &str) -> Result<serde_json::Value, String> {
    let out = Command::new("curl.exe")
        .args(["-sS", "-L", "--fail", "--retry", "2", "--max-time", "30", "-H", "User-Agent: Glint-setup", "-H", "Accept: application/vnd.github+json"])
        .arg(format!("https://api.github.com/repos/{path}"))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl.exe could not be started ({e}). Glint needs Windows 10 version 1803 or newer."))?;
    if !out.status.success() {
        return Err(format!("GitHub did not answer for {path}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    serde_json::from_slice(&out.stdout).map_err(|_| format!("GitHub sent an unreadable answer for {path}"))
}

/// (name, download address, size) of the first asset of a release whose name passes `want`.
fn asset(release: &serde_json::Value, want: impl Fn(&str) -> bool) -> Option<(String, String, u64)> {
    release["assets"].as_array()?.iter().find_map(|a| {
        let name = a["name"].as_str()?;
        want(name).then(|| (name.to_string(), a["browser_download_url"].as_str().unwrap_or("").to_string(), a["size"].as_u64().unwrap_or(0)))
    })
}

/// Download with curl's own progress bar.
fn download(url: &str, to: &Path) -> Result<(), String> {
    let ok = Command::new("curl.exe")
        .args(["-L", "--fail", "--retry", "2", "-#", "-H", "User-Agent: Glint-setup", "-o"])
        .arg(to)
        .arg(url)
        .stdin(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok && to.is_file() { Ok(()) } else { Err(format!("The download failed: {url}")) }
}

fn mb(bytes: u64) -> String {
    format!("{:.0} MB", bytes as f64 / 1e6)
}

/// Unpack a zip (tar.exe on Windows reads zip files). `only` limits it to matching paths.
fn unzip(zip: &Path, into: &Path, only: &[&str]) -> bool {
    let _ = fs::create_dir_all(into);
    Command::new("tar.exe").arg("-xf").arg(zip).arg("-C").arg(into).args(only).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

/// Copy `src` over `dst`. A file in use cannot be overwritten on Windows but can be renamed, so an existing one is moved aside first.
fn put(src: &Path, dst: &Path) -> Result<(), String> {
    if dst.exists() {
        let old = PathBuf::from(format!("{}.old", dst.display()));
        let _ = fs::remove_file(&old);
        fs::rename(dst, &old).map_err(|e| format!("Could not replace {}: {e}", dst.display()))?;
        let _ = fs::remove_file(&old); // fails harmlessly while the old file is still running
    }
    fs::copy(src, dst).map(|_| ()).map_err(|e| format!("Could not write {}: {e}", dst.display()))
}

fn find_file(dir: &Path, name: &str, depth: u32) -> Option<PathBuf> {
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_file() && p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(name)) {
            return Some(p);
        }
        if p.is_dir() && depth > 0 {
            if let Some(found) = find_file(&p, name, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

fn powershell(script: &str) -> bool {
    Command::new("powershell.exe").args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

fn ps_quote(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', "''"))
}

fn shortcut(link: &Path, target: &Path) -> bool {
    powershell(&format!(
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut({}); $s.TargetPath = {}; $s.WorkingDirectory = {}; $s.Description = 'Glint'; $s.Save()",
        ps_quote(link),
        ps_quote(target),
        ps_quote(target.parent().unwrap_or(Path::new(".")))
    ))
}

fn start_menu_link() -> PathBuf {
    env_dir("APPDATA").join(r"Microsoft\Windows\Start Menu\Programs\Glint.lnk")
}

fn desktop_link() -> PathBuf {
    env_dir("USERPROFILE").join(r"Desktop\Glint.lnk")
}

/// True if the WebView2 runtime (which draws Glint's window) is installed.
fn has_webview2() -> bool {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;
    const ID: &str = r"Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let found = |root: RegKey, base: &str| root.open_subkey(format!(r"{base}\{ID}")).and_then(|k| k.get_value::<String, _>("pv")).map(|v| !v.is_empty() && v != "0.0.0.0").unwrap_or(false);
    found(RegKey::predef(HKEY_LOCAL_MACHINE), r"SOFTWARE\WOW6432Node")
        || found(RegKey::predef(HKEY_LOCAL_MACHINE), "SOFTWARE")
        || found(RegKey::predef(HKEY_CURRENT_USER), "Software")
}

/// True if an ffmpeg on PATH has the screen-capture filter Glint uses (ffmpeg 8 or newer).
fn path_ffmpeg_ok() -> bool {
    Command::new("ffmpeg.exe")
        .args(["-hide_banner", "-filters"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(" gfxcapture "))
        .unwrap_or(false)
}

fn install(o: &Opts, tmp: &Path) -> Result<(), String> {
    fs::create_dir_all(&o.dir).map_err(|e| format!("Cannot create {}: {e}", o.dir.display()))?;
    fs::create_dir_all(tmp).map_err(|e| format!("Cannot create a temporary folder: {e}"))?;

    // 1. Glint
    say("\n[1/4] Glint");
    let release = api(&format!("{REPO}/releases/latest"))?;
    let version = release["tag_name"].as_str().unwrap_or("?").to_string();
    let (name, url, size) = asset(&release, |n| n == ASSET).ok_or(format!("Release {version} has no {ASSET}"))?;
    say(&format!("      Downloading Glint {version} ({})", mb(size)));
    let zip = tmp.join(&name);
    download(&url, &zip)?;
    let unpacked = tmp.join("glint");
    if !unzip(&zip, &unpacked, &[]) || !unpacked.join("Glint.exe").is_file() {
        return Err("Glint's download could not be unpacked".into());
    }
    for e in fs::read_dir(&unpacked).map_err(|e| e.to_string())?.flatten().filter(|e| e.path().is_file()) {
        put(&e.path(), &o.dir.join(e.file_name()))?;
    }
    say(&format!("      Installed to {}", o.dir.display()));

    // 2. ffmpeg
    say("\n[2/4] ffmpeg (records and saves the clips)");
    if !o.ffmpeg {
        say("      Skipped.");
    } else if o.dir.join("ffmpeg.exe").is_file() && !o.force_ffmpeg {
        say("      Already here.");
    } else if !o.force_ffmpeg && path_ffmpeg_ok() {
        say("      This PC already has a suitable ffmpeg; Glint will use it.");
    } else {
        let release = api(&format!("{FFMPEG_REPO}/releases/latest"))?;
        let (name, url, size) = asset(&release, |n| n.ends_with("essentials_build.zip")).ok_or("No ffmpeg build was found on GitHub")?;
        say(&format!("      Downloading {name} ({})", mb(size)));
        let zip = tmp.join(&name);
        download(&url, &zip)?;
        let unpacked = tmp.join("ffmpeg");
        // Only the two programs are needed out of the archive.
        if !unzip(&zip, &unpacked, &["*/bin/ffmpeg.exe", "*/bin/ffprobe.exe"]) {
            unzip(&zip, &unpacked, &[]);
        }
        for exe in ["ffmpeg.exe", "ffprobe.exe"] {
            let found = find_file(&unpacked, exe, 4).ok_or(format!("{exe} was not in the ffmpeg download"))?;
            put(&found, &o.dir.join(exe))?;
        }
        say("      Installed beside Glint.");
    }

    // 3. WebView2 (draws the window). Part of Windows 11; older systems may lack it.
    say("\n[3/4] WebView2 runtime (draws Glint's window)");
    if has_webview2() {
        say("      Already on this PC.");
    } else {
        say("      Not found. Downloading Microsoft's installer.");
        let exe = tmp.join("MicrosoftEdgeWebview2Setup.exe");
        download(WEBVIEW2_URL, &exe)?;
        let ok = Command::new(&exe).args(["/silent", "/install"]).status().map(|s| s.success()).unwrap_or(false);
        say(if ok { "      Installed." } else { "      It did not install. Get \"WebView2 Runtime\" from microsoft.com, then start Glint." });
    }

    // 4. Blur (optional)
    say("\n[4/4] Blur (optional: motion-blur renders of finished clips)");
    let blur_dir = env_dir("LOCALAPPDATA").join("Glint").join("blur");
    if blur_dir.join("blur-cli.exe").is_file() {
        say("      Already here.");
    } else if !o.blur {
        say("      Skipped.");
    } else {
        let release = api(&format!("{BLUR_REPO}/releases/tags/{BLUR_TAG}"))?;
        match asset(&release, |n| n == BLUR_ASSET) {
            None => say("      Blur's download was not found; get it later from github.com/f0e/blur."),
            Some((name, url, size)) => {
                if ask(o, &format!("      Install Blur {BLUR_TAG}? It is a {} download.", mb(size)), true) {
                    let exe = tmp.join(&name);
                    download(&url, &exe)?;
                    say("      Installing Blur (Windows may ask for permission)...");
                    // Blur ships as an Inno Setup installer: these are Inno's standard unattended switches.
                    let _ = Command::new(&exe).args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CURRENTUSER"]).arg(format!("/DIR={}", blur_dir.display())).status();
                    if blur_dir.join("blur-cli.exe").is_file() {
                        say("      Installed.");
                    } else {
                        say("      Blur did not end up where Glint looks for it. Install it yourself from github.com/f0e/blur");
                        say("      and set its path on Glint's Blur page. Everything else works without it.");
                    }
                } else {
                    say("      Skipped. You can run this installer again later to add it.");
                }
            }
        }
    }

    // Shortcuts and the entry under "Installed apps".
    let exe = o.dir.join("Glint.exe");
    if o.shortcuts {
        say("\nShortcuts");
        if shortcut(&start_menu_link(), &exe) {
            say("      Added Glint to the Start menu.");
        }
        if ask(o, "      Add a desktop shortcut?", false) && shortcut(&desktop_link(), &exe) {
            say("      Added a desktop shortcut.");
        }
        // Keep a copy of this installer beside Glint: it is also the uninstaller.
        if let Ok(me) = std::env::current_exe() {
            let copy = o.dir.join("GlintSetup.exe");
            if me != copy {
                let _ = put(&me, &copy);
            }
            use winreg::enums::HKEY_CURRENT_USER;
            if let Ok((key, _)) = winreg::RegKey::predef(HKEY_CURRENT_USER).create_subkey(UNINSTALL_KEY) {
                let _ = key.set_value("DisplayName", &"Glint");
                let _ = key.set_value("DisplayVersion", &version.trim_start_matches('v'));
                let _ = key.set_value("Publisher", &"Glint");
                let _ = key.set_value("DisplayIcon", &exe.to_string_lossy().to_string());
                let _ = key.set_value("InstallLocation", &o.dir.to_string_lossy().to_string());
                let _ = key.set_value("UninstallString", &format!("\"{}\" --uninstall", copy.display()));
                let _ = key.set_value("NoModify", &1u32);
                let _ = key.set_value("NoRepair", &1u32);
            }
        }
    }

    say("\nDone. Glint keeps itself up to date from now on: the small arrow at the top of its window.");
    if o.launch && ask(o, "Start Glint now?", true) {
        let _ = Command::new(&exe).current_dir(&o.dir).spawn();
    }
    Ok(())
}

fn uninstall(o: &Opts) {
    say("Remove Glint from this PC?");
    say(&format!("  Program folder: {}", o.dir.display()));
    if !ask(o, "Your clips are kept. Continue?", true) {
        return;
    }
    use winreg::enums::HKEY_CURRENT_USER;
    let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
    let _ = fs::remove_file(start_menu_link());
    let _ = fs::remove_file(desktop_link());
    if let Ok(run) = hkcu.open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Run", winreg::enums::KEY_SET_VALUE) {
        let _ = run.delete_value("Glint");
    }
    let _ = hkcu.delete_subkey_all(UNINSTALL_KEY);
    if ask(o, "Also remove Glint's settings, clip list and the Blur it installed? (Clips stay either way.)", false) {
        let _ = fs::remove_dir_all(env_dir("APPDATA").join("Glint"));
        let _ = fs::remove_dir_all(env_dir("LOCALAPPDATA").join("Glint"));
    }
    // A running program cannot delete itself: hand the job to a hidden shell that waits for this one to exit.
    // It removes only the files this installer put there, never the folder's other contents, and the
    // folder itself only if that leaves it empty (so a wrong --dir cannot wipe anything else).
    if o.dir.join("Glint.exe").is_file() {
        let script = format!(
            "Start-Sleep -Seconds 2; $d = {}; foreach ($f in 'Glint.exe','WebView2Loader.dll','ffmpeg.exe','ffprobe.exe','GlintSetup.exe') {{ Remove-Item -LiteralPath (Join-Path $d $f) -Force -ErrorAction SilentlyContinue }}; Get-ChildItem -LiteralPath $d -Filter '*.old' -File -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue; if (-not (Get-ChildItem -LiteralPath $d -Force -ErrorAction SilentlyContinue)) {{ Remove-Item -LiteralPath $d -Force -ErrorAction SilentlyContinue }}",
            ps_quote(&o.dir)
        );
        let _ = Command::new("powershell.exe").args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    }
    say("Glint has been removed. (If it is still running, quit it from the tray icon.)");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let dir = args.iter().position(|a| a == "--dir").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let o = Opts {
        // When run as the uninstaller from inside the install folder, that folder is the default.
        dir: dir.unwrap_or_else(|| match std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            Some(d) if has("--uninstall") && d.join("Glint.exe").is_file() => d,
            _ => env_dir("LOCALAPPDATA").join("Programs").join("Glint"),
        }),
        yes: has("--yes"),
        ffmpeg: !has("--no-ffmpeg"),
        force_ffmpeg: has("--force-ffmpeg"),
        blur: !has("--no-blur"),
        shortcuts: !has("--no-shortcuts"),
        launch: !has("--no-launch"),
    };
    say("Glint setup");
    say("===========");
    if has("--uninstall") {
        uninstall(&o);
        pause(&o);
        return;
    }
    say("This installs Glint, an offline instant-replay clipper, for your Windows account.");
    say("Everything is downloaded from GitHub releases. No administrator rights are needed for Glint itself.");
    say(&format!("\nInstall folder: {}", o.dir.display()));
    if !ask(&o, "Continue?", true) {
        return;
    }
    let tmp = std::env::temp_dir().join(format!("glint-setup-{}", std::process::id()));
    let result = install(&o, &tmp);
    let _ = fs::remove_dir_all(&tmp);
    if let Err(e) = result {
        say(&format!("\nSetup stopped: {e}"));
        say("Nothing else was changed. Check your internet connection and run it again.");
        pause(&o);
        std::process::exit(1);
    }
    pause(&o);
}
