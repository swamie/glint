//! Live verification, test-only (0.1.41): `glint.exe --live-check <dir>`. A normal run never takes
//! this path. It reads the real screen, windows and libraries and runs three short real captures of
//! a window this program starts itself (`--live-window`), writing pictures and a report into <dir>.
//! No sound, no window takes focus (the test windows are no-activate), no other process is touched.
use crate::recorder::{capture_args, retarget, CaptureCfg};
use crate::{gamedetect, games, monprev, util, win};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

pub fn dpi_aware() -> bool {
    use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_ok() }
}

/// `glint.exe --live-window <x> <y> <w> <h> <rgb hex> <affinity> <secs>`: one plain no-activate
/// window of one color, for the live check's captures. Exits by itself.
pub fn window_main(args: &[String]) {
    use windows::core::w;
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::CreateSolidBrush;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe extern "system" fn proc_(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        DefWindowProcW(h, m, wp, lp)
    }
    let n = |i: usize, d: i64| args.get(i).and_then(|s| s.parse::<i64>().ok()).unwrap_or(d);
    let (x, y, wd, ht) = (n(0, 100) as i32, n(1, 100) as i32, n(2, 640) as i32, n(3, 360) as i32);
    let rgb = args.get(4).and_then(|s| u32::from_str_radix(s, 16).ok()).unwrap_or(0xff00ff);
    let affinity = n(5, 0) as u32;
    let secs = n(6, 20) as u64;
    dpi_aware();
    unsafe {
        let inst = GetModuleHandleW(None).unwrap_or_default();
        let bgr = ((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff);
        let wc = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: inst.into(), lpszClassName: w!("GlintLiveCheck"), hbrBackground: CreateSolidBrush(COLORREF(bgr)), ..Default::default() };
        RegisterClassW(&wc);
        let Ok(h) = CreateWindowExW(WS_EX_NOACTIVATE | WS_EX_TOPMOST, w!("GlintLiveCheck"), w!("Glint live check"), WS_POPUP, x, y, wd, ht, None, None, windows::Win32::Foundation::HINSTANCE::from(inst), None) else { return };
        if affinity != 0 {
            let _ = SetWindowDisplayAffinity(h, WINDOW_DISPLAY_AFFINITY(affinity));
        }
        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
        // Repainted 30 times a second in two near shades: Graphics Capture only sends frames of a window that changes.
        let shade = ((bgr & 0xfefefe) >> 1) + ((bgr & 0xfefefe) >> 2) + ((bgr & 0xfefefe) >> 3); // about 7/8 of the color
        let brushes = [CreateSolidBrush(COLORREF(bgr)), CreateSolidBrush(COLORREF(shade))];
        let end = Instant::now() + Duration::from_secs(secs);
        let mut msg = MSG::default();
        let mut k = 0usize;
        while Instant::now() < end {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let dc = windows::Win32::Graphics::Gdi::GetDC(h);
            let rc = windows::Win32::Foundation::RECT { left: 0, top: 0, right: wd, bottom: ht };
            windows::Win32::Graphics::Gdi::FillRect(dc, &rc, brushes[k % 2]);
            windows::Win32::Graphics::Gdi::ReleaseDC(h, dc);
            k += 1;
            std::thread::sleep(Duration::from_millis(33));
        }
        let _ = DestroyWindow(h);
    }
}

fn cfg(ffmpeg: &Path, monitor: u32, w: u32, h: u32, hz: u32, method: &str, window: u64) -> CaptureCfg {
    CaptureCfg {
        ffmpeg: Some(ffmpeg.to_path_buf()),
        monitor,
        width: w,
        height: h,
        scaled: false,
        fps: 30,
        hz,
        quality: "medium".into(),
        bitrate: 0,
        encoder: "x264".into(),
        method: method.into(),
        cursor: false,
        desktop_audio: false,
        mic: false,
        max_secs: 10.0,
        buffer: "ram".into(),
        buffer_ram_secs: 10.0,
        buffer_dir: util::tmp_dir(),
        window,
    }
}

