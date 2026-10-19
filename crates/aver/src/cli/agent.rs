use std::collections::BTreeSet;
use std::path::PathBuf;

use aver::agents::{AgentName, Capability};
use aver::keys::{self, AgentKey, Home};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Create a key file for a new agent and register it.
    Add {
        #[arg(long)]
        name: AgentName,
        /// Comma-separated capabilities, e.g. workspace:write,exec:run.
        #[arg(long, value_delimiter = ',')]
        caps: Vec<Capability>,
    },
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    match args.command {
        Command::Add { name, caps } => {
            let services = aver::services::Services::open(home)?;
            let (key, path) = add(&services, home, name, caps.into_iter().collect())?;
            println!("{} {}", key.id(), path.display());
            Ok(())
        }
    }
}

/// What an agent tool's key may do: claim tasks, edit workspaces, run and
/// verify, and take leases (`aver integrate`).
pub(super) fn worker_caps() -> BTreeSet<Capability> {
    [
        Capability::TaskClaim,
        Capability::WorkspaceWrite,
        Capability::VerifyRun,
        Capability::ExecRun,
        Capability::LeaseWrite,
    ]
    .into()
}

/// Creates a key file for a new agent named `name` and registers it,
/// issued by the bootstrap admin.
pub(super) fn add(
    services: &aver::services::Services,
    home: &Home,
    name: AgentName,
    caps: BTreeSet<Capability>,
) -> anyhow::Result<(AgentKey, PathBuf)> {
    Ok(keys::add_agent(home, &services.store, name, caps)?)
}
