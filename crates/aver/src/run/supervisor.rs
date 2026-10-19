//! `aver run`'s supervisor (§2.10): prepares N workspaces on one task,
//! starts one agent in each in its own process group, watches them, gates
//! each one that stops, and ends with a ranked comparison.
//!
//! The supervisor owns processes; everything it asks of the store goes
//! through [`Api`](super::api::Api): as the admin for the task, keys and
//! materialization, as each attempt's own key for what only the
//! workspace's owner may do. Process state lives in `run.json`, which only
//! the supervisor writes while it runs.

use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use aver_core::rank::{gate, GateDecision};
use aver_core::{
    Criteria, Criterion, Set, SnapshotId, Task, TaskBody, TaskId, TaskSchema, Text, Timestamp,
    Verdict,
};
use tokio::task::JoinHandle;

use super::agent::{SpawnCtx, Template};
use super::api::{Api, ApiError, Verification};
use super::brief::{self, BriefInput, TaskSource};
use super::compare::{self, Compared};
use super::cross::{self, CrossMode};
use super::report::{agent_text, ReportInput};
use super::state::{
    self, AgentMarker, AgentRun, AgentState, Budget, RunFile, RunState, Supervisor as Sup,
};
use super::view::{Row, View};
use crate::agents::Capability;
use crate::keys::{AgentKey, Home, KeyError};
use crate::place::proc::interrupt;
use crate::workspace::MaterializeOptions;

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error("{0}")]
    Invalid(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// What to run.
#[derive(Debug, Clone)]
pub struct RunSpec {
    pub repo: PathBuf,
    pub rev: String,
    /// The task as the user wrote it.
    pub prompt: String,
    pub title: Option<String>,
    /// `(id, text)` pairs; none makes a regression-only task.
    pub criteria: Vec<(String, String)>,
    /// Paths the agents may not change (the task's `protected_paths`).
    pub protected: Vec<aver_core::PathGlob>,
    /// One template per agent.
    pub agents: Vec<Template>,
    pub budget: Budget,
    pub deps: bool,
    pub git_view: bool,
    /// The `aver` binary agents and hooks call.
    pub aver_bin: PathBuf,
    /// Ports reserved per agent.
    pub ports: u16,
    /// Paths every agent of the run leases exclusively (`--lease`).
    pub leases: Vec<aver_core::PathGlob>,
    /// The run id, when the caller chose it (`aver run --detach`, a batch).
    pub id: Option<String>,
    /// The batch the run is one task of.
    pub batch: Option<state::BatchRef>,
    /// The titles of the batch's other tasks, which the brief names.
    pub siblings: Vec<String>,
    /// What the run builds on when `rev` is other tasks' accepted work;
    /// copied into the run file.
    pub stacked_on: Option<state::Stack>,
    /// More for the brief after the task (a retry's previous attempt, what
    /// a rerun builds on), which is not part of the task's intent.
    pub context: Option<String>,
}

/// Who wrote the run's task: a batch task of an issue was its author.
fn source_of(spec: &RunSpec) -> TaskSource {
    match spec.batch.as_ref().and_then(|b| b.issue) {
        Some(number) => TaskSource::Issue { number },
        None => TaskSource::Operator,
    }
}

pub struct RunOutcome {
    pub state: RunState,
    pub compared: Vec<Compared>,
}

impl RunOutcome {
    /// Whether some attempt passed.
    pub fn any_pass(&self) -> bool {
        self.compared
            .iter()
            .any(|c| c.verdict == Some(Verdict::Pass) && c.diff.is_some_and(|d| d.0 > 0))
    }
}

/// What a check after an agent stopped found.
struct Checked {
    verdict: Option<Verdict>,
    proven: u32,
    criteria: u32,
    withheld: bool,
    verifies: u32,
    report: Option<String>,
    error: Option<String>,
    /// The directory's snapshot when the agent exited, and its attempt's
    /// base: what the next exit is compared with.
    snapshot: Option<SnapshotId>,
    base: Option<SnapshotId>,
}

/// What a periodic look at a working agent found.
#[derive(Default)]
struct Polled {
    notes: Option<Vec<String>>,
    diff: Option<(u32, u32, u32)>,
    verifies: Option<u32>,
    latest: Option<(Verdict, u32, u32)>,
}

/// The supervisor's side of one agent.
struct Live {
    template: Template,
    key: AgentKey,
    brief: String,
    claude_settings: Option<PathBuf>,
    child: Option<Child>,
    log_offset: u64,
    partial: String,
    last_output: Instant,
    started: Instant,
    term: Option<(AgentState, Instant)>,
    check: Option<JoinHandle<Checked>>,
    poll: Option<JoinHandle<Polled>>,
    last_poll: Instant,
    usage_read: Instant,
    files: (u32, u32, u32),
    verdict: Option<Verdict>,
    proven: u32,
    criteria: u32,
    verifies: u32,
    seen_verifies: u32,
    notes: Vec<String>,
    /// The directory's snapshot at its last exit.
    exited_at: Option<SnapshotId>,
    /// Exits in a row that changed no file.
    idle_exits: u32,
}

/// An agent that exits this many times in a row without changing a file is
/// not resumed again: another resume would only spend its budget.
const MAX_IDLE_EXITS: u32 = 2;

/// How often a custom agent's `.aver/usage.json` is read.
const USAGE_EVERY: Duration = Duration::from_secs(1);
/// How often a working agent's diff and receipts are read.
const POLL_EVERY: Duration = Duration::from_secs(3);
/// How long a process group gets between SIGTERM and SIGKILL.
const KILL_GRACE: Duration = Duration::from_secs(10);

pub struct Supervisor<'a> {
    home: Home,
    api: Api,
    admin: AgentKey,
    rt: tokio::runtime::Runtime,
    view: &'a mut dyn View,
    stop: &'a AtomicBool,
    cross: CrossMode,
    after_first_save: Option<AfterSave<'a>>,
}

