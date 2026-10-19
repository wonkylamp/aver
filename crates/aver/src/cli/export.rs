use std::path::PathBuf;

use anyhow::Context;
use aver::git;
use aver::keys::Home;
use aver::services::Services;
use aver_core::SnapshotId;

#[derive(clap::Args)]
pub struct Args {
    snapshot: SnapshotId,
    /// The target repository; created bare if it does not exist.
    repo: PathBuf,
    #[arg(long)]
    branch: String,
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    let commit = export(
        &Services::open(home)?,
        &args.snapshot,
        &args.repo,
        &args.branch,
    )?;
    println!("{commit}");
    Ok(())
}

/// Exports `snapshot` to `branch` of `repo`; returns the commit the branch
/// points at.
fn export(
    services: &Services,
    snapshot: &SnapshotId,
    repo: &std::path::Path,
    branch: &str,
) -> anyhow::Result<String> {
    let repo = std::path::absolute(repo).context("resolving the repository path")?;
    // Commits are the user's when the target repository names them.
    let identity = repo
        .exists()
        .then(|| git::user_identity(&repo).ok())
        .flatten();
    Ok(git::export_with(
        &services.store,
        snapshot,
        &repo,
        branch,
        &git::ExportOptions {
            identity,
            ..Default::default()
        },
    )?
    .commit
    .to_string())
}
