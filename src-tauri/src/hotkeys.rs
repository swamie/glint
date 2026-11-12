//! Global hotkeys through a low-level keyboard hook.
//!
//! A hook (not RegisterHotKey) lets several bindings share one key: F8 and
//! Shift+F8 are separate bindings, and the most specific match wins. Extra
//! held modifiers that belong to no binding (Ctrl while crouching) still fall
//! back to the plain key, and the key press is never swallowed from the game.
use crate::settings::Keybind;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER,
};

const M_CTRL: u8 = 1;
const M_SHIFT: u8 = 2;
const M_ALT: u8 = 4;
const M_WIN: u8 = 8;

#[derive(Clone)]
struct Bind {
    vk: u32,
    mods: u8,
    id: String,
}

static BINDS: Mutex<Vec<Bind>> = Mutex::new(Vec::new());
/// Non-modifier keys currently held, with the time of their last key-down.
static DOWN: Mutex<Vec<(u32, Instant)>> = Mutex::new(Vec::new());
static SUSPEND_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);
static EVENTS: AtomicU64 = AtomicU64::new(0);
/// `GLINT_DEBUG_KEYS=1` logs every key event the hook sees (troubleshooting only).
static DEBUG: OnceLock<bool> = OnceLock::new();
static TX: OnceLock<Sender<String>> = OnceLock::new();

/// Map a KeyboardEvent.code to a Windows virtual-key code.
pub fn vk_from_code(code: &str) -> Option<u32> {
    if let Some(n) = code.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=24).contains(&n).then(|| 0x70 + n - 1);
    }
    if let Some(c) = code.strip_prefix("Key") {
        let b = c.as_bytes();
        return (b.len() == 1 && b[0].is_ascii_uppercase()).then(|| b[0] as u32);
    }
    if let Some(d) = code.strip_prefix("Digit").and_then(|n| n.parse::<u32>().ok()) {
        return (d <= 9).then(|| 0x30 + d);
    }
    if let Some(d) = code.strip_prefix("Numpad").and_then(|n| n.parse::<u32>().ok()) {
        return (d <= 9).then(|| 0x60 + d);
    }
    Some(match code {
        "NumpadMultiply" => 0x6A,
        "NumpadAdd" => 0x6B,
        "NumpadSubtract" => 0x6D,
        "NumpadDecimal" => 0x6E,
        "NumpadDivide" => 0x6F,
        "Insert" => 0x2D,
        "Delete" => 0x2E,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "ArrowLeft" => 0x25,
        "ArrowUp" => 0x26,
        "ArrowRight" => 0x27,
        "ArrowDown" => 0x28,
        "Backquote" => 0xC0,
        "Minus" => 0xBD,
        "Equal" => 0xBB,
        "BracketLeft" => 0xDB,
        "BracketRight" => 0xDD,
        "Backslash" => 0xDC,
        "Semicolon" => 0xBA,
        "Quote" => 0xDE,
        "Comma" => 0xBC,
        "Period" => 0xBE,
        "Slash" => 0xBF,
        "Space" => 0x20,
        "Tab" => 0x09,
        "Enter" => 0x0D,
        "Backspace" => 0x08,
        "CapsLock" => 0x14,
        "PrintScreen" => 0x2C,
        "ScrollLock" => 0x91,
        "Pause" => 0x13,
        _ => return None,
    })
}

pub fn set_binds(keybinds: &[Keybind]) {
    let binds: Vec<Bind> = keybinds
        .iter()
        .filter_map(|k| {
            let vk = vk_from_code(&k.key)?;
            let mods = (k.ctrl as u8 * M_CTRL) | (k.shift as u8 * M_SHIFT) | (k.alt as u8 * M_ALT) | (k.win as u8 * M_WIN);
            Some(Bind { vk, mods, id: k.id.clone() })
        })
        .collect();
    *BINDS.lock().unwrap() = binds;
}

/// Pause matching while the UI is recording a new key combination. The pause
/// expires on its own, so a window closed mid-edit cannot leave hotkeys off;
/// the UI renews it every few seconds while it is still listening.
pub fn suspend(on: bool) {
    *SUSPEND_UNTIL.lock().unwrap() = on.then(|| Instant::now() + Duration::from_secs(12));
}

fn suspended() -> bool {
    SUSPEND_UNTIL.try_lock().map(|g| g.is_some_and(|t| Instant::now() < t)).unwrap_or(false)
}

fn held(vk: i32) -> bool {
    unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
}

/// When each modifier (ctrl, shift, alt, win) last went down through the hook.
static MOD_DOWN: Mutex<[Option<Instant>; 4]> = Mutex::new([None; 4]);

fn mod_index(vk: u32) -> Option<usize> {
    match vk {
        0x11 | 0xA2 | 0xA3 => Some(0),
        0x10 | 0xA0 | 0xA1 => Some(1),
        0x12 | 0xA4 | 0xA5 => Some(2),
        0x5B | 0x5C => Some(3),
        _ => None,
    }
}

