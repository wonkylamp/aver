//! CI mode over real Git repositories and the scripted sandbox: imports
//! both commits, signs a task from the declared spec and verifies head
//! against base (ARCHITECTURE.md §2.9). No Docker needed; `ci_docker.rs`
//! runs the CLI on the fixtures.

mod common;
mod git_support;
mod scripted;
mod world;

use std::path::Path as FsPath;
use std::process::Command;
use std::sync::Arc;

use aver::ci::{self, summary, Ci, CiError, FailOn, Spec};
use aver::env::StaticResolver;
use aver::exec::{ExecConfig, Executor};
use aver::keys::{self, Home};
use aver_core::*;
use common::{assert_matches_schema, assert_value_matches_schema};
use git_support::git_cmd;
use tempfile::TempDir;
use world::{entry, unit_toml};

struct Repo {
    dir: TempDir,
}

impl Repo {
    fn new() -> Self {
        let repo = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }

    fn path(&self) -> &FsPath {
        self.dir.path()
    }

    fn git(&self, args: &[&str]) -> String {
        let out = git_cmd(self.path()).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// Writes `files` and commits them; returns the commit SHA.
    fn commit(&self, files: &[(&str, &str)], message: &str) -> String {
        for (path, content) in files {
            let full = self.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, content).unwrap();
        }
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }
}

struct Harness {
    _dir: TempDir,
    server: keys::AgentKey,
    ci: Ci,
    store: Arc<aver::store::Store>,
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::new(dir.path().join("home"));
    let init = keys::init(&home).unwrap();
    let store = Arc::new(home.open_store().unwrap());
    let server_key = Arc::new(init.server.signing_key.clone());
    let executor = Executor::new(
        store.clone(),
        Arc::new(scripted::Scripted::default()),
        ExecConfig::default(),
    );
    let ci = Ci::new(
        store.clone(),
        server_key,
        &init.admin,
        Arc::new(StaticResolver::default()),
        executor,
    );
    Harness {
        _dir: dir,
        server: init.server,
        ci,
        store,
    }
}

fn request(repo: &Repo, base: &str, head: &str, spec: Spec) -> ci::Request {
    ci::Request::new(
        repo.path().display().to_string(),
        base.to_owned(),
        head.to_owned(),
        spec,
        None,
    )
}

fn markdown(h: &Harness, outcome: &ci::Outcome) -> String {
    markdown_with(h, outcome, true)
}

fn markdown_with(h: &Harness, outcome: &ci::Outcome, throwaway_key: bool) -> String {
    summary::render(&summary::Context {
        outcome,
        public_key: &h.server.to_file().public_key,
        executor: "scripted-v0",
        throwaway_key,
        fail_on: FailOn::FailOwn,
        viewer_url: Some("https://example.test/receipt.html"),
        reproduce: "aver ci --base a --head b",
    })
}

/// A repository whose base commit has a passing test, a pre-existing failure
/// and a stubbed criterion.
fn calc_like() -> (Repo, String) {
    let repo = Repo::new();
    let base = repo.commit(
        &[
            ("aver.toml", &unit_toml("")),
            (
                "tests.json",
                r#"{"adds": "pass", "legacy": "fail", "[crit:div] divides": "fail"}"#,
            ),
            ("test/fixture.txt", "fixture"),
        ],
        "base",
    );
    (repo, base)
}

#[tokio::test]
async fn a_regression_only_run_fails_own_only_when_the_head_breaks_a_test() {
    let (repo, base) = calc_like();
    let broken = repo.commit(
        &[(
            "tests.json",
            r#"{"adds": "fail", "legacy": "fail", "[crit:div] divides": "fail"}"#,
        )],
        "break adds",
    );
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &broken, Spec::regression_only()))
            .await
            .unwrap();

    assert_eq!(outcome.base.commit.as_str(), base);
    assert_eq!(outcome.head.commit.as_str(), broken);
    let task: Task = h.store.get(&outcome.task_id).unwrap();
    assert_eq!(task.body.schema, TaskSchema::V2);
    assert_eq!(task.body.target, None, "CI moves no ref");
    assert_eq!(task.body.base, outcome.base.snapshot);
    assert_matches_schema("task", &task);

    let receipt = &outcome.receipt;
    receipt
        .verify(&h.server.signing_key.verifying_key())
        .unwrap();
    assert_eq!(receipt.id(), outcome.receipt_id);
    assert_eq!(receipt.body.schema, ReceiptSchema::V3);
    assert_eq!(receipt.body.verdict, Verdict::FailOwn);
    let info = receipt.body.ci.as_ref().expect("a CI receipt carries ci");
    assert_eq!(info.base_commit.as_str(), base);
    assert_eq!(info.head_commit.as_str(), broken);
    assert_eq!(
        info.config_path.as_ref().map(|p| p.as_str()),
        Some("aver.toml")
    );
    assert_eq!(info.criteria_text, None);
    assert_eq!(info.subject, None);
    assert_eq!(
        receipt.body.reproduce,
        format!(
            "aver ci --repo <repo> --base {base} --head {broken} --criteria-sha256 {}",
            Spec::regression_only().sha256()
        ),
        "a local repository is a placeholder; no criteria means no criteria file"
    );
    assert_eq!(
        Spec::regression_only().sha256(),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(receipt.body.head_snapshot, outcome.head.snapshot);
    assert_matches_schema("receipt", receipt);
    assert!(FailOn::FailOwn.fails(receipt.body.verdict));

    let md = markdown(&h, &outcome);
    assert!(md.starts_with("## Aver: FAIL_OWN\n"), "{md}");
    assert!(
        md.contains("\nBase: 1 passed, 2 failed | Yours: 0 passed, 3 failed\n"),
        "the counts follow the headline: {md}"
    );
    let broke = md.split("### What broke").nth(1).unwrap();
    let broke = broke.split("###").next().unwrap();
    assert!(
        broke.contains("`unit::adds` NEW, base pass, head fail"),
        "{md}"
    );
    assert!(
        broke.contains("adds failed"),
        "the NEW detail is shown: {md}"
    );
    assert!(!broke.contains("legacy"), "{md}");
    let others = md.split("### Not caused by this change").nth(1).unwrap();
    assert!(others.contains("`unit::legacy` PREEXISTING"), "{md}");
    assert!(md.contains("Regression-only"), "{md}");
    assert!(md.contains(&format!("`{}`", outcome.receipt_id)), "{md}");
    assert!(
        md.contains(&format!(
            "(https://example.test/receipt.html?key={})",
            h.server.to_file().public_key
        )),
        "{md}"
    );

    let fixed = repo.commit(
        &[(
            "tests.json",
            r#"{"adds": "pass", "legacy": "fail", "[crit:div] divides": "fail", "new one": "pass"}"#,
        )],
        "unbreak and add a test",
    );
    let outcome =
        h.ci.run(request(&repo, &base, &fixed, Spec::regression_only()))
            .await
            .unwrap();
    assert_eq!(
        outcome.receipt.body.verdict,
        Verdict::Pass,
        "a pre-existing failure and an unproven tag do not block regression-only mode"
    );
    let md = markdown(&h, &outcome);
    assert!(md.starts_with("## Aver: PASS\n\nNothing broke"), "{md}");
    let shown = md.split("### What the tests show\n").nth(1).expect(&md);
    let shown = shown.split("###").next().unwrap();
    assert!(
        shown.contains("- Nothing shows the change works: no test the PR added or changed fails on the base and passes on the head"),
        "{md}"
    );
    assert!(
        shown.contains("- Still failing on both sides: 2 tests"),
        "{md}"
    );
}

