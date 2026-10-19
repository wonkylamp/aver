//! Host files replaced whole or not at all: a temporary file beside the
//! target (unique name, `O_EXCL`), fsynced, renamed over it, and the
//! directory fsynced. A kill or a full disk at any moment leaves the old
//! file or the new one, never a truncated mix; at worst a temporary file
//! of a dead process stays, which [`sweep`] removes.

use std::ffi::CString;
use std::fs;
use std::io::{self, Write as _};
use std::os::fd::{AsRawFd as _, BorrowedFd, FromRawFd as _, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

/// The mode a new file gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The replaced file's mode, or `0o666` less the umask for a new file
    /// (as `fs::write`).
    Keep,
    /// Exactly this mode.
    Exact(u32),
}

const TMP_MARK: &str = ".aver-tmp-";

fn tmp_name(name: &str) -> String {
    format!(
        ".{name}{TMP_MARK}{}-{}",
        std::process::id(),
        ulid::Ulid::generate()
    )
}

fn split(path: &Path) -> io::Result<(&Path, &str)> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    Ok((dir, name))
}

/// Replaces `path` with `bytes` atomically and durably.
pub fn write(path: &Path, bytes: &[u8], mode: Mode) -> io::Result<()> {
    write_at(None, path, bytes, mode)
}

