//! `aver workspace`: on-disk workspaces, a drop-in for `git worktree add`
//! (ARCHITECTURE.md §2.3.1).

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Context as _};
use aver::agents::AgentName;
use aver::disk::{MatState, Materialization};
use aver::keys::{AgentKey, Home};
use aver::run::api::Api;
use aver::services::{key_in, Services};
use aver::workspace::{DiffChange, MaterializeOptions, WorkspaceFile, WorkspaceState};
use aver_core::{
    Criteria, Criterion, Set, SnapshotId, Task, TaskBody, TaskId, TaskSchema, Timestamp,
    WorkspaceId,
};
use serde_json::json;
use tokio::runtime::Runtime;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Import a commit and materialize workspace directories on it, each an
    /// attempt at one task. Prints each directory on its own line.
    New(NewArgs),
    /// List workspace directories.
    Ls {
        /// Include released and lost directories.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Discard workspaces and delete their directories. Syncs first, and
    /// refuses a directory with changes no commit holds (with a diff stat)
    /// unless --force. A directory equal to a commit of its Git view that a
    /// ref of the repository reaches is removed.
    Rm {
        /// Workspace ids or directories.
        #[arg(required = true)]
        targets: Vec<String>,
        /// Remove a workspace even when its directory holds changes no
        /// commit keeps, or its Git view holds commits the repository has
        /// not fetched (they are lost).
        #[arg(long)]
        force: bool,
    },
    /// Record a directory's changes now (every read does this anyway) and
    /// print what the sync found.
    Sync {
        /// A workspace id or directory (default: the workspace containing
        /// the current directory).
        target: Option<String>,
        /// Rebase onto this commit first (default: the repository's HEAD), as
        /// aver workspace rebase does.
        #[arg(long, value_name = "REV", num_args = 0..=1, default_missing_value = "HEAD")]
        base: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Move the workspace onto a newer commit of its repository (default:
    /// HEAD) by a three-way merge of the directory, so only your own
    /// changes remain. A conflict changes nothing and lists both sides.
    Rebase {
        /// The commit; it must descend from the workspace's base.
        rev: Option<String>,
        /// A workspace id or directory (default: the current directory's).
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Args)]
struct NewArgs {
    /// The Git repository (its root).
    #[arg(default_value = ".")]
    repo: PathBuf,
    /// The commit to start from.
    #[arg(long, default_value = "HEAD")]
    rev: String,
    /// Claim this existing task instead of creating one.
    #[arg(long, conflicts_with_all = ["criterion", "title"])]
    task: Option<TaskId>,
    /// A criterion `id: text` for the created task; repeat for more. Without
    /// any, the task only checks that nothing broke.
    #[arg(short = 'c', long = "criterion", value_name = "ID: TEXT")]
    criterion: Vec<String>,
    /// The created task's title.
    #[arg(long)]
    title: Option<String>,
    /// The directory name under `$AVER_HOME/work/<repo>/` (suffixed -2, -3,
    /// … with --count).
    #[arg(long)]
    label: Option<String>,
    /// How many workspaces to create on the same task.
    #[arg(long, default_value_t = 1)]
    count: u32,
    /// Do not clone the dependencies the repository has installed
    /// (`node_modules`, `.venv`, a Cargo `target`).
    #[arg(long)]
    no_deps: bool,
    /// Accepted and ignored: Aver no longer installs dependencies itself.
    #[arg(long, hide = true)]
    adopt_only: bool,
    /// Skip the Git view (`git status` and `git diff` inside the directory).
    #[arg(long)]
    no_git: bool,
    /// The key that owns the workspaces and authors the task (default: the
    /// admin authors the task, and each workspace gets an agent key of its
    /// own).
    #[arg(long)]
    key: Option<AgentName>,
    /// Print JSON with timings instead of directories.
    #[arg(long)]
    json: bool,
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    match args.command {
        Command::New(args) => new(home, args),
        Command::Ls { all, json } => ls(home, all, json),
        Command::Rm { targets, force } => rm(home, &targets, force),
        Command::Sync { target, base, json } => match base {
            Some(rev) => rebase(home, target.as_deref(), &rev, json),
            None => sync(home, target.as_deref(), json),
        },
        Command::Rebase {
            rev,
            workspace,
            json,
        } => rebase(
            home,
            workspace.as_deref(),
            rev.as_deref().unwrap_or("HEAD"),
            json,
        ),
    }
}

pub(super) fn parse_criterion(text: &str) -> anyhow::Result<Criterion> {
    let (id, rest) = text
        .split_once(':')
        .with_context(|| format!("criterion {text:?} is not `id: text`"))?;
    Ok(Criterion {
        id: id.trim().parse()?,
        text: rest.trim().to_owned(),
    })
}

/// A signed task on `base` with the given criteria; none makes a
/// regression-only task.
pub(super) fn task_from(
    key: &AgentKey,
    base: SnapshotId,
    title: &str,
    intent: &str,
    criteria: &[String],
) -> anyhow::Result<Task> {
    let criteria = Criteria::new(
        criteria
            .iter()
            .map(|c| parse_criterion(c))
            .collect::<anyhow::Result<_>>()?,
    )?;
    let body = TaskBody {
        schema: TaskSchema::for_task(&criteria, &Set::empty()),
        title: title.parse()?,
        intent: intent.parse()?,
        base,
        target: None,
        criteria,
        checks: Set::empty(),
        protected_paths: Set::empty(),
        author: key.id(),
        created_at: Timestamp::now(),
    };
    Ok(Task::sign(body, &key.signing_key))
}

fn new(home: &Home, args: NewArgs) -> anyhow::Result<()> {
    if args.count == 0 {
        bail!("--count must be at least 1");
    }
    let started = Instant::now();
    aver::keys::init(home)?;
    warn_low_disk(home);
    let key = key_in(home, args.key.as_ref())?;
    let repo = std::fs::canonicalize(&args.repo)
        .with_context(|| format!("repository {}", args.repo.display()))?;
    let (_rt, api) = super::open(home)?;
    let services = api.services();
    let import = super::import::import(
        services,
        &key,
        &repo.to_string_lossy(),
        &args.rev,
        None,
        aver::git::SecretPolicy::RejectHigh,
    )?;
    warn_secrets(&import);
    let snapshot: SnapshotId = serde_json::from_value(import["snapshot"].clone())?;
    let commit = import["commit"].as_str().unwrap_or_default().to_owned();
    let task = match args.task {
        Some(task) => task,
        None => {
            let title = args.title.clone().unwrap_or_else(|| {
                let name = repo
                    .file_name()
                    .map_or("repo".into(), |n: &std::ffi::OsStr| n.to_string_lossy());
                format!("Work on {name} at {}", &commit[..12.min(commit.len())])
            });
            let task = task_from(&key, snapshot, &title, &title, &args.criterion)?;
            super::task::create(services, &task)?
        }
    };
    let _ = args.adopt_only;
    let deps = !args.no_deps;
    // Each directory says what it is for and how to finish (§2.3.1), for an
    // agent that starts there without having been told.
    let body = services.tasks.get(&task)?.body;
    let criteria: Vec<(String, String)> = body
        .criteria
        .iter()
        .map(|c| (c.id.to_string(), c.text.clone()))
        .collect();
    if !args.json {
        eprintln!(
            "aver: task {task} on {} ({} files)",
            &commit[..12.min(commit.len())],
            import["files"]
        );
    }
    let mut created = Vec::new();
    for n in 1..=args.count {
        let t = Instant::now();
        // Like `aver run`'s agents, a directory's agent holds a key that can
        // claim, edit, verify and lease but never act as the admin: the key
        // file is in reach of everything that runs in the directory.
        let own;
        let owner = match &args.key {
            Some(_) => &key,
            None => {
                let name = format!("ws-{}", aver::run::state::new_run_id().to_lowercase());
                own =
                    super::agent::add(services, home, name.parse()?, super::agent::worker_caps())?
                        .0;
                &own
            }
        };
        let claim = services.tasks.claim(&owner.id(), &task, Some(snapshot))?;
        let label = args.label.as_ref().map(|l| {
            if n == 1 {
                l.clone()
            } else {
                format!("{l}-{n}")
            }
        });
        let m = services.workspaces().materialize(
            &claim.workspace,
            &MaterializeOptions {
                repo: Some(repo.clone()),
                label,
                deps,
                key: Some(owner.name.to_string()),
                git_view: !args.no_git,
                ports: None,
                reserve: Vec::new(),
            },
        )?;
        let label = m.dir.file_name().map(|n| n.to_string_lossy().into_owned());
        let branch = m
            .git_view
            .then(|| aver::disk::gitview::branch_for(label.as_deref().unwrap_or_default()));
        WorkspaceFile::write_meta(
            &m.dir,
            ".aver/BRIEF.md",
            aver::run::brief::render_workspace(&aver::run::brief::WorkspaceBrief {
                dir: &m.dir,
                title: body.title.as_str(),
                intent: body.intent.as_str(),
                criteria: &criteria,
                max_verifies: None,
                branch: branch.as_deref(),
            }),
        )?;
        let took = t.elapsed();
        // Printed as it lands: a later directory that fails leaves this
        // one named, not an unnamed directory a later `aver gc` keeps.
        if !args.json {
            let view = branch
                .map(|b| format!(", git view on {b}"))
                .unwrap_or_default();
            eprintln!(
                "aver: {} in {:.0} ms ({}{})",
                claim.workspace,
                took.as_secs_f64() * 1000.0,
                m.clone.as_str(),
                view,
            );
            println!("{}", m.dir.display());
        }
        created.push((claim, m, took));
    }
    // After the directories, so it neither competes with them nor starts
    // before one names the repository: the warm-up runs in that
    // repository's base slot, the one its verifies use. A warm-up that
    // cannot start only makes the first verify slower.
    if let Ok(exe) = std::env::current_exe() {
        let _ = api.warm(&exe, &task);
    }
    if args.json {
        let out = json!({
            "task": task,
            "snapshot": snapshot,
            "commit": commit,
            "files": import["files"],
            "total_ms": started.elapsed().as_secs_f64() * 1000.0,
            "workspaces": created.iter().map(|(claim, m, took)| json!({
                "workspace": claim.workspace,
                "attempt": claim.attempt,
                "dir": m.dir,
                "clone": m.clone,
                "absent": m.absent,
                "git_view": m.git_view,
                "ms": took.as_secs_f64() * 1000.0,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    }
    Ok(())
}

fn ls(home: &Home, all: bool, json: bool) -> anyhow::Result<()> {
    let services = Services::open(home)?;
    let rows = sorted_for_ls(services.workspaces().disk().materializations(all)?);
    // A run agent's directory: `<run> #<agent>`, or `#<task>.<agent>` in a batch.
    let runs: std::collections::HashMap<WorkspaceId, String> =
        aver::run::state::RunState::all(home)
            .into_iter()
            .flat_map(|r| {
                r.agents
                    .iter()
                    .map(|a| {
                        let label = match &r.batch {
                            Some(b) => format!("#{}.{}", b.number, a.index),
                            None => format!("{} #{}", r.id, a.index),
                        };
                        (a.workspace, label)
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
    let task_of = |m: &Materialization| {
        WorkspaceFile::find(&m.dir)
            .ok()
            .flatten()
            .filter(|(dir, _)| *dir == m.dir)
            .and_then(|(_, f)| f.task)
    };
    // What the directory is for: the task's first criterion, else its title.
    let title_of = |task: Option<TaskId>| {
        let body = services.tasks.get(&task?).ok()?.body;
        Some(match body.criteria.as_slice().first() {
            Some(c) => c.text.clone(),
            None => body.title.to_string(),
        })
    };
    if json {
        let rows: Vec<serde_json::Value> = rows
            .iter()
            .map(|m| {
                let mut row = serde_json::to_value(m)?;
                let task = task_of(m);
                row["task"] = json!(task);
                row["title"] = json!(title_of(task));
                row["run"] = json!(runs.get(&m.workspace));
                Ok(row)
            })
            .collect::<Result<_, serde_json::Error>>()?;
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    for m in &rows {
        let title = title_of(task_of(m)).unwrap_or_else(|| "-".to_owned());
        println!(
            "{}  {:<8}  {:<10}  {}  {}",
            m.workspace,
            m.state.as_str(),
            runs.get(&m.workspace).map_or("-", String::as_str),
            aver::run::view::fit(title.lines().next().unwrap_or(""), 40),
            m.dir.display()
        );
    }
    Ok(())
}

/// `workspace ls` order: by repository, then oldest first, so one
/// repository's directories read in the order they were made.
fn sorted_for_ls(mut rows: Vec<Materialization>) -> Vec<Materialization> {
    rows.sort_by(|a, b| {
        (a.repo.as_ref(), a.created_at, a.workspace.to_string()).cmp(&(
            b.repo.as_ref(),
            b.created_at,
            b.workspace.to_string(),
        ))
    });
    rows
}

/// The materialization a target names: a workspace id, a directory (or a
/// path inside one), or, without a target, the current directory's.
fn resolve(home: &Home, target: Option<&str>) -> anyhow::Result<Materialization> {
    let disk_rows = Services::open(home)?
        .workspaces()
        .disk()
        .materializations(true)?;
    if let Some(id) = target.and_then(|t| t.parse::<WorkspaceId>().ok()) {
        return disk_rows
            .into_iter()
            .find(|m| m.workspace == id)
            .with_context(|| format!("workspace {id} has no directory"));
    }
    let path = match target {
        Some(t) => PathBuf::from(t),
        None => std::env::current_dir()?,
    };
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    disk_rows
        .into_iter()
        .find(|m| path.starts_with(&m.dir))
        .with_context(|| format!("{} is not in an Aver workspace", path.display()))
}

/// The key that owns `m`'s workspace: the one its workspace file names,
/// else the admin.
fn owner_key(home: &Home, m: &Materialization) -> anyhow::Result<AgentKey> {
    let named = WorkspaceFile::find(&m.dir)
        .ok()
        .flatten()
        .and_then(|(_, f)| f.key)
        .map(|k| k.parse::<AgentName>())
        .transpose()?;
    if named.is_some() {
        return Ok(key_in(home, named.as_ref())?);
    }
    // The directory's file can be gone (a removal cut short), so the owner
    // comes from the store, and its key from the home's keys.
    let admin = key_in(home, None)?;
    let owner = match Services::open(home)?
        .workspaces()
        .get(&admin.id(), &m.workspace)
    {
        Err(aver::workspace::WorkspaceError::NotOwner { owner, .. }) => owner,
        _ => return Ok(admin),
    };
    for entry in std::fs::read_dir(home.keys_dir())?.flatten() {
        if let Ok(key) = AgentKey::read(&entry.path()) {
            if key.id() == owner {
                return Ok(key);
            }
        }
    }
    Ok(admin)
}

/// Discards workspace `ws` as its `owner` and removes its directory; a
/// committed workspace keeps its head and only loses the directory, even
/// one that changed since the commit (the caller checked).
fn remove(services: &Services, owner: &AgentKey, ws: &WorkspaceId) -> anyhow::Result<()> {
    let workspaces = services.workspaces();
    match workspaces.discard(&owner.id(), ws) {
        Ok(_) | Err(aver::workspace::WorkspaceError::Closed { .. }) => {}
        Err(e) => return Err(e.into()),
    }
    aver::fault::point("rm.discarded")?;
    workspaces.release(ws, true)?;
    workspaces.disk().empty_trash()?;
    Ok(())
}

fn rm(home: &Home, targets: &[String], force: bool) -> anyhow::Result<()> {
    let found: Vec<(Materialization, AgentKey)> = targets
        .iter()
        .map(|t| {
            let m = resolve(home, Some(t))?;
            if !force {
                // Unfetched commits would go with the view: say how to
                // fetch them instead.
                super::hook::refuse_unfetched(
                    home.root(),
                    &m.workspace,
                    &m.dir,
                    "refusing to remove",
                )?;
            }
            let key = owner_key(home, &m)?;
            anyhow::Ok((m, key))
        })
        .collect::<anyhow::Result<_>>()?;
    let (rt, api) = super::open(home)?;
    if !force {
        for (m, key) in &found {
            refuse_unsaved(home, &rt, &api, m, key)?;
        }
    }
    for (m, key) in found {
        remove(api.services(), &key, &m.workspace)?;
        eprintln!("aver: removed {} ({})", m.workspace, m.dir.display());
    }
    Ok(())
}

/// Refuses to remove `m` when its directory holds changes nothing else
/// keeps: an open workspace's changes (synced first, so an edit made a
/// moment ago counts), or a committed one's changes since its commit,
/// unless the directory equals a commit of its Git view that the user's
/// repository already has. The error lists them with a diff stat.
fn refuse_unsaved(
    home: &Home,
    rt: &Runtime,
    api: &Api,
    m: &Materialization,
    key: &AgentKey,
) -> anyhow::Result<()> {
    if m.state != MatState::Live {
        return Ok(());
    }
    let workspaces = api.services().workspaces();
    let lines: Vec<String> = match workspaces.get(&key.id(), &m.workspace)?.state {
        WorkspaceState::Open => {
            rt.block_on(api.sync(&key.signing_key, &m.workspace))?;
            let files = rt.block_on(api.diff(&key.signing_key, &m.workspace))?;
            let width = files.iter().map(|f| f.path.len()).max().unwrap_or(0);
            let (mut adds, mut dels) = (0, 0);
            let mut lines: Vec<String> = files
                .iter()
                .map(|f| {
                    let (a, d) = f.lines();
                    adds += a;
                    dels += d;
                    let what = match f.change {
                        DiffChange::Added => "new",
                        DiffChange::Modified => "modified",
                        DiffChange::Deleted => "deleted",
                    };
                    let stat = if f.binary {
                        "binary".to_owned()
                    } else {
                        format!("+{a} -{d}")
                    };
                    format!("  {:<width$} | {stat} ({what})", f.path)
                })
                .collect();
            if !lines.is_empty() {
                lines.push(format!(
                    "  {} files changed, {adds} insertions(+), {dels} deletions(-), never accepted",
                    files.len()
                ));
            }
            lines
        }
        WorkspaceState::Committed => {
            let paths = workspaces.changed_since_commit(&m.workspace)?;
            let mut lines: Vec<String> = paths.iter().map(|p| format!("  {p}")).collect();
            if !lines.is_empty() {
                lines.push(format!(
                    "  {} files changed since the workspace was accepted",
                    paths.len()
                ));
            }
            lines
        }
        WorkspaceState::Discarded => Vec::new(),
    };
    if lines.is_empty() {
        return Ok(());
    }
    if let Some(sha) = merged_match(home, api, m, None) {
        eprintln!(
            "aver: {} matches commit {}, which its repository has",
            m.dir.display(),
            &sha[..12.min(sha.len())]
        );
        return Ok(());
    }
    for line in &lines {
        eprintln!("{line}");
    }
    bail!(
        "refusing to remove {}: no commit holds the changes above (`aver accept` in it keeps them as a branch); --force removes them",
        m.dir.display()
    )
}

/// The commit of `m`'s Git view, already in the user's repository and not
/// reached by `exclude`, whose snapshot the directory equals, if any. The
/// commit is imported from the user's repository, never read from the view
/// (CLAUDE.md rule 4); any failure finds none, so the caller keeps the
/// directory.
fn merged_match(
    home: &Home,
    api: &Api,
    m: &Materialization,
    exclude: Option<&str>,
) -> Option<String> {
    let view = aver::disk::gitview::view_dir(home.root(), &m.workspace);
    if !view.is_dir() {
        return None;
    }
    let repo = match &m.repo {
        Some(repo) => repo.clone(),
        None => {
            let gitdir = aver::disk::gitview::source(&view).ok()?;
            match gitdir.file_name() {
                Some(n) if n == ".git" => gitdir.parent()?.to_owned(),
                _ => gitdir,
            }
        }
    };
    let admin = key_in(home, None).ok()?;
    let workspaces = api.services().workspaces();
    for sha in aver::disk::gitview::merged(&view, 8, exclude).ok()? {
        let Ok(import) = super::import::import(
            api.services(),
            &admin,
            &repo.to_string_lossy(),
            &sha,
            None,
            aver::git::SecretPolicy::Warn,
        ) else {
            continue;
        };
        let Ok(snapshot) = serde_json::from_value::<SnapshotId>(import["snapshot"].clone()) else {
            continue;
        };
        if matches!(workspaces.changed_since(&m.workspace, &snapshot), Ok(p) if p.is_empty()) {
            return Some(sha);
        }
    }
    None
}

/// Below this much free space under the home, `aver workspace new` warns:
/// each directory, its slots and its builds take space from here.
const LOW_DISK_BYTES: u64 = 15 << 30;

/// Warns on stderr, naming `aver gc`, when the home's file system is short
/// of space.
fn warn_low_disk(home: &Home) {
    let free = aver::exec::output::free_bytes(home.root());
    if let Some(line) = low_disk_warning(free, home.root()) {
        eprintln!("{line}");
    }
}

fn low_disk_warning(free: Option<u64>, root: &std::path::Path) -> Option<String> {
    let free = free.filter(|&f| f < LOW_DISK_BYTES)?;
    Some(format!(
        "aver: warning: only {:.1} GiB free under {}; `aver gc` removes merged, released and idle workspace data",
        free as f64 / f64::from(1u32 << 30),
        root.display()
    ))
}

/// A directory `aver gc` removed because it equals a merged commit.
#[derive(serde::Serialize)]
pub(super) struct MergedRemoved {
    pub workspace: WorkspaceId,
    pub dir: PathBuf,
    pub commit: String,
}

/// Removes, as `aver workspace rm` would, every live directory that equals
/// a commit of its Git view that a ref of the user's repository reaches and
/// its base does not: work already merged, with nothing left in the
/// directory alone. A directory equal only to its base (fresh, unchanged),
/// one whose view holds commits the repository lacks or cannot be read, or
/// any it cannot compare, is kept.
pub(super) fn remove_merged(home: &Home, api: &Api) -> anyhow::Result<Vec<MergedRemoved>> {
    let services = api.services();
    let workspaces = services.workspaces();
    let mut removed = Vec::new();
    for m in workspaces.disk().materializations(false)? {
        if m.state != MatState::Live || !m.git_view || !m.dir.is_dir() {
            continue;
        }
        let view = aver::disk::gitview::view_dir(home.root(), &m.workspace);
        if !matches!(aver::disk::gitview::unfetched(&view), Ok(u) if u.is_empty()) {
            continue;
        }
        let Ok(key) = owner_key(home, &m) else {
            continue;
        };
        let Ok(ws) = workspaces.get(&key.id(), &m.workspace) else {
            continue;
        };
        let Ok(Some(base)) = aver::git::imported_commit(&services.store, &ws.base) else {
            continue;
        };
        let Some(commit) = merged_match(home, api, &m, Some(base.as_str())) else {
            continue;
        };
        remove(services, &key, &m.workspace)?;
        removed.push(MergedRemoved {
            workspace: m.workspace,
            dir: m.dir,
            commit,
        });
    }
    Ok(removed)
}

fn sync(home: &Home, target: Option<&str>, json: bool) -> anyhow::Result<()> {
    let home = match target {
        None => match WorkspaceFile::find(&std::env::current_dir()?)? {
            Some((_, file)) => Home::new(file.home),
            None => home.clone(),
        },
        Some(_) => home.clone(),
    };
    let m = resolve(&home, target)?;
    let key = owner_key(&home, &m)?;
    let (rt, api) = super::open(&home)?;
    let report = rt.block_on(api.sync(&key.signing_key, &m.workspace))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "synced {} in {} ms: version {}, {} modified, {} added, {} deleted, {} hashed of {} scanned",
            m.dir.display(),
            report.stats.took_ms,
            report.version,
            report.modified,
            report.added,
            report.deleted,
            report.stats.hashed,
            report.stats.scanned
        );
        for skip in &report.skipped {
            println!("  not synced: {}: {}", skip.path, skip.reason.describe());
        }
    }
    Ok(())
}

/// `aver workspace rebase [<rev>]` and `aver workspace sync --base [<rev>]`.
fn rebase(home: &Home, target: Option<&str>, rev: &str, json: bool) -> anyhow::Result<()> {
    let home = match target {
        None => match WorkspaceFile::find(&std::env::current_dir()?)? {
            Some((_, file)) => Home::new(file.home),
            None => home.clone(),
        },
        Some(_) => home.clone(),
    };
    let m = resolve(&home, target)?;
    if let Some(run) = aver::run::state::RunState::all(&home)
        .into_iter()
        .find(|r| r.agents.iter().any(|a| a.workspace == m.workspace))
    {
        bail!(
            "{} is an agent's directory in aver run {}; a run's agents share one base, so start again on the new one with aver rerun --onto",
            m.dir.display(),
            run.id
        );
    }
    let repo = m
        .repo
        .clone()
        .context("the workspace does not know its repository, so it cannot rebase")?;
    let key = owner_key(&home, &m)?;
    let (rt, api) = super::open(&home)?;
    let import = super::import::import(
        api.services(),
        &key,
        &repo.to_string_lossy(),
        rev,
        None,
        aver::git::SecretPolicy::RejectHigh,
    )?;
    warn_secrets(&import);
    let onto: SnapshotId = serde_json::from_value(import["snapshot"].clone())?;
    let commit = import["commit"].as_str().unwrap_or_default();
    let short = &commit[..12.min(commit.len())];
    let out = rt.block_on(api.rebase(&key.signing_key, &m.workspace, &onto))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else if !out.marked.is_empty() {
        println!(
            "Rebased {} onto {short} with {} conflicting files: each holds both versions between conflict markers (<<<<<<< yours, =======, >>>>>>> {short}). Resolve each conflict and delete the marker lines; until then aver check, aver verify and aver workspace rebase refuse.",
            m.dir.display(),
            out.marked.len()
        );
        println!("{}", aver::run::report::FENCE_NOTE);
        println!("{}", aver::run::report::fence(&out.marked.join("\n")));
    } else if !out.conflicts.is_empty() {
        println!(
            "Rebase onto {short} stopped: {} conflicting, nothing changed. Resolve them (merge it yourself), or keep the base you have.",
            out.conflicts.len()
        );
        println!("{}", aver::run::report::FENCE_NOTE);
        for c in &out.conflicts {
            let mut body = format!("{} ({:?})\n", c.path, c.kind);
            for h in &c.hunks {
                for side in &h.sides {
                    let who = if side.head == 0 { "yours" } else { "theirs" };
                    body.push_str(&format!("  {who}, lines {}:\n", side.lines.human()));
                    for line in &side.text {
                        body.push_str(&format!("    {line}\n"));
                    }
                }
            }
            println!("{}", aver::run::report::fence(&body));
        }
    } else if out.from == out.onto {
        println!(
            "{} is already on {short}; {} files changed",
            m.dir.display(),
            out.changed.len()
        );
    } else {
        println!(
            "Rebased {} onto {short}: wrote {} files from it; your changes are now {} files against the new base.",
            m.dir.display(),
            out.written.len(),
            out.changed.len()
        );
    }
    if !out.marked.is_empty() {
        bail!("rebase conflicts: resolve the marked files");
    }
    if !out.conflicts.is_empty() {
        bail!("rebase conflicts");
    }
    Ok(())
}

/// The workspace containing the current directory: its workspace file and
/// the home it lives in.
pub(super) struct Context {
    pub file: WorkspaceFile,
    pub home: Home,
    /// The workspace directory.
    pub root: PathBuf,
}

impl Context {
    /// The workspace containing the current directory, if any.
    pub(super) fn find() -> anyhow::Result<Option<Self>> {
        let Some((root, file)) = WorkspaceFile::find(&std::env::current_dir()?)? else {
            return Ok(None);
        };
        let home = Home::new(&file.home);
        Ok(Some(Self { file, home, root }))
    }

    /// The workspace containing `dir`, if any.
    pub(super) fn at(dir: &std::path::Path) -> anyhow::Result<Option<Self>> {
        let Some((root, file)) = WorkspaceFile::find(dir)? else {
            return Ok(None);
        };
        let home = Home::new(&file.home);
        Ok(Some(Self { file, home, root }))
    }

    pub(super) fn require(what: &str) -> anyhow::Result<Self> {
        Self::find()?.with_context(|| {
            format!("not inside an Aver workspace directory (no .aver/workspace.toml); {what}")
        })
    }

    /// The key that owns the workspace. In the directory of a run that was
    /// accepted or discarded, whose keys are revoked, the error says so and
    /// names `aver status <run>` rather than a missing key file.
    pub(super) fn key(&self) -> anyhow::Result<AgentKey> {
        let name = self
            .file
            .key
            .as_deref()
            .map(str::parse::<AgentName>)
            .transpose()?;
        key_in(&self.home, name.as_ref()).map_err(|e| self.finished_run(e.into()))
    }

    /// `e`, or why this directory of a finished run has no key.
    fn finished_run(&self, e: anyhow::Error) -> anyhow::Error {
        let Some(file) = aver::run::state::RunFile::read(&self.root) else {
            return e;
        };
        let Ok(run) = aver::run::state::RunState::load(&self.home, &file.run) else {
            return e;
        };
        let what = if run.discarded {
            "was discarded".to_owned()
        } else if let Some(d) = &run.decided {
            format!(
                "was accepted (attempt {}{})",
                d.index,
                d.branch
                    .as_deref()
                    .map(|b| format!(" as branch {b}"))
                    .unwrap_or_default()
            )
        } else {
            return e;
        };
        anyhow::anyhow!(
            "this directory is attempt {} of aver run {}, which {what}; its agents' keys are revoked, so nothing more can be verified or leased here. `aver status {}` shows the run, and `aver gc --accepted` removes this directory.",
            file.index,
            run.id,
            run.id
        )
    }
}

/// Names on stderr the files an import let through despite secret-like
/// content (test keys, fixtures, templates), never the content.
fn warn_secrets(import: &serde_json::Value) {
    let paths: Vec<String> = import["secrets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["path"].as_str().map(str::to_owned))
        .collect();
    if let Some(line) = aver::git::secrets_warning(&paths) {
        eprintln!("{line}");
    }
}

/// Refuses while conflict markers a rebase wrote remain in the workspace
/// directory `ctx` names (`aver check`, `aver verify`).
pub(super) fn refuse_unresolved(ctx: &Context) -> anyhow::Result<()> {
    let services = Services::open(&ctx.home)?;
    let left = services.workspaces().unresolved(&ctx.file.workspace)?;
    if !left.is_empty() {
        return Err(aver::workspace::rebase::unresolved_error(&left).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_new_warns_below_15_gib_free_and_names_gc() {
        let root = std::path::Path::new("/h");
        let line = low_disk_warning(Some(14 << 30), root).expect("warns");
        assert!(line.contains("14.0 GiB free under /h"), "{line}");
        assert!(line.contains("`aver gc`"), "{line}");
        assert_eq!(low_disk_warning(Some(15 << 30), root), None);
        assert_eq!(low_disk_warning(None, root), None);
    }
}
