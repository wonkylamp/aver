//! `aver combine <run|n|workspace>…`: the work of runs and workspace
//! directories on different tasks, all descended from the same commit,
//! merged into one branch and verified against every task it contains, a
//! stacked run's prerequisites included (ARCHITECTURE.md §2.11). After a
//! batch, tasks are named by number and `--all-pass` takes every task whose
//! best attempt is PASS. Nothing is accepted or exported unless every task
//! passes on the merge; with `--partial`, the tasks that conflict or fail
//! are left out, each with its reason, and the rest is exported. Exits 0
//! when everything named was exported, 1 when anything was left out or
//! nothing was exported.

use std::process::ExitCode;

use anyhow::{bail, Context as _};
use aver::keys::Home;
use aver::run::batch::{self, record::CombineStatus};
use aver::run::combine::{self, Combine, Input, Report};
use aver::run::rerun;
use aver::run::state::RunState;
use aver::services::key_in;

#[derive(clap::Args)]
pub struct Args {
    /// Runs (ids), their accepted attempts (att_…), after a batch task
    /// numbers, or workspace directories made outside aver run (by name, by
    /// attempt id, or as the aver/… branch aver accept exported). One is
    /// enough when it is stacked on another task.
    #[arg(
        required_unless_present = "all_pass",
        conflicts_with = "all_pass",
        num_args = 1..
    )]
    runs: Vec<String>,
    /// Every task of the batch whose best attempt is PASS.
    #[arg(long)]
    all_pass: bool,
    /// The batch whose tasks numbers name (default: the newest in this
    /// repository).
    #[arg(long)]
    batch: Option<String>,
    /// When the merge of everything does not pass, keep tasks in number
    /// order, each only if it merges and passes with the ones kept so far,
    /// and export those.
    #[arg(long)]
    partial: bool,
    /// The branch to create (default: aver/combine-<titles>).
    #[arg(long)]
    branch: Option<String>,
    /// Print the outcome as JSON.
    #[arg(long)]
    json: bool,
    /// Accept tasks that are REVIEW (they change what an existing test
    /// expects), and a merge that is: passing this is your approval.
    /// Without it, a task not accepted yet must be PASS.
    #[arg(long)]
    approve_review: bool,
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<ExitCode> {
    if args.approve_review {
        let what = if args.all_pass {
            "--all-pass".to_owned()
        } else {
            args.runs.join(" ")
        };
        aver::run::accept::refuse_agent_approval(&format!("aver combine {what} --approve-review"))?;
    }
    let admin = key_in(home, None)?;
    let rt = tokio::runtime::Runtime::new()?;
    let api = aver::run::api::Api::open(home)?;
    let named_batch = args.batch.is_some()
        || args.all_pass
        || args.runs.iter().any(|r| rerun::task_number(r).is_some());
    let batch = if named_batch {
        super::accept::batch_of(home, args.batch.as_deref())?
    } else {
        None
    };
    let mut notes: Vec<String> = Vec::new();
    let mut inputs = Vec::new();
    if args.all_pass {
        let batch = batch.as_ref().context(
            "no batch: --all-pass combines the passing tasks of a batch (aver run --tasks, --issue)",
        )?;
        if batch.alive() {
            bail!(
                "batch {} is still running; combine its tasks once it ends",
                batch.id
            );
        }
        let summary = rt.block_on(batch::summarize(&api, home, &admin.signing_key, batch));
        for t in &summary.tasks {
            let run = batch.run(home, t.number);
            match (run, &t.outcome, t.attempt) {
                (Some(run), batch::summary::Outcome::Ready { .. }, Some(best)) => {
                    inputs.push(input(run, best))
                }
                _ => notes.push(format!(
                    "#{} left out: {}",
                    t.number,
                    batch::summary::outcome_text(t)
                )),
            }
        }
    } else {
        let repo = super::notices::repo_root(None)?;
        for name in &args.runs {
            let run = match (rerun::task_number(name), &batch) {
                (Some(n), Some(b)) if b.entry(n).is_some() => b
                    .run(home, n)
                    .with_context(|| format!("task #{n} of batch {} has not started", b.id))?,
                (Some(n), None) => bail!(
                    "no batch to find task #{n} in; name runs by id, or the batch with --batch"
                ),
                _ => match combine::find_run(home, name) {
                    Ok(run) if !name.starts_with("aver/") => run,
                    _ => {
                        let attempt = combine::find_solo(&api, home, &repo, name)?;
                        inputs.push(Input::Solo {
                            attempt,
                            repo: repo.clone(),
                        });
                        continue;
                    }
                },
            };
            let best = match run.decided.as_ref() {
                Some(d) => d.index,
                None => rt
                    .block_on(batch::best_attempt(&api, home, &admin.signing_key, &run))
                    .with_context(|| format!("run {} has no attempts", run.id))?,
            };
            inputs.push(input(run, best));
        }
    }
    if inputs.is_empty() {
        for n in &notes {
            eprintln!("aver: {n}");
        }
        bail!("nothing to combine: no task passed");
    }
    let record = args.batch.clone().or_else(|| common_batch(&inputs));
    let mut resolved = Vec::new();
    for i in &inputs {
        resolved.push(rt.block_on(combine::resolve(&api, home, &admin, i))?);
    }
    let report = rt.block_on(
        Combine {
            api: &api,
            home: Some(home),
            admin: &admin,
            branch: args.branch.clone(),
            partial: args.partial,
            batch: record,
            approve_review: args.approve_review,
            before_accept: None,
        }
        .run(resolved),
    )?;
    let next = next(&report, &args);
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": report.status,
                "branch": report.branch,
                "commit": report.commit,
                "included": report.included,
                "excluded": report.excluded,
                "verdicts": report.verdicts,
                "absorbed": report.absorbed,
                "accepted": report.accepted.iter().map(|(r, b)| serde_json::json!({
                    "number": r.number, "run": r.run, "branch": b,
                })).collect::<Vec<_>>(),
                "left_out": notes,
                "approved_review": report.approved_review,
                "next": next,
            }))?
        );
    } else {
        print_text(&report, &notes, &next);
    }
    Ok(match report.status {
        CombineStatus::Exported => ExitCode::SUCCESS,
        _ => ExitCode::from(1),
    })
}

