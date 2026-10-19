//! Execution-service pieces that run nothing: report parsing, output
//! bounding and the generated environment of a run.

mod common;

use std::collections::BTreeMap;

use aver::exec::junit;
use aver::exec::output::{bound_output, check_env, OUTPUT_HEAD_BYTES};
use aver_core::*;
use common::*;

fn unit() -> CheckId {
    p("unit")
}

fn statuses(tests: &Set<TestResult>) -> Vec<(&str, TestStatus)> {
    tests.iter().map(|t| (t.id.as_str(), t.status)).collect()
}

fn message<'a>(tests: &'a Set<TestResult>, id: &str) -> &'a str {
    tests.get(id).unwrap().message.as_ref().unwrap().as_str()
}

/// Captured from `node --test --test-reporter=junit` in the pinned
/// node:22-bookworm-slim image (Node v22.23.3), stack traces shortened.
const NODE_JUNIT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<testsuites>
	<testcase name="other file pass" time="0.000626" classname="test"/>
	<testcase name="top pass" time="0.000636" classname="test"/>
	<testcase name="top fail &lt;x> &amp; &amp;quot;q&amp;quot;" time="0.002425" classname="test" failure="Expected values to be strictly equal:1 !== 2">
		<failure type="testCodeFailure" message="Expected values to be strictly equal:1 !== 2">
[Error [ERR_TEST_FAILURE]: Expected values to be strictly equal:

1 !== 2
] {
  code: 'ERR_TEST_FAILURE',
  cause: AssertionError [ERR_ASSERTION]: Expected values to be strictly equal:
      at TestContext.&lt;anonymous> (/w/test/a.test.js:4:43)
}
		</failure>
	</testcase>
	<testsuite name="suite A" time="0.001256" disabled="0" errors="0" tests="3" failures="1" skipped="1" hostname="b56ce6ca7ed0">
		<testcase name="inner pass" time="0.000228" classname="test"/>
		<testcase name="inner skip" time="0.000076" classname="test">
			<skipped type="skipped" message="true"/>
		</testcase>
		<testsuite name="nested" time="0.000675" disabled="0" errors="0" tests="1" failures="1" skipped="0" hostname="b56ce6ca7ed0">
			<testcase name="deep fail" time="0.000341" classname="test" failure="boom">
				<failure type="testCodeFailure" message="boom">
Error [ERR_TEST_FAILURE]: boom
    at new Promise (&lt;anonymous>) {
  code: 'ERR_TEST_FAILURE',
  cause: Error: boom
      at TestContext.&lt;anonymous> (/w/test/a.test.js:8:70)
}
				</failure>
			</testcase>
		</testsuite>
	</testsuite>
	<testcase name="todo one" time="0.000163" classname="test">
		<skipped type="todo" message="true"/>
	</testcase>
	<testsuite name="parent" time="0.000961" disabled="0" errors="0" tests="1" failures="0" skipped="0" hostname="b56ce6ca7ed0">
		<testcase name="sub ok" time="0.000577" classname="test"/>
	</testsuite>
	<!-- tests 9 -->
	<!-- duration_ms 109.827625 -->
</testsuites>
"#;

#[test]
fn node_junit_report_maps_suite_chains_and_statuses() {
    let tests = junit::parse(&unit(), NODE_JUNIT.as_bytes()).unwrap();
    assert_eq!(
        statuses(&tests),
        vec![
            ("unit::other file pass", TestStatus::Pass),
            ("unit::parent > sub ok", TestStatus::Pass),
            ("unit::suite A > inner pass", TestStatus::Pass),
            ("unit::suite A > inner skip", TestStatus::Skip),
            ("unit::suite A > nested > deep fail", TestStatus::Fail),
            ("unit::todo one", TestStatus::Skip),
            // Node escapes quotes in names twice; the id keeps what it wrote.
            ("unit::top fail <x> & &quot;q&quot;", TestStatus::Fail),
            ("unit::top pass", TestStatus::Pass),
        ]
    );
    let deep = message(&tests, "unit::suite A > nested > deep fail");
    // The body repeats the message attribute, so it is not duplicated.
    assert!(
        deep.starts_with("Error [ERR_TEST_FAILURE]: boom\n"),
        "{deep}"
    );
    assert!(deep.contains("at TestContext.<anonymous> (/w/test/a.test.js:8:70)"));
    let top = message(&tests, "unit::top fail <x> & &quot;q&quot;");
    assert!(top.starts_with("Expected values to be strictly equal:1 !== 2\n"));
    assert!(tests.get("unit::top pass").unwrap().message.is_none());
    assert!(tests.get("unit::inner skip").is_none());
}

