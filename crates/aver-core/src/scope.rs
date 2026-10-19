//! What a verdict covers (§1.9.5): which checks ran and how their results
//! were read, which did not run, which changed files no check exercises,
//! which proofs are weak, and where the runs executed. Derived from what a
//! receipt already records, so every surface (the agent report, `aver ci`'s
//! summary, MCP, commit messages) states the same limits in the same words.
//!
//! [`Scope::lines`] are Aver's own words. [`Scope::names`] carry repository
//! strings (check ids and paths), which every renderer delimits as data.

use serde::{Deserialize, Serialize};

use crate::receipt::{
    Level, Network, ProvenanceMode, ReceiptBody, Uid, BUILT_CHECK, JUDGING_CHECK,
};
use crate::CriterionId;

/// The most names one line of [`Scope::names`] lists; the rest are counted.
pub const MAX_NAMES: usize = 3;
/// The most weak-proof lines [`Scope::lines`] gives; the rest are counted.
const MAX_WEAK_LINES: usize = 3;
/// Entry ids that are not checks: judging-file, protected-path, lease,
/// sensitive-path and built-artifact entries. Only read for a receipt
/// without `levels`.
const NOT_CHECKS: [&str; 5] = [
    JUDGING_CHECK,
    "protected",
    "lease",
    "sensitive",
    BUILT_CHECK,
];

/// Why a proof is weak, from the unsigned diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WeakKind {
    /// The proving tests failed on the original code only because a module
    /// or export they import is missing there.
    MissingModule,
    /// They failed there only because a name they use is missing.
    MissingName,
    /// They did not build on the original code.
    BuildError,
    /// They did not build there because they call what the change adds:
    /// normal for a new API, and the head-tests run checked the assertions.
    NewApi,
}

/// A proven criterion whose proof is weak.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Weak {
    pub criterion: CriterionId,
    pub kind: WeakKind,
}

/// How many of a verification's runs came from the run cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Reuse {
    pub runs: u32,
    pub reused: u32,
    /// Some reused run was cached before the running verifier was
    /// installed, so an older verifier produced it.
    pub older_verifier: bool,
}

/// A side whose runs ran only the test targets the change makes, the
/// others' results taken from a side that ran them all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Narrowing {
    pub side: NarrowSide,
    /// Test targets run, and in all, summed over the checks narrowed.
    pub ran: u32,
    pub of: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NarrowSide {
    Transplant,
    HeadTests,
}

/// Hunk necessity (§1.9.4) on a verdict that would be `PASS`: each source
/// hunk reverted alone under the proving tests, or why that was not tried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Necessity {
    /// `untested` names (`<path>:<start>-<end>`) the hunks whose reversion
    /// left every proving test passing.
    Checked {
        hunks: u32,
        untested: Vec<String>,
    },
    NotChecked {
        hunks: u32,
        why: NotChecked,
    },
}

/// The most hunks the necessity phase reverts, one run each.
pub const MAX_HUNKS: usize = 8;

/// Why hunk necessity was not tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotChecked {
    NoProvingTests,
    NoSelection,
    TooManyHunks,
}

impl NotChecked {
    /// Why, as the end of a sentence.
    pub fn reason(self) -> String {
        match self {
            Self::NoProvingTests => "no test proves the change".into(),
            Self::NoSelection => "a proving test's check cannot run tests by name".into(),
            Self::TooManyHunks => format!("the change has more than {MAX_HUNKS} of them"),
        }
    }
}

/// A check that ran, and whether its result was read from the exit code
/// alone (`None` for a receipt without `levels`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ran {
    pub check: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<Level>,
}

/// Where the runs executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Where {
    pub mode: ProvenanceMode,
    pub network: Network,
    /// `None` for a receipt without provenance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<Uid>,
}