/// Held modifiers. The async key state lags the hook by one event, so a
/// modifier pressed a few milliseconds before the key is also taken from the
/// hook's own record of recent modifier presses.
fn current_mods() -> u8 {
    let mut mods =
        (held(0x11) as u8 * M_CTRL) | (held(0x10) as u8 * M_SHIFT) | (held(0x12) as u8 * M_ALT) | ((held(0x5B) || held(0x5C)) as u8 * M_WIN);
    if let Ok(seen) = MOD_DOWN.try_lock() {
        for (i, bit) in [M_CTRL, M_SHIFT, M_ALT, M_WIN].into_iter().enumerate() {
            if seen[i].is_some_and(|t| t.elapsed() < Duration::from_millis(300)) {
                mods |= bit;
            }
        }
    }
    mods
}

/// A fresh press of `vk` (not an auto-repeat): find the binding and report it.
/// Lines starting with '!' on the channel are diagnostics for the log, not keybind ids.
fn dispatch(vk: u32) {
    let Ok(binds) = BINDS.try_lock() else { return };
    if !binds.iter().any(|b| b.vk == vk) {
        return;
    }
    let Some(tx) = TX.get() else { return };
    if suspended() {
        let _ = tx.send("!hotkeys: key ignored while a keybind is being edited".into());
        return;
    }
    let mods = current_mods();
    let best = binds.iter().filter(|b| b.vk == vk && b.mods & mods == b.mods).max_by_key(|b| b.mods.count_ones());
    if let Some(b) = best {
        let _ = tx.send(b.id.clone());
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        EVENTS.fetch_add(1, Ordering::Relaxed);
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let vk = kb.vkCode;
        let msg = wparam.0 as u32;
        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        if *DEBUG.get_or_init(|| std::env::var_os("GLINT_DEBUG_KEYS").is_some()) {
            if let Some(tx) = TX.get() {
                let _ = tx.send(format!("!key vk={vk:#x} msg={msg:#x} flags={:#x} suspended={}", kb.flags.0, suspended()));
            }
        }
        // try_lock only: this callback must never block system-wide input.
        if let Some(i) = mod_index(vk) {
            if let Ok(mut seen) = MOD_DOWN.try_lock() {
                seen[i] = is_down.then(Instant::now);
            }
        } else if is_down {
            if let Ok(mut down) = DOWN.try_lock() {
                let now = Instant::now();
                // A held key repeats at least once a second. A longer silence means its
                // key-up was never seen (lock screen, elevated window), so treat this
                // as a new press instead of ignoring the key forever.
                let repeat = match down.iter_mut().find(|(k, _)| *k == vk) {
                    Some((_, t)) => {
                        let r = now.duration_since(*t) < Duration::from_millis(1200);
                        *t = now;
                        r
                    }
                    None => {
                        down.push((vk, now));
                        false
                    }
                };
                drop(down);
                if !repeat {
                    dispatch(vk);
                }
            }
        } else if msg == WM_KEYUP || msg == WM_SYSKEYUP {
            if let Ok(mut down) = DOWN.try_lock() {
                down.retain(|(k, _)| *k != vk);
            }
        }
    }
    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

/// Install the hook on its own thread. Matched keybind ids arrive on `tx`.
pub fn start(tx: Sender<String>) {
    if TX.set(tx).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("hotkeys".into())
        .spawn(|| unsafe {
            // Answer the hook ahead of everything else in the process.
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
            let install = || SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), HINSTANCE::default(), 0);
            let mut hook = match install() {
                Ok(h) => {
                    crate::util::log("hotkeys: keyboard hook installed");
                    Some(h)
                }
                Err(e) => {
                    crate::util::log(&format!("hotkeys: keyboard hook FAILED: {e}"));
                    None
                }
            };
            // Windows silently removes a low-level hook whose thread was once slow to
            // answer, and nothing reports it. Re-installing on a timer makes any such
            // loss last at most 20 seconds instead of until the next restart.
            SetTimer(HWND::default(), 0, 20_000, None);
            let mut msg = MSG::default();
            let mut ticks = 0u32;
            while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
                if msg.message != WM_TIMER {
                    continue;
                }
                match install() {
                    Ok(new) => {
                        // New hook first, then drop the old one, so no key is missed.
                        if let Some(old) = hook.replace(new) {
                            let _ = UnhookWindowsHookEx(old);
                        }
                    }
                    Err(e) => crate::util::log(&format!("hotkeys: re-install failed: {e}")),
                }
                ticks += 1;
                if ticks % 90 == 0 {
                    crate::util::log(&format!("hotkeys: alive, {} key events seen", EVENTS.load(Ordering::Relaxed)));
                }
            }
        })
        .ok();
}
