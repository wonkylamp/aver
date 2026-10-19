//! The Markdown summary of a CI run: what a reviewer reads in the job
//! summary and the pull request comment.
//!
//! Test names and failure details are repository content (CLAUDE.md rule 7),
//! so they appear only inside code spans and fenced blocks, where Markdown,
//! HTML and @-mentions are inert. Criterion text comes from the pull request
//! description and is escaped.

use std::fmt::Write;

use aver_core::rules::{self, UnprovenReason};
use aver_core::{
    Cause, ConfigSource, HeadStatus, Listed, ReceiptBody, ReceiptCheck, SetItem, Transplant,
    Verdict,
};

use super::{FailOn, Outcome};
use crate::diagnostics::Failure;

/// Entries listed per section; the rest are counted.
const MAX_ENTRIES: usize = 25;
/// Paths or test names listed per line of the test changes section.
const MAX_NAMED: usize = 10;
/// Bytes of each failure detail shown. Only NEW and ENV entries show one:
/// a PREEXISTING or FLAKY failure is not the reviewer's to fix here.
const MAX_DETAIL: usize = 600;

pub struct Context<'a> {
    pub outcome: &'a Outcome,
    /// The verifier's public key, unpadded base64url.
    pub public_key: &'a str,
    /// The executor profile the checks ran under, e.g. `docker-runsc-v0`.
    pub executor: &'a str,
    /// Whether the verifier key was generated for this run (no `--home`).
    pub throwaway_key: bool,
    pub fail_on: FailOn,
    /// The receipt viewer page, e.g. `https://example.github.io/aver/receipt.html`.
    pub viewer_url: Option<&'a str>,
    /// The `aver ci` command that repeats this run.
    pub reproduce: &'a str,
}