const JEST_JUNIT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="jest tests" tests="3" failures="1" errors="0" time="0.5">
  <testsuite name="math" errors="0" failures="1" skipped="1" timestamp="2026-10-10T12:00:00" time="0.4" tests="3">
    <testcase classname="math add returns the sum" name="math add returns the sum" time="0.001">
    </testcase>
    <testcase classname="math [crit:div-zero] divide throws" name="math [crit:div-zero] divide throws" time="0.002">
      <failure>Error: expect(received).toThrow(expected)
    at Object.&lt;anonymous&gt; (/work/test/math.test.js:10:5)</failure>
    </testcase>
    <testcase classname="math skipped one" name="math skipped one" time="0">
      <skipped/>
    </testcase>
  </testsuite>
</testsuites>
"#;

#[test]
fn jest_junit_report_uses_failure_body_as_message() {
    let tests = junit::parse(&unit(), JEST_JUNIT.as_bytes()).unwrap();
    assert_eq!(
        statuses(&tests),
        vec![
            (
                "unit::math > math [crit:div-zero] divide throws",
                TestStatus::Fail
            ),
            ("unit::math > math add returns the sum", TestStatus::Pass),
            ("unit::math > math skipped one", TestStatus::Skip),
        ]
    );
    assert_eq!(
        message(&tests, "unit::math > math [crit:div-zero] divide throws"),
        "Error: expect(received).toThrow(expected)\n    at Object.<anonymous> (/work/test/math.test.js:10:5)"
    );
}

const VITEST_JUNIT: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<testsuites name="vitest tests" tests="4" failures="2" errors="0" time="0.01">
    <testsuite name="test/math.test.ts" timestamp="2026-10-10T12:00:00.000Z" hostname="h" tests="4" failures="2" errors="0" skipped="1" time="0.003">
        <testcase classname="test/math.test.ts" name="math &gt; adds" time="0.001">
        </testcase>
        <testcase classname="test/math.test.ts" name="math &gt; divides" time="0.001">
            <failure message="expected 3 to be 2 // Object.is equality" type="AssertionError"><![CDATA[
AssertionError: expected 3 to be 2 // Object.is equality
 ❯ test/math.test.ts:9:22
]]></failure>
        </testcase>
        <testcase classname="test/math.test.ts" name="math &gt; todo" time="0">
            <skipped/>
        </testcase>
        <testcase classname="test/math.test.ts" name="crashes &#x2014; hard" time="0">
            <error message="boom" type="Error"/>
        </testcase>
    </testsuite>
</testsuites>
"#;

#[test]
fn vitest_junit_report_counts_errors_as_failures() {
    let tests = junit::parse(&unit(), VITEST_JUNIT.as_bytes()).unwrap();
    assert_eq!(
        statuses(&tests),
        vec![
            (
                "unit::test/math.test.ts > crashes \u{2014} hard",
                TestStatus::Fail
            ),
            ("unit::test/math.test.ts > math > adds", TestStatus::Pass),
            ("unit::test/math.test.ts > math > divides", TestStatus::Fail),
            ("unit::test/math.test.ts > math > todo", TestStatus::Skip),
        ]
    );
    assert_eq!(
        message(&tests, "unit::test/math.test.ts > crashes \u{2014} hard"),
        "boom"
    );
    assert_eq!(
        message(&tests, "unit::test/math.test.ts > math > divides"),
        "AssertionError: expected 3 to be 2 // Object.is equality\n ❯ test/math.test.ts:9:22"
    );
}