/// Called once `run.json` first exists, before any agent starts.
pub type AfterSave<'a> = Box<dyn FnOnce(&RunState) -> std::io::Result<()> + 'a>;

fn task_for(admin: &AgentKey, base: SnapshotId, spec: &RunSpec) -> Result<Task, RunError> {
    let invalid = |e: aver_core::ParseError| RunError::Invalid(e.to_string());
    let criteria = Criteria::new(
        spec.criteria
            .iter()
            .map(|(id, text)| {
                Ok(Criterion {
                    id: id.parse().map_err(invalid)?,
                    text: text.clone(),
                })
            })
            .collect::<Result<_, RunError>>()?,
    )
    .map_err(invalid)?;
    let protected = Set::new(spec.protected.clone()).map_err(invalid)?;
    let first_line = spec.prompt.lines().next().unwrap_or("").trim();
    let title = spec.title.as_deref().unwrap_or(first_line);
    let body = TaskBody {
        schema: TaskSchema::for_task(&criteria, &protected),
        title: Text::truncated(title),
        intent: Text::truncated(spec.prompt.trim()),
        base,
        target: None,
        criteria,
        checks: Set::empty(),
        protected_paths: protected,
        author: admin.id(),
        created_at: Timestamp::now(),
    };
    Ok(Task::sign(body, &admin.signing_key))
}

/// Whether the repository tracks `path` at `commit`.
fn tracked(repo: &Path, commit: &str, path: &str) -> bool {
    Command::new("git")
        .args(["ls-tree", "--name-only", commit, "--", path])
        .current_dir(repo)
        .output()
        .is_ok_and(|o| o.status.success() && !o.stdout.is_empty())
}

/// Whether the repository's working tree has uncommitted changes to
/// tracked files.
fn dirty(repo: &Path) -> bool {
    Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(repo)
        .output()
        .is_ok_and(|o| o.status.success() && !o.stdout.is_empty())
}

fn signal_group(pgid: Option<i32>, signal: i32) {
    if let Some(pgid) = pgid.filter(|p| *p > 1) {
        // SAFETY: signals a process group the supervisor created.
        unsafe { libc::kill(-pgid, signal) };
    }
}

impl<'a> Supervisor<'a> {
    pub fn new(
        home: &Home,
        api: Api,
        admin: AgentKey,
        view: &'a mut dyn View,
        stop: &'a AtomicBool,
    ) -> Result<Self, RunError> {
        Ok(Self {
            home: home.clone(),
            api,
            admin,
            rt: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?,
            view,
            stop,
            cross: CrossMode::Off,
            after_first_save: None,
        })
    }

    /// Calls `f` once the run's `run.json` exists, before its agents start:
    /// `aver rerun` points the batch at the new run only then, so the batch
    /// never names a run that has no file.
    pub fn after_first_save(mut self, f: AfterSave<'a>) -> Self {
        self.after_first_save = Some(f);
        self
    }

    /// Whether the run ends with a cross-check of its top attempts (§2.10);
    /// off unless set.
    pub fn cross(mut self, mode: CrossMode) -> Self {
        self.cross = mode;
        self
    }

    /// Runs `spec` to the end and returns the run and its comparison.
    pub fn run(mut self, spec: RunSpec) -> Result<RunOutcome, RunError> {
        if spec.agents.is_empty() {
            return Err(RunError::Invalid("a run needs at least one agent".into()));
        }
        let (mut run, mut lives) = self.prepare(&spec)?;
        run.save(&self.home)?;
        if let Some(f) = self.after_first_save.take() {
            f(&run)?;
        }
        for (agent, live) in run.agents.iter_mut().zip(lives.iter_mut()) {
            match self.start(agent, live, &spec, None) {
                Ok(()) => self.view.event(agent.index, &agent.template, "started"),
                Err(e) => {
                    agent.state = AgentState::Failed;
                    agent.last_action = Some(format!("could not start: {e}"));
                    agent.ended_at = Some(Timestamp::now());
                    self.view.event(
                        agent.index,
                        &agent.template,
                        &format!("failed to start: {e}"),
                    );
                }
            }
        }
        run.save(&self.home)?;
        self.watch(&mut run, &mut lives, &spec)?;
        run.finished_at = Some(Timestamp::now());
        for agent in &mut run.agents {
            agent.pid = None;
        }
        run.save(&self.home)?;
        let mut compared = self.rt.block_on(compare::gather(
            &self.api,
            &self.home,
            &self.admin.signing_key,
            &run,
        ));
        if self.cross == CrossMode::Auto && cross::eligible(&compared).len() >= 2 {
            self.view
                .note("cross-checking: each top attempt's tests on the others' code");
            let report = self.rt.block_on(cross::run(
                &self.api,
                &self.admin.signing_key,
                &run.task,
                &compared,
            ));
            report.write(&self.home, &run.id)?;
            compare::apply_cross(&mut compared, &report);
        }
        let rows = rows(&run, &lives);
        self.view.finish(&rows);
        Ok(RunOutcome {
            state: run,
            compared,
        })
    }

