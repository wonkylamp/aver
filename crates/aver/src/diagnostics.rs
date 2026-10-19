//! Diagnostics (ARCHITECTURE.md §1.9.4): what a reader needs to act on a
//! receipt's failures and unproven criteria without opening logs. Derived
//! data, not evidence: never signed, stored beside the receipt in the
//! `diagnostics` table and written by `aver ci` as `diagnostics.json`.
//!
//! Every string here that came from the repository or a sandbox (messages,
//! file names, output, test names) is untrusted data.

use aver_core::rules::UnprovenReason;
use aver_core::{Cause, CheckId, CriterionId, EntryId, Path, ReceiptId, RunId};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::store::{Store, StoreError, Tx};

pub const SCHEMA: &str = "aver.diagnostics/v0";
/// Bytes kept from the start and the end of a run's output.
pub const OUTPUT_HEAD_BYTES: usize = 2 * 1024;
pub const OUTPUT_TAIL_BYTES: usize = 6 * 1024;
/// Most test names suggested for an unproven criterion.
pub const MAX_CLOSEST: usize = 5;
/// Characters of a test name or criterion text that edit distance compares.
const MAX_COMPARED_CHARS: usize = 256;
/// Most items a [`Listing`] names; the receipt's `test_changes` lists 20.
pub const MAX_LISTING: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostics {
    pub schema: String,
    pub receipt: ReceiptId,
    pub failures: Vec<Failure>,
    pub unproven: Vec<Unproven>,
    /// The receipt's `test_changes` with longer lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_changes: Option<TestChangeLists>,
    /// Written by `aver ci` only: the wall time of the call and of every
    /// sandbox run, for a consumer that tracks compute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timings: Option<Timings>,
    /// Written by `aver ci` only: what the imports of the two commits
    /// reported without refusing them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import: Option<ImportNotes>,
    /// What the change's tests show about it, beyond the verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Evidence>,
    /// Criteria whose every proof failed on the original code only because
    /// something it uses is missing there; they are unproven (§1.9.4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weak_proofs: Vec<WeakProof>,
    /// What held a verdict that would be `PASS` below it, whatever the
    /// runs found: the change's own contract, or no runtime test.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap: Option<aver_core::rules::Cap>,
    /// The source of the test behind each claimed criterion, so a person
    /// sees what the proof checks (repository content).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proof_sources: Vec<ProofSource>,
    /// The receipt's `uncovered` with up to [`MAX_LISTING`] paths: source
    /// files the head changed that no check could exercise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncovered: Option<Listing>,
    /// The uncovered Rust files whose change is behind Cargo features no
    /// check enables, each with the features and a command that builds
    /// them (§1.9.3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unbuilt: Vec<crate::cargo_features::Unbuilt>,
    /// For a receipt with `CHANGED` entries: the diff of each test or
    /// harness file behind them, what a person reviews (round 13a; the same
    /// diffs `summary.md` shows).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review: Vec<ReviewDiff>,
    /// Why the change rewrites what it does, as the agent (`aver verify
    /// --reason`) or the head commit's `Aver-Why:` trailer states it, at
    /// most [`MAX_REASON_BYTES`]: unchecked agent or repository data, shown
    /// fenced beside the `CHANGED` entries and never read by the verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The transplant and head-tests runs Aver tried to narrow to the test
    /// targets their tree changes: what each ran, or why it ran in full.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub narrowed: Vec<Narrowed>,
    /// For a verdict that would be `PASS` with changed source hunks: what
    /// reverting each hunk did to the proving tests, or why that was not
    /// tried (§1.9.4, hunk necessity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub necessity: Option<Necessity>,
}

/// Hunk necessity (ARCHITECTURE.md §1.9.4): each source hunk of a change
/// reverted alone, the proving tests run on what is left.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Necessity {
    Checked { hunks: Vec<HunkTried> },
    NotChecked { why: NotChecked, hunks: u32 },
}

impl Diagnostics {
    /// The hunks of the change no proving test needs, which hold a `PASS`
    /// at `UNPROVEN`.
    pub fn untested_hunks(&self) -> Vec<&HunkTried> {
        self.necessity
            .iter()
            .flat_map(Necessity::untested)
            .collect()
    }
}

impl HunkTried {
    /// `<path>:<start>-<end>` (repository-derived).
    pub fn at(&self) -> String {
        format!("{}:{}-{}", self.path, self.start, self.end)
    }
}

