#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod bench;
mod blur;
mod bugreport;
mod capture2;
mod clip;
mod compress;
mod core;
mod denoise;
mod deps;
mod discord;
mod editor;
mod games;
mod hotkeys;
mod icons;
mod import;
mod jobs;
mod join;
mod keys;
mod lang;
mod library;
mod naming;
mod polish;
mod procaudio;
mod projects;
mod recorder;
mod settings;
mod sound;
mod spill;
mod update;
mod util;
mod win;

fn main() {
    util::now_secs(); // start the app clock
    if deps::fake_ffmpeg() {
        return; // --deps-test's stand-in for an old ffmpeg (only as ffmpeg.exe with GLINT_FAKE_FFMPEG=1)
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    update::wait_for_old_copy(&args); // started by the copy an update just replaced: let it end first
    if let Some(i) = args.iter().position(|a| a == "--deps-test") {
        // glint.exe --deps-test <report.txt>: Check and repair (downloads ffmpeg once into a scratch folder).
        deps::self_test(args.get(i + 1).map(String::as_str).unwrap_or("deps.txt"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--bench") {
        bench::run(&args[i + 1..]);
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--hotkey-test") {
        keys::self_test(args.get(i + 1).map(String::as_str).unwrap_or("hotkeys.json"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--blur-sync-test") {
        // glint.exe --blur-sync-test <report.txt>: what Blur is handed keeps audio and video in step (renders through Blur when it is installed).
        blur::sync_self_test(args.get(i + 1).map(String::as_str).unwrap_or("blur-sync.txt"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--blur-test") {
        // glint.exe --blur-test <in.mp4> <out.mp4> <report.txt>: run the Blur wrapper headless.
        let (Some(input), Some(output), Some(report)) = (args.get(i + 1), args.get(i + 2), args.get(i + 3)) else { return };
        let s = settings::Settings::load();
        let mut last = 0.0f32;
        let result = match blur::find(&s) {
            Some(cli) => blur::render(&cli, input.as_ref(), output.as_ref(), &s.blur, |p| last = p),
            None => Err("blur-cli.exe was not found".into()),
        };
        let _ = std::fs::write(report, format!("result={result:?} last_progress={last}"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--diag") {
        bench::diag(args.get(i + 1).map(String::as_str).unwrap_or("diag.txt"), args.get(i + 2).map(String::as_str).filter(|s| !s.is_empty()), args.get(i + 3).map(String::as_str));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--update-test") {
        update::self_test(args.get(i + 1).map(String::as_str).unwrap_or("update.txt"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--jobs-test") {
        jobs::self_test(args.get(i + 1).map(String::as_str).unwrap_or("jobs.txt"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--import-test") {
        import::self_test(args.get(i + 1).map(String::as_str).unwrap_or("import.txt"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--editor-test") {
        editor::self_test(args.get(i + 1).map(String::as_str).unwrap_or("editor.txt"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--denoise-test") {
        // glint.exe --denoise-test <in.raw> <out.raw> <rate> <report.txt>: clean raw stereo s16le audio.
        let (Some(input), Some(output), Some(rate), Some(report)) = (args.get(i + 1), args.get(i + 2), args.get(i + 3), args.get(i + 4)) else { return };
        let bytes = std::fs::read(input).unwrap_or_default();
        let mut pcm = audio::Pcm { rate: rate.parse().unwrap_or(48_000), data: bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect() };
        let secs = pcm.data.len() as f64 / 2.0 / pcm.rate as f64;
        let started = std::time::Instant::now();
        denoise::process(&mut pcm);
        let ms = started.elapsed().as_millis();
        let out: Vec<u8> = pcm.data.iter().flat_map(|v| v.to_le_bytes()).collect();
        let _ = std::fs::write(output, out);
        let _ = std::fs::write(report, format!("audio_secs={secs:.1} process_ms={ms}"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--sound-test") {
        // glint.exe --sound-test <dir>: write every built-in clip sound as a .wav file.
        let Some(dir) = args.get(i + 1) else { return };
        let _ = std::fs::create_dir_all(dir);
        for (id, _) in sound::SOUNDS {
            let _ = std::fs::write(std::path::Path::new(dir).join(format!("{id}.wav")), sound::wav(id, 100));
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--gameaudio-test") {
        // glint.exe --gameaudio-test <report.txt>: the "Game" audio track, from ring to clip to editor; then a silent live capture.
        procaudio::self_test(args.get(i + 1).map(String::as_str).unwrap_or("gameaudio.txt"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--buffer-test") {
        // glint.exe --buffer-test <report.txt>: the RAM / disk replay buffer on made-up data.
        recorder::self_test(args.get(i + 1).map(String::as_str).unwrap_or("buffer.txt"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--bug-test") {
        // glint.exe --bug-test <report.txt>: bug reports, sent to a listener on 127.0.0.1 only.
        bugreport::self_test(args.get(i + 1).map(String::as_str).unwrap_or("bug.txt"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--memory-test") {
        // glint.exe --memory-test <report.txt>: the memory figure of the whole process tree.
        win::memory_test(args.get(i + 1).map(String::as_str).unwrap_or("memory.txt"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--join-test") {
        // glint.exe --join-test <report.txt>: "Edit together" on made-up clips.
        join::self_test(args.get(i + 1).map(String::as_str).unwrap_or("join.txt"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--project-test") {
        // glint.exe --project-test <report.txt>: editor projects (add, rebuild, state, rename, delete, start-up sweep) on made-up clips.
        projects::self_test(args.get(i + 1).map(String::as_str).unwrap_or("project.txt"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--compress-test") {
        // glint.exe --compress-test <report.txt>: Compress for Discord on made-up clips (never the clipboard).
        compress::self_test(args.get(i + 1).map(String::as_str).unwrap_or("compress.txt"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--polish-test") {
        // glint.exe --polish-test <report.txt>: capture method choice, themed icon, thumbnail shapes.
        polish::self_test(args.get(i + 1).map(String::as_str).unwrap_or("polish.txt"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--icons-test") {
        // glint.exe --icons-test <report.txt>: game icons by name on a made-up PC, then a read-only count on this one.
        icons::self_test(args.get(i + 1).map(String::as_str).unwrap_or("icons.txt"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--folders-test") {
        bench::folders_test(args.get(i + 1).map(String::as_str).unwrap_or("folders.txt"));
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--naming-test") {
        // glint.exe --naming-test <report.txt>: clip name blocks, then bulk rename and undo on made-up clips.
        naming::self_test(args.get(i + 1).map(String::as_str).unwrap_or("naming.txt"));
        return;
    }
    std::thread::spawn(deps::clean_up_at_start); // old ffmpeg copies an earlier "Reinstall" moved aside
    app::run(args.iter().any(|a| a == "--minimized"));
}
