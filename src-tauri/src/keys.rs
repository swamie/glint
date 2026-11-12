//! Synthetic key presses, used only by the self-tests to exercise the hotkey hook.
use std::mem::size_of;
use std::sync::mpsc::Receiver;
use std::time::Duration;
use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY};

fn key(vk: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 },
        },
    }
}

/// Press `vk` while holding `mods`, then release everything.
pub fn tap(vk: u16, mods: &[u16]) {
    let mut seq: Vec<INPUT> = mods.iter().map(|m| key(*m, false)).collect();
    seq.push(key(vk, false));
    seq.push(key(vk, true));
    seq.extend(mods.iter().rev().map(|m| key(*m, true)));
    for input in seq {
        unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// `glint.exe --hotkey-test <report.json>`: checks that one key can carry several
/// bindings. F13 and Shift+F13 are bound; Ctrl+F13 is not and must fall back to F13.
pub fn self_test(report: &str) {
    use crate::settings::Keybind;
    let binds = vec![
        Keybind { id: "plain".into(), key: "F13".into(), seconds: 30, ..Default::default() },
        Keybind { id: "shift".into(), key: "F13".into(), shift: true, seconds: 600, ..Default::default() },
        Keybind { id: "ctrl-alt".into(), key: "F13".into(), ctrl: true, alt: true, action: "toggle".into(), ..Default::default() },
    ];
    crate::hotkeys::set_binds(&binds);
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    crate::hotkeys::start(tx);
    std::thread::sleep(Duration::from_millis(400));

    // '!' lines are diagnostics, not keybind ids.
    let recv = |rx: &Receiver<String>| loop {
        match rx.recv_timeout(Duration::from_millis(800)) {
            Ok(m) if m.starts_with('!') => continue,
            Ok(m) => break m,
            Err(_) => break "<none>".to_string(),
        }
    };
    let cases: [(&str, &[u16], &str); 5] = [
        ("F13", &[], "plain"),
        ("Shift+F13", &[0x10], "shift"),
        ("Ctrl+F13 (unbound, falls back)", &[0x11], "plain"),
        ("Ctrl+Alt+F13", &[0x11, 0x12], "ctrl-alt"),
        ("Ctrl+Shift+F13 (most specific wins)", &[0x11, 0x10], "shift"),
    ];
    let mut results = Vec::new();
    let mut all_ok = true;
    for (name, mods, expect) in cases {
        tap(0x7C, mods);
        let got = recv(&rx);
        all_ok &= got == expect;
        results.push(serde_json::json!({ "press": name, "expected": expect, "got": got, "ok": got == expect }));
    }
    crate::hotkeys::suspend(true);
    tap(0x7C, &[]);
    let got = recv(&rx);
    all_ok &= got == "<none>";
    results.push(serde_json::json!({ "press": "F13 while suspended", "expected": "<none>", "got": got, "ok": got == "<none>" }));
    let _ = std::fs::write(report, serde_json::to_string_pretty(&serde_json::json!({ "ok": all_ok, "cases": results })).unwrap_or_default());
}