#[test]
fn duplicate_test_ids_merge_with_failure_winning() {
    let xml = r#"<testsuites>
        <testcase name="same"><skipped/></testcase>
        <testcase name="same"><failure message="first"/></testcase>
        <testcase name="same"/>
        <testcase name="same"><failure message="second"/></testcase>
        <testcase name="other"><skipped/></testcase>
        <testcase name="other"/>
        <testcase name="explained"><skipped message="flaky on CI">tracked elsewhere</skipped></testcase>
    </testsuites>"#;
    let tests = junit::parse(&unit(), xml.as_bytes()).unwrap();
    assert_eq!(
        statuses(&tests),
        vec![
            ("unit::explained", TestStatus::Skip),
            ("unit::other", TestStatus::Pass),
            ("unit::same", TestStatus::Fail)
        ]
    );
    assert!(tests.get("unit::explained").unwrap().message.is_none());
    assert_eq!(message(&tests, "unit::same"), "first");
}

#[test]
fn long_messages_and_names_are_bounded() {
    let long_name = "n".repeat(3000);
    let long_body = "é".repeat(3000);
    let xml = format!(
        r#"<testsuite name="s"><testcase name="{long_name}"><failure>{long_body}</failure></testcase></testsuite>"#
    );
    let tests = junit::parse(&unit(), xml.as_bytes()).unwrap();
    let test = tests.iter().next().unwrap();
    assert_eq!(test.id.test_path().len(), 1024);
    assert!(test.id.test_path().starts_with("s > nnn"));
    let message = test.message.as_ref().unwrap().as_str();
    assert!(
        message.len() <= 2048 && message.len() >= 2046,
        "{}",
        message.len()
    );
    assert!(message.chars().all(|c| c == 'é'));
}

#[test]
fn malformed_or_foreign_reports_are_errors() {
    for bad in [
        "",
        "not xml at all",
        "<testsuites><testcase name='a'></testsuite>",
        "<html><body>oops</body></html>",
        // A runner that died mid-write: the open suite and case must not
        // read as a complete report with the later tests missing.
        "<testsuites><testsuite name='math'><testcase name='a'/><testcase name='b'>",
        "<testsuite name='math'><testcase name='a'/>",
        // A test nobody can name, as a bare TAP `ok` line would give.
        "<testsuites><testcase name='a'/><testcase/></testsuites>",
        "<testsuites><testcase name=' '/></testsuites>",
        "<testsuites><testsuite name='s'><testcase name=''/></testsuite></testsuites>",
        // The command's own entry beside the tests.
        "<testsuites><testcase name='(exit)'/></testsuites>",
    ] {
        assert!(junit::parse(&unit(), bad.as_bytes()).is_err(), "{bad:?}");
    }
    let empty = junit::parse(&unit(), b"<testsuites/>").unwrap();
    assert!(empty.is_empty());
    let nested = junit::parse(
        &unit(),
        b"<testsuite name='s'><testcase name='(exit)'/></testsuite>",
    )
    .unwrap();
    assert_eq!(
        nested.iter().next().unwrap().id.as_str(),
        "unit::s > (exit)"
    );
}

#[test]
fn bound_output_keeps_head_and_tail() {
    let budget = 64 * 1024;
    let short = vec![b'x'; budget as usize];
    assert_eq!(bound_output(&short, budget), short);

    let long: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let bounded = bound_output(&long, budget);
    assert_eq!(bounded.len(), 64 * 1024);
    assert_eq!(&bounded[..OUTPUT_HEAD_BYTES], &long[..8 * 1024]);
    assert_eq!(&bounded[8 * 1024..], &long[long.len() - 56 * 1024..]);

    assert_eq!(bound_output(b"abcdef", 0), b"");
    assert_eq!(bound_output(b"abcdefghij", 4), b"abcd");
    let small_budget = bound_output(&long, 10_000);
    assert_eq!(&small_budget[..8192], &long[..8192]);
    assert_eq!(&small_budget[8192..], &long[long.len() - 1808..]);
}

