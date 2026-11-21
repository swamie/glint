//! The one way Glint's long background jobs run ffmpeg: compressing, "Edit together", GIFs and
//! upscaling each had a copy of this loop. It runs below normal priority (the game and the live
//! capture come first), reports progress, ends ffmpeg when the job is cancelled, and ends it too
//! when it has stopped making progress, so a hung ffmpeg no longer holds the job queue for good.
use crate::util;
use std::io::{BufRead, BufReader, Read};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

/// What a cancelled run returns (the same text in every job that uses this).
pub const CANCELLED: &str = "Cancelled";

/// ffmpeg is ended when its position in the output has not moved for this long. Generous: a slow
/// filter on a large frame may take a while per frame, but not minutes.
pub const STALL_SECS: u64 = 300;

/// Run ffmpeg, reporting 0..1 of `expected` seconds done. The position is read from stdout:
/// `out_time_us=` lines (`-progress pipe:1`) or `t=` lines (seconds; the GIF maker's filters).
/// Kills ffmpeg within about 0.1 s once `cancel` is set and returns `CANCELLED`.
/// On success returns what ffmpeg wrote to stderr (its warnings).
pub fn run(ffmpeg: &Path, args: &[String], expected: f64, cancel: &AtomicBool, on_frac: &mut dyn FnMut(f64)) -> Result<String, String> {
    let mut child = util::hidden_cmd(ffmpeg)
        .creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start ffmpeg: {e}"))?;
    util::track_child(&child); // ends with Glint (0.1.41)
    // Drained on its own thread so a chatty stderr can never block ffmpeg while we read progress.
    let err_reader = child.stderr.take().map(|mut err| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            buf
        })
    });
    let (tx, rx) = mpsc::channel::<f64>();
    if let Some(out) = child.stdout.take() {
        thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let line = line.trim();
                let secs = line
                    .strip_prefix("out_time_us=")
                    .and_then(|v| v.trim().parse::<f64>().ok())
                    .map(|us| us / 1e6)
                    .or_else(|| line.strip_prefix("t=").and_then(|v| v.trim().parse::<f64>().ok()));
                if let Some(secs) = secs {
                    if tx.send(secs).is_err() {
                        return;
                    }
                }
            }
        });
    }
    let kill = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };
    let stall = Duration::from_secs(STALL_SECS);
    let mut at = f64::NAN;
    let mut moved = Instant::now();
    loop {
        if cancel.load(Ordering::SeqCst) {
            kill(&mut child);
            return Err(CANCELLED.into());
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(t) => {
                if t != at {
                    at = t;
                    moved = Instant::now();
                }
                if expected > 0.0 && t.is_finite() {
                    on_frac((t / expected).clamp(0.0, 1.0));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if moved.elapsed() >= stall {
            kill(&mut child);
            let stderr = err_reader.and_then(|h| h.join().ok()).unwrap_or_default();
            util::log(&format!("ffmpeg made no progress for {STALL_SECS} s and was ended; it said: {}", util::last_lines(&stderr, 4)));
            return Err(format!("ffmpeg stopped making progress for {} minutes, so it was ended. The log has its last words.", STALL_SECS / 60));
        }
    }
    // Output closed: ffmpeg is finishing (the faststart rewrite). Still honour cancel.
    let status = loop {
        if cancel.load(Ordering::SeqCst) {
            kill(&mut child);
            return Err(CANCELLED.into());
        }
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("ffmpeg failed: {e}")),
        }
    };
    let stderr = err_reader.and_then(|h| h.join().ok()).unwrap_or_default();
    if status.success() {
        Ok(String::from_utf8_lossy(&stderr).into_owned())
    } else {
        let tail = util::last_lines(&stderr, 2);
        Err(if tail.is_empty() { format!("ffmpeg stopped with exit code {}", status.code().unwrap_or(-1)) } else { tail })
    }
}
