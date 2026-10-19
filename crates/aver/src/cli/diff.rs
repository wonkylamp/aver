//! `aver diff`: inside a workspace directory, the directory's changes
//! against the base (after a sync, which is what verify sees); with run
//! attempts, one attempt's changes or two attempts compared, in the
//! terminal or in your diff tool (`--tool`).

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{bail, Context as _};
use aver::keys::Home;
use aver::run::api::DiffFile;
use aver::run::compare::agent_key;
use aver::run::state::RunState;
use aver::workspace::DiffChange;

use super::workspace::Context;

#[derive(clap::Args)]
pub struct Args {
    /// Attempts of a run (`aver run`), by number or attempt id: one shows
    /// its changes, two compare them. After a batch, task numbers.
    #[arg(num_args = 0..=2)]
    attempts: Vec<String>,
    /// The run (default: the newest in this repository).
    #[arg(long, conflicts_with = "batch")]
    run: Option<String>,
    /// The batch whose tasks the numbers name (default: the newest in this
    /// repository, when it is newer than every run there outside a batch).
    #[arg(long)]
    batch: Option<String>,
    /// List changed paths with A, M or D instead of the diff.
    #[arg(long)]
    stat: bool,
    /// With two attempts: open both directories with this command, e.g.
    /// `opendiff`, `meld` or `code --diff`.
    #[arg(long, value_name = "CMD")]
    tool: Option<String>,
}

fn print(files: &[DiffFile], stat: bool) {
    for file in files {
        if stat {
            let tag = match file.change {
                DiffChange::Added => 'A',
                DiffChange::Modified => 'M',
                DiffChange::Deleted => 'D',
            };
            println!("{tag} {}", file.path);
        } else {
            print!("{}", file.diff);
            if !file.diff.is_empty() && !file.diff.ends_with('\n') {
                println!();
            }
        }
    }
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    if args.attempts.is_empty() {
        let ctx = Context::require(
            "aver diff compares a workspace directory with its base; outside one, name run attempts (aver diff 1 2)",
        )?;
        let key = ctx.key()?;
        let (rt, api) = super::open(&ctx.home)?;
        print(
            &rt.block_on(api.diff(&key.signing_key, &ctx.file.workspace))?,
            args.stat,
        );
        return Ok(());
    }
    let batch = match &args.run {
        Some(_) => None,
        None => super::accept::batch_of(home, args.batch.as_deref())?,
    };
    // After a batch, numbers name tasks: each one's best attempt.
    let agents = match &batch {
        Some(b) => {
            let admin = aver::services::key_in(home, None)?;
            let rt = tokio::runtime::Runtime::new()?;
            let api = aver::run::api::Api::open(home)?;
            args.attempts
                .iter()
                .map(|n| {
                    let (run, best) = super::accept::task_attempt(&rt, &api, home, &admin, b, n)?;
                    let mut agent = run
                        .agent(&best.to_string())
                        .cloned()
                        .with_context(|| format!("run {} has no attempt {best}", run.id))?;
                    agent.index = run.batch.as_ref().map_or(best, |r| r.number);
                    anyhow::Ok(agent)
                })
                .collect::<anyhow::Result<Vec<_>>>()?
        }
        None => {
            let run = RunState::find(
                home,
                args.run.as_deref(),
                super::notices::this_repo().as_deref(),
            )?;
            args.attempts
                .iter()
                .map(|n| {
                    run.agent(n)
                        .cloned()
                        .with_context(|| format!("run {} has no attempt {n}", run.id))
                })
                .collect::<anyhow::Result<Vec<_>>>()?
        }
    };
    let (rt, api) = super::open(home)?;
    let diff_of = |a: &aver::run::state::AgentRun| {
        let key = agent_key(home, a)
            .with_context(|| format!("attempt {} was decided or discarded", a.index))?;
        anyhow::Ok(rt.block_on(api.diff(&key.signing_key, &a.workspace))?)
    };
    match agents.as_slice() {
        [one] => print(&diff_of(one)?, args.stat),
        [a, b] => {
            for x in [a, b] {
                if !x.dir.is_dir() {
                    bail!("attempt {}'s directory is gone", x.index);
                }
            }
            if let Some(tool) = args.tool {
                let status = std::process::Command::new("/bin/sh")
                    .args(["-c", &format!("{tool} \"$1\" \"$2\""), "aver-diff"])
                    .arg(&a.dir)
                    .arg(&b.dir)
                    .status()
                    .with_context(|| format!("running {tool}"))?;
                if !status.success() {
                    bail!("{tool} exited with {status}");
                }
                return Ok(());
            }
            let paths: BTreeSet<String> = diff_of(a)?
                .into_iter()
                .chain(diff_of(b)?)
                .map(|f| f.path)
                .collect();
            for path in paths {
                compare_file(&a.dir, &b.dir, &path, a.index, b.index, args.stat);
            }
        }
        _ => unreachable!("clap allows one or two"),
    }
    Ok(())
}

fn compare_file(a: &Path, b: &Path, path: &str, ia: u32, ib: u32, stat: bool) {
    let read = |dir: &Path| std::fs::read(dir.join(path)).ok();
    let (x, y) = (read(a), read(b));
    if x == y {
        return;
    }
    if stat {
        let tag = match (&x, &y) {
            (None, _) => '+',
            (_, None) => '-',
            _ => 'M',
        };
        println!("{tag} {path}");
        return;
    }
    let text = |v: Option<Vec<u8>>| -> String {
        v.map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    };
    let (tx, ty) = (text(x), text(y));
    let diff = similar::TextDiff::from_lines(tx.as_str(), ty.as_str());
    print!(
        "{}",
        diff.unified_diff()
            .header(&format!("{ia}/{path}"), &format!("{ib}/{path}"))
    );
}
