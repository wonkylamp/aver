use aver_core::judge::{self, judge, Base, Judged, Origin, Own};
use aver_core::rules::{
    base_needs_confirmation, failure_settled, inconclusive, needs_rerun, unproven_reason, verdict,
    Classification, Failure, UnprovenReason, MAX_RUNS,
};
use aver_core::*;

use BaseStatus as B;
use Ending::{Cap as C, Fail as F, Pass as P, Sandbox as I};
use HeadStatus as H;

/// The table's classification of a test in a file the head left alone,
/// from each side's runs.
fn classify(base: &[Ending], head: &[Ending]) -> Classification {
    judge(judge::Entry {
        origin: Origin::UntouchedTestFile,
        base: Base::of(base),
        judged: Judged::of(head),
        own: Own::NotRun,
        transplant: judge::Transplant::NotRun,
    })
}

#[test]
fn rerun_policy_table() {
    let cases: &[(&[Ending], bool)] = &[
        (&[], true),
        (&[P], false),
        (&[F], true),
        (&[F, F], true),
        (&[F, F, F], false),
        (&[F, P], false),
        (&[F, F, P], false),
        (&[I], true),
        (&[I, I], false),
        (&[I, P], false),
        (&[I, F], true),
        (&[F, I], true),
        (&[F, I, I], false),
        (&[F, I, F], true),
        (&[F, I, F, F], false),
        (&[F, F, I], true),
        (&[F, F, I, I], false),
        (&[F, F, I, F], false),
        (&[F, F, I, P], false),
        (&[I, F, I, F, I, F], false),
        (&[C], true),
        (&[C, C], false),
        (&[C, I], false),
        (&[I, C], false),
        (&[C, P], false),
        (&[F, C, F], true),
    ];
    for (runs, expected) in cases {
        assert_eq!(
            needs_rerun(runs, MAX_RUNS),
            *expected,
            "needs_rerun({runs:?})"
        );
    }
}

/// Defect: the transplant reruns a proving test's expected failure three
/// times, tripling the slowest side for nothing.
#[test]
fn a_one_run_side_never_reruns_a_failure_but_still_retries_infra() {
    let cases: &[(&[Ending], bool)] = &[
        (&[], true),
        (&[P], false),
        (&[F], false),
        (&[I], true),
        (&[I, F], false),
        (&[I, I], false),
        (&[C, P], false),
    ];
    for (runs, expected) in cases {
        assert_eq!(needs_rerun(runs, 1), *expected, "needs_rerun({runs:?}, 1)");
    }
}

/// Defect: a base that cannot build, or that fails the same way every
/// time, runs three times before the head is judged.
#[test]
fn a_build_error_or_a_repeated_command_failure_settles_the_base() {
    let out = |s: &str| BlobId::of_bytes(s.as_bytes());
    let exit = |code, text| Failure::Command {
        exit_code: code,
        output: out(text),
    };
    let cases: &[(&[Failure], bool)] = &[
        (&[], false),
        (&[Failure::Build], true),
        (&[exit(1, "boom")], false),
        (&[exit(1, "boom"), exit(1, "boom")], true),
        (&[exit(1, "boom"), exit(2, "boom")], false),
        (&[exit(1, "boom at 10:01"), exit(1, "boom at 10:02")], false),
        (&[Failure::Tests], false),
        (&[Failure::Tests, Failure::Tests], false),
        (&[exit(1, "boom"), Failure::Build], true),
    ];
    for (failures, expected) in cases {
        assert_eq!(
            failure_settled(failures),
            *expected,
            "failure_settled({failures:?})"
        );
    }
}

fn reachable_sequences() -> Vec<Vec<Ending>> {
    let mut done = Vec::new();
    let mut open = vec![vec![]];
    while let Some(runs) = open.pop() {
        if needs_rerun(&runs, MAX_RUNS) {
            for next in [P, F, I, C] {
                let mut longer = runs.clone();
                longer.push(next);
                open.push(longer);
            }
        } else {
            done.push(runs);
        }
    }
    done
}

#[test]
fn rerun_policy_always_terminates_within_bounds() {
    let sequences = reachable_sequences();
    assert!(!sequences.is_empty());
    for runs in &sequences {
        let results = runs.iter().filter(|o| !o.is_infra()).count();
        assert!(results <= MAX_RUNS, "{runs:?} has more than three results");
        assert!(
            runs.len() <= 2 * MAX_RUNS,
            "{runs:?} retried infra more than once per run"
        );
        assert!(
            !runs.windows(3).any(|w| w.iter().all(|r| r.is_infra())),
            "{runs:?} retried infra twice in a row"
        );
        let stable_failure = results == MAX_RUNS && !runs.contains(&P);
        let ended_infra = runs.len() >= 2 && runs[runs.len() - 2..].iter().all(|r| r.is_infra());
        let ended = matches!(runs.last(), Some(P))
            || ended_infra
            || (runs.contains(&P) && runs.contains(&F))
            || stable_failure;
        assert!(ended, "{runs:?} stopped without a terminal condition");
    }
}