impl Necessity {
    /// The hunks no proving test needs.
    pub fn untested(&self) -> impl Iterator<Item = &HunkTried> {
        let hunks = match self {
            Self::Checked { hunks } => hunks.as_slice(),
            Self::NotChecked { .. } => &[],
        };
        hunks.iter().filter(|h| h.reason.is_some())
    }
}

/// Why hunk necessity was not tried on a verdict that would be `PASS`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotChecked {
    /// No test proves a criterion and none of the change's own tests
    /// shows it.
    NoProvingTests,
    /// The check holding a proving test cannot run a test by name (its
    /// command is not one Aver can narrow), or the proof is not a named
    /// test.
    NoSelection { check: CheckId },
    /// More hunks than [`MAX_HUNKS`].
    TooManyHunks,
}

/// The most hunks the phase reverts, one run each.
pub const MAX_HUNKS: usize = 8;

/// What reverting one hunk did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ablated {
    /// A proving test failed or went missing, or the run failed or was
    /// lost: the hunk is needed.
    Killed,
    /// Every proving test ran and passed.
    Survived,
    /// No proving test ran: the code did not build or load without it.
    Uncompilable,
}

/// One hunk of a changed source file (repository-derived path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HunkTried {
    pub path: String,
    /// 1-based, inclusive head lines.
    pub start: u32,
    pub end: u32,
    pub outcome: Ablated,
    /// For an `uncompilable` hunk: what reverting it with the hunks of its
    /// file that share its new identifiers did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coupled: Option<Ablated>,
    /// `untested_hunk` when no proving test needs it, which holds the
    /// verdict at `UNPROVEN`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<UnprovenReason>,
}

/// One side of one check whose command Aver tried to narrow (ARCHITECTURE.md
/// §2.5.1, narrowed runs). The rest of a narrowed side's results are the
/// test targets' results on the side it differs from by those files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Narrowed {
    pub check: CheckId,
    pub side: aver_core::scope::NarrowSide,
    /// The test targets the narrowed run ran, as libtest labels them
    /// (repository content); empty when the side ran in full.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ran: Vec<String>,
    /// How many test targets the full command runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<u32>,
    /// Why the side ran its full command instead, in Aver's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full: Option<String>,
}

/// The longest stated reason the diagnostics keep; a longer one is cut at
/// a character boundary.
pub const MAX_REASON_BYTES: usize = 2000;

impl Diagnostics {
    /// The reason stated for the change, if any: data, not instructions.
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

/// One changed test or harness file and its unified diff, cut; repository
/// content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewDiff {
    pub path: String,
    pub diff: String,
}

/// Evidence that a change does what it is for (§1.9.4), whether or not the
/// task has criteria: the head's own tests that fail on the original code
/// and pass with the change, and the tests that failed on the base and now
/// pass or still fail. Derived, never signed: tests that prove no criterion
/// come from transplant runs the receipt does not record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    /// Tests in test files the head added or changed that fail (or do not
    /// load) on the transplant and pass in the head-tests run, in id order,
    /// at most [`MAX_LISTING`].
    pub shows_change: Vec<Shown>,
    pub shows_change_omitted: u64,
    /// Tests that failed on every base run and pass on the head.
    pub fixed: Listing,
    /// Tests that fail on the base and still fail on the head
    /// (`PREEXISTING`): for a task about failing tests, the target.
    pub still_failing: Listing,
}

impl Evidence {
    pub fn is_empty(&self) -> bool {
        self.shows_change.is_empty()
            && self.fixed.items.is_empty()
            && self.still_failing.items.is_empty()
    }
}

/// A test of the head's that fails on the original code and passes with
/// the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shown {
    pub id: EntryId,
    /// How it fared on the transplant: `fail`, or `absent` when no run
    /// reported it (its file did not load, or it did not run).
    pub original: aver_core::Transplant,
    /// Its failure on the transplant only says that a module or an export
    /// it imports is missing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub missing_module: bool,
    /// Its failure on the transplant only says that a name it uses is not
    /// defined there ([`missing_name`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub missing_name: bool,
    /// The name the change adds that its failure on the transplant says is
    /// missing ([`crate::idents::missing`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Shown {
    /// Whether the original code failed it before any assertion about
    /// behaviour ran.
    pub fn weak(&self) -> bool {
        self.original == aver_core::Transplant::Absent || self.missing_module || self.missing_name
    }
}