/// [`write`], passing crash point `point` once the temporary file is
/// durable and before it replaces `path`.
pub fn write_at(point: Option<&str>, path: &Path, bytes: &[u8], mode: Mode) -> io::Result<()> {
    let (dir, name) = split(path)?;
    let tmp = dir.join(tmp_name(name));
    let result = (|| {
        let keep = match mode {
            Mode::Exact(m) => Some(m),
            Mode::Keep => fs::metadata(path)
                .ok()
                .map(|m| m.permissions().mode() & 0o7777),
        };
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(keep.unwrap_or(0o666))
            .open(&tmp)?;
        if let Some(m) = keep {
            // The umask applied at creation; the replaced file's mode wins.
            file.set_permissions(fs::Permissions::from_mode(m))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if let Some(p) = point {
            crate::fault::point(p)?;
        }
        fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    sync_dir(dir)
}

/// Fsyncs directory `dir`, so a rename in it survives a power loss.
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

/// Opens directory `path` for [`write_in`], refusing a symbolic link in
/// its last component.
pub fn open_dir(path: &Path) -> io::Result<OwnedFd> {
    let c = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a NUL in the path"))?;
    // SAFETY: `c` is a valid C string; the result is checked.
    let fd = unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just opened and is owned by nothing else.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Replaces file `name` in the directory `dir` (from [`open_dir`]) with
/// `bytes`, atomically, working only through the descriptor: the directory
/// cannot be swapped for a link between the check and the write.
pub fn write_in(dir: BorrowedFd<'_>, name: &str, bytes: &[u8]) -> io::Result<()> {
    write_in_at(None, dir, name, bytes)
}

/// [`write_in`], passing crash point `point` once the temporary file is
/// durable and before it replaces `name`.
pub fn write_in_at(
    point: Option<&str>,
    dir: BorrowedFd<'_>,
    name: &str,
    bytes: &[u8],
) -> io::Result<()> {
    write_all_in_at(point, dir, &[(name, bytes)])
}

/// [`write_in_at`] for several files of one directory at the cost of one
/// drive flush: every temporary file is written and fsynced, the last with
/// `sync_all` (on macOS `F_FULLFSYNC`, which flushes the whole drive cache
/// and so the earlier ones with it), then `point` passes and each replaces
/// its file in order. A cut leaves each file old or new, never torn.
pub fn write_all_in_at(
    point: Option<&str>,
    dir: BorrowedFd<'_>,
    files: &[(&str, &[u8])],
) -> io::Result<()> {
    let cstr =
        |s: &str| CString::new(s).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a NUL"));
    let dfd = dir.as_raw_fd();
    let mut written: Vec<(CString, CString)> = Vec::with_capacity(files.len());
    let unlink = |written: &[(CString, CString)]| {
        for (tmp, _) in written {
            // SAFETY: valid descriptor and C string.
            unsafe { libc::unlinkat(dfd, tmp.as_ptr(), 0) };
        }
    };
    for (i, (name, bytes)) in files.iter().enumerate() {
        let staged = (|| {
            if name.contains('/') || *name == "." || *name == ".." || name.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{name:?} is not a file name"),
                ));
            }
            let (c_tmp, c_name) = (cstr(&tmp_name(name))?, cstr(name)?);
            // SAFETY: valid descriptor and C strings; the result is checked.
            let fd = unsafe {
                libc::openat(
                    dfd,
                    c_tmp.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o644 as libc::c_uint,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `fd` was just opened and is owned by nothing else.
            let mut file = fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
            written.push((c_tmp, c_name));
            file.write_all(bytes)?;
            if i + 1 == files.len() {
                file.sync_all()
            } else {
                // SAFETY: the descriptor is open.
                match unsafe { libc::fsync(file.as_raw_fd()) } {
                    0 => Ok(()),
                    _ => Err(io::Error::last_os_error()),
                }
            }
        })();
        if let Err(e) = staged {
            unlink(&written);
            return Err(e);
        }
    }
    if let Some(p) = point {
        if let Err(e) = crate::fault::point(p) {
            unlink(&written);
            return Err(e);
        }
    }
    for (i, (c_tmp, c_name)) in written.iter().enumerate() {
        // SAFETY: valid descriptors and C strings; the result is checked.
        if unsafe { libc::renameat(dfd, c_tmp.as_ptr(), dfd, c_name.as_ptr()) } != 0 {
            let e = io::Error::last_os_error();
            unlink(&written[i..]);
            return Err(e);
        }
    }
    // SAFETY: the descriptor is open; fsync makes the renames durable.
    if unsafe { libc::fsync(dfd) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Removes the temporary files a dead process left beside `path`.
pub fn sweep(path: &Path) {
    let Ok((dir, name)) = split(path) else {
        return;
    };
    let prefix = format!(".{name}{TMP_MARK}");
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name();
        let Some(rest) = file.to_str().and_then(|f| f.strip_prefix(&prefix)) else {
            continue;
        };
        let pid = rest.split('-').next().and_then(|p| p.parse::<u32>().ok());
        if pid.is_some_and(|p| !crate::run::state::process_alive(p)) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Whether `name` is a temporary file of [`write`].
pub fn is_tmp(name: &str) -> bool {
    name.starts_with('.') && name.contains(TMP_MARK)
}

/// The temporary files of [`write`] under `dir`, at any depth.
pub fn leftovers(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => out.extend(leftovers(&path)),
            Ok(_) if is_tmp(&entry.file_name().to_string_lossy()) => out.push(path),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_write_replaces_the_file_and_keeps_its_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hook.sh");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o751)).unwrap();
        write(&path, b"new", Mode::Keep).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o751);
        write(&path, b"newer", Mode::Exact(0o600)).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn write_in_refuses_a_linked_directory_and_names_with_slashes() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("link")).unwrap();
        assert!(open_dir(&dir.path().join("link")).is_err());
        let fd = open_dir(&real).unwrap();
        use std::os::fd::AsFd as _;
        write_in(fd.as_fd(), "meta.toml", b"a = 1\n").unwrap();
        write_in(fd.as_fd(), "meta.toml", b"a = 2\n").unwrap();
        assert_eq!(
            fs::read_to_string(real.join("meta.toml")).unwrap(),
            "a = 2\n"
        );
        assert!(write_in(fd.as_fd(), "../escape", b"x").is_err());
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn a_batch_replaces_every_file_or_none_of_them() {
        let dir = tempfile::tempdir().unwrap();
        let fd = open_dir(dir.path()).unwrap();
        use std::os::fd::AsFd as _;
        fs::write(dir.path().join("a"), "old a").unwrap();
        let err = write_all_in_at(None, fd.as_fd(), &[("a", b"new a"), ("b/c", b"x")]);
        assert!(err.is_err());
        assert_eq!(fs::read_to_string(dir.path().join("a")).unwrap(), "old a");
        assert!(leftovers(dir.path()).is_empty());
        write_all_in_at(None, fd.as_fd(), &[("a", b"new a"), ("b", b"new b")]).unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("a")).unwrap(), "new a");
        assert_eq!(fs::read_to_string(dir.path().join("b")).unwrap(), "new b");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn a_dead_writers_temporary_file_is_swept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.json");
        let dead = dir
            .path()
            .join(format!(".run.json{TMP_MARK}{}-X", u32::MAX / 2));
        let live = dir.path().join(tmp_name("run.json"));
        fs::write(&dead, "").unwrap();
        fs::write(&live, "").unwrap();
        sweep(&path);
        assert!(!dead.exists());
        assert!(live.exists(), "this process is alive");
    }
}
