//! `aver brief` inside a workspace directory of a run: reprints what the
//! agent was told at start (`.aver/BRIEF.md`), then what other tasks in
//! the repository change and lease now (the saved brief leaves out what
//! they did at start, which is stale by now).

use anyhow::Context as _;
use aver::keys::Home;

use super::workspace::Context;

#[derive(clap::Args)]
pub struct Args {}

pub fn run(_: &Home, _: Args) -> anyhow::Result<()> {
    let ctx = Context::require("aver brief prints a run agent's brief")?;
    let text = std::fs::read_to_string(ctx.root.join(".aver/BRIEF.md"))
        .context("this workspace has no brief (it is not part of an aver run)")?;
    print!("{text}");
    match super::notices::live(&ctx) {
        Ok(now) => print!("\n{}", aver::run::report::note_once(&ctx.root, now)),
        Err(e) => eprintln!(
            "aver: could not read other tasks' work: {}",
            super::describe(&e)
        ),
    }
    Ok(())
}
