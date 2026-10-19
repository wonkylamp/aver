//! A run's processes (REDESIGN.md §3): one check's commands started in
//! their own process group with their output to a log, bounded by a
//! deadline, recorded while they run so a later process can kill what a
//! killed one left, and reaped afterwards, including what left the group.
//! Optionally as another user (`--run-as`, [`RunAs`]).

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::process::CommandExt as _;
use std::path::{Path as FsPath, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::disk::FileLock;
use crate::exec::output::{bound_output, Exit, SandboxOutput, MAX_LINE_BYTES};
use crate::store::StoreError;

pub mod interrupt;

/// The variable that marks every process a run starts, with a value new
/// for each run, so what the run left running is found after it ends even
/// when it left the run's process group (`setsid`, a daemon's double
/// fork).
pub const RUN_TAG_VAR: &str = "AVER_RUN";

/// Directories a program is looked up in after the run's own `PATH`.
const SYSTEM_PATH: &[&str] = &["/usr/bin", "/bin", "/usr/sbin", "/sbin"];

/// Variables a run never inherits, by exact name: what lets a step write
/// to the CI job (`GITHUB_OUTPUT` and the like), the runner's own tokens,
/// the tag of an outer run, and a custody test's fault rules.
pub const STRIPPED: &[&str] = &[
    "GITHUB_OUTPUT",
    "GITHUB_ENV",
    "GITHUB_PATH",
    "GITHUB_STEP_SUMMARY",
    "GITHUB_STATE",
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "ACTIONS_RUNTIME_TOKEN",
    "ACTIONS_RUNTIME_URL",
    "ACTIONS_RESULTS_URL",
    "ACTIONS_CACHE_URL",
    RUN_TAG_VAR,
    "AVER_NATIVE_RUN",
    "AVER_FAULT",
    crate::exec::output::RESERVED_VAR,
];

/// Prefixes of variables a run never inherits.
pub const STRIPPED_PREFIXES: &[&str] = &["ACTIONS_ID_TOKEN_REQUEST_"];

/// Whether a run never inherits variable `name` ([`STRIPPED`]).
pub fn stripped(name: &str) -> bool {
    STRIPPED.contains(&name) || STRIPPED_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// A run's environment (REDESIGN.md §3): the caller's own, minus
/// [`STRIPPED`], plus `AVER_OUT` (the run's result directory), the run's
/// tag and a private `TMPDIR` (concurrent sides of one suite would collide
/// on fixed names in a shared one). A `CARGO_TARGET_DIR` outside the slot
/// becomes `<slot>/target`, so the sides never share build outputs.
pub fn inherit(
    caller: impl IntoIterator<Item = (String, String)>,
    out: &FsPath,
    tmp: &FsPath,
    slot: &FsPath,
    tag: &str,
) -> BTreeMap<String, String> {
    let mut vars: BTreeMap<String, String> =
        caller.into_iter().filter(|(k, _)| !stripped(k)).collect();
    vars.insert("AVER_OUT".into(), out.to_string_lossy().into_owned());
    vars.insert(RUN_TAG_VAR.into(), tag.to_owned());
    vars.insert("TMPDIR".into(), tmp.to_string_lossy().into_owned());
    if let Some(target) = vars.get("CARGO_TARGET_DIR") {
        if !FsPath::new(target).starts_with(slot) {
            vars.insert(
                "CARGO_TARGET_DIR".into(),
                slot.join("target").to_string_lossy().into_owned(),
            );
        }
    }
    vars
}

/// Why a run has no result.
#[derive(Debug)]
pub enum ProcFailure {
    /// It could not start, or ended without a result of the code under
    /// test: becomes an `infra: sandbox` run (cause `ENV`), with what
    /// output there was.
    Failed { message: String, output: Vec<u8> },
    /// A signal asked this process to stop ([`interrupt`]): nothing of the
    /// run is kept.
    Interrupted,
}

impl ProcFailure {
    pub fn new(message: impl Into<String>) -> Self {
        Self::Failed {
            message: message.into(),
            output: Vec::new(),
        }
    }
}

impl From<io::Error> for ProcFailure {
    fn from(e: io::Error) -> Self {
        if e.raw_os_error() == Some(libc::ENOSPC) {
            return Self::new(format!("{} ({e})", crate::exec::output::DISK_FULL));
        }
        Self::new(format!("native run: {e}"))
    }
}

impl From<StoreError> for ProcFailure {
    fn from(e: StoreError) -> Self {
        Self::new(format!("native run: {e}"))
    }
}

/// A check's commands for [`run`]: the setup commands, then the check,
/// all in `cwd` with exactly `vars`, their output appended to `log`, the
/// result files collected from `out`, killed at `timeout` from the start.
pub struct Spawn<'a> {
    pub commands: &'a [Vec<String>],
    pub cwd: &'a FsPath,
    pub vars: &'a BTreeMap<String, String>,
    /// `$AVER_OUT`: the run's own new directory.
    pub out: &'a FsPath,
    pub log: &'a FsPath,
    pub timeout: Duration,
    pub output_bytes: u32,
    /// Run at lower priority (the transplant).
    pub nice: bool,
    /// Where the run is recorded while it runs (`<dir>.pid`) and the
    /// run's tag, which `vars` carries as [`RUN_TAG_VAR`].
    pub record: (&'a FsPath, &'a str),
    /// The report the check declares, relative to `out`: when written, the
    /// run's tests stand even on a nearly full disk.
    pub report: Option<&'a str>,
    /// The run's own directories, handed to the run user and back.
    pub own: &'a [&'a FsPath],
    /// Directories inside `own` that stay the verifier's: an adopted
    /// checkout's `.git` and installed dependencies.
    pub withheld: &'a [PathBuf],
    pub run_as: Option<&'a RunAs>,
}

