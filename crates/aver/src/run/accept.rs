//! `aver accept` and `aver discard` (§2.10): turning a run's chosen
//! attempt into an ordinary Git branch, and cleaning a run up.
//!
//! Accept records the decision (`Tasks::decide`: attempt records and one
//! ledger entry, no ref moved), commits the winner's workspace, and
//! exports it as a new branch whose parent is the real base commit. It
//! never touches the user's working tree or current branch: the export
//! writes objects and one new ref through `git fast-import`.

use std::path::Path;
use std::process::Command;

use aver_core::{ReceiptId, Verdict};

use super::api::{Api, ApiError};
use super::compare::{self, Compared};
use super::state::{self, Accepting, AgentState, Decision, RunState};
use super::supervisor::RunError;
use crate::keys::{AgentKey, Home};
use crate::tasks;

#[derive(Debug, Clone, Default)]
pub struct AcceptOptions {
    /// The branch to create (default `aver/<slug of the title>`).
    pub branch: Option<String>,
    /// Accept an attempt whose latest receipt is not `PASS`.
    pub force: bool,
    /// Keep the other attempts' directories.
    pub keep_losers: bool,
    /// Keep the accepted directory itself (by default it is released once
    /// the branch is written).
    pub keep: bool,
    /// Accept only this snapshot: the head a combine merged and verified.
    /// A directory or receipt at any other is refused before anything is
    /// verified, decided or committed.
    pub expect_head: Option<aver_core::SnapshotId>,
    /// A person approved a `REVIEW` verdict (`--approve-review`, which the
    /// CLI refuses to an agent: [`refuse_agent_approval`]). Without it a
    /// `REVIEW` is refused: a changed test expectation is approved by a
    /// person, never by the agent that changed it or by a command that
    /// builds on it. The approval covers only the receipt this accept
    /// verifies, for the snapshot it exports.
    pub approve_review: bool,
    /// The commit title (`--title`); see [`commit_title`] for the default.
    pub title: Option<String>,
    /// Called with the directory after the last check that it is the
    /// receipt's snapshot and before the commit: tests stand in for an
    /// edit that lands there.
    #[doc(hidden)]
    pub before_commit: Option<fn(&Path)>,
}

/// Why `what` (an attempt, a task) is not accepted on the user's behalf:
/// it is `REVIEW`, which a person approves.
pub fn needs_review(what: &str) -> String {
    format!(
        "{what} is REVIEW: it changes what an existing test expects, which only a person approves"
    )
}

/// The environment variables that mark a process as an agent's: Aver's
/// own (`aver run` sets both in its agents' environment) and the ones
/// Claude Code (`CLAUDECODE`) and Codex (`CODEX_SANDBOX`) set in every
/// command they run. Set and non-empty, any of them refuses
/// `--approve-review` (docs/CLI.md).
pub const AGENT_ENV: &[&str] = &["AVER_RUN", "AVER_WORKSPACE", "CLAUDECODE", "CODEX_SANDBOX"];

/// The first of [`AGENT_ENV`] set in this process's environment.
pub fn agent_marker() -> Option<&'static str> {
    AGENT_ENV
        .iter()
        .copied()
        .find(|v| std::env::var_os(v).is_some_and(|s| !s.is_empty()))
}

/// Refuses `--approve-review` to an agent: only a person approves a
/// `REVIEW`. `human` is the command the person runs in their own terminal.
/// Stdin must be a terminal too; `AVER_TEST_HUMAN=1` stands in for one in
/// tests and scripts, never under an agent's environment.
pub fn refuse_agent_approval(human: &str) -> Result<(), RunError> {
    use std::io::IsTerminal;
    let refused = |why: String| {
        Err(RunError::Invalid(format!(
            "a human must approve a REVIEW: {why}, and an agent cannot approve the test changes it made. A person reviews them (`aver diff`) and runs `{human}` in their own terminal, or approves the pull request. A person approves from an interactive terminal"
        )))
    };
    if let Some(var) = agent_marker() {
        return refused(format!("{var} is set, so this command runs under an agent"));
    }
    if std::io::stdin().is_terminal()
        || std::env::var_os("AVER_TEST_HUMAN").is_some_and(|v| v == "1")
    {
        return Ok(());
    }
    refused("stdin is not a terminal, so no person is at this command".to_string())
}

/// Why a `REVIEW` is refused without `--approve-review`; `human` is the
/// command that approves it.
fn review_refused(what: &str, receipt: &ReceiptId, human: &str) -> RunError {
    RunError::Invalid(format!(
        "{} (receipt {receipt}); nothing was exported. A person reviews the changed tests (`aver diff`) and runs `{human}` to approve them",
        needs_review(what)
    ))
}

/// What became of the accepted directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cleanup {
    /// Released: moved to the trash, its workspace's materialization
    /// `released`.
    Released,
    /// Kept because `--keep` asked.
    Kept,
    /// Kept because removing it could lose work, or failed: why.
    KeptBecause(String),
}

impl Cleanup {
    /// One line saying what happened to directory `dir`.
    pub fn describe(&self, dir: &Path) -> String {
        match self {
            Self::Released => format!("Removed its directory {}.", dir.display()),
            Self::Kept => format!(
                "Kept its directory {} (--keep); aver gc --accepted removes it.",
                dir.display()
            ),
            Self::KeptBecause(why) => format!("Kept its directory {}: {why}", dir.display()),
        }
    }
}

/// Releases the accepted workspace `ws` (directory `dir`) unless `keep`,
/// or unless its Git view holds commits the user's repository lacks, or
/// cannot be checked, or a file in it changed at or after `committed_at`
/// (the filesystem's clock just before the commit): removing it then could
/// lose work, so it stays and the answer says why.
pub async fn clean_up(
    api: &Api,
    home: &Home,
    ws: &aver_core::WorkspaceId,
    dir: &Path,
    keep: bool,
    committed_at: Option<i64>,
) -> Cleanup {
    if keep {
        return Cleanup::Kept;
    }
    if let Some(why) = unfetched_reason(home, ws, dir) {
        return Cleanup::KeptBecause(why);
    }
    if let Some(path) = committed_at.and_then(|t| changed_since(dir, t)) {
        return Cleanup::KeptBecause(format!(
            "{path} changed after the commit, so the branch does not hold it"
        ));
    }
    match api.release(ws).await {
        Ok(()) => Cleanup::Released,
        // The directory changed after the commit: the message says what
        // and how to remove it anyway.
        Err(ApiError::Refused { code, message, .. }) if code == "DIR_CHANGED" => {
            Cleanup::KeptBecause(message)
        }
        Err(e) => Cleanup::KeptBecause(format!("releasing it failed: {e}")),
    }
}

