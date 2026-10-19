//! `aver feedback [run]`: resume each agent of a finished run and ask it
//! Aver's fixed questions about Aver (ARCHITECTURE.md §2.10); `aver run
//! --feedback` does the same when its run ends.

use std::time::Duration;

use anyhow::bail;
use aver::keys::Home;
use aver::run::feedback;
use aver::run::state::RunState;

use super::run::parse_duration;

#[derive(clap::Args)]
pub struct Args {
    /// The run or batch (default: the newest in this repository).
    run: Option<String>,
    /// Only these agents (by index).
    #[arg(long = "agent", value_name = "N")]
    agents: Vec<u32>,
    /// How to resume a custom agent: a shell command with {session_id} and
    /// {feedback} (the questions).
    #[arg(long, value_name = "TEMPLATE")]
    resume_cmd: Option<String>,
    /// How long each agent may take to answer.
    #[arg(long, default_value = "10m", value_parser = parse_duration)]
    timeout: u64,
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    let repo = super::notices::this_repo();
    let batch = match &args.run {
        Some(id) if aver::run::batch::exists(home, id) => {
            Some(aver::run::batch::BatchState::load(home, id)?)
        }
        Some(_) => None,
        None => aver::run::batch::BatchState::current(home, repo.as_deref()),
    };
    if let Some(b) = batch {
        if b.alive() {
            bail!(
                "batch {} is still running; ask when it ends (or start it with --feedback)",
                b.id
            );
        }
        for t in &b.tasks {
            let Ok(mut run) = RunState::load(home, &t.run) else {
                continue;
            };
            println!("#{} {}", t.number, t.title);
            collect(
                home,
                &mut run,
                args.resume_cmd.as_deref(),
                Duration::from_millis(args.timeout),
                &args.agents,
            )?;
        }
        return Ok(());
    }
    let mut run = RunState::find(home, args.run.as_deref(), repo.as_deref())?;
    if run.supervisor_alive() {
        bail!(
            "run {} is still running; ask when it ends (or start runs with --feedback)",
            run.id
        );
    }
    collect(
        home,
        &mut run,
        args.resume_cmd.as_deref(),
        Duration::from_millis(args.timeout),
        &args.agents,
    )
}

/// Asks, then prints one line per agent and where the answers are.
pub(super) fn collect(
    home: &Home,
    run: &mut RunState,
    resume_cmd: Option<&str>,
    timeout: Duration,
    only: &[u32],
) -> anyhow::Result<()> {
    eprintln!(
        "aver: asking the agents of run {} about Aver (read-only)…",
        run.id
    );
    let asked = feedback::ask(home, run, resume_cmd, timeout, only)?;
    for a in &asked {
        match &a.outcome {
            Ok(f) => println!(
                "  #{} rating {}{}  {}",
                a.index,
                f.rating
                    .map_or("none given".to_owned(), |r| format!("{r}/10")),
                if f.changed_files {
                    " (files changed while it answered)"
                } else {
                    ""
                },
                f.file.display()
            ),
            Err(why) => println!("  #{} not asked: {why}", a.index),
        }
    }
    Ok(())
}