pub fn render(ctx: &Context<'_>) -> String {
    let receipt = &ctx.outcome.receipt.body;
    let checks = receipt.checks.as_slice();
    let with = |cause: Cause| -> Vec<&ReceiptCheck> {
        checks.iter().filter(|c| c.cause == Some(cause)).collect()
    };
    let (new, preexisting, flaky, env, changed) = (
        with(Cause::New),
        with(Cause::Preexisting),
        with(Cause::Flaky),
        with(Cause::Env),
        with(Cause::Changed),
    );
    let failures = ctx.outcome.diagnostics.failures.as_slice();
    let passing = checks
        .iter()
        .filter(|c| c.cause.is_none() && c.head == HeadStatus::Pass)
        .count();
    let regression_only = receipt.criteria.is_empty();
    let rule = receipt.schema.proof_rule();
    let inconclusive: Vec<&ReceiptCheck> = checks
        .iter()
        .filter(|c| rules::inconclusive(c, rule))
        .collect();

    let mut out = String::new();
    let _ = writeln!(out, "## Aver: {}\n", receipt.verdict.as_str());
    let uncovered_total = receipt.uncovered.as_ref().map_or(0, |u| u.total());
    let untested = ctx.outcome.diagnostics.untested_hunks();
    let headline = if receipt.verdict == Verdict::Unproven
        && receipt.unproven.is_empty()
        && !untested.is_empty()
    {
        format!(
            "Nothing broke where checks ran, but no proving test needs {}: with {} reverted alone, every test that proves the change still passes.",
            count(untested.len() as u64, "part of the change", "parts of the change"),
            if untested.len() == 1 { "it" } else { "any of them" }
        )
    } else if receipt.verdict == Verdict::Unproven && receipt.unproven.is_empty() {
        format!(
            "Nothing broke where checks ran, but no check covers {uncovered_total} changed source {}, so nothing verified {}.",
            if uncovered_total == 1 { "file" } else { "files" },
            if uncovered_total == 1 { "it" } else { "them" }
        )
    } else {
        headline(
            receipt.verdict,
            regression_only,
            new.len(),
            receipt.unproven.len(),
            !inconclusive.is_empty() && env.is_empty(),
            changed.len(),
        )
    };
    let _ = writeln!(out, "{headline}\n");
    if let Some(counts) = crate::run::report::counts_line(receipt) {
        let _ = writeln!(out, "{counts}\n");
    }
    for (id, tail) in crate::run::report::output_tails(receipt, Some(&ctx.outcome.diagnostics)) {
        let _ = writeln!(
            out,
            "The last {} lines of output of {}, which failed as a whole:\n",
            crate::run::report::MAX_TAIL_LINES,
            code(&id)
        );
        let _ = writeln!(out, "{}", code_block(&tail));
    }
    let _ = writeln!(
        out,
        "Head `{}` verified against base `{}`. {}\n",
        short(ctx.outcome.head.commit.as_str()),
        short(ctx.outcome.base.commit.as_str()),
        exit_line(ctx.fail_on, receipt),
    );
    if let Some(requested) = receipt.ci.as_ref().and_then(|c| c.requested_base.as_ref()) {
        let _ = writeln!(
            out,
            "The requested base `{}` is not an ancestor of the head, so the run compared the head with their merge base `{}`, the commit it forked from. Comparing with the requested base would count the base branch's newer commits as part of this change, reversed.\n",
            short(requested.as_str()),
            short(ctx.outcome.base.commit.as_str()),
        );
    }
    if let Some(subject) = receipt.ci.as_ref().and_then(|c| c.subject.as_ref()) {
        let _ = writeln!(out, "Subject: {}\n", code(subject.as_str()));
    }
    if receipt.config == Some(ConfigSource::Default) {
        let generic = ctx.outcome.checks.iter().any(|c| {
            c.id.as_str() != crate::env::DEFAULT_CHECK_ID && !crate::presets::is_typecheck(c)
        });
        let _ = writeln!(out, "{}\n", default_config_line(generic));
        for check in &ctx.outcome.checks {
            let why = ctx.outcome.exit_only.get(&check.id).map(String::as_str);
            let _ = writeln!(out, "{}\n", default_check_line(check, why));
        }
    }
    let scope = crate::run::limits::scope(receipt, Some(&ctx.outcome.diagnostics), None);
    let _ = writeln!(
        out,
        "{}",
        crate::run::limits::markdown(&scope, receipt, Some(&ctx.outcome.diagnostics))
    );
    test_changes(&mut out, receipt);
    if let Some(uncovered) = &receipt.uncovered {
        let _ = writeln!(out, "### Not covered by any check\n");
        let _ = writeln!(
            out,
            "Warning: {} ({}). No check runs in their directory with their language, or none builds the code they change, so no test exercised them, and the verdict cannot be PASS. Add a check that covers them to `aver.toml` (`[[check]]` with a `cwd`, or `covers` patterns, or the Cargo features named below).\n",
            crate::diagnostics::UNCOVERED_NOTE,
            uncovered.total()
        );
        for path in uncovered.items.iter() {
            match ctx
                .outcome
                .diagnostics
                .unbuilt
                .iter()
                .find(|u| u.path == path.as_str())
            {
                Some(u) => {
                    let _ = writeln!(out, "- {} {}", code(path.as_str()), u.remedy(code));
                }
                None => {
                    let _ = writeln!(out, "- {}", code(path.as_str()));
                }
            }
        }
        if uncovered.omitted > 0 {
            let _ = writeln!(out, "- and {} more", uncovered.omitted);
        }
        let paths: Vec<&str> = uncovered.items.iter().map(|p| p.as_str()).collect();
        let glob = crate::coverage::covers_glob(&paths);
        let _ = writeln!(
            out,
            "\nTo cover them, add {} to the check in `aver.toml` that tests them.",
            code(&format!("covers = [\"{glob}\"]"))
        );
        out.push('\n');
    }

    if !untested.is_empty() {
        let _ = writeln!(out, "### Code no proving test needs\n");
        for h in &untested {
            let _ = writeln!(out, "- {}", untested_sentence(h));
        }
        out.push('\n');
    }

    if let Some(only) = &ctx.outcome.only {
        let others: Vec<String> = ctx
            .outcome
            .checks
            .iter()
            .filter(|c| c.id != *only)
            .map(|c| code(c.id.as_str()))
            .collect();
        let _ = writeln!(
            out,
            "This receipt covers the check {} only (`--check`){}.\n",
            code(only.as_str()),
            if others.is_empty() {
                String::new()
            } else {
                format!(
                    "; {} ran in jobs of their own, or not at all",
                    others.join(", ")
                )
            }
        );
    }
    if let Some(skipped) = receipt.skipped.as_ref().filter(|s| !s.is_empty()) {
        let names: Vec<String> = skipped.iter().map(|s| code(s.check.as_str())).collect();
        let _ = writeln!(out, "### Checks not run\n");
        let why = if skipped.len() == 1 {
            "the change touches no file it covers, no lockfile it installs from and no harness it runs with, so it would report what it reports on the base"
        } else {
            "the change touches no file they cover, no lockfile they install from and no harness they run with, so they would report what they report on the base"
        };
        let _ = writeln!(
            out,
            "{} {}: {why}. Run with `--all` to run every check.\n",
            if receipt.checks.is_empty() {
                "No check is affected by this change, so none ran:"
            } else {
                "Not affected by this change, so not run:"
            },
            names.join(", "),
        );
    }
    if !new.is_empty() {
        let _ = writeln!(out, "### What broke\n");
        entries(&mut out, &new, failures);
    }
    if !changed.is_empty() {
        needs_review(
            &mut out,
            &changed,
            failures,
            &ctx.outcome.review,
            ctx.outcome.diagnostics.reason(),
        );
    }

    criteria(&mut out, receipt, Some(&ctx.outcome.diagnostics));
    // With no check run, no test could show anything: say nothing.
    shown(
        &mut out,
        ctx.outcome.diagnostics.evidence.as_ref(),
        regression_only && !receipt.checks.is_empty(),
    );
    let protected = &ctx.outcome.task.body.protected_paths;
    if !protected.is_empty() {
        let globs: Vec<String> = protected.iter().map(|g| code(g.as_str())).collect();
        let _ = writeln!(out, "Protected paths: {}.\n", globs.join(", "));
    }

    let others: Vec<&ReceiptCheck> = env
        .iter()
        .chain(&flaky)
        .chain(&preexisting)
        .copied()
        .collect();
    if !others.is_empty() {
        let _ = writeln!(out, "### Not caused by this change\n");
        entries(&mut out, &others, failures);
    }

    let _ = writeln!(out, "### Notes\n");
    let _ = writeln!(
        out,
        "- {}",
        sandbox_note(ctx.executor, ctx.outcome.receipt.body.provenance.as_ref())
    );
    if let Some(note) = ctx.outcome.base_record.as_ref().and_then(base_record_note) {
        let _ = writeln!(out, "- {note}");
    }
    import_notes(&mut out, &ctx.outcome.import);
    if rules::judging_changed(checks) {
        let _ = writeln!(
            out,
            "- The head changes frozen test harness (`judging::` entries, CHANGED): the Aver configuration, `package.json` scripts or test runner fields, `.npmrc`, or test runner configuration. Every check ran with the base's copies, so the edit could not change any result; it is listed under \"Needs review\", and nothing counts as proven."
        );
    }
    if receipt.env_changed {
        let _ = writeln!(
            out,
            "- The head changes the environment (`aver.toml` or the lockfile). Every check ran under the base's configuration on both sides, with the head's lockfile so its dependencies install; a change to `aver.toml` is reported under `judging::`."
        );
    }
    for e in &inconclusive {
        if e.cause == Some(Cause::Preexisting) {
            let _ = writeln!(
                out,
                "- {} fails on the base too and has no per-test results, so this run cannot tell whether the change broke it: the verdict is BLOCKED_NOT_OWN, not PASS. A JUnit report (or a test runner Aver detects, see the documentation of the default check) lets Aver compare the check test by test.",
                code(e.id.as_str())
            );
        } else {
            let _ = writeln!(
                out,
                "- {} passed and failed on the base but failed on every head run: the head never passed it, so the verdict is BLOCKED_NOT_OWN, not PASS.",
                code(e.id.as_str())
            );
        }
    }
    if preexisting.iter().any(|e| !rules::is_command_level(e)) {
        let _ = writeln!(
            out,
            "- Per-test PREEXISTING entries already fail on the base. They do not block."
        );
    }
    if !flaky.is_empty() {
        let _ = writeln!(out, "- FLAKY entries both passed and failed across reruns. They block only a criterion whose every proof is flaky.");
    }
    if !env.is_empty() {
        let _ = writeln!(out, "- ENV entries ended in an infrastructure failure (timeout, out of memory or disk, or a run that could not start) after one retry, need a dependency that could not be built, or gave no evidence to judge, so the run is BLOCKED_NOT_OWN.");
        let mut said = std::collections::BTreeSet::new();
        for f in failures {
            let Some(note) = f
                .note
                .as_deref()
                .filter(|n| crate::diagnostics::is_original_build_note(n))
            else {
                continue;
            };
            if said.insert(note) {
                let first = f.message.as_deref().unwrap_or("not in the output");
                let _ = writeln!(
                    out,
                    "- {note}. First error: {}",
                    code(first.lines().next().unwrap_or(first))
                );
            }
        }
    }
    out.push('\n');

    let _ = writeln!(
        out,
        "| Passing | NEW | CHANGED | PREEXISTING | FLAKY | ENV |"
    );
    let _ = writeln!(out, "|---:|---:|---:|---:|---:|---:|");
    let _ = writeln!(
        out,
        "| {passing} | {} | {} | {} | {} | {} |\n",
        new.len(),
        changed.len(),
        preexisting.len(),
        flaky.len(),
        env.len()
    );

    let _ = writeln!(out, "### Receipt\n");
    let _ = writeln!(
        out,
        "Receipt {} is in `receipt.json`, signed by the verifier key {} (`public-key.txt`); `diagnostics.json` beside it explains each failure and unproven criterion and is not signed. {}\n",
        code(ctx.outcome.receipt_id.to_string().as_str()),
        code(ctx.public_key),
        if ctx.throwaway_key {
            "The key was generated for this run, so the signature shows the receipt has not changed since this runner wrote it; it does not show who ran the runner."
        } else {
            "The key is the verifier key of the Aver home this run used (`--home`): anyone who knows that key belongs to the operator can check that the operator's verifier signed this receipt."
        }
    );
    match ctx.viewer_url {
        Some(viewer) => {
            let sep = if viewer.contains('?') { '&' } else { '?' };
            let _ = writeln!(
                out,
                "To check it in the browser, open the [receipt viewer]({viewer}{sep}key={}) with the key filled in and drop `receipt.json` onto it.\n",
                ctx.public_key
            );
        }
        None => {
            let _ = writeln!(
                out,
                "To check it in the browser, open the Aver receipt viewer (`receipt.html`), paste the key and drop `receipt.json` onto it.\n"
            );
        }
    }
    let _ = writeln!(
        out,
        "To reproduce locally:\n\n{}",
        fenced(ctx.reproduce, "sh")
    );
    out
}

