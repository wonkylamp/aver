//! `aver discard [run|batch]`: stop a run's agents, delete its directories
//! and revoke its keys; for a batch (the default after one), every run of
//! it not accepted yet. Accepted branches stay.

use aver::keys::Home;
use aver::run::accept;
use aver::run::batch::{self, BatchState};
use aver::run::state::RunState;

#[derive(clap::Args)]
pub struct Args {
    /// The run or batch (default: the newest in this repository).
    run: Option<String>,
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    let repo = super::notices::this_repo();
    let batch = match &args.run {
        Some(id) if batch::exists(home, id) => Some(BatchState::load(home, id)?),
        Some(_) => None,
        None => BatchState::current(home, repo.as_deref()),
    };
    let mut runs = match &batch {
        Some(b) => b
            .tasks
            .iter()
            .filter_map(|t| RunState::load(home, &t.run).ok())
            .filter(|r| r.decided.is_none() && !r.discarded)
            .collect(),
        None => vec![RunState::find(home, args.run.as_deref(), repo.as_deref())?],
    };
    let rt = tokio::runtime::Runtime::new()?;
    let api = aver::run::api::Api::open(home)?;
    if let Some(b) = &batch {
        if b.alive() {
            // SAFETY: asks the batch's process to stop its agents and its
            // queue, as ctrl-c would.
            unsafe { libc::kill(b.supervisor.pid as i32, libc::SIGINT) };
        }
    }
    let mut total = 0;
    for run in &mut runs {
        let removed = rt.block_on(accept::discard(&api, home, run))?;
        total += removed;
        println!(
            "Discarded run {}{}: removed {removed} {}.",
            run.id,
            run.batch
                .as_ref()
                .map(|b| format!(" (task #{})", b.number))
                .unwrap_or_default(),
            if removed == 1 {
                "directory"
            } else {
                "directories"
            }
        );
    }
    if let Some(b) = &batch {
        println!(
            "Discarded {} {} of batch {} ({total} directories); accepted tasks keep their branches.",
            runs.len(),
            if runs.len() == 1 { "run" } else { "runs" },
            b.id
        );
    }
    Ok(())
}
