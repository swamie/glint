//! Thin Win32 helpers: monitors, foreground app, process stats, overlay window.
use serde::Serialize;
use std::ffi::c_void;
use std::mem::size_of;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, FILETIME, HWND};
use windows::Win32::Graphics::Gdi::{EnumDisplayDevicesW, EnumDisplaySettingsW, DEVMODEW, DISPLAY_DEVICEW, ENUM_CURRENT_SETTINGS};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowLongPtrW, GetWindowThreadProcessId, SetWindowLongPtrW, ShowWindow, GWL_EXSTYLE, SW_SHOWNOACTIVATE,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

#[derive(Serialize, Clone, Debug)]
pub struct Monitor {
    pub index: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub hz: u32,
    pub primary: bool,
    /// Top-left corner on the virtual desktop, in physical pixels.
    pub x: i32,
    pub y: i32,
    /// The largest mode the display offers: its real shape. A "stretched"
    /// resolution has a different shape from this one.
    pub native_width: u32,
    pub native_height: u32,
}

fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|c| *c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

/// The display's real shape, from the modes it offers: the largest one. Many
/// 16:9 screens also list a slightly wider 17:9 mode of the same height
/// (4096x2160 beside 3840x2160); the 16:9 one is the panel then.
pub fn native_mode(modes: &[(u32, u32)]) -> (u32, u32) {
    let Some(&(w, h)) = modes.iter().max_by_key(|(w, h)| *w as u64 * *h as u64) else { return (1920, 1080) };
    let ratio = w as f64 / h.max(1) as f64;
    if ratio > 1.8 && ratio < 1.95 {
        if let Some(&m) = modes.iter().find(|(mw, mh)| *mh == h && (*mw as f64 / *mh as f64 - 16.0 / 9.0).abs() < 0.01) {
            return m;
        }
    }
    (w, h)
}

/// Displays attached to the desktop, primary first (matches capture index 0).
pub fn monitors() -> Vec<Monitor> {
    let mut out = Vec::new();
    let mut i = 0u32;
    loop {
        let mut dd = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), i, &mut dd, 0) }.as_bool() {
            break;
        }
        i += 1;
        let flags = dd.StateFlags;
        if flags & 1 == 0 {
            continue; // not attached to the desktop
        }
        let mut dm = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
        if unsafe { EnumDisplaySettingsW(PCWSTR(dd.DeviceName.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm) }.as_bool() {
            let mut modes: Vec<(u32, u32)> = vec![(dm.dmPelsWidth, dm.dmPelsHeight)];
            let mut mode = 0u32;
            loop {
                let mut m = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
                if !unsafe { EnumDisplaySettingsW(PCWSTR(dd.DeviceName.as_ptr()), windows::Win32::Graphics::Gdi::ENUM_DISPLAY_SETTINGS_MODE(mode), &mut m) }.as_bool() {
                    break;
                }
                if !modes.contains(&(m.dmPelsWidth, m.dmPelsHeight)) {
                    modes.push((m.dmPelsWidth, m.dmPelsHeight));
                }
                mode += 1;
            }
            let (native_width, native_height) = native_mode(&modes);
            out.push(Monitor {
                native_width,
                native_height,
                index: 0,
                name: wide_to_string(&dd.DeviceName).trim_start_matches("\\\\.\\").to_string(),
                width: dm.dmPelsWidth,
                height: dm.dmPelsHeight,
                hz: dm.dmDisplayFrequency.max(1),
                primary: flags & 4 != 0,
                // For a display device this union member holds the position.
                x: unsafe { dm.Anonymous1.Anonymous2.dmPosition.x },
                y: unsafe { dm.Anonymous1.Anonymous2.dmPosition.y },
            });
        }
    }
    out.sort_by_key(|m| !m.primary);
    for (n, m) in out.iter_mut().enumerate() {
        m.index = n as u32;
    }
    if out.is_empty() {
        out.push(Monitor { index: 0, name: "DISPLAY1".into(), width: 1920, height: 1080, hz: 60, primary: true, x: 0, y: 0, native_width: 1920, native_height: 1080 });
    }
    out
}

