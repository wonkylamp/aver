use aver_core::judge::{judge, proves, Base, Entry, Judged, Origin, Own, Transplant};
use aver_core::Cause;

fn origins() -> [Origin; 8] {
    use Origin::*;
    let all = [
        UntouchedSource,
        ModifiedSource,
        DeletedSourceWithInlineTests,
        UntouchedTestFile,
        ModifiedTestFile,
        DeletedTestFile,
        AddedTestFile,
        BuiltArtifactModified,
    ];
    for o in all {
        match o {
            UntouchedSource
            | ModifiedSource
            | DeletedSourceWithInlineTests
            | UntouchedTestFile
            | ModifiedTestFile
            | DeletedTestFile
            | AddedTestFile
            | BuiltArtifactModified => {}
        }
    }
    all
}

fn bases() -> [Base; 7] {
    use Base::*;
    let all = [Pass, Fail, Flaky, CompileError, Infra, ZeroTests, Absent];
    for b in all {
        match b {
            Pass | Fail | Flaky | CompileError | Infra | ZeroTests | Absent => {}
        }
    }
    all
}

fn judgeds() -> [Judged; 8] {
    use Judged::*;
    let all = [
        Pass,
        Fail,
        Flaky,
        CompileError,
        Exhausted,
        Infra,
        Missing,
        NotApplicable,
    ];
    for j in all {
        match j {
            Pass | Fail | Flaky | CompileError | Exhausted | Infra | Missing | NotApplicable => {}
        }
    }
    all
}

fn owns() -> [Own; 9] {
    use Own::*;
    let all = [
        Pass,
        Fail,
        Flaky,
        Skipped,
        Absent,
        CompileError,
        Exhausted,
        Infra,
        NotRun,
    ];
    for o in all {
        match o {
            Pass | Fail | Flaky | Skipped | Absent | CompileError | Exhausted | Infra | NotRun => {}
        }
    }
    all
}

fn transplants() -> [Transplant; 5] {
    use Transplant::*;
    let all = [FailOnAssertion, FailOnCompileOrImport, Pass, Infra, NotRun];
    for t in all {
        match t {
            FailOnAssertion | FailOnCompileOrImport | Pass | Infra | NotRun => {}
        }
    }
    all
}

fn every_entry() -> Vec<Entry> {
    let mut out = Vec::new();
    for origin in origins() {
        for base in bases() {
            for judged in judgeds() {
                for own in owns() {
                    for transplant in transplants() {
                        out.push(Entry {
                            origin,
                            base,
                            judged,
                            own,
                            transplant,
                        });
                    }
                }
            }
        }
    }
    out
}

fn entry(origin: Origin, base: Base, judged: Judged, own: Own) -> Entry {
    Entry {
        origin,
        base,
        judged,
        own,
        transplant: Transplant::NotRun,
    }
}

fn cause(origin: Origin, base: Base, judged: Judged, own: Own) -> Option<Cause> {
    judge(entry(origin, base, judged, own)).cause
}

/// The whole table as text: a line per origin, base and judged head, then
/// per own result the head status and the cause under each transplant
/// result, in [`transplants`] order.
fn table() -> String {
    let mut out = String::from(
        "# origin base judged | own: head status, cause per transplant (FailOnAssertion FailOnCompileOrImport Pass Infra NotRun)\n# cause: - none, N NEW, P PREEXISTING, F FLAKY, C CHANGED, E ENV\n",
    );
    for origin in origins() {
        for base in bases() {
            for judged in judgeds() {
                out.push_str(&format!("{origin:?} {base:?} {judged:?} |"));
                for own in owns() {
                    let mut head = None;
                    let mut causes = String::new();
                    for transplant in transplants() {
                        let c = judge(Entry {
                            origin,
                            base,
                            judged,
                            own,
                            transplant,
                        });
                        assert!(head.is_none_or(|h| h == c.head), "head status per own");
                        head = Some(c.head);
                        causes.push(match c.cause {
                            None => '-',
                            Some(Cause::New) => 'N',
                            Some(Cause::Preexisting) => 'P',
                            Some(Cause::Flaky) => 'F',
                            Some(Cause::Changed) => 'C',
                            Some(Cause::Env) => 'E',
                        });
                    }
                    let head = format!("{:?}", head.expect("five transplants")).to_lowercase();
                    out.push_str(&format!(" {own:?}: {head} {causes};"));
                }
                out.push('\n');
            }
        }
    }
    out
}