/// The "Criteria" section: each criterion, proven or not, with the test
/// that proves it, its weak proof or why it is unproven, then the proving
/// tests' source.
fn criteria(
    out: &mut String,
    receipt: &ReceiptBody,
    diagnostics: Option<&crate::diagnostics::Diagnostics>,
) {
    let checks = receipt.checks.as_slice();
    let rule = receipt.schema.proof_rule();
    let regression_only = receipt.criteria.is_empty();
    let _ = writeln!(out, "### Criteria\n");
    if regression_only {
        let _ = writeln!(
            out,
            "Regression-only: the pull request declares no criteria, so this run checks only that nothing broke. To declare criteria, add a fenced code block with the info string `aver` to the pull request description, one `- <id>: <text>` line per criterion.\n"
        );
    } else {
        for c in receipt.criteria.iter() {
            let proven = !receipt.unproven.contains(&c.id);
            let _ = write!(
                out,
                "- {} {}: {}",
                if proven { "Proven" } else { "Unproven" },
                code(c.id.as_str()),
                escape(&c.text)
            );
            let proof = checks
                .iter()
                .find(|e| e.proves.get(c.id.as_str()).is_some() && rules::proves(e, rule));
            if let (true, Some(e)) = (proven, proof) {
                let _ = write!(out, " (by {}", code(e.id.as_str()));
                match e.transplant {
                    Some(Transplant::Fail) => out.push_str(", which fails without the change"),
                    Some(Transplant::Absent) => {
                        out.push_str(", which cannot run without the change")
                    }
                    _ => {}
                }
                out.push(')');
            }
            if let Some(weak) = diagnostics
                .iter()
                .flat_map(|d| &d.weak_proofs)
                .find(|w| w.criterion == c.id)
            {
                let _ = write!(out, ". Weak proof: {}", weak.advice());
            }
            let diagnosis = diagnostics
                .iter()
                .flat_map(|d| &d.unproven)
                .find(|u| u.criterion == c.id);
            if let (false, Some(u)) = (
                proven,
                diagnosis.filter(|u| u.reason != UnprovenReason::WeakProof),
            ) {
                let _ = write!(out, ". {}", reason_sentence(u.reason));
                if !u.closest.is_empty() {
                    let names: Vec<String> = u.closest.iter().map(|n| code(n)).collect();
                    let _ = write!(out, " Closest test names: {}.", names.join(", "));
                }
            }
            out.push('\n');
        }
        out.push('\n');
        if let Some(d) = diagnostics {
            proof_sources(out, &d.proof_sources);
        }
    }
}