/// How ffmpeg is told to capture one monitor of `monitors()` so that it cannot take another.
///
/// Glint numbers the monitors "main first". ffmpeg's `monitor_idx` (Graphics Capture) counts
/// them in the order Windows enumerates them, and `output_idx` (Desktop Duplication) counts the
/// graphics card's outputs. On a PC where those orders differ the same number meant another
/// screen: clips of the main monitor showed the second one (reported on 0.1.34). So the
/// monitor is named by what cannot be mixed up: its handle for Graphics Capture, and its place
/// among the card's outputs, found by device name, for Desktop Duplication.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorSource {
    /// The device name without the `\\.\` in front ("DISPLAY1").
    pub name: String,
    /// The HMONITOR; 0 when Windows did not list a monitor of that name.
    pub hmonitor: u64,
    /// Its index among the outputs of the first graphics adapter; None when it is not one of them.
    pub dxgi_output: Option<u32>,
}

/// The device name of a monitor as an API gives it ("\\.\DISPLAY1", zero-terminated UTF-16), bare.
fn device_name(wide: &[u16]) -> String {
    wide_to_string(wide).trim_start_matches("\\\\.\\").to_string()
}

/// Every monitor handle with its device name, in the order Windows enumerates them.
pub fn monitor_handles() -> Vec<(u64, String)> {
    use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};
    unsafe extern "system" fn each(h: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let out = &mut *(data.0 as *mut Vec<(u64, String)>);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(h, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
            out.push((h.0 as usize as u64, device_name(&info.szDevice)));
        }
        BOOL(1)
    }
    let mut out: Vec<(u64, String)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(HDC::default(), None, Some(each), LPARAM(&mut out as *mut Vec<(u64, String)> as isize));
    }
    out
}

/// The device names of the first graphics adapter's outputs, in the order Desktop Duplication counts them.
pub fn dxgi_outputs() -> Vec<String> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    let mut out = Vec::new();
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return out };
        let Ok(adapter) = factory.EnumAdapters1(0) else { return out };
        let mut i = 0u32;
        while let Ok(output) = adapter.EnumOutputs(i) {
            out.push(output.GetDesc().map(|d| device_name(&d.DeviceName)).unwrap_or_default());
            i += 1;
        }
    }
    out
}

/// `MonitorSource` of the monitor called `name`, from the two lists (split out so it can be tested on made-up lists).
pub fn source_among(name: &str, handles: &[(u64, String)], outputs: &[String]) -> MonitorSource {
    let same = |n: &str| n.eq_ignore_ascii_case(name);
    MonitorSource {
        name: name.to_string(),
        hmonitor: handles.iter().find(|(_, n)| same(n)).map_or(0, |(h, _)| *h),
        dxgi_output: outputs.iter().position(|n| same(n)).map(|i| i as u32),
    }
}

/// `MonitorSource` of monitor `index` of `monitors()`; None when there is no such monitor.
pub fn monitor_source(index: u32) -> Option<MonitorSource> {
    let mons = monitors();
    let m = mons.get(index as usize)?;
    Some(source_among(&m.name, &monitor_handles(), &dxgi_outputs()))
}

unsafe fn version_string(path: &str, key: &str) -> Option<String> {
    let wpath: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let size = GetFileVersionInfoSizeW(PCWSTR(wpath.as_ptr()), None);
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    GetFileVersionInfoW(PCWSTR(wpath.as_ptr()), 0, size, data.as_mut_ptr() as *mut c_void).ok()?;
    let mut ptr: *mut c_void = std::ptr::null_mut();
    let mut len = 0u32;
    let tr: Vec<u16> = "\\VarFileInfo\\Translation".encode_utf16().chain(Some(0)).collect();
    if !VerQueryValueW(data.as_ptr() as *const c_void, PCWSTR(tr.as_ptr()), &mut ptr, &mut len).as_bool() || len < 4 {
        return None;
    }
    let lang = *(ptr as *const u16);
    let cp = *(ptr as *const u16).add(1);
    let sub: Vec<u16> = format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\{key}").encode_utf16().chain(Some(0)).collect();
    let mut sptr: *mut c_void = std::ptr::null_mut();
    let mut slen = 0u32;
    if !VerQueryValueW(data.as_ptr() as *const c_void, PCWSTR(sub.as_ptr()), &mut sptr, &mut slen).as_bool() || slen == 0 {
        return None;
    }
    let s = std::slice::from_raw_parts(sptr as *const u16, slen as usize);
    Some(wide_to_string(s))
}