/// What one verdict covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Scope {
    /// In check order, exit-code-only checks first.
    pub ran: Vec<Ran>,
    pub skipped: Vec<String>,
    /// Changed files no check runs, the first of them, and how many in all.
    pub uncovered: Vec<String>,
    pub uncovered_total: u64,
    pub weak: Vec<Weak>,
    #[serde(rename = "where")]
    pub at: Where,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reuse: Option<Reuse>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub narrowed: Vec<Narrowing>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub necessity: Option<Necessity>,
}

impl Scope {
    /// The scope of `body`, with the weak proofs the diagnostics found and
    /// the cache reuse the timing recorded.
    pub fn of(body: &ReceiptBody, weak: &[Weak], reuse: Option<Reuse>) -> Self {
        let mut ran: Vec<Ran> = match &body.levels {
            Some(levels) => levels
                .iter()
                .map(|(check, level)| Ran {
                    check: check.as_str().to_owned(),
                    level: Some(*level),
                })
                .collect(),
            None => {
                let mut checks: Vec<String> = body
                    .checks
                    .iter()
                    .map(|c| {
                        let id = c.id.as_str();
                        id.split_once("::")
                            .map_or(id, |(check, _)| check)
                            .to_owned()
                    })
                    .filter(|c| !NOT_CHECKS.contains(&c.as_str()))
                    .collect();
                checks.sort();
                checks.dedup();
                checks
                    .into_iter()
                    .map(|check| Ran { check, level: None })
                    .collect()
            }
        };
        // Stable: within each group, check order stays.
        ran.sort_by_key(|r| r.level != Some(Level::Exit));
        let at = match &body.provenance {
            Some(p) => Where {
                mode: p.mode,
                network: p.network,
                uid: Some(p.uid),
            },
            None => Where {
                mode: match &body.executor {
                    Some(e) if e.starts_with(crate::NATIVE_EXECUTOR_PREFIX) => {
                        ProvenanceMode::Local
                    }
                    _ => ProvenanceMode::Sandbox,
                },
                network: Network::Off,
                uid: None,
            },
        };
        Self {
            ran,
            skipped: body
                .skipped
                .iter()
                .flat_map(|s| s.iter().map(|s| s.check.as_str().to_owned()))
                .collect(),
            uncovered: body
                .uncovered
                .iter()
                .flat_map(|u| u.items.iter().map(|p| p.as_str().to_owned()))
                .collect(),
            uncovered_total: body.uncovered.as_ref().map_or(0, |u| u.total()),
            weak: weak.to_vec(),
            at,
            reuse: reuse.filter(|r| r.runs > 0),
            narrowed: Vec::new(),
            necessity: None,
        }
    }

    /// With the sides that ran only the changed test targets.
    pub fn with_narrowed(mut self, narrowed: Vec<Narrowing>) -> Self {
        self.narrowed = narrowed;
        self
    }

    /// With what hunk necessity found.
    pub fn with_necessity(mut self, necessity: Option<Necessity>) -> Self {
        self.necessity = necessity;
        self
    }

    fn untested(&self) -> &[String] {
        match &self.necessity {
            Some(Necessity::Checked { untested, .. }) => untested,
            _ => &[],
        }
    }

    fn exit_only(&self) -> usize {
        self.ran
            .iter()
            .filter(|r| r.level == Some(Level::Exit))
            .count()
    }

    /// Nothing but the ran checks and where: the section fits in two lines
    /// and names nothing.
    pub fn compact(&self) -> bool {
        self.exit_only() == 0
            && self.skipped.is_empty()
            && self.uncovered_total == 0
            && self.weak.is_empty()
            && self.untested().is_empty()
    }

