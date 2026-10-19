//! `aver giveup "<reason>"` inside a workspace directory: the agent stops
//! trying. `aver run` ends it as gave up and keeps the reason for the
//! human and the ledger.

use anyhow::bail;
use aver::keys::Home;
use aver::run::state::GIVEUP_PATH;

use super::workspace::Context;

#[derive(clap::Args)]
pub struct Args {
    /// Why: what blocks the task (a missing dependency, a broken test
    /// environment, an impossible criterion).
    reason: String,
}

pub fn run(_: &Home, args: Args) -> anyhow::Result<()> {
    let ctx = Context::require("aver giveup records why an agent stopped trying")?;
    let reason = args.reason.trim();
    if reason.is_empty() {
        bail!("say why: aver giveup \"<reason>\"");
    }
    let reason: String = reason.chars().take(2000).collect();
    aver::workspace::WorkspaceFile::write_meta(&ctx.root, GIVEUP_PATH, format!("{reason}\n"))?;
    println!("Recorded. Stop now: Aver ends this attempt as gave up and shows your reason.");
    Ok(())
}