/// (total CPU seconds, working set bytes) for a process. pid 0 = this process.
pub fn proc_stats(pid: u32) -> Option<(f64, u64)> {
    unsafe {
        let h = if pid == 0 { GetCurrentProcess() } else { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()? };
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let ok = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).is_ok();
        let mut pmc = PROCESS_MEMORY_COUNTERS::default();
        let ok2 = K32GetProcessMemoryInfo(h, &mut pmc, size_of::<PROCESS_MEMORY_COUNTERS>() as u32).as_bool();
        if pid != 0 {
            let _ = CloseHandle(h);
        }
        if !ok || !ok2 {
            return None;
        }
        let ft = |f: FILETIME| (((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64) as f64 / 1e7;
        Some((ft(k) + ft(u), pmc.WorkingSetSize as u64))
    }
}

/// Every process started by `root`, by its children, and so on (not `root` itself).
/// One ToolHelp snapshot of the process list; costs about a millisecond.
pub fn descendants(root: u32) -> Vec<u32> {
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let mut pairs: Vec<(u32, u32)> = Vec::new(); // (pid, parent)
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return Vec::new() };
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            pairs.push((e.th32ProcessID, e.th32ParentProcessID));
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    // A parent id can outlive its process and be reused, so walk outwards from
    // `root` and never visit a process twice.
    let mut out: Vec<u32> = Vec::new();
    let mut todo = vec![root];
    while let Some(parent) = todo.pop() {
        for &(pid, ppid) in &pairs {
            if ppid == parent && pid != root && pid != 0 && !out.contains(&pid) {
                out.push(pid);
                todo.push(pid);
            }
        }
    }
    out
}

/// The full path of every running program this user may look at (others are left out).
/// One ToolHelp snapshot plus one query per process: a few milliseconds.
pub fn process_paths() -> Vec<String> {
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let mut pids: Vec<u32> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            if e.th32ProcessID != 0 && e.th32ProcessID != std::process::id() {
                pids.push(e.th32ProcessID);
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
        for pid in pids {
            let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { continue };
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
            let _ = CloseHandle(h);
            if ok {
                out.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
        }
    }
    out
}

/// Working set of this process plus all of its descendants (ffmpeg, the WebView2
/// processes that draw the window), the sum Task Manager shows for the app's group.
/// The process list is read again at most every 3 seconds; the sizes on every call.
pub fn tree_working_set() -> u64 {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static KIDS: Mutex<Option<(Instant, Vec<u32>)>> = Mutex::new(None);
    let kids = {
        let mut cache = KIDS.lock().unwrap();
        if !cache.as_ref().is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(3)) {
            *cache = Some((Instant::now(), descendants(std::process::id())));
        }
        cache.as_ref().map(|(_, k)| k.clone()).unwrap_or_default()
    };
    let own = proc_stats(0).map(|(_, ws)| ws).unwrap_or(0);
    own + kids.iter().filter_map(|&pid| proc_stats(pid)).map(|(_, ws)| ws).sum::<u64>()
}

