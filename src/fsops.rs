//! File operations that can't lose anything: new names by hardlink or an
//! instant clone (the original stays put), moves only within one volume,
//! and never over an existing file.

#[cfg(unix)]
use std::fs::{File, OpenOptions};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
use std::path::Path;

/// Identity of a file on disk (survives renames; shared by hardlinks).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default)]
pub struct Ident {
    pub dev: u64,
    pub ino: u64,
}

#[cfg(unix)]
pub fn ident(path: &Path) -> Option<Ident> {
    std::fs::metadata(path).ok().map(|m| Ident { dev: m.dev(), ino: m.ino() })
}

/// Volume serial number + file index (NTFS, ReFS and most SMB servers).
#[cfg(windows)]
pub fn ident(path: &Path) -> Option<Ident> {
    win::file_info(path).map(|(dev, ino, _)| Ident { dev, ino })
}

pub fn same_volume(a: &Path, b_dir: &Path) -> bool {
    match (ident(a), ident(b_dir)) {
        (Some(x), Some(y)) => x.dev == y.dev,
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
#[cfg(unix)]
const FICLONE: libc::c_ulong = 0x4004_9409;

#[cfg(windows)]
fn clone_file(_src: &Path, _dst: &Path) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

#[cfg(unix)]
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

/// Move within one volume without ever replacing a file. Windows' own move
/// does exactly that when it's told not to replace and not to copy, and it also
/// handles a change of letter case only.
#[cfg(windows)]
pub fn move_file(src: &Path, dst: &Path) -> Result<(), String> {
    win::move_no_replace(src, dst).map_err(|e| match e.raw_os_error() {
        Some(80) | Some(183) => format!("{} already exists", show(dst)),
        Some(17) => "It would have to move to another drive, which Elixir doesn't do".into(),
        _ => format!("Couldn't rename: {}", plain(&e)),
    })
}

/// Move within one volume without ever replacing a file: link, then drop the old name.
#[cfg(unix)]
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
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o775));
                }
                made.push(d);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("Couldn't create {}: {}", show(&d), plain(&e))),
        }
    }
    Ok(made)
}

/// Longest name a folder allows. Windows allows 255 characters (counted here as bytes, so stricter).
#[cfg(windows)]
pub fn name_limit(_dir: &Path) -> usize {
    255
}

/// Longest name (in bytes) a folder allows. Encrypted Synology shares allow 143.
#[cfg(unix)]
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

/// Whether a program has the file open for writing: opening it while refusing to
/// share write access fails with a sharing violation if one does.
#[cfg(windows)]
pub fn open_for_writing(path: &Path) -> bool {
    win::open_for_writing(path)
}

/// Whether a process has the file open for writing (Linux /proc scan; best effort).
#[cfg(unix)]
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

/// Same path? Windows ignores letter case and accepts / and \\ alike.
pub fn same_path(a: &Path, b: &Path) -> bool {
    norm(a) == norm(b)
}

/// `p` is `root` or inside it.
pub fn within(p: &Path, root: &Path) -> bool {
    let (p, r) = (norm(p), norm(root));
    if r.is_empty() {
        return false;
    }
    p == r || p.starts_with(&format!("{}{}", r.trim_end_matches(SEP), SEP)) || (r == "/" && p.starts_with('/'))
}

#[cfg(windows)]
const SEP: char = '\\';
#[cfg(unix)]
const SEP: char = '/';

