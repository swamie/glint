//! Pictures, GIFs and clips on the editor's picture, and round / soft blur
//! boxes (0.1.35). A child module of `editor`: it uses the export's layer
//! types (`Layers`, `Moving`, `Overlay`, `BlurBox`) and the self-test helpers.
//!
//! A media item comes to the export as an `Overlay` with `media` set: its box
//! `x`, `y`, `w`, `h` (fractions of the picture: the part of the media that is
//! shown, after its crop), its OUTPUT time range `from`..`to`, and maybe a
//! keyframe `track` (the same moves as a moving overlay picture). The UI sends
//! one entry per kept range the item covers, each with its own `off`, so a cut
//! in the video under a clip item skips the same seconds of that clip as the
//! preview does.
//!
//! A blur box with `round` or `soft` gets a mask: a grey picture of the box's
//! size (`mask_alpha`, the same formula as `medMaskAlpha` in `ui/media.js`)
//! written as a PGM next to the overlay PNGs, laid on the blurred copy with
//! `alphamerge`, so the blur fades out toward its edge and its corners.
use super::*;

/// The part of a media item that is cut off, as fractions of its own picture
/// (left, top, right, bottom). All 0 = the whole picture.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct MediaCrop {
    pub l: f64,
    pub t: f64,
    pub r: f64,
    pub b: f64,
}

/// What a media overlay shows: a still ("image"), an animated GIF ("gif") or
/// a video file ("video", its picture only).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct MediaSrc {
    pub path: String,
    pub kind: String,
    /// Seconds into the media at the entry's `from`.
    pub off: f64,
    /// The media's own length in seconds (0 for a still).
    pub mlen: f64,
    /// Starts over at its end (a GIF does by default).
    #[serde(rename = "loop")]
    pub looped: bool,
    /// Media seconds per second of the result (the edit's speed).
    pub speed: f64,
    pub crop: MediaCrop,
    /// 0..1.
    pub opacity: f64,
}