/// `glint.exe --memory-test <report.txt>`: the process-tree memory figure, with a child process to find.
pub fn memory_test(report_path: &str) {
    let mut lines: Vec<String> = Vec::new();
    let check = |lines: &mut Vec<String>, name: &str, ok: bool| lines.push(format!("{} {name}", if ok { "PASS" } else { "FAIL" }));
    // A harmless child that waits a few seconds, with no window.
    let child = crate::util::hidden_cmd(std::path::Path::new("ping.exe")).args(["-n", "6", "127.0.0.1"]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    let Ok(mut child) = child else {
        let _ = std::fs::write(report_path, "FAIL could not start ping.exe\n");
        return;
    };
    std::thread::sleep(std::time::Duration::from_millis(300));
    let started = std::time::Instant::now();
    let kids = descendants(std::process::id());
    let snap_us = started.elapsed().as_micros();
    check(&mut lines, "the child process is a descendant", kids.contains(&child.id()));
    let own = proc_stats(0).map(|(_, ws)| ws).unwrap_or(0);
    let child_ws = proc_stats(child.id()).map(|(_, ws)| ws).unwrap_or(0);
    let total = tree_working_set();
    check(&mut lines, "the total counts this process and the child", own > 0 && child_ws > 0 && total >= own + child_ws / 2);
    let started = std::time::Instant::now();
    for _ in 0..10 {
        let _ = tree_working_set();
    }
    let per_call_us = started.elapsed().as_micros() / 10;
    check(&mut lines, "a cached call is cheap (under 5 ms)", per_call_us < 5000);
    lines.push(format!("INFO own={:.1} MB child={:.1} MB total={:.1} MB descendants={} snapshot={snap_us} us cached call={per_call_us} us", own as f64 / 1e6, child_ws as f64 / 1e6, total as f64 / 1e6, kids.len()));
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::write(report_path, lines.join("\n") + "\n");
}

/// Show a window without activating it, so a toast never steals focus from a game.
pub fn show_no_activate(hwnd: isize) {
    unsafe {
        let h = HWND(hwnd as *mut c_void);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex | (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as isize);
        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
    }
}

/// The window the user is working in.
pub struct Foreground {
    pub pid: u32,
    /// Full path of its executable.
    pub exe: String,
    pub title: String,
    /// The window's class name. Minecraft's Java window is "GLFW30" (or "LWJGL" in old versions).
    pub class: String,
    /// The window covers its whole monitor (fullscreen or borderless).
    pub fullscreen: bool,
}

pub fn foreground() -> Option<Foreground> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, GetWindowTextW};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == std::process::id() {
            return None;
        }
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        let exe = String::from_utf16_lossy(&buf[..len as usize]);
        let mut tbuf = [0u16; 256];
        let tlen = GetWindowTextW(hwnd, &mut tbuf).max(0) as usize;
        let title = String::from_utf16_lossy(&tbuf[..tlen]);
        let mut cbuf = [0u16; 128];
        let clen = windows::Win32::UI::WindowsAndMessaging::GetClassNameW(hwnd, &mut cbuf).max(0) as usize;
        let class = String::from_utf16_lossy(&cbuf[..clen]);
        let mut wr = RECT::default();
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let fullscreen = GetWindowRect(hwnd, &mut wr).is_ok()
            && GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool()
            && wr.left <= mi.rcMonitor.left
            && wr.top <= mi.rcMonitor.top
            && wr.right >= mi.rcMonitor.right
            && wr.bottom >= mi.rcMonitor.bottom;
        Some(Foreground { pid, exe, title, class, fullscreen })
    }
}

/// A visible top-level window of another program: what game detection (game mode, 0.1.27) looks at.
/// Only facts about the window and the path of its program; no process is opened beyond
/// `PROCESS_QUERY_LIMITED_INFORMATION` to read that path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WinInfo {
    /// The window handle, as gfxcapture's `hwnd` option takes it.
    pub hwnd: u64,
    pub pid: u32,
    /// Full path of its executable.
    pub exe: String,
    pub title: String,
    pub class: String,
    /// It covers its whole monitor (fullscreen or borderless).
    pub fullscreen: bool,
    pub minimized: bool,
    /// It is the window the user is working in.
    pub foreground: bool,
    /// Its client area (the picture without the frame), in pixels; 0 while minimized.
    pub width: u32,
    pub height: u32,
    /// Index into `monitors()` of the monitor it is (mostly) on.
    pub monitor: u32,
    /// 0.1.41: hidden by the desktop window manager (a suspended store app, another virtual
    /// desktop). Listed so a game that is only hidden for a moment is not taken for gone.
    pub cloaked: bool,
    /// Its styles: WS_POPUP (a swap chain's window usually is one) and WS_CAPTION (a title bar).
    pub popup: bool,
    pub caption: bool,
    /// The program keeps this window out of every screen capture (SetWindowDisplayAffinity).
    pub excluded: bool,
    /// A store app's window: the frame belongs to ApplicationFrameHost, `pid` and `exe` are the
    /// app's own (its CoreWindow's process).
    pub hosted: bool,
}