/// The first file or directory under `dir` (outside `.aver` and `.git`,
/// and what `.gitignore` leaves out) whose change time is at or after
/// `since`, in nanoseconds of the filesystem's clock; `None` when nothing
/// changed. A closed workspace no longer syncs, so its files are compared
/// with the moment of the commit instead of with its tree.
fn changed_since(dir: &Path, since: i64) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let walk = ignore::WalkBuilder::new(dir)
        .hidden(false)
        .require_git(false)
        .filter_entry(|e| {
            e.depth() != 1 || !matches!(e.file_name().to_str(), Some(".aver" | ".git"))
        })
        .build();
    for entry in walk.flatten() {
        if entry.depth() == 0 {
            continue;
        }
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        if meta.ctime() * 1_000_000_000 + meta.ctime_nsec() >= since {
            return Some(
                entry
                    .path()
                    .strip_prefix(dir)
                    .unwrap_or(entry.path())
                    .display()
                    .to_string(),
            );
        }
    }
    None
}

/// The filesystem's clock beside `dir`, for [`changed_since`]; `None` when
/// it cannot be read, which keeps no directory on its account.
fn fs_now(dir: &Path) -> Option<i64> {
    crate::disk::scan::fs_clock(dir.parent()?).ok()
}

/// Why workspace `ws`'s directory must stay: its Git view holds commits
/// the user's repository does not have, or cannot be checked. `None` when
/// it has no view or every commit is fetched.
pub fn unfetched_reason(home: &Home, ws: &aver_core::WorkspaceId, dir: &Path) -> Option<String> {
    let view = crate::disk::gitview::view_dir(home.root(), ws);
    if !view.is_dir() {
        return None;
    }
    match crate::disk::gitview::unfetched(&view) {
        Ok(lost) if lost.is_empty() => None,
        Ok(lost) => {
            let tips: Vec<String> = lost
                .iter()
                .map(|u| {
                    format!(
                        "{} at {}",
                        u.refname.strip_prefix("refs/heads/").unwrap_or(&u.refname),
                        &u.sha[..12.min(u.sha.len())]
                    )
                })
                .collect();
            Some(format!(
                "its Git view has {} the repository does not ({}); `aver workspace rm {}` says how to fetch {} before removing it",
                if lost.len() == 1 { "a commit" } else { "commits" },
                tips.join(", "),
                dir.display(),
                if lost.len() == 1 { "it" } else { "them" },
            ))
        }
        Err(e) => Some(format!(
            "its Git view could not be checked for unfetched commits ({e})"
        )),
    }
}

#[derive(Debug, Clone)]
pub struct Accepted {
    pub index: u32,
    pub branch: String,
    pub commit: String,
    pub receipt: ReceiptId,
    pub verdict: Verdict,
    /// The commit's title.
    pub title: String,
    /// The receipt in a few lines.
    pub summary: String,
    /// The evidence block of `summary.md` the commit message carries.
    pub evidence: String,
    /// The `REVIEW` receipt a person approved, when it was one.
    pub approved: Option<ReceiptId>,
    /// Commits on the user's current branch since the base.
    pub moved: Option<u32>,
    pub compared: Vec<Compared>,
    /// Other accepted runs on the same base this one could be combined with.
    pub combinable: Vec<super::combine::Candidate>,
    /// What became of the winner's directory.
    pub cleanup: Cleanup,
}

/// The longest title slug a default branch carries.
const SLUG_MAX: usize = 40;

/// `text` in lowercase ASCII words joined by `-`, cut at a word boundary
/// to at most [`SLUG_MAX`] bytes (a first word longer than that is cut).
fn slug(text: &str) -> String {
    let words: Vec<String> = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let mut slug = String::new();
    for word in &words {
        let sep = usize::from(!slug.is_empty());
        if slug.len() + sep + word.len() > SLUG_MAX {
            break;
        }
        if sep == 1 {
            slug.push('-');
        }
        slug.push_str(word);
    }
    if slug.is_empty() {
        if let Some(first) = words.first() {
            slug = first[..first.len().min(SLUG_MAX)].to_owned();
        }
    }
    slug
}

/// `aver/<slug>`: the title in lowercase ASCII words joined by `-`. A title
/// longer than [`SLUG_MAX`] is cut at a word boundary and gets the run id,
/// since two long titles cut short can collide. What `aver combine` names
/// its branch.
pub fn default_branch(title: &str, run: &str) -> String {
    let run = run.to_lowercase();
    let full: Vec<String> = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    if full.is_empty() {
        return format!("aver/run-{run}");
    }
    let full = full.join("-");
    if full.len() <= SLUG_MAX {
        return format!("aver/{full}");
    }
    format!("aver/{}-{run}", slug(title))
}

/// `aver/<slug of title>-<id>`, where `id` is the last four characters of
/// `id` in lowercase: an accept's branch, which two runs or directories
/// with one title (in two batches, say) do not share. A title with no
/// letter or digit gives `fallback`.
pub fn accept_branch(title: &str, id: &str, fallback: &str) -> String {
    let slug = slug(title);
    if slug.is_empty() {
        return fallback.to_owned();
    }
    let short = id[id.len().saturating_sub(4)..].to_ascii_lowercase();
    format!("aver/{slug}-{short}")
}

/// A run's default branch: `aver/<slug>-<id>` from its task's title, or
/// `aver/issue-<n>-<slug>-<id>` for a task that came from a GitHub issue.
/// The task's title rather than a commit an agent made: `aver combine`
/// names the branch an accept of the run writes before it accepts.
pub fn branch_for(run: &RunState) -> String {
    let fallback = format!("aver/run-{}", run.id.to_lowercase());
    match run.batch.as_ref().and_then(|b| b.issue) {
        Some(n) => accept_branch(&format!("issue {n} {}", run.title), &run.id, &fallback),
        None => accept_branch(&run.title, &run.id, &fallback),
    }
}

/// Longest commit title taken from an agent's commit.
const SUBJECT_MAX: usize = 100;