/// Runs a check's commands to their end, blocking: all of them run, and
/// the exit is the check's unless that is 0 and a setup command failed.
/// What the run left running is killed after. A run SIGKILLed from outside
/// has no result, and neither has a failed run that ended on a nearly full
/// disk without its report.
pub fn run(s: &Spawn<'_>) -> Result<SandboxOutput, ProcFailure> {
    if let Some(run_as) = s.run_as {
        for dir in s.own {
            run_as.hand_over(dir, true)?;
        }
        for dir in s.withheld {
            run_as.hand_over(dir, false)?;
        }
    }
    let started = Instant::now();
    let deadline = started + s.timeout;
    let mut last_group = None;
    let mut tracked = Vec::new();
    let ran = (|| -> Result<Option<Exit>, ProcFailure> {
        let mut setup_failed = None;
        let total = s.commands.len();
        for (i, command) in s.commands.iter().enumerate() {
            if interrupt::requested() {
                return Err(ProcFailure::Interrupted);
            }
            let one = One {
                argv: command,
                cwd: s.cwd,
                vars: s.vars,
                log: s.log,
                deadline,
                nice: s.nice,
                run_as: s.run_as,
            };
            let (ended, group) = spawn(&one, Some(s.record), &mut tracked)?;
            if let Some(earlier) = last_group.replace(group) {
                interrupt::LIVE.clear(earlier);
            }
            match ended {
                Ended::Exited(code) if i + 1 == total => {
                    return Ok(Some(Exit::Exited(if code == 0 {
                        setup_failed.unwrap_or(0)
                    } else {
                        code
                    })));
                }
                Ended::Exited(code) => {
                    if code != 0 {
                        setup_failed.get_or_insert(code);
                        append(
                            s.log,
                            format!(
                                "[aver] setup command {} of {} exited with {code}: {}\n",
                                i + 1,
                                total - 1,
                                command.join(" ")
                            )
                            .as_bytes(),
                        )?;
                    }
                }
                Ended::Killed => return Ok(None),
                Ended::TimedOut => return Ok(Some(Exit::Timeout(137))),
                Ended::Interrupted => return Err(ProcFailure::Interrupted),
            }
        }
        Ok(Some(Exit::Exited(1)))
    })();
    let took = started.elapsed();
    if let Some(note) = reap(s.run_as, s.record.1, last_group, &tracked) {
        let _ = append(s.log, note.as_bytes());
    }
    let _ = fs::remove_file(s.record.0.with_extension("pid"));
    if let Some(run_as) = s.run_as {
        for dir in s.own {
            run_as.hand_over(dir, false)?;
        }
        run_as.sweep_tmp();
    }
    let report_written = s.report.is_some_and(|r| s.out.join(r).is_file());
    match ran? {
        Some(exit) if exit != Exit::Exited(0) && disk_low(s.record.0) && !report_written => {
            let message = format!(
                "{} under {}",
                crate::exec::output::DISK_FULL,
                s.record.0.parent().unwrap_or(s.record.0).display()
            );
            Err(ProcFailure::Failed {
                message,
                output: cap_lines(&fs::read(s.log).unwrap_or_default()),
            })
        }
        Some(exit) => Ok(output(exit, s.log, s.out, took, s.output_bytes)),
        None => Err(ProcFailure::Failed {
            message: "the check was killed by SIGKILL from outside the run (the out-of-memory killer, or another process), so it has no result".into(),
            output: cap_lines(&fs::read(s.log).unwrap_or_default()),
        }),
    }
}