/// The evidence block of `summary.md`, for an accept's commit message and
/// pull request body: the counts line, what the verdict covers, the test
/// changes, the changed tests a person reviews with their diffs, and the
/// criteria with the tests proving them.
pub fn evidence(
    receipt: &ReceiptBody,
    diagnostics: Option<&crate::diagnostics::Diagnostics>,
) -> String {
    let mut out = String::new();
    if let Some(counts) = crate::run::report::counts_line(receipt) {
        let _ = writeln!(out, "{counts}\n");
    }
    let scope = crate::run::limits::scope(receipt, diagnostics, None);
    let _ = writeln!(
        out,
        "{}",
        crate::run::limits::markdown(&scope, receipt, diagnostics)
    );
    test_changes(&mut out, receipt);
    let changed: Vec<&ReceiptCheck> = receipt
        .checks
        .iter()
        .filter(|c| c.cause == Some(Cause::Changed))
        .collect();
    if !changed.is_empty() {
        let diffs: Vec<super::ReviewDiff> = diagnostics
            .iter()
            .flat_map(|d| &d.review)
            .map(|r| super::ReviewDiff {
                path: r.path.clone(),
                diff: r.diff.clone(),
            })
            .collect();
        let failures = diagnostics.map_or(&[][..], |d| d.failures.as_slice());
        needs_review(
            &mut out,
            &changed,
            failures,
            &diffs,
            diagnostics.and_then(|d| d.reason()),
        );
    }
    if !receipt.criteria.is_empty() {
        criteria(&mut out, receipt, diagnostics);
    }
    out.trim_end().to_owned()
}

/// "What the tests show" (§1.9.4, unsigned): the PR's own tests that fail on
/// the base and pass on the head, the failing tests it fixed and those that
/// still fail; without criteria, a line when nothing shows the change.
fn shown(out: &mut String, evidence: Option<&crate::diagnostics::Evidence>, regression_only: bool) {
    let names = |items: &mut dyn Iterator<Item = String>, total: u64| {
        let shown: Vec<String> = items.take(MAX_NAMED).map(|n| code(&n)).collect();
        let more = total.saturating_sub(shown.len() as u64);
        if more > 0 {
            format!("{} and {more} more", shown.join(", "))
        } else {
            shown.join(", ")
        }
    };
    let shows = evidence
        .map(|e| e.shows_change.as_slice())
        .unwrap_or_default();
    let fixed = evidence.map(|e| &e.fixed).filter(|f| !f.items.is_empty());
    let still = evidence
        .map(|e| &e.still_failing)
        .filter(|f| !f.items.is_empty());
    if shows.is_empty() && fixed.is_none() && still.is_none() && !regression_only {
        return;
    }
    let _ = writeln!(out, "### What the tests show\n");
    if shows.is_empty() && fixed.is_none() {
        let _ = writeln!(
            out,
            "- Nothing shows the change works: no test the PR added or changed fails on the base and passes on the head, and no test that failed on the base passes now."
        );
    }
    if !shows.is_empty() {
        let total = shows.len() as u64 + evidence.map_or(0, |e| e.shows_change_omitted);
        let _ = writeln!(
            out,
            "- {} the PR added or changed {} on the base (with the PR's test files) and {} on the head: {}.",
            count(total, "test", "tests"),
            if total == 1 { "fails or cannot load" } else { "fail or cannot load" },
            if total == 1 { "passes" } else { "pass" },
            names(&mut shows.iter().map(|s| s.id.as_str().to_owned()), total)
        );
        if shows.iter().all(crate::diagnostics::Shown::weak) {
            let _ = writeln!(
                out,
                "  Each fails on the base only because something it imports is missing there, which shows only that it exists now."
            );
        }
    }
    if let Some(f) = fixed {
        let total = f.items.len() as u64 + f.omitted;
        let _ = writeln!(
            out,
            "- Fixed: {} that failed on the base {} on the head: {}.",
            count(total, "test", "tests"),
            if total == 1 { "passes" } else { "pass" },
            names(&mut f.items.iter().cloned(), total)
        );
    }
    if let Some(s) = still {
        let total = s.items.len() as u64 + s.omitted;
        let _ = writeln!(
            out,
            "- Still failing on both sides: {} (under \"Not caused by this change\"). If the PR is meant to fix failing tests, these are what it has not fixed.",
            count(total, "test", "tests")
        );
    }
    out.push('\n');
}

/// The summary written when the run fails before it has a receipt.
pub fn render_error(message: &str) -> String {
    format!(
        "## Aver: error\n\nAver could not produce a receipt.\n\n{}\n",
        code_block(message)
    )
}

fn headline(
    verdict: Verdict,
    regression_only: bool,
    new: usize,
    unproven: usize,
    inconclusive: bool,
    changed: usize,
) -> String {
    match verdict {
        Verdict::Pass if regression_only => {
            "Nothing broke: no check fails only with this change.".into()
        }
        Verdict::Pass => "Every criterion is proven and nothing broke.".into(),
        Verdict::FailOwn => format!(
            "This change broke something: {new} {} only with it.",
            if new == 1 {
                "check fails"
            } else {
                "checks fail"
            }
        ),
        Verdict::BlockedNotOwn if inconclusive => {
            "Blocked: a check that fails on the base too hides what this change did, so nothing says it is safe.".into()
        }
        Verdict::BlockedNotOwn => {
            "Blocked by infrastructure, flaky tests, a dependency that could not be built or tests that gave no evidence, not by this change.".into()
        }
        Verdict::Review => format!(
            "Needs review: nothing broke, and this change rewrites {}. A human reviews only those, below.",
            if changed == 1 {
                "1 test expectation or harness file".to_owned()
            } else {
                format!("{changed} test expectations or harness files")
            }
        ),
        Verdict::Unproven => format!(
            "Nothing broke, but {unproven} {} no passing proof.",
            if unproven == 1 {
                "criterion has"
            } else {
                "criteria have"
            }
        ),
    }
}

/// The test behind each claimed criterion, fenced: what a reviewer reads to
/// see whether the proof checks behaviour or only that a name exists.
fn proof_sources(out: &mut String, sources: &[crate::diagnostics::ProofSource]) {
    if sources.is_empty() {
        return;
    }
    out.push_str("Proving tests (repository content):\n\n");
    for p in sources {
        let _ = writeln!(
            out,
            "{} by {}, {}:{}\n\n{}\n",
            code(p.criterion.as_str()),
            code(p.entry.as_str()),
            code(&p.path),
            p.line,
            code_block(&p.source)
        );
    }
}

