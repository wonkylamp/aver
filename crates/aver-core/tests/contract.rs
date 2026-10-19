//! Snapshot v1, the contract object and its parse rules, run keys, and
//! trust.

mod common;

use aver_core::*;
use common::*;
use serde_json::{json, Value};

fn to_value<T: serde::Serialize>(t: T) -> Value {
    serde_json::to_value(t).unwrap()
}

fn parse<T: Object>(value: &Value) -> Result<T, ParseError> {
    T::from_canonical(&canonical::to_canonical(value).unwrap())
}

#[test]
fn a_v1_snapshot_is_its_tree_alone_and_v0_still_reads() {
    let v0 = sample_snapshot();
    let v1 = sample_snapshot_v1();
    assert_eq!(v0.schema(), SnapshotSchema::V0);
    assert_eq!(v1.schema(), SnapshotSchema::V1);
    assert_eq!(v0.root(), v1.root());
    assert_eq!(v0.root(), sample_tree().id());
    assert_ne!(v0.id(), v1.id());
    assert_eq!(
        to_value(&v1),
        json!({"schema": "aver.snapshot/v1", "root": sample_tree().id().to_string()})
    );
    assert_eq!(
        to_value(&v0),
        json!({
            "schema": "aver.snapshot/v0",
            "root": sample_tree().id().to_string(),
            "env": sample_env().id().to_string(),
        })
    );

    let mut v0_without_env = to_value(&v0);
    v0_without_env.as_object_mut().unwrap().remove("env");
    assert!(parse::<Snapshot>(&v0_without_env).is_err());
    let mut v1_with_env = to_value(&v0);
    v1_with_env["schema"] = json!("aver.snapshot/v1");
    assert!(parse::<Snapshot>(&v1_with_env).is_err());
    let mut v2 = to_value(&v1);
    v2["schema"] = json!("aver.snapshot/v2");
    assert!(parse::<Snapshot>(&v2).is_err());
}

fn check_json(value: &Value) -> Result<ContractCheck, String> {
    serde_json::from_value(value.clone()).map_err(|e| e.to_string())
}

#[test]
fn a_check_takes_min_tests_exactly_when_it_counts_tests() {
    let base = json!({"id": "unit", "run": "npm test", "report": "junit", "min_tests": 10, "serial": false});
    let unit = check_json(&base).unwrap();
    assert_eq!(unit.min_tests().map(|n| n.get()), Some(10));
    assert_eq!(
        unit.reading,
        Reading::Tests {
            report: ReportSpec::Junit,
            min_tests: 10.try_into().unwrap()
        }
    );

    let with = |patch: Value| {
        let mut v = base.clone();
        for (k, x) in patch.as_object().unwrap() {
            if x.is_null() {
                v.as_object_mut().unwrap().remove(k);
            } else {
                v[k] = x.clone();
            }
        }
        check_json(&v)
    };
    let err = with(json!({"min_tests": null})).unwrap_err();
    assert!(err.contains("min_tests is required"), "{err}");
    assert!(
        with(json!({"min_tests": 0})).is_err(),
        "a floor of 0 is no floor"
    );
    let err = with(json!({"report": "exit"})).unwrap_err();
    assert!(err.contains("takes neither min_tests"), "{err}");
    let exit = with(json!({"report": "exit", "min_tests": null})).unwrap();
    assert_eq!(exit.reading, Reading::Exit);
    assert_eq!(exit.min_tests(), None);
    assert!(
        with(json!({"report_file": "unit.xml"})).is_err(),
        "JUnit reads every *.xml"
    );
    assert!(with(json!({"report": "exit", "min_tests": null, "report_file": "x"})).is_err());

    let stdout = with(json!({"report": "libtest"})).unwrap();
    assert!(matches!(
        stdout.reading,
        Reading::Tests {
            report: ReportSpec::Stdout(TextFormat::Libtest),
            ..
        }
    ));
    let file = with(json!({"report": "go-json", "report_file": "go/test.json"})).unwrap();
    assert!(matches!(
        &file.reading,
        Reading::Tests { report: ReportSpec::File { format: TextFormat::GoJson, path }, .. }
            if path.as_str() == "go/test.json"
    ));

    assert!(
        with(json!({"paths": []})).is_err(),
        "empty paths would never run"
    );
    assert!(with(json!({"paths": ["src/**"]})).is_ok());
    assert!(with(json!({"run": "   "})).is_err());
    assert!(with(json!({"run": "a\u{0}b"})).is_err());
    assert!(with(json!({"run": "x".repeat(4097)})).is_err());
    assert!(
        with(json!({"serial": null})).is_err(),
        "serial is always written"
    );
    assert!(with(json!({"timeout_s": 0})).is_err());
    assert!(with(json!({"extra": 1})).is_err());
    assert!(with(json!({"report": "xunit"})).is_err());
    let tap = with(json!({"report": "tap", "report_file": "test.tap"})).unwrap();
    assert!(matches!(
        &tap.reading,
        Reading::Tests { report: ReportSpec::File { format: TextFormat::Tap, path }, .. }
            if path.as_str() == "test.tap"
    ));

    for value in [base.clone(), to_value(&file), to_value(&exit)] {
        let check = check_json(&value).unwrap();
        assert_eq!(to_value(&check), value, "the flat fields round-trip");
    }
}

