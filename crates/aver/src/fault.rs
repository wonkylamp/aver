//! Named crash points for the custody tests (`tests/custody`). A build with
//! the `faults` feature reads `AVER_FAULT=point[#n]=action,...` and, when
//! the operation reaches `point` (its `n`th time in this process, or every
//! time without `#n`), it:
//!
//! - `abort`: exits at once with status 137, without destructors or
//!   flushes, as a SIGKILL at that line would;
//! - `enospc`: fails with "No space left on device";
//! - `pause`: writes `$AVER_FAULT_DIR/<point>.paused` holding the pid and
//!   waits until `<point>.go` exists, so a test can kill the process, edit
//!   files or start a second command at exactly that moment.
//!
//! Every other build compiles [`point`] to nothing and ignores the variable.

use std::io;

/// Every point an operation passes, as `fault::point` names it.
pub const POINTS: &[&str] = &[
    // aver accept (a run's attempt, or a workspace directory).
    "accept.journaled",
    "accept.committed",
    "accept.decided",
    "export.streamed",
    "export.mapped",
    "export.ref",
    "accept.released",
    "accept.saved",
    // aver accept --pr.
    "pr.pushed",
    "pr.created",
    // aver combine; `combine.accepted` once per input it accepts.
    "combine.merged",
    "combine.verified",
    "combine.accepted",
    "combine.committed",
    "combine.exported",
    // aver rerun.
    "rerun.onto_accepted",
    "rerun.recorded",
    // aver integrate / aver setup, and --remove.
    "integrate.recorded",
    "integrate.file",
    "remove.file",
    "remove.record",
    // Host state files.
    "run.save",
    "batch.save",
    // A disk workspace's sync: the scan's blobs read, the overlay committed
    // (the stat cache is saved after it).
    "sync.blobs",
    "sync.committed",
    // aver verify: the plan made, a check's process group started, the
    // receipt signed and about to be stored, and stored.
    "verify.planned",
    "proc.spawned",
    "verify.before_receipt",
    "verify.after_receipt",
    // A side's slot: cloned and renamed into place (its source record not
    // yet written), and its swap to a tree journaled, one file written, and
    // the journal closed.
    "slot.cloned",
    "swap.journaled",
    "swap.partial",
    "swap.done",
    // Workdir::restore of an adopted checkout: one tracked path put back,
    // and every path back with the journal not yet removed.
    "workdir.restore.partial",
    "workdir.restore.done",
    // Workspace lifecycle: aver workspace new (each directory of --count),
    // rm, gc, restore, and every write of a file in a directory's .aver/.
    "materialize.cloned",
    "materialize.view",
    "materialize.recorded",
    "meta.write",
    "rm.discarded",
    "release.trashed",
    "gc.released",
    "gc.trashed",
    "restore.tmp",
    "restore.partial",
];

/// Passes the crash point `name`; see the module documentation.
#[cfg(not(feature = "faults"))]
#[inline(always)]
pub fn point(_name: &str) -> io::Result<()> {
    Ok(())
}

#[cfg(feature = "faults")]
pub fn point(name: &str) -> io::Result<()> {
    imp::point(name)
}

#[cfg(feature = "faults")]
mod imp {
    use std::collections::HashMap;
    use std::io;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Action {
        Abort,
        Enospc,
        Pause,
    }

    struct Rule {
        point: String,
        nth: Option<u32>,
        action: Action,
    }

    fn rules() -> &'static [Rule] {
        static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
        RULES.get_or_init(|| {
            let spec = std::env::var("AVER_FAULT").unwrap_or_default();
            spec.split(',')
                .filter(|s| !s.trim().is_empty())
                .map(|s| parse(s.trim()).unwrap_or_else(|e| panic!("AVER_FAULT: {e}")))
                .collect()
        })
    }

    fn parse(s: &str) -> Result<Rule, String> {
        let (target, action) = s
            .split_once('=')
            .ok_or_else(|| format!("{s:?} is not point[#n]=action"))?;
        let action = match action {
            "abort" => Action::Abort,
            "enospc" => Action::Enospc,
            "pause" => Action::Pause,
            other => return Err(format!("{other:?} is not abort, enospc or pause")),
        };
        let (point, nth) = match target.split_once('#') {
            Some((p, n)) => (
                p,
                Some(n.parse().map_err(|_| format!("{n:?} is not a count"))?),
            ),
            None => (target, None),
        };
        if !super::POINTS.contains(&point) {
            return Err(format!("{point:?} is not in fault::POINTS"));
        }
        Ok(Rule {
            point: point.to_owned(),
            nth,
            action,
        })
    }

    pub fn point(name: &str) -> io::Result<()> {
        debug_assert!(super::POINTS.contains(&name), "{name} is not in POINTS");
        static HITS: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
        let rules = rules();
        if rules.is_empty() {
            return Ok(());
        }
        let hit = {
            let mut hits = HITS.get_or_init(Default::default).lock().unwrap();
            let n = hits.entry(name.to_owned()).or_default();
            *n += 1;
            *n
        };
        let Some(rule) = rules
            .iter()
            .find(|r| r.point == name && r.nth.is_none_or(|n| n == hit))
        else {
            return Ok(());
        };
        match rule.action {
            // SAFETY: ends the process at once, as a SIGKILL would.
            Action::Abort => unsafe { libc::_exit(137) },
            Action::Enospc => Err(io::Error::from_raw_os_error(libc::ENOSPC)),
            Action::Pause => {
                let dir = PathBuf::from(
                    std::env::var_os("AVER_FAULT_DIR").expect("pause needs AVER_FAULT_DIR"),
                );
                let go = dir.join(format!("{name}.go"));
                let paused = dir.join(format!("{name}.paused"));
                let tmp = dir.join(format!(".{name}.paused.{}", std::process::id()));
                std::fs::write(&tmp, std::process::id().to_string())?;
                std::fs::rename(&tmp, &paused)?;
                while !go.exists() {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(())
            }
        }
    }
}
