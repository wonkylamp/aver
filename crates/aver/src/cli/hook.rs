//! `aver hook …`: Claude Code's hooks in Aver workspace directories
//! (ARCHITECTURE.md §2.10, §2.11). Each reads the hook input on stdin and
//! does nothing outside a workspace directory.
//!
//! - `stop` (Stop, SubagentStop) verifies the directory of an `aver run`
//!   agent and blocks the stop with the agent report when the gate says
//!   continue. Any failure lets the agent stop; the supervisor then
//!   verifies. In any other workspace directory (a Claude Code worktree)
//!   it runs no sandbox: it blocks once when the directory changed files
//!   and no `PASS` or `REVIEW` receipt is for exactly them
//!   ([`hooks::worktree_stop`]).
//! - `session-start` (SessionStart) gives a session in a workspace
//!   directory the short brief: what done means, `aver check`, `aver
//!   verify`, where commits go. A session in the checkout of a repository
//!   set up with Aver (an `aver.toml`, or Aver's hooks in its Claude
//!   settings) is told to make a workspace with `aver workspace new` before
//!   its first edit ([`hooks::ROOT_CONTEXT`]): an instruction, which nothing
//!   enforces.
//! - `worktree-create` (WorktreeCreate) makes an Aver workspace directory
//!   for a Claude Code worktree session (`aver integrate claude`): an
//!   attempt at a regression-only task on the repository's `HEAD`, and
//!   prints its path as the last line of stdout.
//! - `worktree-remove` (WorktreeRemove) discards that workspace and moves
//!   its directory to the trash, unless its Git view holds commits the
//!   repository does not have: then it keeps it and prints how to fetch
//!   them.
//!
//! A subagent's hooks may carry the parent's `cwd`; its worktree is found
//! from its `agent_id` instead ([`hooks::subagent_worktree`]).
//!
//! Every hook fails open: an error is reported on stderr and the agent
//! goes on. The hooks Aver installed before 0.4 (`pre-edit`, `post-edit`,
//! `pre-bash`, `post-bash`, `pre-agent`) are gone; any other name exits 0
//! and prints nothing, so settings an upgrade has not rewritten yet never
//! fail a tool call.
//!
//! The plugin and a repository's settings may both run a hook for one
//! event; [`claim`] lets only the first run act. `--plugin` marks a call
//! from the plugin, which ships with no integration rev to be stale.

use std::io::Read;

use anyhow::{bail, Context as _};
use aver::keys::Home;
use aver::run::hooks::{self, SessionInput, StopAction, StopMode, WorkState};
use aver::run::report::{agent_text, ReportInput};
use aver::run::state::{self, AgentMarker, RunFile};
use aver::services::key_in;
use aver::workspace::WorkspaceFile;
use aver_core::rank::gate;
use aver_core::Verdict;

use super::workspace::Context;

mod claim;
mod stale;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    hook: Hook,
    /// Called by the Claude Code plugin, whose hooks are never stale.
    #[arg(long, global = true)]
    plugin: bool,
    /// The integration rev of the settings that call it (`aver integrate`
    /// writes it), so a stale integration is reported.
    #[arg(long, global = true, hide = true)]
    rev: Option<u32>,
}

