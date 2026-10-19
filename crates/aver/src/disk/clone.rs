//! Copy-on-write tree clones (§2.3.1): one `clonefile(2)` of a directory on
//! APFS, a per-file `FICLONE` (or per-file `clonefile`) walk where the
//! filesystem shares extents, and a plain copy otherwise. Every strategy
//! keeps file modes, symlink targets and modification times, because the
//! checkout manifest seeds a workspace's stat cache from size and mtime.

use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloneStrategy {
    /// One `clonefile(2)` call for the whole directory (macOS, APFS).
    Clonefile,
    /// A walk that clones each file's extents: `ioctl(FICLONE)` on Linux
    /// (btrfs, XFS), per-file `clonefile(2)` on macOS. Files that cannot be
    /// cloned are copied.
    Reflink,
    /// A walk that copies every byte.
    Copy,
}

impl CloneStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clonefile => "clonefile",
            Self::Reflink => "reflink",
            Self::Copy => "copy",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [Self::Clonefile, Self::Reflink, Self::Copy]
            .into_iter()
            .find(|c| c.as_str() == s)
    }

    /// Whether this host can use the strategy at all.
    pub fn supported(self) -> bool {
        match self {
            Self::Clonefile => cfg!(target_os = "macos"),
            Self::Reflink => cfg!(any(target_os = "macos", target_os = "linux")),
            Self::Copy => true,
        }
    }
}

/// Picks the cheapest strategy that works between two directories on the
/// same volume as `scratch`: clones a probe file and a probe directory and
/// falls back as each fails. Different devices force `copy`.
pub fn probe(scratch: &Path, other: &Path) -> io::Result<CloneStrategy> {
    use std::os::unix::fs::MetadataExt;
    fs::create_dir_all(scratch)?;
    fs::create_dir_all(other)?;
    if fs::metadata(scratch)?.dev() != fs::metadata(other)?.dev() {
        return Ok(CloneStrategy::Copy);
    }
    let dir = scratch.join(format!("probe-{}", ulid::Ulid::generate()));
    fs::create_dir_all(dir.join("src"))?;
    fs::write(dir.join("src/f"), b"probe")?;
    let picked =
        if cfg!(target_os = "macos") && clonefile(&dir.join("src"), &dir.join("dst")).is_ok() {
            CloneStrategy::Clonefile
        } else if reflink_file(&dir.join("src/f"), &dir.join("f2")).is_ok() {
            CloneStrategy::Reflink
        } else {
            CloneStrategy::Copy
        };
    let _ = fs::remove_dir_all(&dir);
    Ok(picked)
}

/// Clones the directory `src` to `dst`, which must not exist.
pub fn clone_tree(strategy: CloneStrategy, src: &Path, dst: &Path) -> io::Result<()> {
    match strategy {
        CloneStrategy::Clonefile => clonefile(src, dst),
        CloneStrategy::Reflink | CloneStrategy::Copy => {
            fs::create_dir(dst)?;
            copy_dir(strategy, src, dst)
        }
    }
}

/// Clones one file or symlink (not a directory) from `src` to `dst`.
pub fn clone_entry(strategy: CloneStrategy, src: &Path, dst: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        return symlink(fs::read_link(src)?, dst);
    }
    if meta.is_dir() {
        return clone_tree(strategy, src, dst);
    }
    copy_file(strategy, src, dst, &meta)
}

fn copy_dir(strategy: CloneStrategy, src: &Path, dst: &Path) -> io::Result<()> {
    let entries: Vec<fs::DirEntry> = fs::read_dir(src)?.collect::<io::Result<_>>()?;
    entries.par_iter().try_for_each(|entry| {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            fs::create_dir(&to)?;
            let mode = fs::symlink_metadata(&from)?.permissions().mode();
            copy_dir(strategy, &from, &to)?;
            fs::set_permissions(&to, fs::Permissions::from_mode(mode))
        } else if kind.is_symlink() {
            symlink(fs::read_link(&from)?, &to)
        } else if kind.is_file() {
            copy_file(strategy, &from, &to, &fs::symlink_metadata(&from)?)
        } else {
            Ok(())
        }
    })
}

fn copy_file(
    strategy: CloneStrategy,
    src: &Path,
    dst: &Path,
    meta: &fs::Metadata,
) -> io::Result<()> {
    if strategy == CloneStrategy::Reflink && reflink_file(src, dst).is_ok() {
        return Ok(());
    }
    let mut from = File::open(src)?;
    let mut to = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)?;
    io::copy(&mut from, &mut to)?;
    to.set_permissions(meta.permissions())?;
    to.set_modified(meta.modified()?)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn cstr(path: &Path) -> io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

#[cfg(target_os = "macos")]
fn clonefile(src: &Path, dst: &Path) -> io::Result<()> {
    /// Clone a symlink itself rather than its target (`<sys/clonefile.h>`).
    const CLONE_NOFOLLOW: u32 = 0x0001;
    let (src, dst) = (cstr(src)?, cstr(dst)?);
    // SAFETY: both arguments are valid NUL-terminated paths for the call.
    if unsafe { libc::clonefile(src.as_ptr(), dst.as_ptr(), CLONE_NOFOLLOW) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn clonefile(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// Clones one regular file's extents, keeping mode and mtime.
#[cfg(target_os = "macos")]
fn reflink_file(src: &Path, dst: &Path) -> io::Result<()> {
    clonefile(src, dst)
}

#[cfg(target_os = "linux")]
fn reflink_file(src: &Path, dst: &Path) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let from = File::open(src)?;
    let meta = from.metadata()?;
    let to = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)?;
    // SAFETY: both descriptors are open for the duration of the call.
    let rc = unsafe { libc::ioctl(to.as_raw_fd(), libc::FICLONE as _, from.as_raw_fd()) };
    if rc != 0 {
        let err = io::Error::last_os_error();
        drop(to);
        let _ = fs::remove_file(dst);
        return Err(err);
    }
    to.set_permissions(meta.permissions())?;
    to.set_modified(meta.modified()?)?;
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn reflink_file(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}
