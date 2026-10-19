//! Round 13a, over the scripted sandbox: the red team's ways for a head to
//! get a better verdict than it deserves (holes 1 to 4, 6 and 7), each
//! with the verdict it gets now, plus setup findings, uncovered languages
//! and the REVIEW report. The Docker forms are in `generic_docker.rs`
//! (Rust inline tests, a lost report), `ci_docker.rs` (the sandbox result)
//! and `sandbox_security.rs` (the result files out of reach).

mod common;
mod scripted;
mod world;

use std::sync::Arc;

use aver_core::*;
use scripted::Scripted;
use world::*;

fn toml(checks: &str) -> String {
    format!("[env]\nimage = \"{PINNED_IMAGE}\"\n{checks}")
}

/// One JUnit check `unit` over `argv`.
fn unit(argv: &str) -> String {
    toml(&format!(
        "\n[[check]]\nid = \"unit\"\nrun = {argv}\nreport = {{ format = \"junit\", path = \"/aver/out/unit.xml\" }}\n"
    ))
}

const DIR: &str = r#"["junit-dir", "unit.xml", "test"]"#;

struct Case {
    w: World,
    config: String,
    base: SnapshotId,
    task: TaskId,
}

impl Case {
    fn new(sandbox: Arc<Scripted>, config: String, base: &[(&str, &str)]) -> Self {
        let w = World::with_sandbox(sandbox);
        let mut files = vec![("aver.toml", config.as_str())];
        files.extend_from_slice(base);
        let base = w.snapshot(&files);
        let task = w.create_task(w.task_body(&w.admin, base, &[]));
        Self {
            w,
            config,
            base,
            task,
        }
    }

    async fn verify(&self, head: &[(&str, &str)]) -> Receipt {
        self.verified(head).await.receipt
    }

    async fn verified(&self, head: &[(&str, &str)]) -> aver::verify::Verified {
        let mut files = vec![("aver.toml", self.config.as_str())];
        files.extend_from_slice(head);
        let head = self.w.snapshot(&files);
        let verified = self
            .w
            .verify(&self.w.admin, self.task, head, self.base)
            .await;
        common::assert_matches_schema("receipt", &verified.receipt);
        common::assert_value_matches_schema(
            "diagnostics",
            &serde_json::to_value(&verified.diagnostics).unwrap(),
        );
        verified
    }
}

