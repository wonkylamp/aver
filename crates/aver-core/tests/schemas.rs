//! Every object type validates against its published JSON Schema in
//! `schemas/`, and the schemas reject representative invalid objects.

mod common;

use std::collections::BTreeMap;

use aver_core::*;
use common::*;
use jsonschema::Validator;
use serde_json::{json, to_value, Value};

const SCHEMAS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../schemas");
const DEFS_ID: &str = "https://aver.build/schemas/defs/v0";

/// Schema file kind, version and the version string the Rust type writes.
const KINDS: &[(&str, u32, &str)] = &[
    ("tree", 0, Tree::SCHEMA),
    ("env", 0, Environment::SCHEMA),
    ("env", 1, Environment::SCHEMA_V1),
    ("env", 2, Environment::SCHEMA_V2),
    ("env", 3, Environment::SCHEMA_V3),
    ("env", 4, Environment::SCHEMA_V4),
    ("snapshot", 0, Snapshot::SCHEMA),
    ("snapshot", 1, Snapshot::SCHEMA_V1),
    ("contract", 1, Contract::SCHEMA),
    ("transition", 0, TransitionBody::SCHEMA),
    ("task", 0, TaskBody::SCHEMA),
    ("task", 1, TaskBody::SCHEMA_V1),
    ("task", 2, TaskBody::SCHEMA_V2),
    ("run", 0, CheckRun::SCHEMA),
    ("receipt", 0, ReceiptBody::SCHEMA),
    ("receipt", 1, ReceiptBody::SCHEMA_V1),
    ("receipt", 2, ReceiptBody::SCHEMA_V2),
    ("receipt", 3, ReceiptBody::SCHEMA_V3),
    ("attempt", 0, AttemptRecord::SCHEMA),
    ("ledger", 0, LedgerBody::SCHEMA),
];

fn load(file: &str) -> Value {
    let path = format!("{SCHEMAS_DIR}/{file}");
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// One validator per schema file, keyed by the `schema` string it accepts.
fn validators() -> BTreeMap<&'static str, Validator> {
    let defs = load("aver.defs.v0.json");
    let registry = jsonschema::Registry::new()
        .add(DEFS_ID, &defs)
        .unwrap()
        .prepare()
        .unwrap();
    KINDS
        .iter()
        .map(|(kind, version, schema_id)| {
            let schema = load(&format!("aver.{kind}.v{version}.json"));
            let validator = jsonschema::options()
                .with_registry(&registry)
                .build(&schema)
                .unwrap_or_else(|e| panic!("{kind} v{version}: {e}"));
            (*schema_id, validator)
        })
        .collect()
}

/// The validator for `instance`'s own `schema` string.
fn validator<'a>(validators: &'a BTreeMap<&str, Validator>, instance: &Value) -> &'a Validator {
    let schema = instance["schema"].as_str().expect("objects carry a schema");
    validators
        .get(schema)
        .unwrap_or_else(|| panic!("no schema file for {schema}"))
}

fn assert_valid(validators: &BTreeMap<&str, Validator>, kind: &str, instance: &Value) {
    let errors: Vec<String> = validator(validators, instance)
        .iter_errors(instance)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect();
    assert!(errors.is_empty(), "{kind} sample is invalid: {errors:#?}");
}

#[test]
fn schema_files_match_the_rust_types() {
    for (kind, version, schema_id) in KINDS {
        let schema = load(&format!("aver.{kind}.v{version}.json"));
        assert_eq!(
            schema["$id"],
            format!("https://aver.build/schemas/{kind}/v{version}")
        );
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert_eq!(schema["properties"]["schema"]["const"], *schema_id);
        assert_eq!(*schema_id, format!("aver.{kind}/v{version}"));
    }
}

#[test]
fn every_sample_object_validates() {
    let validators = validators();
    let samples = all_samples();
    let covered: std::collections::BTreeSet<String> = samples
        .iter()
        .map(|s| {
            let v: Value = serde_json::from_slice(&s.2).unwrap();
            v["schema"].as_str().unwrap().to_owned()
        })
        .collect();
    assert_eq!(
        covered.len(),
        KINDS.len(),
        "a sample exists for every schema version"
    );
    for (kind, _, bytes, _) in samples {
        let instance: Value = serde_json::from_slice(&bytes).unwrap();
        assert_valid(&validators, kind, &instance);
    }
}