/// Run a capture's ffmpeg arguments for `secs` into `out` (MPEG-TS). (ok, last stderr line, ms).
fn run_capture(ffmpeg: &Path, mut args: Vec<String>, secs: u32, out: &Path) -> (bool, String, u128) {
    if args.last().map(String::as_str) == Some("pipe:1") {
        args.pop();
    }
    args.extend(["-t".to_string(), secs.to_string(), "-y".to_string(), out.to_string_lossy().into_owned()]);
    let t = Instant::now();
    let child = util::hidden_cmd(ffmpeg).args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn();
    let Ok(mut child) = child else { return (false, "ffmpeg did not start".into(), 0) };
    let deadline = Instant::now() + Duration::from_secs(secs as u64 + 15);
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let out_s = child.wait_with_output().map(|o| String::from_utf8_lossy(&o.stderr).into_owned()).unwrap_or_default();
    let last = out_s.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("").to_string();
    (out.metadata().is_ok_and(|m| m.len() > 1000), last, t.elapsed().as_millis())
}

/// Frames in a file, and the mean RGB of a `crop` (w:h:x:y, or "" for all) of the frame at 1.5 s; one PNG of that frame.
fn inspect(ffmpeg: &Path, file: &Path, crop: &str, png: &Path) -> (u64, Option<(u8, u8, u8)>) {
    // Frames: decode the whole file, count what comes out.
    let frames = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-i"]).arg(file).args(["-map", "0:v", "-f", "framecrc", "-"]).stdin(Stdio::null()).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).lines().filter(|l| !l.starts_with('#')).count() as u64).unwrap_or(0);
    let _ = util::hidden_cmd(ffmpeg).args(["-v", "error", "-y", "-i"]).arg(file).args(["-frames:v", "1"]).arg(png).stdin(Stdio::null()).output();
    let vf = if crop.is_empty() { "scale=1:1:flags=area".to_string() } else { format!("crop={crop},scale=1:1:flags=area") };
    let px = util::hidden_cmd(ffmpeg).args(["-v", "error", "-i"]).arg(file).args(["-frames:v", "1", "-vf", &vf, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]).stdin(Stdio::null()).output().ok().map(|o| o.stdout);
    (frames, px.filter(|p| p.len() >= 3).map(|p| (p[0], p[1], p[2])))
}

/// Within the two shades the test window alternates between, after H.264 and 4:2:0.
fn near(a: Option<(u8, u8, u8)>, b: (u8, u8, u8)) -> bool {
    a.is_some_and(|a| (a.0 as i32 - b.0 as i32).abs() < 70 && (a.1 as i32 - b.1 as i32).abs() < 70 && (a.2 as i32 - b.2 as i32).abs() < 70)
}