    fn prepare(&mut self, spec: &RunSpec) -> Result<(RunState, Vec<Live>), RunError> {
        let id = spec.id.clone().unwrap_or_else(state::new_run_id);
        let started = Instant::now();
        let admin = self.admin.signing_key.clone();
        let imported = self
            .rt
            .block_on(self.api.import(&admin, &spec.repo, &spec.rev))?;
        let commit = imported.commit.to_string();
        let short = &commit[..7.min(commit.len())];
        let mut header = vec![format!(
            "aver: imported {} {short} ({} files, {:.2} s)",
            spec.rev,
            imported.files,
            started.elapsed().as_secs_f64()
        )];
        header.extend(crate::git::secrets_warning(&imported.secrets));
        if dirty(&spec.repo) {
            header.push(format!(
                "aver: warning: {} has uncommitted changes; agents start from the commit {short}, without them",
                super::view::display_path(&spec.repo)
            ));
        }
        if spec.criteria.is_empty() {
            header.push(
                "aver: no criteria, so PASS only means nothing broke; add -c \"id: text\" to say what done means"
                    .into(),
            );
        }
        let task = task_for(&self.admin, imported.snapshot, spec)?;
        let task_id = self.rt.block_on(self.api.create_task(&task))?;
        let run_dir = state::run_dir(&self.home, &id);
        fs::create_dir_all(&run_dir)?;
        let settings_tracked = tracked(&spec.repo, &commit, brief::CLAUDE_SETTINGS);
        self.view.header(&header);
        let prepared_at = Instant::now();
        let mut agents = Vec::new();
        let mut lives = Vec::new();
        let n = spec.agents.len() as u32;
        for (i, template) in spec.agents.iter().enumerate() {
            let index = i as u32 + 1;
            let name = format!("run-{}-{index}", id.to_lowercase());
            let key = AgentKey::generate(
                name.parse()
                    .map_err(|e| RunError::Invalid(format!("{e}")))?,
            );
            key.write(&self.home.key_path(&key.name))?;
            self.rt.block_on(self.api.register_agent(
                &admin,
                &name,
                &key.to_file().public_key,
                &[
                    Capability::TaskClaim,
                    Capability::WorkspaceWrite,
                    Capability::VerifyRun,
                    Capability::LeaseWrite,
                ],
            ))?;
            let claim = self.rt.block_on(self.api.claim(
                &key.signing_key,
                &task_id,
                Some(imported.snapshot),
            ))?;
            let claude = matches!(template, Template::Claude { .. });
            let reserve = if claude && !settings_tracked {
                vec![brief::CLAUDE_SETTINGS.to_owned()]
            } else {
                Vec::new()
            };
            let m = self.rt.block_on(self.api.materialize(
                &claim.workspace,
                &MaterializeOptions {
                    repo: Some(spec.repo.clone()),
                    label: Some(format!("{id}-{index}")),
                    deps: spec.deps,
                    key: Some(name.clone()),
                    git_view: spec.git_view,
                    ports: Some(spec.ports),
                    reserve,
                },
            ))?;
            // The base side of every verification of this run is the same,
            // so it runs once now, while the agents start, instead of inside
            // the first verify; after the first materialization, which names
            // the repository whose base slot the verifies use. A warm-up
            // that cannot start only makes that verify slower.
            if index == 1 {
                let _ = self.api.warm(&spec.aver_bin, &task_id);
            }
            if !spec.leases.is_empty() {
                self.rt.block_on(self.api.acquire_lease(
                    &key.signing_key,
                    &claim.workspace,
                    &crate::coord::LeaseRequest {
                        paths: spec.leases.clone(),
                        mode: crate::coord::LeaseMode::Exclusive,
                        reason: format!("reserved by aver run {id}"),
                        ttl_ms: None,
                    },
                ))?;
            }
            let others = self
                .rt
                .block_on(self.api.notices(&key.signing_key, &claim.workspace))
                .map(|n| crate::coord::agent_text(&n, true))
                .unwrap_or_default();
            let families = crate::languages::families_in(&m.dir);
            let input = BriefInput {
                dir: &m.dir,
                prompt: &spec.prompt,
                criteria: &spec.criteria,
                protected: &spec.protected,
                max_verifies: spec.budget.max_verifies,
                ports: m.ports,
                sandboxed: matches!(template, Template::Codex { .. }),
                others: Some(&others),
                siblings: &spec.siblings,
                families: &families,
                context: spec.context.as_deref(),
                usage_file: matches!(template, Template::Custom { .. }),
            };
            let source = source_of(spec);
            let text = brief::render_for(&input, source);
            // The saved copy leaves out what other tasks did at start: `aver
            // brief` prints it again with what they do now.
            fs::write(
                m.dir.join(".aver/BRIEF.md"),
                brief::render_for(
                    &BriefInput {
                        others: None,
                        ..input
                    },
                    source,
                ),
            )?;
            RunFile {
                run: id.clone(),
                index,
                max_verifies: spec.budget.max_verifies,
            }
            .write(&m.dir)?;
            let claude_settings = if claude {
                let settings = brief::claude_settings(
                    &spec.aver_bin,
                    fs::read(m.dir.join("package.json")).ok().as_deref(),
                );
                let bytes = serde_json::to_vec_pretty(&settings).expect("settings serialize");
                if settings_tracked {
                    let path = m.dir.join(brief::CLAUDE_SETTINGS_FALLBACK);
                    fs::write(&path, bytes)?;
                    Some(path)
                } else {
                    let path = m.dir.join(brief::CLAUDE_SETTINGS);
                    fs::create_dir_all(path.parent().expect("has a parent"))?;
                    fs::write(&path, bytes)?;
                    None
                }
            } else {
                None
            };
            if index == 1 {
                self.view.header(&[format!(
                    "aver: workspace 1 ready in {:.2} s ({}{})",
                    prepared_at.elapsed().as_secs_f64(),
                    m.clone.as_str(),
                    if m.git_view { ", git view" } else { "" },
                )]);
            }
            agents.push(AgentRun {
                index,
                template: template.name().to_owned(),
                attempt: claim.attempt,
                agent: key.id(),
                key: name,
                workspace: claim.workspace,
                dir: m.dir.clone(),
                pid: None,
                pgid: None,
                proc_start: None,
                state: AgentState::Preparing,
                exit: None,
                log: run_dir.join(format!("agent-{index}.log")),
                ports: m.ports,
                session_id: template.new_session_id(),
                usage: Default::default(),
                resumes: 0,
                verifies: 0,
                gave_up: None,
                last_action: None,
                started_at: Timestamp::now(),
                ended_at: None,
                feedback: None,
            });
            lives.push(Live {
                template: template.clone(),
                key,
                brief: text,
                claude_settings,
                child: None,
                log_offset: 0,
                partial: String::new(),
                last_output: Instant::now(),
                started: Instant::now(),
                term: None,
                check: None,
                poll: None,
                last_poll: Instant::now(),
                usage_read: Instant::now(),
                files: (0, 0, 0),
                verdict: None,
                proven: 0,
                criteria: spec.criteria.len() as u32,
                verifies: 0,
                seen_verifies: 0,
                notes: Vec::new(),
                exited_at: None,
                idle_exits: 0,
            });
        }
        let mut header = vec![format!(
            "aver: run {id} · task {task_id} · {n} {}",
            if n == 1 { "agent" } else { "agents" }
        )];
        header.extend(
            agents
                .iter()
                .map(|a| format!("  #{} {}", a.index, super::view::display_path(&a.dir))),
        );
        header.push(format!(
            "aver: logs in {}",
            super::view::display_path(&run_dir)
        ));
        self.view.header(&header);
        let run = RunState {
            schema: state::RUN_SCHEMA.into(),
            id,
            repo: spec.repo.clone(),
            base_commit: commit,
            base: imported.snapshot,
            task: task_id,
            title: task.body.title.to_string(),
            supervisor: Sup {
                pid: std::process::id(),
                started_at: Timestamp::now(),
                start: state::process_start(std::process::id()),
            },
            budget: spec.budget.clone(),
            agents,
            finished_at: None,
            decided: None,
            discarded: false,
            batch: spec.batch.clone(),
            stacked_on: spec.stacked_on.clone(),
        };
        Ok((run, lives))
    }