/// The test behind a criterion and the first lines of its source in the
/// head, fenced wherever it is shown: repository content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProofSource {
    pub criterion: CriterionId,
    pub entry: EntryId,
    pub path: String,
    /// 1-based line of the test's name.
    pub line: u32,
    /// At most [`MAX_PROOF_LINES`] lines from that line.
    pub source: String,
}

/// Most lines of a proving test's source the reports show.
pub const MAX_PROOF_LINES: usize = 30;

/// The test named `name` in `text`: from the first line holding the name
/// (or its last ` > `-separated part, as JUnit reporters join suites) to
/// the end of its body, at most [`MAX_PROOF_LINES`] lines. A body ends where
/// its brackets close, or, after a line ending in `:` (Python), at the
/// next line indented no deeper than the first.
pub fn test_source(text: &str, name: &str) -> Option<(u32, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let short = name.rsplit(" > ").next().unwrap_or(name);
    let start = [name, short]
        .iter()
        .filter(|n| !n.trim().is_empty())
        .find_map(|n| lines.iter().position(|l| l.contains(n)))?;
    let first = lines[start];
    let indent = |l: &str| l.len() - l.trim_start().len();
    let mut out = vec![first];
    if first.trim_end().ends_with(':') {
        for line in &lines[start + 1..] {
            if out.len() >= MAX_PROOF_LINES
                || (!line.trim().is_empty() && indent(line) <= indent(first))
            {
                break;
            }
            out.push(line);
        }
    } else {
        let mut depth: i64 = 0;
        let mut opened = false;
        for (i, line) in lines[start..].iter().enumerate() {
            if i > 0 {
                if out.len() >= MAX_PROOF_LINES {
                    break;
                }
                out.push(line);
            }
            for c in line.chars() {
                match c {
                    '{' | '(' | '[' => {
                        depth += 1;
                        opened = true;
                    }
                    '}' | ')' | ']' => depth -= 1,
                    _ => {}
                }
            }
            if opened && depth <= 0 {
                break;
            }
        }
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    Some((start as u32 + 1, out.join("\n")))
}

/// A criterion whose proving tests all failed on the original code only
/// because something they use is missing there: a module or export, a
/// name, or a build. Such a failure shows that the name is new, not that
/// the test checks what the criterion asks, so the criterion is unproven
/// (§1.9.4): weak proofs alone never prove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeakProof {
    pub criterion: CriterionId,
    /// The entries proving it.
    pub entries: Vec<EntryId>,
    /// [`WEAK_MISSING_MODULE`], [`WEAK_MISSING_NAME`] or [`WEAK_BUILD_ERROR`].
    pub reason: String,
    /// For [`WEAK_BUILD_ERROR`]: every build error is a name the original
    /// code does not define and the head does (a test of a new API), so the
    /// head-tests run is what checked the assertions.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new_api: bool,
    /// For [`WEAK_MISSING_NAME`]: the name the change adds that a proof's
    /// failure on the original code says is missing, when it says which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl WeakProof {
    /// What the reports say about this weak proof.
    pub fn advice(&self) -> &'static str {
        match (self.reason.as_str(), self.new_api) {
            (WEAK_BUILD_ERROR, true) => WEAK_NEW_API_NOTE,
            (WEAK_BUILD_ERROR, false) => WEAK_BUILD_ADVICE,
            (WEAK_MISSING_NAME, _) => WEAK_NAME_ADVICE,
            _ => WEAK_PROOF_ADVICE,
        }
    }
}

/// What the reports say about a proof whose tests do not build on the
/// original code only because they call names the change adds.
pub const WEAK_NEW_API_NOTE: &str = "its tests do not build on the original code only because they call what the change adds, which a test that merely names the new API does too. Add a test that builds on the original code and fails there on an assertion about what the criterion asks, or have a person review it";

/// What the reports say about a proof resting on any other build failure.
pub const WEAK_BUILD_ADVICE: &str = "its tests only fail to build on the original code. Add a test that builds on the original code and fails there on an assertion about what the criterion asks";

/// The note on an entry that cannot prove because the original code does
/// not build for its check (§1.9.3): the transplant then fails to build
/// whatever the change does. Every report says it in these words, and
/// never next to "would prove"; the first error follows as repository data.
pub fn original_build_note(check: &str) -> String {
    format!("The original code doesn't build for check {check}, so no test can show that the change is needed; the verdict is BLOCKED_NOT_OWN")
}

