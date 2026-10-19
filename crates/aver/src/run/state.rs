//! Host files of a run (§2.10): `$AVER_HOME/runs/<id>/run.json`, written
//! only by the supervisor (and by `aver accept` and `aver discard` once it
//! is gone), plus the small files `aver run` and the agent-facing commands
//! leave in a workspace's `.aver/`. None of these is a stored object.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use aver_core::{AgentId, AttemptId, SnapshotId, TaskId, Timestamp, WorkspaceId};
use serde::{Deserialize, Serialize};

use crate::atomic;
use crate::disk::Ports;
use crate::keys::Home;

pub const RUN_SCHEMA: &str = "aver.runstate/v0";

/// What an agent is doing, as the supervisor last saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Preparing,
    Working,
    /// The agent is running `aver verify`.
    Verifying,
    /// The agent exited; the supervisor syncs, verifies and gates it.
    Checking,
    /// Its latest receipt is `PASS`.
    Done,
    GaveUp,
    /// Not done, and its verify budget is spent.
    OutOfVerifies,
    /// Exited not done, and its template cannot be resumed.
    Stopped,
    /// Exited not done twice in a row without changing a file, so it is
    /// not resumed again.
    Idle,
    Timeout,
    Killed,
    Failed,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Working => "working",
            Self::Verifying => "verifying",
            Self::Checking => "checking",
            Self::Done => "done",
            Self::GaveUp => "gave up",
            Self::OutOfVerifies => "budget",
            Self::Stopped => "stopped",
            Self::Idle => "idle",
            Self::Timeout => "timeout",
            Self::Killed => "killed",
            Self::Failed => "failed",
        }
    }

    /// Whether the agent will do nothing more in this run.
    pub fn is_terminal(self) -> bool {
        !matches!(
            self,
            Self::Preparing | Self::Working | Self::Verifying | Self::Checking
        )
    }
}

/// Tokens and money an agent reported, when its output says.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
}

impl Usage {
    /// `$1.84`, `212k tok`, or `-`.
    pub fn short(&self) -> String {
        if let Some(cost) = self.cost_usd {
            return format!("${cost:.2}");
        }
        match (self.input_tokens, self.output_tokens) {
            (None, None) => "-".into(),
            (i, o) => {
                let total = i.unwrap_or(0) + o.unwrap_or(0);
                if total >= 1000 {
                    format!("{}k tok", total / 1000)
                } else {
                    format!("{total} tok")
                }
            }
        }
    }
}

/// One agent of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRun {
    /// 1-based, as `aver accept <n>` names it.
    pub index: u32,
    /// The template's name: `claude`, `codex` or `custom`.
    pub template: String,
    pub attempt: AttemptId,
    pub agent: AgentId,
    /// The key file name under `$AVER_HOME/keys`.
    pub key: String,
    pub workspace: WorkspaceId,
    pub dir: PathBuf,
    #[serde(default)]
    pub pid: Option<u32>,
    /// The agent's process group, which timeouts and `aver discard` signal.
    #[serde(default)]
    pub pgid: Option<i32>,
    /// The agent process's start time ([`process_start`]): `aver discard`
    /// signals the group only while its leader is still that process.
    #[serde(default)]
    pub proc_start: Option<u64>,
    pub state: AgentState,
    #[serde(default)]
    pub exit: Option<i32>,
    pub log: PathBuf,
    #[serde(default)]
    pub ports: Option<Ports>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub usage: Usage,
    /// Times the supervisor resumed the agent with a report.
    #[serde(default)]
    pub resumes: u32,
    /// Receipts the attempt has, as the supervisor last counted them: what
    /// a batch's verify budget sums.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub verifies: u32,
    #[serde(default)]
    pub gave_up: Option<String>,
    /// What the agent did last, from its output.
    #[serde(default)]
    pub last_action: Option<String>,
    pub started_at: Timestamp,
    #[serde(default)]
    pub ended_at: Option<Timestamp>,
    /// What the agent said when asked about Aver after the run
    /// (`aver run --feedback`, `aver feedback`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<AgentFeedback>,
}

/// An agent's answers to [`super::feedback::QUESTIONS`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentFeedback {
    /// Its 1-10 rating against "I absolutely need this", when it gave one.
    #[serde(default)]
    pub rating: Option<u8>,
    /// `runs/<run>/feedback-<n>.md`.
    pub file: PathBuf,
    pub at: Timestamp,
    /// The workspace's files changed while it answered (they should not).
    #[serde(default)]
    pub changed_files: bool,
}