/// A finished run's result: its bounded log and every file under `out`.
pub fn output(
    exit: Exit,
    log: &FsPath,
    out: &FsPath,
    duration: Duration,
    output_bytes: u32,
) -> SandboxOutput {
    let raw = fs::read(log).unwrap_or_default();
    let output = bound_output(&cap_lines(&raw), output_bytes);
    let mut files = BTreeMap::new();
    collect_files(out, out, &mut files);
    SandboxOutput {
        exit,
        output,
        files,
        duration,
        built: None,
    }
}

/// One command for [`spawn`]: `argv` in `cwd` with exactly `vars`, its
/// output appended to `log`, killed at `deadline`, at lower priority when
/// `nice`, and as `run_as`'s user when set.
#[derive(Clone, Copy)]
pub struct One<'a> {
    pub argv: &'a [String],
    pub cwd: &'a FsPath,
    pub vars: &'a BTreeMap<String, String>,
    pub log: &'a FsPath,
    pub deadline: Instant,
    pub nice: bool,
    pub run_as: Option<&'a RunAs>,
}

/// How a command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// It exited with this code, or a signal other than SIGKILL ended it
    /// (128 + the signal).
    Exited(i32),
    /// SIGKILL from outside the run (the out-of-memory killer, another
    /// process): no result of the code under test.
    Killed,
    /// Its deadline passed and Aver killed its process group.
    TimedOut,
    /// A signal asked this process to stop ([`interrupt`]): the group got
    /// SIGTERM, then SIGKILL after [`STOP_GRACE`].
    Interrupted,
}

/// How long a run's group has between SIGTERM and SIGKILL once this
/// process is asked to stop.
pub const STOP_GRACE: Duration = Duration::from_secs(2);

/// Runs one command as [`One`] says, until it exits, its deadline passes
/// or this process is asked to stop (then its process group is killed):
/// how it ended, and its process group, held in [`interrupt::LIVE`] until
/// [`reap`]. A check's command (`record`: its directory and the run's
/// tag) is recorded in `<dir>.pid` while it runs. Every quarter second it
/// adds the command's descendants to `tracked` (where the host cannot
/// read their environments' tags), so one that leaves the group and
/// outlives its parent is still found.
pub fn spawn(
    one: &One<'_>,
    record: Option<(&FsPath, &str)>,
    tracked: &mut Vec<Tracked>,
) -> Result<(Ended, i32), ProcFailure> {
    let One {
        argv,
        cwd,
        vars,
        log,
        deadline,
        nice,
        run_as,
    } = *one;
    let mut full: Vec<String> = Vec::new();
    if let Some(run_as) = run_as {
        full.extend([
            "sudo".into(),
            "-n".into(),
            "-u".into(),
            run_as.user.clone(),
            "--".into(),
        ]);
        full.push("/usr/bin/env".into());
        full.push("-i".into());
        full.extend(vars.iter().map(|(k, v)| format!("{k}={v}")));
    }
    full.extend(argv.iter().cloned());
    let program = if full[0].contains('/') {
        PathBuf::from(&full[0])
    } else {
        let path: Vec<PathBuf> = vars
            .get("PATH")
            .map(|p| std::env::split_paths(&OsString::from(p)).collect())
            .unwrap_or_default();
        path.iter()
            .chain(
                SYSTEM_PATH
                    .iter()
                    .map(PathBuf::from)
                    .collect::<Vec<_>>()
                    .iter(),
            )
            .map(|d| d.join(&full[0]))
            .find(|c| is_executable(c))
            .ok_or_else(|| {
                ProcFailure::new(format!("{} is not installed on this machine", full[0]))
            })?
    };
    let file = fs::OpenOptions::new().create(true).append(true).open(log)?;
    let mut cmd = Command::new(program);
    cmd.args(&full[1..])
        .current_dir(cwd)
        .env_clear()
        .envs(vars)
        .stdin(Stdio::null())
        .stdout(file.try_clone()?)
        .stderr(file)
        .process_group(0);
    if nice {
        // SAFETY: setpriority is async-signal-safe; it lowers only the
        // child's own priority.
        unsafe {
            cmd.pre_exec(|| {
                libc::setpriority(libc::PRIO_PROCESS, 0, 10);
                Ok(())
            });
        }
    }
    let claim = interrupt::LIVE.claim().ok_or_else(|| {
        ProcFailure::new(format!(
            "more than {} commands are running at once in this process",
            interrupt::CAPACITY
        ))
    })?;
    let mut child = cmd
        .spawn()
        .map_err(|e| ProcFailure::new(format!("could not run {}: {e}", argv.join(" "))))?;
    let group = child.id() as i32;
    claim.set(group);
    let recorded = record.map_or(Ok(()), |(dir, tag)| {
        let record = PidFile {
            pgid: group,
            start: crate::run::state::process_start(child.id()),
            tag: tag.to_owned(),
        };
        crate::atomic::write(
            &dir.with_extension("pid"),
            &serde_json::to_vec(&record).expect("pid files serialize"),
            crate::atomic::Mode::Keep,
        )
        .and_then(|()| crate::fault::point("proc.spawned"))
    });
    let ended = recorded
        .map_err(ProcFailure::from)
        .and_then(|()| watch(&mut child, group, deadline, run_as, tracked).map_err(Into::into));
    if ended.is_err() {
        // SAFETY: the group spawned above, its leader not reaped yet.
        unsafe { libc::kill(-group, libc::SIGKILL) };
        let _ = child.wait();
        interrupt::LIVE.clear(group);
    }
    Ok((ended?, group))
}

