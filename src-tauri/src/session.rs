//! The end of the Windows session (0.1.46): restart, shut down, sign out.
//!
//! A friend's PC showed "ffmpeg.exe - Application Error ... (0xc0000142)" at every restart. That
//! is STATUS_DLL_INIT_FAILED: a process started while the session is ending cannot attach to the
//! window station. Windows ended the capture's ffmpeg, the recorder's supervisor started a new one
//! a second later, and that start failed with the box (which can hold up the restart).
//!
//! - `watch` runs a hidden top-level window on its own thread (not message-only: those do not get
//!   the broadcast). `WM_QUERYENDSESSION` sets `ending` and answers TRUE (never blocks the
//!   shutdown); `WM_ENDSESSION` TRUE calls the given stop (the recorder's children end quietly);
//!   FALSE (the shutdown was cancelled) clears `ending`, so the recorder starts again by itself.
//! - While `ending`, `util::hidden_cmd` starts nothing (the gate), and the recorder waits.
//! - `quiet_errors` (first line of `main`): no error box from Glint or any child it starts
//!   (the error mode is inherited; no spawn of Glint passes CREATE_DEFAULT_ERROR_MODE).
//! - `retry_delay` / `start_failure` / `exit_text`: a capture that will not start is retried slowly.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

static ENDING: AtomicBool = AtomicBool::new(false);

/// Windows is ending the session: start no process.
pub fn ending() -> bool {
    ENDING.load(Ordering::SeqCst)
}

pub fn set_ending(on: bool) {
    ENDING.store(on, Ordering::SeqCst);
}

const SEM_FAILCRITICALERRORS: u32 = 0x0001;
const SEM_NOGPFAULTERRORBOX: u32 = 0x0002;
const SEM_NOOPENFILEERRORBOX: u32 = 0x8000;
/// The three bits `quiet_errors` sets.
pub const QUIET: u32 = SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX;

#[link(name = "kernel32")]
extern "system" {
    fn SetErrorMode(mode: u32) -> u32;
    fn GetErrorMode() -> u32;
}

/// No Windows error box for this process or the children it starts (they inherit the mode).
pub fn quiet_errors() {
    unsafe {
        SetErrorMode(QUIET);
    }
}

pub fn error_mode() -> u32 {
    unsafe { GetErrorMode() }
}

/// The wait before try `n` (1 = the first retry) of a capture whose ffmpeg would not start: 2 s,
/// 5 s, 15 s, then every 30 s. Never a tight loop.
pub fn retry_delay(n: u32) -> u64 {
    match n {
        0 | 1 => 2,
        2 => 5,
        3 => 15,
        _ => 30,
    }
}

const DLL_INIT_FAILED: u32 = 0xC000_0142;
const DLL_NOT_FOUND: u32 = 0xC000_0135;
const BAD_IMAGE: u32 = 0xC000_007B;

/// A child that did not really start: it exited within a second, or with one of the loader's codes.
pub fn start_failure(code: Option<i32>, ran_ms: u128) -> bool {
    ran_ms < 1000 || code.is_some_and(|c| [DLL_INIT_FAILED, DLL_NOT_FOUND, BAD_IMAGE].contains(&(c as u32)))
}

/// What an exit code usually means, for the log ("" for an ordinary code).
pub fn exit_text(code: Option<i32>) -> &'static str {
    match code.map(|c| c as u32) {
        Some(DLL_INIT_FAILED) => "DLL initialization failed: usually the Windows session is starting or ending",
        Some(DLL_NOT_FOUND) => "a DLL it needs was not found",
        Some(BAD_IMAGE) => "a DLL it loads is damaged or of the wrong kind (32/64 bit)",
        None => "no exit code: it may not have started at all",
        _ => "",
    }
}

/// An exit code as Windows writes it: 0xC0000142.
pub fn code_hex(code: Option<i32>) -> String {
    code.map_or("none".into(), |c| format!("{:#010X}", c as u32).replacen("0X", "0x", 1))
}

type Stop = Box<dyn Fn() + Send + Sync>;
static STOP: OnceLock<Mutex<Option<Stop>>> = OnceLock::new();
/// How often `WM_ENDSESSION` TRUE ran the stop (tests).
static STOPS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

const WM_QUERYENDSESSION: u32 = 0x0011;
const WM_ENDSESSION: u32 = 0x0016;

/// What the hidden window answers: Some(result) for the two session messages.
pub fn on_message(msg: u32, wparam: usize) -> Option<isize> {
    match msg {
        WM_QUERYENDSESSION => {
            if !ending() {
                crate::util::log("session: Windows is ending the session; no capture is started from now on");
            }
            set_ending(true);
            Some(1) // never block the shutdown
        }
        WM_ENDSESSION if wparam != 0 => {
            set_ending(true);
            STOPS.fetch_add(1, Ordering::SeqCst);
            if let Some(m) = STOP.get() {
                if let Ok(g) = m.lock() {
                    if let Some(stop) = g.as_ref() {
                        stop();
                    }
                }
            }
            Some(0)
        }
        WM_ENDSESSION => {
            if ending() {
                crate::util::log("session: the shutdown was cancelled; capturing again");
            }
            set_ending(false);
            Some(0)
        }
        _ => None,
    }
}

unsafe extern "system" fn proc_(h: windows::Win32::Foundation::HWND, m: u32, w: windows::Win32::Foundation::WPARAM, l: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    match on_message(m, w.0) {
        Some(r) => windows::Win32::Foundation::LRESULT(r),
        None => windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(h, m, w, l),
    }
}