pub fn run(dir: &str) {
    let dir = PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let mut r = String::new();
    let aware = dpi_aware();
    let _ = writeln!(r, "process DPI awareness set to per-monitor v2 (as the app's window library does): {aware}");

    // (1) Every monitor's picture, numbered as `win::monitors()` and named to ffmpeg by handle.
    let mons = win::monitors();
    let _ = writeln!(r, "\n== (1) monitors: {}", mons.len());
    for m in &mons {
        let src = win::monitor_source(m.index);
        let t = Instant::now();
        let g = monprev::grab_raw(m.index, 320);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let t2 = Instant::now();
        let full = monprev::grab(m.index, 320).map(|u| u.len()).unwrap_or(0);
        let ms_full = t2.elapsed().as_secs_f64() * 1000.0;
        match g {
            Ok((px, w, h, rect)) => {
                let file = dir.join(format!("monitor-{}{}.png", m.index + 1, if m.primary { "-main" } else { "" }));
                let _ = std::fs::write(&file, monprev::png_rgb(&px, w as u32, h as u32).unwrap_or_default());
                let mean: u64 = px.iter().map(|v| *v as u64).sum::<u64>() / px.len().max(1) as u64;
                let _ = writeln!(r, "monitor {} {}{}: mode {}x{} @{} Hz at ({},{}); hmonitor {:#x}, dxgi output {:?}; grabbed rect ({},{}) {}x{} -> {}x{} px; whole monitor: {}; mean level {}; grab {:.1} ms, grab+PNG+base64 {:.1} ms ({} bytes) -> {}",
                    m.index + 1, m.name, if m.primary { " (main)" } else { "" }, m.width, m.height, m.hz, m.x, m.y, src.as_ref().map_or(0, |s| s.hmonitor), src.as_ref().and_then(|s| s.dxgi_output),
                    rect.0, rect.1, rect.2, rect.3, w, h, rect.2 as u32 == m.width && rect.3 as u32 == m.height && rect.0 == m.x && rect.1 == m.y, mean, ms, ms_full, full, file.file_name().unwrap().to_string_lossy());
            }
            Err(e) => {
                let _ = writeln!(r, "monitor {} {}: grab failed: {e}", m.index + 1, m.name);
            }
        }
    }

    // (2) The real desktop: the window snapshot, the library index, the decision.
    let _ = writeln!(r, "\n== (2) detection");
    let t = Instant::now();
    let lib = gamedetect::scan();
    let lib_ms = t.elapsed().as_millis();
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for i in &lib.installs {
        match counts.iter_mut().find(|(s, _)| *s == i.store) {
            Some(c) => c.1 += 1,
            None => counts.push((i.store, 1)),
        }
    }
    let _ = writeln!(r, "library index: {} installs in {lib_ms} ms: {counts:?}", lib.installs.len());
    for i in &lib.installs {
        let _ = writeln!(r, "  {}: {}", i.store, i.name);
    }
    let t = Instant::now();
    let wins = win::windows();
    let wins_ms = t.elapsed().as_secs_f64() * 1000.0;
    let _ = writeln!(r, "windows: {} in {wins_ms:.1} ms; shell says D3D fullscreen now: {}", wins.len(), win::d3d_fullscreen());
    for w in &wins {
        let _ = writeln!(r, "  {:<34} class {:<28} {}x{} mon {} popup {} caption {} full {} min {} cloaked {} fg {} excluded {} hosted {} library {:?}",
            games::exe_key(&w.exe), w.class.chars().take(28).collect::<String>(), w.width, w.height, w.monitor + 1, w.popup as u8, w.caption as u8, w.fullscreen as u8, w.minimized as u8, w.cloaked as u8, w.foreground as u8, w.excluded as u8, w.hosted as u8, lib.find(&w.exe).map(|i| i.store));
    }
    let env = gamedetect::Env { window_capture: true, dda: false, d3d_fullscreen: win::d3d_fullscreen(), now: 0.0 };
    let d = gamedetect::decide(&wins, &lib, &gamedetect::Prefs::none(), &env, &mut gamedetect::Memory::default(), &win::process_alive);
    let _ = writeln!(r, "decide (detection on, nothing picked): {:?} -> {}", d.source, d.why.line());
    for c in &d.candidates {
        let _ = writeln!(r, "  candidate {} ({}) score {} {:?}", c.exe, c.name, c.score, c.evidence);
    }

    // (3) Real captures of a window this program starts: window by handle, its monitor by handle, Desktop Duplication.
    let _ = writeln!(r, "\n== (3) captures (x264 on the CPU, 3 s each)");
    let Some(ffmpeg) = util::find_exe("", "ffmpeg") else {
        let _ = writeln!(r, "no ffmpeg");
        return finish(&dir, &r);
    };
    let m0 = mons.iter().find(|m| m.primary).cloned().unwrap_or_else(|| mons[0].clone());
    let exe = std::env::current_exe().unwrap_or_default();
    let (wx, wy) = (m0.x + 120, m0.y + 120);
    let spawn = |x: i32, y: i32, rgb: &str, aff: u32| util::hidden_cmd(&exe).args(["--live-window", &x.to_string(), &y.to_string(), "640", "360", rgb, &aff.to_string(), "90"]).stdin(Stdio::null()).spawn().ok();
    let mut kids = Vec::new();
    if let Some(c) = spawn(wx, wy, "ff00ff", 0) {
        kids.push(c);
    }
    if let Some(c) = spawn(wx + 700, wy, "00c0ff", 0x11) {
        kids.push(c);
    }
    std::thread::sleep(Duration::from_millis(1500));
    let pids: Vec<u32> = kids.iter().map(|k| k.id()).collect();
    let all = win::windows();
    let mine: Vec<&win::WinInfo> = all.iter().filter(|w| pids.contains(&w.pid)).collect();
    let _ = writeln!(r, "test windows seen by win::windows(): {}", mine.len());
    for w in &mine {
        let _ = writeln!(r, "  pid {} {}x{} on monitor {} excluded {} popup {} caption {}", if w.pid == pids[0] { "A (plain, magenta)" } else { "B (WDA_EXCLUDEFROMCAPTURE, cyan)" }, w.width, w.height, w.monitor + 1, w.excluded as u8, w.popup as u8, w.caption as u8);
    }
    let a = mine.iter().find(|w| w.pid == pids.first().copied().unwrap_or(0)).map(|w| w.hwnd);
    let b = mine.iter().find(|w| Some(w.pid) == pids.get(1).copied()).map(|w| w.hwnd);
    let no_border = crate::recorder::gfx_caps(&ffmpeg).1;
    let src = win::monitor_source(m0.index);
    let crop_a = format!("64:64:{}:{}", 120 + 288, 120 + 148); // inside window A, in the monitor picture
    let crop_b = format!("64:64:{}:{}", 820 + 288, 120 + 148);
    let mut runs: Vec<(String, Vec<String>, String, String)> = Vec::new();
    if let Some(h) = a {
        runs.push(("window-A-gfxcapture".into(), capture_args(&cfg(&ffmpeg, m0.index, 640, 360, m0.hz, "wgc", h), "x264", "wgc", false, no_border), String::new(), "magenta".into()));
    }
    if let Some(h) = b {
        runs.push(("window-B-excluded-gfxcapture".into(), capture_args(&cfg(&ffmpeg, m0.index, 640, 360, m0.hz, "wgc", h), "x264", "wgc", false, no_border), String::new(), "excluded (expect black / nothing)".into()));
    }
    let mon_wgc = retarget(capture_args(&cfg(&ffmpeg, m0.index, m0.width, m0.height, m0.hz, "wgc", 0), "x264", "wgc", false, no_border), m0.index, src.as_ref());
    runs.push(("monitor-gfxcapture-by-handle".into(), mon_wgc, crop_a.clone(), "magenta at A, B hidden".into()));
    let mon_dda = retarget(capture_args(&cfg(&ffmpeg, m0.index, m0.width, m0.height, m0.hz, "dda", 0), "x264", "dda", false, false), m0.index, src.as_ref());
    runs.push(("monitor-ddagrab".into(), mon_dda, crop_a.clone(), "magenta at A, B hidden".into()));
    for (name, args, crop, want) in runs {
        let src_arg = args.iter().position(|x| x == "-filter_complex").and_then(|i| args.get(i + 1)).map(|s| s.split(',').next().unwrap_or("").to_string()).unwrap_or_default();
        let file = dir.join(format!("{name}.ts"));
        let (ok, last, ms) = run_capture(&ffmpeg, args, 3, &file);
        let (frames, rgb) = inspect(&ffmpeg, &file, &crop, &dir.join(format!("{name}.png")));
        let rgb_b = if name.starts_with("monitor") { inspect(&ffmpeg, &file, &crop_b, &dir.join(format!("{name}-B.png"))).1 } else { None };
        let verdict = if name == "window-A-gfxcapture" || name.starts_with("monitor") { if near(rgb, (255, 0, 255)) { "RIGHT: the test window's color" } else { "WRONG color" } } else if rgb.map_or(true, |c| c.0 < 30 && c.1 < 30 && c.2 < 30) { "black / no picture, as excluded" } else if near(rgb, (0, 192, 255)) { "the excluded window WAS captured" } else { "other" };
        let _ = writeln!(r, "{name}: source `{src_arg}`; file ok {ok}, {frames} frames, {ms} ms; sample {rgb:?}{} -> {verdict} (want {want}); ffmpeg last line: {}", rgb_b.map(|c| format!(", B area {c:?}")).unwrap_or_default(), last.chars().take(160).collect::<String>());
    }
    for mut k in kids {
        let _ = k.kill(); // only the windows this check started
        let _ = k.wait();
    }

    // (4) The shell's notification state and display affinity on the real desktop.
    let _ = writeln!(r, "\n== (4) shell state and affinity");
    let raw = unsafe { windows::Win32::UI::Shell::SHQueryUserNotificationState() }.map(|s| s.0).unwrap_or(-1);
    let _ = writeln!(r, "SHQueryUserNotificationState = {raw} (1 not present, 2 busy, 3 D3D fullscreen, 4 presentation, 5 accepts notifications, 6 quiet time, 7 app)");
    let ex: Vec<String> = win::windows().iter().filter(|w| w.excluded).map(|w| games::exe_key(&w.exe)).collect();
    let _ = writeln!(r, "windows that keep themselves out of capture now: {ex:?}");
    finish(&dir, &r);
}

fn finish(dir: &Path, r: &str) {
    let _ = std::fs::write(dir.join("report.txt"), r);
}