#[test]
fn a_contract_holds_one_to_thirty_two_uniquely_named_checks() {
    let contract = to_value(sample_contract());
    assert!(parse::<Contract>(&contract).is_ok());

    let one = contract["checks"][0].clone();
    let named = |i: usize| {
        let mut c = one.clone();
        c["id"] = json!(format!("c{i:02}"));
        c
    };
    let with_checks = |checks: Vec<Value>| {
        let mut c = contract.clone();
        c["checks"] = Value::Array(checks);
        parse::<Contract>(&c)
    };
    assert!(with_checks(vec![]).is_err());
    assert!(with_checks((0..32).map(named).collect()).is_ok());
    assert!(with_checks((0..33).map(named).collect()).is_err());
    assert!(
        with_checks(vec![one.clone(), one.clone()]).is_err(),
        "duplicate id"
    );
    assert!(
        with_checks(vec![named(2), named(1)]).is_err(),
        "unsorted ids"
    );

    for (field, value) in [
        ("schema", json!("aver.contract/v0")),
        ("extra", json!(true)),
        (
            "deps",
            json!({"keep": [], "seed": [], "inputs": [], "install": "npm ci"}),
        ),
        (
            "deps",
            json!({"keep": ["b", "a"], "seed": [], "inputs": []}),
        ),
        (
            "judge",
            json!({"tests": [], "harness": [], "built_tests": []}),
        ),
        (
            "judge",
            json!({"tests": [], "harness": [], "built_tests": [], "protected": []}),
        ),
        (
            "ci",
            json!({"review": "auto", "require_recorded_base": false}),
        ),
    ] {
        let mut c = contract.clone();
        c[field] = value.clone();
        assert!(parse::<Contract>(&c).is_err(), "{field}: {value}");
    }
    assert!(
        Deps::new(
            Set::empty(),
            Set::empty(),
            Some("npm ci".parse().unwrap()),
            Set::empty()
        )
        .is_err(),
        "an install with no inputs never runs"
    );
}

#[test]
fn defaults_judge_built_tests_and_ask_for_approval() {
    let judge = JudgeGlobs::default();
    let built: Vec<&str> = judge.built_tests.iter().map(PathGlob::as_str).collect();
    assert_eq!(
        built,
        ["**/*.test.js", "build/**/test/**", "dist/**/test/**"]
    );
    assert!(judge.tests.is_empty() && judge.harness.is_empty() && judge.protected.is_empty());
    let sensitive: Vec<&str> = judge.sensitive.iter().map(PathGlob::as_str).collect();
    assert_eq!(sensitive, [".github/actions/**", ".github/workflows/**"]);
    assert!(judge.sensitive_match(".github/workflows/ci.yml").is_some());
    assert!(judge
        .sensitive_match("src/.github/workflows/ci.yml")
        .is_none());
    assert_eq!(CiPolicy::default().review, ReviewPolicy::Approve);
    assert!(!CiPolicy::default().require_recorded_base);
}

#[test]
fn a_check_key_changes_with_every_field_that_shapes_a_run() {
    let contract = sample_contract();
    let unit = contract.check(&"unit".parse().unwrap()).unwrap().clone();
    assert_eq!(unit.key(), unit.clone().key());
    let mut edits: Vec<ContractCheck> = Vec::new();
    let mut c = unit.clone();
    c.run = "node --test".parse().unwrap();
    edits.push(c);
    let mut c = unit.clone();
    c.cwd = Some("pkg".parse().unwrap());
    edits.push(c);
    let mut c = unit.clone();
    c.prepare = None;
    edits.push(c);
    let mut c = unit.clone();
    c.reading = Reading::Exit;
    edits.push(c);
    let mut c = unit.clone();
    c.id = "unit2".parse().unwrap();
    edits.push(c);
    for edit in edits {
        assert_ne!(edit.key(), unit.key(), "{edit:?}");
    }
}

