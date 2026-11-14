//! GlintSetup.exe's windows: the setup wizard and the small "Remove Glint" window.
//!
//! Plain Win32 controls through the `windows` crate (no web view: WebView2 may be missing, this is
//! what installs it). Visual styles (Common Controls 6) come from the exe's manifest, or from an
//! activation context made at start when the manifest lacks them; per-monitor DPI awareness (v2)
//! is switched on at run time, and every size below is scaled from 96 DPI.
//!
//! Pages: Welcome -> Options -> Installing -> Finished | Failed; and Remove -> Removed.
//! One window, one set of controls: each control belongs to some pages, and `set_page` shows those.
//! The install runs on a thread (setup.rs `install`), which reports through `Shared` and posts
//! WM_APP_PROG / WM_APP_DONE to the window, so the window never freezes.
//!
//! `--ui-test <folder>` (`ui_test`) builds the windows HIDDEN (no ShowWindow on the top-level window,
//! ever), walks every page and state, draws each into a PNG, and writes report.txt.
use super::*;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{BOOL, COLORREF, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Window styles and messages as plain numbers (the crate's typed constants differ per module).
mod k {
    pub const WS_CHILD: u32 = 0x4000_0000;
    pub const WS_VISIBLE: u32 = 0x1000_0000;
    pub const WS_DISABLED: u32 = 0x0800_0000;
    pub const WS_CLIPCHILDREN: u32 = 0x0200_0000;
    pub const WS_CAPTION: u32 = 0x00C0_0000;
    pub const WS_SYSMENU: u32 = 0x0008_0000;
    pub const WS_MINIMIZEBOX: u32 = 0x0002_0000;
    pub const WS_TABSTOP: u32 = 0x0001_0000;
    pub const WS_EX_CONTROLPARENT: u32 = 0x0001_0000;
    pub const SS_RIGHT: u32 = 0x02;
    pub const SS_NOPREFIX: u32 = 0x80;
    pub const SS_EDITCONTROL: u32 = 0x2000;
    pub const SS_ENDELLIPSIS: u32 = 0x4000;
    pub const BS_PUSHBUTTON: u32 = 0x00;
    pub const BS_DEFPUSHBUTTON: u32 = 0x01;
    pub const BS_AUTOCHECKBOX: u32 = 0x03;
    pub const BS_MULTILINE: u32 = 0x2000;
    pub const ES_AUTOHSCROLL: u32 = 0x80;
    pub const PBS_MARQUEE: u32 = 0x08;
    pub const PBM_SETPOS: u32 = 0x0402;
    pub const PBM_SETRANGE32: u32 = 0x0406;
    pub const PBM_SETMARQUEE: u32 = 0x040A;
    pub const BM_GETCHECK: u32 = 0x00F0;
    pub const BM_SETCHECK: u32 = 0x00F1;
    pub const WM_SETFONT: u32 = 0x0030;
    pub const WM_GETFONT: u32 = 0x0031;
    pub const WM_SETICON: u32 = 0x0080;
    pub const DM_GETDEFID: u32 = 0x0400;
    pub const DC_HASDEFID: isize = 0x534B;
    pub const BN_CLICKED: u32 = 0;
    pub const EN_CHANGE: u32 = 0x0300;
    pub const PRF_NONCLIENT: isize = 0x02;
    pub const PRF_CLIENT: isize = 0x04;
    pub const PRF_ERASEBKGND: isize = 0x08;
    pub const PRF_CHILDREN: isize = 0x10;
    pub const VK_RETURN: usize = 0x0D;
    pub const VK_ESCAPE: usize = 0x1B;
    pub const ICC_PROGRESS_CLASS: u32 = 0x20;
    pub const ICC_STANDARD_CLASSES: u32 = 0x4000;
}

const WM_APP_PROG: u32 = 0x8001;
const WM_APP_DONE: u32 = 0x8002;

const ID_NEXT: i32 = 1; // IDOK: Enter
const ID_CANCEL: i32 = 2; // IDCANCEL: Esc
const ID_BACK: i32 = 100;
const ID_BROWSE: i32 = 101;
const ID_FF: i32 = 102;
const ID_BLUR: i32 = 103;
const ID_START: i32 = 104;
const ID_DESK: i32 = 105;
const ID_LAUNCH: i32 = 106;
const ID_PURGE: i32 = 107;
const ID_DIR: i32 = 108;

/// The Glint icon in the exe's resources (Tauri's build puts it there under this id).
const ICON_ID: usize = 32512;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Welcome,
    Options,
    Installing,
    Finished,
    Failed,
    Remove,
    Removed,
}

const ALL: &[Page] = &[Page::Welcome, Page::Options, Page::Installing, Page::Finished, Page::Failed, Page::Remove, Page::Removed];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Title,
    Sub,
    Label,
    Bold,
    Wrap,
    Hint,
    HintWrap,
    Error,
    Button,
    Check,
    Edit,
    Progress,
}

struct Ctl {
    hwnd: HWND,
    name: &'static str,
    kind: Kind,
    pages: &'static [Page],
}

// ---------------------------------------------------------------- the worker's side

#[derive(Clone, Default)]
struct Prog {
    step: String,
    index: usize,
    total: usize,
    done: u64,
    size: u64,
    speed: f64,
    busy: String,
    can_cancel: bool,
}

enum Done {
    Install(Result<Outcome, String>),
    Remove(Removal),
    Probe(Probe),
}

/// What the worker thread and the window share. The thread never touches a window: it updates this
/// and posts a message (one at a time, so a fast download does not flood the queue).
struct Shared {
    hwnd: isize,
    prog: Mutex<Prog>,
    cancel: AtomicBool,
    posted: AtomicBool,
    done: Mutex<Vec<Done>>,
}

impl Shared {
    fn post(&self, msg: u32) {
        unsafe {
            let _ = PostMessageW(HWND(self.hwnd as *mut c_void), msg, WPARAM(0), LPARAM(0));
        }
    }
    fn poke(&self) {
        if !self.posted.swap(true, Ordering::AcqRel) {
            self.post(WM_APP_PROG);
        }
    }
    fn finish(&self, d: Done) {
        self.done.lock().unwrap().push(d);
        self.post(WM_APP_DONE);
    }
}

impl Sink for Shared {
    fn step(&self, text: &str, index: usize, total: usize) {
        let mut p = self.prog.lock().unwrap();
        p.step = text.to_string();
        p.index = index;
        p.total = total;
        p.done = 0;
        p.size = 0;
        p.speed = 0.0;
        p.busy.clear();
        drop(p);
        self.poke();
    }
    fn bytes(&self, done: u64, total: u64, per_sec: f64) {
        let mut p = self.prog.lock().unwrap();
        p.done = done;
        p.size = total;
        p.speed = per_sec;
        p.busy.clear();
        drop(p);
        self.poke();
    }
    fn busy(&self, text: &str) {
        self.prog.lock().unwrap().busy = text.to_string();
        self.poke();
    }
    fn can_cancel(&self, yes: bool) {
        self.prog.lock().unwrap().can_cancel = yes;
        self.poke();
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
}

// ---------------------------------------------------------------- start-up: visual styles, DPI

const CC6_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency><dependentAssembly>
    <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
  </dependentAssembly></dependency>
</assembly>"#;

/// How the windows got their look; filled by `init_ui`, shown in the test report.
#[derive(Default, Clone)]
struct Env {
    styles_from: String,
    comctl_before: u32,
    comctl_after: u32,
    dpi_call: bool,
}

/// The major version of the comctl32.dll that the current activation context loads (5 = old look).
fn comctl_major() -> u32 {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::Win32::UI::Shell::DLLVERSIONINFO;
    unsafe {
        let Ok(h) = LoadLibraryW(w!("comctl32.dll")) else { return 0 };
        let Some(f) = GetProcAddress(h, windows::core::s!("DllGetVersion")) else { return 0 };
        let f: unsafe extern "system" fn(*mut DLLVERSIONINFO) -> windows::core::HRESULT = std::mem::transmute(f);
        let mut v = DLLVERSIONINFO { cbSize: std::mem::size_of::<DLLVERSIONINFO>() as u32, ..Default::default() };
        if f(&mut v).is_ok() { v.dwMajorVersion } else { 0 }
    }
}

/// Make Common Controls 6 the active version for this thread (and so for every window it makes).
fn activate_cc6() -> bool {
    use windows::Win32::System::ApplicationInstallationAndServicing::{ActivateActCtx, CreateActCtxW, ACTCTXW};
    let path = std::env::temp_dir().join(format!("GlintSetup-{}.manifest", std::process::id()));
    if fs::write(&path, CC6_MANIFEST).is_err() {
        return false;
    }
    let src = HSTRING::from(path.as_os_str());
    let ok = unsafe {
        let mut ctx: ACTCTXW = std::mem::zeroed();
        ctx.cbSize = std::mem::size_of::<ACTCTXW>() as u32;
        ctx.lpSource = PCWSTR(src.as_ptr());
        match CreateActCtxW(&ctx) {
            Ok(h) => {
                let mut cookie = 0usize;
                ActivateActCtx(h, &mut cookie).is_ok() // kept active for the life of the process
            }
            Err(_) => false,
        }
    };
    let _ = fs::remove_file(&path);
    ok
}

fn init_ui() -> Env {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    let mut env = Env::default();
    unsafe {
        env.dpi_call = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_ok();
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    env.comctl_before = comctl_major();
    env.styles_from = if env.comctl_before >= 6 {
        "the exe's own manifest".into()
    } else if activate_cc6() {
        "an activation context made at start (the exe's manifest lacks them)".into()
    } else {
        "nowhere: old-style controls".into()
    };
    env.comctl_after = comctl_major();
    // Register the progress bar class from the active comctl32 (looked up, not linked, so the
    // version that loads is the one the activation context picks).
    #[repr(C)]
    struct Icc {
        size: u32,
        icc: u32,
    }
    unsafe {
        if let Ok(h) = LoadLibraryW(w!("comctl32.dll")) {
            if let Some(f) = GetProcAddress(h, windows::core::s!("InitCommonControlsEx")) {
                let f: unsafe extern "system" fn(*const Icc) -> BOOL = std::mem::transmute(f);
                let _ = f(&Icc { size: 8, icc: k::ICC_PROGRESS_CLASS | k::ICC_STANDARD_CLASSES });
            }
        }
    }
    env
}

fn hinst() -> HINSTANCE {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    unsafe { GetModuleHandleW(PCWSTR::null()).map(|m| HINSTANCE(m.0)).unwrap_or_default() }
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

fn text_of(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, &mut buf);
        String::from_utf16_lossy(&buf[..got.max(0) as usize])
    }
}

fn style_of(h: HWND) -> u32 {
    unsafe { GetWindowLongPtrW(h, GWL_STYLE) as u32 }
}

// ---------------------------------------------------------------- the window

pub struct Ui {
    hwnd: HWND,
    remove_mode: bool,
    test: bool,
    page: Page,
    dpi: u32,
    font: HFONT,
    bold: HFONT,
    title: HFONT,
    icon: HICON,
    icon_px: i32,
    white: HBRUSH,
    footer: HBRUSH,
    line: HBRUSH,
    frame: HBRUSH,
    frame_bottom: HBRUSH,
    edit_box: RECT,
    ctls: Vec<Ctl>,
    o: Opts,
    probe: Probe,
    shared: Arc<Shared>,
    working: bool,
    marquee: bool,
    options_for: Option<PathBuf>,
    outcome: Option<Outcome>,
    error: String,
    removal: Option<Removal>,
    glint_running_test: bool,
    // Test hooks: confirmations are recorded instead of shown, and answered with `answer_yes`.
    asked: Vec<String>,
    answer_yes: bool,
    closed: bool,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = lp.0 as *const CREATESTRUCTW;
        let ui = (*cs).lpCreateParams as *mut Ui;
        (*ui).hwnd = hwnd;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ui as isize);
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Ui;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let ui = &mut *ptr;
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            ui.paint(hdc);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_PRINTCLIENT => {
            ui.paint(HDC(wp.0 as *mut c_void));
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => ui.ctl_color(HDC(wp.0 as *mut c_void), HWND(lp.0 as *mut c_void)),
        WM_CTLCOLORBTN => LRESULT(ui.footer.0 as isize),
        WM_COMMAND => {
            ui.command((wp.0 & 0xFFFF) as i32, ((wp.0 >> 16) & 0xFFFF) as u32);
            LRESULT(0)
        }
        k::DM_GETDEFID => LRESULT((k::DC_HASDEFID << 16) | ID_NEXT as isize),
        WM_APP_PROG => {
            ui.on_prog();
            LRESULT(0)
        }
        WM_APP_DONE => {
            ui.on_done();
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let r = *(lp.0 as *const RECT);
            ui.set_dpi((wp.0 & 0xFFFF) as u32);
            let _ = SetWindowPos(hwnd, HWND::default(), r.left, r.top, 0, 0, SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSIZE);
            LRESULT(0)
        }
        WM_CLOSE => {
            ui.on_cancel();
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn register_class() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst(),
            hCursor: LoadCursorW(HINSTANCE::default(), IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("GlintSetupWindow"),
            ..Default::default()
        };
        RegisterClassExW(&wc);
    });
}

