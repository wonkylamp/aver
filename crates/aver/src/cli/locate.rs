//! Finding the programs Aver runs (Docker, Git, the agent CLIs) when the
//! caller's `PATH` is thin. GUI apps on macOS start their hooks with
//! launchd's `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`), which holds none of
//! Homebrew or `~/.local/bin`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// launchd's default `PATH`, what a GUI app on macOS starts with.
pub const GUI_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Where installers put `aver`, Docker and Git besides the system
/// directories, in the order they are searched after `PATH`.
pub fn install_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home_dir() {
        dirs.push(home.join(".local/bin"));
    }
    dirs.push("/opt/homebrew/bin".into());
    dirs.push("/usr/local/bin".into());
    if let Some(home) = home_dir() {
        dirs.push(home.join(".cargo/bin"));
    }
    dirs
}

/// `PATH` with the [`install_dirs`] it lacks appended.
pub fn widened_path() -> OsString {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&current).collect();
    for dir in install_dirs() {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    std::env::join_paths(dirs).unwrap_or(current)
}

fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// `name` in the directories of `path`, first match.
pub fn which_in(name: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|d| d.join(name))
        .find(|p| executable(p))
}

/// `name` on `PATH`.
pub fn which(name: &str) -> Option<PathBuf> {
    which_in(name, &std::env::var_os("PATH").unwrap_or_default())
}