/// Polls `child`, the leader of `group`, until it exits, its deadline
/// passes or this process is asked to stop.
fn watch(
    child: &mut std::process::Child,
    group: i32,
    deadline: Instant,
    run_as: Option<&RunAs>,
    tracked: &mut Vec<Tracked>,
) -> io::Result<Ended> {
    let mut stopping: Option<Instant> = None;
    let mut polls = 0u32;
    loop {
        if let Some(status) = child.try_wait()? {
            // The handler's SIGTERM may end it before this loop sees the
            // request.
            if stopping.is_some() || interrupt::requested() {
                return Ok(Ended::Interrupted);
            }
            return Ok(match (status.code(), status_signal(&status)) {
                (Some(code), _) => Ended::Exited(code),
                (None, libc::SIGKILL) => Ended::Killed,
                (None, signal) => Ended::Exited(128 + signal),
            });
        }
        if stopping.is_none() && interrupt::requested() {
            // The handler signalled only the groups running then.
            // SAFETY: signals the process group spawned above, whose
            // leader is not reaped yet.
            unsafe { libc::kill(-group, libc::SIGTERM) };
            stopping = Some(Instant::now() + STOP_GRACE);
        }
        let now = Instant::now();
        if now >= deadline || stopping.is_some_and(|at| now >= at) {
            // SAFETY: as above.
            unsafe { libc::kill(-group, libc::SIGKILL) };
            if let Some(run_as) = run_as {
                let _ = sudo(&["kill", "-KILL", "--", &format!("-{group}")]);
                let _ = sudo(&["pkill", "-KILL", "-u", &run_as.user]);
            }
            let _ = child.wait();
            return Ok(match stopping {
                Some(_) => Ended::Interrupted,
                None => Ended::TimedOut,
            });
        }
        polls += 1;
        if run_as.is_none() && (matches!(polls, 2 | 5 | 10) || polls.is_multiple_of(25)) {
            track_descendants(group, tracked);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Kills whatever a run left running once its last command ended: its
/// last process group and every process carrying its tag, or, with uid
/// separation, every process of the run user (runs then never overlap, so
/// all of them are this run's). Returns the note for the run's log when
/// there were any.
pub fn reap(
    run_as: Option<&RunAs>,
    tag: &str,
    group: Option<i32>,
    tracked: &[Tracked],
) -> Option<String> {
    let note = reap_all(run_as, tag, group, tracked);
    if let Some(group) = group {
        interrupt::LIVE.clear(group);
    }
    note
}

fn reap_all(
    run_as: Option<&RunAs>,
    tag: &str,
    group: Option<i32>,
    tracked: &[Tracked],
) -> Option<String> {
    if let Some(RunAs { user }) = run_as {
        return sudo(&["pkill", "-KILL", "-u", user]).then(|| {
            format!("[aver] the run left processes of {user} running after it ended; Aver killed them\n")
        });
    }
    let in_group = group.is_some_and(kill_group);
    let tagged = kill_tagged(tag).max(kill_tracked(tracked));
    if in_group || tagged > 0 {
        wait_gone(group, tracked);
    }
    match (in_group, tagged) {
        (false, 0) => None,
        (_, 0) => Some(
            "[aver] the run left processes behind in its process group; Aver killed them\n".into(),
        ),
        (_, n) => Some(format!(
            "[aver] the run left {n} process{} behind; Aver killed {}\n",
            if n == 1 { "" } else { "es" },
            if n == 1 { "it" } else { "them" }
        )),
    }
}

/// Uid separation (`aver ci --run-as`, §3.2): every command runs as `user`
/// through `sudo -n -u`, and runs never overlap, since one run's cleanup
/// (and its leftovers) would reach the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunAs {
    pub user: String,
}

impl RunAs {
    /// Deletes what the run user left in the shared temporary directories,
    /// so nothing of one side reaches the next (a base rerun after head
    /// code, say).
    pub fn sweep_tmp(&self) {
        for dir in ["/tmp", "/var/tmp", "/dev/shm"] {
            if FsPath::new(dir).is_dir() {
                let _ = sudo(&[
                    "find",
                    dir,
                    "-xdev",
                    "-mindepth",
                    "1",
                    "-user",
                    &self.user,
                    "-delete",
                ]);
            }
        }
    }

    /// Gives a run's own directory to the run user before a run
    /// (`to_run_user`) and takes it back after, so the verifier can
    /// rewrite it next time. The shared caches stay the verifier's: the
    /// run user reads them and never writes them.
    pub fn hand_over(&self, dir: &FsPath, to_run_user: bool) -> Result<(), ProcFailure> {
        let owner = if to_run_user {
            self.user.clone()
        } else {
            // SAFETY: getuid cannot fail.
            unsafe { libc::getuid() }.to_string()
        };
        if fs::symlink_metadata(dir).is_err() {
            return Ok(());
        }
        if !sudo(&["chown", "-R", &owner, &dir.to_string_lossy()]) {
            return Err(ProcFailure::new(format!(
                "could not hand {} to {owner} with sudo",
                dir.display()
            )));
        }
        Ok(())
    }
}

/// `sudo -n <args>`, quietly: whether it succeeded.
pub fn sudo(args: &[&str]) -> bool {
    Command::new("sudo")
        .arg("-n")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `<slot>.pid` while a check's command runs: its process group,
/// the leader's start time and the run's tag, so whoever next takes the
/// slot's lock (or the home's recovery, [`kill_leftovers`]) finds a run a
/// killed `aver` left behind.
#[derive(Debug, Serialize, Deserialize)]
struct PidFile {
    pgid: i32,
    #[serde(default)]
    start: Option<u64>,
    tag: String,
}

/// Kills what the run recorded at `dir.pid` left running, and removes the
/// record: the note for the next run's log when something was killed. The
/// caller holds the check's lock, so the record is never a live run's.
pub fn kill_leftover(dir: &FsPath) -> Option<String> {
    let path = dir.with_extension("pid");
    let record: Option<PidFile> = serde_json::from_slice(&fs::read(&path).ok()?).ok();
    let _ = fs::remove_file(&path);
    let record = record?;
    let leader = u32::try_from(record.pgid).ok()?;
    // A live leader with another start time is a process that reused the
    // id. A dead leader whose group still has members is the run's: the
    // kernel never hands out an id still in use as a process group.
    let in_group = match crate::run::state::process_start(leader) {
        Some(now) => record.start.is_none_or(|s| s == now) && kill_group(record.pgid),
        None => kill_group(record.pgid),
    };
    let tagged = if record.tag.is_empty() {
        0
    } else {
        kill_tagged(&record.tag)
    };
    (in_group || tagged > 0).then(|| {
        "[aver] an earlier run of this check, left by an Aver process that was killed, was still running; Aver killed it before this run\n".to_owned()
    })
}

/// Kills the runs the slots under `root` (`$AVER_HOME/sides`) left
/// running when the process running them was killed (§2.7.3, recovery):
/// the slots whose record it found, as `<repo16>/<slot>`. A slot whose lock
/// a live run holds is left alone.
pub fn kill_leftovers(root: &FsPath) -> Vec<String> {
    let mut killed = Vec::new();
    for repo in fs::read_dir(root).into_iter().flatten().flatten() {
        for entry in fs::read_dir(repo.path()).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "pid") {
                continue;
            }
            let dir = path.with_extension("");
            let Ok(Some(_lock)) = FileLock::try_exclusive(&dir.with_extension("lock")) else {
                continue;
            };
            if kill_leftover(&dir).is_some() {
                killed.push(format!(
                    "{}/{}",
                    repo.file_name().to_string_lossy(),
                    dir.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
        }
    }
    killed
}

/// Removes the result directories of earlier runs of the check at `dir`
/// (`run-<ulid>`).
pub fn remove_runs(dir: &FsPath) -> io::Result<()> {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().starts_with("run-") {
            remove_all(&entry.path())?;
        }
    }
    Ok(())
}

/// Refuses to start a run on a nearly full disk: its report would be lost
/// and read as the change's failure.
pub fn disk_guard(dir: &FsPath) -> Result<(), ProcFailure> {
    match crate::exec::output::free_bytes(dir) {
        Some(free) if free < crate::exec::output::MIN_FREE_BYTES => Err(ProcFailure::new(format!(
            "{} ({} MiB free under {})",
            crate::exec::output::DISK_FULL,
            free >> 20,
            dir.display()
        ))),
        _ => Ok(()),
    }
}

/// Whether the disk under `dir` is now nearly full.
pub fn disk_low(dir: &FsPath) -> bool {
    crate::exec::output::free_bytes(dir).is_some_and(|f| f < crate::exec::output::MIN_FREE_BYTES)
}

/// Kills process group `group` if anything is left in it: whether there
/// was. Called right after its leader is reaped, so the id is not reused.
pub fn kill_group(group: i32) -> bool {
    // SAFETY: signal 0 only checks; the group is the one the run started.
    if unsafe { libc::kill(-group, 0) } != 0 {
        return false;
    }
    // SAFETY: as above.
    unsafe { libc::kill(-group, libc::SIGKILL) };
    true
}

/// Waits, at most two seconds, until what [`reap_all`] killed has exited:
/// SIGKILL is delivered asynchronously, and a listener still exiting holds
/// its port against the next run.
fn wait_gone(group: Option<i32>, tracked: &[Tracked]) {
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        // SAFETY: signal 0 only checks.
        let group_left = group.is_some_and(|g| unsafe { libc::kill(-g, 0) } == 0);
        let table = if tracked.is_empty() {
            Vec::new()
        } else {
            proc_table()
        };
        let tracked_left = tracked.iter().any(|(pid, start)| {
            table.iter().any(|p| p.0 == *pid && p.2 == *start)
                && crate::run::state::process_alive(*pid as u32)
        });
        if !group_left && !tracked_left {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Kills every process this user may signal whose environment holds
/// `RUN_TAG_VAR=tag`, until none is left: how many it found.
pub fn kill_tagged(tag: &str) -> usize {
    let needle = format!("{RUN_TAG_VAR}={tag}").into_bytes();
    let mut seen = BTreeSet::new();
    for _ in 0..20 {
        let found = tagged_pids(&needle);
        if found.is_empty() {
            break;
        }
        for pid in found {
            seen.insert(pid);
            // SAFETY: the process carries this run's tag.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    seen.len()
}

/// The processes whose initial environment holds `needle` exactly, among
/// those this user can read.
#[cfg(target_os = "linux")]
fn tagged_pids(needle: &[u8]) -> Vec<i32> {
    let me = std::process::id() as i32;
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| *pid != me)
        .filter(|pid| {
            fs::read(format!("/proc/{pid}/environ"))
                .is_ok_and(|env| env.split(|b| *b == 0).any(|v| v == needle))
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn tagged_pids(needle: &[u8]) -> Vec<i32> {
    use std::mem::size_of;
    let me = std::process::id() as i32;
    let mut pids: Vec<libc::pid_t> = vec![0; 4096];
    let count = loop {
        let bytes = (pids.len() * size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: the buffer holds `bytes` bytes.
        let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        if n < 0 {
            return Vec::new();
        }
        if (n as usize) < pids.len() {
            break n as usize;
        }
        pids.resize(pids.len() * 2, 0);
    };
    pids.truncate(count);
    let mut argmax: libc::c_int = 0;
    let mut size = size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: `argmax` is a c_int and `size` says so.
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&mut argmax as *mut libc::c_int).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    if !ok || argmax <= 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; argmax as usize];
    let mut out = Vec::new();
    for pid in pids {
        if pid <= 0 || pid == me {
            continue;
        }
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut size = buf.len();
        // SAFETY: `buf` holds `size` bytes.
        let ok = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                buf.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } == 0;
        if ok
            && buf[..size.min(buf.len())]
                .split(|b| *b == 0)
                .any(|v| v == needle)
        {
            out.push(pid);
        }
    }
    out
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn tagged_pids(_needle: &[u8]) -> Vec<i32> {
    Vec::new()
}

/// A process seen descending from a run: its id and start time, so a
/// reused id is never taken for it.
pub type Tracked = (i32, u64);

/// Adds the processes descending from `root` or from one already tracked
/// to `tracked`. Only where the environment of another process may hide
/// the run's tag: macOS shows no environment of a platform binary
/// (`/usr/bin/nc`, `/bin/sleep`) to anyone else.
pub fn track_descendants(root: i32, tracked: &mut Vec<Tracked>) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let table = proc_table();
    let mut known: BTreeSet<i32> = tracked
        .iter()
        .filter(|(pid, start)| table.iter().any(|p| p.0 == *pid && p.2 == *start))
        .map(|(pid, _)| *pid)
        .collect();
    known.insert(root);
    loop {
        let mut added = false;
        for &(pid, ppid, start) in &table {
            if known.contains(&ppid) && known.insert(pid) {
                tracked.push((pid, start));
                added = true;
            }
        }
        if !added {
            break;
        }
    }
}

/// Kills every tracked process still running: how many there were.
pub fn kill_tracked(tracked: &[Tracked]) -> usize {
    if tracked.is_empty() {
        return 0;
    }
    let table = proc_table();
    let mut killed = 0;
    for (pid, start) in tracked {
        if table.iter().any(|p| p.0 == *pid && p.2 == *start) {
            // SAFETY: the process descends from the run (same id and start).
            unsafe { libc::kill(*pid, libc::SIGKILL) };
            killed += 1;
        }
    }
    killed
}

/// `(pid, parent pid, start time in microseconds)` of every process.
#[cfg(target_os = "macos")]
fn proc_table() -> Vec<(i32, i32, u64)> {
    use std::mem::size_of;
    let mut pids: Vec<libc::pid_t> = vec![0; 4096];
    let count = loop {
        let bytes = (pids.len() * size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: the buffer holds `bytes` bytes.
        let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        if n < 0 {
            return Vec::new();
        }
        if (n as usize) < pids.len() {
            break n as usize;
        }
        pids.resize(pids.len() * 2, 0);
    };
    pids.truncate(count);
    let mut out = Vec::with_capacity(count);
    for pid in pids.into_iter().filter(|p| *p > 0) {
        // SAFETY: an all-zero proc_bsdinfo is valid.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: `info` holds `size` bytes.
        let got = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        if got == size {
            out.push((
                pid,
                info.pbi_ppid as i32,
                info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
            ));
        }
    }
    out
}

#[cfg(not(target_os = "macos"))]
fn proc_table() -> Vec<(i32, i32, u64)> {
    Vec::new()
}

/// The run's `TMPDIR`: a new private directory under `/tmp`, in the
/// process's group, with a short random name (a Unix socket's path must fit in about 104 bytes and test
/// suites put sockets in `TMPDIR`; a name nobody can predict, so no other
/// run or user can prepare it). Its path is recorded in the check's
/// directory, so the next run removes it if this one never got to.
pub fn private_tmp(dir: &FsPath) -> Result<PathBuf, ProcFailure> {
    let record = dir.join("tmp.last");
    if let Ok(last) = fs::read_to_string(&record) {
        let last = PathBuf::from(last.trim());
        let ours = fs::symlink_metadata(&last).is_ok_and(|m| {
            // SAFETY: getuid cannot fail.
            m.is_dir() && m.uid() == unsafe { libc::getuid() }
        });
        if ours && last.starts_with("/tmp") {
            let _ = remove_all(&last);
        }
    }
    let tmp = tempfile::Builder::new()
        .prefix("aver-")
        .rand_bytes(10)
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")?
        .keep();
    // A new file takes its directory's group on BSD, and `/tmp`'s is
    // `wheel`: tests that check their files belong to the user's group
    // would fail on every side.
    // SAFETY: getegid cannot fail.
    std::os::unix::fs::chown(&tmp, None, Some(unsafe { libc::getegid() }))?;
    fs::create_dir_all(dir)?;
    fs::write(&record, tmp.as_os_str().as_encoded_bytes())?;
    Ok(tmp)
}

/// Takes the exclusive lock at `path`, again if `aver gc` moved the file
/// away while this process waited for it, so two runs never hold locks on
/// different files for one directory.
pub(crate) fn lock_in_place(path: &FsPath) -> io::Result<FileLock> {
    loop {
        let lock = FileLock::exclusive(path)?;
        let same = match (fs::metadata(path), lock.metadata()) {
            (Ok(on_disk), Ok(held)) => on_disk.ino() == held.ino() && on_disk.dev() == held.dev(),
            _ => false,
        };
        if same {
            return Ok(lock);
        }
    }
}

/// Whether `sudo -n` works for `user` here: uid separation is possible.
pub fn sudo_works(user: &str) -> bool {
    Command::new("sudo")
        .args(["-n", "-u", user, "--", "/usr/bin/true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub fn status_signal(status: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt as _;
    status.signal().unwrap_or(0)
}

pub fn append(log: &FsPath, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?
        .write_all(bytes)
}

/// Lines longer than [`MAX_LINE_BYTES`] cut and marked, as the sandbox's
/// wrapper does (§1.9.1).
pub fn cap_lines(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        if line.len() > MAX_LINE_BYTES + 1 {
            out.extend_from_slice(&line[..MAX_LINE_BYTES]);
            out.extend_from_slice(
                format!("\n[aver] line cut at {MAX_LINE_BYTES} bytes\n").as_bytes(),
            );
        } else {
            out.extend_from_slice(line);
        }
    }
    out
}

pub fn collect_files(root: &FsPath, dir: &FsPath, out: &mut BTreeMap<String, Vec<u8>>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            collect_files(root, &path, out);
        } else if meta.is_file() {
            if let (Ok(rel), Ok(bytes)) = (path.strip_prefix(root), fs::read(&path)) {
                out.insert(rel.to_string_lossy().into_owned(), bytes);
            }
        }
    }
}

pub fn remove_all(path: &FsPath) -> io::Result<()> {
    crate::disk::make_writable(path).ok();
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn is_executable(path: &FsPath) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether a process of group `pgid` runs (zombies, which a lazily
    /// reaping init keeps for a while, do not count).
    fn running(pgid: i32) -> bool {
        // SAFETY: signal 0 only checks.
        if unsafe { libc::kill(-pgid, 0) } != 0 {
            return false;
        }
        !cfg!(target_os = "linux")
            || fs::read_dir("/proc").unwrap().flatten().any(|e| {
                fs::read_to_string(e.path().join("stat")).is_ok_and(|stat| {
                    let rest: Vec<&str> = stat[stat.rfind(')').unwrap() + 1..]
                        .split_whitespace()
                        .collect();
                    rest[0] != "Z" && rest[2] == pgid.to_string()
                })
            })
    }

    #[test]
    fn a_leftover_run_is_killed_on_taking_the_lock_crit_k2_stale_reporter() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("repo/B/unit");
        fs::create_dir_all(dir.parent().unwrap()).unwrap();
        let mut child = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let pgid = child.id() as i32;
        let record = PidFile {
            pgid,
            start: crate::run::state::process_start(child.id()),
            tag: String::new(),
        };
        fs::write(
            dir.with_extension("pid"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        assert!(kill_leftover(&dir).is_some());
        child.wait().unwrap();
        assert!(!running(pgid));
        assert!(!dir.with_extension("pid").exists());
        assert!(kill_leftover(&dir).is_none(), "nothing recorded now");
    }

    #[test]
    fn a_record_naming_a_reused_pid_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("unit");
        let mut child = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let pgid = child.id() as i32;
        let record = PidFile {
            pgid,
            start: crate::run::state::process_start(child.id()).map(|s| s + 1),
            tag: String::new(),
        };
        fs::write(
            dir.with_extension("pid"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        assert!(kill_leftover(&dir).is_none());
        assert!(running(pgid), "another process's group is never signalled");
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn enospc_in_the_runner_reads_as_a_full_disk_crit_k2_disk_full() {
        let message = |e: i32| match ProcFailure::from(io::Error::from_raw_os_error(e)) {
            ProcFailure::Failed { message, .. } => message,
            ProcFailure::Interrupted => panic!("an error is never an interruption"),
        };
        let full = message(libc::ENOSPC);
        assert!(full.starts_with(crate::exec::output::DISK_FULL), "{full}");
        assert!(message(libc::EACCES).starts_with("native run:"));
    }

    #[test]
    fn earlier_run_directories_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        for d in ["run", "run-01A", "run-01B", "src", "cache"] {
            fs::create_dir_all(dir.path().join(d).join("out")).unwrap();
        }
        remove_runs(dir.path()).unwrap();
        let mut left: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["cache", "run", "src"]);
    }
}