#[tokio::test]
async fn declared_criteria_are_proven_by_tagged_tests() {
    let (repo, base) = calc_like();
    let head = repo.commit(
        &[(
            "tests.json",
            r#"{"adds": "pass", "legacy": "fail", "[crit:div] divides": "pass"}"#,
        )],
        "implement divide",
    );
    let body =
        "Adds divide.\n\n```aver\n- div: divide works\n- docs: README mentions divide\n```\n";
    let spec = ci::spec::from_markdown(body).unwrap().unwrap();
    let h = harness();

    let outcome =
        h.ci.run(request(&repo, &base, &head, spec.clone()))
            .await
            .unwrap();
    let receipt = &outcome.receipt.body;
    assert_eq!(receipt.schema, ReceiptSchema::V3);
    assert_eq!(receipt.verdict, Verdict::Unproven);
    assert_eq!(receipt.unproven, ["docs".parse::<CriterionId>().unwrap()]);
    let text = "- div: divide works\n- docs: README mentions divide";
    let info = receipt.ci.as_ref().unwrap();
    assert_eq!(info.criteria_text.as_deref(), Some(text));
    assert_eq!(spec.normalized().as_deref(), Some(text));
    assert_eq!(
        ci::spec::from_file(text).unwrap(),
        spec,
        "the text parses back to the spec"
    );
    assert!(
        receipt.reproduce.ends_with(&format!(
            "--criteria-sha256 {} --criteria criteria.txt",
            spec.sha256()
        )),
        "{}",
        receipt.reproduce
    );
    assert_eq!(
        entry(&outcome.receipt, "unit::[crit:div] divides").transplant,
        Some(Transplant::Fail)
    );
    assert_eq!(outcome.diagnostics.unproven.len(), 1);
    assert_eq!(
        outcome.diagnostics.unproven[0].reason,
        aver_core::rules::UnprovenReason::NotClaimed
    );
    let md = markdown(&h, &outcome);
    assert!(
        md.contains("- Proven `div`: divide works (by `unit::[crit:div] divides`, which fails without the change)"),
        "{md}"
    );
    assert!(
        md.contains("- Unproven `docs`: README mentions divide. No test carries its `[crit:<id>]` tag. Closest test names: "),
        "{md}"
    );
    assert!(!FailOn::FailOwn.fails(receipt.verdict));
    assert!(FailOn::NotPass.fails(receipt.verdict));

    let one = ci::spec::from_markdown("```aver\n- div: divide works\n```")
        .unwrap()
        .unwrap();
    let outcome = h.ci.run(request(&repo, &base, &head, one)).await.unwrap();
    assert_eq!(outcome.receipt.body.verdict, Verdict::Pass);
    let task: Task = h.store.get(&outcome.task_id).unwrap();
    assert_eq!(task.body.schema, TaskSchema::V0);
}

#[tokio::test]
async fn changing_a_protected_path_fails_even_when_tests_pass() {
    let (repo, base) = calc_like();
    let head = repo.commit(&[("test/fixture.txt", "rewritten")], "edit the fixture");
    let spec = ci::spec::from_markdown("```aver\nprotected: test/**\n```")
        .unwrap()
        .unwrap();
    let h = harness();
    let outcome = h.ci.run(request(&repo, &base, &head, spec)).await.unwrap();
    let task: Task = h.store.get(&outcome.task_id).unwrap();
    assert_eq!(task.body.schema, TaskSchema::V2);
    assert_eq!(outcome.receipt.body.verdict, Verdict::FailOwn);
    let md = markdown(&h, &outcome);
    assert!(md.contains("`protected::test/fixture.txt` NEW"), "{md}");
    assert!(md.contains("Protected paths: `test/**`."), "{md}");
}

/// A CI workflow edit is reviewed with its diff even when every test
/// passes: the summary shows the workflow change a reviewer approves.
#[tokio::test]
async fn a_sensitive_path_is_reviewed_with_its_diff() {
    let (repo, base) = calc_like();
    let head = repo.commit(
        &[(".github/workflows/ci.yml", "on: pull_request_target\n")],
        "widen the trigger",
    );
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    let md = markdown(&h, &outcome);
    assert_eq!(outcome.receipt.body.verdict, Verdict::Review, "{md}");
    assert!(
        md.contains("- `sensitive::.github/workflows/ci.yml`"),
        "{md}"
    );
    assert!(md.contains("+on: pull_request_target"), "{md}");
}

/// Defect: a merge queue (`--fail-on regressions`) holds a change for a
/// person's review, or lets a regression or an unexplained block through.
/// Only a regression or a block the base does not explain fails it;
/// `REVIEW` and `UNPROVEN`, the default's exit 3, pass.
#[tokio::test]
async fn regressions_mode_exits_0_on_review_and_1_on_what_the_change_broke() {
    let h = harness();
    let mut codes = Vec::new();
    let unproven = ci::spec::from_markdown("```aver\n- docs: README mentions divide\n```")
        .unwrap()
        .unwrap();
    for (files, spec) in [
        (
            vec![(".github/workflows/ci.yml", "on: pull_request_target\n")],
            Spec::regression_only(),
        ),
        (vec![("README", "divide\n")], unproven),
        (
            vec![(
                "tests.json",
                r#"{"adds": "fail", "legacy": "fail", "[crit:div] divides": "fail"}"#,
            )],
            Spec::regression_only(),
        ),
    ] {
        let (repo, base) = calc_like();
        let head = repo.commit(&files, "change");
        let outcome = h.ci.run(request(&repo, &base, &head, spec)).await.unwrap();
        let receipt = &outcome.receipt.body;
        codes.push((
            receipt.verdict,
            FailOn::Unexplained.exit_code(receipt),
            FailOn::Regressions.exit_code(receipt),
        ));
    }
    assert_eq!(
        codes,
        [
            (Verdict::Review, 3, 0),
            (Verdict::Unproven, 3, 0),
            (Verdict::FailOwn, 1, 1),
        ]
    );
    assert_eq!(FailOn::Regressions.as_str(), "regressions");
}