/// The subject of the agent's latest commit in workspace `ws`'s Git view
/// since `base`, as a commit title: agent text, so one line with control
/// characters escaped and at most [`SUBJECT_MAX`] characters. `None` when
/// the agent committed nothing (or has no view).
pub fn agent_subject(home: &Home, ws: &aver_core::WorkspaceId, base: &str) -> Option<String> {
    let view = crate::disk::gitview::view_dir(home.root(), ws);
    let subject = crate::disk::gitview::latest_subject(&view, base).ok()??;
    let mut out = String::new();
    for c in subject.trim().chars() {
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    if out.chars().count() > SUBJECT_MAX {
        out = out.chars().take(SUBJECT_MAX - 3).collect::<String>() + "...";
    }
    (!out.trim().is_empty()).then_some(out)
}

/// Whether `title` is one Aver made up for a task nobody named: `aver
/// workspace new`'s `Work on <repo> at <commit>` or the worktree hook's
/// `Claude Code worktree <name>`.
fn made_up(title: &str) -> bool {
    (title.starts_with("Work on ") && title.contains(" at "))
        || title.starts_with("Claude Code worktree ")
}

/// An accept's commit title, the first of: `--title`; the subject of the
/// agent's latest commit (`committed`); the task's title unless Aver made
/// it up; the first criterion's text; the made-up title.
pub fn commit_title(
    flag: Option<&str>,
    committed: Option<String>,
    task: &str,
    first_criterion: Option<&str>,
) -> String {
    let flag = flag.map(str::trim).filter(|t| !t.is_empty());
    if let Some(t) = flag {
        return t.to_owned();
    }
    if let Some(t) = committed {
        return t;
    }
    if !made_up(task) && !task.trim().is_empty() {
        return task.to_owned();
    }
    first_criterion.map_or_else(|| task.to_owned(), str::to_owned)
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Refuses a branch that exists or that a worktree has checked out. With
/// `adopt`, a branch whose tip is an export of that snapshot (its
/// `Aver-Snapshot:` trailer) passes: an accept or combine run again after
/// it wrote the branch. The export itself then checks that the tip is the
/// very commit it writes.
pub fn check_branch(
    repo: &Path,
    branch: &str,
    adopt: Option<&aver_core::SnapshotId>,
) -> Result<(), RunError> {
    let full = format!("refs/heads/{branch}");
    if git(repo, &["check-ref-format", &full]).is_none() {
        return Err(RunError::Invalid(format!(
            "{branch:?} is not a valid branch name"
        )));
    }
    let checked_out = git(repo, &["worktree", "list", "--porcelain"])
        .unwrap_or_default()
        .lines()
        .any(|l| l.strip_prefix("branch ") == Some(full.as_str()));
    if checked_out {
        return Err(RunError::Invalid(format!(
            "branch {branch} is checked out; Aver never writes to a checked-out branch. Pass --branch <new name>"
        )));
    }
    if git(repo, &["rev-parse", "--verify", "--quiet", &full]).is_some() {
        if let Some(snapshot) = adopt {
            let trailer = format!("Aver-Snapshot: {snapshot}");
            let ours = git(repo, &["log", "-1", "--format=%B", &full])
                .is_some_and(|m| m.lines().any(|l| l.trim() == trailer));
            if ours {
                return Ok(());
            }
        }
        return Err(RunError::Invalid(format!(
            "branch {branch} already exists; pass --branch <new name>"
        )));
    }
    Ok(())
}

fn summary(c: &Compared, run: &RunState) -> String {
    let verdict = c.verdict.map_or("none", |v| v.as_str());
    let mut out = format!(
        "Verdict {verdict}: {}{} new, {} preexisting.",
        if c.criteria > 0 {
            format!(
                "{}/{} criteria proven, ",
                c.criteria - c.unproven.len() as u32,
                c.criteria
            )
        } else {
            String::new()
        },
        c.new,
        c.preexisting
    );
    if !c.unproven.is_empty() {
        let ids: Vec<&str> = c.unproven.iter().map(|(id, _)| id.as_str()).collect();
        out.push_str(&format!("\nUnproven: {}.", ids.join(", ")));
    }
    out.push_str(&format!(
        "\nChosen from {} {} by aver run {}.",
        run.agents.len(),
        if run.agents.len() == 1 {
            "attempt"
        } else {
            "attempts"
        },
        run.id
    ));
    out
}

/// Accepts agent `n` (an index or attempt id) of `run`.
pub async fn accept(
    api: &Api,
    home: &Home,
    admin: &AgentKey,
    run: &mut RunState,
    n: &str,
    opts: &AcceptOptions,
) -> Result<Accepted, RunError> {
    if run.discarded {
        return Err(RunError::Invalid(format!("run {} was discarded", run.id)));
    }
    let agent = run
        .agent(n)
        .ok_or_else(|| RunError::Invalid(format!("run {} has no attempt {n}", run.id)))?
        .clone();
    if let Some(d) = &run.decided {
        return Err(RunError::Invalid(format!(
            "run {} was already accepted: attempt {} as {}",
            run.id,
            d.index,
            d.branch.as_deref().unwrap_or("-")
        )));
    }
    let key = compare::agent_key(home, &agent)
        .ok_or_else(|| RunError::Invalid(format!("the key of attempt {} is gone", agent.index)))?;
    // An accept of this attempt that stopped part way: run again, it
    // finishes the same branch.
    let journal = Accepting::load(home, &run.id).filter(|j| j.attempt == agent.attempt);
    let branch = opts
        .branch
        .clone()
        .or_else(|| journal.as_ref().map(|j| j.branch.clone()))
        .unwrap_or_else(|| branch_for(run));
    // The winner's receipt must describe its directory as it is now.
    let current = api.snapshot(&key.signing_key, &agent.workspace).await?;
    check_branch(&run.repo, &branch, Some(&current))?;
    let identity = crate::git::user_identity(&run.repo).map_err(RunError::Invalid)?;
    if opts.expect_head.is_some_and(|h| h != current) {
        return Err(moved(&format!("attempt {}", agent.index)));
    }
    let attempt = api.attempt(&admin.signing_key, &agent.attempt).await?;
    let fresh = match attempt.receipts.last() {
        Some(id) => api
            .receipt(id)
            .await
            .is_ok_and(|v| v.receipt.body.head_snapshot == current),
        None => false,
    };
    if !fresh {
        api.verify(&key.signing_key, &run.task, &agent.workspace, false)
            .await?;
    }
    let compared = compare::gather(api, home, &admin.signing_key, run).await;
    let winner = compared
        .iter()
        .find(|c| c.agent.attempt == agent.attempt)
        .expect("the winner is one of the run's attempts")
        .clone();
    let verdict = winner.verdict.unwrap_or(Verdict::Unproven);
    if winner.diff.is_some_and(|d| d.0 == 0) {
        return Err(RunError::Invalid(format!(
            "attempt {} changed nothing",
            agent.index
        )));
    }
    if !matches!(verdict, Verdict::Pass | Verdict::Review) && !opts.force {
        return Err(RunError::Invalid(format!(
            "attempt {} is {}, not PASS or REVIEW; pass --force to accept it anyway",
            agent.index,
            verdict.as_str()
        )));
    }
    let receipt = winner.receipt.expect("verified above");
    // What is exported is exactly what the receipt describes: the
    // directory must still be that snapshot now, and the commit refuses
    // any other.
    let verification = api.receipt(&receipt).await?;
    let reviewed = verification.receipt.body.head_snapshot;
    let again = run.batch.as_ref().map_or(agent.index, |b| b.number);
    let approved = approval(
        verdict,
        receipt,
        reviewed,
        opts.approve_review,
        journal
            .as_ref()
            .map(|j| (j.approved, j.receipt, j.snapshot)),
    )
    .map_err(|()| {
        review_refused(
            &format!("attempt {}", agent.index),
            &receipt,
            &format!("aver accept {again} --approve-review"),
        )
    })?;
    let scope = super::limits::scope(
        &verification.receipt.body,
        verification.diagnostics.as_ref(),
        None,
    );
    let what = format!("attempt {}", agent.index);
    still_reviewed(api, &key, &agent.workspace, &reviewed, &what).await?;
    let ranking: Vec<_> = compare::ranked(&compared)
        .into_iter()
        .map(|(r, _)| r)
        .collect();
    let lessons = compared
        .iter()
        .filter_map(|c| {
            c.agent.gave_up.as_ref().map(|why| {
                format!(
                    "{}: gave up: {}",
                    c.agent.attempt,
                    super::report::one_line_plain(why)
                )
            })
        })
        .collect();
    let verdict_lines = summary(&winner, run);
    let summary = format!("{verdict_lines}\n{}", super::limits::commit_lines(&scope));
    let title = commit_title(
        opts.title.as_deref(),
        agent_subject(home, &agent.workspace, &run.base_commit),
        &run.title,
        None,
    );
    let evidence = crate::ci::summary::evidence(
        &verification.receipt.body,
        verification.diagnostics.as_ref(),
    );
    let message = format!(
        "{title}\n\n{verdict_lines}\n\n{evidence}\n\nAver-Task: {}\nAver-Attempt: {}\nAver-Agent: {}\nAver-Receipt: {receipt}\n{}",
        run.task,
        agent.attempt,
        agent_label(run, &agent),
        approved_trailer(approved),
    );
    let (mut journal, fresh) = journal_for(journal, &agent, &branch, receipt, reviewed, approved);
    journal.save(home, &run.id)?;
    crate::fault::point("accept.journaled")?;
    if let Some(hook) = opts.before_commit {
        hook(&agent.dir);
    }
    // Commit before deciding: a directory that moved fails the commit while
    // the attempt is still open, so the next accept can verify it again. The
    // committed head is the reviewed snapshot, which decide then checks.
    let (committed, now) =
        commit_reviewed(api, &key, &agent.workspace, &message, &reviewed, &what).await?;
    let unknown = fresh && !now;
    step(home, run, &mut journal, "committed")?;
    crate::fault::point("accept.committed")?;
    let decided = api
        .decide(
            &admin.signing_key,
            &run.task,
            &tasks::Decision {
                winner: agent.attempt,
                ranking,
                rationale: None,
                lessons,
            },
        )
        .await?;
    step(home, run, &mut journal, "decided")?;
    crate::fault::point("accept.decided")?;
    let commit = api
        .export_reviewed(&committed, &receipt, &run.repo, &branch, Some(&identity))
        .await?;
    let released_before = journal.step == "released";
    step(home, run, &mut journal, "exported")?;
    for other in &run.agents {
        if other.attempt == agent.attempt || opts.keep_losers {
            continue;
        }
        let _ = api.release(&other.workspace).await;
    }
    let cleanup = if released_before {
        Cleanup::Released
    } else if unknown && !opts.keep {
        Cleanup::KeptBecause(COMMITTED_EARLIER.into())
    } else {
        let since = journal.clock;
        clean_up(api, home, &agent.workspace, &agent.dir, opts.keep, since).await
    };
    if cleanup == Cleanup::Released {
        step(home, run, &mut journal, "released")?;
    }
    crate::fault::point("accept.released")?;
    let moved = git(
        &run.repo,
        &["rev-list", "--count", &format!("{}..HEAD", run.base_commit)],
    )
    .and_then(|n| n.parse().ok())
    .filter(|n: &u32| *n > 0);
    let decision = Decision {
        winner: agent.attempt,
        index: agent.index,
        branch: Some(branch.clone()),
        commit: Some(commit.clone()),
        ledger: decided.ledger,
        pr: None,
        at: aver_core::Timestamp::now(),
        head: Some(committed),
        verdict: Some(verdict),
    };
    *run = RunState::update(home, &run.id, |r| r.decided = Some(decision))?;
    Accepting::clear(home, &run.id);
    // Only once the decision is saved: an accept run again before that
    // needs the winner's key to finish.
    revoke_keys(api, home, run).await;
    crate::fault::point("accept.saved")?;
    let combinable = super::combine::candidates(api, home, admin, run).await;
    Ok(Accepted {
        index: agent.index,
        branch,
        commit,
        receipt,
        verdict,
        title,
        summary,
        evidence,
        approved,
        moved,
        compared,
        combinable,
        cleanup,
    })
}

/// Whether `verdict` (of `receipt`, for the `reviewed` snapshot) may be
/// accepted: a `REVIEW` only with a person's approval, which is
/// `approve_review` now or the one an earlier accept journaled for this
/// very receipt and snapshot (`earlier`: its approval, receipt and
/// snapshot). An approval never carries to another receipt: a directory
/// changed since is verified again and approved again. Returns the
/// approved receipt, `None` for a `PASS`; `Err` when a `REVIEW` has no
/// approval.
fn approval(
    verdict: Verdict,
    receipt: ReceiptId,
    reviewed: aver_core::SnapshotId,
    approve_review: bool,
    earlier: Option<(Option<ReceiptId>, ReceiptId, aver_core::SnapshotId)>,
) -> Result<Option<ReceiptId>, ()> {
    if verdict != Verdict::Review {
        return Ok(None);
    }
    let journaled = earlier.is_some_and(|(approved, r, snapshot)| {
        approved == Some(receipt) && r == receipt && snapshot == reviewed
    });
    if approve_review || journaled {
        Ok(Some(receipt))
    } else {
        Err(())
    }
}

/// The commit trailer naming the approved `REVIEW` receipt, with its line
/// break; empty for none.
fn approved_trailer(approved: Option<ReceiptId>) -> String {
    approved
        .map(|r| format!("Aver-Approved-Review: {r}\n"))
        .unwrap_or_default()
}

/// The journal of accepting `agent` as `branch` at `reviewed`: the earlier
/// one when it is of the same branch and snapshot (its step and clock
/// carry over, since its commit may have happened), else a fresh one.
fn journal_for(
    earlier: Option<Accepting>,
    agent: &super::state::AgentRun,
    branch: &str,
    receipt: ReceiptId,
    reviewed: aver_core::SnapshotId,
    approved: Option<ReceiptId>,
) -> (Accepting, bool) {
    match earlier {
        Some(mut j) if j.branch == branch && j.snapshot == reviewed && j.receipt == receipt => {
            j.approved = approved;
            (j, false)
        }
        _ => (
            Accepting {
                attempt: agent.attempt,
                index: agent.index,
                branch: branch.to_owned(),
                receipt,
                snapshot: reviewed,
                step: "journaled".into(),
                clock: fs_now(&agent.dir),
                approved,
            },
            true,
        ),
    }
}

/// Why a directory committed before this accept, with no journal of when,
/// is kept: a change made since cannot be told from the branch.
const COMMITTED_EARLIER: &str = "an earlier accept committed it, so a change made since cannot be told from the branch; `aver workspace rm` removes it";

/// Records that `journal` reached `name`.
fn step(home: &Home, run: &RunState, journal: &mut Accepting, name: &str) -> Result<(), RunError> {
    if journal.step == name {
        return Ok(());
    }
    journal.step = name.to_owned();
    journal.save(home, &run.id)?;
    Ok(())
}

/// A workspace directory made outside `aver run` (`aver workspace new`, a
/// `claude --worktree` session), accepted as a branch of the repository its
/// Git view borrows from.
#[derive(Debug, Clone)]
pub struct WorkspaceAccepted {
    /// The user's repository the branch was written to.
    pub repo: std::path::PathBuf,
    pub branch: String,
    pub commit: String,
    pub receipt: ReceiptId,
    pub verdict: Verdict,
    /// The commit's title.
    pub title: String,
    /// The receipt in a few lines.
    pub summary: String,
    /// The evidence block of `summary.md` the commit message carries.
    pub evidence: String,
    /// The `REVIEW` receipt a person approved, when it was one.
    pub approved: Option<ReceiptId>,
    /// Commits on the user's current branch since the base.
    pub moved: Option<u32>,
    /// Whether the accept ran a verify because none described the
    /// directory as it is now.
    pub verified: bool,
    /// What became of the directory.
    pub cleanup: Cleanup,
    /// The test behind each claimed criterion (repository content).
    pub proofs: Vec<crate::diagnostics::ProofSource>,
}

/// What a workspace directory's accept needs: the directory, its workspace
/// file, the key that owns it and the home's admin key.
pub struct WorkspaceToAccept<'a> {
    pub dir: &'a Path,
    pub file: &'a crate::workspace::WorkspaceFile,
    pub owner: &'a AgentKey,
    pub admin: &'a AgentKey,
    /// The repository to write the branch to (default: the one the
    /// directory's Git view borrows its objects from).
    pub repo: Option<&'a Path>,
}

/// The user's repository behind workspace `ws`: the working tree of the
/// Git directory its Git view borrows objects from.
pub fn view_repo(home: &Home, ws: &aver_core::WorkspaceId) -> Option<std::path::PathBuf> {
    let view = crate::disk::gitview::view_dir(home.root(), ws);
    let gitdir = crate::disk::gitview::source(&view).ok()?;
    match gitdir.file_name() {
        Some(n) if n == ".git" => gitdir.parent().map(Path::to_owned),
        _ => Some(gitdir),
    }
}

fn workspace_summary(receipt: &aver_core::Receipt) -> String {
    let body = &receipt.body;
    let criteria = body.criteria.as_slice().len();
    let count = |cause: aver_core::Cause| {
        body.checks
            .iter()
            .filter(|c| c.cause == Some(cause))
            .count()
    };
    let mut out = format!(
        "Verdict {}: {}{} new, {} preexisting.",
        body.verdict.as_str(),
        if criteria > 0 {
            format!(
                "{}/{criteria} criteria proven, ",
                criteria - body.unproven.len()
            )
        } else {
            String::new()
        },
        count(aver_core::Cause::New),
        count(aver_core::Cause::Preexisting),
    );
    if !body.unproven.is_empty() {
        let ids: Vec<&str> = body.unproven.iter().map(|u| u.as_str()).collect();
        out.push_str(&format!("\nUnproven: {}.", ids.join(", ")));
    }
    out.push_str("\nAccepted from an Aver workspace directory.");
    out
}

/// The commit title and default branch of a solo accept. The title is
/// [`commit_title`]'s: `flag` (`--title`), the subject of the agent's last
/// commit (`committed`), the task's title unless Aver made it up, or its
/// first criterion's text (`aver workspace new -c`). The branch is the
/// title's slug with a short id from the workspace ([`accept_branch`]),
/// which two directories with one title cannot share; with nothing but a
/// made-up title, the branch the directory's Git view shows, `aver/<dir>`.
pub fn solo_names(
    task: Option<&aver_core::TaskBody>,
    ws: &aver_core::WorkspaceId,
    label: &str,
    flag: Option<&str>,
    committed: Option<String>,
) -> (String, String) {
    let view = crate::disk::gitview::branch_for(label);
    let (task_title, first) = match task {
        Some(t) => (
            t.title.as_str(),
            t.criteria.iter().next().map(|c| c.text.as_str()),
        ),
        None => (label, None),
    };
    let title = commit_title(flag, committed, task_title, first);
    let branch = if made_up(&title) {
        view
    } else {
        accept_branch(&title, &ws.to_string(), &view)
    };
    (title, branch)
}

/// Accepts a workspace directory made outside `aver run`: the solo agent's
/// hand-off. As [`accept`] does for a run's winner, it verifies the
/// directory first when no receipt describes it as it is now, refuses
/// anything but `PASS` or `REVIEW` without `force` and a `REVIEW` without a
/// person's approval, records the decision, commits the workspace (which
/// closes it) and exports it as a new branch whose parent is the base
/// commit, as the repository's Git identity, named by [`solo_names`].
pub async fn accept_workspace(
    api: &Api,
    home: &Home,
    ws: &WorkspaceToAccept<'_>,
    opts: &AcceptOptions,
) -> Result<WorkspaceAccepted, RunError> {
    let file = ws.file;
    let (Some(task), Some(attempt_id)) = (file.task, file.attempt) else {
        return Err(RunError::Invalid(
            "this workspace belongs to no task, so there is nothing to accept; export a snapshot with aver export instead".into(),
        ));
    };
    let repo = match ws.repo {
        Some(r) => r.to_owned(),
        None => view_repo(home, &file.workspace).ok_or_else(|| {
            RunError::Invalid(
                "this workspace has no Git view naming its repository; pass --repo <path>".into(),
            )
        })?,
    };
    let label = ws
        .dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let body = api.task(&task).await.ok().map(|t| t.body);
    let committed = crate::git::imported_commit(&api.services().store, &file.base)
        .ok()
        .flatten()
        .and_then(|base| agent_subject(home, &file.workspace, base.as_str()));
    let (title, named) = solo_names(
        body.as_ref(),
        &file.workspace,
        &label,
        opts.title.as_deref(),
        committed,
    );
    let branch = opts.branch.clone().unwrap_or(named);
    let current = api.snapshot(&ws.owner.signing_key, &file.workspace).await?;
    if current == file.base {
        return Err(RunError::Invalid(
            "this directory changed nothing; there is nothing to accept".into(),
        ));
    }
    let earlier = WorkspaceJournal::load(home, &file.workspace)
        .filter(|j| j.snapshot == current && j.repo == repo);
    let branch = match (&opts.branch, &earlier) {
        (None, Some(j)) => j.branch.clone(),
        _ => branch,
    };
    check_branch(&repo, &branch, Some(&current))?;
    let identity = crate::git::user_identity(&repo).map_err(RunError::Invalid)?;
    let attempt = api.attempt(&ws.admin.signing_key, &attempt_id).await?;
    let latest = match attempt.receipts.last() {
        Some(id) => Some(api.receipt(id).await?),
        None => None,
    };
    let (verification, verified) = match latest {
        Some(v) if v.receipt.body.head_snapshot == current => (v, false),
        _ => (
            api.verify(&ws.owner.signing_key, &task, &file.workspace, false)
                .await?,
            true,
        ),
    };
    let verdict = verification.receipt.body.verdict;
    if !matches!(verdict, Verdict::Pass | Verdict::Review) && !opts.force {
        return Err(RunError::Invalid(format!(
            "the directory's verdict is {}, not PASS or REVIEW (`aver verify` says what is left); pass --force to accept it anyway",
            verdict.as_str()
        )));
    }
    let receipt = verification.receipt_id;
    let reviewed = verification.receipt.body.head_snapshot;
    let approved = approval(
        verdict,
        receipt,
        reviewed,
        opts.approve_review,
        earlier
            .as_ref()
            .map(|j| (j.approved, j.receipt, j.snapshot)),
    )
    .map_err(|()| {
        review_refused(
            "the directory",
            &receipt,
            &format!("aver accept --approve-review in {}", ws.dir.display()),
        )
    })?;
    still_reviewed(api, ws.owner, &file.workspace, &reviewed, "the directory").await?;
    let summary = format!(
        "{}\n{}",
        workspace_summary(&verification.receipt),
        super::limits::commit_lines(&super::limits::scope(
            &verification.receipt.body,
            verification.diagnostics.as_ref(),
            verification.timing.as_ref(),
        ))
    );
    let proofs = verification
        .diagnostics
        .as_ref()
        .map(|d| d.proof_sources.clone())
        .unwrap_or_default();
    let evidence = crate::ci::summary::evidence(
        &verification.receipt.body,
        verification.diagnostics.as_ref(),
    );
    let message = format!(
        "{title}\n\n{}\n\n{evidence}\n\nAver-Task: {task}\nAver-Attempt: {attempt_id}\nAver-Receipt: {receipt}\n{}",
        workspace_summary(&verification.receipt),
        approved_trailer(approved),
    );
    let (mut journal, fresh) = match earlier {
        Some(mut j) if j.branch == branch && j.receipt == receipt => {
            j.approved = approved;
            (j, false)
        }
        _ => (
            WorkspaceJournal {
                workspace: file.workspace,
                dir: ws.dir.to_owned(),
                repo: repo.clone(),
                branch: branch.clone(),
                receipt,
                snapshot: reviewed,
                verdict,
                summary: String::new(),
                commit: None,
                step: "journaled".into(),
                clock: fs_now(ws.dir),
                approved,
            },
            true,
        ),
    };
    journal.save(home, None)?;
    crate::fault::point("accept.journaled")?;
    if let Some(hook) = opts.before_commit {
        hook(ws.dir);
    }
    // Committed first, as in [`accept`]: a moved directory leaves the
    // attempt open.
    let (committed, now) = commit_reviewed(
        api,
        ws.owner,
        &file.workspace,
        &message,
        &reviewed,
        "the directory",
    )
    .await?;
    let unknown = fresh && !now;
    journal.save(home, Some("committed"))?;
    crate::fault::point("accept.committed")?;
    api.decide(
        &ws.admin.signing_key,
        &task,
        &tasks::Decision {
            winner: attempt_id,
            ranking: Vec::new(),
            rationale: Some("accepted from its workspace directory".into()),
            lessons: Vec::new(),
        },
    )
    .await?;
    journal.save(home, Some("decided"))?;
    crate::fault::point("accept.decided")?;
    let commit = api
        .export_reviewed(&committed, &receipt, &repo, &branch, Some(&identity))
        .await?;
    journal.commit = Some(commit.clone());
    journal.summary = summary.clone();
    journal.save(home, Some("exported"))?;
    let moved = git(
        &repo,
        &["rev-list", "--count", &format!("{commit}~1..HEAD")],
    )
    .and_then(|n| n.parse().ok())
    .filter(|n: &u32| *n > 0);
    let cleanup = if unknown && !opts.keep {
        Cleanup::KeptBecause(COMMITTED_EARLIER.into())
    } else {
        clean_up(api, home, &file.workspace, ws.dir, opts.keep, journal.clock).await
    };
    if cleanup == Cleanup::Released {
        journal.save(home, Some("released"))?;
    }
    crate::fault::point("accept.released")?;
    WorkspaceJournal::remove(home, &file.workspace);
    Ok(WorkspaceAccepted {
        repo,
        branch,
        commit,
        receipt,
        verdict,
        title,
        summary,
        evidence,
        approved,
        moved,
        verified,
        cleanup,
        proofs,
    })
}

/// `accepts/<workspace>.json`: the journal of an accept of a workspace
/// directory, written before its first step. Run again in the directory,
/// the accept finishes the same branch; once the directory is released,
/// `aver accept` in the repository reads the hand-off from here.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceJournal {
    pub workspace: aver_core::WorkspaceId,
    pub dir: std::path::PathBuf,
    pub repo: std::path::PathBuf,
    pub branch: String,
    pub receipt: ReceiptId,
    pub snapshot: aver_core::SnapshotId,
    pub verdict: Verdict,
    /// The receipt in a few lines, once exported.
    #[serde(default)]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The last step done: `journaled`, `committed`, `decided`,
    /// `exported` or `released`.
    pub step: String,
    /// The filesystem's clock just before the first try at the commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<i64>,
    /// The `REVIEW` receipt a person approved with `--approve-review`; it
    /// finishes an accept run again only for this receipt and `snapshot`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<ReceiptId>,
}

