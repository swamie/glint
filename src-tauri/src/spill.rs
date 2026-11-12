//! Files for the part of the replay buffer that is kept on disk.
//!
//! The buffer can live in RAM, on disk, or both: the newest seconds in RAM and
//! everything older in files (`Settings::buffer`). A piece of buffer that has
//! been written out holds an `Arc<Seg>`; the file is deleted when the last
//! piece in it is dropped, so a clip that is being saved keeps its data even
//! if the buffer moves past it meanwhile.
use crate::util;
use std::fs::{self, File};
use std::io;
use std::os::windows::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// One buffer file. Reads and writes carry their own position, so it can be
/// shared between threads without a lock.
pub struct Seg {
    path: PathBuf,
    file: Option<File>,
}

impl Seg {
    /// A new, empty file in `dir`. `tag` tells video ("v") from the audio sources.
    pub fn create(dir: &Path, tag: &str) -> io::Result<Seg> {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        fs::create_dir_all(dir)?;
        let path = dir.join(format!("glint-{}-{}-{}.buf", std::process::id(), tag, SEQ.fetch_add(1, Ordering::Relaxed)));
        let file = fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path)?;
        Ok(Seg { path, file: Some(file) })
    }

    pub fn write_at(&self, mut data: &[u8], mut off: u64) -> io::Result<()> {
        let file = self.file.as_ref().ok_or(io::ErrorKind::NotFound)?;
        while !data.is_empty() {
            let n = file.seek_write(data, off)?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            data = &data[n..];
            off += n as u64;
        }
        Ok(())
    }

    pub fn read_at(&self, mut buf: &mut [u8], mut off: u64) -> io::Result<()> {
        let file = self.file.as_ref().ok_or(io::ErrorKind::NotFound)?;
        while !buf.is_empty() {
            let n = file.seek_read(buf, off)?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            buf = &mut buf[n..];
            off += n as u64;
        }
        Ok(())
    }
}

impl Drop for Seg {
    fn drop(&mut self) {
        self.file.take(); // close first
        let _ = fs::remove_file(&self.path);
    }
}

/// Folder for the buffer files: the user's choice, else `%LOCALAPPDATA%\Glint\buffer`
/// (large temporary files do not belong in the roaming profile). With
/// `GLINT_DATA_DIR` set (tests, benchmark) it is `<data dir>\buffer`.
pub fn dir(custom: &str) -> PathBuf {
    if !custom.trim().is_empty() {
        return PathBuf::from(custom.trim());
    }
    if std::env::var_os("GLINT_DATA_DIR").is_some() {
        return util::data_dir().join("buffer");
    }
    match std::env::var_os("LOCALAPPDATA") {
        Some(d) => PathBuf::from(d).join("Glint").join("buffer"),
        None => util::data_dir().join("buffer"),
    }
}

/// Seconds of buffer that stay in RAM before going to disk.
/// `None` = everything stays in RAM; `Some(0.0)` = everything goes to disk.
///
/// "mixed" on a buffer barely longer than its RAM part would write to disk for
/// nothing, so it is treated as RAM only.
pub fn window(mode: &str, ram_secs: f64, max_secs: f64) -> Option<f64> {
    match mode {
        "ram" => None,
        "disk" => Some(0.0),
        _ if max_secs <= ram_secs + 15.0 => None,
        _ => Some(ram_secs.max(5.0)),
    }
}

/// Delete buffer files a Glint that is no longer running left in `dir` (a
/// crash, or the PC losing power). Files of running copies are left alone.
pub fn clean(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("glint-").filter(|_| name.ends_with(".buf")) else { continue };
        let pid: u32 = rest.split('-').next().and_then(|p| p.parse().ok()).unwrap_or(0);
        if pid != std::process::id() && !crate::win::process_alive(pid) {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// Free space on the drive that holds `dir`, in bytes.
pub fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    // The folder may not exist yet: ask about its nearest existing parent.
    let mut p = dir.to_path_buf();
    while !p.exists() {
        if !p.pop() {
            return None;
        }
    }
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None).ok()? };
    Some(free)
}