/// Whether a failure note is [`original_build_note`]'s.
pub fn is_original_build_note(note: &str) -> bool {
    note.starts_with("The original code doesn't build for check ")
}

/// [`WeakProof::reason`] when the proofs only import what the change adds.
pub const WEAK_MISSING_MODULE: &str = "missing_module";

/// [`WeakProof::reason`] when the proofs fail on the original code only
/// because a name they use is not defined there.
pub const WEAK_MISSING_NAME: &str = "missing_name";

/// What the reports say about a proof resting on a missing name.
pub const WEAK_NAME_ADVICE: &str = "its tests fail on the original code only because a name they use is missing there (\"is not a function\", \"is not defined\", AttributeError and the like), which a test that only checks that the name exists does too. Add a test that fails on the original code on an assertion about what the criterion asks, such as a result the original code gets wrong";

/// [`WeakProof::reason`] when every proof rests on the transplant failing
/// to build.
pub const WEAK_BUILD_ERROR: &str = "build_error";

/// What the reports say about a weak proof, and what to do instead.
pub const WEAK_PROOF_ADVICE: &str = "its tests fail on the original code only because a module they import is missing there. Add a test that loads on the original code and fails there on an assertion about what the criterion asks";

/// Whether a failure message on the original code says only that a module
/// or an export the test imports is missing, so the test never reached an
/// assertion.
pub fn missing_module(message: &str) -> bool {
    const SIGNS: &[&str] = &[
        "ERR_MODULE_NOT_FOUND",
        "Cannot find module",
        "Cannot find package",
        "does not provide an export named",
        "Failed to load url",
        "Failed to resolve import",
    ];
    SIGNS.iter().any(|s| message.contains(s))
}

/// Whether a failure message on the original code says only that a name
/// the test uses is not defined there (a function, an attribute, an
/// export), or is an assertion that something is a function and it was
/// `undefined`: the test never checked behaviour (M1 of the review).
pub fn missing_name(message: &str) -> bool {
    const SIGNS: &[&str] = &[
        // JavaScript and TypeScript.
        "is not a function",
        "is not defined",
        "is not a constructor",
        "Cannot read properties of undefined",
        // Python.
        "NameError",
        "ImportError: cannot import name",
        "cannot import name ",
        // Go, as a vet or build line in a test's output.
        "undefined: ",
    ];
    if SIGNS.iter().any(|s| message.contains(s)) {
        return true;
    }
    // An attribute missing on `None`, or a property read of `null`, is the
    // original code's wrong result, not a missing name.
    if (message.contains("AttributeError") && !message.contains("'NoneType' object"))
        || (message.contains("Cannot read property") && message.contains("of undefined"))
    {
        return true;
    }
    let quoted = |word: &str| {
        message.contains(&format!("'{word}'")) || message.contains(&format!("\"{word}\""))
    };
    // `typeof m.negate === 'function'` and the like: the only thing checked
    // is that the name exists.
    (quoted("undefined") && quoted("function"))
        || (message.contains("typeof") && message.contains("undefined"))
        || (message.contains("toBeDefined") && message.contains("undefined"))
}

/// What every report says about uncovered changes (§1.9.3), followed by the
/// paths.
pub const UNCOVERED_NOTE: &str = "changed files not covered by any check";

/// What `aver ci`'s imports let through with a warning (§2.9): possible
/// secrets (path and pattern, never the matched bytes) and files over the
/// blob limit imported empty. Each lists at most [`MAX_LISTING`] items in
/// path order, with the rest counted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportNotes {
    pub secrets: Vec<SecretNote>,
    pub secrets_omitted: u64,
    pub placeholders: Vec<PlaceholderNote>,
    pub placeholders_omitted: u64,
}

