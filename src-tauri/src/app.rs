//! Tauri shell: window, tray, toast overlay and the commands the UI calls.
use crate::core::{Core, Status};
use crate::library::ClipMeta;
use crate::settings::Settings;
use crate::{blur, clip, hotkeys, library, util, win};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder};

type St<'a> = State<'a, Arc<Core>>;

/// Open (or focus) the main window. The window is destroyed when closed, so
/// the WebView costs nothing while Glint sits in the tray.
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let app = app.clone();
    // Window creation must not run on the event-loop thread.
    thread::spawn(move || {
        let built = WebviewWindowBuilder::new(&app, "main", WebviewUrl::App("index.html".into()))
            .title("Glint")
            .inner_size(1320.0, 840.0)
            .min_inner_size(980.0, 620.0)
            .decorations(false)
            .center()
            .build();
        if built.is_ok() {
            let zoom = app.state::<Arc<Core>>().settings().ui_zoom;
            apply_zoom(&app, zoom);
        }
    });
}

/// What the clip overlay shows and where.
pub struct Toast {
    pub title: String,
    pub sub: String,
    /// Draw the microphone icon: the microphone is being recorded.
    pub mic: bool,
    /// Corner: "tl" | "tr" | "bl" | "br".
    pub pos: String,
    /// Index into `win::monitors()`: the monitor being recorded.
    pub monitor: u32,
    /// Percent, 20..=100.
    pub opacity: u32,
    /// Size in percent of the normal 330 x 84 box.
    pub scale: u32,
    /// Seconds on screen.
    pub secs: f32,
    /// Colour theme id and the user's CSS, so the overlay matches the app.
    pub theme: String,
    pub css: String,
}

/// Small click-through overlay in a corner of the recorded monitor. Never takes focus.
pub fn show_toast(app: &AppHandle, t: Toast) {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    const MARGIN: f64 = 22.0;
    let zoom = t.scale as f64 / 100.0;
    #[allow(non_snake_case)]
    let (W, H) = (330.0 * zoom, 84.0 * zoom);
    let app = app.clone();
    let pos = if ["tl", "tr", "bl", "br"].contains(&t.pos.as_str()) { t.pos.clone() } else { "tr".to_string() };
    let script = format!(
        "window.__TOAST__ = {};",
        serde_json::json!({ "title": t.title, "sub": t.sub, "mic": t.mic, "pos": pos, "opacity": t.opacity, "zoom": zoom, "secs": t.secs, "theme": t.theme, "css": t.css })
    );
    let stay = Duration::from_millis((t.secs * 1000.0) as u64);
    thread::spawn(move || {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        for (label, w) in app.webview_windows() {
            if label.starts_with("toast") {
                let _ = w.destroy();
            }
        }
        // The recorded monitor, matched to Tauri's list by its position to learn its scale.
        let mons = win::monitors();
        let want = mons.get(t.monitor as usize).or(mons.first()).cloned();
        let on = app.available_monitors().ok().and_then(|list| list.into_iter().find(|m| want.as_ref().is_some_and(|w| m.position().x == w.x && m.position().y == w.y)));
        let on = on.or_else(|| app.primary_monitor().ok().flatten());
        let (mut x, mut y) = (40.0, 40.0);
        if let Some(m) = on {
            let sf = m.scale_factor();
            let (left, top) = (m.position().x as f64 / sf, m.position().y as f64 / sf);
            let (width, height) = (m.size().width as f64 / sf, m.size().height as f64 / sf);
            x = if pos.ends_with('l') { left + MARGIN } else { left + width - W - MARGIN };
            // Bottom corners sit clear of the taskbar.
            y = if pos.starts_with('t') { top + MARGIN } else { top + height - H - MARGIN - 48.0 };
        }
        let built = WebviewWindowBuilder::new(&app, format!("toast{n}"), WebviewUrl::App("toast.html".into()))
            .title("Glint")
            .initialization_script(&script)
            .inner_size(W, H)
            .position(x, y)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .focused(false)
            .visible(false)
            .build();
        let w = match built {
            Ok(w) => w,
            Err(e) => {
                util::log(&format!("toast: could not create window: {e}"));
                return;
            }
        };
        let _ = w.set_ignore_cursor_events(true);
        if let Ok(h) = w.hwnd() {
            win::show_no_activate(h.0 as isize);
        }
        thread::sleep(stay);
        let _ = w.destroy();
    });
}

