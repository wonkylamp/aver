//! What a verdict covers (`aver_core::scope`, §1.9.5), as every surface
//! renders it: the agent report's section and its verdict-line suffix,
//! `aver ci`'s `summary.md`, commit messages, and the `scope` field of
//! `POST /v0/verify` and `GET /v0/receipts/{id}`.

use std::fmt::Write;
use std::sync::OnceLock;

use aver_core::scope::{self, NarrowSide, Narrowing, Reuse, Scope, Weak, WeakKind};
use aver_core::{ReceiptBody, Timestamp};

use crate::diagnostics::{
    Diagnostics, HunkTried, Necessity, NotChecked, WEAK_BUILD_ERROR, WEAK_MISSING_MODULE,
    WEAK_MISSING_NAME,
};
use crate::verify::Timing;

/// The section's first line.
pub const HEADING: &str = "What this verdict covers:";

/// The scope of a receipt, from its diagnostics' weak proofs and the cache
/// reuse its timing recorded (a verify's `timing`, else `aver ci`'s
/// `timings`).
pub fn scope(
    body: &ReceiptBody,
    diagnostics: Option<&Diagnostics>,
    timing: Option<&Timing>,
) -> Scope {
    let weak: Vec<Weak> = diagnostics
        .map(|d| {
            d.weak_proofs
                .iter()
                .map(|w| Weak {
                    criterion: w.criterion.clone(),
                    kind: match w.reason.as_str() {
                        WEAK_MISSING_MODULE => WeakKind::MissingModule,
                        WEAK_MISSING_NAME => WeakKind::MissingName,
                        WEAK_BUILD_ERROR if w.new_api => WeakKind::NewApi,
                        _ => WeakKind::BuildError,
                    },
                })
                .collect()
        })
        .unwrap_or_default();
    let reuse = match (timing, diagnostics.and_then(|d| d.timings.as_ref())) {
        (Some(t), _) => Some(Reuse {
            runs: t.sides.iter().map(|s| s.started + s.cached).sum(),
            reused: t.sides.iter().map(|s| s.cached).sum(),
            older_verifier: installed().is_some_and(|at| {
                t.sides
                    .iter()
                    .any(|s| s.cached_from.is_some_and(|from| from < at))
            }),
        }),
        (None, Some(t)) => Some(Reuse {
            runs: t.runs.len() as u32,
            reused: t.runs.iter().filter(|r| r.cache_hit).count() as u32,
            // `aver ci` verifies in a throwaway home: its cache is its own.
            older_verifier: false,
        }),
        (None, None) => None,
    };
    let mut narrowed: Vec<Narrowing> = Vec::new();
    for n in diagnostics.into_iter().flat_map(|d| &d.narrowed) {
        let (Some(of), false) = (n.of, n.ran.is_empty()) else {
            continue;
        };
        let side = n.side;
        match narrowed.iter_mut().find(|m| m.side == side) {
            Some(m) => {
                m.ran += n.ran.len() as u32;
                m.of += of;
            }
            None => narrowed.push(Narrowing {
                side,
                ran: n.ran.len() as u32,
                of,
            }),
        }
    }
    narrowed.sort_by_key(|n| n.side != NarrowSide::Transplant);
    let necessity = diagnostics
        .and_then(|d| d.necessity.as_ref())
        .map(|n| match n {
            Necessity::Checked { hunks } => scope::Necessity::Checked {
                hunks: hunks.len() as u32,
                untested: n.untested().map(HunkTried::at).collect(),
            },
            Necessity::NotChecked { why, hunks } => scope::Necessity::NotChecked {
                hunks: *hunks,
                why: match why {
                    NotChecked::NoProvingTests => scope::NotChecked::NoProvingTests,
                    NotChecked::NoSelection { .. } => scope::NotChecked::NoSelection,
                    NotChecked::TooManyHunks => scope::NotChecked::TooManyHunks,
                },
            },
        });
    Scope::of(body, &weak, reuse)
        .with_narrowed(narrowed)
        .with_necessity(necessity)
}