/// The hidden top-level window that hears the end of the session (never shown, never focused).
fn create_window() -> Option<windows::Win32::Foundation::HWND> {
    use windows::core::w;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let inst = GetModuleHandleW(None).ok()?;
        let wc = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: inst.into(), lpszClassName: w!("GlintSessionWatch"), ..Default::default() };
        RegisterClassW(&wc);
        CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, w!("GlintSessionWatch"), w!("Glint session"), WS_POPUP, 0, 0, 0, 0, None, None, windows::Win32::Foundation::HINSTANCE::from(inst), None).ok()
    }
}

/// Start listening for the end of the session; `stop` runs once on `WM_ENDSESSION` TRUE.
pub fn watch(stop: Stop) {
    let _ = STOP.set(Mutex::new(Some(stop)));
    std::thread::Builder::new()
        .name("session-watch".into())
        .spawn(|| unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, TranslateMessage, MSG};
            if create_window().is_none() {
                crate::util::log("session: the session watch window could not be made");
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .ok();
}

// ---------------------------------------------------------------------------
// `glint.exe --session-test <report.txt>`: no shutdown, no broadcast, no window shown.

/// `glint.exe --errmode-child <file>`: write this process's error mode (the test's child).
pub fn errmode_child(file: &str) {
    let _ = std::fs::write(file, error_mode().to_string());
}

pub fn self_test(report: &str) {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyWindow, IsWindowVisible, SendMessageW};
    let mut lines: Vec<String> = Vec::new();
    let mut failed = 0;
    let mut check = |name: &str, ok: bool, detail: String| {
        if !ok {
            failed += 1;
        }
        lines.push(format!("{} {name}{}", if ok { "PASS" } else { "FAIL" }, if detail.is_empty() { String::new() } else { format!("  [{detail}]") }));
    };
    // Pure.
    let seq: Vec<u64> = (1..=6).map(retry_delay).collect();
    check("retry: 2 s, 5 s, 15 s, then every 30 s", seq == [2, 5, 15, 30, 30, 30], format!("{seq:?}"));
    check("start failure: within a second, or the loader's codes", start_failure(Some(1), 400) && start_failure(Some(0xC000_0142u32 as i32), 5000) && start_failure(Some(0xC000_0135u32 as i32), 5000) && start_failure(Some(0xC000_007Bu32 as i32), 5000) && !start_failure(Some(1), 5000) && !start_failure(None, 20_000), String::new());
    check("exit texts: the code in hex and what it means", code_hex(Some(0xC000_0142u32 as i32)) == "0xC0000142" && exit_text(Some(0xC000_0142u32 as i32)).contains("session") && exit_text(Some(0xC000_0135u32 as i32)).contains("DLL") && exit_text(Some(1)).is_empty(), code_hex(Some(0xC000_0142u32 as i32)));
    // The error mode, set by `main` before anything else.
    check("error mode: this process has the three bits", error_mode() & QUIET == QUIET, format!("{:#x}", error_mode()));
    // The gate.
    set_ending(false);
    let probe = |dir: &std::path::Path| crate::util::hidden_cmd(&std::env::current_exe().unwrap_or_default()).args(["--errmode-child"]).arg(dir.join("mode.txt")).status();
    let dir = crate::util::data_dir().join("session-test");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::remove_file(dir.join("mode.txt"));
    let ran = probe(&dir).is_ok_and(|s| s.success());
    let child_mode: u32 = std::fs::read_to_string(dir.join("mode.txt")).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(0);
    check("error mode: a child started by Glint inherits it (it reports the three bits)", ran && child_mode & QUIET == QUIET, format!("child {child_mode:#x}"));
    set_ending(true);
    let _ = std::fs::remove_file(dir.join("mode.txt"));
    let refused = probe(&dir);
    check("gate: while the session ends nothing is started (spawn returns an error)", refused.is_err() && !dir.join("mode.txt").exists(), format!("{:?}", refused.as_ref().map(|s| s.code())));
    // The window proc, driven by SendMessage to our own hidden window only.
    set_ending(false);
    let before = STOPS.load(Ordering::SeqCst);
    match create_window() {
        Some(h) => unsafe {
            check("window: hidden (never shown)", !IsWindowVisible(h).as_bool(), String::new());
            let q = SendMessageW(h, WM_QUERYENDSESSION, WPARAM(0), LPARAM(0)).0;
            check("WM_QUERYENDSESSION: answers TRUE (never blocks) and sets the flag", q == 1 && ending(), format!("{q}"));
            let c = SendMessageW(h, WM_ENDSESSION, WPARAM(0), LPARAM(0)).0;
            check("WM_ENDSESSION FALSE (cancelled): the flag is cleared, starting is allowed again", c == 0 && !ending() && probe(&dir).is_ok_and(|s| s.success()), String::new());
            let _ = SendMessageW(h, WM_QUERYENDSESSION, WPARAM(0), LPARAM(0));
            let _ = SendMessageW(h, WM_ENDSESSION, WPARAM(1), LPARAM(0));
            check("WM_ENDSESSION TRUE: the stop runs and nothing may start", STOPS.load(Ordering::SeqCst) == before + 1 && ending(), String::new());
            let _ = DestroyWindow(h);
        },
        None => check("window: made", false, String::new()),
    }
    set_ending(false);
    let _ = std::fs::remove_dir_all(&dir);
    let n = lines.len();
    lines.push(format!("{n} checks, {failed} failed"));
    let _ = std::fs::write(report, lines.join("\n") + "\n");
}
