mod common;

use std::fmt::Debug;

use aver_core::*;
use common::*;
use serde_json::{json, Value};

fn oracle_id(prefix: &str, body: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(format!("aver:{prefix}\0").as_bytes());
    hasher.update(body);
    let encoded = data_encoding::BASE32_NOPAD
        .encode(hasher.finalize().as_bytes())
        .to_ascii_lowercase();
    format!("{prefix}_{encoded}")
}

fn assert_round_trip<T: Object + PartialEq + Debug>(obj: T) {
    let bytes = obj.canonical_bytes();
    let parsed = T::from_canonical(&bytes).expect("canonical bytes parse");
    assert_eq!(parsed, obj);
    assert_eq!(
        parsed.canonical_bytes(),
        bytes,
        "parse then serialize is identity"
    );
    assert_eq!(
        parsed.id().to_string(),
        oracle_id(obj.id().prefix(), &bytes)
    );
}

#[test]
fn every_object_type_round_trips_and_hashes_its_canonical_bytes() {
    assert_round_trip(sample_tree());
    assert_round_trip(sample_env());
    assert_round_trip(sample_snapshot());
    assert_round_trip(sample_snapshot_v1());
    assert_round_trip(sample_contract());
    assert_round_trip(sample_transition());
    assert_round_trip(workspace_transition());
    assert_round_trip(sample_env_v1());
    assert_round_trip(sample_env_v2());
    assert_round_trip(sample_env_v3());
    assert_round_trip(sample_env_v4());
    assert_round_trip(sample_receipt_v2_uncovered());
    assert_round_trip(sample_task());
    assert_round_trip(sample_task_v1());
    assert_round_trip(sample_task_v2());
    assert_round_trip(sample_run());
    assert_round_trip(sample_infra_run());
    assert_round_trip(sample_receipt());
    assert_round_trip(sample_receipt_v1());
    assert_round_trip(sample_receipt_v2());
    assert_round_trip(sample_receipt_v3());
    assert_round_trip(sample_attempt());
    assert_round_trip(sample_ledger());
}

#[test]
fn ids_carry_the_kind_prefix_used_as_hash_domain() {
    let prefixes: Vec<&str> = all_samples().iter().map(|s| s.1).collect();
    assert_eq!(
        prefixes,
        [
            "tree", "env", "env", "env", "env", "env", "snap", "snap", "tran", "tran", "task",
            "task", "task", "run", "run", "rcpt", "rcpt", "rcpt", "rcpt", "rcpt", "atr", "ledg",
            "ctr"
        ]
    );
    assert_eq!(
        BlobId::of_bytes(b"hello\n").to_string(),
        oracle_id("blob", b"hello\n")
    );
    let pk = key().verifying_key();
    assert_eq!(
        AgentId::of_public_key(&pk).to_string(),
        oracle_id("agent", pk.as_bytes())
    );
}

#[test]
fn same_bytes_under_different_kinds_have_different_ids() {
    let tree = Tree::new(Set::empty());
    let bytes = tree.canonical_bytes();
    assert_ne!(BlobId::of_bytes(&bytes).as_bytes(), tree.id().as_bytes());
}

#[test]
fn empty_tree_has_a_fixed_encoding() {
    let tree = Tree::new(Set::empty());
    assert_eq!(
        String::from_utf8(tree.canonical_bytes()).unwrap(),
        r#"{"entries":[],"schema":"aver.tree/v0"}"#
    );
}

fn to_value<T: Object>(obj: &T) -> Value {
    serde_json::from_slice(&obj.canonical_bytes()).unwrap()
}

fn canonical(value: &Value) -> Vec<u8> {
    canonical::to_canonical(value).unwrap()
}

fn assert_rejected<T: Object + Debug>(bytes: &[u8], what: &str) {
    let result = T::from_canonical(bytes);
    assert!(
        result.is_err(),
        "{what} must be rejected, parsed as {result:?} from {}",
        String::from_utf8_lossy(bytes)
    );
}

#[test]
fn extra_fields_are_rejected_at_every_level() {
    let mut tree = to_value(&sample_tree());
    tree["extra"] = json!(1);
    assert_rejected::<Tree>(&canonical(&tree), "top-level extra field");

    let mut tree = to_value(&sample_tree());
    tree["entries"][0]["mode"] = json!("100644");
    assert_rejected::<Tree>(&canonical(&tree), "extra field in tree entry");

    let mut env = to_value(&sample_env());
    env["limits"]["gpus"] = json!(1);
    assert_rejected::<Environment>(&canonical(&env), "extra field in limits");

    let mut receipt = to_value(&sample_receipt());
    receipt["checks"][0]["note"] = json!("x");
    assert_rejected::<Receipt>(&canonical(&receipt), "extra field in receipt check");

    let mut receipt = to_value(&sample_receipt());
    receipt["signature"]["extra"] = json!("x");
    assert_rejected::<Receipt>(&canonical(&receipt), "extra field in signature");

    let mut transition = to_value(&workspace_transition());
    transition["via"]["repo"] = json!("x");
    assert_rejected::<Transition>(&canonical(&transition), "extra field in via");
}