impl WorkspaceJournal {
    fn dir(home: &Home) -> std::path::PathBuf {
        home.root().join("accepts")
    }

    fn path(home: &Home, ws: &aver_core::WorkspaceId) -> std::path::PathBuf {
        Self::dir(home).join(format!("{ws}.json"))
    }

    pub fn load(home: &Home, ws: &aver_core::WorkspaceId) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(Self::path(home, ws)).ok()?).ok()
    }

    /// Writes the journal, at `step` when given.
    fn save(&mut self, home: &Home, step: Option<&str>) -> Result<(), RunError> {
        if let Some(step) = step {
            self.step = step.to_owned();
        }
        std::fs::create_dir_all(Self::dir(home))?;
        let bytes = serde_json::to_vec_pretty(self).expect("journals serialize");
        crate::atomic::write(
            &Self::path(home, &self.workspace),
            &bytes,
            crate::atomic::Mode::Keep,
        )?;
        Ok(())
    }

    pub fn remove(home: &Home, ws: &aver_core::WorkspaceId) {
        let _ = std::fs::remove_file(Self::path(home, ws));
    }

    /// The accepts of directories of `repo` that exported their branch and
    /// whose directory is gone: what an accept run again from the
    /// repository finishes.
    pub fn finished_in(home: &Home, repo: &Path) -> Vec<Self> {
        let Ok(entries) = std::fs::read_dir(Self::dir(home)) else {
            return Vec::new();
        };
        let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
        let repo = canonical(repo);
        let mut out: Vec<Self> = entries
            .flatten()
            .filter_map(|e| serde_json::from_slice::<Self>(&std::fs::read(e.path()).ok()?).ok())
            .filter(|j| {
                canonical(&j.repo) == repo
                    && j.commit.is_some()
                    && matches!(j.step.as_str(), "exported" | "released")
                    && !j.dir.exists()
            })
            .collect();
        out.sort_by(|a, b| a.dir.cmp(&b.dir));
        out
    }
}