/// When the running binary was installed. The run cache records no
/// verifier version, so a run cached before then is taken to be an older
/// verifier's.
fn installed() -> Option<Timestamp> {
    static AT: OnceLock<Option<Timestamp>> = OnceLock::new();
    *AT.get_or_init(|| {
        let modified = std::env::current_exe()
            .ok()?
            .metadata()
            .ok()?
            .modified()
            .ok()?;
        let ms = modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis();
        Timestamp::from_unix_millis(i64::try_from(ms).ok()?).ok()
    })
}

/// The agent report's section: two lines when nothing limits the verdict,
/// else the heading, one indented line each and the names fenced
/// (CLAUDE.md rule 7). Ends with a newline.
pub fn section(scope: &Scope) -> String {
    let lines = scope.lines();
    let mut out = String::new();
    if scope.compact() {
        let _ = writeln!(out, "{HEADING} {}", lines[0]);
        for line in &lines[1..] {
            let _ = writeln!(out, "  {line}");
        }
        return out;
    }
    let _ = writeln!(out, "{HEADING}");
    for line in &lines {
        let _ = writeln!(out, "  {line}");
    }
    let names: Vec<String> = scope
        .names()
        .iter()
        .map(|n| super::report::one_line(n))
        .collect();
    if !names.is_empty() {
        out.push_str(&super::report::fence(&names.join("\n")));
    }
    out
}

/// `summary.md`'s section: the same lines as bullets, then what the run
/// proved and what Aver could not check, which a person should look at;
/// the names in code spans.
pub fn markdown(scope: &Scope, body: &ReceiptBody, diagnostics: Option<&Diagnostics>) -> String {
    let mut out = format!("### {}\n\n", HEADING.trim_end_matches(':'));
    for line in scope.lines() {
        // The blind-spots line below counts them.
        if !line.starts_with("Not covered:") {
            let _ = writeln!(out, "- {line}");
        }
    }
    let shown = diagnostics
        .and_then(|d| d.evidence.as_ref())
        .map_or(0, |e| {
            e.shows_change.len() + e.shows_change_omitted as usize
        });
    let _ = writeln!(
        out,
        "- Proven: {} frozen (each judged with the base's own copy), {} on the base and passed on the head.",
        count(
            super::report::frozen_base_tests(body),
            "base test",
            "base tests"
        ),
        count(shown, "new test failed", "new tests failed"),
    );
    let mut blind = Vec::new();
    if scope.uncovered_total > 0 {
        blind.push(count(
            scope.uncovered_total as usize,
            "changed file no check covers",
            "changed files no check covers",
        ));
    }
    let exit = scope
        .ran
        .iter()
        .filter(|r| r.level == Some(aver_core::Level::Exit))
        .count();
    if exit > 0 {
        blind.push(count(
            exit,
            "check judged by its exit code alone",
            "checks judged by their exit code alone",
        ));
    }
    let sensitive: Vec<String> = body
        .checks
        .iter()
        .filter_map(|c| {
            c.id.as_str()
                .strip_prefix(crate::verify::SENSITIVE_CHECK)?
                .strip_prefix("::")
                .map(|p| crate::ci::summary::code(&super::report::one_line(p)))
        })
        .collect();
    if !sensitive.is_empty() {
        blind.push(format!(
            "{} modified ({})",
            count(sensitive.len(), "sensitive path", "sensitive paths"),
            sensitive.join(", ")
        ));
    }
    if let Some(scope::Necessity::NotChecked { hunks, why }) = &scope.necessity {
        blind.push(format!(
            "whether a test needs each of {} (hunk ablation skipped: {})",
            count(*hunks as usize, "changed code part", "changed code parts"),
            why.reason()
        ));
    }
    let _ = writeln!(
        out,
        "- What Aver could not check: {}.",
        if blind.is_empty() {
            "nothing it can name; it knows only what the checks ran".to_owned()
        } else {
            format!("{}; a person should look at these", blind.join("; "))
        }
    );
    for list in scope.name_lists() {
        let mut parts: Vec<String> = list
            .names
            .iter()
            .map(|(n, exit)| {
                let span = crate::ci::summary::code(&super::report::one_line(n));
                if *exit {
                    format!("{span} (exit code only)")
                } else {
                    span
                }
            })
            .collect();
        if list.more > 0 {
            parts.push(format!("+{} more", list.more));
        }
        let _ = writeln!(out, "- {}: {}", capitalized(list.label), parts.join(", "));
    }
    out
}