fn sample_env() -> Environment {
    parse_aver_toml(
        &format!(
            r#"
[env]
image = "{NODE_IMAGE}"
lockfile = "package-lock.json"
[limits]
cpus = 1
memory_mb = 256
pids = 64
[vars]
NODE_OPTIONS = "--max-old-space-size=128"
HOME = "/work/home"
AVER_RUN_ATTEMPT = "9"
[[check]]
id = "unit"
run = ["node", "--test"]
"#
        ),
        Some(BlobId::of_bytes(b"{}")),
    )
}

#[test]
fn check_env_is_generated_from_the_environment_only() {
    std::env::set_var("AVER_UNIT_CANARY", "leaked");
    let env = sample_env();
    let vars = check_env(&env);
    let expected: BTreeMap<String, String> = [
        ("CI", "1"),
        ("HOME", "/work/home"),
        ("NODE_ENV", "test"),
        ("NODE_OPTIONS", "--max-old-space-size=128"),
        ("CARGO_NET_OFFLINE", "true"),
        ("GOPROXY", "off"),
        ("GOTOOLCHAIN", "local"),
        ("PIP_NO_INDEX", "1"),
        ("UV_OFFLINE", "1"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    assert_eq!(
        vars, expected,
        "no run index, and the environment cannot set AVER_RUN_ATTEMPT"
    );
}

const JEST_JSON: &str = include_str!("reports/jest.json");

/// Captured from `mocha --reporter xunit --reporter-option output=…` (Mocha
/// 10.7.3) in the pinned image.
const MOCHA_XUNIT: &str = include_str!("reports/mocha-xunit.xml");

#[test]
fn jest_json_maps_files_ancestors_and_titles_and_names_files_that_fail_to_run() {
    let (tests, locations) =
        junit::parse_as(&unit(), JEST_JSON.as_bytes(), ReportFormat::JestJson).unwrap();
    assert_eq!(
        statuses(&tests),
        [
            ("unit::test/a.test.js > calc > add > adds", TestStatus::Pass),
            (
                "unit::test/a.test.js > calc > add > fails",
                TestStatus::Fail
            ),
            ("unit::test/a.test.js > calc > skipped", TestStatus::Skip),
            (
                "unit::test/broken.test.js > (suite failed to run)",
                TestStatus::Fail
            ),
        ]
    );
    assert!(message(&tests, "unit::test/a.test.js > calc > add > fails")
        .contains("Expected value to strictly be equal to"));
    assert!(
        message(&tests, "unit::test/broken.test.js > (suite failed to run)")
            .contains("Cannot find module './does-not-exist'")
    );
    assert_eq!(
        locations
            .get("unit::test/a.test.js > calc > add > fails")
            .and_then(|l| l.file.as_deref()),
        Some("test/a.test.js")
    );
    for bad in [
        "[]",
        "{}",
        r#"{"testResults": 3}"#,
        r#"{"testResults": [{"assertionResults": []}]}"#,
        r#"{"testResults": [{"name": "/work/a.test.js", "assertionResults": [{"title": " ", "status": "passed"}]}]}"#,
        r#"{"testResults": [{"name": "/work/a.test.js", "assertionResults": [{"title": "x", "status": "passed"}]"#,
    ] {
        assert!(
            junit::parse_as(&unit(), bad.as_bytes(), ReportFormat::JestJson).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn jest_json_with_a_lone_surrogate_still_parses() {
    let json = r#"{"testResults": [{"name": "/work/a.test.js", "status": "failed", "assertionResults": [
        {"ancestorTitles": [], "title": "emoji \ud83d\ude00", "status": "passed", "failureMessages": []},
        {"ancestorTitles": [], "title": "cut", "status": "failed", "failureMessages": ["Expected: \"a\ud83d\" literal \\ud83d"]}
    ]}]}"#;
    let (tests, _) = junit::parse_as(&unit(), json.as_bytes(), ReportFormat::JestJson).unwrap();
    assert!(tests.get("unit::a.test.js > emoji \u{1f600}").is_some());
    assert_eq!(
        message(&tests, "unit::a.test.js > cut"),
        "Expected: \"a\u{fffd}\" literal \\ud83d"
    );
}

/// Captured from `cargo test --workspace --no-fail-fast` (Rust 1.99, the
/// pinned image): a library, a binary and an integration test, each with
/// a failure or an ignore, and doc-tests, one of two on `add` failing.
const LIBTEST: &str = include_str!("reports/libtest.txt");

/// Captured from `go test -json ./...` (Go 1.27, the pinned image): a
/// failure, a skip, subtests, a package that passes and one that does not
/// build.
const GO_TEST_JSON: &str = include_str!("reports/go-test.json");

/// Captured from `pytest --junitxml=… -o junit_family=xunit1
/// --continue-on-collection-errors` (pytest 9.1.1, the pinned image):
/// classes, nested classes, parameters, a skip, a setup error and a
/// module that fails to import.
const PYTEST_XML: &str = include_str!("reports/pytest.xml");

#[test]
fn libtest_output_names_tests_by_binary_and_merges_doc_tests_by_item() {
    let check: CheckId = p("cargo");
    let (tests, _) = junit::parse_as(&check, LIBTEST.as_bytes(), ReportFormat::Libtest).unwrap();
    assert_eq!(
        statuses(&tests),
        [
            ("cargo::api tests/api.rs > api_adds", TestStatus::Pass),
            ("cargo::api tests/api.rs > api_fails", TestStatus::Fail),
            ("cargo::calc doc-tests > src/lib.rs - add", TestStatus::Fail),
            ("cargo::calc doc-tests > src/lib.rs - div", TestStatus::Pass),
            ("cargo::calc src/lib.rs > tests::adds", TestStatus::Pass),
            ("cargo::calc src/lib.rs > tests::fails", TestStatus::Fail),
            ("cargo::calc src/lib.rs > tests::ignored", TestStatus::Skip),
            (
                "cargo::calc src/lib.rs > tests::ignored_with_reason",
                TestStatus::Skip
            ),
            ("cargo::calc src/lib.rs > tests::panics", TestStatus::Pass),
            ("cargo::calc src/main.rs > tests::adds", TestStatus::Pass),
        ]
    );
    let fails = message(&tests, "cargo::calc src/lib.rs > tests::fails");
    assert!(
        fails.contains("one and one") && fails.contains("some captured output"),
        "{fails}"
    );
    assert!(message(&tests, "cargo::api tests/api.rs > api_fails").contains("the api is broken"));
    assert!(message(&tests, "cargo::calc doc-tests > src/lib.rs - add").contains("right: 5"));
    for bad in [
        "",
        "   Compiling calc v0.1.0\nerror[E0425]: cannot find value `x` in this scope\n",
        "     Running unittests src/lib.rs (target/debug/deps/calc-0123abcd)\n",
    ] {
        assert!(
            junit::parse_as(&check, bad.as_bytes(), ReportFormat::Libtest).is_err(),
            "no test binary ran: {bad:?}"
        );
    }
}

#[test]
fn go_test_json_names_tests_by_package_and_fails_packages_that_do_not_build() {
    let check: CheckId = p("go");
    let (tests, _) =
        junit::parse_as(&check, GO_TEST_JSON.as_bytes(), ReportFormat::GoJson).unwrap();
    assert_eq!(
        statuses(&tests),
        [
            ("go::example.com/calc > TestAdd", TestStatus::Pass),
            ("go::example.com/calc > TestFails", TestStatus::Fail),
            ("go::example.com/calc > TestSkipped", TestStatus::Skip),
            ("go::example.com/calc > TestTable", TestStatus::Fail),
            ("go::example.com/calc > TestTable/one", TestStatus::Pass),
            ("go::example.com/calc > TestTable/two", TestStatus::Fail),
            ("go::example.com/calc/broken > (package)", TestStatus::Fail),
            ("go::example.com/calc/sub > TestSub", TestStatus::Pass),
        ]
    );
    let fails = message(&tests, "go::example.com/calc > TestFails");
    assert!(
        fails.contains("want 3") && !fails.contains("=== RUN"),
        "{fails}"
    );
    assert!(message(&tests, "go::example.com/calc/broken > (package)").contains("cannot use"));
    for bad in ["", "not json\n", "[1, 2]\n"] {
        assert!(
            junit::parse_as(&check, bad.as_bytes(), ReportFormat::GoJson).is_err(),
            "{bad:?}"
        );
    }
}

#[test]
fn pytest_junit_names_tests_by_file_classes_and_name() {
    let check: CheckId = p("pytest");
    let (tests, locations) =
        junit::parse_as(&check, PYTEST_XML.as_bytes(), ReportFormat::PytestJunit).unwrap();
    assert_eq!(
        statuses(&tests),
        [
            (
                "pytest::tests/test_broken.py > tests.test_broken",
                TestStatus::Fail
            ),
            (
                "pytest::tests/test_calc.py > TestDiv > TestNested > test_inner",
                TestStatus::Pass
            ),
            (
                "pytest::tests/test_calc.py > TestDiv > test_divides[1-0]",
                TestStatus::Fail
            ),
            (
                "pytest::tests/test_calc.py > TestDiv > test_divides[4-2]",
                TestStatus::Pass
            ),
            ("pytest::tests/test_calc.py > test_adds", TestStatus::Pass),
            ("pytest::tests/test_calc.py > test_fails", TestStatus::Fail),
            (
                "pytest::tests/test_calc.py > test_setup_error",
                TestStatus::Fail
            ),
            (
                "pytest::tests/test_calc.py > test_skipped",
                TestStatus::Skip
            ),
        ]
    );
    assert!(message(&tests, "pytest::tests/test_calc.py > test_fails").contains("one and one"));
    assert!(
        message(&tests, "pytest::tests/test_broken.py > tests.test_broken")
            .contains("a_module_that_does_not_exist")
    );
    assert_eq!(
        locations
            .get("pytest::tests/test_calc.py > test_fails")
            .and_then(|l| l.line),
        Some(7)
    );
}

#[test]
fn mocha_xunit_takes_the_suite_path_from_classname() {
    let (tests, _) =
        junit::parse_as(&unit(), MOCHA_XUNIT.as_bytes(), ReportFormat::MochaXunit).unwrap();
    assert_eq!(
        statuses(&tests),
        [
            ("unit::calc > skipped", TestStatus::Skip),
            ("unit::calc add > adds", TestStatus::Pass),
            ("unit::calc add > fails", TestStatus::Fail),
        ]
    );
    assert!(message(&tests, "unit::calc add > fails").contains("2 !== 3"));
    let plain = junit::parse(&unit(), MOCHA_XUNIT.as_bytes()).unwrap();
    assert!(
        plain.get("unit::Mocha Tests > fails").is_some(),
        "plain JUnit ignores classname"
    );
}

/// Defect: a Jest report names each file by its absolute path in the
/// slot, so every test id differs between sides; and a slot path inside
/// an XML text node, rewritten raw, would open an element (§4.5).
#[test]
fn unrooted_reports_name_tests_the_same_in_every_slot() {
    let dir = tempfile::tempdir().unwrap();
    let slot = dir.path().join("B-1");
    std::fs::create_dir_all(&slot).unwrap();
    let roots = [(slot.as_path(), junit::SRC_ROOT)];
    let s = slot.display();
    let jest = format!(
        r#"{{"testResults": [{{"name": "{s}/test/a.test.js", "status": "passed", "assertionResults": [{{"title": "reads {s}/x", "status": "passed"}}]}}]}}"#
    );
    let unrooted = junit::unroot(jest.as_bytes(), ReportFormat::JestJson, &roots);
    let (tests, _) = junit::parse_as(&unit(), &unrooted, ReportFormat::JestJson).unwrap();
    assert_eq!(
        statuses(&tests),
        [("unit::test/a.test.js > reads <src>/x", TestStatus::Pass)]
    );

    let real = slot.canonicalize().unwrap();
    let xml = format!(
        r#"<testsuites><testsuite name="s"><testcase name="at[{}]"><failure>at {s}/t.py:3</failure></testcase></testsuite></testsuites>"#,
        real.display()
    );
    let unrooted = junit::unroot(xml.as_bytes(), ReportFormat::Junit, &roots);
    let (tests, _) = junit::parse_as(&unit(), &unrooted, ReportFormat::Junit).unwrap();
    assert_eq!(message(&tests, "unit::s > at[<src>]"), "at <src>/t.py:3");
}