/// Checks that workspace `ws` (`what`, for messages) is still the
/// `reviewed` snapshot its receipt describes (a committed workspace is its
/// head).
async fn still_reviewed(
    api: &Api,
    owner: &AgentKey,
    ws: &aver_core::WorkspaceId,
    reviewed: &aver_core::SnapshotId,
    what: &str,
) -> Result<(), RunError> {
    if api.snapshot(&owner.signing_key, ws).await? != *reviewed {
        return Err(moved(what));
    }
    Ok(())
}

fn moved(what: &str) -> RunError {
    RunError::Invalid(format!(
        "{what} changed after its last verify, so its receipt no longer describes it; nothing was exported. Run aver accept again: it verifies the directory as it is now"
    ))
}

/// Commits workspace `ws` with `message` only as the `reviewed` snapshot
/// (`if_snapshot`): a change made since the check in [`still_reviewed`]
/// fails the commit instead of being exported under the earlier receipt.
/// A workspace committed already (an accept again after a failed export),
/// which [`still_reviewed`] found at `reviewed`, stays as it is. Returns
/// the committed snapshot and whether this call committed it.
async fn commit_reviewed(
    api: &Api,
    owner: &AgentKey,
    ws: &aver_core::WorkspaceId,
    message: &str,
    reviewed: &aver_core::SnapshotId,
    what: &str,
) -> Result<(aver_core::SnapshotId, bool), RunError> {
    match api
        .commit_if(&owner.signing_key, ws, message, Some(reviewed))
        .await
    {
        Ok(c) => Ok((c.snapshot, true)),
        Err(e) if e.code() == Some("SNAPSHOT_MOVED") => Err(moved(what)),
        Err(e) if e.code() == Some("WORKSPACE_CLOSED") => Ok((*reviewed, false)),
        Err(e) => Err(e.into()),
    }
}

