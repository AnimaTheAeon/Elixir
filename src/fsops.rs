//! File operations that can't lose anything: new names by hardlink or an
//! instant clone (the original stays put), moves only within one volume,
//! and never over an existing file.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::Path;

/// Identity of a file on disk (survives renames; shared by hardlinks).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default)]
pub struct Ident {
    pub dev: u64,
    pub ino: u64,
}

pub fn ident(path: &Path) -> Option<Ident> {
    std::fs::metadata(path).ok().map(|m| Ident { dev: m.dev(), ino: m.ino() })
}

pub fn same_volume(a: &Path, b_dir: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b_dir)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev(),
        _ => false,
    }
}

/// How a new name was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Made {
    Hardlink,
    Clone,
}

/// FICLONE: share the data blocks of another file (Btrfs/XFS). Instant, no extra space.
const FICLONE: libc::c_ulong = 0x4004_9409;

fn clone_file(src: &Path, dst: &Path) -> io::Result<()> {
    let from = File::open(src)?;
    let to = OpenOptions::new().write(true).create_new(true).mode(0o664).open(dst)?;
    // SAFETY: both descriptors are open for the duration of the call.
    let rc = unsafe { libc::ioctl(to.as_raw_fd(), FICLONE as _, from.as_raw_fd()) };
    if rc == 0 {
        Ok(())
    } else {
        let err = io::Error::last_os_error();
        drop(to);
        let _ = std::fs::remove_file(dst);
        Err(err)
    }
}

/// Give `src` a second name `dst`. Fails if `dst` exists. Never copies.
pub fn link_or_clone(src: &Path, dst: &Path) -> Result<Made, String> {
    match std::fs::hard_link(src, dst) {
        Ok(()) => Ok(Made::Hardlink),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(format!("{} already exists", show(dst))),
        Err(_) => match clone_file(src, dst) {
            Ok(()) => Ok(Made::Clone),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(format!("{} already exists", show(dst))),
            Err(_) => Err("This folder pair can't share files (no hardlink or instant clone), and Elixir never makes full copies of files that are seeding".into()),
        },
    }
}

/// Move within one volume without ever replacing a file: link, then drop the old name.
pub fn move_file(src: &Path, dst: &Path) -> Result<(), String> {
    match std::fs::hard_link(src, dst) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(format!("{} already exists", show(dst))),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => return Err("It would have to move to another volume, which Elixir doesn't do".into()),
        Err(e) => return Err(format!("Couldn't rename: {}", plain(&e))),
    }
    std::fs::remove_file(src).map_err(|e| {
        // Leave things as they were: the new name goes again.
        let _ = std::fs::remove_file(dst);
        format!("Couldn't rename: {}", plain(&e))
    })
}

/// Create a folder (and parents), group-writable so Midgar and Turks can manage it.
#[cfg(test)]
pub fn make_dirs(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut made = Vec::new();
    let mut missing = Vec::new();
    let mut cur = Some(dir);
    while let Some(d) = cur {
        if d.exists() {
            break;
        }
        missing.push(d.to_path_buf());
        cur = d.parent();
    }
    for d in missing.into_iter().rev() {
        match std::fs::create_dir(&d) {
            Ok(()) => {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o775));
                made.push(d);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("Couldn't create {}: {}", show(&d), plain(&e))),
        }
    }
    Ok(made)
}

/// Longest name (in bytes) a folder allows. Encrypted Synology shares allow 143.
pub fn name_limit(dir: &Path) -> usize {
    const ECRYPTFS_MAGIC: i64 = 0xf15f;
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()) else {
        return 255;
    };
    // SAFETY: statfs writes into a zeroed struct we own.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statfs(c.as_ptr(), &mut st) };
    if rc == 0 && st.f_type as i64 == ECRYPTFS_MAGIC {
        143
    } else {
        255
    }
}

/// Whether a process has the file open for writing (Linux /proc scan; best effort).
pub fn open_for_writing(path: &Path) -> bool {
    let Some(target) = ident(path) else { return false };
    let Ok(procs) = std::fs::read_dir("/proc") else { return false };
    for p in procs.flatten() {
        let Ok(fds) = std::fs::read_dir(p.path().join("fd")) else { continue };
        for fd in fds.flatten() {
            if ident(&fd.path()) != Some(target) {
                continue;
            }
            let info = std::fs::read_to_string(p.path().join("fdinfo").join(fd.file_name())).unwrap_or_default();
            let flags = info.lines().find_map(|l| l.strip_prefix("flags:")).map(|f| f.trim()).and_then(|f| u32::from_str_radix(f, 8).ok()).unwrap_or(0);
            if flags & 0o3 != 0 {
                return true;
            }
        }
    }
    false
}

pub fn show(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

pub fn plain(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::PermissionDenied => "Elixir isn't allowed to change this folder (give the Elixir user Read/Write on the shared folder)".into(),
        io::ErrorKind::NotFound => "the file is gone".into(),
        _ => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_never_overwrites() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.mkv");
        let b = d.path().join("b.mkv");
        std::fs::write(&a, b"video").unwrap();
        assert_eq!(link_or_clone(&a, &b).unwrap(), Made::Hardlink);
        assert_eq!(ident(&a), ident(&b));
        assert!(link_or_clone(&a, &b).is_err());
        std::fs::write(d.path().join("c.mkv"), b"other").unwrap();
        assert!(move_file(&a, &d.path().join("c.mkv")).is_err());
        assert_eq!(std::fs::read(d.path().join("c.mkv")).unwrap(), b"other");
        assert!(a.exists());
    }

    #[test]
    fn move_keeps_data() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.mkv");
        std::fs::write(&a, b"video").unwrap();
        let sub = d.path().join("x/y");
        let made = make_dirs(&sub).unwrap();
        assert_eq!(made.len(), 2);
        let b = sub.join("b.mkv");
        move_file(&a, &b).unwrap();
        assert!(!a.exists());
        assert_eq!(std::fs::read(&b).unwrap(), b"video");
    }
}