/// What an unproven criterion's diagnostics reason means for the author.
/// The hunk necessity pushback for `hunk`, its location as inline code.
fn untested_sentence(hunk: &crate::diagnostics::HunkTried) -> String {
    use aver_core::pushback::{LineRange, Piece, Pushback};
    let Ok(path) = hunk.path.parse() else {
        return code(&hunk.at());
    };
    let pushback = Pushback::UntestedHunk {
        path,
        lines: LineRange {
            start: hunk.start,
            end: hunk.end,
        },
    };
    pushback
        .sentence()
        .pieces()
        .iter()
        .map(|piece| match piece {
            Piece::Words(w) => w.clone(),
            Piece::Data(d) => code(d),
        })
        .collect()
}

fn reason_sentence(reason: UnprovenReason) -> &'static str {
    match reason {
        UnprovenReason::NotClaimed => "No test carries its `[crit:<id>]` tag.",
        UnprovenReason::ClaimedTestNotFound => "A test file carries its tag, but no test report has a test with that tag in its name.",
        UnprovenReason::NoTestNames => "Its checks report no test names (each is judged by its exit code alone), so no test can carry its `[crit:<id>]` tag: give the test check a reporter Aver reads (JUnit, Jest JSON, go-json, libtest) in `aver.toml`.",
        UnprovenReason::PassesWithoutChange => "Its tagged tests pass on the base plus only this change's tests, so they do not show the change is needed.",
        UnprovenReason::FailsAtHead => "Its tagged tests fail on the head.",
        UnprovenReason::JudgingFilesChanged => "The head changes judging files, so nothing is proven.",
        UnprovenReason::Blocked => "Its tagged tests are flaky or hit an infrastructure failure.",
        UnprovenReason::UntestedHunk => "Reverting one hunk of the change left every proving test passing, so no test needs that code.",
        UnprovenReason::WeakProof => "Its tagged tests fail on the base plus only this change's tests just because something they use is missing there, which shows the name is new, not that it works (a weak proof).",
    }
}

fn exit_line(fail_on: FailOn, receipt: &aver_core::ReceiptBody) -> String {
    let verdict = receipt.verdict;
    if fail_on.fails_receipt(receipt) {
        let why = if matches!(fail_on, FailOn::Unexplained | FailOn::Regressions)
            && verdict == Verdict::BlockedNotOwn
        {
            " when the base does not explain the block (an infrastructure failure, or blocking entries that pass on the base)"
        } else {
            ""
        };
        format!(
            "The check fails: `--fail-on {}` fails on {}{why}.",
            fail_on.as_str(),
            verdict.as_str()
        )
    } else if fail_on.exit_code(receipt) == crate::ci::EXIT_NEEDS_MAINTAINER {
        format!(
            "The check needs a maintainer (exit 3): `--fail-on {}` does not pass {}, because nothing broke but nothing proved the change safe either.",
            fail_on.as_str(),
            verdict.as_str()
        )
    } else {
        format!("The check passes under `--fail-on {}`.", fail_on.as_str())
    }
}

/// Where the base's results came from when a record was restored.
fn base_record_note(record: &crate::ci::BaseRecord) -> Option<String> {
    use crate::ci::{record::Restored, RecordSource};
    let (name, by) = match record.source {
        RecordSource::Flag => (
            "The base record (`--base-record`)",
            "recorded by an earlier job on this base commit, runner and dependency inputs (`--base-record`)",
        ),
        RecordSource::Home => (
            "The home's base record (`$AVER_HOME/records`)",
            "recorded by an earlier `aver ci` on this machine for this base commit and dependency inputs (`$AVER_HOME/records`; `--fresh` or `--no-records` reads none)",
        ),
    };
    match &record.restored {
        Restored::Ignored(why) => Some(format!(
            "{name} was ignored: {}. The base ran on this runner.",
            escape(why)
        )),
        Restored::Loaded { .. } if record.served == 0 && record.confirmed > 0 => Some(format!(
            "{name} did not hold here: a fresh base run disagreed with it on a test the head fails, so the base ran on this runner."
        )),
        Restored::Loaded { .. } if record.served == 0 => Some(format!(
            "{name} held none of the runs this verification needed; the base ran on this runner."
        )),
        Restored::Loaded { .. } => {
            let mut note = format!(
                "The base came from a recorded run: {} {by}; {} on this runner.",
                count(record.served as u64, "base run was", "base runs were"),
                count(record.started as u64, "base run started", "base runs started"),
            );
            if record.confirmed > 0 {
                note.push_str(&format!(
                    " A result that rested on the record alone was checked by {} first.",
                    count(record.confirmed, "fresh base run", "fresh base runs")
                ));
            }
            Some(note)
        }
    }
}

fn sandbox_note(executor: &str, provenance: Option<&aver_core::Provenance>) -> String {
    if let Some(p) = provenance.filter(|p| p.mode != aver_core::ProvenanceMode::Sandbox) {
        let network = match p.network {
            aver_core::Network::Off => "the network off",
            aver_core::Network::On => "the network on",
        };
        let uid = match p.uid {
            aver_core::Uid::Separate => "as a separate user that cannot read the verifier key",
            aver_core::Uid::Same => "as the runner's own user",
        };
        return format!(
            "Native ({}): checks ran on this runner {uid}, with {network}, the job's own environment and the base's harness; no sandbox isolated them.",
            code(executor)
        );
    }
    match executor {
        "docker-runsc-v0" => "Sandbox: gVisor. Checks ran in Docker sandboxes under gVisor (`runsc`), with no network, no host environment and no secrets.".into(),
        "docker-runc-v0" => "Sandbox: plain Docker (runc). Checks ran in hardened Docker sandboxes without gVisor, with no network, no host environment and no secrets; gVisor isolates untrusted code more strongly.".into(),
        other => format!("Sandbox: the {} executor.", code(other)),
    }
}

