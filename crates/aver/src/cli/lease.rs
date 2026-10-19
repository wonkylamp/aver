//! `aver lease <glob>… --reason "<why>"` inside a workspace directory:
//! reserve paths so agents working on other tasks in the same repository
//! keep out (ARCHITECTURE.md §2.11). Without globs, lists the live leases.
//! `aver release` gives them back. `aver lease --repo [DIR]`, from
//! anywhere, lists every live lease in a repository.

use anyhow::bail;
use aver::coord::{expires_in, HeldLease, LeaseMode, LeaseRequest};
use aver::keys::Home;
use aver_core::PathGlob;
use serde_json::json;

use super::workspace::Context;

#[derive(clap::Args)]
pub struct Args {
    /// Repo-relative path globs (`src/db`, `src/**/*.sql`); a directory
    /// covers everything under it. None lists the live leases.
    globs: Vec<String>,
    /// Why, in a few words other agents will read.
    #[arg(long)]
    reason: Option<String>,
    /// Only say you work here: other tasks may still take shared leases and
    /// edit. The default, exclusive, keeps them out.
    #[arg(long)]
    shared: bool,
    /// How long the lease lasts after your last activity (each sync renews
    /// it): 30m, 2h.
    #[arg(long, value_parser = super::run::parse_duration)]
    ttl: Option<u64>,
    /// List every live lease in a repository, from anywhere: DIR, or the
    /// repository holding the current directory.
    #[arg(long, value_name = "DIR", num_args = 0..=1, conflicts_with_all = ["globs", "reason", "shared", "ttl"])]
    repo: Option<Option<std::path::PathBuf>>,
    /// With --repo: print JSON.
    #[arg(long, requires = "repo")]
    json: bool,
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    if let Some(dir) = args.repo {
        let repo = super::notices::repo_root(dir.as_deref())?;
        let leases = super::notices::repo_view(home, &repo)?.leases;
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({ "repo": repo, "leases": leases }))?
            );
            return Ok(());
        }
        if leases.is_empty() {
            println!("No live leases in {}.", repo.display());
        }
        for held in &leases {
            println!("{}", describe(held));
        }
        return Ok(());
    }
    let ctx = Context::require(
        "aver lease reserves paths for a workspace's task; aver lease --repo lists a repository's leases",
    )?;
    let key = ctx.key()?;
    let services = aver::services::Services::open(&ctx.home)?;
    let coord = services.coord();
    if args.globs.is_empty() {
        let leases = coord.leases(&key.id(), &ctx.file.workspace)?;
        if leases.is_empty() {
            println!("No live leases in this repository.");
        }
        for held in &leases {
            println!("{}", describe(held));
        }
        return Ok(());
    }
    let Some(reason) = args.reason.filter(|r| !r.trim().is_empty()) else {
        bail!("say why other agents should keep out: aver lease <glob>… --reason \"<why>\"");
    };
    let paths = args
        .globs
        .iter()
        .map(|g| {
            g.trim_end_matches('/')
                .trim_start_matches("./")
                .parse::<PathGlob>()
                .map_err(|e| anyhow::anyhow!("{g}: {e}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let held = coord.acquire(
        &key.id(),
        &ctx.file.workspace,
        LeaseRequest {
            paths,
            mode: if args.shared {
                LeaseMode::Shared
            } else {
                LeaseMode::Exclusive
            },
            reason,
            ttl_ms: args.ttl,
        },
    )?;
    println!(
        "Leased: {}\nIt lasts while you keep working (each aver verify or sync renews it); aver release {} gives it back.",
        describe(&held),
        held.lease.id
    );
    Ok(())
}

/// One line per lease: id, mode, globs, holder, expiry and reason.
pub(super) fn describe(held: &HeldLease) -> String {
    let globs: Vec<&str> = held.lease.paths.iter().map(|g| g.as_str()).collect();
    let whose = if held.mine {
        "yours".to_owned()
    } else {
        let task = held
            .title
            .as_deref()
            .map(|t| format!("{:?}", aver::run::report::one_line_plain(t)))
            .unwrap_or_else(|| "-".into());
        if held.sibling {
            format!("another attempt at this task, {task}")
        } else {
            format!("task {task}")
        }
    };
    format!(
        "{} {} {} ({whose}, expires {}): {}",
        held.lease.id,
        held.lease.mode.as_str(),
        globs.join(" "),
        expires_in(held.lease.expires_at),
        aver::run::report::one_line_plain(&held.lease.reason)
    )
}
