//! `aver release [<lease id>…]` inside a workspace directory: gives back
//! leases (by default every lease the workspace holds). `aver release
//! --repo <lease id>…`, from anywhere, releases any task's lease as the
//! home's admin.

use aver::keys::Home;
use aver::services::Services;
use aver_core::LeaseId;

use super::workspace::Context;

#[derive(clap::Args)]
pub struct Args {
    /// Lease ids; none releases every lease this workspace holds.
    ids: Vec<LeaseId>,
    /// Release the named leases of any task, as the home's admin (aver
    /// lease --repo lists them).
    #[arg(long, requires = "ids")]
    repo: bool,
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    if args.repo {
        return overseer(home, &args.ids);
    }
    let ctx = Context::require(
        "aver release gives back a workspace's leases; aver release --repo <lease id> releases any",
    )?;
    let key = ctx.key()?;
    let coord = Services::open(&ctx.home)?.coord();
    let ids = if args.ids.is_empty() {
        coord
            .leases(&key.id(), &ctx.file.workspace)?
            .into_iter()
            .filter(|h| h.mine)
            .map(|h| h.lease.id)
            .collect()
    } else {
        args.ids
    };
    if ids.is_empty() {
        println!("This workspace holds no leases.");
    }
    for id in ids {
        coord.release(&key.id(), &id, false)?;
        println!("released {id}");
    }
    Ok(())
}

/// Releases `ids` whoever holds them, as the home's admin.
fn overseer(home: &Home, ids: &[LeaseId]) -> anyhow::Result<()> {
    let admin = aver::services::key_in(home, None)?;
    let coord = Services::open(home)?.coord();
    for id in ids {
        coord.release(&admin.id(), id, true)?;
        println!("released {id}");
    }
    Ok(())
}
