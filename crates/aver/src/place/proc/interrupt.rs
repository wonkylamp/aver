//! Ctrl-C (wp2-plan.md §3.6): the one SIGINT and SIGTERM handler, for
//! every command. A run's process group lives outside the terminal's, so a
//! signal to `aver` alone never reaches it; the handler signals each group
//! [`LIVE`] holds instead. The first signal ends a command with no group
//! running at once (exit 130); otherwise it asks every run to stop
//! ([`requested`]), and the runs end as interrupted, storing nothing. A
//! second signal kills every group and exits 130.
//!
//! `aver run` and `aver rerun` are [`graceful`]: their first signal only
//! asks, so the supervisor stops its agents itself.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

/// The exit code of a command ended by a signal.
pub const EXIT: u8 = 130;

/// Process groups a registry holds at most: far above the runs a process
/// starts at once.
pub const CAPACITY: usize = 64;

/// A slot claimed for a group not started yet: the handler counts it as
/// running but signals nothing.
const CLAIMED: i32 = -1;

static STOP: AtomicBool = AtomicBool::new(false);
static GRACEFUL: AtomicBool = AtomicBool::new(false);

/// The running process groups of this process's runs.
pub static LIVE: Registry = Registry::new();

/// Process groups, lock-free, so the signal handler reads them.
pub struct Registry([AtomicI32; CAPACITY]);

/// A claimed slot of a [`Registry`]: freed when dropped unless [`set`].
///
/// [`set`]: Claim::set
pub struct Claim<'a> {
    registry: &'a Registry,
    at: usize,
}

impl Registry {
    pub const fn new() -> Self {
        Self([const { AtomicI32::new(0) }; CAPACITY])
    }

    /// A slot for a group about to start, claimed before it starts so a
    /// signal in between finds the registry not empty. `None` when every
    /// slot is taken.
    pub fn claim(&self) -> Option<Claim<'_>> {
        self.0
            .iter()
            .position(|slot| {
                slot.compare_exchange(0, CLAIMED, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            })
            .map(|at| Claim { registry: self, at })
    }

    /// Forgets group `pgid`, once nothing of it runs.
    pub fn clear(&self, pgid: i32) {
        for slot in &self.0 {
            if slot
                .compare_exchange(pgid, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return;
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|slot| slot.load(Ordering::SeqCst) == 0)
    }

    /// The groups held, in slot order.
    pub fn groups(&self) -> impl Iterator<Item = i32> + '_ {
        self.0
            .iter()
            .map(|slot| slot.load(Ordering::SeqCst))
            .filter(|&g| g > 0)
    }

    /// Sends `signal` to every group held. Async-signal-safe: atomics and
    /// `kill` only.
    fn signal(&self, signal: libc::c_int) {
        for slot in &self.0 {
            let group = slot.load(Ordering::SeqCst);
            if group > 0 {
                // SAFETY: kill is async-signal-safe; the group is one this
                // process started and has not reaped.
                unsafe { libc::kill(-group, signal) };
            }
        }
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Claim<'_> {
    /// Holds group `pgid` in the claimed slot until [`Registry::clear`].
    pub fn set(self, pgid: i32) {
        self.registry.0[self.at].store(pgid, Ordering::SeqCst);
        std::mem::forget(self);
    }
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.registry.0[self.at].store(0, Ordering::SeqCst);
    }
}

/// Whether a signal asked this process to stop.
pub fn requested() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// The flag [`requested`] reads, for a loop that takes one.
pub fn flag() -> &'static AtomicBool {
    &STOP
}

/// Makes the first signal only ask, even with nothing running: for a
/// command that stops its own work (`aver run`, `aver rerun`).
pub fn graceful() {
    GRACEFUL.store(true, Ordering::SeqCst);
}

/// Installs the handler for SIGINT and SIGTERM.
pub fn install() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: the handler only touches atomics and calls kill and
        // _exit, all async-signal-safe.
        unsafe {
            libc::signal(signal, on_signal as *const () as libc::sighandler_t);
        }
    }
}

extern "C" fn on_signal(_: libc::c_int) {
    if STOP.swap(true, Ordering::SeqCst) {
        LIVE.signal(libc::SIGKILL);
        // SAFETY: _exit is async-signal-safe.
        unsafe { libc::_exit(EXIT.into()) };
    }
    if LIVE.is_empty() && !GRACEFUL.load(Ordering::SeqCst) {
        // SAFETY: as above.
        unsafe { libc::_exit(EXIT.into()) };
    }
    LIVE.signal(libc::SIGTERM);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_group_is_held_until_cleared() {
        let live = Registry::new();
        assert!(live.is_empty());
        live.claim().unwrap().set(4242);
        live.claim().unwrap().set(4343);
        assert_eq!(live.groups().collect::<Vec<_>>(), [4242, 4343]);
        live.clear(4242);
        assert_eq!(live.groups().collect::<Vec<_>>(), [4343]);
        live.clear(4242);
        live.clear(4343);
        assert!(live.is_empty());
    }

    #[test]
    fn a_claim_counts_as_running_until_set_or_dropped() {
        let live = Registry::new();
        let claim = live.claim().unwrap();
        assert!(!live.is_empty(), "a group about to start is waited for");
        assert_eq!(live.groups().count(), 0, "and never signalled");
        drop(claim);
        assert!(live.is_empty());
    }

    #[test]
    fn a_full_registry_refuses_the_next_group_and_loses_none() {
        let live = Registry::new();
        for g in 1..=CAPACITY as i32 {
            live.claim().expect("a free slot").set(1000 + g);
        }
        assert!(live.claim().is_none());
        assert_eq!(live.groups().count(), CAPACITY);
        live.clear(1001);
        live.claim().expect("the freed slot").set(2000);
        let groups: Vec<i32> = live.groups().collect();
        assert_eq!(groups.len(), CAPACITY);
        assert!(groups.contains(&2000) && !groups.contains(&1001));
    }
}