/// One representative run sequence per side state, plus equivalent sequences
/// that reach the same state through infra retries. `cap` is a side whose
/// every run hit a resource cap; `env` ended in infra any other way.
fn sides() -> [(&'static str, Vec<Vec<Ending>>); 6] {
    [
        ("absent", vec![vec![]]),
        ("pass", vec![vec![P], vec![I, P]]),
        (
            "fail",
            vec![vec![F, F, F], vec![I, F, F, F], vec![F, I, F, I, F]],
        ),
        (
            "flaky",
            vec![vec![F, P], vec![F, F, P], vec![F, I, P], vec![C, F, P]],
        ),
        (
            "env",
            vec![
                vec![I, I],
                vec![F, I, I],
                vec![F, F, I, I],
                vec![C, I],
                vec![I, C],
                vec![F, C, C],
            ],
        ),
        ("cap", vec![vec![C, C]]),
    ]
}

fn expected(base: &str, head: &str) -> Classification {
    let c = |base, head, cause| Classification { base, head, cause };
    use Cause::*;
    match (base, head) {
        ("absent", "pass") => c(B::NotApplicable, H::Pass, None),
        ("absent", "fail") => c(B::NotApplicable, H::Fail, Some(New)),
        // An added test the head fails some of the time is judged failing.
        ("absent", "flaky") => c(B::NotApplicable, H::Flaky, Some(New)),
        ("absent", "env" | "cap") => c(B::NotApplicable, H::Fail, Some(Env)),
        ("absent", "absent") => c(B::NotApplicable, H::Fail, Some(New)),

        ("pass", "pass") => c(B::Pass, H::Pass, None),
        ("pass", "fail") => c(B::Pass, H::Fail, Some(New)),
        // Stable on the base, failing on some head runs: the change's.
        ("pass", "flaky") => c(B::Pass, H::Flaky, Some(New)),
        ("pass", "env") => c(B::Pass, H::Fail, Some(Env)),
        ("pass", "cap") => c(B::Pass, H::Fail, Some(New)),
        ("pass", "absent") => c(B::Pass, H::Fail, Some(New)),

        ("fail", "pass") => c(B::Fail, H::Pass, None),
        ("fail", "fail") => c(B::Fail, H::Fail, Some(Preexisting)),
        ("fail", "flaky") => c(B::Fail, H::Flaky, Some(Flaky)),
        ("fail", "env") => c(B::Fail, H::Fail, Some(Env)),
        // The base finished (red); the head ran out of time or memory.
        ("fail", "cap") => c(B::Fail, H::Fail, Some(New)),
        ("fail", "absent") => c(B::Fail, H::Fail, Some(New)),

        ("flaky", "pass") => c(B::Flaky, H::Pass, Some(Flaky)),
        ("flaky", "fail") => c(B::Flaky, H::Fail, Some(Flaky)),
        ("flaky", "flaky") => c(B::Flaky, H::Flaky, Some(Flaky)),
        ("flaky", "env") => c(B::Flaky, H::Fail, Some(Env)),
        ("flaky", "cap") => c(B::Flaky, H::Fail, Some(New)),
        ("flaky", "absent") => c(B::Flaky, H::Fail, Some(New)),

        ("env" | "cap", "pass") => c(B::Fail, H::Pass, Some(Env)),
        ("env" | "cap", "fail") => c(B::Fail, H::Fail, Some(Env)),
        ("env" | "cap", "flaky") => c(B::Fail, H::Flaky, Some(Env)),
        ("env" | "cap", "env" | "cap") => c(B::Fail, H::Fail, Some(Env)),
        ("env" | "cap", "absent") => c(B::Fail, H::Fail, Some(Env)),
        _ => unreachable!(),
    }
}

#[test]
fn cause_rules_table_is_exhaustive_over_side_states() {
    let mut checked = 0;
    for (base_label, base_variants) in sides() {
        for (head_label, head_variants) in sides() {
            let want = expected(base_label, head_label);
            for base in &base_variants {
                for head in &head_variants {
                    assert_eq!(
                        classify(base, head),
                        want,
                        "base {base_label} {base:?}, head {head_label} {head:?}"
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 25);
}

#[test]
fn every_reachable_pair_classifies_to_a_legal_entry() {
    let sequences = reachable_sequences();
    for base in &sequences {
        for head in &sequences {
            let c = classify(base, head);
            let ended = |runs: &[Ending]| runs.last().is_some_and(|r| r.is_infra());
            let completed = base.iter().any(|r| !r.is_infra());
            let own_exhaustion =
                !head.is_empty() && head.iter().all(|r| *r == C) && completed && !ended(base);
            assert_eq!(
                c.cause == Some(Cause::Env),
                (ended(base) || ended(head)) && !own_exhaustion,
                "{base:?} {head:?}"
            );
            if own_exhaustion {
                assert_eq!(c.cause, Some(Cause::New), "{base:?} {head:?}");
            }
            if c.cause.is_none() {
                assert_eq!(c.head, H::Pass, "only a passing head has no cause");
            }
        }
    }
}

fn entry(id: &str, head: HeadStatus, cause: Option<Cause>, proves: &[&str]) -> ReceiptCheck {
    ReceiptCheck {
        id: id.parse().unwrap(),
        base: B::Pass,
        head,
        cause,
        proves: Set::new(proves.iter().map(|p| p.parse().unwrap()).collect()).unwrap(),
        source: CheckSource::Task,
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

/// [`entry`] with a transplant result and a passing head-tests run, as a
/// v2 proving entry carries.
fn moved(
    id: &str,
    head: HeadStatus,
    cause: Option<Cause>,
    proves: &[&str],
    transplant: Transplant,
) -> ReceiptCheck {
    with_head_tests(entry(id, head, cause, proves), transplant, Transplant::Pass)
}

fn with_head_tests(
    mut e: ReceiptCheck,
    transplant: Transplant,
    head_tests: Transplant,
) -> ReceiptCheck {
    e.transplant = Some(transplant);
    e.head_tests = Some(head_tests);
    e
}

fn criteria(ids: &[&str]) -> Criteria {
    Criteria::new(
        ids.iter()
            .map(|id| Criterion {
                id: id.parse().unwrap(),
                text: String::new(),
            })
            .collect(),
    )
    .unwrap()
}

#[test]
fn verdict_table() {
    use Cause::*;
    let pass = |id: &str, proves: &[&str]| entry(id, H::Pass, None, proves);
    let ab = criteria(&["a", "b"]);
    let cases: Vec<(&str, Vec<ReceiptCheck>, Vec<&str>, Verdict)> = vec![
        (
            "all proven",
            vec![pass("t1", &["a"]), pass("t2", &["b"])],
            vec![],
            Verdict::Pass,
        ),
        (
            "one entry proves both",
            vec![pass("t1", &["a", "b"])],
            vec![],
            Verdict::Pass,
        ),
        (
            "NEW wins over everything",
            vec![
                pass("t1", &["a", "b"]),
                entry("t2", H::Fail, Some(Env), &[]),
                entry("t3", H::Fail, Some(New), &[]),
            ],
            vec![],
            Verdict::FailOwn,
        ),
        (
            "ENV blocks even when all proven",
            vec![
                pass("t1", &["a", "b"]),
                entry("t2", H::Fail, Some(Env), &[]),
            ],
            vec![],
            Verdict::BlockedNotOwn,
        ),
        (
            "no prover for b",
            vec![pass("t1", &["a"])],
            vec!["b"],
            Verdict::Unproven,
        ),
        (
            "no entries at all",
            vec![],
            vec!["a", "b"],
            Verdict::Unproven,
        ),
        (
            "b's only prover is flaky",
            vec![
                pass("t1", &["a"]),
                entry("t2", H::Flaky, Some(Flaky), &["b"]),
            ],
            vec!["b"],
            Verdict::BlockedNotOwn,
        ),
        (
            "head passes but base was flaky",
            vec![
                pass("t1", &["a"]),
                entry("t2", H::Pass, Some(Flaky), &["b"]),
            ],
            vec!["b"],
            Verdict::BlockedNotOwn,
        ),
        (
            "b's provers are flaky and preexisting",
            vec![
                pass("t1", &["a"]),
                entry("t2", H::Flaky, Some(Flaky), &["b"]),
                entry("t3", H::Fail, Some(Preexisting), &["b"]),
            ],
            vec!["b"],
            Verdict::Unproven,
        ),
        (
            "preexisting failure that proves nothing does not block",
            vec![
                pass("t1", &["a", "b"]),
                entry("t2", H::Fail, Some(Preexisting), &[]),
            ],
            vec![],
            Verdict::Pass,
        ),
        (
            "one passing prover is enough",
            vec![
                pass("t1", &["a"]),
                entry("t2", H::Fail, Some(Preexisting), &["b"]),
                pass("t3", &["b"]),
            ],
            vec![],
            Verdict::Pass,
        ),
        (
            "flaky entries elsewhere do not block a fully proven task",
            vec![
                pass("t1", &["a", "b"]),
                entry("t2", H::Flaky, Some(Flaky), &[]),
            ],
            vec![],
            Verdict::Pass,
        ),
    ];
    for (name, entries, unproven, want) in cases {
        let entries = Set::new(entries).unwrap();
        let (got_unproven, got) = verdict(entries.as_slice(), &ab, ProofRule::HeadPasses);
        let got_unproven: Vec<&str> = got_unproven.iter().map(|c| c.as_str()).collect();
        assert_eq!((got_unproven, got), (unproven, want), "{name}");
    }
}

#[test]
fn an_inconclusive_entry_blocks_a_v2_verdict() {
    use Cause::*;
    let none = criteria(&[]);
    let flaky_base = |head| {
        let mut e = entry("unit::slow", head, Some(Flaky), &[]);
        e.base = B::Flaky;
        e
    };
    let cases: Vec<(&str, Vec<ReceiptCheck>, Verdict, Verdict)> = vec![
        (
            "a command-level check red on both sides hides the change",
            vec![entry("test", H::Fail, Some(Preexisting), &[])],
            Verdict::BlockedNotOwn,
            Verdict::Pass,
        ),
        (
            "a per-test pre-existing failure does not block",
            vec![
                entry("unit::legacy", H::Fail, Some(Preexisting), &[]),
                entry("unit::adds", H::Pass, None, &[]),
            ],
            Verdict::Pass,
            Verdict::Pass,
        ),
        (
            "an exit entry red on both sides does not block",
            vec![entry("unit::(exit)", H::Fail, Some(Preexisting), &[])],
            Verdict::Pass,
            Verdict::Pass,
        ),
        (
            "flaky on the base and failing on every head run blocks",
            vec![flaky_base(H::Fail)],
            Verdict::BlockedNotOwn,
            Verdict::Pass,
        ),
        (
            "flaky on both sides does not block",
            vec![flaky_base(H::Flaky)],
            Verdict::Pass,
            Verdict::Pass,
        ),
        (
            "NEW still wins",
            vec![
                entry("test", H::Fail, Some(Preexisting), &[]),
                entry("lint", H::Fail, Some(New), &[]),
            ],
            Verdict::FailOwn,
            Verdict::FailOwn,
        ),
    ];
    for (name, entries, v2, v1) in cases {
        let entries = Set::new(entries).unwrap();
        let (_, got) = verdict(entries.as_slice(), &none, ProofRule::Transplant);
        assert_eq!(got, v2, "v2: {name}");
        let (_, got) = verdict(entries.as_slice(), &none, ProofRule::HeadPasses);
        assert_eq!(got, v1, "v0 and v1 keep their meaning: {name}");
    }
}

/// A test the change breaks (base pass, head fail) whose file the head
/// edited to skip or delete it: no own version passes, so there is no new
/// expectation to review and the base's copy failing is the change's
/// regression, `NEW` and `FAIL_OWN`. Before, `absent` counted as a passing
/// own version and the entry was `CHANGED` (`REVIEW`, exit 0 in CI).
#[test]
fn a_skipped_or_deleted_broken_test_stays_new_crit_ci_trust_skip_test() {
    let broken = |own| {
        judge(judge::Entry {
            origin: Origin::ModifiedTestFile,
            base: Base::Pass,
            judged: Judged::Fail,
            own,
            transplant: judge::Transplant::NotRun,
        })
    };
    for own in [Own::Skipped, Own::Absent] {
        let gone = broken(own);
        assert_eq!(gone.cause, Some(Cause::New), "{own:?}");
        let set = Set::new(vec![ReceiptCheck {
            base: gone.base,
            ..entry("unit::add", gone.head, gone.cause, &[])
        }])
        .unwrap();
        let (_, v) = verdict(set.as_slice(), &criteria(&[]), ProofRule::Transplant);
        assert_eq!(v, Verdict::FailOwn);
    }
    // A rewritten expectation whose own version passes is still REVIEW.
    assert_eq!(broken(Own::Pass).cause, Some(Cause::Changed));
}

/// The verdict with `CHANGED` entries: `REVIEW` exactly when nothing is
/// `NEW`, `ENV` or inconclusive, nothing is unproven, and some entry is
/// `CHANGED`; `UNPROVEN` outranks it.
#[test]
fn review_verdict_table() {
    use Cause::*;
    let a = criteria(&["a"]);
    let none = criteria(&[]);
    let proving = moved("p", H::Pass, None, &["a"], Transplant::Fail);
    let cases: Vec<(&str, Vec<ReceiptCheck>, &Criteria, Verdict)> = vec![
        (
            "a changed test alone",
            vec![entry("unit::t", H::Fail, Some(Changed), &[])],
            &none,
            Verdict::Review,
        ),
        (
            "a changed harness file alone",
            vec![entry(
                "judging::jest.config.js",
                H::Fail,
                Some(Changed),
                &[],
            )],
            &none,
            Verdict::Review,
        ),
        (
            "NEW outranks CHANGED",
            vec![
                entry("unit::t", H::Fail, Some(Changed), &[]),
                entry("unit::u", H::Fail, Some(New), &[]),
            ],
            &none,
            Verdict::FailOwn,
        ),
        (
            "ENV outranks CHANGED",
            vec![
                entry("unit::t", H::Fail, Some(Changed), &[]),
                entry("unit::u", H::Fail, Some(Env), &[]),
            ],
            &none,
            Verdict::BlockedNotOwn,
        ),
        (
            "an inconclusive entry outranks CHANGED",
            vec![
                entry("unit::t", H::Fail, Some(Changed), &[]),
                entry("test", H::Fail, Some(Preexisting), &[]),
            ],
            &none,
            Verdict::BlockedNotOwn,
        ),
        (
            "every criterion proven and a changed test",
            vec![
                proving.clone(),
                entry("unit::t", H::Fail, Some(Changed), &[]),
            ],
            &a,
            Verdict::Review,
        ),
        (
            "UNPROVEN outranks CHANGED",
            vec![entry("unit::t", H::Fail, Some(Changed), &[])],
            &a,
            Verdict::Unproven,
        ),
        (
            "a changed harness file proves nothing, so UNPROVEN",
            vec![
                proving.clone(),
                entry("judging::package.json#scripts", H::Fail, Some(Changed), &[]),
            ],
            &a,
            Verdict::Unproven,
        ),
        (
            "per-test PREEXISTING and FLAKY do not hide a review",
            vec![
                entry("unit::t", H::Fail, Some(Changed), &[]),
                entry("unit::old", H::Fail, Some(Preexisting), &[]),
                entry("unit::slow", H::Flaky, Some(Flaky), &[]),
            ],
            &none,
            Verdict::Review,
        ),
        ("nothing changed", vec![proving], &a, Verdict::Pass),
    ];
    for (name, entries, criteria, want) in cases {
        let entries = Set::new(entries).unwrap();
        let (_, got) = verdict(entries.as_slice(), criteria, ProofRule::Transplant);
        assert_eq!(got, want, "{name}");
    }
}

/// The reviewer's c2 scenario on fixtures/calc: `divide` was unimplemented,
/// so both of its tests failed on the base. The head implements it as
/// `a / b` and rewrites the zero-division test to expect `Infinity`. With
/// the base's copy, `div-basic` passes on the head (fixed) and `div-zero`
/// still fails (it expects a throw); the head's own `div-zero` passes. That
/// test was rewritten, not fixed, so it is `CHANGED` and the verdict
/// `REVIEW`, never `PASS`. Without the rewrite it stays `PREEXISTING`.
#[test]
fn rewriting_an_already_failing_test_is_review() {
    let fixed = classify(&[F, F, F], &[P]);
    assert_eq!(fixed.cause, None);
    let zero = classify(&[F, F, F], &[F]);
    assert_eq!(zero.cause, Some(Cause::Preexisting));
    let edited = |origin, own| {
        judge(judge::Entry {
            origin,
            base: Base::Fail,
            judged: Judged::Fail,
            own,
            transplant: judge::Transplant::NotRun,
        })
    };
    let rewritten = edited(Origin::ModifiedTestFile, Own::Pass);
    assert_eq!(
        rewritten,
        Classification {
            base: B::Fail,
            head: H::Fail,
            cause: Some(Cause::Changed)
        }
    );
    let as_entry = |id: &str, c: Classification| ReceiptCheck {
        base: c.base,
        ..entry(id, c.head, c.cause, &[])
    };
    let none = criteria(&[]);
    let gamed = Set::new(vec![
        as_entry("unit::div-basic", fixed),
        as_entry("unit::div-zero", rewritten),
    ])
    .unwrap();
    let (_, v) = verdict(gamed.as_slice(), &none, ProofRule::Transplant);
    assert_eq!(v, Verdict::Review);
    // Left alone (or edited elsewhere in its file), it is still failing.
    for (origin, own) in [
        (Origin::UntouchedTestFile, Own::Pass),
        (Origin::ModifiedTestFile, Own::Fail),
    ] {
        let honest = Set::new(vec![
            as_entry("unit::div-basic", fixed),
            as_entry("unit::div-zero", edited(origin, own)),
        ])
        .unwrap();
        let (_, v) = verdict(honest.as_slice(), &none, ProofRule::Transplant);
        assert_eq!(v, Verdict::Pass, "{origin:?} {own:?}");
    }
}

#[test]
fn unproven_follows_criteria_order() {
    let (unproven, v) = verdict(&[], &criteria(&["c", "a", "b"]), ProofRule::Transplant);
    let ids: Vec<&str> = unproven.iter().map(|c| c.as_str()).collect();
    assert_eq!(ids, ["c", "a", "b"]);
    assert_eq!(v, Verdict::Unproven);
}

#[test]
fn transplant_verdict_table() {
    use Cause::*;
    use Transplant::{Absent, Fail as TFail, Infra, Pass as TPass};
    let a = criteria(&["a"]);
    let cases: Vec<(&str, Vec<ReceiptCheck>, bool, Verdict)> = vec![
        (
            "fails on the transplant, passes at head",
            vec![moved("t1", H::Pass, None, &["a"], TFail)],
            true,
            Verdict::Pass,
        ),
        (
            "absent from the transplant counts as failing there",
            vec![moved("t1", H::Pass, None, &["a"], Absent)],
            true,
            Verdict::Pass,
        ),
        (
            "passes on the transplant: proves nothing",
            vec![moved("t1", H::Pass, None, &["a"], TPass)],
            false,
            Verdict::Unproven,
        ),
        (
            "no transplant result: proves nothing in v2",
            vec![entry("t1", H::Pass, None, &["a"])],
            false,
            Verdict::Unproven,
        ),
        (
            "one proving entry is enough",
            vec![
                moved("t1", H::Pass, None, &["a"], TPass),
                moved("t2", H::Pass, None, &["a"], TFail),
            ],
            true,
            Verdict::Pass,
        ),
        (
            "a judging change proves nothing and fails",
            vec![
                moved("t1", H::Pass, None, &["a"], TFail),
                entry("judging::package.json", H::Fail, Some(New), &[]),
            ],
            false,
            Verdict::FailOwn,
        ),
        (
            "a transplant that ended in infra is ENV",
            vec![moved(
                "t1",
                H::Pass,
                judge(judge::Entry {
                    origin: Origin::UntouchedTestFile,
                    base: Base::Fail,
                    judged: Judged::Pass,
                    own: Own::Pass,
                    transplant: judge::Transplant::Infra,
                })
                .cause,
                &["a"],
                Infra,
            )],
            false,
            Verdict::BlockedNotOwn,
        ),
        (
            "fails in the head's version of the tests: proves nothing",
            vec![with_head_tests(
                entry("t1", H::Pass, None, &["a"]),
                TFail,
                TFail,
            )],
            false,
            Verdict::Unproven,
        ),
        (
            "absent from the head's version of the tests: proves nothing",
            vec![with_head_tests(
                entry("t1", H::Pass, None, &["a"]),
                Absent,
                Absent,
            )],
            false,
            Verdict::Unproven,
        ),
        (
            "a head-tests run that ended in infra is ENV",
            vec![with_head_tests(
                entry(
                    "t1",
                    H::Pass,
                    judge(judge::Entry {
                        origin: Origin::AddedTestFile,
                        base: Base::Absent,
                        judged: Judged::Pass,
                        own: Own::Infra,
                        transplant: judge::Transplant::FailOnAssertion,
                    })
                    .cause,
                    &["a"],
                ),
                TFail,
                Infra,
            )],
            false,
            Verdict::BlockedNotOwn,
        ),
    ];
    for (name, entries, proven, want) in cases {
        let entries = Set::new(entries).unwrap();
        let (unproven, got) = verdict(entries.as_slice(), &a, ProofRule::Transplant);
        assert_eq!((unproven.is_empty(), got), (proven, want), "{name}");
    }
}

/// Every proving entry a verification can produce: each side state pair,
/// judged, with each transplant and head-tests result (or neither).
fn prover_states() -> Vec<(Classification, Option<(Transplant, Transplant)>)> {
    let results = [
        Transplant::Pass,
        Transplant::Fail,
        Transplant::Absent,
        Transplant::Infra,
    ];
    let transplant = |t| match t {
        Transplant::Pass => judge::Transplant::Pass,
        Transplant::Fail | Transplant::Absent => judge::Transplant::FailOnAssertion,
        Transplant::Infra => judge::Transplant::Infra,
    };
    let own = |h| match h {
        Transplant::Pass => Own::Pass,
        Transplant::Fail => Own::Fail,
        Transplant::Absent => Own::Absent,
        Transplant::Infra => Own::Infra,
    };
    let mut out = Vec::new();
    for (_, base) in sides() {
        for (_, head) in sides() {
            let e = judge::Entry {
                origin: Origin::UntouchedTestFile,
                base: Base::of(&base[0]),
                judged: Judged::of(&head[0]),
                own: Own::NotRun,
                transplant: judge::Transplant::NotRun,
            };
            out.push((judge(e), None));
            let rewritten = judge::Entry {
                origin: Origin::ModifiedTestFile,
                own: Own::Pass,
                ..e
            };
            out.push((judge(rewritten), None));
            for t in results {
                for h in results {
                    let proved = judge::Entry {
                        own: own(h),
                        transplant: transplant(t),
                        ..e
                    };
                    out.push((judge(proved), Some((t, h))));
                }
            }
        }
    }
    out.sort_by_key(|(c, t)| format!("{c:?}{t:?}"));
    out.dedup();
    out
}

/// Diagnostics' unproven reasons and the verdict rules come from separate
/// functions; over every receipt of up to two proving entries, an optional
/// judging-file entry and an optional unrelated entry, they never disagree.
#[test]
fn unproven_reasons_never_disagree_with_the_verdict() {
    use Cause::*;
    let a = criteria(&["a"]);
    let states = prover_states();
    let prover = |i: usize, (c, t): (Classification, Option<(Transplant, Transplant)>)| {
        let mut e = entry(&format!("p{i}"), c.head, c.cause, &["a"]);
        e.base = c.base;
        e.transplant = t.map(|t| t.0);
        e.head_tests = t.map(|t| t.1);
        e
    };
    let mut prover_sets: Vec<Vec<ReceiptCheck>> = vec![vec![]];
    for (i, s1) in states.iter().enumerate() {
        prover_sets.push(vec![prover(1, *s1)]);
        for s2 in &states[i..] {
            prover_sets.push(vec![prover(1, *s1), prover(2, *s2)]);
        }
    }
    let others: Vec<Option<Cause>> = vec![
        None,
        Some(New),
        Some(Env),
        Some(Preexisting),
        Some(Flaky),
        Some(Changed),
    ];
    let mut checked = 0;
    for rule in [ProofRule::HeadPasses, ProofRule::Transplant] {
        for provers in &prover_sets {
            for judging in [false, true] {
                for other in &others {
                    let mut entries = provers.clone();
                    if judging {
                        entries.push(entry("judging::aver.toml", H::Fail, Some(Changed), &[]));
                    }
                    let other_head = if other.is_none() { H::Pass } else { H::Fail };
                    entries.push(entry("other", other_head, *other, &[]));
                    let entries = Set::new(entries).unwrap();
                    let entries = entries.as_slice();
                    let (unproven, v) = verdict(entries, &a, rule);
                    let any = |cause| entries.iter().any(|e| e.cause == Some(cause));
                    let inconclusive = entries.iter().any(|e| inconclusive(e, rule));
                    let context = format!("{rule:?} {entries:?} -> {v:?} {unproven:?}");
                    if unproven.is_empty() {
                        assert!(
                            provers.iter().any(|e| aver_core::rules::proves(e, rule)),
                            "{context}"
                        );
                        let clean = !any(New) && !any(Env) && !inconclusive;
                        assert_eq!(v == Verdict::Pass, clean && !any(Changed), "{context}");
                        assert_eq!(v == Verdict::Review, clean && any(Changed), "{context}");
                        checked += 1;
                        continue;
                    }
                    let reason = unproven_reason(entries, &unproven[0], rule);
                    assert_ne!(v, Verdict::Pass, "{context}");
                    assert_ne!(v, Verdict::Review, "{context}");
                    assert_ne!(reason, UnprovenReason::ClaimedTestNotFound, "{context}");
                    match reason {
                        UnprovenReason::JudgingFilesChanged => {
                            assert!(judging && rule == ProofRule::Transplant, "{context}");
                            if any(New) {
                                assert_eq!(v, Verdict::FailOwn, "{context}");
                            }
                        }
                        UnprovenReason::Blocked => {
                            assert!(!provers.is_empty(), "{context}");
                            if !any(New) {
                                assert_eq!(v, Verdict::BlockedNotOwn, "{context}");
                            }
                        }
                        UnprovenReason::NotClaimed => assert!(provers.is_empty(), "{context}"),
                        UnprovenReason::PassesWithoutChange => {
                            assert_eq!(rule, ProofRule::Transplant, "{context}");
                            assert!(
                                provers.iter().any(|e| e.head == H::Pass
                                    && e.cause.is_none()
                                    && matches!(e.head_tests, None | Some(Transplant::Pass))
                                    && !matches!(
                                        e.transplant,
                                        Some(Transplant::Fail | Transplant::Absent)
                                    )),
                                "{context}"
                            );
                        }
                        UnprovenReason::FailsAtHead => {
                            assert!(
                                provers.iter().any(|e| (e.head != H::Pass
                                    && matches!(e.cause, Some(New | Preexisting | Changed)))
                                    || (e.head == H::Pass
                                        && e.cause.is_none()
                                        && matches!(
                                            e.head_tests,
                                            Some(Transplant::Fail | Transplant::Absent)
                                        ))),
                                "{context}"
                            )
                        }
                        // These need more than the receipt: the head's files, the
                        // transplant's failure messages, the checks' reports.
                        UnprovenReason::ClaimedTestNotFound
                        | UnprovenReason::WeakProof
                        | UnprovenReason::NoTestNames
                        | UnprovenReason::UntestedHunk => {
                            unreachable!()
                        }
                    }
                    if v == Verdict::Unproven {
                        assert_ne!(reason, UnprovenReason::Blocked, "{context}");
                    }
                    if v == Verdict::BlockedNotOwn && !any(Env) && !inconclusive {
                        assert_eq!(reason, UnprovenReason::Blocked, "{context}");
                    }
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 100_000, "{checked}");
}

#[test]
fn uncovered_changes_hold_pass_and_review_at_unproven() {
    use aver_core::rules::verdict_with_coverage;
    use Cause::*;
    let none = criteria(&[]);
    let cases: Vec<(&str, Vec<ReceiptCheck>, &Criteria, Verdict)> = vec![
        (
            "a pass",
            vec![entry("unit::adds", H::Pass, None, &[])],
            &none,
            Verdict::Unproven,
        ),
        (
            "a review",
            vec![entry("judging::aver.toml", H::Fail, Some(Changed), &[])],
            &none,
            Verdict::Unproven,
        ),
        (
            "a new failure still fails",
            vec![entry("unit::adds", H::Fail, Some(New), &[])],
            &none,
            Verdict::FailOwn,
        ),
        (
            "an infrastructure failure still blocks",
            vec![entry("unit::adds", H::Fail, Some(Env), &[])],
            &none,
            Verdict::BlockedNotOwn,
        ),
    ];
    for (what, entries, criteria, want) in cases {
        let (unproven, got) =
            verdict_with_coverage(&entries, criteria, ProofRule::Transplant, true);
        assert_eq!(got, want, "{what}");
        assert!(
            unproven.is_empty(),
            "{what}: no criterion is unproven by it"
        );
        let (_, covered) = verdict_with_coverage(&entries, criteria, ProofRule::Transplant, false);
        assert_eq!(
            covered,
            verdict(&entries, criteria, ProofRule::Transplant).1,
            "{what}"
        );
    }
}

/// Red team hole 7: a head that fails some runs of a test the base passes
/// on every run is the change's failure, not a flaky test, and a flaky head
/// asks for the base to be rerun to three results first.
#[test]
fn a_flaky_head_over_a_stable_base_is_new() {
    for base in [vec![P], vec![P, P, P], vec![I, P]] {
        for head in [vec![F, P], vec![F, F, P], vec![P, F]] {
            let c = classify(&base, &head);
            assert_eq!(c.cause, Some(Cause::New), "{base:?} {head:?}");
            assert_eq!(c.head, H::Flaky);
        }
    }
    // The base flaky on its own reruns: the test is flaky, not broken.
    assert_eq!(classify(&[P, F], &[F, P]).cause, Some(Cause::Flaky));
    assert_eq!(classify(&[P, P, F], &[F, P]).cause, Some(Cause::Flaky));
    let cases: &[(&[Ending], &[Ending], bool)] = &[
        (&[P], &[F, P], true),
        (&[P, P], &[F, P], true),
        (&[P, P, P], &[F, P], false),
        (&[P], &[P], false),
        (&[P], &[F, F, F], false),
        (&[F, F, F], &[F, P], false),
        (&[P, F], &[F, P], false),
        (&[I, I], &[F, P], false),
        (&[], &[F, P], false),
    ];
    for (base, head, want) in cases {
        assert_eq!(
            base_needs_confirmation(base, head),
            *want,
            "base {base:?}, head {head:?}"
        );
    }
}

/// Red team hole 3: a head that hits the time or memory cap on every run
/// while the base finished is the change's, even when the base fails some
/// test; a base that itself ended in infra leaves it ENV.
#[test]
fn own_exhaustion_counts_whatever_the_base_result() {
    for base in [vec![P], vec![F, F, F], vec![F, P], vec![I, F, F, F]] {
        let c = classify(&base, &[C, C]);
        assert_eq!(c.cause, Some(Cause::New), "base {base:?}");
    }
    assert_eq!(classify(&[I, I], &[C, C]).cause, Some(Cause::Env));
    assert_eq!(classify(&[F, C, C], &[C, C]).cause, Some(Cause::Env));
    assert_eq!(classify(&[], &[C, C]).cause, Some(Cause::Env));
    assert_eq!(classify(&[F, F, F], &[I, I]).cause, Some(Cause::Env));
}

/// Red team hole 2: a head that lost the report every base run produced is
/// missing its result, `NEW`, never `PREEXISTING` behind a test the base
/// already fails; a rewritten test whose own version passes is `CHANGED`.
#[test]
fn a_lost_report_is_new() {
    let lost = |origin, base, own| {
        judge(judge::Entry {
            origin,
            base,
            judged: Judged::Missing,
            own,
            transplant: judge::Transplant::NotRun,
        })
        .cause
    };
    for base in [Base::Fail, Base::Flaky, Base::Pass] {
        assert_eq!(
            lost(Origin::UntouchedSource, base, Own::NotRun),
            Some(Cause::New),
            "{base:?}"
        );
    }
    assert_eq!(
        lost(Origin::ModifiedTestFile, Base::Pass, Own::Pass),
        Some(Cause::Changed)
    );
}

/// A cap never raises a verdict: no runtime tests turns `PASS` and `REVIEW`
/// into `UNPROVEN`, a proposed contract `PASS` into `REVIEW`, and a failing
/// or blocked verdict stands.
#[test]
fn caps_only_lower_a_verdict() {
    use aver_core::rules::{cap, Cap};
    use Verdict::*;
    assert_eq!(cap(Pass, true, false), (Pass, None));
    assert_eq!(
        cap(Pass, false, false),
        (Unproven, Some(Cap::NoRuntimeTests))
    );
    assert_eq!(
        cap(Review, false, false),
        (Unproven, Some(Cap::NoRuntimeTests))
    );
    assert_eq!(
        cap(Pass, false, true),
        (Unproven, Some(Cap::NoRuntimeTests))
    );
    assert_eq!(cap(Pass, true, true), (Review, Some(Cap::ProposedContract)));
    for v in [FailOwn, BlockedNotOwn, Unproven] {
        assert_eq!(cap(v, false, true), (v, None), "{v:?}");
    }
    assert_eq!(cap(Review, true, true), (Review, None));
}
