//! `aver` with no command: where things stand here and the one command to
//! run next. It reads only: the directory is compared with the snapshots
//! its receipts and checkpoints judged by a scan that records nothing, and
//! no run starts.

use aver::keys::{Home, ADMIN_KEY};
use aver::run::state::{gave_up, RunFile};
use aver::workspace::checkpoint::Checkpoint;
use aver_core::{Receipt, SnapshotId, Verdict};

use super::workspace::Context;

pub fn run(home: &Home) -> anyhow::Result<()> {
    let text = match Context::find()? {
        Some(ctx) => in_workspace(&ctx)?,
        None => outside(home)?,
    };
    println!("{text}");
    Ok(())
}

fn outside(home: &Home) -> anyhow::Result<String> {
    const DEMO: &str = "`aver demo` shows Aver catch a gamed test in a throwaway repository.";
    let Some(repo) = super::notices::this_repo() else {
        return Ok(format!(
            "Not inside a Git repository.\nNext: `aver init` in a repository sets Aver up there; {DEMO}"
        ));
    };
    let keys = home.key_path(&ADMIN_KEY.parse()?).exists();
    if !keys || !repo.join(aver::env::CONFIG_PATH).is_file() {
        let missing = match (keys, repo.join(aver::env::CONFIG_PATH).is_file()) {
            (false, false) => "no Aver home keys and no aver.toml",
            (false, true) => "no Aver home keys",
            _ => "no aver.toml naming its checks",
        };
        return Ok(format!(
            "{}: {missing}.\nNext: `aver init`; {DEMO}",
            repo.display()
        ));
    }
    Ok(format!(
        "{}: set up for Aver.\nNext: `aver workspace new .` gives an agent its own directory (`aver run \"<task>\"` runs agents for you).",
        repo.display()
    ))
}

fn in_workspace(ctx: &Context) -> anyhow::Result<String> {
    if let Some(reason) = gave_up(&ctx.root) {
        return Ok(format!(
            "This workspace gave up: {reason}\nNext: nothing here; `aver status` from the repository shows the run."
        ));
    }
    let key = match ctx.key() {
        Ok(key) => key,
        // A finished run's directory: the error says what happened to it.
        Err(e) => return Ok(super::describe(&e)),
    };
    let (rt, api) = super::open(&ctx.home)?;
    let workspaces = api.services().workspaces();
    let id = &ctx.file.workspace;
    let holds = |snapshot: &SnapshotId| -> anyhow::Result<bool> {
        Ok(workspaces.changed_since(id, snapshot)?.is_empty())
    };
    let in_run = RunFile::read(&ctx.root).is_some();

    let latest: Option<Receipt> = match ctx.file.attempt {
        Some(attempt) => rt
            .block_on(api.attempt(&key.signing_key, &attempt))?
            .receipts
            .last()
            .map(|rid| api.services().store.get(rid))
            .transpose()?,
        None => None,
    };
    if let Some(receipt) = latest.filter(|r| holds(&r.body.head_snapshot).unwrap_or(false)) {
        let verdict = receipt.body.verdict;
        let next = match verdict {
            Verdict::Pass if in_run => {
                "nothing; the run ranks this attempt (`aver status` from the repository)."
            }
            Verdict::Pass => "`aver accept` turns this directory into a branch.",
            Verdict::Review => {
                "a person must approve the changed tests or harness: `aver accept --approve-review`, at their terminal."
            }
            _ => "fix what `aver verify --last` names, then `aver check`.",
        };
        return Ok(format!(
            "Latest verify: {} for the files here.\nNext: {next}",
            verdict.as_str()
        ));
    }

    let last: Option<Checkpoint> = workspaces.checkpoints(id)?.pop();
    if let Some(c) = last.filter(|c| holds(&c.snapshot).unwrap_or(false)) {
        if c.verdict == Verdict::Pass {
            return Ok(
                "Latest check: verify should PASS for the files here.\nNext: `aver verify`."
                    .to_owned(),
            );
        }
        if c.verdict == Verdict::Review {
            return Ok(
                "Latest check: verify would give REVIEW: the change rewrites tests or harness.\nNext: `aver verify`; a person then approves it with `aver accept --approve-review`."
                    .to_owned(),
            );
        }
        let restore = if c.green() {
            ""
        } else {
            "; `aver restore --last-green` puts back the last check without a NEW failure"
        };
        return Ok(format!(
            "Latest check: verify would stop at {} ({}).\nNext: fix it, then `aver check`{restore}.",
            c.verdict.as_str(),
            c.delta()
        ));
    }

    let changed = workspaces.changed_since(id, &ctx.file.base)?;
    if changed.is_empty() {
        return Ok("No changes yet against the base.\nNext: edit, then `aver check`.".to_owned());
    }
    Ok(format!(
        "{} changed that no check or verify covers ({}).\nNext: `aver check`.",
        files(changed.len()),
        listed(&changed)
    ))
}

fn files(n: usize) -> String {
    if n == 1 {
        "1 file".to_owned()
    } else {
        format!("{n} files")
    }
}

/// At most three paths, then how many more.
fn listed(paths: &[String]) -> String {
    let mut text = paths
        .iter()
        .take(3)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > 3 {
        text.push_str(&format!(" and {} more", paths.len() - 3));
    }
    text
}