fn modified(receipt: &Receipt) -> Vec<String> {
    receipt
        .body
        .test_changes
        .as_ref()
        .unwrap()
        .files_modified
        .items
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// Hole 1, repro A: the head changes the rendering and rewrites the golden
/// file outside `test/` to match. The golden file is a test file now, so
/// the base's copy judges the head: the rewritten expectation is CHANGED
/// and listed, never a silent PASS.
#[tokio::test]
async fn a_rewritten_golden_file_is_review_not_pass() {
    let c = Case::new(
        Arc::default(),
        unit(DIR),
        &[
            (
                "test/render.test.json",
                r#"{"renders the golden list": "agree:out/list.txt=fixtures/golden.txt"}"#,
            ),
            ("out/list.txt", "- a\n- b\n"),
            ("fixtures/golden.txt", "- a\n- b\n"),
        ],
    );
    let receipt = c
        .verify(&[
            (
                "test/render.test.json",
                r#"{"renders the golden list": "agree:out/list.txt=fixtures/golden.txt"}"#,
            ),
            ("out/list.txt", "* a\n* b\n"),
            ("fixtures/golden.txt", "* a\n* b\n"),
        ])
        .await;
    let golden = entry(&receipt, "unit::renders the golden list");
    assert_eq!(
        (golden.base, golden.head, golden.cause),
        (BaseStatus::Pass, HeadStatus::Fail, Some(Cause::Changed))
    );
    assert_eq!(modified(&receipt), ["fixtures/golden.txt"]);
    assert_eq!(receipt.body.verdict, Verdict::Review);
}

/// Hole 1, repro C: Django's `tests.py` is a test file, so a head that
/// weakens the implementation and rewrites the expectation there with it
/// gets REVIEW, with the file in `test_changes`.
#[tokio::test]
async fn a_rewritten_django_tests_py_is_review_not_pass() {
    let c = Case::new(
        Arc::default(),
        unit(r#"["junit", "unit.xml", "shop/tests.py"]"#),
        &[
            ("shop/pricing.py", "def total(prices): return sum(prices)\n"),
            (
                "shop/tests.py",
                r#"{"total sums every price": "contains:shop/pricing.py:sum(prices)"}"#,
            ),
        ],
    );
    let receipt = c
        .verify(&[
            (
                "shop/pricing.py",
                "def total(prices): return sum(prices[1:])\n",
            ),
            (
                "shop/tests.py",
                r#"{"total sums every price": "contains:shop/pricing.py:sum(prices[1:])"}"#,
            ),
        ])
        .await;
    let total = entry(&receipt, "unit::total sums every price");
    assert_eq!(total.cause, Some(Cause::Changed));
    assert_eq!(modified(&receipt), ["shop/tests.py"]);
    // The check at the root covers every file, so CHANGED holds the
    // verdict at REVIEW.
    assert_eq!(receipt.body.verdict, Verdict::Review);
}

const LIB: &str = "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n    #[test]\n    fn adds() {\n        assert_eq!(add(2, 2), 4);\n    }\n}\n";

/// Hole 1, repro B: a Rust `#[cfg(test)]` module the head rewrites is a
/// test change: `src/lib.rs` is listed as a modified test file, the judged
/// head runs the head's code with the base's test module, and the head's
/// tests run with its own.
#[tokio::test]
async fn a_rust_test_module_the_head_rewrites_is_judged_from_the_base() {
    let sandbox = Arc::new(Scripted::default());
    let c = Case::new(
        sandbox.clone(),
        unit(DIR),
        &[
            ("test/lib.test.json", r#"{"adds": "pass"}"#),
            ("src/lib.rs", LIB),
        ],
    );
    let head = LIB
        .replace("    a + b\n", "    if a == b { a * 3 } else { a + b }\n")
        .replace("add(2, 2), 4", "add(2, 2), 6");
    let receipt = c
        .verify(&[
            ("test/lib.test.json", r#"{"adds": "pass"}"#),
            ("src/lib.rs", &head),
        ])
        .await;
    assert_eq!(modified(&receipt), ["src/lib.rs"]);
    let libs: Vec<String> = sandbox
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter_map(|r| scripted::files(&r.input).get("src/lib.rs").cloned())
        .map(|b| String::from_utf8(b).unwrap())
        .collect();
    let judged = head.replace("add(2, 2), 6", "add(2, 2), 4");
    assert!(libs.contains(&judged), "the judged head: {libs:#?}");
    assert!(libs.contains(&head), "the head's own tests: {libs:#?}");

    // Code changed, test module not: no test change.
    let receipt = c
        .verify(&[
            ("test/lib.test.json", r#"{"adds": "pass"}"#),
            ("src/lib.rs", &LIB.replace("a + b", "b + a")),
        ])
        .await;
    assert!(modified(&receipt).is_empty());
}

/// Hole 2: the base reports every test (one fails there anyway) and the
/// head's runs end without a report. The head lost it, so the check is
/// NEW: FAIL_OWN, not BLOCKED_NOT_OWN behind the base's failure.
#[tokio::test]
async fn a_head_that_loses_its_report_over_a_red_base_fails_own() {
    let c = Case::new(
        Arc::default(),
        unit(r#"["junit", "unit.xml", "tests.json"]"#),
        &[(
            "tests.json",
            r#"{"adds": "pass", "talks to the upstream API": "fail"}"#,
        )],
    );
    let receipt = c.verify(&[]).await;
    let whole = entry(&receipt, "unit");
    assert_eq!(
        (whole.base, whole.head, whole.cause),
        (BaseStatus::Fail, HeadStatus::Fail, Some(Cause::New))
    );
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);

    // Control: a check with no per-test report on either side that fails
    // on both stays inconclusive.
    let c = Case::new(
        Arc::default(),
        toml("\n[[check]]\nid = \"build\"\nrun = [\"exit\", \"1\"]\n"),
        &[("src/a.js", "1")],
    );
    let receipt = c.verify(&[("src/a.js", "2")]).await;
    assert_eq!(entry(&receipt, "build").cause, Some(Cause::Preexisting));
    assert_eq!(receipt.body.verdict, Verdict::BlockedNotOwn);
}

/// Hole 2, repro B: a test the head adds is named `(exit)`, which makes
/// the report malformed; the head also breaks `add`. The base's tests run
/// without the head's added files, so the regression shows: FAIL_OWN.
#[tokio::test]
async fn a_reserved_test_name_cannot_hide_a_regression() {
    let c = Case::new(
        Arc::default(),
        unit(DIR),
        &[
            ("src/add-correct", ""),
            (
                "test/calc.test.json",
                r#"{"adds": "pass-if:src/add-correct", "talks to the upstream API": "fail"}"#,
            ),
        ],
    );
    let receipt = c
        .verify(&[
            (
                "test/calc.test.json",
                r#"{"adds": "pass-if:src/add-correct", "talks to the upstream API": "fail"}"#,
            ),
            ("test/zz.test.json", r#"{"(exit)": "pass"}"#),
        ])
        .await;
    assert_eq!(entry(&receipt, "unit::adds").cause, Some(Cause::New));
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);
}

/// Hole 3: the head hangs on every run while the base, red with one
/// failing test, finished. The timeout is the change's: NEW.
#[tokio::test]
async fn a_head_timeout_over_a_red_base_fails_own() {
    let c = Case::new(
        Arc::default(),
        unit(DIR),
        &[
            ("src/terminates", ""),
            (
                "test/calc.test.json",
                r#"{"adds": "timeout-unless:src/terminates", "talks to the upstream API": "fail"}"#,
            ),
        ],
    );
    let receipt = c
        .verify(&[(
            "test/calc.test.json",
            r#"{"adds": "timeout-unless:src/terminates", "talks to the upstream API": "fail"}"#,
        )])
        .await;
    let whole = entry(&receipt, "unit");
    assert_eq!(whole.cause, Some(Cause::New));
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);
}

/// Hole 6: a test file the head adds patches the code under test so the
/// base's test passes. The base environment's per-test checks judge the
/// base's tests in a run without the head's added test files, so the
/// regression is NEW; the added file's own test still runs, in the
/// head-tests tree.
#[tokio::test]
async fn an_added_test_file_cannot_patch_the_base_tests() {
    let c = Case::new(
        Arc::default(),
        unit(DIR),
        &[
            ("src/add-correct", ""),
            (
                "test/calc.test.json",
                r#"{"adds": "pass-if:src/add-correct"}"#,
            ),
        ],
    );
    let receipt = c
        .verify(&[
            (
                "test/calc.test.json",
                r#"{"adds": "pass-if:src/add-correct"}"#,
            ),
            (
                "test/zz_new.test.json",
                r#"{"!force-pass:adds": "", "new works": "pass"}"#,
            ),
        ])
        .await;
    let adds = entry(&receipt, "unit::adds");
    assert_eq!(
        (adds.base, adds.head, adds.cause),
        (BaseStatus::Pass, HeadStatus::Fail, Some(Cause::New))
    );
    let new = entry(&receipt, "unit::new works");
    assert_eq!((new.head, new.cause), (HeadStatus::Pass, None));
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);
}

/// Hole 7: a test the base passes and the head fails on its first run only
/// is NEW once the base has passed it three times, not FLAKY.
#[tokio::test]
async fn a_head_that_flakes_over_a_stable_base_fails_own() {
    let c = Case::new(
        Arc::default(),
        unit(DIR),
        &[("test/calc.test.json", r#"{"adds": "flaky-if:src/race.js"}"#)],
    );
    let receipt = c
        .verify(&[
            ("test/calc.test.json", r#"{"adds": "flaky-if:src/race.js"}"#),
            ("src/race.js", "racy"),
        ])
        .await;
    let adds = entry(&receipt, "unit::adds");
    assert_eq!(
        (adds.base, adds.head, adds.cause),
        (BaseStatus::Pass, HeadStatus::Flaky, Some(Cause::New))
    );
    assert_eq!(
        adds.runs.base.len(),
        3,
        "the base was rerun to three passes"
    );
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);
}

/// Hole 7, control: a test whose base copy fails on a rerun is flaky on
/// the original code too, so a flaky head is FLAKY and does not block.
#[tokio::test]
async fn a_head_that_flakes_where_the_base_flakes_too_is_flaky() {
    let c = Case::new(
        Arc::default(),
        unit(DIR),
        &[("test/calc.test.json", r#"{"adds": "race:src/race.js"}"#)],
    );
    let receipt = c
        .verify(&[
            ("test/calc.test.json", r#"{"adds": "race:src/race.js"}"#),
            ("src/race.js", "racy"),
        ])
        .await;
    let adds = entry(&receipt, "unit::adds");
    assert_eq!(
        (adds.base, adds.head, adds.cause),
        (BaseStatus::Flaky, HeadStatus::Flaky, Some(Cause::Flaky))
    );
    assert_eq!(receipt.body.verdict, Verdict::Pass);
}

/// Hole 8 (stopgap): a change to source of a language no check runs (Java
/// beside a Node package) is uncovered, so UNPROVEN instead of "PASS, no
/// check is affected"; a change to documentation is not.
#[tokio::test]
async fn a_change_in_a_language_no_check_runs_is_unproven() {
    let config = toml(
        "\n[[check]]\nid = \"web\"\ncwd = \"web\"\nrun = [\"junit-dir\", \"web.xml\", \"test\"]\nreport = { format = \"junit\", path = \"/aver/out/web.xml\" }\n",
    );
    let java = "class Pricing { int add(int a, int b) { return a + b; } }\n";
    let c = Case::new(
        Arc::default(),
        config,
        &[
            ("web/test/ui.test.json", r#"{"renders": "pass"}"#),
            ("src/main/java/app/Pricing.java", java),
            ("README.md", "# shop\n"),
        ],
    );
    let receipt = c
        .verify(&[
            ("web/test/ui.test.json", r#"{"renders": "pass"}"#),
            (
                "src/main/java/app/Pricing.java",
                &java.replace("a + b", "a - b"),
            ),
            ("README.md", "# shop\n"),
        ])
        .await;
    let uncovered: Vec<String> = receipt
        .body
        .uncovered
        .as_ref()
        .unwrap()
        .items
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(uncovered, ["src/main/java/app/Pricing.java"]);
    assert_eq!(receipt.body.verdict, Verdict::Unproven);

    let receipt = c
        .verify(&[
            ("web/test/ui.test.json", r#"{"renders": "pass"}"#),
            ("src/main/java/app/Pricing.java", java),
            ("README.md", "# shop, documented\n"),
        ])
        .await;
    assert!(receipt.body.uncovered.is_none());
    assert_eq!(receipt.body.verdict, Verdict::Pass);
}

/// Base findings hiding new ones: a setup command (`go vet`, `mypy`) that
/// already fails on the base no longer hides the findings a change adds.
/// Each finding is an entry, so the new one is NEW while the old one and
/// the `(exit)` entry are PREEXISTING.
#[tokio::test]
async fn a_new_finding_of_a_failing_setup_command_is_new() {
    let config = toml(
        "\n[[check]]\nid = \"unit\"\nrun = [\"junit-dir\", \"unit.xml\", \"test\"]\nsetup = [[\"findings\", \"vet.out\"]]\nreport = { format = \"junit\", path = \"/aver/out/unit.xml\" }\n",
    );
    let old = "calc.go:3:1: fmt.Printf format %d has arg s of wrong type string\n";
    let c = Case::new(
        Arc::default(),
        config,
        &[
            ("test/calc.test.json", r#"{"adds": "pass"}"#),
            ("vet.out", old),
        ],
    );
    let both = format!("{old}calc.go:12:2: unreachable code\n");
    let receipt = c
        .verify(&[
            ("test/calc.test.json", r#"{"adds": "pass"}"#),
            ("vet.out", &both),
        ])
        .await;
    let old_entry = entry(
        &receipt,
        "unit::(setup) calc.go: fmt.Printf format %d has arg s of wrong type string",
    );
    assert_eq!(old_entry.cause, Some(Cause::Preexisting));
    let new_entry = entry(&receipt, "unit::(setup) calc.go: unreachable code");
    assert_eq!(
        (new_entry.base, new_entry.head, new_entry.cause),
        (BaseStatus::Pass, HeadStatus::Fail, Some(Cause::New))
    );
    assert_eq!(
        entry(&receipt, "unit::(exit)").cause,
        Some(Cause::Preexisting)
    );
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);

    // The same findings on both sides: the setup command failed on the
    // base, which blocks the verdict (REDESIGN.md §4.1).
    let receipt = c
        .verify(&[
            ("test/calc.test.json", r#"{"adds": "pass"}"#),
            ("vet.out", old),
        ])
        .await;
    assert_eq!(
        entry(&receipt, "unit::(evidence) setup failed on the base").cause,
        Some(Cause::Env)
    );
    assert_eq!(receipt.body.verdict, Verdict::BlockedNotOwn);
}

/// Gaming a preexisting test: a head that loosens a test the base already
/// fails gets REVIEW, and the agent report says the agent edited it, shows
/// the before and after lines, never lists it as evidence that the change
/// works, and does not say "nothing" is left to do.
#[tokio::test]
async fn loosening_an_already_failing_test_is_review_with_the_edit_shown() {
    let c = Case::new(
        Arc::default(),
        unit(DIR),
        &[(
            "test/parse.test.json",
            "{\n  \"keeps\": \"pass\",\n  \"parses the config\": \"fail\"\n}\n",
        )],
    );
    let v = c
        .verified(&[(
            "test/parse.test.json",
            "{\n  \"keeps\": \"pass\",\n  \"parses the config\": \"pass\"\n}\n",
        )])
        .await;
    let parses = entry(&v.receipt, "unit::parses the config");
    assert_eq!(
        (parses.base, parses.head, parses.cause),
        (BaseStatus::Fail, HeadStatus::Fail, Some(Cause::Changed))
    );
    assert_eq!(v.receipt.body.verdict, Verdict::Review);
    let shown = v
        .diagnostics
        .evidence
        .as_ref()
        .map(|e| e.shows_change.len());
    assert!(
        shown.unwrap_or(0) == 0,
        "a CHANGED test is no evidence: {shown:?}"
    );
    let text = aver::run::report::agent_text(&aver::run::report::ReportInput {
        receipt_id: &v.id,
        receipt: &v.receipt,
        diagnostics: Some(&v.diagnostics),
        sync: None,
        took: None,
        budget: None,
        notices: None,
        timing: None,
    });
    assert!(
        text.contains("Changed by you, for a human to review (1;"),
        "{text}"
    );
    assert!(
        text.contains("-  \"parses the config\": \"fail\""),
        "{text}"
    );
    assert!(
        text.contains("+  \"parses the config\": \"pass\""),
        "{text}"
    );
    assert!(!text.contains("Your tests show the change"), "{text}");
    assert!(!text.contains("Next: nothing"), "{text}");
    assert!(text.contains("a person must approve"), "{text}");
}