impl Default for MediaSrc {
    fn default() -> Self {
        MediaSrc { path: String::new(), kind: "image".into(), off: 0.0, mlen: 0.0, looped: false, speed: 1.0, crop: MediaCrop::default(), opacity: 1.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum MediaKind {
    Still,
    Gif,
    Video,
}

/// A media item checked for the export.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct MediaPlan {
    pub kind: MediaKind,
    pub path: String,
    pub off: f64,
    pub looped: bool,
    pub speed: f64,
    /// (l, t, r, b), each side at most what leaves 1 % of the picture.
    pub crop: (f64, f64, f64, f64),
    pub opacity: f64,
    /// Its box (fractions of the picture) before any keyframe move.
    pub bx: (f64, f64, f64, f64),
}

/// `o`'s media checked: a known kind, an existing file, numbers in range.
/// None when the box is too small to show.
pub(super) fn media_plan(o: &Overlay, ms: &MediaSrc) -> Result<Option<MediaPlan>, String> {
    let fin = |v: f64, d: f64| if v.is_finite() { v } else { d };
    let kind = match ms.kind.as_str() {
        "gif" => MediaKind::Gif,
        "video" => MediaKind::Video,
        _ => MediaKind::Still,
    };
    if !Path::new(&ms.path).is_file() {
        let name = Path::new(&ms.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return Err(format!("A picture or clip of the edit is missing: {name}. Was it moved or deleted?"));
    }
    let (w, h) = (fin(o.w, 0.0).min(4.0), fin(o.h, 0.0).min(4.0));
    if w < 0.0005 || h < 0.0005 {
        return Ok(None);
    }
    let side = |v: f64| fin(v, 0.0).clamp(0.0, 0.99);
    let (mut l, mut t, mut r, mut b) = (side(ms.crop.l), side(ms.crop.t), side(ms.crop.r), side(ms.crop.b));
    if l + r > 0.99 {
        let k = 0.99 / (l + r);
        (l, r) = (l * k, r * k);
    }
    if t + b > 0.99 {
        let k = 0.99 / (t + b);
        (t, b) = (t * k, b * k);
    }
    let mlen = fin(ms.mlen, 0.0).max(0.0);
    let mut off = fin(ms.off, 0.0).max(0.0);
    if ms.looped && mlen > 0.01 {
        off %= mlen;
    }
    Ok(Some(MediaPlan {
        kind,
        path: ms.path.clone(),
        off,
        looped: ms.looped,
        speed: fin(ms.speed, 1.0).clamp(0.25, 4.0),
        crop: (l, t, r, b),
        opacity: fin(ms.opacity, 1.0).clamp(0.0, 1.0),
        bx: (fin(o.x, 0.0).clamp(-4.0, 4.0), fin(o.y, 0.0).clamp(-4.0, 4.0), w, h),
    }))
}

/// The ffmpeg input options of a media item shown `a`..`b` seconds of a
/// result `total` long (`moving`: it has a keyframe track; `rate`: the frame
/// rate a moving still is looped at).
pub(super) fn media_input_args(mp: &MediaPlan, (a, b): (f64, f64), moving: bool, rate: u32, total: f64) -> Vec<String> {
    let read = (b - a).max(0.0) * mp.speed + 0.5;
    let mut v: Vec<String> = Vec::new();
    match mp.kind {
        MediaKind::Still => {
            if moving {
                v.extend(["-loop".to_string(), "1".into(), "-framerate".into(), rate.to_string(), "-t".into(), format!("{:.3}", total + 1.0)]);
            }
            v.extend(["-f", "image2", "-pattern_type", "none"].map(String::from));
        }
        MediaKind::Gif => {
            // The GIF demuxer plays it again at its end (`ignore_loop 0`); its timestamps run on, so `trim` finds `off`.
            v.extend(["-f".to_string(), "gif".into(), "-ignore_loop".into(), if mp.looped { "0".into() } else { "1".into() }]);
            v.extend(["-t".to_string(), format!("{:.3}", mp.off + read)]);
        }
        MediaKind::Video => {
            if mp.looped {
                v.extend(["-stream_loop", "-1"].map(String::from));
            }
            v.extend(["-ss".to_string(), format!("{:.6}", mp.off), "-t".into(), format!("{read:.3}")]);
        }
    }
    v.extend(["-i".to_string(), mp.path.clone()]);
    v
}

/// The chain that lays media item `j` (ffmpeg input `input`) from `[cur]` to
/// `[vo{j}]` on a `pw` x `ph` picture between `a` and `b` seconds: its frames
/// placed on the result's time (from `off`, at the edit's speed), cropped,
/// made see-through by `opacity`, scaled to its box (and moved along its
/// track, as a moving overlay picture is).
pub(super) fn media_chain(mp: &MediaPlan, moving: Option<&Moving>, input: usize, cur: &str, j: usize, (pw, ph): (u32, u32), (a, b): (f64, f64), rate: u32) -> String {
    let mut f: Vec<String> = Vec::new();
    if mp.kind == MediaKind::Gif && mp.off > 0.0005 {
        f.push(format!("trim=start={}", ex_num(mp.off)));
    }
    if mp.kind != MediaKind::Still {
        f.push(format!("setpts=(PTS-STARTPTS)/{}+{}/TB", ex_num(mp.speed), ex_num(a)));
        if moving.is_some() {
            // A frame for every size along the track, even while the media holds one picture.
            f.push(format!("fps={rate}"));
        }
    }
    f.push("format=rgba".into());
    let (l, t, r, bm) = mp.crop;
    if l + t + r + bm > 1e-6 {
        f.push(format!("crop=w=iw*{}:h=ih*{}:x=iw*{}:y=ih*{}", ex_num(1.0 - l - r), ex_num(1.0 - t - bm), ex_num(l), ex_num(t)));
    }
    if mp.opacity < 0.999 {
        f.push(format!("colorchannelmixer=aa={}", ex_num(mp.opacity)));
    }
    let on = format!("enable='between(t,{a:.4},{b:.4})'");
    let (bx, by, bw, bh) = mp.bx;
    match moving {
        None => {
            let w = ((bw * pw as f64).round() as i64).max(2);
            let h = ((bh * ph as f64).round() as i64).max(2);
            let (x, y) = ((bx * pw as f64).round() as i64, (by * ph as f64).round() as i64);
            f.push(format!("scale={w}:{h}"));
            format!("[{input}:v]{}[op{j}];[{cur}][op{j}]overlay={x}:{y}:{on}[vo{j}]", f.join(","))
        }
        Some(mv) => {
            let (sx, sy, dx, dy) = (move_expr(&mv.pts, 2), move_expr(&mv.pts, 3), move_expr(&mv.pts, 0), move_expr(&mv.pts, 1));
            let (w0, h0) = (ex_num(pw as f64 * bw), ex_num(ph as f64 * bh));
            let (mx, my) = (ex_num(bx + bw / 2.0), ex_num(by + bh / 2.0));
            f.push(format!("scale=w='max(2,{w0}*({sx}))':h='max(2,{h0}*({sy}))':eval=frame"));
            format!("[{input}:v]{}[op{j}];[{cur}][op{j}]overlay=x='{pw}*({mx}+({dx}))-overlay_w/2':y='{ph}*({my}+({dy}))-overlay_h/2':{on}[vo{j}]", f.join(","))
        }
    }
}

/* ---------- round / soft blur boxes ---------- */

/// How much of the blur shows at pixel centre (`px`, `py`) of a `w` x `h`
/// box (0..1): `round` 0..1 rounds the corners (1 = an ellipse; corner radii
/// `round * w / 2` by `round * h / 2`), `soft` 0..1 fades it out over that
/// part of half the box's smaller side toward the edge (smoothstep). The UI's
/// `medMaskAlpha` is the same formula.
pub(super) fn mask_alpha(px: f64, py: f64, w: f64, h: f64, round: f64, soft: f64) -> f64 {
    let (ex, ey) = (px.min(w - px), py.min(h - py));
    let mut d = ex.min(ey);
    let (rx, ry) = (round * w / 2.0, round * h / 2.0);
    if rx > 0.5 && ry > 0.5 && ex < rx && ey < ry {
        let (qx, qy) = ((rx - ex) / rx, (ry - ey) / ry);
        d = d.min((1.0 - (qx * qx + qy * qy).sqrt()) * rx.min(ry));
    }
    let feather = soft * w.min(h) / 2.0;
    if feather > 0.5 {
        let u = (d / feather).clamp(0.0, 1.0);
        u * u * (3.0 - 2.0 * u)
    } else {
        (d + 0.5).clamp(0.0, 1.0)
    }
}

/// True when blur box values ask for a mask.
pub(super) fn wants_mask(round: f64, soft: f64) -> bool {
    round > 0.005 || soft > 0.005
}

/// A `w` x `h` mask as a binary PGM (P5) file in the overlay folder.
fn write_mask(w: u32, h: u32, round: f64, soft: f64) -> Result<PathBuf, String> {
    let dir = overlay_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut bytes = format!("P5\n{w} {h}\n255\n").into_bytes();
    bytes.reserve((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            bytes.push((mask_alpha(x as f64 + 0.5, y as f64 + 0.5, w as f64, h as f64, round, soft) * 255.0).round() as u8);
        }
    }
    let path = dir.join(format!("{}.pgm", util::new_id()));
    fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// The pixel size of blur box `i`'s blurred copy on a `pw` x `ph` picture:
/// the box itself, or for a moving box the fixed size `moving_blur_chain`
/// cuts it at.
fn blur_copy_size(l: &Layers, i: usize, (pw, ph): (u32, u32)) -> (u32, u32) {
    let &(x, y, w, h, _, _, _, ref moving, _) = &l.blurs[i];
    let even = |v: f64| ((v / 2.0).round() as i64 * 2).max(0) as u32;
    if let Some(mv) = moving {
        let ev = |v: f64| (((v / 2.0).round() as i64) * 2).max(2) as u32;
        let min_of = |k: usize| mv.pts.iter().map(|p| p.1[k]).fold(f64::INFINITY, f64::min);
        return (ev(w * min_of(2) * pw as f64).min(pw & !1), ev(h * min_of(3) * ph as f64).min(ph & !1));
    }
    let bx = even(x * pw as f64).min(pw.saturating_sub(2));
    let by = even(y * ph as f64).min(ph.saturating_sub(2));
    (even(w * pw as f64).clamp(2, pw - bx), even(h * ph as f64).clamp(2, ph - by))
}

/// The masks of the export's round / soft blur boxes (one per box, None for a
/// plain one), written for a `size` picture; removed when the result drops.
pub(super) fn write_blur_masks(l: &Layers, size: (u32, u32)) -> Result<(OverlayFiles, Vec<Option<PathBuf>>), String> {
    let mut files = OverlayFiles(Vec::new());
    let mut out = Vec::new();
    for i in 0..l.blurs.len() {
        let (round, soft) = l.blurs[i].8;
        if !wants_mask(round, soft) {
            out.push(None);
            continue;
        }
        let (w, h) = blur_copy_size(l, i, size);
        let p = write_mask(w, h, round, soft)?;
        files.0.push(p.clone());
        out.push(Some(p));
    }
    Ok((files, out))
}

/// The inputs every overlay picture, media item and mask needs, in the order
/// `layer_chain` numbers them (from its `first_png`).
pub(super) fn layer_input_args(l: &Layers, masks: &[Option<PathBuf>], rate: u32, total: f64) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    for (png, from, to, moving, media) in &l.overlays {
        if let Some(mp) = media {
            a.extend(media_input_args(mp, (*from, *to), moving.is_some(), rate, total));
            continue;
        }
        if moving.is_some() {
            // A frame `rate` times a second for the whole result (the overlay ends with the picture: `shortest`).
            a.extend(["-loop".to_string(), "1".into(), "-framerate".into(), rate.to_string(), "-t".into(), format!("{:.3}", total + 1.0)]);
        }
        // One still picture each; `pattern_type none` so no character of the path is read as a frame number pattern.
        a.extend(["-f", "image2", "-pattern_type", "none", "-i"].map(String::from));
        a.push(png.clone());
    }
    for p in masks.iter().flatten() {
        a.extend(["-f", "image2", "-pattern_type", "none", "-i"].map(String::from));
        a.push(p.to_string_lossy().into_owned());
    }
    a
}

/// The ffmpeg input number of blur box `i`'s mask (masks follow the overlays).
pub(super) fn mask_input(masks: &[Option<PathBuf>], i: usize, first_mask: usize) -> Option<usize> {
    masks.get(i)?.as_ref()?;
    Some(first_mask + masks[..i].iter().filter(|m| m.is_some()).count())
}

/* ---------- what a file is ---------- */

/// A picture, GIF or video file the user picked for the picture.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct MediaInfo {
    pub path: String,
    /// "image", "gif" or "video".
    pub kind: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    /// Seconds (0 for a still).
    pub duration: f64,
    /// A video file with sound (the UI offers to add it as an audio row).
    pub audio: bool,
}

const STILL_EXT: [&str; 6] = ["png", "jpg", "jpeg", "bmp", "webp", "jfif"];

/// The file dialog's filter: pictures, GIFs and videos.
pub const MEDIA_EXT: [&str; 13] = ["png", "jpg", "jpeg", "bmp", "webp", "jfif", "gif", "mp4", "mkv", "mov", "webm", "avi", "m4v"];

/// Check that `path` is a picture, GIF or video and describe it (ffprobe).
pub fn media_info(s: &Settings, path: &str) -> Result<MediaInfo, String> {
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    media_info_with(&ffmpeg, path)
}

fn media_info_with(ffmpeg: &Path, path: &str) -> Result<MediaInfo, String> {
    let file = Path::new(path.trim());
    if !file.is_file() {
        return Err("That file does not exist (was it moved or deleted?)".into());
    }
    let ffprobe = library::ffprobe_for(ffmpeg).ok_or("ffprobe.exe was not found")?;
    let out = util::hidden_cmd(&ffprobe)
        .args(["-v", "error", "-show_entries", "stream=codec_type,codec_name,width,height,nb_frames,duration:format=format_name,duration", "-of", "json"])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("Could not start ffprobe: {e}"))?;
    let bad = || "That file could not be read. It may be damaged, or not a picture, GIF or video.".to_string();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|_| bad())?;
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    let video = streams.iter().find(|s| s["codec_type"] == "video").ok_or_else(bad)?;
    let num = |x: &serde_json::Value| x.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| x.as_f64()).unwrap_or(0.0);
    let (width, height) = (video["width"].as_u64().unwrap_or(0) as u32, video["height"].as_u64().unwrap_or(0) as u32);
    if width == 0 || height == 0 {
        return Err(bad());
    }
    let codec = video["codec_name"].as_str().unwrap_or("");
    let format = v["format"]["format_name"].as_str().unwrap_or("");
    let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let duration = num(&v["format"]["duration"]).max(num(&video["duration"]));
    let frames = num(&video["nb_frames"]);
    let kind = if codec == "gif" {
        if frames > 1.0 || duration > 0.05 { "gif" } else { "image" }
    } else if STILL_EXT.contains(&ext.as_str()) || format.contains("pipe") || format == "image2" || format.starts_with("webp") {
        // An animated WebP is shown as its first picture.
        "image"
    } else if duration > 0.05 {
        "video"
    } else {
        "image"
    };
    Ok(MediaInfo {
        path: file.to_string_lossy().into_owned(),
        kind: kind.into(),
        title: file.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        width,
        height,
        duration: if kind == "image" { 0.0 } else { duration },
        audio: kind == "video" && streams.iter().any(|s| s["codec_type"] == "audio"),
    })
}