impl ImportNotes {
    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty() && self.placeholders.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretNote {
    pub path: String,
    pub pattern: String,
    /// `low` for templates, test, fixture and example paths and Markdown
    /// documentation; `high` otherwise.
    pub severity: String,
    /// The commits (`base`, `head`) whose import found it.
    pub sides: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaceholderNote {
    pub path: String,
    /// The size of the file in Git.
    pub bytes: u64,
    pub sides: Vec<String>,
}

/// [`aver_core::TestChanges`] with up to [`MAX_LISTING`] items per list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestChangeLists {
    pub files_added: Listing,
    pub files_modified: Listing,
    pub files_deleted: Listing,
    pub tests_removed: Listing,
    pub tests_skipped: Listing,
    pub harness_changed: Listing,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listing {
    pub items: Vec<String>,
    pub omitted: u64,
}

impl Listing {
    /// The first [`MAX_LISTING`] of `items` (already in order), the rest
    /// counted.
    pub fn of(mut items: Vec<String>) -> Self {
        let total = items.len();
        items.truncate(MAX_LISTING);
        Self {
            omitted: (total - items.len()) as u64,
            items,
        }
    }
}

/// Where the time of an `aver ci` call went. Sandbox times are wall clock
/// from start to removal; a cache hit, or a run counted on an earlier side,
/// is 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timings {
    /// The whole call, from start to the outputs being written.
    pub total_ms: u64,
    /// Sums over `runs`.
    pub install_ms: u64,
    pub sandbox_ms: u64,
    pub runs: Vec<RunTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunTime {
    /// `base`, `head`, `head_tests` or `transplant`.
    pub side: crate::verify::Side,
    pub check: CheckId,
    pub index: u8,
    pub run: RunId,
    pub cache_hit: bool,
    /// Dependency install (and layer probe) time this run started.
    pub install_ms: u64,
    /// The check sandbox.
    pub sandbox_ms: u64,
}

impl Timings {
    pub fn of(total: std::time::Duration, runs: &[crate::verify::TimedRun]) -> Self {
        let ms = |d: std::time::Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        let runs: Vec<RunTime> = runs
            .iter()
            .map(|r| RunTime {
                side: r.side,
                check: r.check.clone(),
                index: r.index,
                run: r.run,
                cache_hit: r.cache_hit,
                install_ms: ms(r.install),
                sandbox_ms: ms(r.sandbox),
            })
            .collect();
        Self {
            total_ms: ms(total),
            install_ms: runs.iter().map(|r| r.install_ms).sum(),
            sandbox_ms: runs.iter().map(|r| r.sandbox_ms).sum(),
            runs,
        }
    }
}

/// A receipt entry with a cause or a head that is not `pass`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub id: EntryId,
    pub cause: Option<Cause>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Aver's words, not repository content: why the entry holds the
    /// verdict at `BLOCKED_NOT_OWN` although its cause does not
    /// ([`aver_core::rules::inconclusive`]), why it is `CHANGED`, or why a
    /// failure is `ENV` because a dependency could not be built.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// For a `NEW` or `CHANGED` test or check: how the head's own version of
    /// it fared in the head-tests run (`pass`, `fail`, `absent` when the
    /// head deleted it, `infra`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own: Option<aver_core::Transplant>,
    /// For a `CHANGED` test: the existing test file the head changed that
    /// defines it, when the test's name says which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<Path>,
}

/// The note on a command-level entry that fails on both sides.
pub const COMMAND_LEVEL_NOTE: &str = "inconclusive: this check fails on the base too and has no per-test results, so it cannot show whether the change broke it; the verdict is BLOCKED_NOT_OWN";
/// The note on a `CHANGED` test whose own version the head changed and passes.
pub const CHANGED_NOTE: &str =
    "needs review: the old version of this test fails on the new code; the PR's version passes";
/// The note on a `CHANGED` test that already failed on the base: the
/// head's rewrite, not its code, is what makes it pass.
pub const CHANGED_FAILING_NOTE: &str = "needs review: this test already failed on the base and its old version still fails on the new code; only the PR's rewritten version passes, so it was rewritten, not fixed";
/// The note on a `CHANGED` test the head deleted.
pub const CHANGED_DELETED_NOTE: &str =
    "needs review: the old version of this test fails on the new code; the PR deleted it";
/// The note on a `CHANGED` test the head skipped or deleted although its
/// old version passes on the head: removing a test weakens what is checked.
pub const CHANGED_REMOVED_NOTE: &str =
    "needs review: the old version of this test passes on the new code, and the PR skipped or deleted it";
/// The note on a `NEW` test whose file the head changed to skip or delete
/// it: the base's copy, which passed on the base, fails on the change.
pub const HIDDEN_REGRESSION_NOTE: &str = "the PR skipped or deleted this test, and its base version, which passes on the base, fails on the new code: restore the test and fix the code";
/// The note on an `ENV` entry whose base runs ended before their first test
/// result without a toolchain error Aver recognizes.
pub const NO_TESTS_NOTE: &str = "the original code's runs ended before their first test result (the test runner failed to start, or wrote no report), so no test compares the change; the verdict is BLOCKED_NOT_OWN";
/// The note on an `ENV` entry whose base runs a toolchain refused to start,
/// with the fix ([`crate::exec::toolchain`]'s, Aver's words).
pub fn toolchain_note(fix: &str) -> String {
    format!("the toolchain could not run the tests on the original code, so no test compares the change; the verdict is BLOCKED_NOT_OWN (fix: {fix})")
}

/// Whether a failure note says the base ran no test ([`NO_TESTS_NOTE`],
/// [`toolchain_note`]).
pub fn is_no_tests_note(note: &str) -> bool {
    note == NO_TESTS_NOTE || note.starts_with("the toolchain could not run the tests")
}
/// The note on a `lease::` entry.
pub const LEASED_NOTE: &str = "needs review: the change edits a path another task holds an exclusive lease on; a person decides whether both changes may land, or the edit is undone";
/// The note on a `sensitive::` entry.
pub const SENSITIVE_NOTE: &str = "needs review: the change edits a path the base's aver.toml marks sensitive; nothing failed, but a person reviews it before it lands";
/// The note on a `built::` entry.
pub const BUILT_NOTE: &str = "needs review: the change's build wrote this compiled test file with other content than the base's build, from the same test sources, so the tests that ran from it are not the base's";
/// The note on a `judging::` entry.
pub const CHANGED_HARNESS_NOTE: &str =
    "needs review: the PR changes the test harness; every check ran with the base's version";

/// The note on a test the base never ran that failed in a test target the
/// head's `Cargo.toml` declares and the base's frozen copy lacks.
pub const RETARGETED_NOTE: &str = "needs review: this test's target is declared in a [[test]] table of the head's Cargo.toml that the base's copy lacks (its required-features, harness or path), and verification builds test targets from the base's copy, so the test ran without what its target declares";

/// What the agent report says when a [`RETARGETED_NOTE`] entry is there.
pub const MERGED_HINT: &str = "If you merged a newer commit (such as main) into this directory, its Cargo.toml runs as the original's here: run `aver workspace rebase` (it takes the repository's HEAD; name the commit to pick another), which moves the workspace onto it so the merged changes stop counting as yours.";

/// The note on the entry of a restored test file that does not compile on
/// the judged head.
pub const UNBUILT_TEST_NOTE: &str = "needs review: the PR changed or deleted this test file, and its old version does not compile against the new code (it uses something the change removed or changed), so its tests did not run; the other tests were judged one by one";

/// The note on the entry of a deleted Rust source file whose base version
/// held tests.
pub const DELETED_SOURCE_NOTE: &str = "needs review: the PR deleted this source file, and the tests inside it went with the code they tested, so they did not run; the other tests were judged one by one";

/// The note on a test target that built on the judged head but that Aver's
/// run over the built targets left out.
pub const NOT_RERUN_NOTE: &str = "Aver built this test target but did not run its tests again, so they are not judged: a fault in Aver, not in the change; verify again, and report it if it repeats";

/// The note on a check whose head runs lost the per-test report every base
/// run produced.
pub const LOST_REPORT_NOTE: &str = "the base's runs reported every test and the head's did not (a build or a test module the change breaks, or a malformed report), so the change is why, whatever already failed on the base";

/// The note on a check whose head lost its report and whose build probe
/// Aver lost too.
pub const LOST_PROBE_NOTE: &str = "the head's runs lost the report, and Aver lost the result of the build that would show whether restored test files or the change stopped it: a fault in Aver, not in the change; verify again, and report it if it repeats";

/// The note on a test the base passed on every run and the head failed on
/// some runs.
pub const FLAKY_HEAD_NOTE: &str = "the base passed this test on every run and the head failed it on some, so the change makes it fail some of the time";

/// The note on a test that fails on both sides because it reaches for the
/// external network, which native test runs do not have (loopback they
/// do): it says nothing about the change either way.
pub const NETWORK_NOTE: &str = "needs the network, which test runs do not have (loopback only), so it fails on both sides and says nothing about the change";

/// Whether a failure message says the test could not reach the network or
/// resolve a host name: Linux's `ENETUNREACH` as Rust, Go, Python and Node
/// print it, and failed DNS lookups.
pub fn needs_network(message: &str) -> bool {
    [
        "Network is unreachable",
        "network is unreachable",
        "ENETUNREACH",
        "EAI_AGAIN",
        "Temporary failure in name resolution",
        "failed to lookup address information",
        "getaddrinfo ENOTFOUND",
        ": no such host",
    ]
    .iter()
    .any(|p| message.contains(p))
}

/// The note on a test module that failed to collect on the base (pytest's
/// `collection failure`).
pub const UNCOLLECTED_NOTE: &str = "inconclusive: this test module failed to import on the base (a collection error), so none of its tests ran there and nothing shows whether the change broke them; fix the import (an installed dependency or a generated file the checkout lacks) and verify again; the verdict is BLOCKED_NOT_OWN";

/// The note on an entry that was flaky on the base and failed on every head run.
pub const FLAKY_BASE_NOTE: &str = "inconclusive: flaky on the base, but it failed on every head run, so the head never passed it; the verdict is BLOCKED_NOT_OWN";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub file: Path,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unproven {
    pub criterion: CriterionId,
    pub reason: UnprovenReason,
    /// The entries that prove (claim) the criterion.
    pub entries: Vec<EntryId>,
    /// Head test names nearest the criterion's text, for `not_claimed` and
    /// `claimed_test_not_found`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closest: Vec<String>,
}

/// The first [`OUTPUT_HEAD_BYTES`] and last [`OUTPUT_TAIL_BYTES`] of
/// `output`, cut on character boundaries, with a line saying how much was
/// left out between them.
pub fn excerpt(output: &str) -> String {
    if output.len() <= OUTPUT_HEAD_BYTES + OUTPUT_TAIL_BYTES {
        return output.to_owned();
    }
    let mut head_end = OUTPUT_HEAD_BYTES;
    while !output.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = output.len() - OUTPUT_TAIL_BYTES;
    while !output.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let mut out = String::with_capacity(OUTPUT_HEAD_BYTES + OUTPUT_TAIL_BYTES + 64);
    out.push_str(&output[..head_end]);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("[aver] {} bytes omitted\n", tail_start - head_end));
    out.push_str(&output[tail_start..]);
    out
}

