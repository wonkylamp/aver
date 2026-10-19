//! Progress of a verification or a preview while it runs: a log of run
//! starts and ends that the verifier writes ([`Log`]), and the stderr
//! lines `aver verify` and `aver check` print from it ([`Printer`]), so an
//! agent waiting minutes for a Rust build sees that something happens.
//!
//! The command that verifies prints the log from a thread of its own
//! ([`print_until`]). The printer announces a run only once it has been
//! running for [`ANNOUNCE_AFTER`], so cache hits print nothing but one
//! count, says how each run ended, and prints a heartbeat after
//! [`HEARTBEAT`] without a line.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::verify::Side;

/// A run is announced when it is still running this long after it started.
pub const ANNOUNCE_AFTER: Duration = Duration::from_secs(2);
/// Silence after which the printer says what is still running.
pub const HEARTBEAT: Duration = Duration::from_secs(30);
/// Most events one read of a log returns.
const MAX_EVENTS: usize = 500;

/// One run starting or ending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Milliseconds since the log began.
    pub at_ms: u64,
    pub side: Side,
    pub check: String,
    pub run: u8,
    #[serde(flatten)]
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Kind {
    /// Waiting for an executor slot, `position` runs ahead of it.
    Queued {
        position: u32,
    },
    Started,
    Finished {
        /// `pass`, `fail` or `infra`.
        ending: String,
        cached: bool,
        ms: u64,
    },
}

/// The events of one verification, in order.
#[derive(Debug)]
pub struct Log {
    started: Instant,
    events: Mutex<Vec<Event>>,
}

impl Default for Log {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            events: Mutex::new(Vec::new()),
        }
    }
}

impl Log {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Milliseconds since the log began.
    pub fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    pub fn push(&self, side: Side, check: &str, run: u8, kind: Kind) {
        let event = Event {
            at_ms: self.now_ms(),
            side,
            check: check.to_owned(),
            run,
            kind,
        };
        self.events
            .lock()
            .expect("the progress log is never poisoned")
            .push(event);
    }

    /// The events after the first `after`, at most [`MAX_EVENTS`] of them.
    pub fn since(&self, after: usize) -> Vec<Event> {
        let events = self
            .events
            .lock()
            .expect("the progress log is never poisoned");
        events
            .iter()
            .skip(after)
            .take(MAX_EVENTS)
            .cloned()
            .collect()
    }
}

/// What a side is called in progress lines.
pub fn side_name(side: Side) -> &'static str {
    match side {
        Side::Base => "the original code",
        Side::Head => "your code",
        Side::HeadTests => "your code with your test files",
        Side::Transplant => "the original code with your test files",
        Side::Ablation => "your code with one change reverted",
    }
}

/// What a side of `aver check`'s native runs is called.
fn native_side_name(side: Side) -> &'static str {
    match side {
        Side::HeadTests | Side::Ablation => "your copy",
        Side::Head => "your code with the original tests",
        Side::Base | Side::Transplant => "the original code",
    }
}

/// Turns events into stderr lines: one short status line (`running 3 ·
/// waiting 2 · done 5`) when the counts change and something has run or
/// waited for [`ANNOUNCE_AFTER`], a line while every open run waits for a
/// slot behind other tasks' runs, one count of cache hits, and a
/// heartbeat after [`HEARTBEAT`] of silence naming the longest run.
pub struct Printer {
    /// `aver verify` or `aver check`.
    prefix: &'static str,
    /// Where runs run: `in a sandbox`, `natively`, `here`.
    place: &'static str,
    /// What each side is called.
    names: fn(Side) -> &'static str,
    /// Runs started and not finished, with when they started.
    open: BTreeMap<(u8, String, u8), u64>,
    /// Runs waiting for a slot, with the runs ahead and since when.
    queued: BTreeMap<(u8, String, u8), (u32, u64)>,
    finished: u32,
    cached: u32,
    cached_said: u32,
    last_line_ms: u64,
    /// The counts the last status line gave.
    said: Option<(usize, usize, u32)>,
    /// The runs ahead the last waiting line gave.
    ahead_said: Option<u32>,
    /// Print a line for every run that ends (`-v`); otherwise only what
    /// still runs, so a passing change never reads as "failed".
    verbose: bool,
}