/// The test files whose base versions did not compile on the judged head,
/// from their entries.
fn unbuilt_files(receipt: &ReceiptBody) -> Vec<&str> {
    receipt
        .checks
        .iter()
        .filter_map(|e| e.id.as_str().split_once("::"))
        .filter_map(|(_, test)| test.strip_prefix(crate::verify::UNBUILT_TEST_PATH))
        .map(str::trim_start)
        .collect()
}

/// The "What this PR did to the tests" section: every change to test files
/// and harness, and what the base's copies found, in plain words.
fn test_changes(out: &mut String, receipt: &ReceiptBody) {
    let Some(tc) = receipt.test_changes.as_ref() else {
        return;
    };
    let _ = writeln!(out, "### What this PR did to the tests\n");
    if tc.is_empty() {
        let _ = writeln!(
            out,
            "Nothing: no test file or test harness changed, and every test that ran on the base still ran.\n"
        );
        return;
    }
    let unbuilt = unbuilt_files(receipt);
    let among = |l: &Listed<aver_core::TestCheckId>, cause: Cause| -> usize {
        l.items
            .iter()
            .filter(|id| {
                receipt
                    .checks
                    .get(id.as_str())
                    .is_some_and(|e| e.cause == Some(cause))
            })
            .count()
    };
    let still_ran = |l: &Listed<aver_core::TestCheckId>| -> String {
        let (new, changed) = (among(l, Cause::New), among(l, Cause::Changed));
        let scope = if l.omitted > 0 {
            " among those listed"
        } else {
            ""
        };
        let mut found = Vec::new();
        if changed > 0 {
            found.push(format!(
                "{changed} now {} CHANGED (needs review)",
                if changed == 1 { "fails as" } else { "fail as" }
            ));
        }
        if new > 0 {
            found.push(format!(
                "{new} now {} NEW",
                if new == 1 { "fails as" } else { "fail as" }
            ));
        }
        let lead = match unbuilt.len() as u64 {
            0 => "they still ran from the base version".to_owned(),
            n => format!(
                "{} did not compile against the new code in {} base version (CHANGED, needs review), so {} tests did not run; the remaining tests were judged one by one",
                count(n, "test file", "test files"),
                if n == 1 { "its" } else { "their" },
                if n == 1 { "its" } else { "their" },
            ),
        };
        if found.is_empty() {
            let none = if unbuilt.is_empty() {
                "none"
            } else {
                "none of them"
            };
            format!("{lead}, and {none} fails because of this change{scope}")
        } else {
            format!("{lead}: {}{scope}", found.join(", "))
        }
    };
    let not_compiled = |files: &Listed<aver_core::Path>| -> String {
        let n = files
            .items
            .iter()
            .filter(|p| unbuilt.contains(&p.as_str()))
            .count() as u64;
        match n {
            0 => String::new(),
            1 => " The base version of 1 of them did not compile against the new code (CHANGED, needs review), so its tests did not run; the remaining tests were judged one by one.".to_owned(),
            n => format!(" The base versions of {n} of them did not compile against the new code (CHANGED, needs review), so their tests did not run; the remaining tests were judged one by one."),
        }
    };
    if !tc.files_added.is_empty() {
        let _ = writeln!(
            out,
            "- Added {}: {}.",
            count(tc.files_added.total(), "test file", "test files"),
            named(&tc.files_added)
        );
    }
    if !tc.files_modified.is_empty() {
        let _ = writeln!(
            out,
            "- Modified {}: {}. Regressions were judged with the base's versions; the PR's versions ran separately, to prove criteria.{}",
            count(tc.files_modified.total(), "existing test file", "existing test files"),
            named(&tc.files_modified),
            not_compiled(&tc.files_modified)
        );
    }
    if !tc.files_deleted.is_empty() {
        let _ = writeln!(
            out,
            "- Deleted {}: {}.{}",
            count(
                tc.files_deleted.total(),
                "existing test file",
                "existing test files"
            ),
            named(&tc.files_deleted),
            match not_compiled(&tc.files_deleted) {
                s if s.is_empty() => " They still ran from the base version.".to_owned(),
                s => s,
            }
        );
    }
    if !tc.tests_removed.is_empty() {
        let _ = writeln!(
            out,
            "- Deleted {} ({}): {}.",
            count(tc.tests_removed.total(), "existing test", "existing tests"),
            still_ran(&tc.tests_removed),
            named(&tc.tests_removed)
        );
    }
    if !tc.tests_skipped.is_empty() {
        let _ = writeln!(
            out,
            "- Skipped {} in the PR's version ({}): {}.",
            count(tc.tests_skipped.total(), "existing test", "existing tests"),
            still_ran(&tc.tests_skipped),
            named(&tc.tests_skipped)
        );
    }
    if !tc.harness_changed.is_empty() {
        let _ = writeln!(
            out,
            "- Changed frozen test harness ({}, CHANGED; verification ran the base's version): {}.",
            count(tc.harness_changed.total(), "change", "changes"),
            named(&tc.harness_changed)
        );
    }
    out.push('\n');
}

/// Notes on what the imports let through: possible secrets (never their
/// values) and files imported empty.
fn import_notes(out: &mut String, notes: &crate::diagnostics::ImportNotes) {
    let more = |shown: usize, omitted: u64| -> String {
        let rest = omitted + shown.saturating_sub(MAX_NAMED) as u64;
        if rest > 0 {
            format!(", and {rest} more in `diagnostics.json`")
        } else {
            String::new()
        }
    };
    if !notes.secrets.is_empty() {
        let named: Vec<String> = notes
            .secrets
            .iter()
            .take(MAX_NAMED)
            .map(|s| {
                let low = if s.severity == "low" { ", low" } else { "" };
                format!("{} ({}{low})", code(&s.path), s.pattern)
            })
            .collect();
        let _ = writeln!(
            out,
            "- Possible secrets in: {}{}. Their values are never shown. The import went on because the code is already in this repository's Git, and checks never get the CI job's tokens or job files, but they run natively with the network on, so treat a real key here as exposed and rotate it. Pass `--strict-secrets` to refuse such imports instead. Low means a template, a test, fixture or example path, or Markdown documentation.",
            named.join(", "),
            more(notes.secrets.len(), notes.secrets_omitted)
        );
    }
    if !notes.placeholders.is_empty() {
        let named: Vec<String> = notes
            .placeholders
            .iter()
            .take(MAX_NAMED)
            .map(|p| format!("{} ({} bytes)", code(&p.path), p.bytes))
            .collect();
        let _ = writeln!(
            out,
            "- Files over the 64 MiB blob limit were imported empty: {}{}. A check that reads one sees an empty file.",
            named.join(", "),
            more(notes.placeholders.len(), notes.placeholders_omitted)
        );
    }
}