/// A comparable form of a path.
pub fn norm(p: &Path) -> String {
    let s = p.to_string_lossy();
    #[cfg(windows)]
    {
        let s = s.replace('/', "\\");
        let s = s.strip_prefix(r"\\?\UNC\").map(|r| format!(r"\\{r}")).unwrap_or_else(|| s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s.clone()));
        let t = s.trim_end_matches('\\');
        // Keep "C:\" whole.
        let t = if t.len() == 2 && t.ends_with(':') { format!("{t}\\") } else { t.to_string() };
        t.to_lowercase()
    }
    #[cfg(unix)]
    {
        let t = s.trim_end_matches('/');
        if t.is_empty() { "/".to_string() } else { t.to_string() }
    }
}

/// Hide a file Elixir keeps in a watched folder (Windows; dot files are hidden elsewhere).
pub fn hide(_p: &Path) {
    #[cfg(windows)]
    win::hide(_p);
}

pub fn show(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

pub fn plain(e: &io::Error) -> String {
    match e.kind() {
        #[cfg(unix)]
        io::ErrorKind::PermissionDenied => "Elixir isn't allowed to change this folder (give the Elixir user Read/Write on the shared folder)".into(),
        #[cfg(windows)]
        io::ErrorKind::PermissionDenied => "Windows says no: the file is in use, or Elixir isn't allowed to change this folder".into(),
        io::ErrorKind::NotFound => "the file is gone".into(),
        _ => e.to_string(),
    }
}

#[cfg(windows)]
mod win {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetFileAttributesW, GetFileInformationByHandle, MoveFileExW, SetFileAttributesW, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_HIDDEN, FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_READ,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, INVALID_FILE_ATTRIBUTES, OPEN_EXISTING,
    };

    fn wide(p: &Path) -> Vec<u16> {
        // std adds the \\?\ prefix for long paths when it opens files; do the same here.
        let s = p.as_os_str();
        let mut v: Vec<u16> = if s.len() >= 240 && !s.to_string_lossy().starts_with(r"\\?\") {
            let text = s.to_string_lossy().replace('/', "\\");
            let long = if let Some(unc) = text.strip_prefix(r"\\") { format!(r"\\?\UNC\{unc}") } else { format!(r"\\?\{text}") };
            std::ffi::OsStr::new(&long).encode_wide().collect()
        } else {
            s.encode_wide().collect()
        };
        v.push(0);
        v
    }

    /// (volume serial, file index, hardlink count)
    pub fn file_info(p: &Path) -> Option<(u64, u64, u32)> {
        let w = wide(p);
        // SAFETY: valid NUL-terminated path; the handle is closed below.
        let h = unsafe { CreateFileW(w.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, std::ptr::null_mut()) };
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        // SAFETY: zeroed plain-data struct, filled by the call.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe { GetFileInformationByHandle(h, &mut info) } != 0;
        unsafe { CloseHandle(h) };
        ok.then(|| (info.dwVolumeSerialNumber as u64, ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64, info.nNumberOfLinks))
    }

    pub fn move_no_replace(src: &Path, dst: &Path) -> io::Result<()> {
        let (a, b) = (wide(src), wide(dst));
        // Flags 0: never replace an existing file, never copy to another drive.
        // SAFETY: both are valid NUL-terminated paths.
        if unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), 0) } != 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(unsafe { GetLastError() } as i32))
        }
    }

    pub fn open_for_writing(p: &Path) -> bool {
        let w = wide(p);
        // Read access, sharing read and delete but not write.
        // SAFETY: valid NUL-terminated path; the handle is closed below.
        let h = unsafe { CreateFileW(w.as_ptr(), FILE_GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_DELETE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
        if h == INVALID_HANDLE_VALUE {
            // 32 = another program has it open in a way that clashes (writing).
            return unsafe { GetLastError() } == 32;
        }
        unsafe { CloseHandle(h) };
        false
    }

    pub fn hide(p: &Path) {
        let w = wide(p);
        // SAFETY: valid NUL-terminated path.
        unsafe {
            let a = GetFileAttributesW(w.as_ptr());
            if a != INVALID_FILE_ATTRIBUTES && a & FILE_ATTRIBUTE_HIDDEN == 0 {
                SetFileAttributesW(w.as_ptr(), a | FILE_ATTRIBUTE_HIDDEN);
            }
        }
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
    fn paths_compare_like_the_system() {
        if cfg!(windows) {
            assert!(same_path(Path::new(r"D:\Downloads\Show.mkv"), Path::new("d:/downloads/show.MKV")));
            assert!(within(Path::new(r"D:\Downloads\a\b.mkv"), Path::new(r"d:\downloads\")));
            assert!(within(Path::new(r"\\NAS\data\torrents\x.mkv"), Path::new(r"\\nas\Data")));
            assert!(within(Path::new(r"C:\x"), Path::new(r"C:\")));
            assert!(!within(Path::new(r"D:\Downloads2\a.mkv"), Path::new(r"D:\Downloads")));
        } else {
            assert!(within(Path::new("/volume1/data/torrents/a.mkv"), Path::new("/volume1/data/")));
            assert!(!within(Path::new("/volume1/data2/a.mkv"), Path::new("/volume1/data")));
            assert!(!same_path(Path::new("/volume1/TV"), Path::new("/volume1/tv")));
            assert!(within(Path::new("/x"), Path::new("/")));
        }
    }

    // Windows: a change of letter case only is one move of the same file.
    #[cfg(windows)]
    #[test]
    fn case_only_rename() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("show.mkv");
        std::fs::write(&a, b"video").unwrap();
        let b = d.path().join("Show.mkv");
        move_file(&a, &b).unwrap();
        let names: Vec<String> = std::fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["Show.mkv"]);
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