/// Build the window, hidden. `remove_mode` = the "Remove Glint" window.
fn create(o: Opts, remove_mode: bool, test: bool) -> Result<Box<Ui>, String> {
    register_class();
    let mut ui = Box::new(Ui {
        hwnd: HWND::default(),
        remove_mode,
        test,
        page: if remove_mode { Page::Remove } else { Page::Welcome },
        dpi: 96,
        font: HFONT::default(),
        bold: HFONT::default(),
        title: HFONT::default(),
        icon: HICON::default(),
        icon_px: 40,
        white: unsafe { CreateSolidBrush(rgb(255, 255, 255)) },
        footer: unsafe { CreateSolidBrush(rgb(243, 243, 243)) },
        line: unsafe { CreateSolidBrush(rgb(229, 229, 229)) },
        frame: unsafe { CreateSolidBrush(rgb(200, 200, 200)) },
        frame_bottom: unsafe { CreateSolidBrush(rgb(134, 134, 134)) },
        edit_box: RECT::default(),
        ctls: Vec::new(),
        o,
        probe: Probe::default(),
        shared: Arc::new(Shared { hwnd: 0, prog: Mutex::new(Prog::default()), cancel: AtomicBool::new(false), posted: AtomicBool::new(false), done: Mutex::new(Vec::new()) }),
        working: false,
        marquee: false,
        options_for: None,
        outcome: None,
        error: String::new(),
        removal: None,
        glint_running_test: false,
        asked: Vec::new(),
        answer_yes: false,
        closed: false,
    });
    // Start on the monitor under the mouse, so the window gets that monitor's DPI from the start.
    let mut at = POINT { x: 0, y: 0 };
    unsafe {
        let mut cur = POINT::default();
        if !test && GetCursorPos(&mut cur).is_ok() {
            let mon = MonitorFromPoint(cur, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(mon, &mut mi).as_bool() {
                at = POINT { x: mi.rcWork.left + 40, y: mi.rcWork.top + 40 };
            }
        }
    }
    let title = if remove_mode { w!("Remove Glint") } else { w!("Glint Setup") };
    let ptr: *mut Ui = &mut *ui;
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(k::WS_EX_CONTROLPARENT),
            w!("GlintSetupWindow"),
            title,
            WINDOW_STYLE(k::WS_CAPTION | k::WS_SYSMENU | k::WS_MINIMIZEBOX | k::WS_CLIPCHILDREN),
            at.x,
            at.y,
            300,
            200,
            HWND::default(),
            HMENU::default(),
            hinst(),
            Some(ptr as *const c_void),
        )
    }
    .map_err(|e| format!("The window could not be made: {}", e.message()))?;
    ui.hwnd = hwnd;
    Arc::get_mut(&mut ui.shared).map(|s| s.hwnd = hwnd.0 as isize);
    ui.dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96);
    ui.make_fonts();
    ui.make_controls();
    ui.apply_fonts();
    unsafe {
        let _ = SendMessageW(ui.h("i_bar"), k::PBM_SETRANGE32, WPARAM(0), LPARAM(1000));
    }
    ui.refresh_texts();
    let first = ui.page;
    ui.set_page(first);
    Ok(ui)
}

impl Ui {
    fn s(&self, v: i32) -> i32 {
        v * self.dpi as i32 / 96
    }

    fn h(&self, name: &str) -> HWND {
        self.ctls.iter().find(|c| c.name == name).map(|c| c.hwnd).unwrap_or_default()
    }

    fn set_text(&self, name: &str, text: &str) {
        let h = self.h(name);
        if text_of(h) != text {
            unsafe {
                let _ = SetWindowTextW(h, &HSTRING::from(text));
            }
        }
    }