/// An exclusive-fullscreen Direct3D program is in front (the shell's own notification state,
/// `QUNS_RUNNING_D3D_FULL_SCREEN`). Cheap; reads nothing of another process.
pub fn d3d_fullscreen() -> bool {
    use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN};
    unsafe { SHQueryUserNotificationState().is_ok_and(|s| s == QUNS_RUNNING_D3D_FULL_SCREEN) }
}

/// The index of the monitor called `device` ("DISPLAY2") among `mons`, else the one whose corner
/// is at (`x`, `y`), else 0. By name first: positions move while a game switches the display mode.
pub fn monitor_index_of(mons: &[Monitor], device: &str, x: i32, y: i32) -> u32 {
    mons.iter()
        .find(|m| !device.is_empty() && m.name.eq_ignore_ascii_case(device))
        .or_else(|| mons.iter().find(|m| m.x == x && m.y == y))
        .map_or(0, |m| m.index)
}

#[link(name = "dwmapi")]
extern "system" {
    fn DwmGetWindowAttribute(hwnd: isize, attr: u32, value: *mut c_void, size: u32) -> i32;
}

/// Every visible top-level window with no owner and no tool-window style, of every program but
/// Glint itself: a few milliseconds. Cloaked ones are listed with `cloaked` set (0.1.41; they were
/// left out before). Only window facts, DWM attributes and the program's path are read.
pub fn windows() -> Vec<WinInfo> {
    use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::WindowsAndMessaging::{EnumChildWindows, EnumWindows, GetClassNameW, GetClientRect, GetWindow, GetWindowDisplayAffinity, GetWindowRect, GetWindowTextW, IsIconic, IsWindowVisible, GWL_STYLE, GW_OWNER, WS_CAPTION, WS_POPUP};
    unsafe extern "system" fn collect(hwnd: HWND, lp: LPARAM) -> BOOL {
        let list = &mut *(lp.0 as *mut Vec<HWND>);
        list.push(hwnd);
        BOOL(1)
    }
    /// A store app's frame: the process of its CoreWindow child, if that is another process.
    unsafe fn hosted_pid(frame: HWND, frame_pid: u32) -> Option<u32> {
        let mut kids: Vec<HWND> = Vec::new();
        let _ = EnumChildWindows(frame, Some(collect), LPARAM(&mut kids as *mut Vec<HWND> as isize));
        kids.into_iter().find_map(|k| {
            let mut cbuf = [0u16; 64];
            let n = GetClassNameW(k, &mut cbuf).max(0) as usize;
            if String::from_utf16_lossy(&cbuf[..n]) != "Windows.UI.Core.CoreWindow" {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(k, Some(&mut pid));
            (pid != 0 && pid != frame_pid).then_some(pid)
        })
    }
    let mut all: Vec<HWND> = Vec::new();
    let mons = monitors();
    let own = std::process::id();
    let mut exes: Vec<(u32, Option<String>)> = Vec::new();
    let mut out = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut Vec<HWND> as isize));
        let fg = GetForegroundWindow();
        for hwnd in all {
            if !IsWindowVisible(hwnd).as_bool() || GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.0.is_null()) {
                continue;
            }
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if ex & WS_EX_TOOLWINDOW.0 as isize != 0 {
                continue;
            }
            // Cloaked: a window of a suspended store app or on another virtual desktop (DWMWA_CLOAKED = 14).
            let mut cloak = 0u32;
            let cloaked = DwmGetWindowAttribute(hwnd.0 as isize, 14, &mut cloak as *mut u32 as *mut c_void, 4) == 0 && cloak != 0;
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 || pid == own {
                continue;
            }
            let mut cbuf = [0u16; 128];
            let clen = GetClassNameW(hwnd, &mut cbuf).max(0) as usize;
            let class = String::from_utf16_lossy(&cbuf[..clen]);
            // A store app (UWP): the frame is ApplicationFrameHost's, the app is its CoreWindow's process.
            let hosted = if class == "ApplicationFrameWindow" { hosted_pid(hwnd, pid) } else { None };
            let pid = hosted.unwrap_or(pid);
            let exe = match exes.iter().find(|(p, _)| *p == pid) {
                Some((_, e)) => e.clone(),
                None => {
                    let e = exe_of(pid);
                    exes.push((pid, e.clone()));
                    e
                }
            };
            let Some(exe) = exe else { continue };
            let mut tbuf = [0u16; 256];
            let tlen = GetWindowTextW(hwnd, &mut tbuf).max(0) as usize;
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            let mut affinity = 0u32;
            let excluded = GetWindowDisplayAffinity(hwnd, &mut affinity).is_ok() && affinity != 0;
            let minimized = IsIconic(hwnd).as_bool();
            let mut cr = RECT::default();
            let (width, height) = if !minimized && GetClientRect(hwnd, &mut cr).is_ok() { ((cr.right - cr.left).max(0) as u32, (cr.bottom - cr.top).max(0) as u32) } else { (0, 0) };
            let mut wr = RECT::default();
            let mut mx = MONITORINFOEXW::default();
            mx.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
            let have_mon = GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mx as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool();
            let mi = mx.monitorInfo;
            let fullscreen = !minimized
                && have_mon
                && GetWindowRect(hwnd, &mut wr).is_ok()
                && wr.left <= mi.rcMonitor.left
                && wr.top <= mi.rcMonitor.top
                && wr.right >= mi.rcMonitor.right
                && wr.bottom >= mi.rcMonitor.bottom;
            // By device name (0.1.41): matching positions alone fell back to the main monitor
            // whenever they did not agree, and the main monitor was recorded instead of the game's.
            let monitor = if have_mon { monitor_index_of(&mons, &device_name(&mx.szDevice), mi.rcMonitor.left, mi.rcMonitor.top) } else { 0 };
            out.push(WinInfo {
                hwnd: hwnd.0 as usize as u64,
                pid,
                exe,
                title: String::from_utf16_lossy(&tbuf[..tlen]),
                class,
                fullscreen,
                minimized,
                foreground: hwnd == fg,
                width,
                height,
                monitor,
                cloaked,
                popup: style & WS_POPUP.0 as isize != 0,
                caption: style & WS_CAPTION.0 as isize == WS_CAPTION.0 as isize,
                excluded,
                hosted: hosted.is_some(),
            });
        }
    }
    out
}