fn key(event: &Event) -> (u8, String, u8) {
    (event.side as u8, event.check.clone(), event.run)
}

fn side_of(n: u8) -> Side {
    match n {
        0 => Side::Base,
        1 => Side::Head,
        2 => Side::HeadTests,
        3 => Side::Transplant,
        _ => Side::Ablation,
    }
}

/// `1m05s`, `42s`.
pub fn human(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else if ms >= 10_000 {
        format!("{secs}s")
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

impl Printer {
    /// A printer for `aver verify`, whose runs are sandboxes.
    pub fn new(prefix: &'static str) -> Self {
        Self::at(prefix, "in a sandbox")
    }

    /// A printer for `aver check`, whose runs are native: your copy and the
    /// original code with your test files.
    pub fn native(prefix: &'static str) -> Self {
        Self {
            names: native_side_name,
            ..Self::at(prefix, "here")
        }
    }

    /// A printer whose runs run at `place`.
    pub fn at(prefix: &'static str, place: &'static str) -> Self {
        Self {
            prefix,
            place,
            names: side_name,
            open: BTreeMap::new(),
            queued: BTreeMap::new(),
            finished: 0,
            cached: 0,
            cached_said: 0,
            last_line_ms: 0,
            said: None,
            ahead_said: None,
            verbose: false,
        }
    }

    /// With `verbose`, a line for every run that ends, saying how it ended.
    /// Without, runs that end say nothing: a run of the original code that
    /// fails is often the proof working (a test that fails without the
    /// change), and the report judges every failure.
    pub fn verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    fn run_name(names: fn(Side) -> &'static str, check: &str, side: Side, run: u8) -> String {
        if run > 1 {
            format!("{check} on {}, run {run}", names(side))
        } else {
            format!("{check} on {}", names(side))
        }
    }

    /// The lines to print for `events`, which arrived by `now_ms`, and for
    /// what has been running or waiting since.
    pub fn feed(&mut self, events: &[Event], now_ms: u64) -> Vec<String> {
        let mut lines = Vec::new();
        for event in events {
            match &event.kind {
                Kind::Queued { position } => {
                    let since = self.queued.get(&key(event)).map_or(event.at_ms, |q| q.1);
                    self.queued.insert(key(event), (*position, since));
                }
                Kind::Started => {
                    self.queued.remove(&key(event));
                    self.open.insert(key(event), event.at_ms);
                }
                Kind::Finished { ending, cached, ms } => {
                    self.queued.remove(&key(event));
                    self.open.remove(&key(event));
                    self.finished += 1;
                    if *cached {
                        self.cached += 1;
                        continue;
                    }
                    if !self.verbose {
                        continue;
                    }
                    let name = Self::run_name(self.names, &event.check, event.side, event.run);
                    lines.push(match ending.as_str() {
                        // The original code failing is how a new test proves
                        // itself, so it never reads as a failure here.
                        "fail" if matches!(event.side, Side::Base | Side::Transplant) => format!(
                            "{}: {name}: failing, as expected for a new test; the report judges each failure ({})",
                            self.prefix,
                            human(*ms),
                        ),
                        ending => format!(
                            "{}: {name} ended after {}, {}",
                            self.prefix,
                            human(*ms),
                            match ending {
                                "pass" => "all passing",
                                "fail" => "some failing (the report says whose)",
                                _ => "an infrastructure problem",
                            }
                        ),
                    });
                }
            }
        }
        if self.cached > self.cached_said {
            lines.insert(
                0,
                format!(
                    "{}: {} from the cache",
                    self.prefix,
                    if self.cached == 1 {
                        "1 run".to_owned()
                    } else {
                        format!("{} runs", self.cached)
                    }
                ),
            );
            self.cached_said = self.cached;
        }
        let due = |at: u64| now_ms.saturating_sub(at) >= ANNOUNCE_AFTER.as_millis() as u64;
        // Every open run waits behind other tasks' runs: say how many.
        let waiting_ahead = self
            .queued
            .values()
            .filter(|(_, since)| due(*since))
            .map(|(ahead, _)| *ahead)
            .min();
        match waiting_ahead {
            Some(ahead) if self.open.is_empty() && self.ahead_said != Some(ahead) => {
                self.ahead_said = Some(ahead);
                lines.push(format!(
                    "{}: waiting for a slot: {ahead} ahead (other tasks)",
                    self.prefix
                ));
            }
            None => self.ahead_said = None,
            _ => {}
        }
        let counts = (self.open.len(), self.queued.len(), self.finished);
        let long = self.open.values().any(|at| due(*at)) || waiting_ahead.is_some();
        if long && self.said != Some(counts) && !(self.open.is_empty() && waiting_ahead.is_some()) {
            self.said = Some(counts);
            lines.push(format!(
                "{}: running {} · waiting {} · done {} ({})",
                self.prefix, counts.0, counts.1, counts.2, self.place
            ));
        }
        if lines.is_empty()
            && !self.open.is_empty()
            && now_ms.saturating_sub(self.last_line_ms) >= HEARTBEAT.as_millis() as u64
        {
            let ((side, check, run), at) = self
                .open
                .iter()
                .min_by_key(|(_, at)| **at)
                .expect("open is not empty");
            lines.push(format!(
                "{}: running {} · waiting {} · done {}; longest: {} ({})",
                self.prefix,
                counts.0,
                counts.1,
                counts.2,
                Self::run_name(self.names, check, side_of(*side), *run),
                human(now_ms.saturating_sub(*at))
            ));
        }
        if !lines.is_empty() {
            self.last_line_ms = now_ms;
        }
        lines
    }
}

/// Calls `f` with a new log that `printer`, when given, prints on stderr
/// from another thread until `f` returns.
pub fn printed<T>(printer: Option<Printer>, f: impl FnOnce(Option<Arc<Log>>) -> T) -> T {
    let Some(printer) = printer else {
        return f(None);
    };
    let log = Log::new();
    let done = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| print_until(&log, printer, &done));
        let out = f(Some(log.clone()));
        done.store(true, std::sync::atomic::Ordering::SeqCst);
        out
    })
}