/// The agent behind an attempt, for a commit trailer: template, index,
/// run and agent id.
pub fn agent_label(run: &RunState, agent: &super::state::AgentRun) -> String {
    format!(
        "{} #{} of aver run {} ({})",
        agent.template, agent.index, run.id, agent.agent
    )
}

async fn revoke_keys(api: &Api, home: &Home, run: &RunState) {
    for agent in &run.agents {
        let _ = api.revoke_agent(&agent.agent).await;
        if let Ok(name) = agent.key.parse() {
            let _ = std::fs::remove_file(home.key_path(&name));
        }
    }
}

/// Stops a run's agents, removes every directory, revokes the run's keys
/// and marks it discarded. Returns how many directories went.
pub async fn discard(api: &Api, home: &Home, run: &mut RunState) -> Result<u32, RunError> {
    if run.supervisor_alive() {
        // SAFETY: asks the supervisor of this run to stop its agents.
        unsafe { libc::kill(run.supervisor.pid as i32, libc::SIGINT) };
    }
    for agent in &mut run.agents {
        if let (Some(pid), Some(pgid)) = (agent.pid, agent.pgid) {
            // Only while the group's leader is still the agent's process: a
            // pid reused since must not be signalled.
            if !agent.state.is_terminal()
                && state::same_process(pid, agent.proc_start)
                && pgid == pid as i32
            {
                // SAFETY: the process group `aver run` created for this agent.
                unsafe { libc::kill(-pgid, libc::SIGKILL) };
            }
        }
        if !agent.state.is_terminal() {
            agent.state = AgentState::Killed;
        }
        agent.pid = None;
    }
    let mut removed = 0;
    for agent in &run.agents {
        if let Some(key) = compare::agent_key(home, agent) {
            let _ = api.discard(&key.signing_key, &agent.workspace).await;
        }
        if api.release(&agent.workspace).await.is_ok() {
            removed += 1;
        }
    }
    revoke_keys(api, home, run).await;
    let agents = run.agents.clone();
    *run = RunState::update(home, &run.id, |r| {
        for a in r.agents.iter_mut() {
            if let Some(stopped) = agents.iter().find(|s| s.index == a.index) {
                a.state = stopped.state;
            }
            a.pid = None;
        }
        r.discarded = true;
        r.finished_at.get_or_insert_with(aver_core::Timestamp::now);
    })?;
    Ok(removed)
}