fn tool(version: &[u8]) -> HostTool {
    HostTool {
        name: "node".into(),
        path: "/usr/local/bin/node".into(),
        version: version.to_vec(),
    }
}

#[test]
fn a_run_key_names_tree_check_place_deps_and_index() {
    let contract = sample_contract();
    let check = contract.checks.iter().next().unwrap().key();
    let place = PlaceKey::Host(HostKey::of(
        HostOs::Darwin,
        HostArch::Arm64,
        &[tool(b"v24.1.0")],
    ));
    let lock: Path = "package-lock.json".parse().unwrap();
    let manifest: Path = "package.json".parse().unwrap();
    let (a, b) = (BlobId::of_bytes(b"a"), BlobId::of_bytes(b"b"));
    let deps = DepsState::of([(&lock, &a), (&manifest, &b)]);
    assert_eq!(
        deps,
        DepsState::of([(&manifest, &b), (&lock, &a), (&lock, &a)]),
        "order and repeats do not matter"
    );
    let key = RunKey {
        tree: sample_tree().id(),
        check: check.clone(),
        place: place.clone(),
        deps: deps.clone(),
        index: RunIndex::FIRST,
    };
    assert_eq!(key.id(), key.clone().id());
    assert_eq!(key.id().as_str().len(), 64);

    let others = [
        RunKey {
            tree: sample_snapshot_tree_other(),
            ..key.clone()
        },
        RunKey {
            check: contract.checks.iter().nth(1).unwrap().key(),
            ..key.clone()
        },
        RunKey {
            place: PlaceKey::Host(HostKey::of(
                HostOs::Darwin,
                HostArch::Arm64,
                &[tool(b"v24.2.0")],
            )),
            ..key.clone()
        },
        RunKey {
            place: PlaceKey::Host(HostKey::of(
                HostOs::Linux,
                HostArch::Arm64,
                &[tool(b"v24.1.0")],
            )),
            ..key.clone()
        },
        RunKey {
            deps: DepsState::of([(&lock, &b)]),
            ..key.clone()
        },
        RunKey {
            index: RunIndex::SECOND,
            ..key.clone()
        },
    ];
    for other in others {
        assert_ne!(other.id(), key.id(), "{other:?}");
    }
    let wire = to_value(&key);
    assert_eq!(wire["place"]["host"].as_str().map(str::len), Some(64));
    assert_eq!(serde_json::from_value::<RunKey>(wire).unwrap(), key);
}

fn sample_snapshot_tree_other() -> TreeId {
    Tree::new(Set::empty()).id()
}

#[test]
fn only_a_base_approved_contract_in_ci_on_unchanged_workflows_is_trusted() {
    let base = Approval::Base {
        blob: BlobId::of_bytes(b"[[check]]"),
    };
    let proposed = Approval::Proposed {
        blob: BlobId::of_bytes(b"[[check]]"),
    };
    let adhoc = Approval::Adhoc {
        line: "pytest -q".parse().unwrap(),
    };
    use AdvisoryKind as A;
    use Venue::{Ci, Local};
    use Workflows::{EditedUnapproved as Edited, Unchanged};
    let cases = [
        (&base, Ci, Unchanged, Trust::Trusted),
        (&base, Ci, Edited, Trust::Advisory(A::WorkflowEdited)),
        (&base, Local, Unchanged, Trust::Advisory(A::Local)),
        (&base, Local, Edited, Trust::Advisory(A::Local)),
        (&proposed, Ci, Unchanged, Trust::Advisory(A::Proposed)),
        (&proposed, Local, Unchanged, Trust::Advisory(A::Proposed)),
        (&adhoc, Ci, Unchanged, Trust::Advisory(A::Adhoc)),
        (&adhoc, Local, Edited, Trust::Advisory(A::Adhoc)),
    ];
    for (approval, venue, workflows, trust) in cases {
        assert_eq!(
            Trust::of(approval, venue, workflows),
            trust,
            "{approval:?} {venue:?} {workflows:?}"
        );
    }
    assert!(base.approves_exit());
    assert!(!proposed.approves_exit());
    assert!(!adhoc.approves_exit());
}