/// Prints `log`'s progress on stderr until `done` is set.
pub fn print_until(log: &Log, mut printer: Printer, done: &std::sync::atomic::AtomicBool) {
    use std::sync::atomic::Ordering;
    let mut seen = 0;
    while !done.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(200));
        let events = log.since(seen);
        seen += events.len();
        for line in printer.feed(&events, log.now_ms()) {
            eprintln!("{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(at_ms: u64, side: Side, check: &str, run: u8, kind: Kind) -> Event {
        Event {
            at_ms,
            side,
            check: check.into(),
            run,
            kind,
        }
    }

    fn done(ending: &str, cached: bool, ms: u64) -> Kind {
        Kind::Finished {
            ending: ending.into(),
            cached,
            ms,
        }
    }

    /// Cache hits are counted once; runs that take a while bring one short
    /// status line, again only when the counts change.
    #[test]
    fn one_short_status_line_and_one_cache_count() {
        let mut p = Printer::new("aver verify");
        let lines = p.feed(
            &[
                ev(0, Side::Base, "mcp", 1, Kind::Started),
                ev(5, Side::Base, "mcp", 1, done("pass", true, 0)),
                ev(6, Side::Base, "rust", 1, Kind::Started),
                ev(6, Side::Head, "rust", 1, Kind::Started),
            ],
            100,
        );
        assert_eq!(lines, vec!["aver verify: 1 run from the cache"]);
        assert_eq!(
            p.feed(&[], 2500),
            vec!["aver verify: running 2 · waiting 0 · done 1 (in a sandbox)"]
        );
        assert!(p.feed(&[], 3000).is_empty(), "nothing changed");
        let lines = p.feed(
            &[ev(4000, Side::Base, "rust", 1, done("fail", false, 4000))],
            4000,
        );
        assert_eq!(
            lines,
            vec!["aver verify: running 1 · waiting 0 · done 2 (in a sandbox)"]
        );
        assert!(lines.iter().all(|l| !l.contains("fail")), "{lines:?}");
    }

    /// While every run of this verify waits behind other tasks' runs, the
    /// line says how many are ahead; once one runs, the status line counts
    /// the waiting ones.
    #[test]
    fn a_queued_run_says_how_many_are_ahead() {
        let mut p = Printer::new("aver verify");
        let queued = |at, position| ev(at, Side::Head, "unit", 1, Kind::Queued { position });
        assert!(p.feed(&[queued(0, 2)], 100).is_empty(), "not yet");
        assert_eq!(
            p.feed(&[], 2_100),
            vec!["aver verify: waiting for a slot: 2 ahead (other tasks)"]
        );
        assert_eq!(
            p.feed(&[queued(3_000, 1)], 3_000),
            vec!["aver verify: waiting for a slot: 1 ahead (other tasks)"]
        );
        let lines = p.feed(
            &[
                ev(4_000, Side::Head, "unit", 1, Kind::Started),
                ev(4_000, Side::Base, "unit", 1, Kind::Queued { position: 0 }),
            ],
            6_500,
        );
        assert_eq!(
            lines,
            vec!["aver verify: running 1 · waiting 1 · done 0 (in a sandbox)"]
        );
    }

    /// The original code with the change's tests failing is the proof
    /// working, never "failed after"; without `-v` a run that ends says
    /// nothing.
    #[test]
    fn a_failing_original_side_reads_as_expected() {
        let mut p = Printer::native("aver check").verbose(true);
        let lines = p.feed(
            &[
                ev(0, Side::Transplant, "unit", 1, Kind::Started),
                ev(300, Side::Transplant, "unit", 1, done("fail", false, 300)),
            ],
            300,
        );
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with(
                "aver check: unit on the original code: failing, as expected for a new test"
            ),
            "{lines:?}"
        );
        let mut quiet = Printer::native("aver check");
        let lines = quiet.feed(
            &[
                ev(0, Side::Transplant, "unit", 1, Kind::Started),
                ev(300, Side::Transplant, "unit", 1, done("fail", false, 300)),
            ],
            400,
        );
        assert!(lines.is_empty(), "{lines:?}");
    }

    #[test]
    fn silence_brings_a_heartbeat_naming_the_longest_run() {
        let mut p = Printer::new("aver verify");
        p.feed(&[ev(0, Side::Head, "rust", 2, Kind::Started)], 0);
        assert_eq!(p.feed(&[], 2_000).len(), 1, "announced");
        assert!(p.feed(&[], 20_000).is_empty(), "no heartbeat before 30 s");
        assert_eq!(
            p.feed(&[], 32_000),
            vec!["aver verify: running 1 · waiting 0 · done 0; longest: rust on your code, run 2 (32s)"]
        );
        assert!(p.feed(&[], 40_000).is_empty(), "rate-limited");
    }

    #[test]
    fn a_quick_cached_run_is_never_announced() {
        let mut p = Printer::new("aver check");
        let lines = p.feed(
            &[
                ev(0, Side::Base, "a", 1, Kind::Started),
                ev(10, Side::Base, "a", 1, done("pass", true, 0)),
            ],
            5000,
        );
        assert_eq!(lines, vec!["aver check: 1 run from the cache"]);
    }
}