/// A test's location: its JUnit `file` and `line` when both are usable,
/// else the first stack frame in `message` under the run's slot (`<src>/`,
/// or `/work/` in an older run's output) outside `node_modules`. Paths are
/// made repo-relative and must be valid paths.
pub fn location(file: Option<&str>, line: Option<u32>, message: Option<&str>) -> Option<Location> {
    const ROOTS: [&str; 2] = ["<src>/", "/work/"];
    let relative = |f: &str| -> Option<Path> {
        ROOTS
            .iter()
            .find_map(|r| f.strip_prefix(r))
            .unwrap_or(f)
            .parse()
            .ok()
    };
    if let (Some(file), Some(line)) = (file.and_then(relative), line) {
        return Some(Location { file, line });
    }
    let message = message?;
    let mut starts: Vec<usize> = ROOTS
        .iter()
        .flat_map(|r| message.match_indices(r).map(|(at, r)| at + r.len()))
        .collect();
    starts.sort_unstable();
    starts.into_iter().find_map(|at| {
        let rest = &message[at..];
        let end = rest
            .find(|c: char| c == ':' || c == ')' || c.is_whitespace())
            .unwrap_or(rest.len());
        let (path, after) = rest.split_at(end);
        if path.split('/').any(|s| s == "node_modules") {
            return None;
        }
        let line: u32 = after
            .strip_prefix(':')?
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()?;
        Some(Location {
            file: path.parse().ok()?,
            line,
        })
    })
}