fn count(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Up to [`MAX_NAMED`] items as code spans, and how many more there are.
fn named<T: SetItem + AsRef<str>>(list: &Listed<T>) -> String {
    let mut names: Vec<String> = list
        .items
        .iter()
        .take(MAX_NAMED)
        .map(|i| code(i.as_ref()))
        .collect();
    let more = list.total() - names.len() as u64;
    if more > 0 {
        names.push(format!("and {more} more in `diagnostics.json`"));
    }
    names.join(", ")
}

fn entries(out: &mut String, list: &[&ReceiptCheck], failures: &[Failure]) {
    let mut shown: Vec<String> = Vec::new();
    for e in list.iter().take(MAX_ENTRIES) {
        let cause = e.cause.map_or("", Cause::as_str);
        let failure = failures.iter().find(|f| f.id == e.id);
        let _ = writeln!(
            out,
            "- {} {cause}, base {}, head {}",
            code(e.id.as_str()),
            base_str(e),
            head_str(e.head)
        );
        if e.cause == Some(Cause::New) {
            if let Some(own) = failure.and_then(|f| f.own) {
                let _ = writeln!(out, "  {}.", own_sentence(own));
            }
        }
        if let Some(note) = failure
            .and_then(|f| f.note.as_deref())
            .filter(|_| e.cause == Some(Cause::Env))
        {
            let _ = writeln!(out, "  Aver: {}.", escape(note));
        }
        let explains = matches!(e.cause, Some(Cause::New | Cause::Env));
        if let Some(detail) = e.detail.as_ref().filter(|_| explains) {
            let text = clip(&crate::run::report::strip_slot(detail.as_str()), MAX_DETAIL);
            if shown.contains(&text) {
                let _ = writeln!(out, "  The same failure as above.");
                continue;
            }
            for line in code_block(&text).lines() {
                let _ = writeln!(out, "  {line}");
            }
            shown.push(text);
        }
    }
    if list.len() > MAX_ENTRIES {
        let _ = writeln!(
            out,
            "- and {} more in `receipt.json`",
            list.len() - MAX_ENTRIES
        );
    }
    out.push('\n');
}

/// Whether the PR's own version of a NEW test passes, in words.
fn own_sentence(own: Transplant) -> &'static str {
    match own {
        Transplant::Pass => "The PR's own version of this test passes; the base's version fails",
        Transplant::Fail => "The PR's own version of this test fails too",
        Transplant::Absent => "The PR's own version of the tests no longer has this test",
        Transplant::Infra => {
            "The PR's own version of this test could not run (infrastructure failure)"
        }
    }
}

/// The "Needs review" section: each `CHANGED` entry with what it means, then
/// the diff of each test or harness file behind them. The reviewer checks
/// the intent of a changed expectation, not every line of the change.
fn needs_review(
    out: &mut String,
    changed: &[&ReceiptCheck],
    failures: &[Failure],
    diffs: &[super::ReviewDiff],
    reason: Option<&str>,
) {
    let _ = writeln!(out, "### Needs review: test changes\n");
    let _ = writeln!(
        out,
        "This PR changes what existing tests expect, or the test harness. Aver cannot tell an intended behaviour change from a weakened test, so it asks a person to review the intent of these changes instead of every line of the PR: approve them if the PR meant to change this behaviour. Every regression was still judged with the base's tests and harness.\n"
    );
    for e in changed.iter().take(MAX_ENTRIES) {
        let failure = failures.iter().find(|f| f.id == e.id);
        let unbuilt =
            e.id.as_str()
                .split_once("::")
                .is_some_and(|(_, t)| t.starts_with(crate::verify::UNBUILT_TEST_PATH));
        let deleted_source =
            e.id.as_str()
                .split_once("::")
                .is_some_and(|(_, t)| t.starts_with(crate::verify::DELETED_SOURCE_TEST_PATH));
        let sensitive =
            e.id.as_str()
                .split_once("::")
                .is_some_and(|(check, _)| check == crate::verify::SENSITIVE_CHECK);
        let why = if sensitive {
            "the PR edits a path the base's aver.toml marks sensitive (by default CI workflows and actions, which run with the repository's secrets); nothing failed, and a person reviews the edit before it lands"
        } else if deleted_source {
            "the PR deleted this source file, and the tests inside it went with the code they tested, so they did not run; Aver cannot tell a removed feature from a regression these tests would catch"
        } else if unbuilt {
            "the PR changed or deleted this test file, and its base version does not compile against the new code, so its tests did not run; Aver cannot tell a removed feature from a regression these tests would catch. The first compiler error"
        } else if rules::judging_changed(std::slice::from_ref(*e)) {
            "the PR changes the test harness; every check ran with the base's version"
        } else if failure.and_then(|f| f.own) == Some(Transplant::Absent) {
            "the old version of this test fails on the new code; the PR deleted it"
        } else if e.base == aver_core::BaseStatus::Fail {
            "this test already failed on the base and its old version still fails on the new code; only the PR's rewritten version passes, so it was rewritten, not fixed"
        } else {
            "the old version of this test fails on the new code; the PR's version passes"
        };
        let file = failure
            .and_then(|f| f.file.as_ref())
            .map(|p| format!(" (in {})", code(p.as_str())))
            .unwrap_or_default();
        match e.detail.as_ref().filter(|_| unbuilt) {
            Some(detail) => {
                let error = detail
                    .as_str()
                    .split_once("First error:\n")
                    .map_or(detail.as_str(), |(_, e)| e);
                let _ = writeln!(
                    out,
                    "- {}{file}: {why}:\n\n{}\n",
                    code(e.id.as_str()),
                    fenced(error, "text")
                );
            }
            None => {
                let _ = writeln!(out, "- {}{file}: {why}.", code(e.id.as_str()));
            }
        }
    }
    if changed.len() > MAX_ENTRIES {
        let _ = writeln!(
            out,
            "- and {} more in `receipt.json`",
            changed.len() - MAX_ENTRIES
        );
    }
    out.push('\n');
    if let Some(reason) = reason {
        let _ = writeln!(
            out,
            "Stated reason (from the agent, not checked):\n\n{}\n",
            fenced(reason, "text")
        );
    }
    for d in diffs {
        let _ = writeln!(out, "{}:\n\n{}\n", code(&d.path), fenced(&d.diff, "diff"));
    }
}