/// The table is frozen: a change to any cell shows as a diff of
/// `testdata/judge_table.txt`. It was first written from a table that
/// agreed with the reclassifier chain it replaced, except the intended
/// changes ARCHITECTURE.md §1.9.3 lists. Regenerate with
/// `AVER_UPDATE_GOLDEN=1 cargo test -p aver-core --test judge`.
#[test]
fn the_table_is_frozen() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/judge_table.txt");
    let got = table();
    if std::env::var_os("AVER_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; run with AVER_UPDATE_GOLDEN=1", path.display()));
    for (i, (g, w)) in got.lines().zip(want.lines()).enumerate() {
        assert_eq!(g, w, "line {} of {}", i + 1, path.display());
    }
    assert_eq!(got.lines().count(), want.lines().count(), "line count");
}

#[test]
fn zero_evidence_is_never_a_pass() {
    for e in every_entry()
        .into_iter()
        .filter(|e| e.base == Base::ZeroTests)
    {
        assert_eq!(judge(e).cause, Some(Cause::Env), "{e:?}");
    }
}

#[test]
fn nothing_weakens_silently() {
    for e in every_entry() {
        let weakening = matches!(
            e.origin,
            Origin::DeletedTestFile
                | Origin::DeletedSourceWithInlineTests
                | Origin::BuiltArtifactModified
        ) || (e.base == Base::Pass && e.own == Own::Skipped);
        if weakening {
            assert_ne!(judge(e).cause, None, "{e:?}");
        }
    }
}

#[test]
fn infra_on_the_base_or_the_judged_head_is_env() {
    for e in every_entry()
        .into_iter()
        .filter(|e| e.base == Base::Infra || e.judged == Judged::Infra)
    {
        assert_eq!(judge(e).cause, Some(Cause::Env), "{e:?}");
    }
}

#[test]
fn a_lost_own_or_transplant_result_only_excuses_nothing() {
    for e in every_entry() {
        let lost_own = Entry {
            own: Own::Infra,
            ..e
        };
        let lost_transplant = Entry {
            transplant: Transplant::Infra,
            ..e
        };
        for lost in [lost_own, lost_transplant] {
            assert_ne!(judge(lost).cause, None, "{lost:?}");
        }
        let unknown_own = Entry {
            own: Own::NotRun,
            ..e
        };
        if e.judged != Judged::NotApplicable && judge(unknown_own).cause == Some(Cause::New) {
            assert_eq!(judge(lost_own).cause, Some(Cause::New), "{lost_own:?}");
        }
        let unknown_transplant = Entry {
            transplant: Transplant::NotRun,
            ..e
        };
        if judge(unknown_transplant).cause == Some(Cause::New) {
            assert_eq!(
                judge(lost_transplant).cause,
                Some(Cause::New),
                "{lost_transplant:?}"
            );
        }
    }
}

#[test]
fn losing_a_result_never_helps_the_change() {
    for e in every_entry() {
        let before = judge(e).cause;
        let lost = [
            Entry {
                base: Base::Infra,
                ..e
            },
            Entry {
                judged: Judged::Infra,
                ..e
            },
            Entry {
                own: Own::Infra,
                ..e
            },
            Entry {
                transplant: Transplant::Infra,
                ..e
            },
        ];
        for after in lost.map(|l| judge(l).cause) {
            assert_ne!(after, None, "{e:?}: {before:?} became a pass");
            if before == Some(Cause::Env) {
                assert_ne!(after, Some(Cause::New), "{e:?}: ENV became NEW");
            }
        }
    }
}