/// Up to [`MAX_CLOSEST`] of `names` nearest `text` by edit distance (ties in
/// name order), comparing at most the first 256 characters of each.
pub fn closest<'a>(text: &str, names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let target: Vec<char> = text
        .to_lowercase()
        .chars()
        .take(MAX_COMPARED_CHARS)
        .collect();
    let mut scored: Vec<(usize, &str)> = names
        .into_iter()
        .map(|name| {
            let candidate: Vec<char> = name
                .to_lowercase()
                .chars()
                .take(MAX_COMPARED_CHARS)
                .collect();
            (edit_distance(&target, &candidate), name)
        })
        .collect();
    scored.sort();
    scored.dedup_by(|a, b| a.1 == b.1);
    scored
        .into_iter()
        .take(MAX_CLOSEST)
        .map(|(_, name)| name.to_owned())
        .collect()
}

fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(ca != cb));
            diagonal = above;
        }
    }
    row[b.len()]
}

/// Stores `diagnostics` beside its receipt, inside the receipt's transaction.
pub fn store(tx: &Tx<'_>, diagnostics: &Diagnostics) -> Result<(), StoreError> {
    let body = serde_json::to_string(diagnostics).expect("diagnostics serialize");
    tx.sql.execute(
        "INSERT INTO diagnostics (receipt, body) VALUES (?1, ?2)
         ON CONFLICT (receipt) DO UPDATE SET body = excluded.body",
        params![diagnostics.receipt.to_string(), body],
    )?;
    Ok(())
}