#[test]
fn non_canonical_encodings_of_valid_objects_are_rejected() {
    let tree = sample_tree();
    let pretty = serde_json::to_vec_pretty(&tree).unwrap();
    assert_eq!(Tree::from_canonical(&pretty), Err(ParseError::NonCanonical));

    let reordered = String::from_utf8(tree.canonical_bytes())
        .unwrap()
        .replacen(
            r#"{"entries":"#,
            r#"{"schema":"aver.tree/v0","entries":"#,
            1,
        )
        .replacen(r#","schema":"aver.tree/v0"}"#, "}", 1);
    assert!(reordered.starts_with(r#"{"schema""#));
    assert_eq!(
        Tree::from_canonical(reordered.as_bytes()),
        Err(ParseError::NonCanonical),
        "keys out of order"
    );

    let mut env = to_value(&sample_env());
    env["lockfile"] = Value::Null;
    assert_eq!(
        Environment::from_canonical(&canonical(&env)),
        Err(ParseError::NonCanonical),
        "explicit null for an omitted optional field"
    );

    let mut env = to_value(&sample_env());
    env["checks"][0]["cwd"] = Value::Null;
    assert_eq!(
        Environment::from_canonical(&canonical(&env)),
        Err(ParseError::NonCanonical)
    );

    let mut task = to_value(&sample_task());
    task["checks"] = json!([]);
    assert_eq!(
        Task::from_canonical(&canonical(&task)),
        Err(ParseError::NonCanonical),
        "empty optional array present"
    );

    let escaped = String::from_utf8(sample_ledger().canonical_bytes())
        .unwrap()
        .replacen("Rejected attempt", "Rejected \\u0061ttempt", 1);
    assert_eq!(
        LedgerEntry::from_canonical(escaped.as_bytes()),
        Err(ParseError::NonCanonical),
        "unnecessary string escape"
    );
}

#[test]
fn duplicate_keys_are_rejected() {
    let bytes = String::from_utf8(sample_snapshot().canonical_bytes()).unwrap();
    let doubled = bytes.replacen("{", &format!(r#"{{"env":"{}","#, sample_env().id()), 1);
    assert_rejected::<Snapshot>(doubled.as_bytes(), "duplicate key");

    let mut env = to_value(&sample_env());
    env["vars"] = json!({"A": "1"});
    let with_dup = String::from_utf8(canonical(&env)).unwrap().replacen(
        r#""vars":{"A":"1"}"#,
        r#""vars":{"A":"1","A":"2"}"#,
        1,
    );
    assert_rejected::<Environment>(with_dup.as_bytes(), "duplicate map key");
}

#[test]
fn floats_are_rejected_even_when_integral() {
    let run = String::from_utf8(sample_run().canonical_bytes()).unwrap();
    for replacement in [r#""duration_ms":1234.0"#, r#""duration_ms":1.234e3"#] {
        let bytes = run.replacen(r#""duration_ms":1234"#, replacement, 1);
        assert_ne!(bytes, run);
        assert_rejected::<CheckRun>(bytes.as_bytes(), replacement);
    }
    let mut env = to_value(&sample_env());
    env["limits"]["cpu_millis"] = json!(2000.5);
    assert!(canonical::to_canonical(&env).is_err());
}

#[test]
fn integers_outside_their_field_range_are_rejected() {
    for bad in [json!(-1), json!(4_294_967_296u64)] {
        let mut env = to_value(&sample_env());
        env["limits"]["pids"] = bad.clone();
        assert_rejected::<Environment>(&canonical(&env), &format!("pids = {bad}"));
    }
}

#[test]
fn unsorted_or_duplicate_sets_are_rejected() {
    let mut tree = to_value(&sample_tree());
    tree["entries"].as_array_mut().unwrap().reverse();
    assert_rejected::<Tree>(&canonical(&tree), "tree entries out of order");

    let mut tree = to_value(&sample_tree());
    let first = tree["entries"][0].clone();
    tree["entries"].as_array_mut().unwrap().insert(0, first);
    assert_rejected::<Tree>(&canonical(&tree), "duplicate tree entry");

    let mut env = to_value(&sample_env());
    env["checks"].as_array_mut().unwrap().reverse();
    assert_rejected::<Environment>(&canonical(&env), "env checks out of order");

    let mut task = to_value(&sample_task());
    task["checks"][0]["proves"] = json!(["div-zero", "div-zero"]);
    assert_rejected::<Task>(&canonical(&task), "duplicate proves");

    let mut task = to_value(&sample_task());
    task["checks"][0]["proves"] = json!(["docs", "div-zero"]);
    assert_rejected::<Task>(&canonical(&task), "unsorted proves");

    let mut receipt = to_value(&sample_receipt());
    receipt["checks"].as_array_mut().unwrap().reverse();
    assert_rejected::<Receipt>(&canonical(&receipt), "receipt checks out of order");

    let mut run = to_value(&sample_run());
    run["tests"].as_array_mut().unwrap().reverse();
    assert_rejected::<CheckRun>(&canonical(&run), "run tests out of order");
}

#[test]
fn ordered_arrays_keep_their_order() {
    let mut task = sample_task().body;
    let reversed: Vec<Criterion> = task.criteria.iter().rev().cloned().collect();
    task.criteria = Criteria::new(reversed).unwrap();
    let signed = Task::sign(task, &key());
    assert_round_trip(signed.clone());
    assert_ne!(signed.id(), sample_task().id());
}

#[test]
fn set_new_sorts_and_drops_exact_duplicates_but_rejects_key_collisions() {
    let a = || CriterionId::try_from("a".to_string()).unwrap();
    let b = || CriterionId::try_from("b".to_string()).unwrap();
    let set = Set::new(vec![b(), a(), b()]).unwrap();
    assert_eq!(set.as_slice(), &[a(), b()]);

    let entry = |blob: &[u8]| TreeEntry {
        name: p("x"),
        node: Node::File(BlobId::of_bytes(blob)),
    };
    assert!(Set::new(vec![entry(b"1"), entry(b"2")]).is_err());
}

#[test]
fn wrong_or_unknown_schema_versions_are_rejected() {
    let mut tree = to_value(&sample_tree());
    tree["schema"] = json!("aver.tree/v1");
    assert_rejected::<Tree>(&canonical(&tree), "future version");

    let snapshot = to_value(&sample_snapshot());
    assert_rejected::<Tree>(&canonical(&snapshot), "snapshot parsed as tree");

    let mut tree = to_value(&sample_tree());
    tree.as_object_mut().unwrap().remove("schema");
    assert_rejected::<Tree>(&canonical(&tree), "missing schema");
}

#[test]
fn tree_entry_id_kind_must_match_entry_kind() {
    let mut tree = to_value(&sample_tree());
    let entries = tree["entries"].as_array_mut().unwrap();
    let src = entries.iter_mut().find(|e| e["name"] == "src").unwrap();
    src["kind"] = json!("file");
    assert_rejected::<Tree>(&canonical(&tree), "file entry with a tree id");

    let mut tree = to_value(&sample_tree());
    tree["entries"][0]["kind"] = json!("gitlink");
    assert_rejected::<Tree>(&canonical(&tree), "unknown entry kind");
}

#[test]
fn check_run_infra_and_exit_code_presence_is_enforced() {
    let cases = [
        (json!("infra"), None, Some(json!(1))),
        (json!("pass"), Some(json!("oom")), Some(json!(0))),
        (json!("pass"), None, None),
        (json!("infra"), Some(json!("sandbox")), Some(json!(137))),
        (json!("infra"), Some(json!("oom")), None),
    ];
    for (outcome, infra, exit_code) in cases {
        let mut run = to_value(&sample_run());
        let map = run.as_object_mut().unwrap();
        map.insert("outcome".into(), outcome.clone());
        map.remove("infra");
        map.remove("exit_code");
        if let Some(infra) = &infra {
            map.insert("infra".into(), infra.clone());
        }
        if let Some(code) = &exit_code {
            map.insert("exit_code".into(), code.clone());
        }
        assert_rejected::<CheckRun>(
            &canonical(&run),
            &format!("outcome {outcome} infra {infra:?} exit_code {exit_code:?}"),
        );
    }

    let sandbox = String::from_utf8(sample_infra_run().canonical_bytes()).unwrap();
    assert!(sandbox.contains(r#""infra":"sandbox""#));
    assert!(!sandbox.contains("exit_code"));
}

#[test]
fn nullable_fields_must_be_present() {
    let receipt = to_value(&sample_receipt());
    let mut missing = receipt.clone();
    missing.as_object_mut().unwrap().remove("attempt_id");
    assert_rejected::<Receipt>(&canonical(&missing), "missing nullable attempt_id");

    let ledger = String::from_utf8(sample_ledger().canonical_bytes()).unwrap();
    assert!(ledger.contains(r#""prev":null"#));
    let mut missing = to_value(&sample_ledger());
    missing.as_object_mut().unwrap().remove("prev");
    assert_rejected::<LedgerEntry>(&canonical(&missing), "missing nullable prev");

    let receipt = String::from_utf8(sample_receipt().canonical_bytes()).unwrap();
    assert!(receipt.contains(r#""cause":null"#));
}

#[test]
fn secret_looking_env_var_names_are_rejected() {
    for name in [
        "GITHUB_TOKEN",
        "db_password",
        "MY_API_KEY",
        "PrivateThing",
        "AUTH",
    ] {
        let mut env = to_value(&sample_env());
        env["vars"] = json!({ name: "x" });
        assert_rejected::<Environment>(&canonical(&env), name);
    }
}

#[test]
fn signatures_verify_against_the_named_key_only() {
    let transition = sample_transition();
    let pk = key().verifying_key();
    assert_eq!(transition.verify(&pk), Ok(()));
    assert!(matches!(
        transition.verify(&other_key().verifying_key()),
        Err(SignatureError::KeyMismatch { .. })
    ));

    let mut tampered = transition.clone();
    tampered.body.message = "Something else".into();
    assert_eq!(tampered.verify(&pk), Err(SignatureError::Invalid));

    let reparsed = Transition::from_canonical(&transition.canonical_bytes()).unwrap();
    assert_eq!(reparsed.verify(&pk), Ok(()));
}

#[test]
fn signing_message_is_domain_separated_by_kind() {
    let task = sample_task();
    let message = Task::signing_message(&task.body);
    let body_canonical = canonical(&serde_json::to_value(&task.body).unwrap());
    let mut expected = b"aver-sig:v0\0task\0".to_vec();
    expected.extend_from_slice(&body_canonical);
    assert_eq!(message, expected);
    assert!(!body_canonical.windows(9).any(|w| w == b"signature"));

    let receipt = sample_receipt();
    assert!(Receipt::signing_message(&receipt.body).starts_with(b"aver-sig:v0\0rcpt\0"));
    let ledger = sample_ledger();
    assert!(LedgerEntry::signing_message(&ledger.body).starts_with(b"aver-sig:v0\0ledg\0"));
    let transition = sample_transition();
    assert!(Transition::signing_message(&transition.body).starts_with(b"aver-sig:v0\0tran\0"));
}

#[test]
fn signed_object_ids_commit_to_the_signer() {
    let by_server = sample_ledger();
    let by_other = LedgerEntry::sign(by_server.body.clone(), &other_key());
    assert_ne!(by_server.id(), by_other.id());
    let bytes = String::from_utf8(by_server.canonical_bytes()).unwrap();
    assert!(bytes.contains(r#""signature":{"alg":"ed25519","key":"agent_"#));
}

#[test]
fn signed_objects_without_a_signature_are_rejected() {
    let mut value = to_value(&sample_transition());
    value.as_object_mut().unwrap().remove("signature");
    assert_rejected::<Transition>(&canonical(&value), "missing signature");

    let mut value = to_value(&sample_transition());
    value["signature"]["alg"] = json!("rsa");
    assert_rejected::<Transition>(&canonical(&value), "unknown algorithm");

    let mut value = to_value(&sample_transition());
    value["signature"]["sig"] = json!("AAAA");
    assert_rejected::<Transition>(&canonical(&value), "short signature");
}

#[test]
fn task_criteria_are_unique_and_empty_only_under_v2() {
    let mut task = to_value(&sample_task());
    task["criteria"] = json!([]);
    assert_rejected::<Task>(&canonical(&task), "no criteria under v0");

    let mut task = to_value(&sample_task_v1());
    task["criteria"] = json!([]);
    task.as_object_mut().unwrap().remove("checks");
    assert_rejected::<Task>(&canonical(&task), "no criteria under v1");

    let mut task = to_value(&sample_task());
    task["criteria"] = json!([{"id": "a", "text": "x"}, {"id": "a", "text": "y"}]);
    assert_rejected::<Task>(&canonical(&task), "duplicate criterion ids");

    let mut task = to_value(&sample_task());
    task["title"] = json!("x".repeat(201));
    assert_rejected::<Task>(&canonical(&task), "title over 200 bytes");
}

/// v1 exists only to carry protected paths, so each encoding of a task has
/// exactly one version: v0 never has them, v1 always does.
#[test]
fn task_versions_are_tied_to_protected_paths() {
    let v0 = sample_task();
    assert_eq!(to_value(&v0)["schema"], "aver.task/v0");
    assert!(to_value(&v0).get("protected_paths").is_none());
    let v1 = sample_task_v1();
    assert_eq!(to_value(&v1)["schema"], "aver.task/v1");
    assert_eq!(
        to_value(&v1)["protected_paths"],
        json!(["package.json", "test/**"])
    );
    assert_ne!(v0.id(), v1.id());

    let mut task = to_value(&v0);
    task["protected_paths"] = json!(["test/**"]);
    assert_rejected::<Task>(&canonical(&task), "protected paths under v0");

    let mut task = to_value(&v1);
    task.as_object_mut().unwrap().remove("protected_paths");
    assert_rejected::<Task>(&canonical(&task), "v1 without protected paths");

    let mut task = to_value(&v1);
    task["protected_paths"] = json!([]);
    assert_rejected::<Task>(&canonical(&task), "explicit empty protected paths");

    let mut task = to_value(&v1);
    task["protected_paths"] = json!(["test/**", "package.json"]);
    assert_rejected::<Task>(&canonical(&task), "unsorted protected paths");

    for bad in [
        "", "/test", "test/", "a//b", "../x", "a/./b", "a**", "a/**b/c", "a\\b",
    ] {
        let mut task = to_value(&v1);
        task["protected_paths"] = json!([bad]);
        assert_rejected::<Task>(&canonical(&task), &format!("glob {bad:?}"));
    }

    let mut task = to_value(&v1);
    task["protected_paths"] = json!((0..65).map(|i| format!("p{i:02}")).collect::<Vec<_>>());
    assert_rejected::<Task>(&canonical(&task), "65 protected paths");

    let mut task = to_value(&v1);
    task["schema"] = json!("aver.task/v3");
    assert_rejected::<Task>(&canonical(&task), "unknown task version");
}

/// v2 exists only to carry an empty criteria list (regression-only mode), so
/// it never has criteria and v0 and v1 tasks keep their encodings.
#[test]
fn task_v2_is_exactly_the_task_without_criteria() {
    let v2 = sample_task_v2();
    assert_eq!(to_value(&v2)["schema"], "aver.task/v2");
    assert_eq!(to_value(&v2)["criteria"], json!([]));
    assert_eq!(
        to_value(&v2)["protected_paths"],
        json!(["package.json", "test/**"])
    );

    let mut bare = to_value(&v2);
    bare.as_object_mut().unwrap().remove("protected_paths");
    let bare = canonical(&bare);
    let parsed = Task::from_canonical(&bare).expect("v2 without protected paths parses");
    assert_eq!(parsed.body.schema, TaskSchema::V2);

    let mut task = to_value(&v2);
    task["criteria"] = json!([{"id": "a", "text": "x"}]);
    assert_rejected::<Task>(&canonical(&task), "criteria under v2");

    let mut task = to_value(&v2);
    task.as_object_mut().unwrap().remove("criteria");
    assert_rejected::<Task>(&canonical(&task), "v2 without a criteria field");
}

#[test]
fn receipt_v1_is_exactly_the_receipt_without_criteria() {
    let v0 = sample_receipt();
    let v1 = sample_receipt_v1();
    assert_eq!(to_value(&v0)["schema"], "aver.receipt/v0");
    assert_eq!(to_value(&v1)["schema"], "aver.receipt/v1");
    assert_eq!(to_value(&v1)["criteria"], json!([]));

    let mut receipt = to_value(&v0);
    receipt["criteria"] = json!([]);
    receipt["unproven"] = json!([]);
    assert_rejected::<Receipt>(&canonical(&receipt), "no criteria under v0");

    let mut receipt = to_value(&v1);
    receipt["criteria"] = json!([{"id": "a", "text": "x"}]);
    assert_rejected::<Receipt>(&canonical(&receipt), "criteria under v1");

    let mut receipt = to_value(&v1);
    receipt["schema"] = json!("aver.receipt/v4");
    assert_rejected::<Receipt>(&canonical(&receipt), "unknown receipt version");
}

/// v3 is v2 plus where the runs executed and how each check was read: a
/// native receipt names its host profile and its executor is that
/// profile's, a sandboxed one names none, and neither field may appear in
/// an older version (so a v2 reader fails closed on a native receipt).
#[test]
fn receipt_v3_records_provenance_and_levels_strictly() {
    let v3 = sample_receipt_v3();
    let value = to_value(&v3);
    assert_eq!(value["schema"], "aver.receipt/v3");
    assert_eq!(v3.body.schema.proof_rule(), ProofRule::Transplant);
    assert_eq!(value["provenance"]["mode"], "local");
    assert_eq!(value["provenance"]["network"], "off");
    assert_eq!(value["provenance"]["uid"], "same");
    assert_eq!(
        value["levels"],
        json!({ "typecheck": "native", "unit": "junit" })
    );
    let profile = sample_host_profile();
    assert_eq!(
        value["executor"],
        format!("native-v0.{}", &profile.as_str()[..16])
    );

    let edit = |f: &dyn Fn(&mut Value)| {
        let mut v = value.clone();
        f(&mut v);
        canonical(&v)
    };
    assert_rejected::<Receipt>(
        &edit(&|v| {
            v.as_object_mut().unwrap().remove("provenance");
        }),
        "a v3 receipt without provenance",
    );
    assert_rejected::<Receipt>(
        &edit(&|v| {
            v.as_object_mut().unwrap().remove("levels");
        }),
        "a v3 receipt without levels",
    );
    assert_rejected::<Receipt>(
        &edit(&|v| {
            v["provenance"]
                .as_object_mut()
                .unwrap()
                .remove("host_profile");
        }),
        "a local receipt without a host profile",
    );
    assert_rejected::<Receipt>(
        &edit(&|v| v["executor"] = json!("docker-runsc-v0")),
        "a local receipt under another executor",
    );
    assert_rejected::<Receipt>(
        &edit(&|v| v["provenance"]["mode"] = json!("sandbox")),
        "a sandboxed receipt naming a host profile",
    );
    assert_rejected::<Receipt>(
        &edit(&|v| v["provenance"]["mode"] = json!("remote")),
        "an unknown mode",
    );
    assert_rejected::<Receipt>(
        &edit(&|v| v["provenance"]["extra"] = json!(1)),
        "an unknown provenance field",
    );
    assert_rejected::<Receipt>(
        &edit(&|v| v["levels"]["unit"] = json!("tap")),
        "an unknown level",
    );
    assert_rejected::<Receipt>(
        &edit(&|v| v["provenance"]["host_profile"] = json!("ABC")),
        "a host profile that is not 64 lowercase hex digits",
    );
    assert_rejected::<Receipt>(
        &edit(&|v| v["skipped"] = json!([{ "check": "typecheck", "reason": "not_affected" }])),
        "a skipped check with a level",
    );
    assert_rejected::<Receipt>(
        &edit(&|v| v["schema"] = json!("aver.receipt/v2")),
        "provenance in a v2 receipt",
    );

    let sandboxed = edit(&|v| {
        v["provenance"] = json!({ "mode": "sandbox", "network": "off", "uid": "separate" });
        v["executor"] = json!("docker-runsc-v0");
    });
    let parsed = Receipt::from_canonical(&sandboxed).expect("a sandboxed v3 receipt");
    assert_eq!(
        parsed.body.provenance.as_ref().map(|p| p.mode),
        Some(ProvenanceMode::Sandbox)
    );
}

/// The host profile is a pure function of the OS, the CPU and each tool's
/// name, real path and version output; any of them changes it, and so the
/// native executor id every run is cached under.
#[test]
fn the_host_profile_follows_every_tool() {
    let tool = |name: &str, path: &str, version: &str| HostTool {
        name: name.into(),
        path: path.into(),
        version: version.as_bytes().to_vec(),
    };
    let base = [
        tool("cargo", "/r/1.90/bin/cargo", "cargo 1.90.0"),
        tool("node", "/n/24/bin/node", "v24.2.0"),
    ];
    let a = HostProfile::of(HostOs::Darwin, HostArch::Arm64, &base);
    let reordered = [base[1].clone(), base[0].clone()];
    assert_eq!(
        a,
        HostProfile::of(HostOs::Darwin, HostArch::Arm64, &reordered)
    );
    let variants = [
        HostProfile::of(HostOs::Linux, HostArch::Arm64, &base),
        HostProfile::of(HostOs::Darwin, HostArch::Amd64, &base),
        HostProfile::of(
            HostOs::Darwin,
            HostArch::Arm64,
            &[base[0].clone(), tool("node", "/n/24/bin/node", "v24.3.0")],
        ),
        HostProfile::of(
            HostOs::Darwin,
            HostArch::Arm64,
            &[base[0].clone(), tool("node", "/n/25/bin/node", "v24.2.0")],
        ),
        HostProfile::of(HostOs::Darwin, HostArch::Arm64, &base[..1]),
    ];
    for v in &variants {
        assert_ne!(&a, v);
        assert_ne!(a.executor(), v.executor());
    }
    assert!(a.executor().starts_with(NATIVE_EXECUTOR_PREFIX));
    assert_eq!(a.executor().len(), NATIVE_EXECUTOR_PREFIX.len() + 16);
    assert_eq!(a.as_str().parse::<HostProfile>().unwrap(), a);
}

/// v2 takes criteria or none, and is the only version with the transplant
/// fields and the CI object, which must be consistent with each other.
#[test]
fn receipt_v2_carries_the_transplant_and_ci_fields_strictly() {
    let v2 = sample_receipt_v2();
    assert_eq!(to_value(&v2)["schema"], "aver.receipt/v2");
    assert_eq!(v2.body.schema.proof_rule(), ProofRule::Transplant);
    assert_eq!(
        sample_receipt().body.schema.proof_rule(),
        ProofRule::HeadPasses
    );
    let value = to_value(&v2);
    assert_eq!(value["ci"]["config_path"], "aver.toml");
    assert_eq!(value["ci"]["subject"], "example/calc#7");

    let mut receipt = value.clone();
    receipt["criteria"] = json!([]);
    receipt["unproven"] = json!([]);
    for c in receipt["checks"].as_array_mut().unwrap() {
        c["proves"] = json!([]);
        c.as_object_mut().unwrap().remove("transplant");
        c.as_object_mut().unwrap().remove("head_tests");
        c["runs"].as_object_mut().unwrap().remove("transplant");
        c["runs"].as_object_mut().unwrap().remove("head_tests");
    }
    receipt.as_object_mut().unwrap().remove("signature");
    let body: ReceiptBody = serde_json::from_value(receipt).unwrap();
    assert!(
        body.criteria.is_empty(),
        "v2 takes a regression-only receipt"
    );

    let mut receipt = value.clone();
    receipt["schema"] = json!("aver.receipt/v0");
    assert_rejected::<Receipt>(&canonical(&receipt), "v2 fields under v0");

    let mut receipt = value.clone();
    receipt["checks"][0]["transplant"] = json!("fail");
    receipt["checks"][0]["runs"]["transplant"] = receipt["checks"][0]["runs"]["head"].clone();
    assert_rejected::<Receipt>(
        &canonical(&receipt),
        "transplant on an entry that proves nothing",
    );

    let mut receipt = value.clone();
    receipt["checks"][2]["runs"]["transplant"] = json!([]);
    assert_rejected::<Receipt>(&canonical(&receipt), "explicit empty transplant runs");

    let mut receipt = value.clone();
    receipt["checks"][2]
        .as_object_mut()
        .unwrap()
        .remove("transplant");
    assert_rejected::<Receipt>(&canonical(&receipt), "transplant runs without a result");

    let mut receipt = value.clone();
    receipt["checks"][2]["transplant"] = json!("maybe");
    assert_rejected::<Receipt>(&canonical(&receipt), "unknown transplant result");

    let mut receipt = value.clone();
    receipt["checks"][2]
        .as_object_mut()
        .unwrap()
        .remove("head_tests");
    receipt["checks"][2]["runs"]
        .as_object_mut()
        .unwrap()
        .remove("head_tests");
    assert_rejected::<Receipt>(&canonical(&receipt), "a transplant without head_tests");

    let mut receipt = value.clone();
    receipt["checks"][2]["runs"]
        .as_object_mut()
        .unwrap()
        .remove("head_tests");
    assert_rejected::<Receipt>(&canonical(&receipt), "head_tests without its runs");

    for field in ["executor", "config", "test_changes"] {
        let mut receipt = value.clone();
        receipt.as_object_mut().unwrap().remove(field);
        assert_rejected::<Receipt>(&canonical(&receipt), &format!("v2 without {field}"));
        let mut receipt = to_value(&sample_receipt());
        receipt[field] = value[field].clone();
        assert_rejected::<Receipt>(&canonical(&receipt), &format!("{field} under v0"));
    }
    for bad in [json!(""), json!("docker runc"), json!("x".repeat(65))] {
        let mut receipt = value.clone();
        receipt["executor"] = bad.clone();
        assert_rejected::<Receipt>(&canonical(&receipt), &format!("executor {bad}"));
    }
    let mut receipt = value.clone();
    receipt["ci"]["requested_base"] = receipt["ci"]["base_commit"].clone();
    assert_rejected::<Receipt>(
        &canonical(&receipt),
        "a requested base equal to the base commit",
    );
    let mut receipt = value.clone();
    receipt["ci"]["requested_base"] = json!("3".repeat(40));
    Receipt::from_canonical(&canonical(&receipt)).expect("a requested base that differs");
    let mut receipt = value.clone();
    receipt["config"] = json!("strata.toml");
    assert_rejected::<Receipt>(&canonical(&receipt), "an unknown config source");
    let mut receipt = value.clone();
    receipt["test_changes"]["files_added"]["items"] = json!(["b.test.js", "a.test.js"]);
    assert_rejected::<Receipt>(&canonical(&receipt), "unsorted test_changes items");
    let mut receipt = value.clone();
    let many: Vec<String> = (0..21).map(|i| format!("t{i:02}.test.js")).collect();
    receipt["test_changes"]["files_added"]["items"] = json!(many);
    assert_rejected::<Receipt>(&canonical(&receipt), "more than 20 listed items");
    let listed = Listed::of(
        (0..25)
            .map(|i| format!("t{i:02}.test.js").parse().unwrap())
            .collect::<Vec<Path>>(),
    );
    assert_eq!(
        (listed.items.len(), listed.omitted, listed.total()),
        (20, 5, 25)
    );

    for bad in [json!(""), json!("tab\there"), json!("x".repeat(257))] {
        let mut receipt = value.clone();
        receipt["ci"]["subject"] = bad.clone();
        assert_rejected::<Receipt>(&canonical(&receipt), &format!("subject {bad}"));
    }
    let mut receipt = value.clone();
    receipt["ci"]["subject"] = json!("é".repeat(128));
    assert!(
        Receipt::from_canonical(&canonical(&receipt)).is_ok(),
        "256 bytes is the limit"
    );

    let mut receipt = value.clone();
    receipt["ci"].as_object_mut().unwrap().remove("config_path");
    assert_rejected::<Receipt>(&canonical(&receipt), "config_path omitted instead of null");

    let mut receipt = value;
    receipt["ci"]["criteria_text"] = Value::Null;
    receipt["ci"]["config_path"] = Value::Null;
    assert!(
        Receipt::from_canonical(&canonical(&receipt)).is_ok(),
        "both nullable"
    );
}

#[test]
fn deps_key_hashes_the_canonical_layer_identity_under_its_own_domain() {
    let env = sample_env();
    let lockfile = env.lockfile.as_ref().unwrap();
    let body = format!(
        r#"{{"image":"{}","install":{{"allow_scripts":false,"argv":["npm","ci","--ignore-scripts"]}},"lockfile":"{}","platform":"linux/arm64"}}"#,
        env.image.digest, lockfile.blob
    );
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"aver:deps\0");
    hasher.update(body.as_bytes());
    assert_eq!(
        env.deps_key(None).as_deref(),
        Some(hasher.finalize().to_hex().as_str())
    );

    let mut moved = env.clone();
    moved.lockfile.as_mut().unwrap().path = p("app/package-lock.json");
    assert_eq!(
        moved.deps_key(None),
        env.deps_key(None),
        "the path is not identity"
    );

    let manifest = BlobId::of_bytes(b"{\"scripts\":{\"postinstall\":\"x\"}}");
    assert_eq!(
        env.deps_key(Some(&manifest)),
        env.deps_key(None),
        "without scripts the manifest does not shape the layer"
    );
    let mut scripted = env.clone();
    scripted.install.allow_scripts = true;
    let body = format!(
        r#"{{"image":"{}","install":{{"allow_scripts":true,"argv":["npm","ci","--ignore-scripts"]}},"lockfile":"{}","manifest":"{manifest}","platform":"linux/arm64"}}"#,
        env.image.digest, lockfile.blob
    );
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"aver:deps\0");
    hasher.update(body.as_bytes());
    assert_eq!(
        scripted.deps_key(Some(&manifest)).as_deref(),
        Some(hasher.finalize().to_hex().as_str()),
        "with scripts the manifest's lifecycle scripts run, so it is identity"
    );
    assert_ne!(scripted.deps_key(None), scripted.deps_key(Some(&manifest)));

    let mut none = env;
    none.lockfile = None;
    assert_eq!(none.deps_key(None), None);
}

#[test]
fn build_cache_key_hashes_platform_inputs_and_command_under_its_own_domain() {
    use aver_core::env::{BuildCacheKey, BuildPlatform};
    let env = sample_env();
    let check = env.checks.iter().find(|c| c.id.as_str() == "unit").unwrap();
    let digest = env.image.digest.clone();
    let (lock, a, b) = (
        BlobId::of_bytes(b"lock"),
        BlobId::of_bytes(b"a"),
        BlobId::of_bytes(b"b"),
    );
    let key = |env: &Environment, check: &CheckDef, platform: BuildPlatform, root: &str| {
        BuildCacheKey::of_check(
            env,
            check,
            platform,
            vec![lock],
            vec![("b/Cargo.toml".into(), b), ("Cargo.toml".into(), a)],
            root,
        )
        .hex()
    };
    let image = || BuildPlatform::ImageDigest {
        digest: digest.clone(),
        platform: Platform::LinuxArm64,
    };
    let body = format!(
        r#"{{"argv":["node","--test"],"cwd":null,"lockfiles":["{lock}"],"manifests":[["Cargo.toml","{a}"],["b/Cargo.toml","{b}"]],"platform":{{"digest":"{digest}","kind":"image_digest","platform":"linux/arm64"}},"protocol":1,"root":"","setup":[],"vars":{{"NODE_OPTIONS":"--max-old-space-size=1024"}}}}"#
    );
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"aver:buildcache\0");
    hasher.update(body.as_bytes());
    let base = key(&env, check, image(), "");
    assert_eq!(
        base,
        hasher.finalize().to_hex().as_str(),
        "manifests are a set"
    );

    let mut other = check.clone();
    other.argv.push("--release".into());
    assert_ne!(
        key(&env, &other, image(), ""),
        base,
        "the command shapes artifacts"
    );
    let mut timed = check.clone();
    timed.timeout_ms = Some(1);
    assert_eq!(key(&env, &timed, image(), ""), base, "the timeout does not");
    assert_ne!(
        key(&env, check, image(), "rust"),
        base,
        "build scripts record absolute paths"
    );
    let host = BuildPlatform::HostProfile {
        profile: "darwin arm64 rustc 1.94.0".into(),
    };
    assert_ne!(
        key(&env, check, host, ""),
        base,
        "a native cache is its own"
    );
    let mut flagged = env.clone();
    flagged.vars.insert(p("RUSTFLAGS"), "-Copt-level=1".into());
    assert_ne!(
        key(&flagged, check, image(), ""),
        base,
        "variables reach rustc"
    );
}

/// v1 exists only to carry declared test paths, so every v0 environment
/// keeps its bytes and id, and each environment has one encoding.
#[test]
fn env_versions_are_tied_to_declared_tests() {
    let v0 = sample_env();
    assert_eq!(to_value(&v0)["schema"], "aver.env/v0");
    assert!(to_value(&v0).get("tests").is_none());
    assert_eq!(
        v0.test_globs().len(),
        aver_core::env::DEFAULT_TEST_GLOBS.len()
    );
    assert!(v0.is_test_file("src/a.test.js") && v0.is_test_file("pkg/test/x.js"));
    assert!(!v0.is_test_file("src/a.js"));
    let v1 = sample_env_v1();
    assert_eq!(to_value(&v1)["schema"], "aver.env/v1");
    assert_eq!(to_value(&v1)["tests"], json!(["**/*.spec.ts", "test/**"]));
    assert!(v1.is_test_file("test/a.js") && !v1.is_test_file("src/a.test.js"));

    let mut env = to_value(&v0);
    env["tests"] = json!(["test/**"]);
    assert_rejected::<Environment>(&canonical(&env), "tests under v0");
    let mut env = to_value(&v1);
    env.as_object_mut().unwrap().remove("tests");
    assert_rejected::<Environment>(&canonical(&env), "v1 without tests");
    let mut env = to_value(&v1);
    env["tests"] = json!([]);
    assert_rejected::<Environment>(&canonical(&env), "explicit empty tests");
    let mut env = to_value(&v1);
    env["tests"] = json!(["test/**", "**/*.spec.ts"]);
    assert_rejected::<Environment>(&canonical(&env), "unsorted tests");
}

#[test]
fn env_v2_carries_rebuild_setup_and_other_report_formats_exactly_when_used() {
    let v2 = sample_env_v2();
    let value = to_value(&v2);
    assert_eq!(value["schema"], "aver.env/v2");
    assert_eq!(value["install"]["rebuild"], json!(true));
    assert_eq!(
        value["checks"][1]["setup"],
        json!([["npm", "run", "build"]])
    );
    assert_eq!(value["checks"][1]["report"]["format"], "jest-json");
    assert!(v2.rebuilds());

    let v0 = to_value(&sample_env());
    assert!(v0["install"].get("rebuild").is_none());
    for (what, mutate) in [
        (
            "rebuild under v0",
            (|v: &mut serde_json::Value| v["install"]["rebuild"] = json!(true))
                as fn(&mut serde_json::Value),
        ),
        ("setup under v0", |v| {
            v["checks"][0]["setup"] = json!([["true"]])
        }),
        ("a jest-json report under v0", |v| {
            v["checks"][1]["report"]["format"] = json!("jest-json")
        }),
    ] {
        let mut env = v0.clone();
        mutate(&mut env);
        assert_rejected::<Environment>(&canonical(&env), what);
        env["schema"] = json!("aver.env/v2");
        Environment::from_canonical(&canonical(&env))
            .unwrap_or_else(|e| panic!("{what} under v2: {e}"));
    }

    let mut env = value.clone();
    env["install"]["rebuild"] = json!(false);
    assert_rejected::<Environment>(&canonical(&env), "an explicit rebuild = false");
    let mut env = value.clone();
    env["checks"][1]["setup"] = json!([]);
    assert_rejected::<Environment>(&canonical(&env), "an explicit empty setup");
    let mut env = value.clone();
    env["checks"][1]["setup"] = json!([[]]);
    assert_rejected::<Environment>(&canonical(&env), "an empty setup command");
    let mut env = value.clone();
    env["checks"][1]["setup"] = json!(vec![vec!["true"]; 9]);
    assert_rejected::<Environment>(&canonical(&env), "nine setup commands");
    let mut env = v0.clone();
    env["schema"] = json!("aver.env/v2");
    assert_rejected::<Environment>(&canonical(&env), "v2 without a v2 feature");

    let mut allow = v2.clone();
    allow.install.allow_scripts = true;
    assert!(
        !allow.rebuilds(),
        "an install that ran scripts has nothing to rebuild"
    );
}

#[test]
fn env_v3_carries_layers_and_per_check_images_exactly_when_used() {
    let v3 = sample_env_v3();
    let value = to_value(&v3);
    assert_eq!(value["schema"], "aver.env/v3");
    assert_eq!(value["layers"][0]["kind"], "cargo");
    let rust = v3.checks.get("rust").unwrap();
    assert_eq!(v3.check_image(rust).reference, "rust:1.99-bookworm");
    assert_eq!(
        v3.check_layers(rust)
            .iter()
            .map(|l| l.lockfile.path.as_str())
            .collect::<Vec<_>>(),
        ["Cargo.lock"]
    );
    let unit = v3.checks.get("unit").unwrap();
    assert_eq!(
        v3.check_layers(unit)
            .iter()
            .map(|l| l.lockfile.path.as_str())
            .collect::<Vec<_>>(),
        ["package-lock.json"],
        "without deps a check mounts the environment's own lockfile"
    );
    assert!(
        v3.is_test_file("pkg/test_calc.py"),
        "pytest's pattern in v3"
    );
    assert!(
        !sample_env().is_test_file("pkg/test_calc.py"),
        "v0 keeps Node's patterns"
    );

    let check_index = value["checks"]
        .as_array()
        .unwrap()
        .iter()
        .position(|c| c["id"] == "rust")
        .unwrap();
    for (what, mutate) in [
        (
            "v3 features under v2",
            (|v: &mut Value| v["schema"] = json!("aver.env/v2")) as fn(&mut Value),
        ),
        ("a layer repeating the environment's lockfile", |v| {
            v["layers"][0]["lockfile"]["path"] = json!("package-lock.json")
        }),
        ("a Cargo layer that rebuilds", |v| {
            v["layers"][0]["install"]["rebuild"] = json!(true)
        }),
        ("an unknown layer kind", |v| {
            v["layers"][0]["kind"] = json!("gem")
        }),
        ("an empty layer list written out", |v| {
            v["layers"] = json!([]);
            v["checks"] = json!([]);
        }),
    ] {
        let mut env = value.clone();
        mutate(&mut env);
        assert_rejected::<Environment>(&canonical(&env), what);
    }
    let mut env = value.clone();
    env["checks"][check_index]["deps"] = json!(["missing.lock"]);
    assert_rejected::<Environment>(&canonical(&env), "deps naming no layer");
    let mut env = value.clone();
    env["checks"][check_index]["deps"] = json!(["package-lock.json"]);
    assert_rejected::<Environment>(&canonical(&env), "deps spelling out the default");
    let mut env = value.clone();
    env["checks"][check_index]["report"]["format"] = json!("libtest");
    env["checks"][check_index]
        .as_object_mut()
        .unwrap()
        .retain(|k, _| ["id", "argv", "report"].contains(&k.as_str()));
    env["layers"] = json!(null);
    env.as_object_mut().unwrap().remove("layers");
    Environment::from_canonical(&canonical(&env)).expect("a libtest report alone is v3");
}

#[test]
fn env_v4_carries_floors_and_unapproved_checks_exactly_when_used() {
    let v4 = sample_env_v4();
    let value = to_value(&v4);
    assert_eq!(value["schema"], "aver.env/v4");
    let rust = v4.checks.get("rust").unwrap();
    assert_eq!(rust.floor().map(|n| n.get()), Some(40));
    assert_eq!(
        v4.checks.get("unit").unwrap().floor().map(|n| n.get()),
        Some(1),
        "a check with a report defaults to a floor of 1"
    );
    let lint = v4.checks.get("lint").unwrap();
    assert_eq!(lint.floor(), None, "an exit-only check has no floor");
    assert!(!lint.approved);
    assert!(rust.approved, "approved is the default");
    assert!(
        v4.is_test_file("pkg/test_calc.py"),
        "v4 keeps v3's patterns"
    );

    let index = |id: &str| {
        value["checks"]
            .as_array()
            .unwrap()
            .iter()
            .position(|c| c["id"] == id)
            .unwrap()
    };
    let (rust_at, lint_at) = (index("rust"), index("lint"));
    assert_eq!(value["checks"][rust_at]["min_tests"], 40);
    assert_eq!(value["checks"][lint_at]["approved"], false);
    assert!(value["checks"][rust_at].get("approved").is_none());
    let mut cases: Vec<(&str, Value)> = Vec::new();
    let mut v = value.clone();
    v["schema"] = json!("aver.env/v3");
    cases.push(("v4 fields under v3", v));
    let mut v = value.clone();
    v["checks"][rust_at]["min_tests"] = json!(1);
    cases.push(("min_tests spelling out the default", v));
    let mut v = value.clone();
    v["checks"][rust_at]["min_tests"] = json!(0);
    cases.push(("min_tests 0", v));
    let mut v = value.clone();
    v["checks"][lint_at]["min_tests"] = json!(5);
    cases.push(("min_tests on an exit-only check", v));
    let mut v = value.clone();
    v["checks"][rust_at]["approved"] = json!(true);
    cases.push(("approved spelled out", v));
    for (what, env) in cases {
        assert_rejected::<Environment>(&canonical(&env), what);
    }
}

#[test]
fn layer_keys_keep_npm_identities_and_name_every_other_input() {
    let env = sample_env_v3();
    let npm = env.primary_layer().unwrap();
    assert_eq!(
        env.layer_key(&npm, &env.image, &[]),
        env.deps_key(None).unwrap(),
        "the environment's own layer keeps its v0 key"
    );
    let cargo = env.layers.iter().next().unwrap().clone();
    let rust = env.checks.get("rust").unwrap().image.clone().unwrap();
    let manifest = BlobId::of_bytes(b"[package]\n");
    let with = env.layer_key(&cargo, &rust, &[("Cargo.toml".into(), manifest)]);
    assert_ne!(
        with,
        env.layer_key(&cargo, &rust, &[]),
        "manifests shape a Cargo layer"
    );
    assert_ne!(
        with,
        env.layer_key(&cargo, &env.image, &[("Cargo.toml".into(), manifest)]),
        "the image shapes a layer"
    );
}

#[test]
fn receipts_list_uncovered_changes_only_in_v2_and_never_with_pass() {
    let receipt = sample_receipt_v2_uncovered();
    let value = to_value(&receipt);
    assert_eq!(
        value["uncovered"]["items"],
        json!(["crates/core/src/lib.rs"])
    );
    let mut pass = value.clone();
    pass["verdict"] = json!("PASS");
    assert_rejected::<Receipt>(&canonical(&pass), "PASS with uncovered changes");
    let mut empty = value.clone();
    empty["uncovered"] = json!({"items": [], "omitted": 0});
    assert_rejected::<Receipt>(&canonical(&empty), "an empty uncovered list");
    let mut v1 = to_value(&sample_receipt_v1());
    v1["uncovered"] = value["uncovered"].clone();
    assert_rejected::<Receipt>(&canonical(&v1), "uncovered in v1");
}