#[derive(Serialize)]
struct AppState {
    settings: Settings,
    monitors: Vec<win::Monitor>,
    status: Status,
    ffmpeg: Option<String>,
    blur: Option<String>,
    hardware: win::Hardware,
    version: &'static str,
}

#[tauri::command(async)]
fn get_state(core: St) -> AppState {
    let s = core.settings();
    AppState {
        monitors: win::monitors(),
        status: core.status(),
        ffmpeg: util::find_exe(&s.ffmpeg_path, "ffmpeg").map(|p| p.to_string_lossy().into_owned()),
        blur: blur::find(&s).map(|p| p.to_string_lossy().into_owned()),
        hardware: win::hardware(),
        version: env!("CARGO_PKG_VERSION"),
        settings: s,
    }
}

#[tauri::command(async)]
fn save_settings(app: AppHandle, core: St, settings: Settings) -> Status {
    let zoom = settings.ui_zoom;
    core.apply_settings(settings);
    apply_zoom(&app, zoom);
    core.status()
}

/// Scale the whole interface, like the browser's own zoom. `percent` is 50..=200.
fn apply_zoom(app: &AppHandle, percent: u32) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_zoom(percent.clamp(50, 200) as f64 / 100.0);
    }
}

#[tauri::command(async)]
fn get_status(core: St) -> Status {
    core.status()
}

#[tauri::command(async)]
fn toggle_recording(core: St) -> Status {
    core.toggle_recording();
    core.status()
}

#[tauri::command(async)]
fn clip_now(core: St, seconds: u32) -> Result<ClipMeta, String> {
    core.clip_blocking(seconds.clamp(5, 3600))
}

#[tauri::command(async)]
fn list_clips(core: St) -> Vec<ClipMeta> {
    core.lib.lock().unwrap().clips.clone()
}

#[tauri::command(async)]
fn rescan_library(core: St) -> Vec<ClipMeta> {
    core.rescan();
    core.lib.lock().unwrap().clips.clone()
}

#[tauri::command(async)]
fn rename_clip(core: St, id: String, title: String) -> Result<ClipMeta, String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("Title cannot be empty".into());
    }
    core.lib.lock().unwrap().update(&id, |c| c.title = title).ok_or_else(|| "Clip not found".into())
}

#[tauri::command(async)]
fn set_favorite(core: St, id: String, favorite: bool) -> Result<ClipMeta, String> {
    core.lib.lock().unwrap().update(&id, |c| c.favorite = favorite).ok_or_else(|| "Clip not found".into())
}

/// Send the clip to the Recycle Bin and drop it from the library.
#[tauri::command(async)]
fn delete_clip(core: St, id: String) -> Result<(), String> {
    let meta = core.lib.lock().unwrap().get(&id).ok_or("Clip not found")?;
    if Path::new(&meta.path).exists() {
        trash::delete(&meta.path).map_err(|e| format!("Could not move the clip to the Recycle Bin: {e}"))?;
    }
    if !meta.thumb.is_empty() {
        let _ = std::fs::remove_file(&meta.thumb);
    }
    core.lib.lock().unwrap().remove(&id);
    Ok(())
}

/// Put the clip file on the clipboard, ready to paste into Discord, Explorer or any chat.
#[tauri::command(async)]
fn copy_clip(core: St, id: String) -> Result<(), String> {
    use clipboard_win::{formats, Clipboard, Setter};
    let meta = core.lib.lock().unwrap().get(&id).ok_or("Clip not found")?;
    if !Path::new(&meta.path).is_file() {
        return Err("The clip file no longer exists".into());
    }
    let _clip = Clipboard::new_attempts(10).map_err(|e| format!("Clipboard is busy: {e}"))?;
    formats::FileList.write_clipboard(&[meta.path.as_str()][..]).map_err(|e| format!("Could not copy: {e}"))
}

