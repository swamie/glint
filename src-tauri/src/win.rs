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
        let mut wr = RECT::default();
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let fullscreen = GetWindowRect(hwnd, &mut wr).is_ok()
            && GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool()
            && wr.left <= mi.rcMonitor.left
            && wr.top <= mi.rcMonitor.top
            && wr.right >= mi.rcMonitor.right
            && wr.bottom >= mi.rcMonitor.bottom;
        Some(Foreground { pid, exe, title, fullscreen })
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

/// An executable's icon as RGBA pixels, `size` x `size`, top row first.
pub fn exe_icon_rgba(path: &str, size: u32) -> Option<Vec<u8>> {
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
        let got = PrivateExtractIconsW(&wide, 0, size as i32, size as i32, Some(&mut icon), None, 0);
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
    Hardware { gpus, cpu, gpu, encoder: encoder.to_string() }
}
