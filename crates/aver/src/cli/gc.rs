//! `aver gc`: trash released and orphaned workspace directories, mark
//! vanished ones lost, drop idle checkouts, reclaim the build artifacts of
//! workspaces idle for a day and the slots nothing needs (those of
//! released workspaces, and base slots of no live workspace idle for a
//! week), and empty the trash. `--builds` reclaims more: the build
//! artifacts of workspaces idle for 10 minutes. Sizes are apparent: a
//! copy-on-write clone counts the blocks it shares. `--accepted` also
//! removes the directories no one can work in any more: those of accepted
//! and discarded runs and accepted workspace directories, unless their Git
//! view holds commits the repository lacks, their commit was never exported
//! to a branch, or the directory changed since the commit. Always, a
//! directory equal to a commit of its Git view that the repository already
//! merged goes as `aver workspace rm` would remove it. Nothing else a live
//! workspace uses is removed. Base records `aver ci` wrote in the home
//! are removed after 14 days unwritten, and beyond the 64 newest.

use std::os::unix::fs::PermissionsExt;

use anyhow::{bail, Context as _};

use aver::disk::{GcPolicy, GcReport};
use aver::keys::Home;

/// How long a workspace must be idle before `aver gc --builds` reclaims its
/// build artifacts: long enough that no build is running in it.
const BUILDS_IDLE_S: u64 = 600;

#[derive(clap::Args)]
pub struct Args {
    /// Also reclaim the build artifacts (Cargo `target` directories, `aver
    /// check` copies) of every workspace idle for 10 minutes, not only a
    /// day.
    #[arg(long)]
    builds: bool,
    /// Also remove the directories of accepted and discarded runs (every
    /// attempt's) and of accepted workspace directories, keeping any whose
    /// Git view holds commits the repository lacks, whose commit no branch
    /// holds, or that changed since the commit.
    #[arg(long)]
    accepted: bool,
    #[arg(long)]
    json: bool,
    /// Delete the Aver home DIR instead (a throwaway or old home): it must
    /// hold `keys/` and the state database, with no other aver process
    /// using it. Read-only stores in it are made writable first.
    #[arg(long, value_name = "DIR", conflicts_with_all = ["builds", "accepted"])]
    delete_home: Option<std::path::PathBuf>,
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    if let Some(dir) = &args.delete_home {
        return delete_home(dir);
    }
    let mut policy = GcPolicy::default();
    if args.builds {
        policy.build_idle = std::time::Duration::from_secs(BUILDS_IDLE_S);
    }
    policy.accepted = args.accepted;
    let lock = home.lock(|| {
        eprintln!(
            "aver: waiting for another aver process using {}",
            home.root().display()
        )
    })?;
    aver::keys::init_locked(home, &lock)?;
    let (_rt, api) = super::open(home)?;
    let merged = super::workspace::remove_merged(home, &api)?;
    let report: GcReport = api.services().workspaces().gc(&policy)?;
    let records_removed = aver::ci::record::prune(
        &aver::ci::record::home_dir(home.root()),
        aver::ci::record::HOME_RECORD_AGE,
        aver::ci::record::HOME_RECORDS_KEPT,
    )
    .context("pruning the base records")?;
    if args.json {
        let mut json = serde_json::to_value(&report)?;
        json["records_removed"] = records_removed.into();
        json["merged_removed"] = serde_json::to_value(&merged)?;
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        println!(
            "removed {} merged workspace directories, trashed {} directories, marked {} lost, removed {} checkouts, reclaimed {} build directories ({}), removed {} slots ({}), deleted {} trash entries",
            merged.len(),
            report.trashed,
            report.lost,
            report.checkouts_removed,
            report.builds_reclaimed,
            gib(report.build_bytes),
            report.slots_removed,
            gib(report.slot_bytes),
            report.trash_emptied
        );
        for m in &merged {
            println!(
                "removed {} (matches {})",
                m.dir.display(),
                &m.commit[..12.min(m.commit.len())]
            );
        }
        if records_removed > 0 {
            println!("removed {records_removed} base records");
        }
        if args.accepted {
            println!(
                "removed {} directories of accepted or discarded work",
                report.finished_released
            );
            for kept in &report.finished_kept {
                println!("kept {kept}");
            }
        }
    }
    Ok(())
}

/// `bytes` in GiB with one decimal.
fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / f64::from(1u32 << 30))
}

/// `aver gc --delete-home <dir>`: removes a whole Aver home. Dependency
/// stores and checkouts inside are read-only on purpose, so `rm -rf` alone
/// fails on them.
fn delete_home(dir: &std::path::Path) -> anyhow::Result<()> {
    let dir =
        std::fs::canonicalize(dir).with_context(|| format!("{} does not exist", dir.display()))?;
    let target = Home::new(&dir);
    if !target.keys_dir().is_dir() || !target.db_path().is_file() {
        bail!(
            "{} is not an Aver home (it needs keys/ and {}); refusing to delete it",
            dir.display(),
            target
                .db_path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
    }
    if aver::recover::others_alive(&target) {
        bail!(
            "another aver process is using {}; wait for it to finish",
            dir.display()
        );
    }
    let root = std::fs::metadata(&dir)?;
    std::fs::set_permissions(
        &dir,
        std::fs::Permissions::from_mode(root.permissions().mode() | 0o700),
    )?;
    let Some(lock) = target.try_lock()? else {
        bail!(
            "another aver process is using {}; wait for it to finish",
            dir.display()
        );
    };
    make_writable(&dir).with_context(|| format!("making {} writable", dir.display()))?;
    std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    drop(lock);
    println!("deleted the Aver home {}", dir.display());
    Ok(())
}

/// Gives the owner write access to every directory and file under `dir`,
/// without following symlinks.
fn make_writable(dir: &std::path::Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    let mode = meta.permissions().mode() | 0o200 | if meta.is_dir() { 0o100 } else { 0 };
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode))?;
    if meta.is_dir() {
        for entry in std::fs::read_dir(dir)? {
            make_writable(&entry?.path())?;
        }
    }
    Ok(())
}