impl AgentRun {
    pub fn wall_ms(&self) -> u64 {
        let end = self.ended_at.unwrap_or_else(Timestamp::now);
        (end.unix_millis() - self.started_at.unix_millis()).max(0) as u64
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    pub timeout_ms: u64,
    pub idle_ms: u64,
    pub max_verifies: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Supervisor {
    pub pid: u32,
    pub started_at: Timestamp,
    /// The process's start time ([`process_start`]), so a reused pid is
    /// never taken for the supervisor.
    #[serde(default)]
    pub start: Option<u64>,
}

/// How the run ended for the human.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub winner: AttemptId,
    pub index: u32,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub ledger: Option<aver_core::LedgerId>,
    #[serde(default)]
    pub pr: Option<String>,
    pub at: Timestamp,
    /// The winner's committed snapshot, which `aver combine` merges.
    #[serde(default)]
    pub head: Option<SnapshotId>,
    /// The winner's verdict when it was accepted. `REVIEW` here is a
    /// person's approval of its changed test expectations (`aver accept`,
    /// or `--approve-review`), which lets work that carries it build on
    /// it; a decision recorded before Aver kept it has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<aver_core::Verdict>,
}

/// `run.json` (`aver.runstate/v0`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunState {
    pub schema: String,
    pub id: String,
    pub repo: PathBuf,
    pub base_commit: String,
    pub base: SnapshotId,
    pub task: TaskId,
    pub title: String,
    pub supervisor: Supervisor,
    pub budget: Budget,
    pub agents: Vec<AgentRun>,
    #[serde(default)]
    pub finished_at: Option<Timestamp>,
    #[serde(default)]
    pub decided: Option<Decision>,
    #[serde(default)]
    pub discarded: bool,
    /// The batch this run is one task of (`aver run --tasks`, `--issue`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<BatchRef>,
    /// Where the run starts when it builds on other tasks' accepted work (a
    /// task graph's `after: #n`, `aver rerun --onto`) instead of the
    /// batch's base; `base_commit` is then `stacked_on.commit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stacked_on: Option<Stack>,
}

/// `runs/<id>/accepting.json`: the journal of an accept under way, written
/// before its first step and removed once `decided` is saved, so an accept
/// run again after a crash knows the branch it was writing and the
/// snapshot it verified. A file of its own rather than a `run.json` field,
/// so `run.json` keeps its shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Accepting {
    pub attempt: AttemptId,
    pub index: u32,
    pub branch: String,
    pub receipt: aver_core::ReceiptId,
    /// The snapshot the receipt verified: what the branch must hold.
    pub snapshot: SnapshotId,
    /// The last step done: `journaled`, `committed`, `decided`,
    /// `exported` or `released`.
    pub step: String,
    /// The filesystem's clock beside the directory just before the first
    /// try at the commit (nanoseconds): a file changed since is not in the
    /// branch, so the directory is kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<i64>,
    /// The `REVIEW` receipt a person approved with `--approve-review`: an
    /// accept run again finishes with it only while `receipt` and
    /// `snapshot` are still this receipt's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<aver_core::ReceiptId>,
}

impl Accepting {
    fn path(home: &Home, run: &str) -> PathBuf {
        run_dir(home, run).join("accepting.json")
    }

    pub fn load(home: &Home, run: &str) -> Option<Self> {
        serde_json::from_slice(&fs::read(Self::path(home, run)).ok()?).ok()
    }

    pub fn save(&self, home: &Home, run: &str) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).expect("journals serialize");
        atomic::write(&Self::path(home, run), &bytes, atomic::Mode::Keep)
    }

    pub fn clear(home: &Home, run: &str) {
        let _ = fs::remove_file(Self::path(home, run));
    }
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// A run stacked on other work: its base is a prerequisite's accepted
/// winner, or a combine of several.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stack {
    pub onto: StackOnto,
    /// The commit the run starts from (its `base_commit`).
    pub commit: String,
    /// The accepted snapshot it builds on.
    pub head: SnapshotId,
    /// Where the chain starts: the batch's base snapshot, of which every
    /// run in the chain is a descendant.
    pub root: SnapshotId,
}

/// What a stacked run builds on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StackOnto {
    /// Another run's accepted winner; `number` is its task number in the
    /// batch, when it has one.
    Run { run: String, number: Option<u32> },
    /// The accepted winners of several runs, combined and exported as
    /// `branch`.
    Combine { branch: String, runs: Vec<String> },
}

/// Where a run sits in a batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchRef {
    pub id: String,
    /// The task's number in the batch: its line's position in a tasks file,
    /// or the issue number.
    pub number: u32,
    /// The GitHub issue the task came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<u64>,
}