/// Send several clips to the Recycle Bin. Returns how many went.
#[tauri::command(async)]
fn delete_clips(core: St, ids: Vec<String>) -> Result<usize, String> {
    let mut done = 0;
    let mut failed = 0;
    for id in &ids {
        let Some(meta) = core.lib.lock().unwrap().get(id) else { continue };
        if Path::new(&meta.path).exists() && trash::delete(&meta.path).is_err() {
            failed += 1;
            continue;
        }
        if !meta.thumb.is_empty() {
            let _ = std::fs::remove_file(&meta.thumb);
        }
        core.lib.lock().unwrap().remove(id);
        done += 1;
    }
    if failed > 0 {
        return Err(format!("{failed} of {} clips could not be moved to the Recycle Bin", ids.len()));
    }
    Ok(done)
}

/// Put several clip files on the clipboard at once.
#[tauri::command(async)]
fn copy_clips(core: St, ids: Vec<String>) -> Result<usize, String> {
    use clipboard_win::{formats, Clipboard, Setter};
    let paths: Vec<String> = {
        let lib = core.lib.lock().unwrap();
        ids.iter().filter_map(|id| lib.get(id)).map(|m| m.path).filter(|p| Path::new(p).is_file()).collect()
    };
    if paths.is_empty() {
        return Err("The clip files no longer exist".into());
    }
    let _clip = Clipboard::new_attempts(10).map_err(|e| format!("Clipboard is busy: {e}"))?;
    formats::FileList.write_clipboard(&paths[..]).map_err(|e| format!("Could not copy: {e}"))?;
    Ok(paths.len())
}

#[tauri::command(async)]
fn set_favorites(core: St, ids: Vec<String>, favorite: bool) {
    let mut lib = core.lib.lock().unwrap();
    for c in lib.clips.iter_mut().filter(|c| ids.contains(&c.id)) {
        c.favorite = favorite;
    }
    lib.save();
}

/// Retag clips with a game name (the tabs and the search use it).
#[tauri::command(async)]
fn set_game(core: St, ids: Vec<String>, game: String) -> Result<(), String> {
    core.set_game(&ids, &game)
}

/// Move the clips that sit loose in the clips folder into per-game folders.
#[tauri::command(async)]
fn sort_by_game(core: St) -> Result<usize, String> {
    core.sort_by_game()
}

#[derive(Serialize)]
struct SoundInfo {
    id: &'static str,
    name: &'static str,
}

#[tauri::command]
fn list_sounds() -> Vec<SoundInfo> {
    crate::sound::SOUNDS.iter().map(|(id, name)| SoundInfo { id, name }).collect()
}

/// Play the clip sound as currently set, so the user can hear a choice.
#[tauri::command(async)]
fn preview_sound(core: St) {
    let s = core.settings();
    crate::sound::play(&s.sound_name, s.sound_volume.max(5), &s.sound_file);
}

#[tauri::command(async)]
fn pick_sound_file() -> Option<String> {
    rfd::FileDialog::new().add_filter("WAV audio", &["wav"]).pick_file().map(|p| p.to_string_lossy().into_owned())
}

/// Show the clip overlay as currently set, so the user can see where it lands.
#[tauri::command(async)]
fn preview_toast(core: St) {
    preview_overlay(&core);
}

fn preview_overlay(core: &Core) {
    let s = core.settings();
    let game = core.status().game;
    core.show_toast(crate::lang::tr(&s.language, "Clip saved"), &crate::core::captured_text(&s, s.default_clip_secs() as f64, if game.is_empty() { "Overwatch" } else { &game }));
}

/// Every game Glint has seen, with its icon.
#[tauri::command(async)]
fn list_games(core: St) -> Vec<crate::games::Game> {
    core.games.lock().unwrap().list.clone()
}