fn input(run: RunState, best: u32) -> Input {
    if run.decided.is_some() {
        Input::Accepted(run)
    } else {
        Input::Best(run, best)
    }
}

/// The batch every input belongs to, when they share one.
fn common_batch(inputs: &[Input]) -> Option<String> {
    let mut ids = inputs.iter().map(|i| match i {
        Input::Accepted(r) | Input::Best(r, _) => r.batch.as_ref().map(|b| b.id.clone()),
        Input::Solo { .. } => None,
    });
    let first = ids.next()??;
    ids.all(|i| i.as_deref() == Some(first.as_str()))
        .then_some(first)
}

/// What to do after: never the same failing combine again.
fn next(report: &Report, args: &Args) -> Vec<String> {
    let mut out = Vec::new();
    match report.status {
        CombineStatus::Exported | CombineStatus::Partial => {
            out.push(format!(
                "git switch {}",
                report.branch.as_deref().unwrap_or("-")
            ));
            out.extend(rerun::hints(report));
        }
        CombineStatus::Conflicts | CombineStatus::NotPass => {
            out.extend(rerun::hints(report));
            let what = if args.all_pass {
                "--all-pass".to_owned()
            } else {
                args.runs.join(" ")
            };
            let review = report.excluded.iter().any(|l| {
                matches!(
                    l.why,
                    combine::LeftWhy::NotPass {
                        verdict: aver_core::Verdict::Review,
                        ..
                    }
                )
            });
            if review && !args.approve_review {
                out.push(format!("aver combine {what} --approve-review"));
            }
            if !args.partial {
                out.push(format!("aver combine {what} --partial"));
            }
        }
    }
    out
}

fn print_text(report: &Report, notes: &[String], next: &[String]) {
    for n in notes {
        println!("{n}");
    }
    for a in &report.absorbed {
        println!(
            "run {} is already in run {}'s work (stacked on it), so it is merged once, through it",
            a.run, a.by
        );
    }
    match report.status {
        CombineStatus::Conflicts if !report.conflicts.is_empty() => {
            println!(
                "Conflicts: nothing was merged, accepted or exported. Rerun one task on the other's work, or leave it out with --partial:"
            );
            print!(
                "{}",
                combine::conflicts_text(&report.conflicts, &report.labels)
            );
        }
        CombineStatus::Conflicts | CombineStatus::NotPass => {
            if report.verdicts.is_empty() {
                println!("Nothing was merged, accepted or exported:");
            } else {
                println!(
                    "Merged cleanly, but not every task passes on the result; nothing was accepted or exported:"
                );
                print_verdicts(report);
            }
        }
        CombineStatus::Exported | CombineStatus::Partial => {
            for (r, branch) in &report.accepted {
                println!("{} accepted as {branch}", r.name());
            }
            let commit = report.commit.as_deref().unwrap_or("");
            let parent = report.parent.as_deref().unwrap_or(&report.base_commit);
            println!(
                "Combined {} runs as branch {} ({}), child of {}.",
                report.included.len(),
                report.branch.as_deref().unwrap_or("-"),
                &commit[..12.min(commit.len())],
                &parent[..12.min(parent.len())]
            );
            if report
                .parent
                .as_deref()
                .is_some_and(|p| p != report.base_commit)
            {
                println!(
                    "One head carries every task, so the branch is its own history: one commit per task, on {}.",
                    &report.base_commit[..12.min(report.base_commit.len())]
                );
            }
            if !report.merged.is_empty() {
                println!("Merged line by line: {}", report.merged.join(", "));
            }
            print_verdicts(report);
            if !report.approved_review.is_empty() {
                let names: Vec<String> = report.approved_review.iter().map(|r| r.label()).collect();
                println!(
                    "REVIEW approved by --approve-review: {} (changed what existing tests expect)",
                    names.join(", ")
                );
            }
        }
    }
    let exported = report.branch.is_some();
    for l in &report.excluded {
        // A task failing on its own is already on its verdict line.
        let shown = matches!(&l.why, combine::LeftWhy::NotPass { task, receipt, .. }
            if task.run == l.task.run
                && l.carries.is_empty()
                && report.verdicts.iter().any(|v| v.receipt == *receipt));
        if shown {
            continue;
        }
        let carries = if l.carries.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = l.carries.iter().map(|c| c.name()).collect();
            format!(" (with {} under it)", names.join(", "))
        };
        println!(
            "  {}{}{carries}: {}",
            l.task.label(),
            if exported { " left out" } else { "" },
            combine::why_text(&l.why)
        );
    }
    if !next.is_empty() {
        println!("Next: {}", next.join("   "));
    }
}

fn print_verdicts(report: &Report) {
    for v in &report.verdicts {
        let who = v.name();
        let via = v
            .via
            .as_deref()
            .map(|r| format!(", via run {r}"))
            .unwrap_or_default();
        println!(
            "  {who} {:?}: {} (receipt {}{via})",
            v.title,
            v.verdict.as_str(),
            v.receipt
        );
    }
}