/// A fresh run id: six characters of a ULID's random part.
pub fn new_run_id() -> String {
    let ulid = ulid::Ulid::generate().to_string();
    ulid[ulid.len() - 6..].to_owned()
}

pub fn runs_dir(home: &Home) -> PathBuf {
    home.root().join("runs")
}

pub fn run_dir(home: &Home, id: &str) -> PathBuf {
    runs_dir(home).join(id)
}

impl RunState {
    pub fn path(&self, home: &Home) -> PathBuf {
        run_dir(home, &self.id).join("run.json")
    }

    fn lock(home: &Home, id: &str) -> io::Result<crate::disk::FileLock> {
        crate::disk::FileLock::exclusive(&run_dir(home, id).join("run.lock"))
    }

    fn write(&self, home: &Home) -> io::Result<()> {
        let path = self.path(home);
        fs::create_dir_all(path.parent().expect("run.json has a directory"))?;
        atomic::sweep(&path);
        let bytes = serde_json::to_vec_pretty(self).expect("run state serializes");
        atomic::write_at(Some("run.save"), &path, &bytes, atomic::Mode::Keep)
    }

    /// Loads run `id`, applies `f` and saves it, holding `runs/<id>/run.lock`
    /// throughout, as [`super::batch::BatchState::update`] does. Returns
    /// the saved state. Every writer but the supervisor goes through here.
    pub fn update(home: &Home, id: &str, f: impl FnOnce(&mut RunState)) -> io::Result<Self> {
        let _lock = Self::lock(home, id)?;
        let mut state = Self::load(home, id)?;
        f(&mut state);
        state.write(home)?;
        Ok(state)
    }

    /// The supervisor's save, under `runs/<id>/run.lock`: its view of the
    /// run over what the file holds, keeping what other commands wrote
    /// there meanwhile through [`Self::update`] (a decision, a discard, an
    /// agent's feedback).
    pub fn save(&self, home: &Home) -> io::Result<()> {
        let _lock = Self::lock(home, &self.id)?;
        let mut merged = self.clone();
        if let Ok(disk) = Self::load(home, &self.id) {
            merged.decided = disk.decided.or(merged.decided);
            merged.discarded |= disk.discarded;
            merged.finished_at = merged.finished_at.or(disk.finished_at);
            for agent in &mut merged.agents {
                let theirs = disk.agents.iter().find(|a| a.index == agent.index);
                if agent.feedback.is_none() {
                    agent.feedback = theirs.and_then(|a| a.feedback.clone());
                }
            }
        }
        merged.write(home)
    }

    pub fn load(home: &Home, id: &str) -> io::Result<Self> {
        let bytes = fs::read(run_dir(home, id).join("run.json"))?;
        serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Every run of the home, newest first.
    pub fn all(home: &Home) -> Vec<Self> {
        let Ok(entries) = fs::read_dir(runs_dir(home)) else {
            return Vec::new();
        };
        let mut runs: Vec<Self> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| Self::load(home, &e.file_name().to_string_lossy()).ok())
            .collect();
        runs.sort_by(|a, b| {
            b.supervisor
                .started_at
                .unix_millis()
                .cmp(&a.supervisor.started_at.unix_millis())
                .then_with(|| b.id.cmp(&a.id))
        });
        runs
    }

    /// The run named `id`, or the newest one in `repo` (a repository's
    /// root; `None`: in any).
    pub fn find(home: &Home, id: Option<&str>, repo: Option<&Path>) -> io::Result<Self> {
        if let Some(id) = id {
            return Self::load(home, &id.to_uppercase());
        }
        Self::all(home)
            .into_iter()
            .find(|r| r.in_repo(repo))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, no_runs(repo)))
    }

    /// Whether the run works in `repo` (`None`: any). `aver run --repo`
    /// may have named a directory under the repository's root.
    pub fn in_repo(&self, repo: Option<&Path>) -> bool {
        repo.is_none_or(|r| self.repo.starts_with(r))
    }

    /// The agent `n` names: an index (`2`) or an attempt id.
    pub fn agent(&self, n: &str) -> Option<&AgentRun> {
        match n.parse::<u32>() {
            Ok(i) => self.agents.iter().find(|a| a.index == i),
            Err(_) => self.agents.iter().find(|a| a.attempt.to_string() == n),
        }
    }

    /// Whether the supervisor process is still alive.
    pub fn supervisor_alive(&self) -> bool {
        self.finished_at.is_none() && same_process(self.supervisor.pid, self.supervisor.start)
    }
}