/// Let the user pick an image as a game's icon.
#[tauri::command(async)]
fn pick_game_icon(core: St, game: String) -> Result<Option<crate::games::Game>, String> {
    let Some(file) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp", "ico", "bmp", "gif"]).pick_file() else { return Ok(None) };
    core.games.lock().unwrap().set_icon(&game, &file).map(Some)
}

#[tauri::command(async)]
fn move_clips(core: St, ids: Vec<String>, folder: String) -> Result<usize, String> {
    core.move_clips(&ids, &folder)
}

#[tauri::command(async)]
fn list_folders(core: St) -> Vec<String> {
    core.folders()
}

#[tauri::command(async)]
fn create_folder(core: St, name: String) -> Result<String, String> {
    core.create_folder(&name)
}

#[tauri::command(async)]
fn rename_folder(core: St, from: String, to: String) -> Result<String, String> {
    core.rename_folder(&from, &to)
}

#[tauri::command(async)]
fn delete_folder(core: St, name: String) -> Result<(), String> {
    core.delete_folder(&name)
}

#[tauri::command(async)]
fn reveal_clip(core: St, id: String) -> Result<(), String> {
    let meta = core.lib.lock().unwrap().get(&id).ok_or("Clip not found")?;
    std::process::Command::new("explorer").arg(format!("/select,{}", meta.path)).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command(async)]
fn open_clips_dir(core: St) -> Result<(), String> {
    let dir = PathBuf::from(core.settings().clips_dir);
    let _ = std::fs::create_dir_all(&dir);
    std::process::Command::new("explorer").arg(dir).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command(async)]
fn pick_folder(core: St) -> Option<String> {
    rfd::FileDialog::new().set_directory(core.settings().clips_dir).pick_folder().map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command(async)]
fn trim_clip(core: St, id: String, start: f64, end: f64, include_mic: bool) -> Result<ClipMeta, String> {
    let meta = core.lib.lock().unwrap().get(&id).ok_or("Clip not found")?;
    let new = clip::trim(&core.settings(), &meta, start, end, include_mic)?;
    core.lib.lock().unwrap().add(new.clone());
    Ok(new)
}

/// The clip's audio tracks as separate files, for the editor's track mixer.
#[tauri::command(async)]
fn editor_audio_tracks(core: St, id: String) -> Result<Vec<crate::editor::AudioTrack>, String> {
    let meta = core.lib.lock().unwrap().get(&id).ok_or("Clip not found")?;
    crate::editor::audio_tracks(&core.settings(), &meta)
}

/// Export an edited copy of a clip (trim, speed, colour, audio mix). Progress
/// is sent as "export-progress" events: { id, pct }.
#[tauri::command(async)]
fn export_clip(core: St, id: String, opts: crate::editor::ExportOpts) -> Result<ClipMeta, String> {
    let meta = core.lib.lock().unwrap().get(&id).ok_or("Clip not found")?;
    let emitter = core.inner().clone();
    let source = id.clone();
    let mut progress = |pct: f32| emitter.emit("export-progress", serde_json::json!({ "id": source, "pct": pct }));
    let new = crate::editor::export(&core.settings(), &meta, &opts, &mut progress)?;
    core.lib.lock().unwrap().add(new.clone());
    core.emit("clips-changed", new.id.clone());
    Ok(new)
}

/// Loudness outlines of the clip's audio streams, for the timeline's waveforms.
#[tauri::command(async)]
fn editor_waveforms(core: St, id: String) -> Result<Vec<crate::editor::Waveform>, String> {
    let meta = core.lib.lock().unwrap().get(&id).ok_or("Clip not found")?;
    crate::editor::waveforms(&core.settings(), &meta)
}

/// Small frames across the clip, for the timeline's film strip.
#[tauri::command(async)]
fn editor_thumbs(core: St, id: String, count: u32) -> Result<Vec<String>, String> {
    let meta = core.lib.lock().unwrap().get(&id).ok_or("Clip not found")?;
    crate::editor::thumbs(&core.settings(), &meta, count)
}

/// Describe an audio file the user dropped on the editor.
#[tauri::command(async)]
fn editor_audio_info(core: St, path: String) -> Result<crate::editor::AudioInfo, String> {
    crate::editor::audio_info(&core.settings(), &path)
}