    /// The section's lines in Aver's own words: "Ran" and "Where" always,
    /// the others when they have content.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        let ran = self.ran.len();
        let total = ran + self.skipped.len();
        let exit = self.exit_only();
        let known = self.ran.iter().all(|r| r.level.is_some());
        let how = match (ran, known, exit) {
            (0, _, _) | (_, false, _) => ".".to_owned(),
            (_, true, 0) => ", all per test.".to_owned(),
            (n, true, e) if e == n => ", all by exit code only.".to_owned(),
            (n, true, e) => format!(": {} per test, {e} by exit code only.", n - e),
        };
        out.push(format!(
            "Ran {ran} of {}{how}",
            count(total, "check", "checks")
        ));
        if !self.skipped.is_empty() {
            out.push(format!(
                "Skipped {} your change cannot affect.",
                count(self.skipped.len(), "check", "checks")
            ));
        }
        if self.uncovered_total > 0 {
            out.push(format!(
                "Not covered: {} no check runs.",
                count(
                    self.uncovered_total as usize,
                    "changed file",
                    "changed files"
                )
            ));
        }
        for w in self.weak.iter().take(MAX_WEAK_LINES) {
            let why = match w.kind {
                WeakKind::MissingModule => "only because a module is missing",
                WeakKind::MissingName => "only because a name its tests use is missing there",
                WeakKind::BuildError => "only because its tests do not build there",
                WeakKind::NewApi => {
                    "only because its tests call what your change adds (normal for a new API)"
                }
            };
            out.push(format!(
                "Weak proof: criterion {} fails without your change {why}.",
                w.criterion
            ));
        }
        if self.weak.len() > MAX_WEAK_LINES {
            out.push(format!(
                "Weak proof: {} more.",
                count(self.weak.len() - MAX_WEAK_LINES, "criterion", "criteria")
            ));
        }
        match &self.necessity {
            Some(Necessity::Checked { hunks, untested }) if untested.is_empty() => {
                out.push(format!(
                    "Necessity: reverting any one of {} made a proving test fail.",
                    count(*hunks as usize, "changed code part", "changed code parts")
                ));
            }
            Some(Necessity::Checked { hunks, untested }) => {
                out.push(format!(
                    "Necessity: no proving test needs {} of {}.",
                    untested.len(),
                    count(*hunks as usize, "changed code part", "changed code parts"),
                ));
            }
            Some(Necessity::NotChecked { hunks, why }) => {
                out.push(format!(
                    "Necessity: not checked for {}, because {}.",
                    count(*hunks as usize, "changed code part", "changed code parts"),
                    why.reason()
                ));
            }
            None => {}
        }
        out.push(format!("Where: {}.", self.where_text()));
        out
    }

    fn where_text(&self) -> String {
        let mut parts = vec![match self.at.mode {
            ProvenanceMode::Local => "natively on this machine",
            ProvenanceMode::Ci => "natively, by `aver ci`",
            ProvenanceMode::Sandbox => {
                "in a non-native executor (the scripted test executor; before Aver 0.4, the Docker sandbox)"
            }
        }
        .to_owned()];
        if let Some(uid) = self.at.uid {
            parts.push(
                match (uid, self.at.mode) {
                    (Uid::Separate, _) => "as a separate user",
                    (Uid::Same, ProvenanceMode::Local) => "as your user",
                    (Uid::Same, _) => "as the verifier's user",
                }
                .to_owned(),
            );
        }
        parts.push(
            match (self.at.network, self.at.mode) {
                (Network::On, _) => "network on",
                (Network::Off, ProvenanceMode::Sandbox) => "network off",
                (Network::Off, _) => "network off (best effort)",
            }
            .to_owned(),
        );
        let mut text = parts.join(", ");
        if let Some(r) = self.reuse.filter(|r| r.reused > 0) {
            text.push_str(&format!(
                "; {} of {} reused from the cache",
                r.reused,
                count(r.runs as usize, "run", "runs")
            ));
            if r.older_verifier {
                text.push_str(", some cached by an older verifier");
            }
        }
        for n in &self.narrowed {
            let side = match n.side {
                NarrowSide::Transplant => "the transplant",
                NarrowSide::HeadTests => "your tests' run",
            };
            text.push_str(&format!(
                "; {side} ran {} of {}, the rest read from {}",
                n.ran,
                count(n.of as usize, "test target", "test targets"),
                match n.side {
                    NarrowSide::Transplant => "the original code's run",
                    NarrowSide::HeadTests => "your code's run",
                }
            ));
        }
        text
    }

    /// Repository strings by kind, each with at most [`MAX_NAMES`] names
    /// and a count of the rest. Empty when the section is
    /// [`compact`](Self::compact).
    pub fn name_lists(&self) -> Vec<NameList> {
        if self.compact() {
            return Vec::new();
        }
        let ran: Vec<(String, bool)> = self
            .ran
            .iter()
            .map(|r| (r.check.clone(), r.level == Some(Level::Exit)))
            .collect();
        let plain = |v: &[String]| v.iter().map(|n| (n.clone(), false)).collect::<Vec<_>>();
        [
            NameList::of("ran", ran, self.ran.len() as u64),
            NameList::of("skipped", plain(&self.skipped), self.skipped.len() as u64),
            NameList::of("not covered", plain(&self.uncovered), self.uncovered_total),
            NameList::of(
                "not needed by a proving test",
                plain(self.untested()),
                self.untested().len() as u64,
            ),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// [`name_lists`](Self::name_lists) as lines, `label: a, b, c, +N more`,
    /// a check read by exit code alone marked `(exit code only)`. Renderers
    /// escape and delimit them.
    pub fn names(&self) -> Vec<String> {
        self.name_lists().iter().map(NameList::line).collect()
    }

    /// The material limits, for the verdict line's suffix: checks read by
    /// exit code alone, weak proofs (a new API's is normal, not a limit),
    /// network on, runs an older verifier cached. Uncovered files are not
    /// here: they already make the verdict `UNPROVEN`.
    pub fn limits(&self) -> Vec<String> {
        let mut out = Vec::new();
        let exit = self.exit_only();
        if exit > 0 {
            out.push(format!("{} by exit code", count(exit, "check", "checks")));
        }
        let weak = self
            .weak
            .iter()
            .filter(|w| w.kind != WeakKind::NewApi)
            .count();
        if weak > 0 {
            out.push(count(weak, "weak proof", "weak proofs"));
        }
        if self.at.network == Network::On {
            out.push("network on".to_owned());
        }
        if self.reuse.is_some_and(|r| r.older_verifier && r.reused > 0) {
            out.push("runs cached by an older verifier".to_owned());
        }
        out
    }

    /// `; limits: …` for the verdict line, or nothing.
    pub fn suffix(&self) -> String {
        let limits = self.limits();
        if limits.is_empty() {
            String::new()
        } else {
            format!("; limits: {}", limits.join(", "))
        }
    }

    /// What `POST /v0/verify` and `GET /v0/receipts/{id}` return as `scope`:
    /// the data, and the rendered lines, names and limits, so that every
    /// client prints what the CLI prints.
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).expect("a scope serializes");
        v["lines"] = serde_json::json!(self.lines());
        v["names"] = serde_json::json!(self.names());
        v["limits"] = serde_json::json!(self.limits());
        v["compact"] = serde_json::json!(self.compact());
        v
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// One kind of repository name the section lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameList {
    pub label: &'static str,
    /// The names shown, each with whether it is a check read by exit code
    /// alone.
    pub names: Vec<(String, bool)>,
    /// How many more there are.
    pub more: u64,
}

impl NameList {
    fn of(label: &'static str, mut names: Vec<(String, bool)>, total: u64) -> Option<Self> {
        if total == 0 {
            return None;
        }
        names.truncate(MAX_NAMES);
        Some(Self {
            label,
            more: total - names.len() as u64,
            names,
        })
    }

    pub fn line(&self) -> String {
        let mut parts: Vec<String> = self
            .names
            .iter()
            .map(|(n, exit)| {
                if *exit {
                    format!("{n} (exit code only)")
                } else {
                    n.clone()
                }
            })
            .collect();
        if self.more > 0 {
            parts.push(format!("+{} more", self.more));
        }
        format!("{}: {}", self.label, parts.join(", "))
    }
}