    fn checked(&self, name: &str) -> bool {
        unsafe { SendMessageW(self.h(name), k::BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == 1 }
    }

    fn set_check(&self, name: &str, on: bool) {
        unsafe {
            SendMessageW(self.h(name), k::BM_SETCHECK, WPARAM(on as usize), LPARAM(0));
        }
    }

    fn enable(&self, name: &str, on: bool) {
        unsafe {
            let _ = EnableWindow(self.h(name), on);
        }
    }

    fn add(&mut self, class: PCWSTR, name: &'static str, kind: Kind, pages: &'static [Page], text: &str, style: u32, ex: u32, id: i32) {
        let hwnd = unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(ex), class, &HSTRING::from(text), WINDOW_STYLE(k::WS_CHILD | style), 0, 0, 10, 10, self.hwnd, HMENU(id as isize as *mut c_void), hinst(), None)
        }
        .unwrap_or_default();
        self.ctls.push(Ctl { hwnd, name, kind, pages });
    }

    /// Every control, in tab order (creation order = z-order = tab order).
    fn make_controls(&mut self) {
        use Page::*;
        let st = w!("STATIC");
        let bt = w!("BUTTON");
        let line = k::SS_NOPREFIX | k::SS_ENDELLIPSIS;
        let wrap = k::SS_NOPREFIX | k::SS_EDITCONTROL;
        let chk = k::WS_TABSTOP | k::BS_AUTOCHECKBOX | k::BS_MULTILINE;
        self.add(st, "title", Kind::Title, ALL, "", line, 0, -1);
        self.add(st, "sub", Kind::Sub, ALL, "", line, 0, -1);
        // Welcome
        self.add(st, "w_intro", Kind::Wrap, &[Welcome], "Setup downloads Glint from its GitHub page and installs it for your Windows account. No administrator rights are needed.", wrap, 0, -1);
        self.add(st, "w_head", Kind::Bold, &[Welcome], "What it installs", line, 0, -1);
        self.add(st, "w_list", Kind::Wrap, &[Welcome], "", wrap, 0, -1);
        self.add(st, "w_dir_label", Kind::Bold, &[Welcome], "Install folder", line, 0, -1);
        self.add(w!("EDIT"), "w_dir", Kind::Edit, &[Welcome], &self.o.dir.to_string_lossy().to_string(), k::WS_TABSTOP | k::ES_AUTOHSCROLL, 0, ID_DIR);
        self.add(bt, "w_browse", Kind::Button, &[Welcome], "Browse…", k::WS_TABSTOP | k::BS_PUSHBUTTON, 0, ID_BROWSE);
        self.add(st, "w_space", Kind::Hint, &[Welcome], "", line, 0, -1);
        self.add(st, "w_dir_err", Kind::Error, &[Welcome], "", wrap, 0, -1);
        // Options
        self.add(bt, "o_ff", Kind::Check, &[Options], "ffmpeg (records and saves the clips)", chk, 0, ID_FF);
        self.add(st, "o_ff_state", Kind::HintWrap, &[Options], "", wrap, 0, -1);
        self.add(bt, "o_blur", Kind::Check, &[Options], "Blur (optional: motion-blur renders of finished clips)", chk, 0, ID_BLUR);
        self.add(st, "o_blur_state", Kind::HintWrap, &[Options], "", wrap, 0, -1);
        self.add(bt, "o_start", Kind::Check, &[Options], "Start menu shortcut", chk, 0, ID_START);
        self.add(bt, "o_desk", Kind::Check, &[Options], "Desktop shortcut", chk, 0, ID_DESK);
        self.add(st, "o_wv", Kind::HintWrap, &[Options], "", wrap, 0, -1);
        // Installing
        self.add(st, "i_step", Kind::Label, &[Installing], "", line, 0, -1);
        self.add(st, "i_count", Kind::Hint, &[Installing], "", line | k::SS_RIGHT, 0, -1);
        self.add(w!("msctls_progress32"), "i_bar", Kind::Progress, &[Installing], "", 0, 0, -1);
        self.add(st, "i_detail", Kind::Hint, &[Installing], "", line, 0, -1);
        // Finished
        self.add(st, "f_text", Kind::Wrap, &[Finished], "", wrap, 0, -1);
        self.add(st, "f_notes", Kind::Error, &[Finished], "", wrap, 0, -1);
        self.add(bt, "f_launch", Kind::Check, &[Finished], "Start Glint now", chk, 0, ID_LAUNCH);
        // Failed
        self.add(st, "e_reason", Kind::Wrap, &[Failed], "", wrap, 0, -1);
        self.add(st, "e_hint", Kind::HintWrap, &[Failed], "", wrap, 0, -1);
        // Remove
        self.add(st, "r_intro", Kind::Wrap, &[Remove], "", wrap, 0, -1);
        self.add(bt, "r_purge", Kind::Check, &[Remove], "Also remove Glint's settings, the clip list and the Blur it installed", chk, 0, ID_PURGE);
        self.add(st, "r_warn", Kind::Error, &[Remove], "", wrap, 0, -1);
        // Removed
        self.add(st, "d_text", Kind::Wrap, &[Removed], "", wrap, 0, -1);
        // Footer
        self.add(bt, "back", Kind::Button, ALL, "Back", k::WS_TABSTOP | k::BS_PUSHBUTTON, 0, ID_BACK);
        self.add(bt, "next", Kind::Button, ALL, "Next", k::WS_TABSTOP | k::BS_DEFPUSHBUTTON, 0, ID_NEXT);
        self.add(bt, "cancel", Kind::Button, ALL, "Cancel", k::WS_TABSTOP | k::BS_PUSHBUTTON, 0, ID_CANCEL);
        self.set_check("o_start", self.o.start_menu);
        self.set_check("o_desk", self.o.desktop);
        self.set_check("f_launch", self.o.launch);
        self.set_check("r_purge", self.o.purge);
    }

    fn make_fonts(&mut self) {
        use windows::Win32::UI::HiDpi::{GetSystemMetricsForDpi, SystemParametersInfoForDpi};
        unsafe {
            let mut ncm: NONCLIENTMETRICSW = std::mem::zeroed();
            ncm.cbSize = std::mem::size_of::<NONCLIENTMETRICSW>() as u32;
            let mut lf = if SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS.0, ncm.cbSize, Some(&mut ncm as *mut _ as *mut c_void), 0, self.dpi).is_ok() {
                ncm.lfMessageFont
            } else {
                let mut lf: LOGFONTW = std::mem::zeroed();
                for (i, c) in "Segoe UI".encode_utf16().enumerate() {
                    lf.lfFaceName[i] = c;
                }
                lf
            };
            if lf.lfHeight == 0 {
                lf.lfHeight = -self.s(12);
            }
            for f in [self.font, self.bold, self.title] {
                if !f.is_invalid() {
                    let _ = DeleteObject(f);
                }
            }
            self.font = CreateFontIndirectW(&lf);
            let mut b = lf;
            b.lfWeight = 600;
            self.bold = CreateFontIndirectW(&b);
            let mut t = lf;
            t.lfHeight = lf.lfHeight * 5 / 3;
            t.lfWeight = 600;
            self.title = CreateFontIndirectW(&t);
            // The header icon and the window's own icons, at this DPI.
            self.icon_px = self.s(40);
            let load = |px: i32| LoadImageW(hinst(), PCWSTR(ICON_ID as *const u16), IMAGE_ICON, px, px, LR_DEFAULTCOLOR).map(|h| HICON(h.0)).unwrap_or_default();
            if !self.icon.is_invalid() {
                let _ = DestroyIcon(self.icon);
            }
            self.icon = load(self.icon_px);
            let big = load(GetSystemMetricsForDpi(SM_CXICON, self.dpi));
            let small = load(GetSystemMetricsForDpi(SM_CXSMICON, self.dpi));
            if !self.hwnd.is_invalid() {
                SendMessageW(self.hwnd, k::WM_SETICON, WPARAM(1), LPARAM(big.0 as isize));
                SendMessageW(self.hwnd, k::WM_SETICON, WPARAM(0), LPARAM(small.0 as isize));
            }
        }
    }

    fn font_for(&self, kind: Kind) -> HFONT {
        match kind {
            Kind::Title => self.title,
            Kind::Bold => self.bold,
            _ => self.font,
        }
    }

    fn apply_fonts(&self) {
        for c in &self.ctls {
            unsafe {
                SendMessageW(c.hwnd, k::WM_SETFONT, WPARAM(self.font_for(c.kind).0 as usize), LPARAM(1));
            }
        }
    }

    pub fn set_dpi(&mut self, dpi: u32) {
        self.dpi = dpi.max(96);
        self.make_fonts();
        self.apply_fonts();
        self.layout();
    }

    /// (width, height) of `text` in `font`; wrapped at `width` when given.
    fn measure(&self, font: HFONT, text: &str, width: Option<i32>) -> (i32, i32) {
        unsafe {
            let hdc = GetDC(self.hwnd);
            let old = SelectObject(hdc, font);
            let mut buf: Vec<u16> = if text.is_empty() { vec![b' ' as u16] } else { text.encode_utf16().collect() };
            let mut rc = RECT { left: 0, top: 0, right: width.unwrap_or(100_000), bottom: 0 };
            let flags = DT_CALCRECT | DT_NOPREFIX | if width.is_some() { DT_WORDBREAK | DT_EDITCONTROL } else { DT_SINGLELINE };
            DrawTextW(hdc, &mut buf, &mut rc, flags);
            SelectObject(hdc, old);
            ReleaseDC(self.hwnd, hdc);
            (rc.right, rc.bottom)
        }
    }

    fn line_h(&self, font: HFONT) -> i32 {
        self.measure(font, "Ag", None).1
    }

    fn put(&self, name: &str, x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            let _ = SetWindowPos(self.h(name), HWND::default(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    fn client_size(&self) -> (i32, i32) {
        if self.remove_mode { (self.s(560), self.s(370)) } else { (self.s(600), self.s(440)) }
    }

    fn header_h(&self) -> i32 {
        self.s(72)
    }

    fn footer_h(&self) -> i32 {
        self.s(56)
    }

    /// Height of a wrapping text control at `w`.
    fn wrap_h(&self, name: &str, w: i32) -> i32 {
        let c = self.ctls.iter().find(|c| c.name == name).unwrap();
        let t = text_of(c.hwnd);
        if t.is_empty() { 0 } else { self.measure(self.font_for(c.kind), &t, Some(w)).1 }
    }

    /// Position every control of the current page (and the header and footer) for the current DPI and texts.
    fn layout(&mut self) {
        use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
        let (cw, ch) = self.client_size();
        unsafe {
            let mut r = RECT { left: 0, top: 0, right: cw, bottom: ch };
            let _ = AdjustWindowRectExForDpi(&mut r, WINDOW_STYLE(style_of(self.hwnd)), false, WINDOW_EX_STYLE(k::WS_EX_CONTROLPARENT), self.dpi);
            let _ = SetWindowPos(self.hwnd, HWND::default(), 0, 0, r.right - r.left, r.bottom - r.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        let s = |v: i32| self.s(v);
        let lh = self.line_h(self.font);
        let lb = self.line_h(self.bold);
        let lt = self.line_h(self.title);
        // Header: icon, title, subtitle.
        let hh = self.header_h();
        let tx = s(24) + self.icon_px + s(14);
        let top = (hh - (lt + s(2) + lh)) / 2;
        self.put("title", tx, top, cw - tx - s(24), lt);
        self.put("sub", tx, top + lt + s(2), cw - tx - s(24), lh);
        // Content.
        let x = s(32);
        let w = cw - s(64);
        let mut y = hh + s(20);
        let check_h = |name: &str, w: i32| self.wrap_h(name, w - s(20)).max(lh) + s(4);
        let mut edit_box = RECT::default();
        match self.page {
            Page::Welcome => {
                let h = self.wrap_h("w_intro", w);
                self.put("w_intro", x, y, w, h);
                y += h + s(14);
                self.put("w_head", x, y, w, lb);
                y += lb + s(4);
                let h = self.wrap_h("w_list", w - s(4));
                self.put("w_list", x + s(4), y, w - s(4), h);
                y += h + s(18);
                self.put("w_dir_label", x, y, w, lb);
                y += lb + s(6);
                // The folder box: a borderless edit inside a frame this window draws (so it looks
                // the same everywhere, including the test pictures).
                let eh = (lh + s(12)).max(s(30));
                let bw = s(96).max(self.measure(self.font, "Browse…", None).0 + s(24));
                let boxw = w - bw - s(8);
                edit_box = RECT { left: x, top: y, right: x + boxw, bottom: y + eh };
                let ih = lh + s(2);
                self.put("w_dir", x + s(8), y + (eh - ih) / 2, boxw - s(16), ih);
                self.put("w_browse", x + w - bw, y, bw, eh);
                y += eh + s(6);
                self.put("w_space", x, y, w, lh);
                y += lh + s(4);
                let h = self.wrap_h("w_dir_err", w);
                self.put("w_dir_err", x, y, w, h);
            }
            Page::Options => {
                for (check, state) in [("o_ff", "o_ff_state"), ("o_blur", "o_blur_state")] {
                    let h = check_h(check, w);
                    self.put(check, x, y, w, h);
                    y += h + s(2);
                    let h = self.wrap_h(state, w - s(20));
                    self.put(state, x + s(20), y, w - s(20), h);
                    y += h + s(14);
                }
                for name in ["o_start", "o_desk"] {
                    let h = check_h(name, w);
                    self.put(name, x, y, w, h);
                    y += h + s(6);
                }
                y += s(12);
                let h = self.wrap_h("o_wv", w);
                self.put("o_wv", x, y, w, h);
            }
            Page::Installing => {
                y += s(8);
                let cwid = s(110);
                self.put("i_step", x, y, w - cwid - s(8), lh);
                self.put("i_count", x + w - cwid, y, cwid, lh);
                y += lh + s(10);
                self.put("i_bar", x, y, w, s(18));
                y += s(18) + s(10);
                self.put("i_detail", x, y, w, lh);
            }
            Page::Finished => {
                let h = self.wrap_h("f_text", w);
                self.put("f_text", x, y, w, h);
                y += h + s(14);
                let h = self.wrap_h("f_notes", w);
                if h > 0 {
                    self.put("f_notes", x, y, w, h);
                    y += h + s(14);
                }
                let h = check_h("f_launch", w);
                self.put("f_launch", x, y, w, h);
            }
            Page::Failed => {
                let h = self.wrap_h("e_reason", w);
                self.put("e_reason", x, y, w, h);
                y += h + s(14);
                let h = self.wrap_h("e_hint", w);
                self.put("e_hint", x, y, w, h);
            }
            Page::Remove => {
                let h = self.wrap_h("r_intro", w);
                self.put("r_intro", x, y, w, h);
                y += h + s(16);
                let h = check_h("r_purge", w);
                self.put("r_purge", x, y, w, h);
                y += h + s(12);
                let h = self.wrap_h("r_warn", w);
                self.put("r_warn", x, y, w, h);
            }
            Page::Removed => {
                let h = self.wrap_h("d_text", w);
                self.put("d_text", x, y, w, h);
            }
        }
        // Footer: the visible buttons from the right edge: Cancel, Next, Back.
        let bh = (lh + s(12)).max(s(28));
        let by = ch - self.footer_h() + (self.footer_h() - bh) / 2;
        let mut right = cw - s(20);
        for name in ["cancel", "next", "back"] {
            if self.wanted(name) {
                let bw = s(92).max(self.measure(self.font, &text_of(self.h(name)), None).0 + s(24));
                self.put(name, right - bw, by, bw, bh);
                right -= bw + if name == "cancel" { s(12) } else { s(8) };
            }
        }
        self.edit_box = edit_box;
        unsafe {
            let _ = InvalidateRect(self.hwnd, None, true);
        }
    }

    /// (visible, enabled, text) of a footer button on the current page.
    fn button(&self, name: &str) -> (bool, bool, &'static str) {
        let stopping = self.shared.cancel.load(Ordering::Acquire);
        let can_cancel = self.shared.prog.lock().unwrap().can_cancel;
        match (self.page, name) {
            (Page::Welcome, "back") => (true, false, "Back"),
            (Page::Welcome, "next") => (true, true, "Next"),
            (Page::Options, "back") => (true, true, "Back"),
            (Page::Options, "next") => (true, true, "Install"),
            (Page::Installing, "back") => (true, false, "Back"),
            (Page::Installing, "next") => (true, false, "Install"),
            (Page::Installing, "cancel") => (true, can_cancel && !stopping, "Cancel"),
            (Page::Finished, "next") => (true, true, "Finish"),
            (Page::Failed, "next") => (true, true, "Retry"),
            (Page::Failed, "cancel") => (true, true, "Close"),
            (Page::Remove, "next") => (true, !self.working, "Remove"),
            (Page::Remove, "cancel") => (true, !self.working, "Cancel"),
            (Page::Removed, "next") => (true, true, "Close"),
            (Page::Welcome | Page::Options, "cancel") => (true, true, "Cancel"),
            _ => (false, false, ""),
        }
    }

    /// Whether a control should be visible on the current page.
    fn wanted(&self, name: &str) -> bool {
        let Some(c) = self.ctls.iter().find(|c| c.name == name) else { return false };
        if matches!(name, "back" | "next" | "cancel") {
            return self.button(name).0;
        }
        c.pages.contains(&self.page) && (!matches!(c.kind, Kind::Error) || !text_of(c.hwnd).is_empty())
    }

    fn header(&self) -> (&'static str, &'static str) {
        match self.page {
            Page::Welcome => ("Install Glint", "Instant replays of your games, kept on your PC"),
            Page::Options => ("Choose what to install", "Glint needs ffmpeg to record. The rest is up to you."),
            Page::Installing => ("Installing Glint", "This takes a minute or two. You can keep using your PC."),
            Page::Finished => ("Glint is installed", "Everything is in place."),
            Page::Failed => ("Setup could not finish", "Nothing was changed on this PC."),
            Page::Remove => ("Remove Glint", "Your clips stay where they are."),
            Page::Removed => ("Glint has been removed", "Your clips are still in their folder."),
        }
    }

    pub fn set_page(&mut self, p: Page) {
        self.page = p;
        let (title, sub) = self.header();
        self.set_text("title", title);
        self.set_text("sub", sub);
        for name in ["back", "next", "cancel"] {
            let (_, on, text) = self.button(name);
            self.set_text(name, text);
            self.enable(name, on);
        }
        self.show_wanted();
        self.layout();
        if !self.test {
            unsafe {
                let _ = SetFocus(self.h(if p == Page::Welcome || p == Page::Options || p == Page::Finished || p == Page::Failed || p == Page::Remove || p == Page::Removed { "next" } else { "cancel" }));
            }
        }
    }

    /// Show the controls of this page and hide the rest. These are child windows: nothing appears on
    /// screen while the top-level window is hidden.
    fn show_wanted(&self) {
        for c in &self.ctls {
            let on = self.wanted(c.name);
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | if on { SWP_SHOWWINDOW } else { SWP_HIDEWINDOW };
            unsafe {
                let _ = SetWindowPos(c.hwnd, HWND::default(), 0, 0, 0, 0, flags);
            }
        }
    }

    fn relayout(&mut self) {
        self.show_wanted();
        self.layout();
    }

    // ------------------------------------------------------------ texts

    fn refresh_texts(&mut self) {
        let p = &self.probe;
        let glint = if p.version.is_empty() {
            "•  Glint, the latest version".to_string()
        } else if p.glint_size > 0 {
            format!("•  Glint {} ({})", p.version, mb(p.glint_size))
        } else {
            format!("•  Glint {}", p.version)
        };
        let list = format!(
            "{glint}\n•  ffmpeg, which records and saves the clips, if this PC needs it\n•  Microsoft's WebView2 runtime, only if it is missing\n•  Blur for motion-blur renders, if you choose it on the next page"
        );
        self.set_text("w_list", &list);
        self.refresh_space();
        self.refresh_options();
        let dir = self.o.dir.display().to_string();
        self.set_text("r_intro", &format!("This removes:\n•  Glint and its ffmpeg from {dir}\n•  the Start menu and desktop shortcuts\n•  Glint's entry under Installed apps, and its start with Windows"));
    }

    fn refresh_space(&self) {
        let dir = self.dir_from_edit().unwrap_or_else(|_| self.o.dir.clone());
        let drive = dir.components().next().map(|c| c.as_os_str().to_string_lossy().to_string()).unwrap_or_default();
        let text = match free_space(&dir) {
            Some(b) if b >= 10_000_000_000 => format!("{:.0} GB free on {drive}", b as f64 / 1e9),
            Some(b) => format!("{:.1} GB free on {drive}", b as f64 / 1e9),
            None => String::new(),
        };
        self.set_text("w_space", &text);
    }

    fn ff_found(&self) -> Option<&'static str> {
        if ffmpeg_in(&self.o.dir) {
            Some("folder")
        } else if self.probe.ffmpeg_on_path {
            Some("path")
        } else {
            None
        }
    }

    /// The Options page: the ticks once per install folder, the state lines every time.
    fn refresh_options(&mut self) {
        if self.options_for.as_ref() != Some(&self.o.dir) && self.probe.done {
            self.options_for = Some(self.o.dir.clone());
            self.set_check("o_ff", if self.ff_found().is_some() { self.o.force_ffmpeg } else { self.o.ffmpeg });
            self.set_check("o_blur", self.o.blur || self.probe.blur_installed);
        }
        self.enable("o_ff", self.probe.done);
        self.enable("o_blur", self.probe.done && !self.probe.blur_installed);
        let size = |b: u64| if b > 0 { format!("a {} download", mb(b)) } else { "a download".to_string() };
        let ff = match (self.probe.done, self.ff_found(), self.checked("o_ff")) {
            (false, _, _) => "Checking this PC…".to_string(),
            (_, None, true) => format!("Not on this PC yet: {}.", size(self.probe.ffmpeg_size)),
            (_, None, false) => "Without ffmpeg Glint cannot record. You can add it later in Glint under Settings, Components.".into(),
            (_, Some("folder"), false) => "Already in the install folder. Tick to download a fresh copy.".into(),
            (_, Some("folder"), true) => format!("Setup downloads a fresh copy ({}) over the one in the install folder.", mb(self.probe.ffmpeg_size).replace("0.0 MB", "size unknown")),
            (_, _, false) => "This PC already has a suitable ffmpeg, and Glint uses it. Tick to download Glint's own copy anyway.".into(),
            (_, _, true) => format!("Setup downloads Glint's own copy ({}) and puts it beside Glint.", size(self.probe.ffmpeg_size)),
        };
        self.set_text("o_ff_state", &ff);
        let blur = match (self.probe.done, self.probe.blur_installed, self.checked("o_blur")) {
            (false, _, _) => "Checking this PC…".to_string(),
            (_, true, _) => "Already installed.".into(),
            (_, _, true) => format!("{}. Its own installer runs by itself.", size(self.probe.blur_size).replacen('a', "A", 1)),
            (_, _, false) => "Motion blur stays off. You can add Blur later in Glint under Settings, Components.".into(),
        };
        self.set_text("o_blur_state", &blur);
        let wv = match (self.probe.done, self.probe.webview2) {
            (false, _) => "WebView2 runtime (draws Glint's window): checking…",
            (_, true) => "WebView2 runtime (draws Glint's window): already on this PC.",
            (_, false) => "WebView2 runtime (draws Glint's window): missing, so Setup gets it from Microsoft.",
        };
        self.set_text("o_wv", wv);
    }

    fn dir_from_edit(&self) -> Result<PathBuf, &'static str> {
        let raw = text_of(self.h("w_dir"));
        let t = raw.trim().trim_matches('"').trim();
        let p = PathBuf::from(t);
        let drive_ok = t.len() >= 3 && t.as_bytes()[1] == b':' && (t.as_bytes()[2] == b'\\' || t.as_bytes()[2] == b'/') || t.starts_with(r"\\");
        if t.is_empty() || !p.is_absolute() || !drive_ok {
            return Err("Type a full folder path, for example C:\\Games\\Glint.");
        }
        if p.is_file() {
            return Err("That is a file. Choose a folder.");
        }
        if t.contains(['*', '?', '<', '>', '|']) {
            return Err("A folder name cannot contain * ? < > or |.");
        }
        Ok(p)
    }

    // ------------------------------------------------------------ events

    fn command(&mut self, id: i32, code: u32) {
        match id {
            ID_NEXT => self.on_next(),
            ID_CANCEL => self.on_cancel(),
            ID_BACK => self.on_back(),
            ID_BROWSE => self.browse(),
            ID_DIR if code == k::EN_CHANGE => {
                if !text_of(self.h("w_dir_err")).is_empty() {
                    self.set_text("w_dir_err", "");
                    self.relayout();
                }
                self.refresh_space();
            }
            ID_FF | ID_BLUR if code == k::BN_CLICKED => {
                self.refresh_options();
                self.layout();
            }
            _ => {}
        }
    }

    fn usable(&self, name: &str) -> bool {
        unsafe { style_of(self.h(name)) & k::WS_VISIBLE != 0 && IsWindowEnabled(self.h(name)).as_bool() }
    }

    fn on_next(&mut self) {
        if !self.usable("next") {
            return;
        }
        match self.page {
            Page::Welcome => match self.dir_from_edit() {
                Err(e) => {
                    self.set_text("w_dir_err", e);
                    self.relayout();
                }
                Ok(dir) => {
                    self.o.dir = dir;
                    self.refresh_options();
                    self.set_page(Page::Options);
                }
            },
            Page::Options => {
                let ff = self.checked("o_ff");
                self.o.ffmpeg = ff;
                self.o.force_ffmpeg = ff && self.ff_found().is_some();
                self.o.blur = self.checked("o_blur") && !self.probe.blur_installed;
                self.o.start_menu = self.checked("o_start");
                self.o.desktop = self.checked("o_desk");
                self.start_install();
            }
            Page::Installing => {}
            Page::Finished => {
                if self.checked("f_launch") && !self.test {
                    if let Some(out) = &self.outcome {
                        launch(&out.exe);
                    }
                }
                self.close();
            }
            Page::Failed => self.start_install(),
            Page::Remove => {
                let running = if self.test { self.glint_running_test } else { glint_running(&self.o.dir) };
                if running {
                    self.set_text("r_warn", "Glint is running. Right-click its tray icon, choose Quit Glint, then press Remove.");
                    self.relayout();
                } else {
                    self.set_text("r_warn", "");
                    self.o.purge = self.checked("r_purge");
                    self.start_remove();
                }
            }
            Page::Removed => self.close(),
        }
    }

    fn on_back(&mut self) {
        if self.page == Page::Options && self.usable("back") {
            self.set_page(Page::Welcome);
        }
    }

    fn on_cancel(&mut self) {
        match self.page {
            Page::Installing => {
                let can = self.shared.prog.lock().unwrap().can_cancel;
                if can && !self.shared.cancel.load(Ordering::Acquire) && self.confirm("Stop the installation? Nothing will be changed on this PC.") {
                    self.shared.cancel.store(true, Ordering::Release);
                    self.on_prog();
                }
            }
            Page::Remove if self.working => {}
            _ => self.close(),
        }
    }

    fn confirm(&mut self, question: &str) -> bool {
        if self.test {
            self.asked.push(question.to_string());
            return self.answer_yes;
        }
        unsafe { MessageBoxW(self.hwnd, &HSTRING::from(question), w!("Glint Setup"), MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2) == IDYES }
    }

    fn close(&mut self) {
        self.closed = true;
        if !self.test {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }

    fn browse(&mut self) {
        use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
        use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH};
        if self.test {
            return;
        }
        let picked = unsafe {
            (|| -> windows::core::Result<String> {
                let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
                let opts = dlg.GetOptions()?;
                dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
                dlg.SetTitle(w!("Choose where to install Glint"))?;
                dlg.Show(self.hwnd)?;
                let item = dlg.GetResult()?;
                let p = item.GetDisplayName(SIGDN_FILESYSPATH)?;
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as *const c_void));
                Ok(s)
            })()
        };
        if let Ok(s) = picked {
            if s.is_empty() {
                return;
            }
            let mut dir = PathBuf::from(s);
            if !dir.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Glint")) {
                dir.push("Glint");
            }
            self.set_text("w_dir", &dir.to_string_lossy());
            self.refresh_space();
        }
    }

    fn start_install(&mut self) {
        self.shared.cancel.store(false, Ordering::Release);
        *self.shared.prog.lock().unwrap() = Prog { step: "Getting ready".into(), can_cancel: true, ..Default::default() };
        self.working = true;
        self.set_page(Page::Installing);
        self.on_prog();
        if self.test {
            return; // the test feeds the progress itself
        }
        let sh = self.shared.clone();
        let o = self.o.clone();
        std::thread::spawn(move || {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
            }
            let tmp = temp_work_dir();
            let r = install(&o, &tmp, &*sh);
            let _ = fs::remove_dir_all(&tmp);
            if let Err(e) = &r {
                log(&format!("Setup stopped: {e}"));
            }
            sh.finish(Done::Install(r));
        });
    }

    fn start_remove(&mut self) {
        self.working = true;
        self.set_page(Page::Remove);
        if self.test {
            return;
        }
        let sh = self.shared.clone();
        let o = self.o.clone();
        std::thread::spawn(move || {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
            }
            sh.finish(Done::Remove(uninstall(&o)));
        });
    }

    fn set_marquee(&mut self, on: bool) {
        if self.marquee == on {
            return;
        }
        self.marquee = on;
        let bar = self.h("i_bar");
        unsafe {
            let st = style_of(bar);
            SetWindowLongPtrW(bar, GWL_STYLE, (if on { st | k::PBS_MARQUEE } else { st & !k::PBS_MARQUEE }) as isize);
            SendMessageW(bar, k::PBM_SETMARQUEE, WPARAM(on as usize), LPARAM(30));
        }
    }

    fn on_prog(&mut self) {
        self.shared.posted.store(false, Ordering::Release);
        let p = self.shared.prog.lock().unwrap().clone();
        let stopping = self.shared.cancel.load(Ordering::Acquire);
        self.set_text("i_step", if stopping { "Stopping…" } else { &p.step });
        self.set_text("i_count", &if p.total > 0 { format!("Step {} of {}", p.index, p.total) } else { String::new() });
        if !p.busy.is_empty() && !stopping {
            self.set_marquee(true);
            self.set_text("i_detail", &p.busy);
        } else {
            self.set_marquee(false);
            let pos = if p.size > 0 { (p.done.min(p.size) * 1000 / p.size) as usize } else { 0 };
            unsafe {
                SendMessageW(self.h("i_bar"), k::PBM_SETPOS, WPARAM(pos), LPARAM(0));
            }
            let mut detail = if stopping {
                "Removing the part that was downloaded.".to_string()
            } else if p.size > 0 {
                format!("{} of {}", mb(p.done), mb(p.size))
            } else if p.done > 0 {
                format!("{} downloaded", mb(p.done))
            } else {
                String::new()
            };
            if p.speed > 1.0 && !stopping {
                detail.push_str(&format!(", {}/s", mb(p.speed as u64)));
            }
            self.set_text("i_detail", &detail);
        }
        let (_, on, _) = self.button("cancel");
        self.enable("cancel", on);
    }

    fn on_done(&mut self) {
        let items: Vec<Done> = std::mem::take(&mut *self.shared.done.lock().unwrap());
        for d in items {
            match d {
                Done::Probe(p) => {
                    self.probe = p;
                    self.refresh_texts();
                    self.layout();
                }
                Done::Install(Ok(out)) => {
                    self.working = false;
                    let dir = self.o.dir.display().to_string();
                    self.set_text("f_text", &format!("Glint {} is installed in {dir}. It keeps itself up to date: updates install by themselves, never while a game is running.", out.version));
                    self.set_text("f_notes", &out.notes.join("\n"));
                    self.outcome = Some(out);
                    self.set_page(Page::Finished);
                }
                Done::Install(Err(e)) => {
                    self.working = false;
                    self.set_text("e_reason", &e);
                    self.set_text(
                        "e_hint",
                        &if e == CANCELLED { "Press Retry to start again, or Close to leave.".to_string() } else { format!("Press Retry to try again, or Close to leave. Details are in {}.", log_path().display()) },
                    );
                    self.error = e;
                    self.set_page(Page::Failed);
                }
                Done::Remove(r) => {
                    self.working = false;
                    let mut lines = r.done.clone();
                    lines.extend(r.problems.iter().cloned());
                    self.set_text("d_text", &lines.join("\n"));
                    self.removal = Some(r);
                    self.set_page(Page::Removed);
                }
            }
        }
    }

    // ------------------------------------------------------------ drawing

    fn paint(&self, hdc: HDC) {
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut rc);
            let hh = self.header_h();
            let fh = self.footer_h();
            let px = self.s(1).max(1);
            FillRect(hdc, &RECT { left: 0, top: 0, right: rc.right, bottom: rc.bottom - fh }, self.white);
            FillRect(hdc, &RECT { left: 0, top: hh, right: rc.right, bottom: hh + px }, self.line);
            FillRect(hdc, &RECT { left: 0, top: rc.bottom - fh, right: rc.right, bottom: rc.bottom }, self.footer);
            FillRect(hdc, &RECT { left: 0, top: rc.bottom - fh, right: rc.right, bottom: rc.bottom - fh + px }, self.line);
            if self.page == Page::Welcome {
                // The folder box's frame: light all round, darker along the bottom (as Windows 11 draws it).
                let b = self.edit_box;
                FillRect(hdc, &RECT { left: b.left, top: b.top, right: b.right, bottom: b.top + px }, self.frame);
                FillRect(hdc, &RECT { left: b.left, top: b.top, right: b.left + px, bottom: b.bottom }, self.frame);
                FillRect(hdc, &RECT { left: b.right - px, top: b.top, right: b.right, bottom: b.bottom }, self.frame);
                FillRect(hdc, &RECT { left: b.left, top: b.bottom - px, right: b.right, bottom: b.bottom }, self.frame_bottom);
            }
            if !self.icon.is_invalid() {
                let _ = DrawIconEx(hdc, self.s(24), (hh - self.icon_px) / 2, self.icon, self.icon_px, self.icon_px, 0, HBRUSH::default(), DI_NORMAL);
            }
        }
    }

    fn ctl_color(&self, hdc: HDC, child: HWND) -> LRESULT {
        let kind = self.ctls.iter().find(|c| c.hwnd == child).map(|c| c.kind).unwrap_or(Kind::Label);
        let color = match kind {
            Kind::Sub | Kind::Hint | Kind::HintWrap => rgb(96, 96, 96),
            Kind::Error => rgb(176, 46, 32),
            _ => rgb(26, 26, 26),
        };
        unsafe {
            SetTextColor(hdc, color);
            SetBkColor(hdc, rgb(255, 255, 255));
        }
        LRESULT(self.white.0 as isize)
    }

    fn show(&mut self) {
        assert!(!self.test, "the test never shows a window");
        unsafe {
            let mut wr = RECT::default();
            let _ = GetWindowRect(self.hwnd, &mut wr);
            let mon = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(mon, &mut mi).as_bool() {
                let (w, h) = (wr.right - wr.left, wr.bottom - wr.top);
                let x = mi.rcWork.left + (mi.rcWork.right - mi.rcWork.left - w) / 2;
                let y = mi.rcWork.top + (mi.rcWork.bottom - mi.rcWork.top - h) / 2;
                let _ = SetWindowPos(self.hwnd, HWND::default(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER);
            }
            let _ = ShowWindow(self.hwnd, SW_SHOWNORMAL);
            let _ = UpdateWindow(self.hwnd);
            let _ = SetFocus(self.h("next"));
        }
    }
}