/// The full path of process `pid`'s executable (PROCESS_QUERY_LIMITED_INFORMATION only).
pub fn exe_of(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// ProductName, else FileDescription, from an executable's version resource.
pub fn exe_product(path: &str) -> Option<String> {
    unsafe { version_string(path, "ProductName").or_else(|| version_string(path, "FileDescription")) }.map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s.len() < 60)
}

pub fn process_alive(pid: u32) -> bool {
    use windows::Win32::System::Threading::GetExitCodeProcess;
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code).is_ok();
        let _ = CloseHandle(h);
        ok && code == 259 // STILL_ACTIVE
    }
}

/// Wait until process `pid` has ended, at most `max_ms`. True if it has gone (or was never there).
pub fn wait_for_exit(pid: u32, max_ms: u32) -> bool {
    use windows::Win32::Foundation::WAIT_TIMEOUT;
    use windows::Win32::System::Threading::{WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) else { return true };
        let r = WaitForSingleObject(h, max_ms);
        let _ = CloseHandle(h);
        r != WAIT_TIMEOUT
    }
}

/// An executable's icon as RGBA pixels, `size` x `size`, top row first.
pub fn exe_icon_rgba(path: &str, size: u32) -> Option<Vec<u8>> {
    exe_icon_rgba_at(path, 0, size)
}

