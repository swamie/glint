//! Smart rendering of a cuts-only export (0.1.46, agent S's recipe, CONTRACT S change 1): of every kept range only
//! its head (the frames from the range's first frame up to the next keyframe) is encoded; from that keyframe to the
//! range's end the source's own packets are copied. The heads (and the sound, cut as the full export cuts it) are
//! made in ONE ffmpeg run, then a concat-demuxer list joins heads and copied stretches without re-encoding.
//!
//! Only where that is exact (`eligible`): one H.264 4:2:0 source without B-frames (a picture's packets in display
//! order, so a copied stretch never needs a frame from before its keyframe), starting at time 0, the encoder NVENC,
//! and the heads' SPS the source's own, field for field (a decoder reads the joined stream with ONE set of
//! parameters). Everything else is the full re-encode.
use super::{run_ffmpeg, run_with_progress, CANCELLED};
use crate::library;
use crate::util;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// The ffprobe next to `ffmpeg`.
fn probe(ffmpeg: &Path) -> Result<PathBuf, String> {
    library::ffprobe_for(ffmpeg).ok_or_else(|| "no ffprobe".to_string())
}

fn probe_text(ffprobe: &Path, args: &[&str], file: &Path) -> String {
    util::hidden_cmd(ffprobe).args(args).arg(file).stdin(Stdio::null()).stderr(Stdio::null()).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// The first SPS of `file`'s video as `trace_headers` prints it (field lines only), up to its VUI: the fields that
/// decide how the pictures decode. The VUI (the rate the stream was made for, its clock, its colour tags) does not:
/// a recording made with a rate cap has HRD values there that the heads, made without one, do not.
fn sps_fields(ffmpeg: &Path, file: &Path) -> Vec<String> {
    let Ok(out) = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-nostdin", "-i"]).arg(file).args(["-map", "0:v:0", "-c", "copy", "-bsf:v", "trace_headers", "-frames:v", "1", "-f", "null", "-"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output() else { return Vec::new() };
    let text = String::from_utf8_lossy(&out.stderr);
    let mut fields = Vec::new();
    let mut inside = false;
    for l in text.lines() {
        let body = l.split_once("] ").map(|x| x.1).unwrap_or(l).trim();
        if body.starts_with("Sequence Parameter Set") {
            inside = true;
            continue;
        }
        if inside {
            if body.starts_with("Picture Parameter Set") || body.starts_with("Slice") || body.starts_with("Access Unit") || body.starts_with("Supplemental") {
                break;
            }
            // "<bit offset>  <name>  <bits> = <value>": the name and the value.
            let parts: Vec<&str> = body.split_whitespace().collect();
            if parts.len() >= 4 && parts[parts.len() - 2] == "=" {
                if parts[1] == "vui_parameters_present_flag" {
                    break;
                }
                fields.push(format!("{}={}", parts[1], parts[parts.len() - 1]));
            }
        }
    }
    fields
}

/// The encoder arguments of the heads (Glint's export settings for NVENC, no B-frames, high profile).
fn head_codec() -> Vec<String> {
    ["-c:v", "h264_nvenc", "-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", "19", "-b:v", "0", "-bf", "0", "-profile:v", "high", "-g", "600", "-forced-idr", "1", "-pix_fmt", "yuv420p"].map(String::from).to_vec()
}

/// Why `file` cannot be smart-rendered with `enc`, or Ok. Checks the source and encodes a 4-frame head to compare
/// its SPS with the source's.
pub(super) fn eligible(ffmpeg: &Path, file: &Path, enc: &str) -> Result<(), String> {
    if enc != "nvenc" {
        return Err(format!("the encoder is {enc} (heads must match the source: NVENC only)"));
    }
    let ffprobe = probe(ffmpeg)?;
    let v = probe_text(&ffprobe, &["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_name,pix_fmt,has_b_frames,start_time", "-of", "default=nw=1"], file);
    let get = |k: &str| v.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).unwrap_or("").trim().to_string();
    if get("codec_name") != "h264" || get("pix_fmt") != "yuv420p" {
        return Err(format!("the clip is {} {}", get("codec_name"), get("pix_fmt")));
    }
    if get("has_b_frames") != "0" {
        return Err("the clip has B-frames".into());
    }
    // A head made the way the export makes them: its SPS must be the source's.
    let dir = util::tmp_dir();
    let _ = fs::create_dir_all(&dir);
    let head = dir.join(format!("smart-sps-{}.mp4", util::new_id()));
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y", "-i"].map(String::from).to_vec();
    a.push(file.to_string_lossy().into_owned());
    a.extend(["-frames:v", "4", "-an"].map(String::from));
    a.extend(head_codec());
    a.push(head.to_string_lossy().into_owned());
    let args: Vec<&str> = a.iter().map(String::as_str).collect();
    let made = run_ffmpeg(ffmpeg, &args);
    let (src_sps, head_sps) = (sps_fields(ffmpeg, file), if made.is_ok() { sps_fields(ffmpeg, &head) } else { Vec::new() });
    let _ = fs::remove_file(&head);
    made?;
    if src_sps.is_empty() || src_sps != head_sps {
        let diff: Vec<String> = src_sps.iter().zip(&head_sps).filter(|(x, y)| x != y).take(3).map(|(x, y)| format!("{x} vs {y}")).collect();
        return Err(format!("the heads' SPS is not the clip's ({} vs {} fields; {})", src_sps.len(), head_sps.len(), diff.join(", ")));
    }
    Ok(())
}