fn message_loop(hwnd: HWND) {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            if IsDialogMessageW(hwnd, &msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

pub fn run_wizard(o: Opts) -> i32 {
    init_ui();
    let mut ui = match create(o, false, false) {
        Ok(ui) => ui,
        Err(e) => {
            log(&e);
            return 1;
        }
    };
    // What is on this PC (fast), then the download sizes (GitHub): the pages fill in as they arrive.
    let sh = ui.shared.clone();
    let po = ui.o.clone();
    std::thread::spawn(move || {
        let mut p = Probe { ffmpeg_on_path: path_ffmpeg_ok(), webview2: has_webview2(), blur_installed: blur_installed(&po.places), done: true, ..Default::default() };
        sh.finish(Done::Probe(p.clone()));
        let full = probe(&po);
        p.version = full.version;
        p.glint_size = full.glint_size;
        p.ffmpeg_size = full.ffmpeg_size;
        p.blur_size = full.blur_size;
        sh.finish(Done::Probe(p));
    });
    ui.show();
    message_loop(ui.hwnd);
    if ui.outcome.is_some() { 0 } else { 1 }
}

pub fn run_remove(o: Opts) -> i32 {
    init_ui();
    let mut ui = match create(o, true, false) {
        Ok(ui) => ui,
        Err(e) => {
            log(&e);
            return 1;
        }
    };
    ui.show();
    message_loop(ui.hwnd);
    match &ui.removal {
        Some(r) => {
            if r.files_later {
                spawn_remover(&ui.o.dir, &r.files);
            }
            if r.problems.is_empty() { 0 } else { 1 }
        }
        None => 1,
    }
}

// ---------------------------------------------------------------- --ui-test

struct Report {
    lines: Vec<String>,
    pass: usize,
    fail: usize,
}

impl Report {
    fn check(&mut self, ok: bool, what: &str, detail: &str) {
        if ok {
            self.pass += 1;
            self.lines.push(format!("PASS  {what}"));
        } else {
            self.fail += 1;
            self.lines.push(format!("FAIL  {what}: {detail}"));
        }
    }
    fn info(&mut self, text: &str) {
        self.lines.push(format!("INFO  {text}"));
    }
}

fn rect_in_parent(h: HWND, parent: HWND) -> RECT {
    unsafe {
        let mut r = RECT::default();
        let _ = GetWindowRect(h, &mut r);
        let mut pts = [POINT { x: r.left, y: r.top }, POINT { x: r.right, y: r.bottom }];
        MapWindowPoints(HWND::default(), parent, &mut pts);
        RECT { left: pts[0].x, top: pts[0].y, right: pts[1].x, bottom: pts[1].y }
    }
}

fn children(parent: HWND) -> Vec<HWND> {
    let mut out = Vec::new();
    unsafe {
        let mut c = GetWindow(parent, GW_CHILD).unwrap_or_default();
        while !c.is_invalid() {
            out.push(c);
            c = GetWindow(c, GW_HWNDNEXT).unwrap_or_default();
        }
    }
    out
}

/// Handle this thread's messages (only the hidden test windows have any) for `ms` milliseconds.
fn pump(ms: u64) {
    let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    unsafe {
        let mut msg = MSG::default();
        while std::time::Instant::now() < end {
            while PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

fn save_png(path: &Path, w: i32, h: i32, bgra: &[u8]) -> Result<(), String> {
    let file = fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(|e| e.to_string())?;
    let rgb: Vec<u8> = bgra.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0]]).collect();
    wr.write_image_data(&rgb).map_err(|e| e.to_string())
}

/// Draw a window into a 32-bit bitmap. `whole` = PrintWindow on the top-level window (frame included,
/// if Windows draws it for a hidden window); otherwise the client area: the window paints itself
/// (WM_PRINTCLIENT), then every visible child draws itself at its place (WM_PRINT).
fn draw(hwnd: HWND, whole: bool) -> (i32, i32, Vec<u8>) {
    unsafe {
        let mut r = RECT::default();
        if whole {
            let _ = GetWindowRect(hwnd, &mut r);
        } else {
            let _ = GetClientRect(hwnd, &mut r);
        }
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let screen = GetDC(hwnd);
        let mem = CreateCompatibleDC(screen);
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -h;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0).unwrap_or_default();
        let old = SelectObject(mem, bmp);
        FillRect(mem, &RECT { left: 0, top: 0, right: w, bottom: h }, HBRUSH(GetStockObject(GRAY_BRUSH).0));
        if whole {
            let _ = windows::Win32::Storage::Xps::PrintWindow(hwnd, mem, windows::Win32::Storage::Xps::PRINT_WINDOW_FLAGS(0));
        } else {
            SendMessageW(hwnd, WM_PRINTCLIENT, WPARAM(mem.0 as usize), LPARAM(k::PRF_CLIENT | k::PRF_ERASEBKGND));
            for c in children(hwnd).into_iter().rev() {
                if style_of(c) & k::WS_VISIBLE == 0 {
                    continue;
                }
                let cr = rect_in_parent(c, hwnd);
                let saved = SaveDC(mem);
                let _ = SetViewportOrgEx(mem, cr.left, cr.top, None);
                IntersectClipRect(mem, 0, 0, cr.right - cr.left, cr.bottom - cr.top);
                SendMessageW(c, WM_PRINT, WPARAM(mem.0 as usize), LPARAM(k::PRF_CLIENT | k::PRF_NONCLIENT | k::PRF_ERASEBKGND | k::PRF_CHILDREN));
                let _ = RestoreDC(mem, saved);
            }
        }
        let _ = GdiFlush();
        let data = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec();
        SelectObject(mem, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem);
        ReleaseDC(hwnd, screen);
        (w, h, data)
    }
}

impl Ui {
    fn capture(&self, folder: &Path, file: &str, rep: &mut Report) {
        pump(700); // let the controls' own timers run (the progress bar glides to a new position)
        let (w, h, data) = draw(self.hwnd, false);
        let path = folder.join(file);
        let r = save_png(&path, w, h, &data);
        rep.check(r.is_ok(), &format!("{file}: drawn ({w}x{h})"), &r.err().unwrap_or_default());
    }

    /// Every check for the page on screen now. `tabs` = the expected tab order (visible, enabled tab stops).
    fn check_page(&self, rep: &mut Report, label: &str, tabs: &[&str]) {
        let (cw, ch) = self.client_size();
        let hh = self.header_h();
        let fh = self.footer_h();
        let mut problems: Vec<String> = Vec::new();
        let mut shown: Vec<(&str, RECT)> = Vec::new();
        for c in &self.ctls {
            let vis = style_of(c.hwnd) & k::WS_VISIBLE != 0;
            let want = self.wanted(c.name);
            unsafe {
                if !IsWindow(c.hwnd).as_bool() {
                    problems.push(format!("{} does not exist", c.name));
                    continue;
                }
            }
            if vis != want {
                problems.push(format!("{} is {} but should be {}", c.name, if vis { "shown" } else { "hidden" }, if want { "shown" } else { "hidden" }));
            }
            if !vis {
                continue;
            }
            let r = rect_in_parent(c.hwnd, self.hwnd);
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            if r.left < 0 || r.top < 0 || r.right > cw || r.bottom > ch || w <= 0 || h <= 0 {
                problems.push(format!("{} is outside the window ({},{} {}x{})", c.name, r.left, r.top, w, h));
            }
            let (lo, hi) = match c.kind {
                Kind::Title | Kind::Sub => (0, hh),
                _ if matches!(c.name, "back" | "next" | "cancel") => (ch - fh, ch),
                _ => (hh, ch - fh),
            };
            if r.top < lo || r.bottom > hi {
                problems.push(format!("{} is not inside its area ({}..{} vs {}..{})", c.name, r.top, r.bottom, lo, hi));
            }
            let text = text_of(c.hwnd);
            if c.kind != Kind::Progress && text.trim().is_empty() {
                problems.push(format!("{} has no text", c.name));
            }
            let font = unsafe { HFONT(SendMessageW(c.hwnd, k::WM_GETFONT, WPARAM(0), LPARAM(0)).0 as *mut c_void) };
            if c.kind != Kind::Progress && font != self.font_for(c.kind) {
                problems.push(format!("{} does not use the system font", c.name));
            }
            let f = self.font_for(c.kind);
            let fits = match c.kind {
                Kind::Title | Kind::Sub | Kind::Label | Kind::Bold | Kind::Hint => {
                    let (tw, th) = self.measure(f, &text, None);
                    tw <= w && th <= h
                }
                Kind::Wrap | Kind::HintWrap | Kind::Error => self.measure(f, &text, Some(w)).1 <= h,
                Kind::Check => self.measure(f, &text, Some(w - self.s(20))).1 <= h,
                Kind::Button => self.measure(f, &text, None).0 + self.s(12) <= w,
                Kind::Edit | Kind::Progress => true,
            };
            if !fits {
                problems.push(format!("the text of {} does not fit ({}x{}): {:?}", c.name, w, h, text));
            }
            shown.push((c.name, r));
        }
        for (i, (a, ra)) in shown.iter().enumerate() {
            for (b, rb) in &shown[i + 1..] {
                if ra.left < rb.right && rb.left < ra.right && ra.top < rb.bottom && rb.top < ra.bottom {
                    problems.push(format!("{a} overlaps {b}"));
                }
            }
        }
        rep.check(problems.is_empty(), &format!("{label}: {} controls shown, each inside its area, with its text, in the system font, fitting, no overlaps", shown.len()), &problems.join("; "));
        // Tab order: the z-order of the visible, enabled tab stops.
        let order: Vec<&str> = children(self.hwnd)
            .into_iter()
            .filter(|&c| {
                let st = style_of(c);
                st & k::WS_VISIBLE != 0 && st & k::WS_TABSTOP != 0 && st & k::WS_DISABLED == 0
            })
            .filter_map(|c| self.ctls.iter().find(|x| x.hwnd == c).map(|x| x.name))
            .collect();
        rep.check(order == tabs, &format!("{label}: tab order {}", tabs.join(" > ")), &format!("is {}", order.join(" > ")));
        // The footer buttons.
        let mut states = Vec::new();
        let mut bad = Vec::new();
        for name in ["back", "next", "cancel"] {
            let (vis, on, text) = self.button(name);
            let h = self.h(name);
            let real = (style_of(h) & k::WS_VISIBLE != 0, unsafe { IsWindowEnabled(h).as_bool() }, text_of(h));
            if real.0 != vis || (vis && (real.1 != on || real.2 != text)) {
                bad.push(format!("{name} is {:?}", real));
            }
            if vis {
                states.push(format!("{text}{}", if on { "" } else { " (off)" }));
            }
        }
        rep.check(bad.is_empty(), &format!("{label}: buttons {}", states.join(", ")), &bad.join("; "));
    }

    /// A key as the message loop would hand it to IsDialogMessage (in-process, to this hidden window only).
    fn key(&self, on: &str, vk: usize) -> bool {
        let msg = MSG { hwnd: self.h(on), message: WM_KEYDOWN, wParam: WPARAM(vk), lParam: LPARAM(0), ..Default::default() };
        unsafe { IsDialogMessageW(self.hwnd, &msg).as_bool() }
    }

    fn deliver(&mut self, d: Done) {
        self.shared.done.lock().unwrap().push(d);
        self.on_done();
    }

    fn feed(&mut self, p: Prog) {
        *self.shared.prog.lock().unwrap() = p;
        self.on_prog();
    }
}

pub fn ui_test(folder: &Path, mut o: Opts) -> i32 {
    let _ = fs::create_dir_all(folder);
    let mut rep = Report { lines: Vec::new(), pass: 0, fail: 0 };
    let env = init_ui();
    // Nothing real is ever touched: test places, a made-up folder, no network, no threads.
    o.places = Places::test(&folder.join("places"));
    o.dir = folder.join("Glint");
    o.yes = false;
    o.launch = true;

    // How the exe looks to Windows.
    unsafe {
        use windows::Win32::System::LibraryLoader::{FindResourceW, LoadResource, LockResource, SizeofResource};
        let m = windows::Win32::System::LibraryLoader::GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let res = FindResourceW(m, PCWSTR(1 as *const u16), PCWSTR(24 as *const u16));
        let text = if res.is_invalid() {
            String::new()
        } else {
            let size = SizeofResource(m, res) as usize;
            let data = LoadResource(m, res).ok().map(|g| LockResource(g) as *const u8);
            data.map(|p| String::from_utf8_lossy(std::slice::from_raw_parts(p, size)).to_string()).unwrap_or_default()
        };
        let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        rep.info(&format!("manifest resource #1 ({} bytes): {}", text.len(), if flat.is_empty() { "none".into() } else { flat.chars().take(400).collect::<String>() }));
        rep.info(&format!("manifest asks for Common Controls 6: {}; dpiAware in manifest: {}", text.contains("Common-Controls"), text.contains("dpiAware")));
    }
    rep.info(&format!("visual styles from {}", env.styles_from));
    rep.check(env.comctl_after >= 6, &format!("Common Controls version {} (was {} before start-up)", env.comctl_after, env.comctl_before), "old-style controls");
    rep.check(env.dpi_call, "per-monitor DPI awareness (v2) switched on at run time", "SetProcessDpiAwarenessContext failed");
    unsafe {
        use windows::Win32::UI::HiDpi::{AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
        rep.check(AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).as_bool(), "this thread is per-monitor DPI aware (v2)", "it is not");
    }

    let mut ui = match create(o.clone(), false, true) {
        Ok(ui) => ui,
        Err(e) => {
            rep.check(false, "the wizard window is made", &e);
            return finish_report(folder, rep);
        }
    };
    rep.info(&format!("window DPI {} (pages drawn at this DPI, then at 144 = 150 %)", ui.dpi));
    ui.probe = Probe { done: true, version: "0.1.18".into(), glint_size: 8_412_160, ffmpeg_size: 101_234_567, blur_size: 63_500_000, ffmpeg_on_path: false, webview2: true, blur_installed: false };
    ui.refresh_texts();
    ui.set_page(Page::Welcome);
    unsafe {
        rep.check(!IsWindowVisible(ui.hwnd).as_bool(), "the window is never shown (hidden the whole test)", "it is visible");
        rep.check(!windows::Win32::UI::Controls::GetWindowTheme(ui.h("next")).is_invalid(), "buttons are drawn with the visual style (themed)", "no theme on the Next button");
        rep.check(!windows::Win32::UI::Controls::GetWindowTheme(ui.h("o_ff")).is_invalid(), "check boxes are drawn with the visual style (themed)", "no theme on a check box");
        let mut lf: LOGFONTW = std::mem::zeroed();
        GetObjectW(ui.font, std::mem::size_of::<LOGFONTW>() as i32, Some(&mut lf as *mut _ as *mut c_void));
        let face = String::from_utf16_lossy(&lf.lfFaceName[..lf.lfFaceName.iter().position(|&c| c == 0).unwrap_or(32)]);
        rep.check(!face.is_empty(), &format!("system message font: {face}, {} px", -lf.lfHeight), "no font");
    }
    rep.check(!ui.icon.is_invalid(), "Glint's icon loaded from the exe (header and title bar)", "no icon resource 32512");
    let (_, _, whole) = draw(ui.hwnd, true);
    let blank = whole.chunks_exact(4).all(|p| p[0] == whole[0] && p[1] == whole[1] && p[2] == whole[2]);
    rep.info(&format!(
        "PrintWindow of the hidden window (title bar and frame included) gives {}, so the PNGs show the client area, drawn control by control (WM_PRINTCLIENT / WM_PRINT); the title bar is Windows' own and is not in them",
        if blank { "a blank picture" } else { "a picture" }
    ));

    // Welcome
    ui.check_page(&mut rep, "welcome", &["w_dir", "w_browse", "next", "cancel"]);
    ui.capture(folder, "01-welcome.png", &mut rep);
    // A bad folder: the error line.
    ui.set_text("w_dir", "Glint");
    ui.on_next();
    rep.check(ui.page == Page::Welcome && !text_of(ui.h("w_dir_err")).is_empty(), "a folder that is not a full path is refused with a message", &format!("page {:?}", ui.page));
    ui.check_page(&mut rep, "welcome, bad folder", &["w_dir", "w_browse", "next", "cancel"]);
    ui.capture(folder, "01b-welcome-bad-folder.png", &mut rep);
    ui.set_text("w_dir", &o.dir.to_string_lossy());
    rep.check(text_of(ui.h("w_dir_err")).is_empty(), "typing in the folder box clears the message", "still there");
    // Enter = Next.
    let handled = ui.key("w_dir", k::VK_RETURN);
    rep.check(handled && ui.page == Page::Options, "Enter in the folder box = Next (Options page)", &format!("handled {handled}, page {:?}", ui.page));
    if ui.page != Page::Options {
        ui.on_next();
    }

    // Options
    ui.check_page(&mut rep, "options", &["o_ff", "o_blur", "o_start", "o_desk", "back", "next", "cancel"]);
    rep.check(ui.checked("o_ff") && ui.checked("o_blur") && ui.checked("o_start") && !ui.checked("o_desk"), "options start as: ffmpeg on, Blur on, Start menu on, desktop off", "other ticks");
    ui.capture(folder, "02-options.png", &mut rep);
    ui.command(ID_BACK, k::BN_CLICKED);
    rep.check(ui.page == Page::Welcome, "Back goes to Welcome", &format!("{:?}", ui.page));
    ui.command(ID_NEXT, k::BN_CLICKED);
    // Found on this PC, Blur installed.
    let keep = ui.probe.clone();
    ui.probe.ffmpeg_on_path = true;
    ui.probe.blur_installed = true;
    ui.options_for = None;
    ui.refresh_options();
    ui.layout();
    rep.check(!ui.checked("o_ff") && ui.checked("o_blur") && !ui.usable("o_blur"), "with ffmpeg on PATH and Blur installed: ffmpeg off, Blur ticked and greyed", "other ticks");
    ui.check_page(&mut rep, "options, all found", &["o_ff", "o_start", "o_desk", "back", "next", "cancel"]);
    ui.capture(folder, "02b-options-found.png", &mut rep);
    ui.probe = keep;
    ui.options_for = None;
    ui.refresh_options();
    ui.set_check("o_ff", false);
    ui.command(ID_FF, k::BN_CLICKED);
    rep.check(text_of(ui.h("o_ff_state")).starts_with("Without ffmpeg"), "unticking ffmpeg says what that means", &text_of(ui.h("o_ff_state")));
    ui.set_check("o_ff", true);
    ui.command(ID_FF, k::BN_CLICKED);

    // Installing
    ui.command(ID_NEXT, k::BN_CLICKED);
    rep.check(ui.page == Page::Installing && ui.o.ffmpeg && !ui.o.force_ffmpeg && ui.o.blur && ui.o.start_menu && !ui.o.desktop, "Install starts with the ticked options", &format!("{:?}", ui.page));
    let size = 8_412_160u64;
    let step = |done: u64, speed: f64| Prog { step: "Downloading Glint 0.1.18".into(), index: 1, total: 4, done, size, speed, busy: String::new(), can_cancel: true };
    for (file, done, speed, pct) in [("03-installing-0.png", 0, 0.0, 0), ("04-installing-40.png", size * 2 / 5, 4_200_000.0, 40), ("05-installing-100.png", size, 0.0, 100)] {
        ui.feed(step(done, speed));
        let pos = unsafe { SendMessageW(ui.h("i_bar"), 0x0408, WPARAM(0), LPARAM(0)).0 }; // PBM_GETPOS
        rep.check(pos == pct * 10, &format!("installing {pct} %: bar at {pos}/1000, \"{}\", \"{}\"", text_of(ui.h("i_step")), text_of(ui.h("i_detail"))), "wrong position");
        ui.check_page(&mut rep, &format!("installing {pct} %"), &["cancel"]);
        ui.capture(folder, file, &mut rep);
        // How much of the drawn bar is coloured (the fill), read from the picture.
        let (w, _, data) = draw(ui.hwnd, false);
        let r = rect_in_parent(ui.h("i_bar"), ui.hwnd);
        let row = (r.top + r.bottom) / 2;
        let inner = (r.left + 2..r.right - 2).collect::<Vec<i32>>();
        let coloured = inner
            .iter()
            .filter(|&&x| {
                let i = ((row * w + x) * 4) as usize;
                let (b, g, rr) = (data[i] as i32, data[i + 1] as i32, data[i + 2] as i32);
                b.max(g).max(rr) - b.min(g).min(rr) > 40
            })
            .count();
        let drawn = coloured as f64 / inner.len() as f64 * 100.0;
        rep.check((drawn - pct as f64).abs() <= 6.0, &format!("installing {pct} %: the picture shows the bar {drawn:.0} % filled"), "the drawn bar does not match");
    }
    ui.feed(Prog { step: "Downloading ffmpeg (101 MB)".into(), index: 2, total: 4, busy: "Unpacking ffmpeg".into(), can_cancel: true, ..Default::default() });
    rep.check(ui.marquee, "work without a known length shows a moving bar", "no marquee");
    ui.capture(folder, "05b-installing-unpacking.png", &mut rep);
    // Esc asks first.
    ui.answer_yes = false;
    let handled = ui.key("cancel", k::VK_ESCAPE);
    rep.check(handled && ui.asked.len() == 1 && ui.page == Page::Installing && !ui.shared.cancel.load(Ordering::Acquire), "Esc while installing asks first; No keeps going", &format!("handled {handled}, asked {}", ui.asked.len()));
    ui.answer_yes = true;
    ui.command(ID_CANCEL, k::BN_CLICKED);
    rep.check(ui.shared.cancel.load(Ordering::Acquire) && !ui.usable("cancel") && text_of(ui.h("i_step")) == "Stopping…", "Cancel, Yes: the work is told to stop, Cancel greys out", "not stopped");
    ui.capture(folder, "05c-installing-stopping.png", &mut rep);
    ui.shared.cancel.store(false, Ordering::Release);
    ui.feed(Prog { step: "Installing".into(), index: 4, total: 4, busy: "Copying files".into(), can_cancel: false, ..Default::default() });
    rep.check(!ui.usable("cancel"), "while files are put in place Cancel is off", "Cancel still on");
    let asked = ui.asked.len();
    ui.key("cancel", k::VK_ESCAPE);
    rep.check(ui.asked.len() == asked, "Esc then does nothing (no question)", "asked");

    // Finished
    ui.deliver(Done::Install(Ok(Outcome { version: "0.1.18".into(), exe: o.dir.join("Glint.exe"), notes: vec!["Blur did not end up where Glint looks for it. Add it later in Glint under Settings, Components. Everything else works without it.".into()] })));
    rep.check(ui.page == Page::Finished && ui.checked("f_launch"), "finished: \"Start Glint now\" is ticked", &format!("{:?}", ui.page));
    ui.check_page(&mut rep, "finished", &["f_launch", "next"]);
    ui.capture(folder, "06-finished.png", &mut rep);
    ui.deliver(Done::Install(Ok(Outcome { version: "0.1.18".into(), exe: o.dir.join("Glint.exe"), notes: vec![] })));
    ui.check_page(&mut rep, "finished, no notes", &["f_launch", "next"]);
    ui.capture(folder, "06b-finished-plain.png", &mut rep);

    // Failed
    ui.deliver(Done::Install(Err("The download failed (Failed to connect to github.com port 443 after 21034 ms: Could not connect to server). Check your internet connection, then try again.".into())));
    rep.check(ui.page == Page::Failed, "a failure shows the Failed page", &format!("{:?}", ui.page));
    ui.check_page(&mut rep, "failed", &["next", "cancel"]);
    ui.capture(folder, "07-failed.png", &mut rep);
    ui.command(ID_NEXT, k::BN_CLICKED);
    rep.check(ui.page == Page::Installing, "Retry starts again", &format!("{:?}", ui.page));
    ui.deliver(Done::Install(Err(CANCELLED.into())));
    ui.check_page(&mut rep, "cancelled", &["next", "cancel"]);
    ui.capture(folder, "07b-cancelled.png", &mut rep);
    ui.closed = false;
    ui.command(ID_CANCEL, k::BN_CLICKED);
    rep.check(ui.closed, "Close closes", "still open");

    // 150 %
    ui.set_dpi(144);
    ui.set_page(Page::Welcome);
    ui.check_page(&mut rep, "welcome at 150 %", &["w_dir", "w_browse", "next", "cancel"]);
    ui.capture(folder, "11-welcome-150.png", &mut rep);
    ui.set_page(Page::Options);
    ui.check_page(&mut rep, "options at 150 %", &["o_ff", "o_blur", "o_start", "o_desk", "back", "next", "cancel"]);
    ui.capture(folder, "12-options-150.png", &mut rep);
    ui.deliver(Done::Install(Err("The download failed (Failed to connect to github.com port 443 after 21034 ms: Could not connect to server). Check your internet connection, then try again.".into())));
    ui.check_page(&mut rep, "failed at 150 %", &["next", "cancel"]);
    ui.capture(folder, "13-failed-150.png", &mut rep);

    // Remove Glint
    let mut ro = o.clone();
    ro.dir = PathBuf::from(r"C:\Users\Someone\AppData\Local\Programs\Glint");
    let mut rm = match create(ro, true, true) {
        Ok(ui) => ui,
        Err(e) => {
            rep.check(false, "the Remove window is made", &e);
            return finish_report(folder, rep);
        }
    };
    rep.check(!rm.checked("r_purge"), "Remove: settings and clip list are kept unless ticked", "ticked");
    rm.check_page(&mut rep, "remove", &["r_purge", "next", "cancel"]);
    rm.capture(folder, "08-remove.png", &mut rep);
    rm.glint_running_test = true;
    rm.command(ID_NEXT, k::BN_CLICKED);
    rep.check(rm.page == Page::Remove && !text_of(rm.h("r_warn")).is_empty(), "Remove while Glint runs: asks to quit it first", &format!("{:?}", rm.page));
    rm.check_page(&mut rep, "remove, Glint running", &["r_purge", "next", "cancel"]);
    rm.capture(folder, "09-remove-running.png", &mut rep);
    rm.glint_running_test = false;
    rm.command(ID_NEXT, k::BN_CLICKED);
    rep.check(rm.working && !rm.usable("next") && !rm.usable("cancel"), "while removing, Remove and Cancel are off", "still on");
    rm.deliver(Done::Remove(Removal {
        done: vec![
            "Removed the Start menu shortcut.".into(),
            "Removed Glint from Installed apps.".into(),
            r"Glint's files in C:\Users\Someone\AppData\Local\Programs\Glint go as soon as this window closes.".into(),
        ],
        problems: vec![],
        files_later: true,
        files: vec![],
    }));
    rm.check_page(&mut rep, "removed", &["next"]);
    rm.capture(folder, "10-removed.png", &mut rep);
    rm.set_dpi(144);
    rm.set_page(Page::Remove);
    rm.check_page(&mut rep, "remove at 150 %", &["r_purge", "next", "cancel"]);
    rm.capture(folder, "14-remove-150.png", &mut rep);

    // Shortcuts through COM, on a .lnk in the test folder.
    let link = folder.join(r"link-test\Glint.lnk");
    let target = std::env::current_exe().unwrap_or_default();
    let made = make_link(&link, &target);
    let back = read_link(&link);
    rep.check(
        made.is_ok() && back.as_ref().is_some_and(|(t, d, desc)| same_path(t, &target) && target.parent().is_some_and(|p| same_path(d, p)) && desc.starts_with("Glint")),
        &format!("shortcut made and read back through IShellLink: {}", link.display()),
        &format!("{made:?} {back:?}"),
    );

    unsafe {
        let _ = DestroyWindow(rm.hwnd);
        let _ = DestroyWindow(ui.hwnd);
    }
    engine_checks(folder, &o, &mut rep);
    finish_report(folder, rep)
}

/// The parts of the install and the uninstall that need no network, on files in the test folder.
fn engine_checks(folder: &Path, o: &Opts, rep: &mut Report) {
    let base = folder.join("engine");
    let _ = fs::remove_dir_all(&base);
    let _ = fs::create_dir_all(&base);
    // All or nothing: a failed put puts every replaced file back.
    let dir = base.join("txn");
    let _ = fs::create_dir_all(&dir);
    let _ = fs::write(dir.join("a.txt"), "old");
    let _ = fs::write(base.join("new.txt"), "new");
    let mut tx = Txn::default();
    let first = tx.put(&base.join("new.txt"), &dir.join("a.txt"));
    let replaced = fs::read_to_string(dir.join("a.txt")).unwrap_or_default() == "new";
    let second = tx.put(&base.join("missing.txt"), &dir.join("b.txt"));
    tx.rollback();
    let back = fs::read_to_string(dir.join("a.txt")).unwrap_or_default();
    let names: Vec<String> = fs::read_dir(&dir).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    rep.check(first.is_ok() && replaced && second.is_err() && back == "old" && names == ["a.txt"], "a failed install puts the old files back (nothing half-installed)", &format!("{first:?} {replaced} {second:?} {back:?} {names:?}"));
    // The uninstaller removes only Glint's own files.
    let dir = base.join("prog");
    let _ = fs::create_dir_all(&dir);
    for f in ["Glint.exe", "Glint.exe.old", "ffmpeg.exe", "my notes.txt"] {
        let _ = fs::write(dir.join(f), "x");
    }
    let names: Vec<String> = KNOWN_FILES.iter().map(|s| s.to_string()).collect();
    let left = remove_files(&dir, &names);
    let kept = dir.join("my notes.txt").is_file() && !dir.join("Glint.exe").exists() && !dir.join("Glint.exe.old").exists() && !dir.join("ffmpeg.exe").exists();
    rep.check(left.is_empty() && kept, "removing files takes only Glint's own (another file in the folder stays, and so does the folder)", &format!("{left:?}"));
    let _ = fs::remove_file(dir.join("my notes.txt"));
    let _ = remove_files(&dir, &names);
    rep.check(!dir.exists(), "an emptied program folder is removed too", "still there");
    // Uninstall in the test places: a shortcut that points at another folder stays.
    let mut uo = o.clone();
    uo.dir = base.join("installed");
    let _ = fs::create_dir_all(&uo.dir);
    let _ = fs::write(uo.dir.join("Glint.exe"), "not a program");
    let other = std::env::current_exe().unwrap_or_default();
    let ours = make_link(&uo.places.start_menu_link, &uo.dir.join("Glint.exe")).is_ok();
    let theirs = make_link(&uo.places.desktop_link, &other).is_ok();
    let r = uninstall(&uo);
    rep.check(
        ours && theirs && !uo.places.start_menu_link.exists() && uo.places.desktop_link.exists() && !uo.dir.exists() && !r.files_later,
        "uninstall (test places): removes its own shortcut and files, keeps a shortcut to another Glint",
        &format!("{:?}", r),
    );
    let _ = fs::remove_file(&uo.places.desktop_link);
}

fn finish_report(folder: &Path, rep: Report) -> i32 {
    let mut text = format!("GlintSetup --ui-test: {} passed, {} failed\n\n", rep.pass, rep.fail);
    text.push_str(&rep.lines.join("\n"));
    text.push('\n');
    let _ = fs::write(folder.join("report.txt"), &text);
    log(&format!("ui-test: {} passed, {} failed ({})", rep.pass, rep.fail, folder.join("report.txt").display()));
    if rep.fail == 0 { 0 } else { 1 }
}