#[test]
fn a_test_deleted_with_its_source_is_changed_crit_deleted_source() {
    for base in [Base::Pass, Base::Fail, Base::Flaky, Base::CompileError] {
        assert_eq!(
            cause(
                Origin::DeletedSourceWithInlineTests,
                base,
                Judged::NotApplicable,
                Own::Absent
            ),
            Some(Cause::Changed)
        );
    }
    for e in every_entry()
        .into_iter()
        .filter(|e| e.origin == Origin::DeletedSourceWithInlineTests)
    {
        assert_ne!(judge(e).cause, Some(Cause::New), "{e:?}");
    }
}

#[test]
fn a_restored_test_file_that_does_not_compile_is_changed_crit_unbuilt_file() {
    for origin in [Origin::ModifiedTestFile, Origin::DeletedTestFile] {
        for own in [Own::Pass, Own::Absent] {
            assert_eq!(
                cause(origin, Base::Pass, Judged::CompileError, own),
                Some(Cause::Changed),
                "{origin:?} {own:?}"
            );
        }
    }
    assert_eq!(
        cause(
            Origin::UntouchedTestFile,
            Base::Pass,
            Judged::CompileError,
            Own::Pass
        ),
        Some(Cause::New)
    );
}

#[test]
fn a_restored_test_that_compiles_and_fails_is_new_crit_h2() {
    for origin in [Origin::ModifiedTestFile, Origin::DeletedTestFile] {
        for own in [Own::Absent, Own::Skipped, Own::Fail] {
            assert_eq!(
                cause(origin, Base::Pass, Judged::Fail, own),
                Some(Cause::New),
                "{origin:?} {own:?}"
            );
        }
    }
}

#[test]
fn a_rewritten_expectation_is_changed_crit_rewritten() {
    let modified = Origin::ModifiedTestFile;
    assert_eq!(
        cause(modified, Base::Pass, Judged::Fail, Own::Pass),
        Some(Cause::Changed)
    );
    for own in [Own::Pass, Own::Absent] {
        assert_eq!(
            cause(modified, Base::Fail, Judged::Fail, own),
            Some(Cause::Changed)
        );
    }
}

#[test]
fn a_base_that_ran_no_test_is_env_crit_zero_tests() {
    assert_eq!(
        cause(
            Origin::UntouchedTestFile,
            Base::ZeroTests,
            Judged::Pass,
            Own::NotRun
        ),
        Some(Cause::Env)
    );
}

#[test]
fn infra_is_env_and_own_exhaustion_is_new_crit_infra() {
    let o = Origin::UntouchedTestFile;
    assert_eq!(
        cause(o, Base::Infra, Judged::Fail, Own::NotRun),
        Some(Cause::Env)
    );
    assert_eq!(
        cause(o, Base::Pass, Judged::Infra, Own::NotRun),
        Some(Cause::Env)
    );
    for base in [Base::Pass, Base::Fail, Base::Flaky] {
        assert_eq!(
            cause(o, base, Judged::Exhausted, Own::NotRun),
            Some(Cause::New)
        );
    }
    for base in [Base::Infra, Base::ZeroTests] {
        assert_eq!(
            cause(o, base, Judged::Exhausted, Own::NotRun),
            Some(Cause::Env)
        );
    }
}

#[test]
fn a_test_missing_from_the_report_is_new_crit_crashed() {
    for base in [Base::Pass, Base::Fail] {
        assert_eq!(
            cause(
                Origin::UntouchedTestFile,
                base,
                Judged::Missing,
                Own::NotRun
            ),
            Some(Cause::New)
        );
    }
}