/// The video packets of `file`: (pts seconds from the file's start, keyframe), in time order. From the start:
/// `-ss` and the concat demuxer's `inpoint` both count from it (a joined draft can start at 0.02 s).
fn packets(ffmpeg: &Path, file: &Path) -> Result<Vec<(f64, bool)>, String> {
    let ffprobe = probe(ffmpeg)?;
    let start: f64 = probe_text(&ffprobe, &["-v", "error", "-show_entries", "format=start_time", "-of", "csv=p=0"], file).trim().parse().unwrap_or(0.0);
    let text = probe_text(&ffprobe, &["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time,flags", "-of", "csv=p=0"], file);
    let mut p: Vec<(f64, bool)> = text.lines().filter_map(|l| { let mut it = l.split(','); Some((it.next()?.trim().parse::<f64>().ok()? - start, it.next().unwrap_or("").starts_with('K'))) }).collect();
    p.sort_by(|a, b| a.0.total_cmp(&b.0));
    if p.is_empty() {
        return Err("no video packets".into());
    }
    Ok(p)
}

fn f6(x: f64) -> String {
    format!("{x:.6}")
}

/// One concat-list path, quoted for the concat demuxer.
fn quoted(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\\', "/").replace('\'', "'\\''"))
}

/// Smart-render `ranges` (source seconds, in result order, frame times) of `file` to `out`. `titles`: the audio
/// tracks' titles (one entry per audio stream). Heads and sound in one run, then the join. Returns how many heads were
/// encoded and how many frames were copied, for the log.
pub(super) fn export(ffmpeg: &Path, file: &Path, ranges: &[(f64, f64)], titles: &[Option<String>], title: &str, out: &Path, progress: &mut dyn FnMut(f32)) -> Result<(usize, usize), String> {
    let pk = packets(ffmpeg, file)?;
    // The heads in the clip's own clock: the concat demuxer does not rescale a copied stretch's packet times to the
    // first file's (a 1/15360 recording after 1/90000 heads came out 11 microseconds a frame).
    let tb: u64 = {
        let ffprobe = probe(ffmpeg)?;
        let v = probe_text(&ffprobe, &["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=time_base", "-of", "csv=p=0"], file);
        v.trim().trim_end_matches(',').strip_prefix("1/").and_then(|x| x.parse().ok()).filter(|x| *x >= 1000).ok_or_else(|| format!("the clip's clock is {}", v.trim()))?
    };
    let t: Vec<f64> = pk.iter().map(|p| p.0).collect();
    let last_le = |v: f64| -> Option<usize> { let i = t.partition_point(|x| *x <= v); i.checked_sub(1) };
    let dir = util::tmp_dir().join(format!("smart-{}", util::new_id()));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    struct Clean(PathBuf);
    impl Drop for Clean {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _clean = Clean(dir.clone());
    let heads = dir.join("heads.mp4");
    let sound = dir.join("sound.mka");
    let src = file.to_string_lossy().into_owned();
    let n = ranges.len();
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin", "-y"].map(String::from).to_vec();
    let mut g: Vec<String> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    let mut list = String::from("ffconcat version 1.0\n");
    let (mut slot, mut nh, mut copied) = (0.0f64, 0usize, 0usize);
    let mut total = 0.0;
    for &(ra, rb) in ranges {
        total += rb - ra;
        let fa = last_le(ra + 0.0005).unwrap_or(0);
        // The last frame before the range's end (frames before end - 0.5 ms, as the full export's `trim`).
        let fb = last_le(rb - 0.0005 - 1e-7).unwrap_or(fa).max(fa);
        let mut k1 = fa;
        while k1 <= fb && !pk[k1].1 {
            k1 += 1;
        }
        if k1 > fa {
            // A head: frames fa .. k1-1, encoded, laid in `heads` from `slot` on (a keyframe there).
            let seek = (t[fa] - 0.0005).max(0.0);
            let endt = if k1 <= fb { t[k1] } else { rb };
            let len = endt - ra;
            if n > 4 {
                a.extend(["-threads", "4"].map(String::from));
            }
            a.extend(["-reinit_filter".into(), "0".into(), "-ss".into(), f6(seek), "-t".into(), f6(endt - seek + 0.3), "-i".into(), src.clone()]);
            g.push(format!("[{nh}:v:0]trim=end={},setpts='({}+if(eq(N,0),0,max(0,T-{})))/TB'[h{nh}]", f6(endt - seek - 0.0005), f6(slot), f6(ra - seek)));
            keys.push(f6(slot));
            list += &format!("file {}\ninpoint {}\noutpoint {}\nduration {}\n", quoted(&heads), f6(slot), f6(slot + len - 0.0005), f6(len));
            slot += len.ceil() + 1.0;
            nh += 1;
        }
        if k1 <= fb {
            copied += fb + 1 - k1;
            list += &format!("file {}\ninpoint {}\noutpoint {}\nduration {}\n", quoted(file), f6(t[k1]), f6(rb - 0.0005), f6(rb - t[k1]));
        }
    }
    // The sound: every track cut as the full export's Tracks plan cuts it (asplit, atrim, concat), one input.
    let base = ranges.iter().map(|r| r.0).fold(f64::INFINITY, f64::min);
    let top = ranges.iter().map(|r| r.1).fold(f64::NEG_INFINITY, f64::max);
    let ai = nh;
    if !titles.is_empty() {
        a.extend(["-reinit_filter".into(), "0".into(), "-ss".into(), f6(base), "-t".into(), f6(top - base), "-i".into(), src.clone()]);
    }
    let mut amaps: Vec<String> = Vec::new();
    for i in 0..titles.len() {
        let mut s = format!("[{ai}:a:{i}]asplit={n}");
        for k in 0..n {
            s += &format!("[t{i}c{k}]");
        }
        let mut parts = String::new();
        for (k, &(ra, rb)) in ranges.iter().enumerate() {
            s += &format!(";[t{i}c{k}]atrim=start={}:end={},asetpts=PTS-STARTPTS[t{i}s{k}]", f6(ra - base), f6(rb - base));
            parts += &format!("[t{i}s{k}]");
        }
        g.push(format!("{s};{parts}concat=n={n}:v=0:a=1[a{i}]"));
        amaps.extend(["-map".into(), format!("[a{i}]")]);
    }
    if nh > 0 {
        let labels: String = (0..nh).map(|k| format!("[h{k}]")).collect();
        g.push(format!("{labels}interleave=nb_inputs={nh},format=yuv420p[hv]"));
    }
    if !g.is_empty() {
        a.extend(["-filter_complex".into(), g.join(";")]);
    }
    if nh > 0 {
        a.extend(["-map".into(), "[hv]".into()]);
        a.extend(head_codec());
        a.extend(["-force_key_frames".into(), keys.join(","), "-fps_mode".into(), "passthrough".into(), "-enc_time_base:v".into(), format!("1/{tb}"), "-video_track_timescale".into(), tb.to_string(), "-an".into()]);
        a.push(heads.to_string_lossy().into_owned());
    }
    if !amaps.is_empty() {
        a.extend(amaps);
        a.extend(["-c:a", "aac", "-b:a", "160k", "-vn"].map(String::from));
        a.push(sound.to_string_lossy().into_owned());
    }
    if nh > 0 || !titles.is_empty() {
        let mut half = |p: f32| progress(p * 0.6);
        a.extend(["-progress".into(), "pipe:1".into()]);
        run_with_progress(ffmpeg, &a, total, &mut half)?;
    }
    let lst = dir.join("list.txt");
    fs::write(&lst, list).map_err(|e| e.to_string())?;
    let mut j: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-f", "concat", "-safe", "0", "-i"].map(String::from).to_vec();
    j.push(lst.to_string_lossy().into_owned());
    if !titles.is_empty() {
        j.extend(["-i".into(), sound.to_string_lossy().into_owned(), "-map".into(), "0:v".into(), "-map".into(), "1:a".into()]);
        for (i, tt) in titles.iter().enumerate() {
            j.extend([format!("-metadata:s:a:{i}"), format!("title={}", tt.clone().unwrap_or_else(|| format!("Track {}", i + 1))), format!("-disposition:a:{i}"), if i == 0 { "default".into() } else { "0".into() }]);
        }
    } else {
        j.extend(["-map".into(), "0:v".into()]);
    }
    j.extend(["-c".into(), "copy".into(), "-metadata".into(), format!("title={title}"), "-movflags".into(), "+faststart".into(), "-progress".into(), "pipe:1".into()]);
    j.push(out.to_string_lossy().into_owned());
    let mut rest = |p: f32| progress(60.0 + p * 0.4);
    run_with_progress(ffmpeg, &j, total, &mut rest).map_err(|e| if e == CANCELLED { e } else { format!("the join: {e}") })?;
    Ok((nh, copied))
}
