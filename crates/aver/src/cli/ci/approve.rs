//! `aver ci approve`: binds a pull request review's approval to the head
//! commit, from GitHub's review API through `gh`, and writes
//! `approval.json`. No check runs, so the token `gh` uses never meets the
//! pull request's code.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{bail, Context};
use aver::ci::approve::{self, Approval, Event, PullRequest};
use aver::keys::Home;
use aver_core::{GitSha, ReceiptId};

const APPROVAL_FILE: &str = "approval.json";

#[derive(clap::Args)]
pub struct Args {
    /// The pull_request_review event payload ($GITHUB_EVENT_PATH).
    #[arg(long)]
    github_event: PathBuf,
    /// The Git repository holding the head commit, whose tree the approval records.
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    /// Where approval.json is written.
    #[arg(long, default_value = "aver-out")]
    out_dir: PathBuf,
    /// The REVIEW or UNPROVEN receipt the reviewer read, recorded in approval.json.
    #[arg(long, value_name = "RECEIPT_ID")]
    receipt: Option<ReceiptId>,
}

pub fn run(_: &Home, args: Args) -> anyhow::Result<ExitCode> {
    let approval = args.out_dir.join(APPROVAL_FILE);
    match std::fs::remove_file(&approval) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(e).with_context(|| format!("removing {}", approval.display()))
        }
        _ => {}
    }
    let event: Event = serde_json::from_slice(
        &std::fs::read(&args.github_event)
            .with_context(|| format!("reading {}", args.github_event.display()))?,
    )
    .with_context(|| {
        format!(
            "{} is not a pull_request_review event",
            args.github_event.display()
        )
    })?;
    let repository = event.repository();
    if !valid_repository(repository) {
        bail!("the event names the repository {repository:?}, which is not owner/name");
    }
    let number = event.pull_request.number;
    let pull: PullRequest =
        serde_json::from_slice(&gh(&[&format!("repos/{repository}/pulls/{number}")])?)
            .context("reading the pull request from the GitHub API")?;
    let reviews = approve::parse_reviews(&gh(&[
        "--paginate",
        &format!("repos/{repository}/pulls/{number}/reviews?per_page=100"),
    ])?)
    .context("reading the pull request's reviews from the GitHub API")?;
    let head = pull.head.sha;
    let review = match approve::binding(&event, &head, &reviews) {
        Ok(review) => review,
        Err(why) => {
            println!("NOT APPROVED: {why}");
            return Ok(ExitCode::from(aver::ci::EXIT_NEEDS_MAINTAINER));
        }
    };
    let record = Approval {
        repository: repository.to_owned(),
        pull_request: number,
        reviewer: review
            .user
            .as_ref()
            .map(|u| u.login.clone())
            .unwrap_or_default(),
        review_id: review.id,
        head_tree: tree(&args.repo, &head)?,
        head_commit: head,
        receipt: args.receipt.map(|r| r.to_string()),
    };
    std::fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("creating {}", args.out_dir.display()))?;
    std::fs::write(&approval, serde_json::to_string_pretty(&record)? + "\n")
        .with_context(|| format!("writing {}", approval.display()))?;
    println!(
        "APPROVED {} by {} (review {}); wrote {}",
        record.head_commit,
        record.reviewer,
        record.review_id,
        approval.display()
    );
    Ok(ExitCode::SUCCESS)
}

fn valid_repository(name: &str) -> bool {
    let mut parts = name.split('/');
    let ok = |p: Option<&str>| {
        p.is_some_and(|p| {
            !p.is_empty()
                && !p.starts_with('.')
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        })
    };
    ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

/// `gh api <args>`'s output; its token comes from `GH_TOKEN` as usual.
fn gh(args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let out = Command::new("gh")
        .arg("api")
        .args(args)
        .output()
        .context("running gh (GitHub's CLI), which aver ci approve reads reviews with")?;
    if !out.status.success() {
        bail!(
            "gh api {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// The head commit's tree, from the checkout's objects.
fn tree(repo: &Path, commit: &GitSha) -> anyhow::Result<GitSha> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("{commit}^{{tree}}"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!(
            "the head commit {commit} is not in {}; fetch it first (git fetch origin {commit})",
            repo.display()
        );
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .map_err(|e| anyhow::anyhow!("git rev-parse printed no tree id: {e}"))
}