const COVERS: &str = "covers = [\"src/**\", \"test/**\", \"tests.json\"]\n";

/// Defect (WP6 PY-3): a hand-written check at the root without `covers`
/// leaves every change in another language than its image's uncovered, so
/// a good Python change is UNPROVEN. Such a check runs the whole
/// repository's suite: it covers every file.
#[tokio::test]
async fn a_root_check_without_covers_covers_every_file() {
    let repo = Repo::new();
    let config = unit_toml("");
    let base = repo.commit(
        &[
            ("aver.toml", &config),
            ("tests.json", r#"{"adds": "pass"}"#),
        ],
        "base",
    );
    let head = repo.commit(&[("src/click/types.py", "x = 1\n")], "change");
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    let receipt = &outcome.receipt.body;
    assert_eq!(receipt.verdict, Verdict::Pass, "{:?}", receipt.uncovered);
    assert!(receipt.uncovered.is_none(), "{:?}", receipt.uncovered);
}

/// The PY-3 rule is for checks a person declared. A check Aver detected at
/// the root covers its image's language only: the default `npm test` says
/// nothing about a Java or Rust change beside it, so that change stays
/// uncovered and the verdict UNPROVEN, not PASS.
#[tokio::test]
async fn a_detected_root_check_covers_only_its_language() {
    let repo = Repo::new();
    let base = repo.commit(
        &[(
            "package.json",
            r#"{"scripts": {"test": "node run-tests.js"}}"#,
        )],
        "base",
    );
    let head = repo.commit(
        &[
            ("src/main/java/app/Pricing.java", "class Pricing {}\n"),
            ("native/src/lib.rs", "pub fn f() {}\n"),
            ("lib.js", "x"),
        ],
        "change",
    );
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    let receipt = &outcome.receipt.body;
    let uncovered: Vec<String> = receipt
        .uncovered
        .as_ref()
        .map(|u| u.items.iter().map(ToString::to_string).collect())
        .unwrap_or_default();
    assert_eq!(
        uncovered,
        ["native/src/lib.rs", "src/main/java/app/Pricing.java"]
    );
    assert_eq!(receipt.verdict, Verdict::Unproven);
}

/// The scope block says what the run proved (base tests judged from the
/// base's copies, new tests that fail on the base and pass on the head) and
/// what a person should look at because no check could.
#[tokio::test]
async fn the_summary_says_what_was_proven_and_what_aver_could_not_check() {
    let repo = Repo::new();
    let config = format!(
        "{}\n[[check]]\nid = \"files\"\nrun = [\"junit-dir\", \"files.xml\", \"test\"]\nreport = {{ format = \"junit\", path = \"/aver/out/files.xml\" }}\n{COVERS}\n[[check]]\nid = \"lint\"\nrun = [\"exit\", \"0\"]\n{COVERS}",
        unit_toml(COVERS)
    );
    let base = repo.commit(
        &[
            ("aver.toml", &config),
            ("tests.json", r#"{"adds": "pass", "subtracts": "pass"}"#),
            ("test/math.test.json", r#"{"keeps": "pass"}"#),
        ],
        "base",
    );
    let head = repo.commit(
        &[
            (
                "test/div.test.json",
                r#"{"div splits": "pass-if:src/div.js"}"#,
            ),
            ("src/div.js", "x"),
            ("tools/gen.py", "print(1)\n"),
            (".github/workflows/ci.yml", "on: push\n"),
        ],
        "implement divide",
    );
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    let md = markdown(&h, &outcome);
    let scope = md
        .split("### What this verdict covers\n\n")
        .nth(1)
        .unwrap()
        .split("###")
        .next()
        .unwrap();
    assert_eq!(outcome.receipt.body.verdict, Verdict::Unproven, "{md}");
    for line in [
        "- Proven: 3 base tests frozen (each judged with the base's own copy), 1 new test failed on the base and passed on the head.\n",
        "- What Aver could not check: 1 changed file no check covers; 1 check judged by its exit code alone; 1 sensitive path modified (`.github/workflows/ci.yml`); a person should look at these.\n",
        "- Not covered: `tools/gen.py`\n",
    ] {
        assert!(scope.contains(line), "{line}\n{scope}");
    }
    assert!(
        md.contains("To cover them, add `covers = [\"tools/**\"]` to the check in `aver.toml` that tests them."),
        "{md}"
    );
    assert!(!scope.contains("- Not covered: 1 changed file"), "{scope}");

    let quiet = repo.commit(&[("src/NOTES.md", "docs")], "docs only");
    let outcome =
        h.ci.run(request(&repo, &head, &quiet, Spec::regression_only()))
            .await
            .unwrap();
    assert!(
        markdown(&h, &outcome).contains(
            "- What Aver could not check: 1 check judged by its exit code alone; a person should look at these.\n"
        ),
        "{}",
        markdown(&h, &outcome)
    );
}

/// An `Aver-Why:` trailer on the head commit is the author's stated reason:
/// summary.md shows it fenced right above the diffs a reviewer judges, and
/// the verdict is what it would be without it.
#[tokio::test]
async fn an_aver_why_trailer_reaches_the_summary_fenced() {
    let (repo, base) = calc_like();
    let message = "widen the trigger\n\nThe old trigger missed forks.\n\nAver-Why: forks need CI ```too\nSigned-off-by: someone";
    let head = repo.commit(
        &[(".github/workflows/ci.yml", "on: [push, pull_request]\n")],
        message,
    );
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    assert_eq!(outcome.receipt.body.verdict, Verdict::Review);
    assert_eq!(outcome.diagnostics.reason(), Some("forks need CI ```too"));
    let md = markdown(&h, &outcome);
    let at = md
        .find("Stated reason (from the agent, not checked):\n\n````text\nforks need CI ```too\n````\n")
        .unwrap_or_else(|| panic!("{md}"));
    let diff = md
        .find("+on: [push, pull_request]")
        .unwrap_or_else(|| panic!("{md}"));
    assert!(at < diff, "the reason sits above the diffs: {md}");

    let plain = repo.commit(&[(".github/workflows/ci.yml", "on: push\n")], "no reason");
    let outcome =
        h.ci.run(request(&repo, &base, &plain, Spec::regression_only()))
            .await
            .unwrap();
    assert_eq!(outcome.diagnostics.reason(), None);
    assert!(!markdown(&h, &outcome).contains("Stated reason"));
}

#[tokio::test]
async fn hostile_test_names_and_details_stay_inside_code() {
    let repo = Repo::new();
    let base = repo.commit(
        &[
            ("aver.toml", &unit_toml("")),
            ("tests.json", r#"{"ok": "pass"}"#),
        ],
        "base",
    );
    let hostile = "@octocat ``` [link](https://evil.test) *bold*";
    let head = repo.commit(
        &[(
            "tests.json",
            &serde_json::json!({ "ok": "pass", hostile: "fail" }).to_string(),
        )],
        "hostile",
    );
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    let md = markdown(&h, &outcome);
    assert!(
        md.contains(&format!(
            "\n- ````unit::{hostile}```` NEW, base n/a, head fail\n"
        )),
        "the test id is one code span whose fence outgrows the backticks inside: {md}"
    );
    assert!(
        md.contains(&format!("\n  ````text\n  {hostile} failed\n  ````\n")),
        "the detail is one fenced block: {md}"
    );
    assert_eq!(
        md.matches("@octocat").count(),
        2,
        "the name appears only in the span and the block: {md}"
    );
}

#[tokio::test]
async fn an_unknown_revision_is_an_import_error_naming_the_side() {
    let (repo, base) = calc_like();
    let h = harness();
    let err =
        h.ci.run(request(
            &repo,
            &base,
            "no-such-branch",
            Spec::regression_only(),
        ))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, CiError::Import { side: "head", rev, .. } if rev == "no-such-branch"),
        "{err}"
    );
}

#[test]
fn the_cli_reports_a_malformed_block_with_exit_2_and_a_summary() {
    let out_dir = tempfile::tempdir().unwrap();
    std::fs::write(out_dir.path().join("receipt.json"), "stale").unwrap();
    let event = out_dir.path().join("event.json");
    std::fs::write(
        &event,
        serde_json::json!({ "pull_request": { "number": 7, "body": "Fix\n\n```aver\n- div: works\nwhat is this\n```" } }).to_string(),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_aver"))
        .args(["ci", "--base", "HEAD~1", "--head", "HEAD", "--github-event"])
        .arg(&event)
        .arg("--out-dir")
        .arg(out_dir.path())
        .env("AVER_HOME", out_dir.path().join("must-not-be-used"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("line 5 of the aver block: expected `- <id>: <text>` or `protected: <glob>`, found \"what is this\""),
        "{stderr}"
    );
    let summary = std::fs::read_to_string(out_dir.path().join("summary.md")).unwrap();
    assert!(summary.starts_with("## Aver: error\n"), "{summary}");
    assert!(summary.contains("line 5 of the aver block"), "{summary}");
    assert!(
        !out_dir.path().join("receipt.json").exists(),
        "a stale receipt is removed"
    );
    assert!(!out_dir.path().join("must-not-be-used").exists());

    let out = Command::new(env!("CARGO_BIN_EXE_aver"))
        .args([
            "ci",
            "--base",
            "a",
            "--head",
            "b",
            "--criteria",
            "x",
            "--pr-body",
            "y",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "the criteria sources are exclusive"
    );
}

/// A subject is recorded in the signed receipt; a URL repository appears in
/// the reproduce line without its credentials, a local one as `<repo>`.
#[tokio::test]
async fn the_subject_and_a_shareable_reproduce_line_are_signed() {
    let (repo, base) = calc_like();
    let head = repo.commit(&[("README.md", "divide")], "docs");
    let h = harness();
    let mut req = request(&repo, &base, &head, Spec::regression_only());
    req.subject = Some("acme/calc#12 run 7".parse().unwrap());
    let outcome = h.ci.run(req).await.unwrap();
    let info = outcome.receipt.body.ci.as_ref().unwrap();
    assert_eq!(
        info.subject.as_ref().unwrap().as_str(),
        "acme/calc#12 run 7"
    );
    outcome
        .receipt
        .verify(&h.server.signing_key.verifying_key())
        .unwrap();
    assert!(markdown(&h, &outcome).contains("Subject: `acme/calc#12 run 7`"));

    let sha: GitSha = "1".repeat(40).parse().unwrap();
    let spec = Spec::regression_only();
    assert!(ci::reproduce(
        "https://user:secret@example.test/a/b.git",
        &sha,
        &sha,
        &spec
    )
    .starts_with("aver ci --repo https://example.test/a/b.git --base 1111"));
    assert!(ci::reproduce("git@example.test:a/b.git", &sha, &sha, &spec)
        .starts_with("aver ci --repo git@example.test:a/b.git "));
    assert!(ci::reproduce("/srv/repo", &sha, &sha, &spec).starts_with("aver ci --repo <repo> "));
    assert!(
        ci::reproduce("file:///srv/repo", &sha, &sha, &spec).starts_with("aver ci --repo <repo> ")
    );
}

/// `--criteria-sha256` refuses criteria that do not hash to the given value,
/// before anything runs, and says how to supply them.
#[test]
fn a_criteria_hash_mismatch_refuses_to_run() {
    let dir = tempfile::tempdir().unwrap();
    let criteria = dir.path().join("criteria.txt");
    std::fs::write(&criteria, "- div: divide works\n").unwrap();
    let out_dir = dir.path().join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_aver"))
        .args([
            "ci",
            "--base",
            "a",
            "--head",
            "b",
            "--criteria-sha256",
            &"0".repeat(64),
            "--criteria",
        ])
        .arg(&criteria)
        .arg("--out-dir")
        .arg(&out_dir)
        .env("AVER_HOME", dir.path().join("must-not-be-used"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let spec = ci::spec::from_file("- div: divide works").unwrap();
    assert!(
        stderr.contains(&format!("the criteria hash to {}", spec.sha256())),
        "{stderr}"
    );
    assert!(std::fs::read_to_string(out_dir.join("summary.md"))
        .unwrap()
        .contains("--criteria-sha256"));
    assert!(!dir.path().join("must-not-be-used").exists());
}

/// The summary leads with what the pull request did to the tests: deleted
/// and skipped tests still ran from the base version and say what they
/// found (a test the change broke and then removed is NEW), and frozen
/// harness changes are named.
#[tokio::test]
async fn the_summary_says_what_the_pull_request_did_to_the_tests() {
    let repo = Repo::new();
    let config = format!(
        "{}\n[[check]]\nid = \"files\"\nrun = [\"junit-dir\", \"files.xml\", \"test\"]\nreport = {{ format = \"junit\", path = \"/aver/out/files.xml\" }}\n",
        unit_toml("")
    );
    let base = repo.commit(
        &[
            ("aver.toml", &config),
            ("tests.json", r#"{"adds": "pass"}"#),
            ("src/add.js", "x"),
            (
                "test/math.test.json",
                r#"{"keeps": "pass", "gone": "pass-if:src/add.js", "quiet": "pass-if:src/add.js"}"#,
            ),
            ("package.json", r#"{"scripts": {"test": "node --test"}}"#),
        ],
        "base",
    );
    repo.git(&["rm", "-q", "src/add.js"]);
    let head = repo.commit(
        &[
            (
                "test/math.test.json",
                r#"{"keeps": "pass", "quiet": "skip"}"#,
            ),
            ("test/new.test.json", r#"{"fresh": "pass"}"#),
            ("package.json", r#"{"scripts": {"test": "exit 0"}}"#),
        ],
        "tamper",
    );
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    let tc = outcome.receipt.body.test_changes.as_ref().unwrap();
    assert_eq!(tc.tests_removed.total(), 1);
    assert_eq!(tc.tests_skipped.total(), 1);
    let md = markdown_with(&h, &outcome, false);
    let section = md
        .split("### What this PR did to the tests\n\n")
        .nth(1)
        .unwrap()
        .split("###")
        .next()
        .unwrap();
    for line in [
        "- Added 1 test file: `test/new.test.json`.",
        "- Modified 1 existing test file: `test/math.test.json`. Regressions were judged with the base's versions",
        "- Deleted 1 existing test (they still ran from the base version: 1 now fails as NEW): `files::gone`.",
        "- Skipped 1 existing test in the PR's version (they still ran from the base version: 1 now fails as NEW): `files::quiet`.",
        "- Changed frozen test harness (1 change, CHANGED; verification ran the base's version): `judging::package.json#scripts`.",
    ] {
        assert!(section.contains(line), "{line}\n{md}");
    }
    assert_eq!(outcome.receipt.body.verdict, Verdict::FailOwn, "{md}");
    assert!(
        md.find("### What this PR did to the tests") < md.find("### Needs review: test changes")
    );
    assert!(
        md.contains("- `files::gone` NEW, base pass, head fail"),
        "{md}"
    );
    assert!(
        md.contains("- `files::quiet` NEW, base pass, head fail"),
        "{md}"
    );
    assert!(md.contains("Sandbox: the `scripted-v0` executor."), "{md}");
    assert!(
        md.contains("verifier key of the Aver home this run used"),
        "a persistent home's key is not called generated: {md}"
    );
    assert!(!md.contains("generated for this run"), "{md}");
    assert!(markdown(&h, &outcome).contains("generated for this run"));
    assert!(!md.contains("No aver.toml at base"), "{md}");

    let quiet = repo.commit(&[("README.md", "docs")], "docs only");
    let outcome =
        h.ci.run(request(&repo, &head, &quiet, Spec::regression_only()))
            .await
            .unwrap();
    assert!(markdown(&h, &outcome).contains(
        "### What this PR did to the tests\n\nNothing: no test file or test harness changed"
    ));
}

/// A base without aver.toml is verified with the default environment, and
/// the receipt and the summary say so; `--require-config` refuses it.
#[tokio::test]
async fn a_base_without_aver_toml_is_named_and_can_be_refused() {
    let repo = Repo::new();
    let base = repo.commit(&[("package.json", "{}")], "no config");
    let head = repo.commit(&[("README.md", "docs")], "docs");
    let h = harness();
    let mut req = request(&repo, &base, &head, Spec::regression_only());
    req.require_config = true;
    let err = h.ci.run(req).await.unwrap_err();
    assert!(matches!(err, CiError::NoConfig(_)), "{err}");
    assert!(err.to_string().contains("has no aver.toml"), "{err}");

    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    assert_eq!(outcome.receipt.body.config, Some(ConfigSource::Default));
    assert_eq!(outcome.receipt.body.ci.as_ref().unwrap().config_path, None);
    assert!(
        markdown(&h, &outcome).contains("No aver.toml at base: used the default checks, `npm test` and for a TypeScript project a type check")
    );
}

/// Zero-config CI reads only the exit code of a `test` script whose runner
/// Aver does not know. The script is the repository's own, merged by a
/// person, so it counts; `npm test` where `package.json` declares no
/// script is Aver's guess, and its exit code alone is no evidence.
#[tokio::test]
async fn a_detected_exit_only_check_counts_only_as_the_repositorys_own_script() {
    let repo = Repo::new();
    let base = repo.commit(
        &[(
            "package.json",
            r#"{"scripts": {"test": "node run-tests.js"}}"#,
        )],
        "own script",
    );
    let head = repo.commit(&[("lib.js", "x")], "change");
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    assert_eq!(
        outcome.receipt.body.verdict,
        Verdict::Pass,
        "{:?}",
        outcome.receipt.body.checks
    );
    let md = markdown(&h, &outcome);
    assert!(
        md.contains("judged by its exit code alone: its `test` script runs `node`, which is not a test runner Aver reads per test"),
        "{md}"
    );

    let repo = Repo::new();
    let base = repo.commit(&[("package.json", "{}")], "no script");
    let head = repo.commit(&[("lib.js", "x")], "change");
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    assert_eq!(outcome.receipt.body.verdict, Verdict::BlockedNotOwn);
    let ids: Vec<&str> = outcome
        .receipt
        .body
        .checks
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    assert!(ids.contains(&"test::(evidence) exit code only"), "{ids:?}");
}

#[tokio::test]
async fn possible_secrets_are_reported_not_refused_unless_strict() {
    let (repo, base) = calc_like();
    let key = format!("-----BEGIN RSA {}-----\nMIIfake\n", "PRIVATE KEY");
    let head = repo.commit(
        &[
            ("test/fixtures/.env", "API_TOKEN=fixture\n"),
            ("src/demo-key.js", &format!("module.exports = `{key}`;\n")),
        ],
        "add fixtures",
    );
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    assert_eq!(outcome.receipt.body.verdict, Verdict::Pass);
    let notes = &outcome.import;
    let got: Vec<(&str, &str, &str, Vec<&str>)> = notes
        .secrets
        .iter()
        .map(|s| {
            (
                s.path.as_str(),
                s.pattern.as_str(),
                s.severity.as_str(),
                s.sides.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("src/demo-key.js", "pem-private-key", "high", vec!["head"]),
            ("test/fixtures/.env", "dotenv-file", "low", vec!["head"]),
        ]
    );
    let mut diagnostics = outcome.diagnostics.clone();
    diagnostics.import = Some(notes.clone());
    assert_value_matches_schema("diagnostics", &serde_json::to_value(&diagnostics).unwrap());
    let md = markdown(&h, &outcome);
    assert!(
        md.contains("Possible secrets in: `src/demo-key.js` (pem-private-key), `test/fixtures/.env` (dotenv-file, low)."),
        "{md}"
    );
    assert!(!md.contains("MIIfake") && !md.contains("API_TOKEN"), "{md}");

    let mut strict = request(&repo, &base, &head, Spec::regression_only());
    strict.strict_secrets = true;
    let err = h.ci.run(strict).await.unwrap_err();
    assert!(
        matches!(&err, CiError::Import { side: "head", .. }),
        "{err}"
    );
    assert!(err.to_string().contains("pem-private-key"), "{err}");
}

#[tokio::test]
async fn a_command_level_check_red_on_both_sides_blocks_instead_of_passing() {
    let repo = Repo::new();
    let config = unit_toml("\n[[check]]\nid = \"lint\"\nrun = [\"exit\", \"1\"]\n");
    let base = repo.commit(
        &[
            ("aver.toml", &config),
            ("tests.json", r#"{"adds": "pass", "legacy": "fail"}"#),
        ],
        "base",
    );
    let head = repo.commit(&[("src/x.js", "module.exports = 1;\n")], "change");
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    let receipt = &outcome.receipt.body;
    let lint = receipt.checks.get("lint").unwrap();
    assert_eq!(lint.cause, Some(Cause::Preexisting));
    assert_eq!(
        receipt.checks.get("unit::legacy").unwrap().cause,
        Some(Cause::Preexisting)
    );
    assert_eq!(receipt.verdict, Verdict::BlockedNotOwn);
    // A check read by its exit code alone that fails on the base explains
    // nothing: no test result says what it ran there (WP6 ky).
    assert!(FailOn::Unexplained.fails_receipt(receipt));
    let md = markdown(&h, &outcome);
    assert!(
        md.contains("Blocked: a check that fails on the base too hides what this change did"),
        "{md}"
    );
    assert!(
        md.contains("`lint` fails on the base too and has no per-test results"),
        "{md}"
    );
    assert!(md.contains("Per-test PREEXISTING entries"), "{md}");
    let lint = outcome
        .diagnostics
        .failures
        .iter()
        .find(|f| f.id.as_str() == "lint")
        .unwrap();
    assert!(lint.note.as_deref().unwrap().starts_with("inconclusive"));
    let legacy = outcome
        .diagnostics
        .failures
        .iter()
        .find(|f| f.id.as_str() == "unit::legacy")
        .unwrap();
    assert_eq!(legacy.note, None);
    assert_value_matches_schema(
        "diagnostics",
        &serde_json::to_value(&outcome.diagnostics).unwrap(),
    );
}

#[tokio::test]
async fn a_base_that_is_not_an_ancestor_is_replaced_by_the_merge_base() {
    let (repo, fork) = calc_like();
    let head = repo.commit(&[("src/feature.js", "module.exports = 1;\n")], "feature");
    repo.git(&["checkout", "-q", "-b", "trunk", &fork]);
    let moved = repo.commit(
        &[(
            "tests.json",
            r#"{"adds": "pass", "legacy": "fail", "[crit:div] divides": "fail", "trunk only": "pass"}"#,
        )],
        "trunk moves on",
    );
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &moved, &head, Spec::regression_only()))
            .await
            .unwrap();
    assert!(outcome.resolution.moved());
    assert_eq!(outcome.base.commit.as_str(), fork);
    let info = outcome.receipt.body.ci.as_ref().unwrap();
    assert_eq!(info.base_commit.as_str(), fork);
    assert_eq!(
        info.requested_base.as_ref().map(|c| c.as_str()),
        Some(moved.as_str())
    );
    assert!(outcome
        .receipt
        .body
        .reproduce
        .contains(&format!("--base {fork}")));
    assert_eq!(
        outcome.receipt.body.verdict,
        Verdict::Pass,
        "the trunk's newer test is not counted as deleted by the head"
    );
    assert!(outcome
        .receipt
        .body
        .checks
        .get("unit::trunk only")
        .is_none());
    assert_matches_schema("receipt", &outcome.receipt);
    let md = markdown(&h, &outcome);
    assert!(md.contains("is not an ancestor of the head"), "{md}");

    let outcome =
        h.ci.run(request(&repo, &fork, &head, Spec::regression_only()))
            .await
            .unwrap();
    assert!(!outcome.resolution.moved());
    assert_eq!(
        outcome.receipt.body.ci.as_ref().unwrap().requested_base,
        None
    );
}

/// A pull request no check is affected by verifies without running any:
/// `summary.md` names the checks it did not run and asks for no test, and
/// `--all` (the request's `all`) runs them, with `--all` in `reproduce`.
#[tokio::test]
async fn checks_the_pull_request_does_not_affect_are_named_and_not_run() {
    let repo = Repo::new();
    let config = unit_toml("covers = [\"src/**\", \"tests.json\"]\n");
    let base = repo.commit(
        &[
            ("aver.toml", &config),
            ("tests.json", r#"{"adds": "pass"}"#),
            ("src/a.js", "1"),
        ],
        "base",
    );
    let head = repo.commit(&[("docs/guide.md", "# Guide\n")], "docs");
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    let receipt = &outcome.receipt.body;
    assert_eq!(receipt.verdict, Verdict::Pass);
    assert!(receipt.checks.is_empty());
    let skipped: Vec<&str> = receipt
        .skipped
        .iter()
        .flat_map(|s| s.iter().map(|s| s.check.as_str()))
        .collect();
    assert_eq!(skipped, ["unit"]);
    let md = markdown(&h, &outcome);
    assert!(md.contains("### Checks not run"), "{md}");
    assert!(
        md.contains("No check is affected by this change, so none ran: `unit`"),
        "{md}"
    );
    assert!(!md.contains("Nothing shows the change works"), "{md}");

    let mut all = request(&repo, &base, &head, Spec::regression_only());
    all.all = true;
    let outcome = h.ci.run(all).await.unwrap();
    assert!(outcome.receipt.body.skipped.is_none());
    assert!(outcome.receipt.body.checks.get("unit::adds").is_some());
    assert!(outcome.receipt.body.reproduce.ends_with(" --all"));
}

/// The default `--fail-on unexplained` fails a BLOCKED_NOT_OWN whose base
/// was green: the head's own sandbox runs failed, nothing showed the
/// change is safe, and no failure of the base stands in the way.
#[tokio::test]
async fn a_block_over_a_green_base_fails_the_default() {
    let repo = Repo::new();
    let config =
        unit_toml("\n[[check]]\nid = \"build\"\nrun = [\"sandbox-fail-if\", \"src/x.js\"]\n");
    let base = repo.commit(
        &[
            ("aver.toml", &config),
            ("tests.json", r#"{"adds": "pass"}"#),
        ],
        "base",
    );
    let head = repo.commit(&[("src/x.js", "module.exports = 1;\n")], "change");
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    let receipt = &outcome.receipt.body;
    let build = receipt.checks.get("build").unwrap();
    assert_eq!(
        (build.base, build.cause),
        (BaseStatus::Pass, Some(Cause::Env))
    );
    assert_eq!(receipt.verdict, Verdict::BlockedNotOwn);
    assert!(FailOn::Unexplained.fails_receipt(receipt));
    assert!(FailOn::Regressions.fails_receipt(receipt));
    assert!(!FailOn::FailOwn.fails_receipt(receipt));
    let md = summary::render(&summary::Context {
        outcome: &outcome,
        public_key: &h.server.to_file().public_key,
        executor: "scripted-v0",
        throwaway_key: true,
        fail_on: FailOn::Unexplained,
        viewer_url: None,
        reproduce: "aver ci --base a --head b",
    });
    assert!(md.contains("The check fails: `--fail-on unexplained` fails on BLOCKED_NOT_OWN when the base does not explain the block"), "{md}");
}

/// A red check (it fails on every base run) whose head runs end in
/// infrastructure: the head is retried, the entry is `ENV` and the verdict
/// `BLOCKED_NOT_OWN`, but the base completed, so its failure does not
/// explain the block and the default `--fail-on unexplained` fails it.
/// Before, a base `fail` explained any `ENV` entry, so a head that provoked
/// a sandbox failure over a red base exited 0.
#[tokio::test]
async fn head_only_infra_over_a_red_base_fails_the_default() {
    let repo = Repo::new();
    let config =
        unit_toml("\n[[check]]\nid = \"build\"\nrun = [\"fail-or-sandbox-if\", \"src/x.js\"]\n");
    let base = repo.commit(
        &[
            ("aver.toml", &config),
            ("tests.json", r#"{"adds": "pass"}"#),
        ],
        "base",
    );
    let head = repo.commit(&[("src/x.js", "module.exports = 1;\n")], "change");
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    let receipt = &outcome.receipt.body;
    let build = receipt.checks.get("build").unwrap();
    assert_eq!(
        (build.base, build.cause),
        (BaseStatus::Fail, Some(Cause::Env))
    );
    assert_eq!(
        build.runs.base.len(),
        2,
        "the red base completed, failing the same way twice"
    );
    assert_eq!(
        build.runs.head.len(),
        2,
        "the head's infra run was retried once"
    );
    assert_eq!(receipt.verdict, Verdict::BlockedNotOwn);
    assert!(FailOn::Unexplained.fails_receipt(receipt));
    assert!(!FailOn::FailOwn.fails_receipt(receipt));
    let md = summary::render(&summary::Context {
        outcome: &outcome,
        public_key: &h.server.to_file().public_key,
        executor: "scripted-v0",
        throwaway_key: true,
        fail_on: FailOn::Unexplained,
        viewer_url: None,
        reproduce: "aver ci --base a --head b",
    });
    assert!(
        md.contains("The check fails: `--fail-on unexplained`"),
        "{md}"
    );
}

/// The other way round: every base run ends in infrastructure (a tool the
/// base cannot find, a layer that cannot install) and the head completes
/// red. No test ran on the base, so it explains nothing: before, its infra
/// counted as a red base and every pull request of a repository whose
/// checks cannot run exited 0 without a test run (red team pass 2, §1).
#[tokio::test]
async fn a_base_whose_runs_all_end_in_infra_fails_the_default() {
    let repo = Repo::new();
    let config = unit_toml(
        "\n[[check]]\nid = \"build\"\nrun = [\"fail-or-sandbox-unless\", \"src/x.js\"]\n",
    );
    let base = repo.commit(
        &[
            ("aver.toml", &config),
            ("tests.json", r#"{"adds": "pass"}"#),
        ],
        "base",
    );
    let head = repo.commit(&[("src/x.js", "module.exports = 1;\n")], "change");
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    let receipt = &outcome.receipt.body;
    let build = receipt.checks.get("build").unwrap();
    assert_eq!(build.cause, Some(Cause::Env));
    assert_eq!(
        build.runs.base.len(),
        2,
        "the base's infra run was retried once"
    );
    assert_eq!(receipt.verdict, Verdict::BlockedNotOwn);
    assert!(FailOn::Unexplained.fails_receipt(receipt));
    assert!(FailOn::NotPass.fails_receipt(receipt));
}

/// A check that fails as a whole shows the last lines of its output under
/// the headline, slot paths taken out (REDESIGN.md §5.1).
#[tokio::test]
async fn a_check_failing_as_a_whole_shows_its_output_tail() {
    let repo = Repo::new();
    let config = unit_toml("\n[[check]]\nid = \"lint\"\nrun = [\"say\", \"lint.txt\", \"1\"]\n");
    let lines = |n: usize| -> String {
        (1..=n)
            .map(|i| format!("<src>/src/x.js:{i}: problem {i}\n"))
            .collect()
    };
    let base = repo.commit(
        &[
            ("aver.toml", &config),
            ("tests.json", r#"{"adds": "pass"}"#),
            ("lint.txt", &lines(3)),
        ],
        "base",
    );
    let head = repo.commit(&[("lint.txt", &lines(20))], "change");
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    let md = markdown(&h, &outcome);
    let tail = md
        .split("lines of output of `lint`, which failed as a whole:")
        .nth(1)
        .unwrap_or_else(|| panic!("{md}"));
    let tail = tail.split("\n\n").nth(1).unwrap_or_default();
    assert!(tail.contains("src/x.js:20: problem 20"), "{md}");
    assert!(tail.contains("src/x.js:6: problem 6"), "{md}");
    assert!(
        !tail.contains("problem 5\n"),
        "only the last 15 lines: {md}"
    );
    assert!(!tail.contains("<src>"), "{md}");
}

/// `aver ci --check <id>` (the action's matrix `check` input): only that
/// check runs, the receipt has no entry of another and its `reproduce`
/// line repeats `--check`, the summary says what the receipt covers, and
/// an id the base does not define is refused, naming the ones it does.
#[tokio::test]
async fn one_check_of_a_matrix_covers_only_itself() {
    let repo = Repo::new();
    let config =
        unit_toml("\n[[check]]\nid = \"build\"\nrun = [\"sandbox-fail-if\", \"src/x.js\"]\n");
    let base = repo.commit(
        &[
            ("aver.toml", &config),
            ("tests.json", r#"{"adds": "pass"}"#),
        ],
        "base",
    );
    let head = repo.commit(&[("src/x.js", "module.exports = 1;\n")], "change");
    let h = harness();
    let mut only = request(&repo, &base, &head, Spec::regression_only());
    only.only = Some("unit".parse().unwrap());
    let outcome = h.ci.run(only).await.unwrap();
    let receipt = &outcome.receipt.body;
    assert!(receipt.checks.get("unit::adds").is_some());
    assert!(
        receipt.checks.get("build").is_none(),
        "{:?}",
        receipt.checks
    );
    assert_ne!(receipt.verdict, Verdict::BlockedNotOwn);
    assert!(
        receipt.reproduce.ends_with(" --check unit"),
        "{}",
        receipt.reproduce
    );
    let md = markdown(&h, &outcome);
    assert!(
        md.contains("This receipt covers the check `unit` only (`--check`); `build` ran in jobs of their own, or not at all."),
        "{md}"
    );

    let mut unknown = request(&repo, &base, &head, Spec::regression_only());
    unknown.only = Some("lint".parse().unwrap());
    let err = h.ci.run(unknown).await.unwrap_err().to_string();
    assert!(
        err.contains("--check lint: the base has no such check"),
        "{err}"
    );
    assert!(err.contains("unit") && err.contains("build"), "{err}");
}

/// A check red on both sides explains only itself: beside it, a check whose
/// head ended in infrastructure is not explained, so the default fails the
/// run. Before, one base-failing blocker explained the whole receipt and a
/// regression whose other checks never ran exited 0 (WP6 matrix JS-5).
#[tokio::test]
async fn a_red_check_beside_an_env_entry_fails_the_default() {
    let repo = Repo::new();
    let config = unit_toml(concat!(
        "\n[[check]]\nid = \"lint\"\nrun = [\"exit\", \"1\"]\n",
        "\n[[check]]\nid = \"build\"\nrun = [\"sandbox-fail-if\", \"src/x.js\"]\n",
    ));
    let base = repo.commit(
        &[
            ("aver.toml", &config),
            ("tests.json", r#"{"adds": "pass"}"#),
        ],
        "base",
    );
    let head = repo.commit(&[("src/x.js", "module.exports = 1;\n")], "change");
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    let receipt = &outcome.receipt.body;
    let cause = |id: &str| receipt.checks.get(id).unwrap().cause;
    assert_eq!(cause("lint"), Some(Cause::Preexisting));
    assert_eq!(cause("build"), Some(Cause::Env));
    assert_eq!(receipt.verdict, Verdict::BlockedNotOwn);
    assert!(!ci::blocked_by_the_base(receipt));
    assert_eq!(FailOn::Unexplained.exit_code(receipt), ci::EXIT_FAIL);
    assert_eq!(FailOn::Regressions.exit_code(receipt), ci::EXIT_FAIL);
    let md = summary::render(&summary::Context {
        outcome: &outcome,
        public_key: &h.server.to_file().public_key,
        executor: "scripted-v0",
        throwaway_key: true,
        fail_on: FailOn::Unexplained,
        viewer_url: None,
        reproduce: "aver ci --base a --head b",
    });
    assert!(
        md.contains("The check fails: `--fail-on unexplained` fails on BLOCKED_NOT_OWN"),
        "{md}"
    );
}

/// An exit-only check whose command a shell cannot find on either side
/// (`sh: mocha: command not found`, exit 127) ran nothing: `ENV`, naming
/// the line and the missing dependencies, never a base failure that
/// explains the head's.
#[tokio::test]
async fn an_exit_only_check_whose_command_is_missing_is_env() {
    let repo = Repo::new();
    let config = unit_toml("\n[[check]]\nid = \"test\"\nrun = [\"say\", \"out.txt\", \"127\"]\n");
    let base = repo.commit(
        &[
            ("aver.toml", &config),
            ("tests.json", r#"{"adds": "pass"}"#),
            (
                "out.txt",
                "\n> pkg@1.0.0 test\n> mocha\n\nsh: mocha: command not found\n",
            ),
        ],
        "base",
    );
    let head = repo.commit(&[("src/x.js", "module.exports = 1;\n")], "regression");
    let h = harness();
    let outcome =
        h.ci.run(request(&repo, &base, &head, Spec::regression_only()))
            .await
            .unwrap();
    let receipt = &outcome.receipt.body;
    let test = receipt.checks.get("test").unwrap();
    assert_eq!(test.cause, Some(Cause::Env), "{test:?}");
    let detail = test.detail.as_ref().unwrap().as_str();
    assert!(detail.contains("sh: mocha: command not found"), "{detail}");
    assert!(detail.contains("dependencies are not"), "{detail}");
    assert_eq!(receipt.verdict, Verdict::BlockedNotOwn);
    assert_eq!(FailOn::Unexplained.exit_code(receipt), ci::EXIT_FAIL);
}