#[derive(clap::Subcommand)]
enum Hook {
    /// Claude Code's Stop and SubagentStop hook: the verify gate.
    Stop,
    /// Claude Code's SessionStart hook: the short brief.
    SessionStart,
    /// Claude Code's WorktreeCreate hook: an Aver workspace directory.
    WorktreeCreate,
    /// Claude Code's WorktreeRemove hook.
    WorktreeRemove,
    /// A hook Aver no longer has, still named in settings it wrote before.
    #[command(external_subcommand)]
    Removed(#[allow(dead_code)] Vec<String>),
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    if matches!(args.hook, Hook::Removed(_)) {
        return Ok(());
    }
    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .context("reading the hook input")?;
    let create = matches!(args.hook, Hook::WorktreeCreate);
    // A claim Aver cannot take (an unwritable home) must not stop the hook.
    let claim = match claim::claim(home, args.hook.name(), &raw, create) {
        Ok(claim::Outcome::Run(c)) => Some(c),
        Ok(claim::Outcome::Skip) => return Ok(()),
        Ok(claim::Outcome::Reuse(dir)) => {
            // Claude Code reads the path from the last line of stdout.
            println!("{dir}");
            return Ok(());
        }
        Ok(claim::Outcome::Failed) => {
            bail!("the other WorktreeCreate hook for this worktree failed; see its error")
        }
        Err(e) => {
            eprintln!("aver hook: could not claim the event ({e}); running anyway");
            None
        }
    };
    let worktree = create || matches!(args.hook, Hook::WorktreeRemove);
    let result = match stale::due(home, args.rev, args.plugin || worktree, &raw) {
        Some(due) => {
            stale::deliver(home, due, || dispatch(home, &args.hook, &raw).map(drop)).map(|()| None)
        }
        None => dispatch(home, &args.hook, &raw),
    };
    match (claim, &result) {
        (Some(c), Err(_)) if !create => c.release(),
        (Some(c), _) => c.finish(result.as_ref().ok().cloned().flatten()),
        (None, _) => {}
    }
    result.map(drop)
}

/// Runs `hook` on its input `raw`; returns the directory a
/// `worktree-create` printed.
fn dispatch(home: &Home, hook: &Hook, raw: &str) -> anyhow::Result<Option<String>> {
    // An agent answering Aver's feedback questions (§2.10) stops without
    // verifying and may not edit anything.
    if std::env::var_os(aver::run::feedback::FEEDBACK_ENV).is_some() {
        return match hook {
            Hook::WorktreeCreate => worktree_create(home, raw).map(Some),
            Hook::WorktreeRemove => worktree_remove(raw).map(|()| None),
            Hook::Stop | Hook::SessionStart | Hook::Removed(_) => Ok(None),
        };
    }
    match hook {
        Hook::Stop => {
            if let Err(e) = stop(home, raw) {
                eprintln!(
                    "aver hook stop: {}; letting the agent stop",
                    super::describe(&e)
                );
            }
            Ok(None)
        }
        Hook::SessionStart => {
            if let Err(e) = session_start(home, raw) {
                eprintln!("aver hook session-start: {}", super::describe(&e));
            }
            Ok(None)
        }
        Hook::WorktreeCreate => worktree_create(home, raw).map(Some),
        Hook::WorktreeRemove => worktree_remove(raw).map(|()| None),
        Hook::Removed(_) => Ok(None),
    }
}

impl Hook {
    /// The name settings call it by (`aver hook <name>`).
    fn name(&self) -> &'static str {
        match self {
            Hook::Stop => "stop",
            Hook::SessionStart => "session-start",
            Hook::WorktreeCreate => "worktree-create",
            Hook::WorktreeRemove => "worktree-remove",
            Hook::Removed(_) => "removed",
        }
    }
}

#[derive(serde::Deserialize)]
struct WorktreeInput {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    cwd: Option<std::path::PathBuf>,
    #[serde(default)]
    worktree_path: Option<std::path::PathBuf>,
}

fn worktree_input(raw: &str) -> anyhow::Result<WorktreeInput> {
    serde_json::from_str(raw).context("reading the hook input")
}

fn git_out(dir: &std::path::Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Files Claude Code needs in the workspace that the commit may not hold
/// yet: the project's settings, skill and commands as `aver integrate
/// claude` wrote them before they were committed, the local settings and
/// the user's own MCP servers, as they are in the checkout.
fn carried() -> Vec<&'static str> {
    let mut out = vec![
        aver::integrate::SETTINGS,
        aver::integrate::LOCAL_SETTINGS,
        aver::integrate::MCP,
    ];
    out.extend(aver::integrate::plugin::SHARED.iter().map(|f| f.project));
    out
}