    /// Starts (or, with `feedback`, resumes) the agent's process.
    fn start(
        &self,
        agent: &mut AgentRun,
        live: &mut Live,
        spec: &RunSpec,
        feedback: Option<&str>,
    ) -> Result<(), RunError> {
        let brief_file = agent.dir.join(".aver/BRIEF.md");
        let prompt = brief::agent_prompt(source_of(spec), &spec.prompt);
        let ctx = SpawnCtx {
            dir: &agent.dir,
            brief: &live.brief,
            brief_file: &brief_file,
            prompt: &prompt,
            session_id: agent.session_id.as_deref(),
            claude_settings: live.claude_settings.as_deref(),
        };
        let argv = match feedback {
            None => live.template.spawn_argv(&ctx),
            Some(f) => live
                .template
                .resume_argv(&ctx, f)
                .ok_or_else(|| RunError::Invalid("this agent cannot be resumed".into()))?,
        };
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&agent.log)?;
        let err = log.try_clone()?;
        let path = match (spec.aver_bin.parent(), std::env::var_os("PATH")) {
            (Some(dir), Some(path)) => {
                let mut dirs = vec![dir.to_path_buf()];
                dirs.extend(std::env::split_paths(&path));
                std::env::join_paths(dirs).unwrap_or(path)
            }
            (_, path) => path.unwrap_or_default(),
        };
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .current_dir(&agent.dir)
            .env("PATH", path)
            .env("AVER_HOME", self.home.root())
            .env("AVER_RUN", "1")
            .env("AVER_WORKSPACE", agent.workspace.to_string())
            .env("AVER_ATTEMPT", agent.attempt.to_string())
            .env("AVER_AGENT_INDEX", agent.index.to_string())
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(err);
        if let Some(p) = agent.ports {
            cmd.env("PORT", p.base.to_string());
        }
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        // Held by the Ctrl-C handler, so a second Ctrl-C kills the agent's
        // group (one that ignores SIGTERM too) before `aver` exits. With
        // every slot taken it is still stopped the graceful way.
        let claim = interrupt::LIVE.claim();
        let child = cmd
            .spawn()
            .map_err(|e| RunError::Invalid(format!("cannot start {}: {e}", argv[0])))?;
        if let Some(claim) = claim {
            claim.set(child.id() as i32);
        }
        agent.pid = Some(child.id());
        agent.pgid = Some(child.id() as i32);
        agent.proc_start = state::process_start(child.id());
        agent.state = AgentState::Working;
        agent.exit = None;
        live.child = Some(child);
        live.last_output = Instant::now();
        live.last_poll = Instant::now();
        if feedback.is_none() {
            live.started = Instant::now();
            agent.started_at = Timestamp::now();
        }
        Ok(())
    }

    fn watch(
        &mut self,
        run: &mut RunState,
        lives: &mut [Live],
        spec: &RunSpec,
    ) -> Result<(), RunError> {
        let mut last_draw = Instant::now() - Duration::from_secs(1);
        let mut last_save = Instant::now();
        let mut stopping = false;
        let mut events: VecDeque<(u32, String, String)> = VecDeque::new();
        let task = run.task;
        loop {
            if self.stop.load(Ordering::SeqCst) && !stopping {
                stopping = true;
                for (agent, live) in run.agents.iter_mut().zip(lives.iter_mut()) {
                    if agent.state.is_terminal() {
                        continue;
                    }
                    signal_group(agent.pgid, libc::SIGTERM);
                    live.term = Some((AgentState::Killed, Instant::now() + KILL_GRACE));
                    if let Some(h) = live.check.take() {
                        h.abort();
                    }
                    if live.child.is_none() {
                        agent.state = AgentState::Killed;
                        agent.ended_at = Some(Timestamp::now());
                    }
                }
            }
            let mut changed = false;
            for (agent, live) in run.agents.iter_mut().zip(lives.iter_mut()) {
                changed |= self.read_log(agent, live);
                if live.usage_read.elapsed() >= USAGE_EVERY {
                    live.usage_read = Instant::now();
                    changed |= read_usage_file(agent, live);
                }
                if agent.state.is_terminal() {
                    continue;
                }
                if let Some(child) = live.child.as_mut() {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            live.child = None;
                            if let Some(pgid) = agent.pgid {
                                interrupt::LIVE.clear(pgid);
                            }
                            read_usage_file(agent, live);
                            agent.exit = status.code();
                            agent.pid = None;
                            if let Some(h) = live.poll.take() {
                                h.abort();
                            }
                            changed = true;
                            events.push_back((
                                agent.index,
                                agent.template.clone(),
                                match status.code() {
                                    Some(code) => format!("exited {code}"),
                                    None => "exited on a signal".into(),
                                },
                            ));
                            match live.term {
                                Some((AgentState::Killed, _)) => {
                                    agent.state = AgentState::Killed;
                                    agent.ended_at = Some(Timestamp::now());
                                }
                                _ => {
                                    agent.state = AgentState::Checking;
                                    live.check = Some(self.spawn_check(task, agent, live, spec));
                                }
                            }
                        }
                        Ok(None) => {
                            let verifying = AgentMarker::is_verifying(&agent.dir);
                            let state = if verifying {
                                AgentState::Verifying
                            } else {
                                AgentState::Working
                            };
                            if state != agent.state {
                                agent.state = state;
                                changed = true;
                            }
                            if verifying {
                                live.last_output = Instant::now();
                            }
                            let now = Instant::now();
                            match live.term {
                                Some((_, kill_at)) if now >= kill_at => {
                                    signal_group(agent.pgid, libc::SIGKILL);
                                }
                                Some(_) => {}
                                None => {
                                    let wall = now.duration_since(live.started).as_millis() as u64;
                                    let idle =
                                        now.duration_since(live.last_output).as_millis() as u64;
                                    if wall >= spec.budget.timeout_ms || idle >= spec.budget.idle_ms
                                    {
                                        let why = if wall >= spec.budget.timeout_ms {
                                            format!(
                                                "timed out after {}",
                                                super::view::elapsed(wall)
                                            )
                                        } else {
                                            format!(
                                                "idle for {}; stopping it",
                                                super::view::elapsed(idle)
                                            )
                                        };
                                        events.push_back((
                                            agent.index,
                                            agent.template.clone(),
                                            why,
                                        ));
                                        signal_group(agent.pgid, libc::SIGTERM);
                                        live.term = Some((AgentState::Timeout, now + KILL_GRACE));
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            agent.state = AgentState::Failed;
                            agent.last_action = Some(e.to_string());
                            agent.ended_at = Some(Timestamp::now());
                            live.child = None;
                            if let Some(pgid) = agent.pgid {
                                interrupt::LIVE.clear(pgid);
                            }
                        }
                    }
                }
                if live.check.as_ref().is_some_and(|h| h.is_finished()) {
                    let handle = live.check.take().expect("checked above");
                    let checked = match self.rt.block_on(handle) {
                        Ok(c) => c,
                        Err(e) => Checked {
                            verdict: None,
                            proven: 0,
                            criteria: 0,
                            withheld: false,
                            verifies: live.verifies,
                            report: None,
                            error: Some(e.to_string()),
                            snapshot: None,
                            base: None,
                        },
                    };
                    changed = true;
                    self.after_check(agent, live, checked, spec, &mut events);
                }
                if matches!(agent.state, AgentState::Working | AgentState::Verifying) {
                    if live.poll.as_ref().is_some_and(|h| h.is_finished()) {
                        let handle = live.poll.take().expect("checked above");
                        if let Ok(p) = self.rt.block_on(handle) {
                            changed |= apply_poll(agent, live, p, &mut events);
                        }
                    } else if live.poll.is_none() && live.last_poll.elapsed() >= POLL_EVERY {
                        live.last_poll = Instant::now();
                        live.poll = Some(self.spawn_poll(agent, live));
                    }
                }
            }
            while let Some((i, t, what)) = events.pop_front() {
                self.view.event(i, &t, &what);
            }
            let done = run.agents.iter().all(|a| a.state.is_terminal());
            if changed && last_save.elapsed() >= Duration::from_millis(500) || done {
                run.save(&self.home)?;
                last_save = Instant::now();
            }
            if done {
                return Ok(());
            }
            if last_draw.elapsed() >= Duration::from_millis(200) {
                self.view.update(&rows(run, lives));
                last_draw = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Reads new output from the agent's log; returns whether it changed
    /// anything the view shows.
    fn read_log(&self, agent: &mut AgentRun, live: &mut Live) -> bool {
        let Ok(mut file) = fs::File::open(&agent.log) else {
            return false;
        };
        let Ok(len) = file.metadata().map(|m| m.len()) else {
            return false;
        };
        if len <= live.log_offset {
            return false;
        }
        let mut buf = Vec::new();
        if file.seek(SeekFrom::Start(live.log_offset)).is_err()
            || file.take(1 << 20).read_to_end(&mut buf).is_err()
        {
            return false;
        }
        live.log_offset += buf.len() as u64;
        live.last_output = Instant::now();
        live.partial.push_str(&String::from_utf8_lossy(&buf));
        let mut changed = false;
        while let Some(nl) = live.partial.find('\n') {
            let line: String = live.partial.drain(..=nl).collect();
            let event = live.template.parse_line(&line);
            if let Some(action) = event.action.filter(|a| !a.is_empty()) {
                agent.last_action = Some(action);
                changed = true;
            }
            if event.session_id.is_some() && agent.session_id.is_none() {
                agent.session_id = event.session_id;
            }
            if let Some(usage) = event.usage {
                // Each Claude turn reports its own cost; a run sums them.
                let u = &mut agent.usage;
                u.cost_usd = match (u.cost_usd, usage.cost_usd) {
                    (Some(a), Some(b)) => Some(a + b),
                    (a, b) => a.or(b),
                };
                u.input_tokens = sum(u.input_tokens, usage.input_tokens);
                u.output_tokens = sum(u.output_tokens, usage.output_tokens);
                changed = true;
            }
        }
        changed
    }

    fn spawn_poll(&self, agent: &AgentRun, live: &Live) -> JoinHandle<Polled> {
        let api = self.api.clone();
        let owner = live.key.signing_key.clone();
        let admin = self.admin.signing_key.clone();
        let (ws, att) = (agent.workspace, agent.attempt);
        let seen = live.seen_verifies;
        self.rt.spawn(async move {
            let mut p = Polled::default();
            if let Ok(files) = api.diff(&owner, &ws).await {
                let (mut a, mut d) = (0, 0);
                for f in &files {
                    let (x, y) = f.lines();
                    a += x;
                    d += y;
                }
                p.diff = Some((files.len() as u32, a, d));
            }
            if let Ok(n) = api.notices(&owner, &ws).await {
                p.notes = Some(notes(&n));
            }
            if let Ok(attempt) = api.attempt(&admin, &att).await {
                let n = attempt.receipts.len() as u32;
                p.verifies = Some(n);
                if n != seen {
                    if let Some(id) = attempt.receipts.last() {
                        if let Ok(v) = api.receipt(id).await {
                            p.latest = Some(proof(&v));
                        }
                    }
                }
            }
            p
        })
    }

    fn spawn_check(
        &self,
        task: TaskId,
        agent: &AgentRun,
        live: &Live,
        spec: &RunSpec,
    ) -> JoinHandle<Checked> {
        let api = self.api.clone();
        let owner = live.key.signing_key.clone();
        let admin = self.admin.signing_key.clone();
        let (ws, att) = (agent.workspace, agent.attempt);
        let max = spec.budget.max_verifies;
        self.rt
            .spawn(async move { check(api, owner, admin, task, ws, att, max).await })
    }

    fn after_check(
        &self,
        agent: &mut AgentRun,
        live: &mut Live,
        c: Checked,
        spec: &RunSpec,
        events: &mut VecDeque<(u32, String, String)>,
    ) {
        live.verifies = c.verifies;
        live.seen_verifies = c.verifies;
        agent.verifies = c.verifies;
        if c.verdict.is_some() {
            live.verdict = c.verdict;
            live.proven = c.proven;
            live.criteria = c.criteria;
        }
        let t = agent.template.clone();
        if let Some(e) = &c.error {
            events.push_back((agent.index, t.clone(), format!("could not verify: {e}")));
        } else if let Some(v) = c.verdict {
            events.push_back((
                agent.index,
                t.clone(),
                format!("verified {}{}", v.as_str(), crit(c.proven, c.criteria)),
            ));
        }
        agent.gave_up = state::gave_up(&agent.dir);
        if let Some((AgentState::Timeout, _)) = live.term {
            agent.state = AgentState::Timeout;
            agent.last_action = Some("timed out".into());
            agent.ended_at = Some(Timestamp::now());
            return;
        }
        // An unchanged directory is never done (the verify report says
        // so), whatever its verdict.
        let unchanged = c.snapshot.is_some() && c.snapshot == c.base;
        let latest = match c.verdict {
            Some(Verdict::Pass | Verdict::Review) if c.withheld || unchanged => {
                Some(Verdict::Unproven)
            }
            other => other,
        };
        let decision = if c.error.is_some() {
            GateDecision::BudgetExhausted
        } else {
            gate(
                latest,
                c.verifies,
                spec.budget.max_verifies,
                agent.gave_up.is_some(),
            )
        };
        let before = live.exited_at.or(c.base);
        live.idle_exits = match c.snapshot {
            Some(s) if Some(s) == before => live.idle_exits + 1,
            _ => 0,
        };
        live.exited_at = c.snapshot.or(live.exited_at);
        let idle = live.idle_exits >= MAX_IDLE_EXITS;
        let resumable =
            live.template.can_resume() && agent.resumes < spec.budget.max_verifies && !idle;
        match decision {
            GateDecision::Continue if resumable && !self.stop.load(Ordering::SeqCst) => {
                let feedback = c
                    .report
                    .map(|r| super::report::note_once(&agent.dir, r))
                    .unwrap_or_else(|| {
                        "aver verify did not pass yet. Run aver verify and fix what it reports."
                            .into()
                    });
                agent.resumes += 1;
                match self.start(agent, live, spec, Some(&feedback)) {
                    Ok(()) => events.push_back((
                        agent.index,
                        t,
                        format!("resumed with the report ({})", agent.resumes),
                    )),
                    Err(e) => {
                        agent.state = AgentState::Failed;
                        agent.ended_at = Some(Timestamp::now());
                        events.push_back((agent.index, t, format!("could not resume: {e}")));
                    }
                }
            }
            other => {
                agent.state = match other {
                    GateDecision::Done => AgentState::Done,
                    GateDecision::GiveUp => AgentState::GaveUp,
                    GateDecision::BudgetExhausted if c.error.is_none() => AgentState::OutOfVerifies,
                    GateDecision::Continue if c.error.is_none() && idle => AgentState::Idle,
                    GateDecision::Continue if c.error.is_none() => AgentState::Stopped,
                    _ => AgentState::Failed,
                };
                agent.ended_at = Some(Timestamp::now());
                let verdict = match c.verdict {
                    Some(_) if unchanged => ": changed nothing".to_owned(),
                    Some(v) => format!(": {}{}", v.as_str(), crit(c.proven, c.criteria)),
                    None => String::new(),
                };
                let what = match agent.state {
                    AgentState::Done => format!(
                        "done: {}{}",
                        c.verdict.map_or("PASS", |v| v.as_str()),
                        crit(c.proven, c.criteria)
                    ),
                    AgentState::GaveUp => format!(
                        "gave up: {}",
                        super::report::one_line_plain(agent.gave_up.as_deref().unwrap_or(""))
                    ),
                    AgentState::OutOfVerifies => format!("out of verifies{verdict}"),
                    AgentState::Stopped => format!("stopped, not done{verdict}"),
                    AgentState::Idle => format!(
                        "stopped: exited {} times in a row without changing a file{verdict}",
                        live.idle_exits
                    ),
                    _ => "failed".into(),
                };
                agent.last_action = Some(what.clone());
                events.push_back((agent.index, t, what));
            }
        }
    }
}

/// A custom agent reports what it spent so far in `.aver/usage.json`
/// (`{"cost_usd": …, "input_tokens": …, "output_tokens": …}`); Claude and
/// Codex report it in their output instead. Returns whether it changed.
fn read_usage_file(agent: &mut AgentRun, live: &Live) -> bool {
    if !matches!(live.template, Template::Custom { .. }) {
        return false;
    }
    let Ok(bytes) = fs::read(agent.dir.join(state::USAGE_PATH)) else {
        return false;
    };
    match serde_json::from_slice::<state::Usage>(&bytes) {
        Ok(usage) if usage != agent.usage => {
            agent.usage = usage;
            true
        }
        _ => false,
    }
}

fn sum(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
    }
}

fn crit(proven: u32, criteria: u32) -> String {
    if criteria == 0 {
        String::new()
    } else {
        format!(" {proven}/{criteria}")
    }
}

/// A receipt's verdict, proven criteria and criteria.
fn proof(v: &Verification) -> (Verdict, u32, u32) {
    let body = &v.receipt.body;
    let criteria = body.criteria.as_slice().len() as u32;
    (
        body.verdict,
        criteria - body.unproven.len() as u32,
        criteria,
    )
}

fn apply_poll(
    agent: &mut AgentRun,
    live: &mut Live,
    p: Polled,
    events: &mut VecDeque<(u32, String, String)>,
) -> bool {
    let mut changed = false;
    if let Some(notes) = p.notes {
        for note in notes.iter().filter(|n| !live.notes.contains(n)) {
            events.push_back((
                agent.index,
                agent.template.clone(),
                format!("other task: {note}"),
            ));
        }
        changed |= live.notes != notes;
        live.notes = notes;
    }
    if let Some(d) = p.diff {
        changed |= live.files != d;
        live.files = d;
    }
    if let Some(n) = p.verifies {
        if let Some((v, proven, criteria)) = p.latest {
            if n != live.seen_verifies {
                events.push_back((
                    agent.index,
                    agent.template.clone(),
                    format!("verify {n}: {}{}", v.as_str(), crit(proven, criteria)),
                ));
            }
            live.verdict = Some(v);
            live.proven = proven;
            live.criteria = criteria;
            changed = true;
        }
        live.verifies = n;
        live.seen_verifies = n;
        agent.verifies = n;
    }
    changed
}

/// After an agent stopped: sync, verify unless its latest receipt already
/// covers the directory, and render the report the gate resumes it with.
async fn check(
    api: Api,
    owner: aver_core::SigningKey,
    admin: aver_core::SigningKey,
    task: TaskId,
    ws: aver_core::WorkspaceId,
    att: aver_core::AttemptId,
    max_verifies: u32,
) -> Checked {
    let failed = |e: ApiError, verifies: u32| Checked {
        verdict: None,
        proven: 0,
        criteria: 0,
        withheld: false,
        verifies,
        report: None,
        error: Some(e.to_string()),
        snapshot: None,
        base: None,
    };
    let sync = match api.sync(&owner, &ws).await {
        Ok(s) => s,
        Err(e) => return failed(e, 0),
    };
    let attempt = match api.attempt(&admin, &att).await {
        Ok(a) => a,
        Err(e) => return failed(e, 0),
    };
    let mut verifies = attempt.receipts.len() as u32;
    let current = match api.snapshot(&owner, &ws).await {
        Ok(s) => s,
        Err(e) => return failed(e, verifies),
    };
    let reuse = match attempt.receipts.last() {
        Some(id) => match api.receipt(id).await {
            Ok(v) if v.receipt.body.head_snapshot == current => Some(v),
            _ => None,
        },
        None => None,
    };
    let (v, sync) = match reuse {
        Some(v) => (v, sync),
        None => match api.verify(&owner, &task, &ws, false).await {
            Ok(v) => {
                verifies += 1;
                let sync = v.sync.clone().unwrap_or(sync);
                (v, sync)
            }
            Err(e) => return failed(e, verifies),
        },
    };
    let input = ReportInput {
        receipt_id: &v.receipt_id,
        receipt: &v.receipt,
        diagnostics: v.diagnostics.as_ref(),
        sync: Some(&sync),
        took: None,
        budget: Some((verifies, max_verifies)),
        notices: v.notices.as_ref(),
        timing: v.timing.as_ref(),
    };
    let (verdict, proven, criteria) = proof(&v);
    Checked {
        verdict: Some(verdict),
        proven,
        criteria,
        withheld: sync.skipped.iter().any(|s| s.tracked)
            || !crate::run::report::leased(&v.receipt.body.checks).is_empty(),
        verifies,
        report: Some(agent_text(&input)),
        error: None,
        snapshot: Some(current),
        base: Some(attempt.base),
    }
}

fn rows(run: &RunState, lives: &[Live]) -> Vec<Row> {
    run.agents
        .iter()
        .zip(lives)
        .map(|(a, l)| Row {
            index: a.index,
            template: a.template.clone(),
            state: a.state,
            action: a.last_action.clone().unwrap_or_default(),
            files: l.files.0,
            added: l.files.1,
            removed: l.files.2,
            verdict: l.verdict,
            proven: l.proven,
            criteria: l.criteria,
            verifies: l.verifies,
            notes: l.notes.clone(),
        })
        .collect()
}

/// Notices as short lines for the live view (a human's terminal).
fn notes(n: &crate::coord::Notices) -> Vec<String> {
    use crate::coord::Overlap;
    let name = |title: &Option<String>| {
        title
            .as_deref()
            .map(|t| format!("{:?}", super::report::one_line_plain(t)))
            .unwrap_or_else(|| "a workspace".into())
    };
    let mut out: Vec<String> = n
        .notices
        .iter()
        .map(|x| {
            let how = match x.overlap {
                Overlap::Lines => "overlapping lines",
                Overlap::File => "same file",
                Overlap::Disjoint => "other lines",
            };
            format!("{} also changes {} ({how})", name(&x.title), x.path)
        })
        .collect();
    out.extend(
        n.leased
            .iter()
            .map(|l| format!("{} leased {}, changed anyway", name(&l.title), l.path)),
    );
    out
}