/// The lines a commit message carries: what ran, what no check covers and
/// where, in Aver's words only.
pub fn commit_lines(scope: &Scope) -> String {
    scope
        .lines()
        .into_iter()
        .filter(|l| {
            l.starts_with("Ran ") || l.starts_with("Not covered:") || l.starts_with("Where:")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!(
        "{} {}",
        super::report::thousands(n),
        if n == 1 { one } else { many }
    )
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aver_core::{
        BaseStatus, CheckSource, ConfigSource, Criteria, HeadStatus, Level, Listed, Network,
        ProvenanceMode, Receipt, ReceiptBody, ReceiptCheck, ReceiptId, ReceiptSchema, Set,
        SideRuns, SigningKey, SkipReason, Skipped, TestChanges, Uid, Verdict,
    };

    use super::*;
    use crate::diagnostics::{RunTime, Timings, WeakProof};
    use crate::run::report::{agent_text, last_line, ReportInput};

    const HASH: &str = "hbxnfo5ka4n2yknfwrktfu2mmqy34zduaxcdoa2vmrdpxbuzzpaq";

    fn id<T: std::str::FromStr>(prefix: &str) -> T
    where
        T::Err: std::fmt::Debug,
    {
        format!("{prefix}_{HASH}").parse().unwrap()
    }

    fn entry(name: &str) -> ReceiptCheck {
        ReceiptCheck {
            id: name.parse().unwrap(),
            base: BaseStatus::Pass,
            head: HeadStatus::Pass,
            cause: None,
            proves: Set::empty(),
            source: CheckSource::Env,
            runs: SideRuns {
                base: vec![],
                head: vec![],
                transplant: vec![],
                head_tests: vec![],
            },
            detail: None,
            transplant: None,
            head_tests: None,
        }
    }

    /// A v3 receipt with `verdict`, run as `mode` says, reading `levels`.
    fn body(verdict: Verdict, mode: Option<(ProvenanceMode, Network, Uid)>) -> ReceiptBody {
        ReceiptBody {
            schema: if mode.is_some() {
                ReceiptSchema::V3
            } else {
                ReceiptSchema::V2
            },
            task_id: id("task"),
            attempt_id: None,
            agent_id: id("agent"),
            base_snapshot: id("snap"),
            head_snapshot: id("snap"),
            environment_hash: id("env"),
            env_changed: false,
            criteria: Criteria::new(Vec::new()).unwrap(),
            checks: Set::new(vec![entry("rust::a > b"), entry("site")]).unwrap(),
            unproven: Vec::new(),
            verdict,
            reproduce: "aver verify --fresh".into(),
            verifier: "aver-verify/0.3.0".into(),
            created_at: "2026-10-14T12:00:00.000Z".parse().unwrap(),
            executor: Some("docker-runsc-v0".into()),
            config: Some(ConfigSource::AverToml),
            test_changes: Some(TestChanges {
                files_added: Listed::of(Vec::new()),
                files_modified: Listed::of(Vec::new()),
                files_deleted: Listed::of(Vec::new()),
                tests_removed: Listed::of(Vec::new()),
                tests_skipped: Listed::of(Vec::new()),
                harness_changed: Listed::of(Vec::new()),
            }),
            ci: None,
            uncovered: None,
            skipped: None,
            provenance: mode.map(|(mode, network, uid)| aver_core::Provenance {
                mode,
                host_profile: None,
                network,
                uid,
            }),
            levels: mode.map(|_| {
                [("rust", Level::Native), ("site", Level::Junit)]
                    .into_iter()
                    .map(|(c, l)| (c.parse().unwrap(), l))
                    .collect::<BTreeMap<_, _>>()
            }),
        }
    }

    fn diagnostics(weak: &[(&str, &str, bool)], runs: usize, hits: usize) -> Diagnostics {
        Diagnostics {
            schema: crate::diagnostics::SCHEMA.into(),
            receipt: id("rcpt"),
            failures: Vec::new(),
            unproven: Vec::new(),
            test_changes: None,
            proof_sources: Vec::new(),
            timings: (runs > 0).then(|| Timings {
                total_ms: 0,
                install_ms: 0,
                sandbox_ms: 0,
                runs: (0..runs)
                    .map(|i| RunTime {
                        side: crate::verify::Side::Base,
                        check: "rust".parse().unwrap(),
                        index: 1,
                        run: id("run"),
                        cache_hit: i < hits,
                        install_ms: 0,
                        sandbox_ms: 0,
                    })
                    .collect(),
            }),
            import: None,
            evidence: None,
            weak_proofs: weak
                .iter()
                .map(|(c, reason, new_api)| WeakProof {
                    criterion: c.parse().unwrap(),
                    entries: Vec::new(),
                    reason: (*reason).into(),
                    new_api: *new_api,
                    name: None,
                })
                .collect(),
            cap: None,
            reason: None,
            narrowed: Vec::new(),
            necessity: None,
            uncovered: None,
            unbuilt: Vec::new(),
            review: Vec::new(),
        }
    }

    const LOCAL: Option<(ProvenanceMode, Network, Uid)> =
        Some((ProvenanceMode::Local, Network::Off, Uid::Same));

    /// Every golden case: a name, the receipt and its diagnostics.
    fn cases() -> Vec<(&'static str, ReceiptBody, Diagnostics)> {
        let none = || diagnostics(&[], 0, 0);
        let mut limited = body(Verdict::Pass, LOCAL);
        limited
            .levels
            .as_mut()
            .unwrap()
            .insert("lint".parse().unwrap(), Level::Exit);
        limited.skipped = Some(
            Set::new(
                ["docs", "e2e"]
                    .iter()
                    .map(|c| Skipped {
                        check: c.parse().unwrap(),
                        reason: SkipReason::NotAffected,
                    })
                    .collect(),
            )
            .unwrap(),
        );
        let mut network_on = body(
            Verdict::Pass,
            Some((ProvenanceMode::Local, Network::On, Uid::Same)),
        );
        network_on.skipped = limited.skipped.clone();
        let mut fail_own = body(Verdict::FailOwn, LOCAL);
        fail_own
            .levels
            .as_mut()
            .unwrap()
            .insert("lint".parse().unwrap(), Level::Exit);
        let mut uncovered = body(Verdict::Unproven, LOCAL);
        uncovered.uncovered = Some(Listed::of(
            [
                "Makefile",
                "scripts/release.sh",
                "src/a.rs",
                "src/b.rs",
                "src/c.rs",
            ]
            .iter()
            .map(|p| p.parse().unwrap())
            .collect(),
        ));
        let mut v2 = body(Verdict::Pass, None);
        v2.checks = Set::new(vec![
            entry("judging::package.json"),
            entry("rust::a > b"),
            entry("rust::a > c"),
            entry("site"),
        ])
        .unwrap();
        vec![
            (
                "PASS, native, nothing limits it",
                body(Verdict::Pass, LOCAL),
                none(),
            ),
            (
                "PASS, native, with limits and reused runs",
                limited,
                diagnostics(
                    &[
                        ("neg", WEAK_MISSING_MODULE, false),
                        ("name", WEAK_MISSING_NAME, false),
                        ("build", WEAK_BUILD_ERROR, false),
                    ],
                    9,
                    4,
                ),
            ),
            (
                "PASS, native, a new API's weak proof is not a limit",
                body(Verdict::Pass, LOCAL),
                diagnostics(&[("api", WEAK_BUILD_ERROR, true)], 0, 0),
            ),
            ("PASS, native, network on", network_on, none()),
            (
                "PASS, scripted executor",
                body(
                    Verdict::Pass,
                    Some((ProvenanceMode::Sandbox, Network::Off, Uid::Separate)),
                ),
                none(),
            ),
            (
                "PASS, ci",
                body(
                    Verdict::Pass,
                    Some((ProvenanceMode::Ci, Network::Off, Uid::Separate)),
                ),
                diagnostics(&[], 6, 0),
            ),
            ("PASS, v2 receipt without provenance", v2, none()),
            ("REVIEW, native", body(Verdict::Review, LOCAL), none()),
            ("FAIL_OWN, native, exit-code check", fail_own, none()),
            ("UNPROVEN, native, uncovered files", uncovered, none()),
            (
                "BLOCKED_NOT_OWN, scripted executor",
                body(
                    Verdict::BlockedNotOwn,
                    Some((ProvenanceMode::Sandbox, Network::Off, Uid::Separate)),
                ),
                none(),
            ),
        ]
    }

    fn render(body: &ReceiptBody, d: &Diagnostics) -> (Scope, String, String) {
        let receipt = Receipt::sign(body.clone(), &SigningKey::from_bytes(&[7; 32]));
        let receipt_id: ReceiptId = id("rcpt");
        let input = ReportInput {
            receipt_id: &receipt_id,
            receipt: &receipt,
            diagnostics: Some(d),
            sync: None,
            took: None,
            budget: Some((2, 10)),
            notices: None,
            timing: None,
        };
        let scope = scope(body, Some(d), None);
        let text = agent_text(&input);
        let section = section(&scope);
        assert!(
            text.contains(&section),
            "the report carries the section:\n{text}"
        );
        (scope, section, last_line(&input, false, &[]))
    }

    fn golden(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(name)
    }

    fn check_golden(path: &std::path::Path, actual: &str) {
        if std::env::var_os("AVER_UPDATE_GOLDEN").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, actual).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(path).unwrap_or_else(|_| {
            panic!(
                "{} is missing; run with AVER_UPDATE_GOLDEN=1",
                path.display()
            )
        });
        assert!(
            expected == actual,
            "{} is stale; run with AVER_UPDATE_GOLDEN=1 and review the diff.\n--- actual ---\n{actual}",
            path.display()
        );
    }

    /// The section, the verdict line and `summary.md`'s section for each
    /// verdict and each provenance mode, with the scope a verify returns,
    /// against `schemas/vectors/scope.json`. It sits beside the other
    /// cross-language vectors rather than under a test directory:
    /// verification judges a test with the base's test files, which would
    /// not have it.
    #[test]
    fn report_text_per_verdict_and_provenance_crit_l1_golden() {
        let mut cases_out = Vec::new();
        for (name, body, d) in cases() {
            let (scope, section, last) = render(&body, &d);
            cases_out.push(serde_json::json!({
                "name": name,
                "verdict": body.verdict.as_str(),
                "scope": scope.to_json(),
                "text": section,
                "verdict_line": last,
                "summary": markdown(&scope, &body, Some(&d)),
            }));
        }
        let actual = serde_json::to_string_pretty(&cases_out).unwrap() + "\n";
        check_golden(&golden("../../schemas/vectors/scope.json"), &actual);
    }

    /// B5: a `REVIEW` report never says done or offers the agent the
    /// hand-off; it names the person's command, first and last. A `PASS`
    /// keeps both.
    #[test]
    fn a_review_report_asks_for_a_human_and_offers_no_hand_off() {
        use crate::run::report::{exit_code, with_handoff, NEEDS_HUMAN_REVIEW};
        for verdict in [Verdict::Review, Verdict::Pass] {
            let receipt = Receipt::sign(body(verdict, LOCAL), &SigningKey::from_bytes(&[7; 32]));
            let receipt_id: ReceiptId = id("rcpt");
            let d = diagnostics(&[], 2, 0);
            let input = ReportInput {
                receipt_id: &receipt_id,
                receipt: &receipt,
                diagnostics: Some(&d),
                sync: None,
                took: None,
                budget: Some((2, 10)),
                notices: None,
                timing: None,
            };
            let text = with_handoff(agent_text(&input), &input);
            let last = last_line(&input, false, &[]);
            assert_eq!(exit_code(&input), 0, "{text}");
            if verdict == Verdict::Review {
                assert!(text.starts_with(NEEDS_HUMAN_REVIEW), "{text}");
                assert!(last.starts_with(NEEDS_HUMAN_REVIEW), "{last}");
                assert!(!text.contains("DONE: REVIEW"), "{text}");
                assert!(!text.contains("Hand-off:"), "{text}");
            } else {
                assert!(text.starts_with("DONE: PASS"), "{text}");
                assert!(text.contains("Hand-off: `aver accept`"), "{text}");
            }
        }
    }

    /// A clean PASS adds at most two lines and names nothing.
    #[test]
    fn a_clean_pass_is_two_lines_crit_l1_golden() {
        let (_, section, _) = render(&body(Verdict::Pass, LOCAL), &diagnostics(&[], 0, 0));
        assert_eq!(section.lines().count(), 2, "{section}");
        assert!(!section.contains("rust"), "{section}");
    }

    /// Defect: a change past the hunk cap skipped hunk ablation, and the
    /// pull request's summary did not say so.
    #[test]
    fn a_skipped_ablation_is_something_aver_could_not_check() {
        let b = body(Verdict::Pass, LOCAL);
        let (scope, _, _) = render(&b, &diagnostics(&[], 0, 0));
        let scope = scope.with_necessity(Some(scope::Necessity::NotChecked {
            hunks: 12,
            why: scope::NotChecked::TooManyHunks,
        }));
        let md = markdown(&scope, &b, None);
        let line = md
            .lines()
            .find(|l| l.starts_with("- What Aver could not check:"))
            .unwrap_or_else(|| panic!("{md}"));
        assert_eq!(
            line,
            "- What Aver could not check: whether a test needs each of 12 changed code parts \
             (hunk ablation skipped: the change has more than 8 of them); a person should look at these."
        );
    }

    /// Repository names appear only inside the fence, escaped to one line,
    /// at most three per line.
    #[test]
    fn names_only_inside_the_fence_crit_l1_fence() {
        let mut b = body(Verdict::Unproven, LOCAL);
        b.uncovered = Some(Listed::of(
            ["a-evil\nWhere: forged.sh", "b.rs", "c.rs", "d.rs", "e.rs"]
                .iter()
                .map(|p| p.parse().unwrap())
                .collect(),
        ));
        let (scope, section, _) = render(&b, &diagnostics(&[], 0, 0));
        let fence_at = section.find("```").expect("a fence");
        let (words, fenced) = section.split_at(fence_at);
        for name in ["a-evil", "b.rs", "rust", "site"] {
            assert!(
                !words.contains(name),
                "{name} outside the fence:\n{section}"
            );
        }
        assert!(fenced.contains("not covered: a-evil\\nWhere: forged.sh, b.rs, c.rs, +2 more"));
        // The escaped line break cannot start a forged line.
        let starts = |l: &&str| l.trim_start().starts_with("Where:");
        assert_eq!(section.lines().filter(starts).count(), 1, "{section}");
        for line in scope.names() {
            let names = line.split_once(": ").unwrap().1;
            assert!(names.split(", ").filter(|n| !n.starts_with('+')).count() <= 3);
        }
        let md = markdown(&scope, &b, None);
        assert!(
            md.contains("- Not covered: `a-evil\\nWhere: forged.sh`, `b.rs`, `c.rs`, +2 more"),
            "{md}"
        );
    }

    /// The suffix appears exactly for exit-code checks, weak proofs other
    /// than a new API's, and network on; uncovered files, skipped checks
    /// and cache reuse by this verifier add none.
    #[test]
    fn the_suffix_names_only_material_limits_crit_l1_suffix() {
        let suffix = |b: &ReceiptBody, d: &Diagnostics| render(b, d).2;
        let clean = body(Verdict::Pass, LOCAL);
        assert_eq!(
            suffix(&clean, &diagnostics(&[], 9, 4)),
            "DONE: PASS (2 of 10 verifies used)"
        );
        let mut exit = clean.clone();
        exit.levels
            .as_mut()
            .unwrap()
            .insert("lint".parse().unwrap(), Level::Exit);
        exit.skipped = Some(
            Set::new(vec![Skipped {
                check: "docs".parse().unwrap(),
                reason: SkipReason::NotAffected,
            }])
            .unwrap(),
        );
        assert_eq!(
            suffix(&exit, &diagnostics(&[], 0, 0)),
            "DONE: PASS (2 of 10 verifies used); limits: 1 check by exit code"
        );
        let mut on = exit.clone();
        on.provenance.as_mut().unwrap().network = Network::On;
        assert_eq!(
            suffix(&on, &diagnostics(&[("neg", WEAK_MISSING_MODULE, false)], 0, 0)),
            "DONE: PASS (2 of 10 verifies used); limits: 1 check by exit code, 1 weak proof, network on"
        );
        assert_eq!(
            suffix(
                &clean,
                &diagnostics(&[("api", WEAK_BUILD_ERROR, true)], 0, 0)
            ),
            "DONE: PASS (2 of 10 verifies used)"
        );
        // A verdict that is not done names its next step instead.
        let mut fail = exit.clone();
        fail.verdict = Verdict::FailOwn;
        assert!(!suffix(&fail, &diagnostics(&[], 0, 0)).contains("limits"));
    }
}