/// The diagnostics stored for `receipt`, if any.
pub fn load(store: &Store, receipt: &ReceiptId) -> Result<Option<Diagnostics>, StoreError> {
    let body: Option<String> = store.read(|conn| {
        Ok(conn
            .query_row(
                "SELECT body FROM diagnostics WHERE receipt = ?1",
                [receipt.to_string()],
                |r| r.get(0),
            )
            .optional()?)
    })?;
    body.map(|b| {
        serde_json::from_str(&b).map_err(|e| StoreError::Corrupt {
            id: receipt.to_string(),
            reason: format!("diagnostics: {e}"),
        })
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpts_keep_the_head_and_tail() {
        let short = "a".repeat(OUTPUT_HEAD_BYTES + OUTPUT_TAIL_BYTES);
        assert_eq!(excerpt(&short), short);
        let long = format!(
            "{}{}{}",
            "h".repeat(3000),
            "m".repeat(5000),
            "t".repeat(7000)
        );
        let cut = excerpt(&long);
        assert!(cut.starts_with(&"h".repeat(2048)));
        assert!(cut.ends_with(&"t".repeat(6144)));
        assert!(cut.contains(&format!(
            "[aver] {} bytes omitted\n",
            long.len() - 2048 - 6144
        )));
        let wide = "é".repeat(10_000);
        assert!(excerpt(&wide).len() <= 2048 + 6144 + 40);
    }

    #[test]
    fn locations_come_from_attributes_else_the_first_author_frame() {
        assert_eq!(
            location(Some("/work/test/a.test.js"), Some(7), None),
            Some(Location {
                file: "test/a.test.js".parse().unwrap(),
                line: 7
            })
        );
        let stack = "AssertionError: 1 == 2\n    at f (/work/node_modules/x/index.js:1:1)\n    at TestContext.<anonymous> (/work/test/divide.test.js:11:10)";
        assert_eq!(
            location(Some("/work/test"), None, Some(stack)),
            Some(Location {
                file: "test/divide.test.js".parse().unwrap(),
                line: 11
            })
        );
        let slot = "AssertionError: 1 == 2\n    at TestContext.<anonymous> (<src>/test/divide.test.js:12:3)";
        assert_eq!(
            location(Some("<src>/test"), None, Some(slot)),
            Some(Location {
                file: "test/divide.test.js".parse().unwrap(),
                line: 12
            })
        );
        assert_eq!(
            location(Some("<src>/test/a.test.js"), Some(3), None).map(|l| l.file),
            Some("test/a.test.js".parse().unwrap())
        );
        assert_eq!(location(None, None, Some("no frames")), None);
        assert_eq!(
            location(None, None, Some("at (/work/../etc/passwd:1:1)")),
            None
        );
    }

    #[test]
    fn missing_modules_are_told_from_assertions() {
        assert!(missing_module(
            "Error [ERR_MODULE_NOT_FOUND]: Cannot find module '/work/src/engine.ts'"
        ));
        assert!(missing_module(
            "SyntaxError: The requested module '../src/a.js' does not provide an export named 'negate'"
        ));
        assert!(!missing_module(
            "AssertionError [ERR_ASSERTION]: Expected values to be strictly equal:\n\n1 !== -1"
        ));
    }

    #[test]
    fn closest_names_rank_by_edit_distance() {
        let names = [
            "unit::divide throws on zero",
            "unit::adds",
            "unit::divide returns the quotient",
            "unit::subtracts",
        ];
        let got = closest(
            "divide throws on division by zero",
            names.iter().map(|n| &n[6..]),
        );
        assert_eq!(got[0], "divide throws on zero");
        assert_eq!(got.len(), 4);
        assert_eq!(edit_distance(&['a', 'b'], &['a', 'c', 'b']), 1);
    }
}