#[test]
fn schemas_reject_invalid_objects() {
    let validators = validators();
    let value = |obj: Vec<u8>| -> Value { serde_json::from_slice(&obj).unwrap() };
    type Mutation = fn(&mut Value);
    let cases: Vec<(&str, Value, &str, Mutation)> = vec![
        (
            "tree",
            value(sample_tree().canonical_bytes()),
            "extra field",
            |v| v["extra"] = json!(1),
        ),
        (
            "tree",
            value(sample_tree().canonical_bytes()),
            "future schema version",
            |v| v["schema"] = json!("aver.tree/v1"),
        ),
        (
            "tree",
            value(sample_tree().canonical_bytes()),
            "tree entry with a blob id",
            |v| {
                let src = v["entries"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|e| e["name"] == "src")
                    .unwrap();
                src["id"] = json!(BlobId::of_bytes(b"x").to_string());
            },
        ),
        (
            "tree",
            value(sample_tree().canonical_bytes()),
            ".git entry name",
            |v| v["entries"][0]["name"] = json!(".GIT"),
        ),
        (
            "snapshot",
            value(sample_snapshot().canonical_bytes()),
            "wrong id prefix",
            |v| v["root"] = v["env"].clone(),
        ),
        (
            "snapshot",
            value(sample_snapshot().canonical_bytes()),
            "missing env",
            |v| {
                v.as_object_mut().unwrap().remove("env");
            },
        ),
        (
            "env",
            value(sample_env().canonical_bytes()),
            "float limit",
            |v| v["limits"]["pids"] = json!(1.5),
        ),
        (
            "env",
            value(sample_env().canonical_bytes()),
            "secret var name",
            |v| v["vars"] = json!({"NPM_TOKEN": "x"}),
        ),
        (
            "env",
            value(sample_env().canonical_bytes()),
            "report outside /aver/out",
            |v| v["checks"][1]["report"]["path"] = json!("/tmp/unit.xml"),
        ),
        (
            "env",
            value(sample_env().canonical_bytes()),
            "tests under v0",
            |v| v["tests"] = json!(["test/**"]),
        ),
        (
            "env",
            value(sample_env_v1().canonical_bytes()),
            "empty tests under v1",
            |v| v["tests"] = json!([]),
        ),
        (
            "env",
            value(sample_env().canonical_bytes()),
            "a jest-json report under v0",
            |v| v["checks"][1]["report"]["format"] = json!("jest-json"),
        ),
        (
            "env",
            value(sample_env_v2().canonical_bytes()),
            "an unknown report format under v2",
            |v| v["checks"][1]["report"]["format"] = json!("tap"),
        ),
        (
            "env",
            value(sample_env_v2().canonical_bytes()),
            "an empty setup command",
            |v| v["checks"][1]["setup"] = json!([[]]),
        ),
        (
            "env",
            value(sample_env_v2().canonical_bytes()),
            "rebuild false written out",
            |v| v["install"]["rebuild"] = json!(false),
        ),
        (
            "run",
            value(sample_run().canonical_bytes()),
            "infra outcome without infra kind",
            |v| v["outcome"] = json!("infra"),
        ),
        (
            "run",
            value(sample_infra_run().canonical_bytes()),
            "sandbox failure with exit code",
            |v| v["exit_code"] = json!(1),
        ),
        (
            "run",
            value(sample_run().canonical_bytes()),
            "index 4",
            |v| v["index"] = json!(4),
        ),
        (
            "receipt",
            value(sample_receipt().canonical_bytes()),
            "missing nullable attempt_id",
            |v| {
                v.as_object_mut().unwrap().remove("attempt_id");
            },
        ),
        (
            "receipt",
            value(sample_receipt().canonical_bytes()),
            "unknown verdict",
            |v| v["verdict"] = json!("MAYBE"),
        ),
        (
            "receipt",
            value(sample_receipt().canonical_bytes()),
            "malformed signature",
            |v| v["signature"]["sig"] = json!("AAAA"),
        ),
        (
            "task",
            value(sample_task().canonical_bytes()),
            "no criteria",
            |v| v["criteria"] = json!([]),
        ),
        (
            "task",
            value(sample_task().canonical_bytes()),
            "timestamp without millis",
            |v| v["created_at"] = json!("2026-10-10T11:00:00Z"),
        ),
        (
            "task",
            value(sample_task().canonical_bytes()),
            "protected paths under v0",
            |v| v["protected_paths"] = json!(["test/**"]),
        ),
        (
            "task",
            value(sample_task_v1().canonical_bytes()),
            "v1 without protected paths",
            |v| {
                v.as_object_mut().unwrap().remove("protected_paths");
            },
        ),
        (
            "task",
            value(sample_task_v1().canonical_bytes()),
            "'**' inside a segment",
            |v| v["protected_paths"] = json!(["test/a**"]),
        ),
        (
            "task",
            value(sample_task_v1().canonical_bytes()),
            "a '..' segment",
            |v| v["protected_paths"] = json!(["../x"]),
        ),
        (
            "task",
            value(sample_task_v1().canonical_bytes()),
            "a leading '/'",
            |v| v["protected_paths"] = json!(["/test"]),
        ),
        (
            "task",
            value(sample_task_v2().canonical_bytes()),
            "criteria under v2",
            |v| v["criteria"] = json!([{"id": "a", "text": "x"}]),
        ),
        (
            "task",
            value(sample_task_v2().canonical_bytes()),
            "explicit empty protected paths under v2",
            |v| v["protected_paths"] = json!([]),
        ),
        (
            "receipt",
            value(sample_receipt_v1().canonical_bytes()),
            "criteria under v1",
            |v| v["criteria"] = json!([{"id": "a", "text": "x"}]),
        ),
        (
            "receipt",
            value(sample_receipt_v1().canonical_bytes()),
            "unproven under v1",
            |v| v["unproven"] = json!(["a"]),
        ),
        (
            "receipt",
            value(sample_receipt_v2().canonical_bytes()),
            "transplant on an entry that proves nothing",
            |v| {
                v["checks"][0]["transplant"] = json!("fail");
                v["checks"][0]["runs"]["transplant"] = v["checks"][0]["runs"]["head"].clone();
            },
        ),
        (
            "receipt",
            value(sample_receipt_v2().canonical_bytes()),
            "transplant without its runs",
            |v| {
                v["checks"][2]["runs"]
                    .as_object_mut()
                    .unwrap()
                    .remove("transplant");
            },
        ),
        (
            "receipt",
            value(sample_receipt_v2().canonical_bytes()),
            "transplant runs without a transplant result",
            |v| {
                v["checks"][2].as_object_mut().unwrap().remove("transplant");
            },
        ),
        (
            "receipt",
            value(sample_receipt_v2().canonical_bytes()),
            "a transplant without head_tests",
            |v| {
                v["checks"][2].as_object_mut().unwrap().remove("head_tests");
                v["checks"][2]["runs"]
                    .as_object_mut()
                    .unwrap()
                    .remove("head_tests");
            },
        ),
        (
            "receipt",
            value(sample_receipt_v2().canonical_bytes()),
            "head_tests without its runs",
            |v| {
                v["checks"][2]["runs"]
                    .as_object_mut()
                    .unwrap()
                    .remove("head_tests");
            },
        ),
        (
            "receipt",
            value(sample_receipt_v2().canonical_bytes()),
            "no executor",
            |v| {
                v.as_object_mut().unwrap().remove("executor");
            },
        ),
        (
            "receipt",
            value(sample_receipt_v2().canonical_bytes()),
            "no config",
            |v| {
                v.as_object_mut().unwrap().remove("config");
            },
        ),
        (
            "receipt",
            value(sample_receipt_v2().canonical_bytes()),
            "no test_changes",
            |v| {
                v.as_object_mut().unwrap().remove("test_changes");
            },
        ),
        (
            "receipt",
            value(sample_receipt_v2().canonical_bytes()),
            "more than 20 listed tests",
            |v| {
                let many: Vec<String> = (0..21).map(|i| format!("unit::t{i:02}")).collect();
                v["test_changes"]["tests_removed"]["items"] = json!(many);
            },
        ),
        (
            "receipt",
            value(sample_receipt().canonical_bytes()),
            "test_changes under v0",
            |v| v["test_changes"] = to_value(&sample_receipt_v2().body.test_changes).unwrap(),
        ),
        (
            "receipt",
            value(sample_receipt_v2().canonical_bytes()),
            "a subject with a newline",
            |v| v["ci"]["subject"] = json!("a\nb"),
        ),
        (
            "receipt",
            value(sample_receipt_v2().canonical_bytes()),
            "a short commit",
            |v| v["ci"]["base_commit"] = json!("abc123"),
        ),
        (
            "receipt",
            value(sample_receipt_v2().canonical_bytes()),
            "a short requested base",
            |v| v["ci"]["requested_base"] = json!("abc123"),
        ),
        (
            "receipt",
            value(sample_receipt_v2().canonical_bytes()),
            "a missing nullable criteria_text",
            |v| {
                v["ci"].as_object_mut().unwrap().remove("criteria_text");
            },
        ),
        (
            "receipt",
            value(sample_receipt().canonical_bytes()),
            "ci under v0",
            |v| v["ci"] = to_value(&sample_receipt_v2().body.ci).unwrap(),
        ),
        (
            "transition",
            value(workspace_transition().canonical_bytes()),
            "git via with workspace fields",
            |v| v["via"]["kind"] = json!("git-import"),
        ),
        (
            "attempt",
            value(sample_attempt().canonical_bytes()),
            "lowercase live id",
            |v| v["attempt"] = json!(v["attempt"].as_str().unwrap().to_ascii_lowercase()),
        ),
        (
            "ledger",
            value(sample_ledger().canonical_bytes()),
            "unknown kind",
            |v| v["kind"] = json!("merge"),
        ),
    ];
    for (kind, mut instance, what, mutate) in cases {
        assert_valid(&validators, kind, &instance);
        let validator = validator(&validators, &instance);
        mutate(&mut instance);
        assert!(
            !validator.is_valid(&instance),
            "{kind} schema accepted an object with {what}"
        );
    }
}

#[test]
fn nullable_fields_accept_null() {
    let validators = validators();
    let mut receipt: Value = serde_json::from_slice(&sample_receipt().canonical_bytes()).unwrap();
    receipt["attempt_id"] = Value::Null;
    assert_valid(&validators, "receipt", &receipt);
}