/// The pull request body: the verdict, the evidence block of
/// `summary.md`, the comparison and how to reproduce the receipt.
pub fn pr_body(run: &RunState, a: &Accepted) -> String {
    let fixes = run
        .batch
        .as_ref()
        .and_then(|b| b.issue)
        .map(|n| format!("Fixes #{n}.\n\n"))
        .unwrap_or_default();
    format!(
        "{fixes}{}\n\n{}\n\n{}\n\n### Attempts\n\n```\n{}```\n\nReceipt `{}`. Reproduce with `aver verify --task {} --head <snapshot> --fresh`.\n\nAver-Task: {}\n",
        run.title,
        a.summary,
        a.evidence,
        compare::table(&a.compared),
        a.receipt,
        run.task,
        run.task,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names_come_from_the_title() {
        assert_eq!(
            default_branch("Rate-limit POST /login to 5/min!", "7KQ2X1"),
            "aver/rate-limit-post-login-to-5-min"
        );
        assert_eq!(default_branch("¿?", "7KQ2X1"), "aver/run-7kq2x1");
    }

    #[test]
    fn branch_names_carry_a_short_id_against_collisions() {
        assert_eq!(
            accept_branch("fix: handle negative zero", "WS01HZX7B3G", "aver/x"),
            "aver/fix-handle-negative-zero-7b3g"
        );
        assert_eq!(
            accept_branch("issue 41 Add negate", "01J9RUN7KQ2", "aver/x"),
            "aver/issue-41-add-negate-7kq2"
        );
        assert_eq!(
            accept_branch("¿?", "7KQ2X1", "aver/run-7kq2x1"),
            "aver/run-7kq2x1"
        );
    }

    #[test]
    fn the_commit_title_prefers_the_flag_then_the_agents_commit_then_a_real_title() {
        let made_up = "Work on calc at 0123456789ab";
        let commit = || Some("fix: handle negative zero".to_owned());
        assert_eq!(
            commit_title(Some(" Mine "), commit(), "Add negate", Some("neg(x)")),
            "Mine"
        );
        assert_eq!(
            commit_title(Some(" "), commit(), made_up, Some("neg(x)")),
            "fix: handle negative zero"
        );
        assert_eq!(
            commit_title(None, None, "Add negate", Some("neg(x)")),
            "Add negate"
        );
        assert_eq!(commit_title(None, None, made_up, Some("neg(x)")), "neg(x)");
        assert_eq!(
            commit_title(None, None, "Claude Code worktree a", Some("neg(x)")),
            "neg(x)"
        );
        assert_eq!(commit_title(None, None, made_up, None), made_up);
    }

    #[test]
    fn an_approval_covers_only_the_receipt_and_snapshot_it_approved() {
        // The last character holds only the hash's last bit: `a`.
        fn id<T: std::str::FromStr>(prefix: &str, c: char) -> T
        where
            T::Err: std::fmt::Debug,
        {
            format!("{prefix}_{}a", c.to_string().repeat(51))
                .parse()
                .unwrap()
        }
        let (x, y): (ReceiptId, ReceiptId) = (id("rcpt", 'x'), id("rcpt", 'y'));
        let (sx, sy): (aver_core::SnapshotId, aver_core::SnapshotId) =
            (id("snap", 'x'), id("snap", 'y'));
        assert_eq!(approval(Verdict::Pass, x, sx, false, None), Ok(None));
        assert_eq!(approval(Verdict::Review, x, sx, false, None), Err(()));
        assert_eq!(approval(Verdict::Review, x, sx, true, None), Ok(Some(x)));
        // Resumed on the receipt it approved: still approved.
        assert_eq!(
            approval(Verdict::Review, x, sx, false, Some((Some(x), x, sx))),
            Ok(Some(x))
        );
        // Journaled for X; the head is now Y.
        assert_eq!(
            approval(Verdict::Review, y, sy, false, Some((Some(x), x, sx))),
            Err(())
        );
        assert_eq!(
            approval(Verdict::Review, y, sy, true, Some((Some(x), x, sx))),
            Ok(Some(y))
        );
        // A journal with no approval approves nothing.
        assert_eq!(
            approval(Verdict::Review, x, sx, false, Some((None, x, sx))),
            Err(())
        );
    }

    #[test]
    fn long_titles_cut_at_a_word_boundary_and_carry_the_run_id() {
        let branch = default_branch(
            "combine Add negate and Add double and Add triple and more",
            "7KQ2X1",
        );
        assert_eq!(branch, "aver/combine-add-negate-and-add-double-and-7kq2x1");
        let slug = branch.strip_prefix("aver/").unwrap();
        assert!(slug.len() <= 40 + 7, "{slug}");
        assert_eq!(
            default_branch(&"x".repeat(60), "7KQ2X1"),
            format!("aver/{}-7kq2x1", "x".repeat(40))
        );
        assert_eq!(
            default_branch("exactly forty characters long title here", "R"),
            "aver/exactly-forty-characters-long-title-here"
        );
    }
}
