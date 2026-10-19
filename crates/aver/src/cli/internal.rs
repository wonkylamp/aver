//! `aver internal`: what other aver commands start in a process of its own.
//! `warm <task>` runs the task's checks on its base (§2.10), detached from
//! `aver workspace new`, `aver run` and the WorktreeCreate hook
//! ([`aver::run::api::Api::warm`]).

use aver::keys::Home;
use aver_core::TaskId;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    Warm { task: TaskId },
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    match args.command {
        Command::Warm { task } => {
            let (rt, api) = super::open(home)?;
            let admin = aver::services::key_in(home, None)?;
            rt.block_on(api.services().verifier.warm(admin.id(), task))?;
            Ok(())
        }
    }
}