#[tauri::command(async)]
fn pick_audio_file() -> Option<String> {
    rfd::FileDialog::new().add_filter("Audio", &["mp3", "wav", "m4a", "aac", "ogg", "opus", "flac", "wma", "mp4", "mkv", "mov", "webm"]).pick_file().map(|p| p.to_string_lossy().into_owned())
}

/// What is known about updates (from the last check; no network).
#[tauri::command(async)]
fn update_state() -> crate::update::UpdateInfo {
    crate::update::load()
}

/// Ask GitHub now.
#[tauri::command(async)]
fn update_check(core: St) -> crate::update::UpdateInfo {
    let info = crate::update::check();
    core.emit("update-changed", info.clone());
    info
}

/// Download the latest release, put it in place of this program and restart.
/// Progress is sent as "update-progress" events: { pct }.
#[tauri::command(async)]
fn update_install(app: AppHandle, core: St) -> Result<(), String> {
    let info = crate::update::load();
    if !info.available {
        return Err("Glint is already up to date".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("Glint's own folder was not found")?.to_path_buf();
    let emitter = core.inner().clone();
    crate::update::install_into(&info, &dir, &mut |pct| emitter.emit("update-progress", serde_json::json!({ "pct": pct })))?;
    util::log(&format!("update: installed {} over {}; restarting", info.latest, info.current));
    crate::update::relaunch_soon(&dir.join("Glint.exe"));
    app.exit(0);
    Ok(())
}

#[tauri::command(async)]
fn list_jobs(core: St) -> Vec<crate::jobs::Job> {
    core.jobs.list()
}

#[tauri::command(async)]
fn cancel_job(core: St, id: String) -> bool {
    core.jobs.cancel(&id)
}

#[tauri::command(async)]
fn clear_jobs(core: St) {
    core.jobs.clear_finished();
}

#[tauri::command(async)]
fn import_sources() -> Vec<crate::import::Source> {
    crate::import::sources()
}

#[tauri::command(async)]
fn import_scan(core: St, path: String) -> Result<Vec<crate::import::Found>, String> {
    crate::import::scan(core.inner(), &path)
}

#[tauri::command(async)]
fn import_run(core: St, items: Vec<crate::import::Found>, copy: bool) -> Result<usize, String> {
    crate::import::run(core.inner(), items, copy)
}

#[tauri::command(async)]
fn pick_import_folder() -> Option<String> {
    rfd::FileDialog::new().pick_folder().map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command(async)]
fn blur_clip(core: St, id: String) -> Result<(), String> {
    core.inner().blur_clip(id)
}

#[tauri::command]
fn suspend_hotkeys(on: bool) {
    hotkeys::suspend(on);
}

/// A keybind pressed while Glint's own window has focus. Windows does not feed
/// the global hook in that case, so the window reports the press itself.
#[tauri::command(async)]
fn fire_keybind(core: St, id: String) {
    core.inner().on_hotkey(&id);
}

#[tauri::command]
fn win_action(window: tauri::WebviewWindow, action: String) {
    let _ = match action.as_str() {
        "min" => window.minimize(),
        "max" => {
            if window.is_maximized().unwrap_or(false) { window.unmaximize() } else { window.maximize() }
        }
        "close" => window.close(),
        _ => Ok(()),
    };
}

fn build_tray(app: &AppHandle, core: Arc<Core>) -> tauri::Result<()> {
    // The tray menu is built once, so a language change shows here after the next start.
    let lang = core.settings().language;
    let tr = |text: &'static str| crate::lang::tr(&lang, text);
    let open = MenuItem::with_id(app, "open", tr("Open Glint"), true, None::<&str>)?;
    let clip = MenuItem::with_id(app, "clip", tr("Save clip now"), true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", tr("Pause / resume clipping"), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", tr("Quit Glint"), true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &clip, &toggle, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main").tooltip("Glint").menu(&menu).show_menu_on_left_click(false);
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.on_menu_event(move |app, event| match event.id.as_ref() {
        "open" => show_main(app),
        "clip" => core.clip(core.settings().default_clip_secs()),
        "toggle" => {
            core.toggle_recording();
            core.emit("status-changed", ());
        }
        "quit" => {
            core.rec.stop();
            app.exit(0);
        }
        _ => {}
    })
    .on_tray_icon_event(|tray, event| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
            show_main(tray.app_handle());
        }
    })
    .build(app)?;
    Ok(())
}

pub fn run(minimized: bool) {
    let core = Core::new();
    let setup_core = core.clone();
    let run_core = core.clone();

    let app = tauri::Builder::default()
        // A second launch lands here, in the copy already running. `Glint.exe --clip`
        // saves a clip (for scripts and macro keys), `--show-overlay` previews the
        // clip overlay; anything else opens the window.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            let core = app.state::<Arc<Core>>().inner().clone();
            if args.iter().any(|a| a == "--clip") {
                util::log("command line: --clip");
                core.clip(core.settings().default_clip_secs());
            } else if args.iter().any(|a| a == "--show-overlay") {
                preview_overlay(&core);
            } else {
                show_main(app);
            }
        }))
        .manage(core)
        .invoke_handler(tauri::generate_handler![
            get_state,
            save_settings,
            get_status,
            toggle_recording,
            clip_now,
            list_clips,
            rescan_library,
            rename_clip,
            set_favorite,
            delete_clip,
            copy_clip,
            delete_clips,
            copy_clips,
            set_favorites,
            set_game,
            sort_by_game,
            list_sounds,
            preview_sound,
            pick_sound_file,
            preview_toast,
            list_games,
            pick_game_icon,
            move_clips,
            list_folders,
            create_folder,
            rename_folder,
            delete_folder,
            reveal_clip,
            open_clips_dir,
            pick_folder,
            trim_clip,
            editor_audio_tracks,
            editor_waveforms,
            editor_thumbs,
            editor_audio_info,
            pick_audio_file,
            update_state,
            update_check,
            update_install,
            list_jobs,
            cancel_job,
            clear_jobs,
            import_sources,
            import_scan,
            import_run,
            pick_import_folder,
            export_clip,
            blur_clip,
            suspend_hotkeys,
            fire_keybind,
            win_action
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let core = setup_core;
            let _ = core.app.set(handle.clone());
            let _ = app.asset_protocol_scope().allow_directory(library::thumbs_dir(), true);
            let _ = app.asset_protocol_scope().allow_directory(crate::games::icons_dir(), true);
            build_tray(&handle, core.clone())?;
            core.watch();

            // Updates: clear what the last update left behind, then look for a new
            // release shortly after start and about once a day after that.
            crate::update::cleanup();
            let up_core = core.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(25));
                loop {
                    if up_core.settings().update_check && crate::update::due(&crate::update::load()) {
                        let info = crate::update::check();
                        if !info.error.is_empty() {
                            util::log(&format!("update check: {}", info.error));
                        } else if info.available {
                            util::log(&format!("update check: {} is available", info.latest));
                        }
                        up_core.emit("update-changed", info);
                    }
                    thread::sleep(Duration::from_secs(3600));
                }
            });

            let (tx, rx) = std::sync::mpsc::channel::<String>();
            hotkeys::start(tx);
            let hk_core = core.clone();
            thread::spawn(move || {
                for id in rx {
                    match id.strip_prefix('!') {
                        Some(note) => util::log(note),
                        None => hk_core.on_hotkey(&id),
                    }
                }
            });

            if core.settings().record_on_start {
                core.start_recording();
            }
            if !minimized {
                show_main(&handle);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to start Glint");

    app.run(move |_app, event| match event {
        RunEvent::ExitRequested { api, code, .. } => {
            // Closing the window keeps the recorder alive in the tray.
            if code.is_none() && run_core.settings().close_to_tray {
                api.prevent_exit();
            }
        }
        RunEvent::Exit => {
            // Stop renders first so no blur-cli is left running without Glint.
            run_core.jobs.shutdown();
            run_core.rec.stop();
        }
        _ => {}
    });
}