/// When process `pid` started, in the kernel's own units (microseconds
/// since the epoch on macOS, clock ticks since boot on Linux): a pid and
/// its start time name one process even after the pid is reused. `None`
/// when the process does not exist or the platform cannot say.
pub fn process_start(pid: u32) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: proc_pidinfo fills at most `size` bytes of `info`.
        let n = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            )
        };
        (n == size).then(|| info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
    }
    #[cfg(target_os = "linux")]
    {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // Field 22, counted after the command name, which may hold spaces.
        let rest = &stat[stat.rfind(')')? + 1..];
        if dead_state(rest) {
            return None;
        }
        rest.split_whitespace().nth(19)?.parse().ok()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = pid;
        None
    }
}

/// Whether `pid` is still the process that started at `start` (when the
/// start was recorded; otherwise whether `pid` exists).
pub fn same_process(pid: u32, start: Option<u64>) -> bool {
    match start {
        Some(start) => process_start(pid) == Some(start),
        None => process_alive(pid),
    }
}

/// Whether process `pid` exists. A process this one may not signal
/// (`EPERM`, as from inside an agent's sandbox) still exists.
pub fn process_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    let found = unsafe { libc::kill(pid as i32, 0) } == 0;
    let exists = found || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    #[cfg(target_os = "linux")]
    if exists {
        // Signal 0 succeeds on a zombie, and in a container whose PID 1
        // never reaps (a cloud VM's agent, `docker run` without `--init`)
        // a killed agent or check stays one for good.
        if let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) {
            if let Some(end) = stat.rfind(')') {
                return !dead_state(&stat[end + 1..]);
            }
        }
    }
    #[cfg(target_os = "macos")]
    if exists {
        // Signal 0 also succeeds on a killed process while it exits and
        // until launchd reaps it.
        const PROC_FLAG_INEXIT: u32 = 4;
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        // SAFETY: `info` is a writable buffer of exactly `size` bytes.
        let got = unsafe {
            libc::proc_pidinfo(
                pid as i32,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        if got == size {
            return info.pbi_status != libc::SZOMB && info.pbi_flags & PROC_FLAG_INEXIT == 0;
        }
    }
    exists
}

/// Whether `/proc/<pid>/stat` after the command name says the process has
/// exited: a zombie (`Z`) or dead (`X`) waiting to be reaped.
#[cfg(target_os = "linux")]
fn dead_state(rest: &str) -> bool {
    matches!(rest.split_whitespace().next(), Some("Z" | "X"))
}

/// `.aver/run.toml` in a workspace of a run: what the agent-facing commands
/// (the Stop hook, `aver verify`'s budget line) need to know about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunFile {
    pub run: String,
    pub index: u32,
    pub max_verifies: u32,
}

impl RunFile {
    pub const PATH: &'static str = ".aver/run.toml";

    pub fn read(dir: &Path) -> Option<Self> {
        toml::from_str(&fs::read_to_string(dir.join(Self::PATH)).ok()?).ok()
    }

    pub fn write(&self, dir: &Path) -> io::Result<()> {
        fs::write(
            dir.join(Self::PATH),
            toml::to_string(self).expect("run files serialize"),
        )
    }
}

/// `.aver/usage.json`: what a custom agent spent so far, as [`Usage`]
/// (Claude and Codex report it in their output).
pub const USAGE_PATH: &str = ".aver/usage.json";

/// `.aver/giveup`: the reason an agent gave up (`aver giveup`).
pub const GIVEUP_PATH: &str = ".aver/giveup";

pub fn gave_up(dir: &Path) -> Option<String> {
    fs::read_to_string(dir.join(GIVEUP_PATH))
        .ok()
        .map(|s| s.trim().to_owned())
}

/// `.aver/verifying` exists while `aver verify` runs in the directory, so
/// the live view can say so.
pub struct AgentMarker(PathBuf);

impl AgentMarker {
    pub const VERIFYING: &'static str = ".aver/verifying";

    pub fn verifying(dir: &Path) -> Option<Self> {
        let path = dir.join(Self::VERIFYING);
        fs::write(&path, std::process::id().to_string()).ok()?;
        Some(Self(path))
    }

    pub fn is_verifying(dir: &Path) -> bool {
        fs::read_to_string(dir.join(Self::VERIFYING))
            .ok()
            .and_then(|pid| pid.trim().parse::<u32>().ok())
            .is_some_and(process_alive)
    }
}