fn format_name(format: aver_core::ReportFormat) -> &'static str {
    format.as_str()
}

fn default_config_line(generic: bool) -> String {
    format!(
        "No aver.toml at base: used {}, run natively with the tools and dependencies installed on this runner. Add an `aver.toml` to choose the checks, or run with `--require-config` to refuse this.",
        if generic {
            "the checks Aver detects for the packages in the repository"
        } else {
            "the default checks, `npm test` and for a TypeScript project a type check"
        }
    )
}

/// What the default check ran (§1.4.1): its command as the slot runs it
/// (the report under `$AVER_OUT`), the runner whose reporter Aver added,
/// and its setup commands.
fn default_check_line(check: &aver_core::CheckDef, exit_only: Option<&str>) -> String {
    let command = |argv: &[String]| {
        code(
            crate::contract::shell_line(argv)
                .map(|l| l.as_str().to_owned())
                .unwrap_or_else(|_| argv.join(" "))
                .as_str(),
        )
    };
    let mut line = format!(
        "Default check {}: {}",
        code(check.id.as_str()),
        command(&check.argv)
    );
    if crate::presets::is_typecheck(check) {
        line.push_str(", the type-check gate the base gets for listing `typescript` in its devDependencies beside a `tsconfig.json`, one entry per type error in its `tsc` output.");
        return line;
    }
    match (crate::adhoc::runner::runner_of(check), &check.report) {
        (Some(runner), Some(report)) => {
            let _ = write!(
                line,
                ", runner {} detected, reporter {} at {} for per-test results",
                code(runner.name()),
                code(format_name(report.format)),
                code(&report.path.as_str().replacen("/aver/out", "$AVER_OUT", 1))
            );
        }
        (None, Some(report)) => {
            let _ = write!(line, ", reporter {}", code(format_name(report.format)));
        }
        (_, None) => match exit_only {
            Some(why) => {
                let _ = write!(line, ", judged by its exit code alone: {why}");
            }
            None => line.push_str(
                ", no test runner Aver can add a reporter to, so it is judged by its exit code alone",
            ),
        },
    }
    if check.setup.is_empty() {
        line.push_str("; no setup steps.");
    } else {
        let steps: Vec<String> = check.setup.iter().map(|s| command(s)).collect();
        let _ = write!(line, "; setup steps first: {}.", steps.join(", "));
    }
    line
}

fn base_str(e: &ReceiptCheck) -> &'static str {
    use aver_core::BaseStatus::*;
    match e.base {
        Pass => "pass",
        Fail => "fail",
        Flaky => "flaky",
        NotApplicable => "n/a",
    }
}

fn head_str(head: HeadStatus) -> &'static str {
    match head {
        HeadStatus::Pass => "pass",
        HeadStatus::Fail => "fail",
        HeadStatus::Flaky => "flaky",
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… ({} more bytes)", &text[..end], text.len() - end)
}

fn longest_run(text: &str, ch: char) -> usize {
    let (mut best, mut run) = (0, 0);
    for c in text.chars() {
        run = if c == ch { run + 1 } else { 0 };
        best = best.max(run);
    }
    best
}

/// An inline code span holding `text` literally on one line.
pub fn code(text: &str) -> String {
    let text = text.replace(['\n', '\r'], " ");
    let fence = "`".repeat(longest_run(&text, '`') + 1);
    let pad = if text.starts_with(['`', ' ']) || text.ends_with(['`', ' ']) {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

/// A fenced block holding `text` literally.
pub fn code_block(text: &str) -> String {
    fenced(text, "text")
}

fn fenced(text: &str, info: &str) -> String {
    let fence = "`".repeat((longest_run(text, '`') + 1).max(3));
    format!(
        "{fence}{info}\n{}\n{fence}",
        text.trim_end_matches(['\n', '\r'])
    )
}

/// Text from the pull request description as plain Markdown text: ASCII
/// punctuation escaped, one line, and `@` broken so it mentions nobody.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' | '\r' => out.push(' '),
            '@' => out.push_str("@\u{200b}"),
            c if c.is_ascii_punctuation() => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_check_reads_as_the_slot_runs_it() {
        let manifest = br#"{"scripts":{"test":"node --test"}}"#;
        let mut check = crate::presets::default_check(Some(manifest), None, &|_| false);
        let line = default_check_line(&check, None);
        assert!(line.contains("$AVER_OUT/"), "{line}");
        check.argv = ["sh", "-c", "exec go test -json ./... >/aver/out/go.json"]
            .map(String::from)
            .to_vec();
        let line = default_check_line(&check, None);
        assert!(
            line.contains("`exec go test -json ./... >$AVER_OUT/go.json`"),
            "{line}"
        );
        for generic in [true, false] {
            let text = format!("{line}\n{}", default_config_line(generic));
            for stale in ["/aver/out", "image", "limits", "install command"] {
                assert!(!text.contains(stale), "{stale}: {text}");
            }
        }
    }
}