/// Icon number `index` of an .exe, .dll or .ico file (a negative index is a resource id,
/// as in a shortcut's or an uninstall entry's "file,-101") as RGBA pixels, `size` x `size`.
pub fn exe_icon_rgba_at(path: &str, index: i32, size: u32) -> Option<Vec<u8>> {
    use windows::Win32::Graphics::Gdi::{CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, PrivateExtractIconsW, HICON, ICONINFO};
    unsafe {
        let mut wide = [0u16; 260];
        let chars: Vec<u16> = path.encode_utf16().collect();
        if chars.len() >= wide.len() {
            return None;
        }
        wide[..chars.len()].copy_from_slice(&chars);
        let mut icon = [HICON::default()];
        let got = PrivateExtractIconsW(&wide, index, size as i32, size as i32, Some(&mut icon), None, 0);
        if got == 0 || got == u32::MAX || icon[0].is_invalid() {
            return None;
        }
        let mut info = ICONINFO::default();
        let mut out = None;
        if GetIconInfo(icon[0], &mut info).is_ok() {
            let dc = CreateCompatibleDC(None);
            let read = |hbm| -> Option<Vec<u8>> {
                let mut bmi = BITMAPINFO::default();
                bmi.bmiHeader = BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: size as i32, biHeight: -(size as i32), biPlanes: 1, biBitCount: 32, ..Default::default() };
                let mut px = vec![0u8; (size * size * 4) as usize];
                let lines = GetDIBits(dc, hbm, 0, size, Some(px.as_mut_ptr() as *mut c_void), &mut bmi, DIB_RGB_COLORS);
                (lines as u32 == size).then_some(px)
            };
            if let Some(mut px) = read(info.hbmColor) {
                // Old icons carry no alpha channel: take transparency from the mask instead.
                if px.chunks_exact(4).all(|p| p[3] == 0) {
                    let mask = read(info.hbmMask);
                    for (i, p) in px.chunks_exact_mut(4).enumerate() {
                        p[3] = match &mask {
                            Some(m) if m[i * 4] != 0 => 0,
                            _ => 255,
                        };
                    }
                }
                for p in px.chunks_exact_mut(4) {
                    p.swap(0, 2); // BGRA -> RGBA
                }
                out = Some(px);
            }
            let _ = DeleteDC(dc);
            let _ = DeleteObject(info.hbmColor);
            let _ = DeleteObject(info.hbmMask);
        }
        let _ = DestroyIcon(icon[0]);
        out
    }
}

/// A graphics card found on this PC.
#[derive(Serialize, Clone, Debug)]
pub struct Gpu {
    pub name: String,
    /// "nvidia" | "amd" | "intel" | "other"
    pub vendor: String,
}

/// What this PC has, and the settings that follow from it.
#[derive(Serialize, Clone, Debug)]
pub struct Hardware {
    pub gpus: Vec<Gpu>,
    pub cpu: String,
    /// The card to encode on: the first NVIDIA, else AMD, else Intel card.
    pub gpu: Option<Gpu>,
    /// Encoder that card provides: "nvenc" | "amf" | "qsv", or "x264" with no usable card.
    pub encoder: String,
    /// Windows build number (`windows_build`), 0 if unknown.
    pub windows_build: u32,
}

