//! What a check run produces and the limits it runs under, shared by native
//! runs (`exec::native`) and the scripted runner of `exec::seam`: the exit,
//! the bounded output and result files, and how a full disk reads.

use std::collections::BTreeMap;
use std::time::Duration;

use aver_core::{BlobId, Environment};

/// Bytes kept from the start of a command's output; the rest of the budget
/// (`limits.output_bytes`) goes to the end.
pub const OUTPUT_HEAD_BYTES: usize = 8 * 1024;

/// Longest output line kept whole; longer lines are cut, so a single huge
/// line (minified code echoed in a stack trace) cannot push the end of the
/// output, where errors usually are, out of the budget.
pub const MAX_LINE_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxLimits {
    pub cpu_millis: u32,
    pub memory_mb: u32,
    pub pids: u32,
    /// Wall clock for the command.
    pub timeout: Duration,
    pub output_bytes: u32,
}

impl SandboxLimits {
    /// `limits` with `timeout_ms` as the wall clock.
    pub fn of(limits: &aver_core::Limits, timeout_ms: u32) -> Self {
        Self {
            cpu_millis: limits.cpu_millis,
            memory_mb: limits.memory_mb,
            pids: limits.pids,
            timeout: Duration::from_millis(timeout_ms.into()),
            output_bytes: limits.output_bytes,
        }
    }

    /// Each limit lowered to `max`'s where it is higher.
    pub fn clamped(self, max: &Self) -> Self {
        Self {
            cpu_millis: self.cpu_millis.min(max.cpu_millis),
            memory_mb: self.memory_mb.min(max.memory_mb),
            pids: self.pids.min(max.pids),
            timeout: self.timeout.min(max.timeout),
            output_bytes: self.output_bytes.min(max.output_bytes),
        }
    }
}

/// How the command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Exited(i32),
    /// The kernel killed a process for exceeding the memory limit.
    Oom(i32),
    /// The command outlived its timeout and was killed.
    Timeout(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxOutput {
    pub exit: Exit,
    /// Combined stdout and stderr, bounded per [`bound_output`].
    pub output: Vec<u8>,
    /// The result files the command wrote (reports), keyed by path relative
    /// to its output directory.
    pub files: BTreeMap<String, Vec<u8>>,
    pub duration: Duration,
    /// The built test files the run left in its directory, by path, as
    /// content hashes ([`crate::place::workdir::Workdir::derived`]); `None`
    /// when the runner did not look.
    pub built: Option<BTreeMap<String, BlobId>>,
}

/// What a run that lost its result to a full disk says: an `infra:
/// sandbox` run (cause `ENV`), never the change's failure.
pub const DISK_FULL: &str = "the disk is full";

/// Below this much free space a run does not start, and a failed native
/// run is taken as one the full disk cut short.
pub const MIN_FREE_BYTES: u64 = 64 << 20;

/// What a process prints when a write fails for lack of space.
const NO_SPACE: &[&[u8]] = &[b"No space left on device", b"ENOSPC"];

/// Bytes free to this user on the file system holding `dir` (`statvfs`).
pub fn free_bytes(dir: &std::path::Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt as _;
    let mut at = dir;
    while !at.exists() {
        at = at.parent()?;
    }
    let path = std::ffi::CString::new(at.as_os_str().as_bytes()).ok()?;
    // SAFETY: a zeroed statvfs is valid; statvfs fills it.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is a C string and `st` a statvfs.
    if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}

/// Whether `output` says a write failed because the disk was full.
pub fn says_disk_full(output: &[u8]) -> bool {
    NO_SPACE
        .iter()
        .any(|needle| output.windows(needle.len()).any(|w| w == *needle))
}

/// The generated environment of a check run: `HOME=/tmp`, `CI=1`,
/// `NODE_ENV=test`, the offline switches of the toolchains Aver has
/// presets for ([`OFFLINE_ENV`]), then `env.vars`, except
/// [`RESERVED_VAR`]. Nothing in it says which run of a check this is:
/// every run of a side gets the same environment, so code under test
/// cannot behave one way on a first run and another on a rerun (§1.9.3, a
/// head flaky over a stable base is `NEW`).
pub fn check_env(env: &Environment) -> BTreeMap<String, String> {
    let mut vars = generated_env();
    for (name, value) in OFFLINE_ENV {
        vars.insert((*name).to_owned(), (*value).to_owned());
    }
    for (name, value) in &env.vars {
        vars.insert(name.as_str().to_owned(), value.clone());
    }
    vars.remove(RESERVED_VAR);
    vars
}

/// The variable that once carried a run's index to the command; an
/// environment's `vars` cannot set it, so a test that reads it sees it
/// unset on every run.
pub const RESERVED_VAR: &str = "AVER_RUN_ATTEMPT";

/// The variables that keep toolchains off the network in a test run.
pub const OFFLINE_ENV: &[(&str, &str)] = &[
    ("CARGO_NET_OFFLINE", "true"),
    ("GOPROXY", "off"),
    ("GOTOOLCHAIN", "local"),
    ("PIP_NO_INDEX", "1"),
    ("UV_OFFLINE", "1"),
];

fn generated_env() -> BTreeMap<String, String> {
    [("HOME", "/tmp"), ("CI", "1"), ("NODE_ENV", "test")]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

/// Keeps the first [`OUTPUT_HEAD_BYTES`] and the last
/// `output_bytes - OUTPUT_HEAD_BYTES` bytes of `bytes`, dropping the middle.
/// With the default budget of 64 KiB that is the first 8 KiB plus the last
/// 56 KiB.
pub fn bound_output(bytes: &[u8], output_bytes: u32) -> Vec<u8> {
    let limit = output_bytes as usize;
    let head = OUTPUT_HEAD_BYTES.min(limit);
    let tail = limit - head;
    if bytes.len() <= head + tail {
        return bytes.to_vec();
    }
    [&bytes[..head], &bytes[bytes.len() - tail..]].concat()
}

#[cfg(test)]
mod tests {
    #[test]
    fn disk_full_output_and_free_space_are_read_crit_k2_disk_full() {
        assert!(super::says_disk_full(
            b"ok 1\nwrite /aver/out/unit.xml: No space left on device\n"
        ));
        assert!(super::says_disk_full(b"Error: ENOSPC: no space left"));
        assert!(!super::says_disk_full(b"FAIL negates\n"));
        let dir = tempfile::tempdir().unwrap();
        assert!(super::free_bytes(&dir.path().join("not/yet")).is_some_and(|f| f > 0));
    }
}
