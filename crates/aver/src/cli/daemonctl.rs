//! `aver daemon start|status|stop`: removed in 0.4 with the daemon, when
//! every command began to open the home's services in its own process
//! (ARCHITECTURE.md §2.7).

use std::process::ExitCode;

use aver::keys::Home;

#[derive(clap::Args)]
pub struct Args {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    rest: Vec<String>,
}

pub fn run(_: &Home, _: Args) -> anyhow::Result<ExitCode> {
    Ok(super::removed())
}