/* ---------- self-test ---------- */

/// Media items and round / soft blur boxes in the export (part of `--editor-test`).
/// The test clip is 640x360, 8 s, still from 3 to 5 s.
pub(super) fn media_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, src: &ClipMeta, src_file: &Path) {
    let ex = |opts: &ExportOpts| export(s, src, opts, &mut |_| {});
    let dir = src_file.parent().map(Path::to_path_buf).unwrap_or_else(|| util::data_dir().join("editor-test"));
    let make = |name: &str, args: &[&str]| -> Option<String> {
        let p = dir.join(name);
        let ps = p.to_string_lossy().into_owned();
        let mut a: Vec<&str> = vec!["-hide_banner", "-loglevel", "error", "-y"];
        a.extend_from_slice(args);
        a.push(&ps);
        run_ffmpeg(ffmpeg, &a).ok()?;
        p.is_file().then_some(ps)
    };
    let white = make("m-white.png", &["-f", "lavfi", "-i", "color=c=white:s=64x36", "-frames:v", "1"]);
    // Left half black, right half white.
    let halves = make("m-halves.png", &["-f", "lavfi", "-i", "color=c=white:s=160x90,drawbox=x=0:y=0:w=80:h=90:color=black:t=fill", "-frames:v", "1"]);
    // A GIF: one second black, one second white (20 frames).
    let gif = make("m-anim.gif", &["-f", "lavfi", "-i", "color=c=black:s=64x36:r=10:d=1[a];color=c=white:s=64x36:r=10:d=1[b];[a][b]concat=n=2:v=1:a=0", "-loop", "0"]);
    // A clip: black for 2 s, then white for 2 s, with a tone.
    let clip = make("m-clip.mp4", &["-f", "lavfi", "-i", "color=c=black:s=160x90:r=30:d=2[a];color=c=white:s=160x90:r=30:d=2[b];[a][b]concat=n=2:v=1:a=0", "-f", "lavfi", "-i", "sine=frequency=500:duration=4", "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"]);
    r.check("media: test pictures, GIF and clip made", white.is_some() && halves.is_some() && gif.is_some() && clip.is_some(), "");
    let (Some(white), Some(halves), Some(gif), Some(clip)) = (white, halves, gif, clip) else { return };

    // media_info: kinds, sizes, lengths.
    let info = |p: &str| media_info_with(ffmpeg, p);
    let (iw, ig, ic) = (info(&white), info(&gif), info(&clip));
    let ok = matches!(&iw, Ok(i) if i.kind == "image" && (i.width, i.height) == (64, 36) && i.duration == 0.0)
        && matches!(&ig, Ok(i) if i.kind == "gif" && near(i.duration, 2.0, 0.15))
        && matches!(&ic, Ok(i) if i.kind == "video" && near(i.duration, 2.0 + 2.0, 0.2) && i.audio && (i.width, i.height) == (160, 90))
        && info(&dir.join("no-such.png").to_string_lossy()).is_err();
    r.check("media_info tells a still, a GIF and a clip apart (size, length, sound)", ok, &format!("{iw:?} / {ig:?} / {ic:?}"));

    let item = |path: &str, kind: &str, (x, y, w, h): (f64, f64, f64, f64), (from, to): (f64, f64), off: f64, extra: MediaSrc| Overlay {
        from,
        to,
        x,
        y,
        w,
        h,
        media: Some(MediaSrc { path: path.into(), kind: kind.into(), off, ..extra }),
        ..Default::default()
    };
    let plain = MediaSrc::default;
    const W: usize = 64;
    const H: usize = 36;
    // Grey 64x36 frames of the result and of the source.
    let at = |p: &Path, t: f64| grey_at(ffmpeg, p, t, W, H);
    let mean_in = |f: &Option<Vec<u8>>, b: (usize, usize, usize, usize)| f.as_ref().map(|f| box_mean(f, W, b)).unwrap_or(-1.0);

    // Old options with no media still parse; new ones do.
    let parsed: Result<ExportOpts, _> = serde_json::from_str(r#"{"overlays":[{"png":"","from":1,"to":2,"x":0.5,"y":0.5,"w":0.25,"h":0.25,"media":{"path":"a.gif","kind":"gif","off":0.5,"mlen":2,"loop":true,"crop":{"l":0.1}}}],"blurs":[{"x":0,"y":0,"w":0.5,"h":0.5,"from":0,"to":3,"strength":0.03,"round":1,"soft":0.5}]}"#);
    r.check(
        "export options with a media item and a round blur box parse",
        parsed.as_ref().is_ok_and(|o| o.overlays[0].media.as_ref().is_some_and(|m| m.looped && m.crop.l == 0.1 && m.opacity == 1.0 && m.speed == 1.0) && o.blurs[0].round == 1.0 && o.blurs[0].soft == 0.5),
        &format!("{:?}", parsed.err()),
    );

    // 1. A still in the lower right quarter from 1 to 2.5 s, its chain shown in the report.
    let still = item(&white, "image", (0.5, 0.5, 0.25, 0.25), (1.0, 2.5), 0.0, plain());
    let opts = ExportOpts { overlays: vec![still], ..Default::default() };
    if let Ok(Some(l)) = layer_plan(&opts, 8.0) {
        r.line(&format!("INFO media chain: {}", layer_chain(&l, 4, (640, 360), 8.0, &[]).join(";")));
        r.line(&format!("INFO media inputs: {}", layer_input_args(&l, &[], 30, 8.0).join(" ")));
    }
    let inside = (34, 20, 46, 25);
    match ex(&opts) {
        Ok(m) => {
            let out = Path::new(&m.path);
            let (on, src_on, late, src_late) = (at(out, 1.5), at(src_file, 1.5), at(out, 6.0), at(src_file, 6.0));
            let (wm, rest, after) = (mean_in(&on, inside), box_diff(&on, &src_on, W, (2, 2, 30, 16)), box_diff(&late, &src_late, W, inside));
            r.check("media: a still lies in its box inside its time range only", wm > 200.0 && rest < 8.0 && after < 8.0, &format!("box mean {wm:.1}, rest off by {rest:.1}, at 6 s off by {after:.1}"));
            let left = Path::new(&white).is_file();
            r.check("media: the user's picture is not removed by the export", left, "");
        }
        Err(e) => r.check("media: a still lies in its box inside its time range only", false, &e),
    }

    // 2. Crop: the black left half cut off, the box holds the white half only.
    let crop = MediaSrc { crop: MediaCrop { l: 0.5, ..Default::default() }, ..plain() };
    match ex(&ExportOpts { overlays: vec![item(&halves, "image", (0.0, 0.0, 0.25, 0.5), (0.0, 3.0), 0.0, crop)], ..Default::default() }) {
        Ok(m) => {
            let f = at(Path::new(&m.path), 1.0);
            let (l, rr) = (mean_in(&f, (1, 1, 7, 17)), mean_in(&f, (9, 1, 15, 17)));
            r.check("media: a crop cuts off its part (no black half left in the box)", l > 200.0 && rr > 200.0, &format!("left of the box {l:.1}, right {rr:.1}"));
        }
        Err(e) => r.check("media: a crop cuts off its part (no black half left in the box)", false, &e),
    }

    // 3. A looping GIF over the top-left quarter: black at 0.5 s, white at 1.5 s, black again at 2.5 s.
    let looped = MediaSrc { looped: true, mlen: 2.0, ..plain() };
    let tl = (2, 2, 30, 16);
    match ex(&ExportOpts { overlays: vec![item(&gif, "gif", (0.0, 0.0, 0.5, 0.5), (0.0, 6.0), 0.0, looped.clone())], ..Default::default() }) {
        Ok(m) => {
            let out = Path::new(&m.path);
            let (a, b, c) = (mean_in(&at(out, 0.5), tl), mean_in(&at(out, 1.5), tl), mean_in(&at(out, 2.5), tl));
            r.check("media: a GIF shows its frames at their times and loops", (0.0..40.0).contains(&a) && b > 200.0 && (0.0..40.0).contains(&c), &format!("box mean at 0.5 s {a:.1}, 1.5 s {b:.1}, 2.5 s {c:.1}"));
        }
        Err(e) => r.check("media: a GIF shows its frames at their times and loops", false, &e),
    }
    // ... and started 1.2 s into it: white at 0.3 s.
    match ex(&ExportOpts { overlays: vec![item(&gif, "gif", (0.0, 0.0, 0.5, 0.5), (0.0, 3.0), 1.2, looped)], ..Default::default() }) {
        Ok(m) => {
            let out = Path::new(&m.path);
            let (a, b) = (mean_in(&at(out, 0.3), tl), mean_in(&at(out, 1.2), tl));
            r.check("media: a GIF started part way (off) shows that part first", a > 200.0 && (0.0..40.0).contains(&b), &format!("box mean at 0.3 s {a:.1}, 1.2 s {b:.1}"));
        }
        Err(e) => r.check("media: a GIF started part way (off) shows that part first", false, &e),
    }

    // 4. A clip item from 1 to 3.5 s started 1.5 s into the clip: black at 1.2 s (clip 1.7 s), white at 1.8 s (clip 2.3 s); no sound of its own.
    match ex(&ExportOpts { overlays: vec![item(&clip, "video", (0.0, 0.0, 0.5, 0.5), (1.0, 3.5), 1.5, MediaSrc { mlen: 4.0, ..plain() })], ..Default::default() }) {
        Ok(m) => {
            let out = Path::new(&m.path);
            let (a, b) = (mean_in(&at(out, 1.2), tl), mean_in(&at(out, 1.8), tl));
            let streams = library::probe(ffmpeg, out, false).map(|p| p.audio_streams).unwrap_or(0);
            let tone = band_db_at(ffmpeg, out, 0, 500, 1.2, 3.3).unwrap_or(-99.0);
            r.check("media: a clip item shows the frame of its own time (seeked by off), without its sound", (0.0..40.0).contains(&a) && b > 200.0 && streams == 3 && tone < -45.0, &format!("box mean at 1.2 s {a:.1}, 1.8 s {b:.1}; {streams} audio streams, its tone {tone:.1} dB"));
        }
        Err(e) => r.check("media: a clip item shows the frame of its own time (seeked by off), without its sound", false, &e),
    }

    // 4b (0.1.37). A piece of a clip dragged up onto an overlay row: a clip item over the WHOLE picture.
    //     Over footage, from 1 to 3 s, 1.5 s into the clip: all black at 1.3 s (clip 1.8 s), all white at 2.0 s (clip 2.5 s), the source after it.
    let whole = |from: f64, to: f64, off: f64| item(&clip, "video", (0.0, 0.0, 1.0, 1.0), (from, to), off, MediaSrc { mlen: 4.0, ..plain() });
    let everywhere = |f: &Option<Vec<u8>>| [(2, 2, 30, 16), (34, 2, 62, 16), (2, 20, 30, 34), (34, 20, 62, 34)].map(|b| mean_in(f, b));
    match ex(&ExportOpts { overlays: vec![whole(1.0, 3.0, 1.5)], ..Default::default() }) {
        Ok(m) => {
            let out = Path::new(&m.path);
            let (a, b) = (everywhere(&at(out, 1.3)), everywhere(&at(out, 2.0)));
            let after = box_diff(&at(out, 3.5), &at(src_file, 3.5), W, (2, 2, 62, 34));
            let ok = a.iter().all(|v| (0.0..30.0).contains(v)) && b.iter().all(|v| *v > 215.0) && after < 8.0;
            r.check("media: a full-picture clip item over footage covers all of it at its own time, the footage after it", ok, &format!("quarters at 1.3 s {a:.0?}, at 2.0 s {b:.0?}; after it off the source by {after:.1}"));
        }
        Err(e) => r.check("media: a full-picture clip item over footage covers all of it at its own time, the footage after it", false, &e),
    }
    //     Over a gap of V1 (0..2 s footage, 2 s gap, 2..4 s footage): from 2 to 4 s, 2 s into the clip: all white in the gap, the footage after it.
    let seg = |a: f64, b: f64| Range { start: a, end: b, ..Default::default() };
    let gapped = ExportOpts { segments: vec![seg(0.0, 2.0), Range { gap: 2.0, ..Default::default() }, seg(2.0, 4.0)], ordered: true, overlays: vec![whole(2.0, 4.0, 2.0)], ..Default::default() };
    // (After the item the result must be the same edit without it: compared with that export.)
    match (ex(&gapped), ex(&ExportOpts { overlays: vec![], ..gapped.clone() })) {
        (Ok(m), Ok(plain_gap)) => {
            let out = Path::new(&m.path);
            let len = library::probe(ffmpeg, out, false).map(|p| p.duration).unwrap_or(0.0);
            let (a, b) = (everywhere(&at(out, 2.3)), everywhere(&at(out, 3.8)));
            let without = at(Path::new(&plain_gap.path), 2.3);
            let gap_black = everywhere(&without).iter().all(|v| (0.0..30.0).contains(v));
            let after = box_diff(&at(out, 4.5), &at(Path::new(&plain_gap.path), 4.5), W, (2, 2, 62, 34));
            let ok = near(len, 6.0, 0.25) && a.iter().all(|v| *v > 215.0) && b.iter().all(|v| *v > 215.0) && gap_black && after < 6.0;
            r.check("media: a full-picture clip item over a gap fills the gap with its own frames", ok, &format!("length {len:.2}; quarters at 2.3 s {a:.0?} (black without it: {gap_black}), at 3.8 s {b:.0?}; at 4.5 s off the edit without it by {after:.1}"));
        }
        (a, b) => r.check("media: a full-picture clip item over a gap fills the gap with its own frames", false, &format!("{:?} / {:?}", a.err(), b.err())),
    }

    // 5. A still with keyframes: at the top left at first, moved half the picture right by 2 s.
    let mv = |t: f64, dx: f64| MovePoint { t, dx, dy: 0.0, sx: 1.0, sy: 1.0, e: false };
    let moving = Overlay { track: vec![mv(0.0, 0.0), mv(2.0, 0.5)], ..item(&white, "image", (0.0, 0.0, 0.25, 0.25), (0.0, 3.0), 0.0, plain()) };
    match ex(&ExportOpts { overlays: vec![moving], ..Default::default() }) {
        Ok(m) => {
            let out = Path::new(&m.path);
            let (f0, f1) = (at(out, 0.1), at(out, 2.5));
            let (a, b, c) = (mean_in(&f0, (2, 2, 14, 7)), mean_in(&f1, (34, 2, 46, 7)), box_diff(&f1, &at(src_file, 2.5), W, (2, 2, 14, 7)));
            r.check("media: a still with keyframes moves along its track", a > 200.0 && b > 200.0 && c < 10.0, &format!("start box {a:.1}, end box {b:.1}, old place off by {c:.1}"));
        }
        Err(e) => r.check("media: a still with keyframes moves along its track", false, &e),
    }

    // 6. After the end of the video (A's `tail`): a still on the black picture there.
    let has_tail = serde_json::to_value(ExportOpts::default()).ok().is_some_and(|v| v.get("tail").is_some());
    if has_tail {
        let opts = ExportOpts { overlays: vec![item(&white, "image", (0.5, 0.5, 0.25, 0.25), (8.5, 9.5), 0.0, plain())], ..Default::default() };
        let tailed = serde_json::to_value(&opts).ok().and_then(|mut v| {
            v["tail"] = serde_json::json!(2.0);
            serde_json::from_value::<ExportOpts>(v).ok()
        });
        match tailed.map(|o| ex(&o)) {
            Some(Ok(m)) => {
                let f = at(Path::new(&m.path), 9.0);
                let (wm, black) = (mean_in(&f, inside), mean_in(&f, (2, 2, 30, 16)));
                r.check("media: an item after the end of the video is on the black picture there", wm > 200.0 && (0.0..30.0).contains(&black), &format!("box {wm:.1}, elsewhere {black:.1}"));
            }
            Some(Err(e)) => r.check("media: an item after the end of the video is on the black picture there", false, &e),
            None => r.check("media: an item after the end of the video is on the black picture there", false, "options with a tail did not parse"),
        }
    } else {
        r.line("NOTE media after the end of the video: skipped, ExportOpts has no `tail` yet");
    }

    // 7. A missing file is refused before ffmpeg runs, by name.
    let gone = ex(&ExportOpts { overlays: vec![item(&dir.join("gone.png").to_string_lossy(), "image", (0.0, 0.0, 0.5, 0.5), (0.0, 2.0), 0.0, plain())], ..Default::default() });
    r.check("media: a missing picture is refused cleanly", gone.as_ref().is_err_and(|e| e.contains("gone.png")), &format!("{:?}", gone.err()));

    // 8. mask_alpha: the corner of a round box is out, its middle in; a soft edge rises from 0.
    let (corner, middle, edge_soft, mid_soft) = (mask_alpha(2.0, 2.0, 100.0, 60.0, 1.0, 0.0), mask_alpha(50.0, 30.0, 100.0, 60.0, 1.0, 0.0), mask_alpha(1.0, 30.0, 100.0, 60.0, 0.0, 1.0), mask_alpha(50.0, 30.0, 100.0, 60.0, 0.0, 1.0));
    let plain_box = mask_alpha(0.5, 0.5, 100.0, 60.0, 0.0, 0.0);
    r.check("blur mask: round corner out, middle in, soft edge fades, plain box full", corner == 0.0 && middle == 1.0 && edge_soft < 0.01 && mid_soft > 0.99 && plain_box == 1.0, &format!("{corner} {middle} {edge_soft:.3} {mid_soft:.3} {plain_box}"));

    // 9. A round blur box (an ellipse) over the top-left quarter, standing still and with a track:
    //    its corner keeps the source's detail, its middle is blurred. 10. A soft one: the edge hardly blurred.
    const BW: usize = 320;
    let ratio_at = |out: &Path, t: f64, b: (usize, usize, usize, usize)| {
        let (e, s0) = (grey_at(ffmpeg, out, t, BW, 180), grey_at(ffmpeg, src_file, t, BW, 180));
        let (d, d0) = (box_detail(&e, BW, b), box_detail(&s0, BW, b));
        if d0 > 0.3 { d / d0 } else { 9.0 }
    };
    // (The test picture's detail at 1.5 s lies in its top rows and left columns: both boxes have some.)
    let (corner_px, middle_px) = ((1, 1, 18, 11), (42, 14, 62, 30));
    let blur = |round: f64, soft: f64, track: Vec<MovePoint>| BlurBox { x: 0.0, y: 0.0, w: 0.5, h: 0.5, from: 0.0, to: 2.5, strength: 0.03, round, soft, track };
    let still_track = vec![mv(0.0, 0.0), mv(2.0, 0.0)];
    for (name, b) in [("round blur box: the corner is not blurred, the middle is", blur(1.0, 0.0, vec![])), ("round blur box with keyframes: the corner is not blurred, the middle is", blur(1.0, 0.0, still_track))] {
        let opts = ExportOpts { blurs: vec![b], ..Default::default() };
        if name.starts_with("round blur box:") {
            if let Ok(Some(l)) = layer_plan(&opts, 8.0) {
                let masks = vec![Some(PathBuf::from("mask.pgm"))];
                r.line(&format!("INFO round blur chain: {}", layer_chain(&l, 4, (640, 360), 8.0, &masks).join(";")));
            }
        }
        match ex(&opts) {
            Ok(m) => {
                let out = Path::new(&m.path);
                let (c, mid) = (ratio_at(out, 1.5, corner_px), ratio_at(out, 1.5, middle_px));
                r.check(name, (0.7..1.4).contains(&c) && mid < 0.4, &format!("detail ratio in the corner {c:.2}, in the middle {mid:.2}"));
            }
            Err(e) => r.check(name, false, &e),
        }
    }
    // Soft 50 %: the blur rises over the outer 45 px (of 640) of the box: the outer 8 px hardly blurred, 52 px in fully.
    match ex(&ExportOpts { blurs: vec![blur(0.0, 0.5, vec![])], ..Default::default() }) {
        Ok(m) => {
            let out = Path::new(&m.path);
            let (edge, mid) = (ratio_at(out, 1.5, (1, 1, 4, 89)), ratio_at(out, 1.5, (26, 26, 60, 64)));
            r.check("soft blur box: the blur fades out toward the edge", edge > 0.7 && mid < 0.5, &format!("detail ratio at the edge {edge:.2}, in the middle {mid:.2}"));
        }
        Err(e) => r.check("soft blur box: the blur fades out toward the edge", false, &e),
    }
    let left: Vec<_> = fs::read_dir(overlay_dir()).map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "pgm")).collect()).unwrap_or_default();
    r.check("blur masks removed after the exports", left.is_empty(), &format!("{left:?}"));
}
