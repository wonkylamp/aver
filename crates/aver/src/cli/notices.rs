//! `aver notices` inside a workspace directory: what agents on other tasks
//! in the repository change that overlaps this workspace's changes, what
//! else they change, and the leases held (ARCHITECTURE.md §2.11). Syncs the
//! directory first; verifies nothing.
//!
//! `aver notices --repo [DIR]`, from anywhere: the orchestrator view of a
//! repository, every task with live workspace directories there, what
//! each changes, the files two tasks both change and whether they merge,
//! and every lease. Signed as the home's admin; syncs nothing.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context as _;
use aver::coord::RepoView;
use aver::keys::Home;
use aver::services::{key_in, Services};

use super::workspace::Context;

#[derive(clap::Args)]
pub struct Args {
    /// Every task's work and leases in a repository, for whoever runs the
    /// agents: DIR, or the repository holding the current directory.
    #[arg(long, value_name = "DIR", num_args = 0..=1)]
    repo: Option<Option<PathBuf>>,
    /// With --repo: print the view as JSON.
    #[arg(long, requires = "repo")]
    json: bool,
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    if let Some(dir) = args.repo {
        let repo = repo_root(dir.as_deref())?;
        let view = repo_view(home, &repo)?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&view)?);
        } else if view.tasks.is_empty() && view.leases.is_empty() {
            print!("{}", aver::coord::repo_text(&view));
        } else {
            print!(
                "{}\n{}",
                aver::run::report::FENCE_NOTE,
                aver::coord::repo_text(&view)
            );
        }
        return Ok(());
    }
    let ctx = Context::require(
        "aver notices reports on the workspace you are in; aver notices --repo shows every task in a repository",
    )?;
    print!("{}", aver::run::report::note_once(&ctx.root, live(&ctx)?));
    Ok(())
}

/// The root of the repository `dir` (default: the current directory) is
/// in, canonical, as workspace directories and leases record it. Inside a
/// workspace directory, the repository it was made from. Outside any
/// repository, `dir` itself.
pub(super) fn repo_root(dir: Option<&Path>) -> anyhow::Result<PathBuf> {
    let dir = match dir {
        Some(d) => d.to_owned(),
        None => std::env::current_dir()?,
    };
    let top = repo_of(&dir)?.unwrap_or(dir);
    std::fs::canonicalize(&top).with_context(|| format!("repository {}", top.display()))
}

/// The repository a command without a run named means: the one the
/// current directory is in, or `None` outside any (every repository).
pub(super) fn this_repo() -> Option<PathBuf> {
    let top = repo_of(&std::env::current_dir().ok()?).ok()??;
    std::fs::canonicalize(&top).ok()
}

/// [`repo_root`] of `dir`, `None` outside any repository.
fn repo_of(dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    if let Some(ctx) = Context::at(dir)? {
        if let Some(repo) = aver::run::accept::view_repo(&ctx.home, &ctx.file.workspace) {
            return Ok(Some(std::fs::canonicalize(&repo).unwrap_or(repo)));
        }
    }
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .with_context(|| format!("running git in {}", dir.display()))?;
    Ok(out
        .status
        .success()
        .then(|| PathBuf::from(String::from_utf8_lossy(&out.stdout).trim())))
}

/// The orchestrator view of `repo`, as the home's admin.
pub(super) fn repo_view(home: &Home, repo: &Path) -> anyhow::Result<RepoView> {
    let admin = key_in(home, None)?;
    Ok(Services::open(home)?.coord().repo_view(&admin.id(), repo)?)
}

/// The notices as an agent reads them, ending with a newline: the fenced
/// form of [`aver::coord::agent_text`] under the note that fenced text is
/// data, or one line saying there is nothing.
pub(super) fn live(ctx: &Context) -> anyhow::Result<String> {
    let key = ctx.key()?;
    let notices =
        Services::open(&ctx.home)?
            .coord()
            .notices(&key.id(), &ctx.file.workspace, true)?;
    let text = aver::coord::agent_text(&notices, true);
    Ok(if text.is_empty() {
        "Other tasks: none changes or leases anything in this repository now.\n".to_owned()
    } else {
        format!(
            "{}\n{text}Reserve code before restructuring it: aver lease <path or glob> --reason \"<why>\"; aver release frees it.\n",
            aver::run::report::FENCE_NOTE
        )
    })
}