/// Read the display adapters and the CPU name from the registry (no WMI, so it is instant).
pub fn hardware() -> Hardware {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let mut gpus: Vec<Gpu> = Vec::new();
    // The "display adapters" device class. Each numbered subkey is one installed adapter.
    if let Ok(class) = hklm.open_subkey(r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}") {
        for sub in class.enum_keys().flatten().filter(|k| k.len() == 4 && k.chars().all(|c| c.is_ascii_digit())) {
            let Ok(key) = class.open_subkey(&sub) else { continue };
            let Ok(name) = key.get_value::<String, _>("DriverDesc") else { continue };
            let id = key.get_value::<String, _>("MatchingDeviceId").unwrap_or_default().to_lowercase();
            let vendor = if id.contains("ven_10de") {
                "nvidia"
            } else if id.contains("ven_1002") || id.contains("ven_1022") {
                "amd"
            } else if id.contains("ven_8086") {
                "intel"
            } else {
                "other" // virtual displays, remote-desktop and capture drivers
            };
            if !gpus.iter().any(|g| g.name == name) {
                gpus.push(Gpu { name, vendor: vendor.to_string() });
            }
        }
    }
    let cpu = hklm
        .open_subkey(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0")
        .and_then(|k| k.get_value::<String, _>("ProcessorNameString"))
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    let gpu = ["nvidia", "amd", "intel"].iter().find_map(|v| gpus.iter().find(|g| g.vendor == *v).cloned());
    let encoder = match gpu.as_ref().map(|g| g.vendor.as_str()) {
        Some("nvidia") => "nvenc",
        Some("amd") => "amf",
        Some("intel") => "qsv",
        _ => "x264",
    };
    Hardware { gpus, cpu, gpu, encoder: encoder.to_string(), windows_build: windows_build() }
}

/// The Windows build number (19045 = Windows 10 22H2, 22000 and up = Windows 11), 0 if unknown.
/// Read from the registry once; no window is needed.
pub fn windows_build() -> u32 {
    static BUILD: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *BUILD.get_or_init(|| {
        use winreg::enums::HKEY_LOCAL_MACHINE;
        use winreg::RegKey;
        RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
            .and_then(|k| k.get_value::<String, _>("CurrentBuildNumber"))
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    })
}

// ---------------------------------------------------------------------------
// The main window's frame (0.1.25)
//
// Glint's window has no title bar of its own (`decorations(false)`), but it is
// still an ordinary resizable window to Windows. On some messages the default
// window procedure repaints the non-client area with the classic themed
// caption (the look of a Windows 7 "Basic" window): the owner saw it flash
// for a few milliseconds while resizing the window fast. The two undocumented
// messages UxTheme sends to draw that caption and frame are swallowed here,
// and WM_NCACTIVATE is passed on with lParam -1, which tells the default
// procedure not to repaint the non-client area. Both are what other
// frameless windows do (Chromium among them). The window's own subclass
// (tao's) still sees every other message unchanged.

#[link(name = "comctl32")]
extern "system" {
    fn SetWindowSubclass(hwnd: isize, proc_: unsafe extern "system" fn(isize, u32, usize, isize, usize, usize) -> isize, id: usize, data: usize) -> i32;
    fn DefSubclassProc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
}

const WM_NCACTIVATE: u32 = 0x0086;
const WM_NCUAHDRAWCAPTION: u32 = 0x00AE;
const WM_NCUAHDRAWFRAME: u32 = 0x00AF;

/// What `calm_frame`'s subclass does with a message: Some(result) = answer it without the default
/// procedure; None = pass it on, with this lParam.
pub fn frame_message(msg: u32, lparam: isize) -> (Option<isize>, isize) {
    match msg {
        WM_NCUAHDRAWCAPTION | WM_NCUAHDRAWFRAME => (Some(0), lparam),
        WM_NCACTIVATE => (None, -1),
        _ => (None, lparam),
    }
}

unsafe extern "system" fn frame_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize, _id: usize, _data: usize) -> isize {
    match frame_message(msg, lparam) {
        (Some(result), _) => result,
        (None, lparam) => DefSubclassProc(hwnd, msg, wparam, lparam),
    }
}

/// Keep the classic caption from flashing on window `hwnd`. Must run on the thread that owns
/// the window (the event loop's). False if Windows refused the subclass.
pub fn calm_frame(hwnd: isize) -> bool {
    unsafe { SetWindowSubclass(hwnd, frame_proc, 0x676c_696e, 0) != 0 }
}