/// Makes the workspace directory and returns its path, which `run` prints
/// as the last line of stdout.
fn worktree_create(home: &Home, raw: &str) -> anyhow::Result<String> {
    let input = worktree_input(raw)?;
    let cwd = match input.cwd {
        Some(dir) => dir,
        None => std::env::current_dir()?,
    };
    let repo = git_out(&cwd, &["rev-parse", "--show-toplevel"])
        .map(std::path::PathBuf::from)
        .with_context(|| format!("{} is not inside a Git repository", cwd.display()))?;
    let repo = std::fs::canonicalize(repo)?;
    let name = input.name.unwrap_or_else(|| "worktree".into());
    let label = hooks::worktree_label(&name);
    let admin = key_in(home, None)?;
    let (rt, api) = super::open(home)?;
    let imported = rt.block_on(api.import(&admin.signing_key, &repo, "HEAD"))?;
    let commit = imported.commit.to_string();
    let title = format!("Claude Code worktree {name}");
    let body = aver_core::TaskBody {
        schema: aver_core::TaskSchema::V2,
        title: aver_core::Text::truncated(&title),
        intent: aver_core::Text::truncated(&format!(
            "Work of the Claude Code session in worktree {name}, from {} at {commit}.",
            repo.display()
        )),
        base: imported.snapshot,
        target: None,
        criteria: aver_core::Criteria::new(Vec::new())?,
        checks: aver_core::Set::empty(),
        protected_paths: aver_core::Set::empty(),
        author: admin.id(),
        created_at: aver_core::Timestamp::now(),
    };
    let task = aver_core::Task::sign(body, &admin.signing_key);
    let task_id = rt.block_on(api.create_task(&task))?;
    let suffix = aver::run::state::new_run_id().to_lowercase();
    let key_name: aver::agents::AgentName = format!("wt-{suffix}").parse()?;
    let key = aver::keys::AgentKey::generate(key_name.clone());
    key.write(&home.key_path(&key_name))?;
    rt.block_on(api.register_agent(
        &admin.signing_key,
        key_name.as_str(),
        &key.to_file().public_key,
        &[
            aver::agents::Capability::TaskClaim,
            aver::agents::Capability::WorkspaceWrite,
            aver::agents::Capability::VerifyRun,
            aver::agents::Capability::ExecRun,
            aver::agents::Capability::LeaseWrite,
        ],
    ))?;
    let claim = rt.block_on(api.claim(&key.signing_key, &task_id, Some(imported.snapshot)))?;
    let carried: Vec<&str> = carried()
        .into_iter()
        .filter(|rel| repo.join(rel).is_file())
        .filter(|rel| {
            git_out(&repo, &["ls-tree", "--name-only", &commit, "--", rel])
                .is_none_or(|o| o.is_empty())
        })
        .collect();
    let m = rt.block_on(api.materialize(
        &claim.workspace,
        &aver::workspace::MaterializeOptions {
            repo: Some(repo.clone()),
            label: Some(label),
            deps: true,
            key: Some(key_name.to_string()),
            git_view: true,
            ports: Some(10),
            reserve: carried.iter().map(|s| s.to_string()).collect(),
        },
    ))?;
    // The base's runs start now, detached, so the session's first verify
    // (on a directory that may not have changed at all) finds them cached:
    // after the materialization, which names the repository whose base
    // slot that verify uses. One that cannot start only means a slower
    // verify.
    if let Ok(exe) = std::env::current_exe() {
        let _ = api.warm(&exe, &task_id);
    }
    for rel in &carried {
        let to = m.dir.join(rel);
        // A tracked symlink on the way (a `.claude` link) would carry the
        // user's settings out of the directory.
        let linked = std::path::Path::new(rel)
            .ancestors()
            .skip(1)
            .filter(|a| !a.as_os_str().is_empty())
            .any(|a| std::fs::symlink_metadata(m.dir.join(a)).is_ok_and(|m| !m.is_dir()));
        if linked || std::fs::symlink_metadata(&to).is_ok() {
            continue;
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(repo.join(rel), &to)?;
    }
    let branch = m.git_view.then(|| {
        aver::disk::gitview::branch_for(
            &m.dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    });
    // The session learns what the directory is and how to finish from the
    // brief, as an agent of `aver run` does.
    WorkspaceFile::write_meta(
        &m.dir,
        ".aver/BRIEF.md",
        aver::run::brief::render_workspace(&aver::run::brief::WorkspaceBrief {
            dir: &m.dir,
            title: task.body.title.as_str(),
            intent: task.body.intent.as_str(),
            criteria: &[],
            max_verifies: None,
            branch: branch.as_deref(),
        }),
    )?;
    eprintln!(
        "aver: workspace for worktree {name}: {} (task {task_id}, from {}); read .aver/BRIEF.md there",
        m.dir.display(),
        &commit[..12.min(commit.len())]
    );
    if let Some(branch) = branch {
        eprintln!(
            "aver: commits land on {branch}; fetch them with git fetch {} {branch}:<branch>",
            aver::disk::gitview::view_dir(home.root(), &claim.workspace).display()
        );
    }
    // Claude Code reads the path from the last line of stdout.
    let dir = m.dir.display().to_string();
    println!("{dir}");
    Ok(dir)
}

fn worktree_remove(raw: &str) -> anyhow::Result<()> {
    let input = worktree_input(raw)?;
    let path = input
        .worktree_path
        .context("the hook input names no worktree_path")?;
    let Some(ctx) = Context::at(&path)? else {
        bail!(
            "{} is not an Aver workspace directory; leaving it",
            path.display()
        );
    };
    if std::fs::canonicalize(&ctx.root)? != std::fs::canonicalize(&path)? {
        bail!(
            "{} is inside an Aver workspace, not one; leaving it",
            path.display()
        );
    }
    keep_unfetched(&ctx, &path)?;
    let owner = ctx.key()?;
    let (rt, api) = super::open(&ctx.home)?;
    rt.block_on(async {
        match api.discard(&owner.signing_key, &ctx.file.workspace).await {
            Err(e) if e.code() == Some("WORKSPACE_CLOSED") => {}
            other => other?,
        }
        api.release(&ctx.file.workspace).await?;
        let _ = api.revoke_agent(&owner.id()).await;
        anyhow::Ok(())
    })?;
    let _ = std::fs::remove_file(ctx.home.key_path(&owner.name));
    eprintln!("aver: removed workspace {}", ctx.file.workspace);
    Ok(())
}

/// Refuses to remove a workspace whose Git view holds commits the user's
/// repository does not have, and says how to fetch them. A hook cannot ask:
/// Claude Code owns the terminal and gives the hook only its JSON input, so
/// refusing is the one safe answer, and Claude Code then keeps the
/// worktree. A view Aver cannot check is kept too.
fn keep_unfetched(ctx: &Context, dir: &std::path::Path) -> anyhow::Result<()> {
    refuse_unfetched(ctx.home.root(), &ctx.file.workspace, dir, "kept")
}

/// Fails, naming the commits and the commands that fetch them, when the Git
/// view of workspace `ws` (directory `dir`) holds commits the user's
/// repository lacks, or cannot be checked. `verb` starts the message
/// (`kept`, `refusing to remove`).
pub(super) fn refuse_unfetched(
    home: &std::path::Path,
    ws: &aver_core::WorkspaceId,
    dir: &std::path::Path,
    verb: &str,
) -> anyhow::Result<()> {
    let view = aver::disk::gitview::view_dir(home, ws);
    if !view.is_dir() {
        return Ok(());
    }
    let lost = aver::disk::gitview::unfetched(&view).with_context(|| {
        format!(
            "{verb} {}: could not check its Git view {} for commits the repository lacks",
            dir.display(),
            view.display()
        )
    })?;
    if lost.is_empty() {
        return Ok(());
    }
    let repo = aver::disk::gitview::source(&view)?;
    let repo = match repo.file_name() {
        Some(n) if n == ".git" => repo.parent().unwrap_or(&repo).to_owned(),
        _ => repo,
    };
    let label = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut text = format!(
        "{verb} {}: its Git view has commits that {} does not have:\n",
        dir.display(),
        repo.display()
    );
    for u in &lost {
        let short = u.refname.strip_prefix("refs/heads/").unwrap_or(&u.refname);
        text.push_str(&format!(
            "  {short} at {} {}\n",
            &u.sha[..12.min(u.sha.len())],
            u.subject
        ));
    }
    text.push_str("Fetch them into the repository, then remove the workspace:\n");
    let mut taken = Vec::new();
    for u in &lost {
        let (from, into) = match u.refname.strip_prefix("refs/heads/") {
            Some(branch) => (u.refname.clone(), branch.to_owned()),
            None if u.refname == "refs/stash" => (u.refname.clone(), format!("aver/{label}-stash")),
            None => {
                // A detached HEAD is not advertised; name it first.
                let name = format!("aver/{label}-head");
                text.push_str(&format!("  git -C {} branch {name}\n", dir.display()));
                (format!("refs/heads/{name}"), name)
            }
        };
        let into = free_branch(&repo, &into, &taken);
        taken.push(into.clone());
        text.push_str(&format!(
            "  git -C {} fetch {} {from}:refs/heads/{into}\n",
            repo.display(),
            view.display(),
        ));
    }
    text.push_str(&format!("  aver workspace rm {}", dir.display()));
    if verb != "kept" {
        text.push_str(&format!(
            "\nOr discard the commits: aver workspace rm --force {}",
            dir.display()
        ));
    }
    bail!(text)
}

/// `name`, or when the repository already has that branch (`aver accept`
/// exports the squashed commit as `aver/<dir>`, the view's own branch
/// name) `name-commits`, numbered until free: fetching the view's history
/// over an existing branch is refused as non-fast-forward.
fn free_branch(repo: &std::path::Path, name: &str, taken: &[String]) -> String {
    let exists = |b: &str| {
        taken.iter().any(|t| t == b)
            || std::process::Command::new("git")
                .args(["rev-parse", "--verify", "--quiet"])
                .arg(format!("refs/heads/{b}"))
                .current_dir(repo)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
    };
    if !exists(name) {
        return name.to_owned();
    }
    (1..)
        .map(|n| match n {
            1 => format!("{name}-commits"),
            n => format!("{name}-commits-{n}"),
        })
        .find(|b| !exists(b))
        .expect("some name is free")
}

fn session_input(raw: &str) -> SessionInput {
    serde_json::from_str(raw).unwrap_or_default()
}

/// The workspace directory a hook input is about. A subagent's is the
/// worktree Claude Code made for it, found by its `agent_id` under the
/// home's `work/` (its `cwd` may be the parent's, whose changes are not the
/// subagent's to verify); otherwise the one holding its `cwd`.
fn workspace_of(home: &Home, input: &SessionInput) -> anyhow::Result<Option<Context>> {
    if let Some(id) = input.agent_id.as_deref() {
        let label = hooks::worktree_label(&hooks::subagent_worktree(id));
        let found = std::fs::read_dir(home.root().join("work"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|repo| repo.path().join(&label))
            .find(|dir| dir.join(WorkspaceFile::PATH).is_file());
        if let Some(dir) = found {
            return Context::at(&dir);
        }
        // Without a worktree of its own, a subagent's start and stop are
        // not about the directory it shares with its parent.
        if input.subagent_event() {
            return Ok(None);
        }
    }
    match &input.cwd {
        Some(dir) => Context::at(dir),
        None => Context::find(),
    }
}

/// The short brief for the workspace at `root`, naming the branch of its
/// Git view when it has one.
fn start_text(root: &std::path::Path) -> String {
    let branch = root.join(".git").exists().then(|| {
        aver::disk::gitview::branch_for(
            &root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    });
    hooks::start_context(root, branch.as_deref())
}

/// Whether the checkout holding `dir` is of a repository set up with
/// Aver: an `aver.toml` at its root, or Aver's hooks in its Claude
/// settings.
fn set_up_with_aver(dir: &std::path::Path) -> bool {
    let Some(root) = git_out(dir, &["rev-parse", "--show-toplevel"]) else {
        return false;
    };
    let root = std::path::Path::new(&root);
    root.join(aver::env::CONFIG_PATH).is_file()
        || [aver::integrate::SETTINGS, aver::integrate::LOCAL_SETTINGS]
            .iter()
            .filter_map(|rel| std::fs::read_to_string(root.join(rel)).ok())
            .filter_map(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .any(|settings| {
                settings["hooks"]
                    .as_object()
                    .into_iter()
                    .flat_map(|events| events.values())
                    .filter_map(|groups| groups.as_array())
                    .flatten()
                    .filter_map(|group| group["hooks"].as_array())
                    .flatten()
                    .filter_map(|hook| hook["command"].as_str())
                    .any(|command| aver::integrate::parse_hook(command).is_some())
            })
}

fn session_start(home: &Home, raw: &str) -> anyhow::Result<()> {
    let input = session_input(raw);
    let Some(ctx) = workspace_of(home, &input)? else {
        let in_checkout = input.agent_id.is_none()
            && !input.subagent_event()
            && input.cwd.as_deref().is_some_and(set_up_with_aver);
        if in_checkout {
            println!("{}", hooks::start_json("SessionStart", hooks::ROOT_CONTEXT));
        }
        return Ok(());
    };
    if RunFile::read(&ctx.root).is_some() {
        return Ok(());
    }
    println!(
        "{}",
        hooks::start_json("SessionStart", &start_text(&ctx.root))
    );
    Ok(())
}

fn stop(home: &Home, raw: &str) -> anyhow::Result<()> {
    let input = session_input(raw);
    let Some(ctx) = workspace_of(home, &input)? else {
        return Ok(());
    };
    match RunFile::read(&ctx.root) {
        Some(run) if !input.subagent_event() => run_stop(&input, &ctx.root, ctx.file, run),
        Some(_) => Ok(()),
        None => worktree_stop(&input, &ctx),
    }
}

/// The stop gate's mode from `[agents] stop` in the `aver.toml` of the
/// workspace's base: the agent's own copy, which it may edit, is not read.
fn stop_mode(api: &aver::run::api::Api, base: &aver_core::SnapshotId) -> anyhow::Result<StopMode> {
    let store = &api.services().store;
    let snapshot: aver_core::Snapshot = store.get(base)?;
    let path = aver::env::CONFIG_PATH.parse().expect("a valid path");
    Ok(aver::env::read_file(&**store, &snapshot.root, &path)?
        .map(|bytes| StopMode::from_config(&String::from_utf8_lossy(&bytes)))
        .unwrap_or_default())
}

/// How many of the attempt's newest receipts the gate looks through for one
/// of the directory's current files.
const RECENT_RECEIPTS: usize = 5;

fn worktree_stop(input: &SessionInput, ctx: &Context) -> anyhow::Result<()> {
    let Some(attempt) = ctx.file.attempt else {
        return Ok(());
    };
    let stopper = input.stopper();
    let mut records = hooks::read_gate(&ctx.root);
    let record = records.get(&stopper).cloned().unwrap_or_default();
    let gave_up = state::gave_up(&ctx.root).is_some();
    if gave_up {
        return Ok(());
    }
    let key = ctx.key()?;
    let (rt, api) = super::open(&ctx.home)?;
    let mode = stop_mode(&api, &ctx.file.base)?;
    let (action, record, reason) =
        hooks::worktree_stop(mode, input.stop_hook_active, gave_up, &record, || {
            let sync = rt.block_on(api.sync(&key.signing_key, &ctx.file.workspace))?;
            if sync.differ() == 0 && sync.skipped.is_empty() {
                return anyhow::Ok(WorkState::Unchanged);
            }
            let head = rt.block_on(api.snapshot(&key.signing_key, &ctx.file.workspace))?;
            if head == ctx.file.base {
                return Ok(WorkState::Unchanged);
            }
            let recent = recent_verdicts(&rt, &api, &key, &attempt)?;
            let verified = recent.iter().find(|(h, _)| *h == head).map(|(_, v)| *v);
            let stale = recent.first().filter(|(h, _)| *h != head).map(|(_, v)| *v);
            Ok(WorkState::Changed {
                head: head.to_string(),
                files: sync.differ(),
                verified,
                stale,
            })
        })?;
    if action == StopAction::Block {
        records.insert(stopper, record);
        hooks::write_gate(&ctx.root, &records);
        if let Some(reason) = reason {
            println!("{}", hooks::block_json(&reason));
        }
    }
    Ok(())
}

/// Attempt `id`'s newest receipts, at most [`RECENT_RECEIPTS`], as
/// `(head snapshot, verdict)`.
fn recent_verdicts(
    rt: &tokio::runtime::Runtime,
    api: &aver::run::api::Api,
    key: &aver::keys::AgentKey,
    id: &aver_core::AttemptId,
) -> anyhow::Result<Vec<(aver_core::SnapshotId, aver_core::Verdict)>> {
    let attempt = rt.block_on(api.attempt(&key.signing_key, id))?;
    let mut out = Vec::new();
    for rid in attempt.receipts.iter().rev().take(RECENT_RECEIPTS) {
        let receipt: aver_core::Receipt = api.services().store.get(rid)?;
        out.push((receipt.body.head_snapshot, receipt.body.verdict));
    }
    Ok(out)
}

fn run_stop(
    input: &SessionInput,
    root: &std::path::Path,
    file: WorkspaceFile,
    run: RunFile,
) -> anyhow::Result<()> {
    let (Some(task), Some(attempt)) = (file.task, file.attempt) else {
        return Ok(());
    };
    if state::gave_up(root).is_some() {
        return Ok(());
    }
    let home = Home::new(&file.home);
    let name = file
        .key
        .as_deref()
        .map(str::parse::<aver::agents::AgentName>)
        .transpose()?;
    let key = key_in(&home, name.as_ref())?;
    let (rt, api) = super::open(&home)?;
    let v = {
        let _marker = AgentMarker::verifying(root);
        let flags = aver::run::api::VerifyFlags::default();
        rt.block_on(api.verify_with(&key.signing_key, &task, &file.workspace, flags, None))?
    };
    let verifies = rt
        .block_on(api.attempt(&key.signing_key, &attempt))?
        .receipts
        .len() as u32;
    let notices = aver::seen::for_report(root, v.notices.as_ref(), aver_core::Timestamp::now());
    let input_report = ReportInput {
        receipt_id: &v.receipt_id,
        receipt: &v.receipt,
        diagnostics: v.diagnostics.as_ref(),
        sync: v.sync.as_ref(),
        took: None,
        budget: Some((verifies, run.max_verifies)),
        notices: notices.as_ref(),
        timing: v.timing.as_ref(),
    };
    let done = aver::run::report::exit_code(&input_report) == 0;
    let latest = if done {
        Some(v.receipt.body.verdict)
    } else if matches!(v.receipt.body.verdict, Verdict::Pass | Verdict::Review) {
        Some(Verdict::Unproven)
    } else {
        Some(v.receipt.body.verdict)
    };
    let decision = gate(latest, verifies, run.max_verifies, false);
    let (action, blocks) = hooks::stop(input.stop_hook_active, hooks::read_blocks(root), decision);
    hooks::write_blocks(root, blocks);
    if action == StopAction::Block {
        let reason = aver::run::report::stop_reason(&aver::run::report::note_once(
            root,
            agent_text(&input_report),
        ));
        println!("{}", hooks::block_json(&reason));
    }
    Ok(())
}