impl Drop for AgentMarker {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// What a command finding no run says, for `repo` or the whole home.
fn no_runs(repo: Option<&Path>) -> String {
    match repo {
        Some(r) => format!(
            "no runs in {} yet; start one with aver run (aver status --all lists every repository's)",
            r.display()
        ),
        None => "no runs yet; start one with aver run".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A child that exited but is not yet reaped is a zombie: not alive and
    /// no longer the process that started, though signal 0 still finds it.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_zombie_is_not_alive_crit_linux_green() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        let start = process_start(pid);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while process_alive(pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // SAFETY: signal 0 only probes; the child is unreaped, so its id is ours.
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, 0, "still unreaped");
        assert!(!process_alive(pid), "a zombie is not alive");
        assert!(!same_process(pid, start), "a zombie is not the process");
        assert!(!same_process(pid, None));
        child.wait().unwrap();
        assert!(!process_alive(pid));
    }

    fn id<T: std::str::FromStr>(prefix: &str) -> T
    where
        T::Err: std::fmt::Debug,
    {
        let hash = aver_core::BlobId::of_bytes(prefix.as_bytes()).to_string();
        format!("{prefix}_{}", &hash["blob_".len()..])
            .parse()
            .unwrap()
    }

    fn sample(id_: &str, agents: u32) -> RunState {
        let agent = |index| AgentRun {
            index,
            template: "custom".into(),
            attempt: AttemptId::generate(),
            agent: id("agent"),
            key: format!("k{index}"),
            workspace: WorkspaceId::generate(),
            dir: PathBuf::from("/nowhere"),
            pid: None,
            pgid: None,
            proc_start: None,
            state: AgentState::Working,
            exit: None,
            log: PathBuf::from("/nowhere/log"),
            ports: None,
            session_id: None,
            usage: Usage::default(),
            resumes: 0,
            verifies: 0,
            gave_up: None,
            last_action: None,
            started_at: Timestamp::now(),
            ended_at: None,
            feedback: None,
        };
        RunState {
            schema: RUN_SCHEMA.into(),
            id: id_.into(),
            repo: PathBuf::from("/repo"),
            base_commit: "0".repeat(40),
            base: id("snap"),
            task: id("task"),
            title: "t".into(),
            supervisor: Supervisor {
                pid: std::process::id(),
                started_at: Timestamp::now(),
                start: None,
            },
            budget: Budget {
                timeout_ms: 1,
                idle_ms: 1,
                max_verifies: 1,
                max_usd: None,
            },
            agents: (1..=agents).map(agent).collect(),
            finished_at: None,
            decided: None,
            discarded: false,
            batch: None,
            stacked_on: None,
        }
    }

    #[test]
    fn concurrent_updates_lose_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::new(dir.path());
        sample("RUNAAA", 4).save(&home).unwrap();
        std::thread::scope(|s| {
            for i in 1..=4u32 {
                let home = &home;
                s.spawn(move || {
                    for _ in 0..25 {
                        RunState::update(home, "RUNAAA", |r| {
                            r.agents.iter_mut().find(|a| a.index == i).unwrap().resumes += 1;
                            r.agents[0].verifies += 1;
                        })
                        .unwrap();
                    }
                });
            }
        });
        let run = RunState::load(&home, "RUNAAA").unwrap();
        assert!(run.agents.iter().all(|a| a.resumes == 25), "{run:?}");
        assert_eq!(run.agents[0].verifies, 100);
        assert!(atomic::leftovers(dir.path()).is_empty());
    }

    #[test]
    fn the_supervisors_save_keeps_what_others_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::new(dir.path());
        let mut supervised = sample("RUNBBB", 2);
        supervised.save(&home).unwrap();
        let file = PathBuf::from("/f.md");
        RunState::update(&home, "RUNBBB", |r| {
            r.discarded = true;
            r.agents[1].feedback = Some(AgentFeedback {
                rating: Some(9),
                file: file.clone(),
                at: Timestamp::now(),
                changed_files: false,
            });
        })
        .unwrap();
        supervised.agents[0].state = AgentState::Done;
        supervised.finished_at = Some(Timestamp::now());
        supervised.save(&home).unwrap();
        let run = RunState::load(&home, "RUNBBB").unwrap();
        assert!(run.discarded);
        assert_eq!(run.agents[0].state, AgentState::Done);
        assert_eq!(run.agents[1].feedback.as_ref().unwrap().rating, Some(9));
        assert!(run.finished_at.is_some());
    }

    #[test]
    fn a_process_is_known_by_its_start_time() {
        let me = std::process::id();
        let start = process_start(me);
        assert!(start.is_some());
        assert_eq!(process_start(me), start, "stable");
        assert!(same_process(me, start));
        assert!(
            !same_process(me, start.map(|s| s + 1)),
            "another process with this pid"
        );
        assert_eq!(process_start(u32::MAX / 2), None);
    }
}