#[test]
fn a_flaky_head_is_new_over_a_stable_base_crit_flaky() {
    let o = Origin::UntouchedTestFile;
    assert_eq!(
        cause(o, Base::Pass, Judged::Flaky, Own::NotRun),
        Some(Cause::New)
    );
    assert_eq!(
        cause(o, Base::Flaky, Judged::Flaky, Own::NotRun),
        Some(Cause::Flaky)
    );
    assert_eq!(
        cause(
            Origin::AddedTestFile,
            Base::Absent,
            Judged::NotApplicable,
            Own::Flaky
        ),
        Some(Cause::New)
    );
}

#[test]
fn skipping_a_passing_test_is_changed_crit_skipped() {
    assert_eq!(
        cause(
            Origin::ModifiedTestFile,
            Base::Pass,
            Judged::Pass,
            Own::Skipped
        ),
        Some(Cause::Changed)
    );
}

#[test]
fn a_modified_built_artifact_is_changed_unless_new_crit_built() {
    let o = Origin::BuiltArtifactModified;
    assert_eq!(
        cause(o, Base::Pass, Judged::Pass, Own::NotRun),
        Some(Cause::Changed)
    );
    assert_eq!(
        cause(o, Base::Pass, Judged::Fail, Own::NotRun),
        Some(Cause::New)
    );
    assert_eq!(
        cause(o, Base::Pass, Judged::Infra, Own::NotRun),
        Some(Cause::Env)
    );
}

#[test]
fn a_modified_test_file_may_pass_crit_modified_pass() {
    assert_eq!(
        cause(
            Origin::ModifiedTestFile,
            Base::Pass,
            Judged::Pass,
            Own::Pass
        ),
        None
    );
    assert_eq!(
        cause(
            Origin::ModifiedTestFile,
            Base::Absent,
            Judged::NotApplicable,
            Own::Pass
        ),
        None
    );
}

#[test]
fn a_weak_proof_proves_nothing_crit_weak_proof() {
    assert!(proves(None, Own::Pass, Transplant::FailOnAssertion));
    assert!(!proves(None, Own::Pass, Transplant::FailOnCompileOrImport));
    assert!(!proves(None, Own::Pass, Transplant::Pass));
    assert!(!proves(
        Some(Cause::Changed),
        Own::Pass,
        Transplant::FailOnAssertion
    ));
    assert!(!proves(None, Own::Skipped, Transplant::FailOnAssertion));
}

#[test]
fn a_lost_own_result_keeps_new_crit_own_infra_keeps_new() {
    for origin in [Origin::ModifiedTestFile, Origin::DeletedTestFile] {
        assert_eq!(
            cause(origin, Base::Pass, Judged::Fail, Own::Infra),
            Some(Cause::New),
            "{origin:?}"
        );
    }
    let lost_transplant = Entry {
        transplant: Transplant::Infra,
        ..entry(
            Origin::UntouchedTestFile,
            Base::Pass,
            Judged::Fail,
            Own::NotRun,
        )
    };
    assert_eq!(judge(lost_transplant).cause, Some(Cause::New));
}

#[test]
fn an_own_version_that_hangs_is_the_heads_crit_own_exhausted_is_new() {
    assert_eq!(
        cause(
            Origin::AddedTestFile,
            Base::Absent,
            Judged::NotApplicable,
            Own::Exhausted
        ),
        Some(Cause::New)
    );
    assert_eq!(
        cause(
            Origin::ModifiedTestFile,
            Base::Pass,
            Judged::Fail,
            Own::Exhausted
        ),
        Some(Cause::New)
    );
}

#[test]
fn exhaustion_with_a_passing_own_version_is_changed_crit_rewritten_exhausted_own_pass_changed() {
    let o = Origin::ModifiedTestFile;
    assert_eq!(
        cause(o, Base::Pass, Judged::Exhausted, Own::Pass),
        Some(Cause::Changed)
    );
    assert_eq!(
        cause(o, Base::Pass, Judged::Exhausted, Own::Fail),
        Some(Cause::New)
    );
}
