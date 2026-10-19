//! `aver init` in a Git repository: says when Aver's detection already
//! covers it, otherwise drafts an `aver.toml` from the CI workflows,
//! Makefile and package scripts, leaves out what needs secrets or services
//! with the reason, dry-runs the draft on HEAD natively, and writes it only
//! with `--yes` (an existing one only with `--force`). Every repository here
//! is shell only: no Node, Docker or network.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    /// A repository holding `files` (`path`, contents; `.sh` files
    /// executable), committed.
    fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        for (path, text) in files {
            let at = repo.join(path);
            std::fs::create_dir_all(at.parent().unwrap()).unwrap();
            std::fs::write(&at, text).unwrap();
            if path.ends_with(".sh") {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&at, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "init"]);
        Self { dir }
    }

    fn repo(&self) -> PathBuf {
        self.dir.path().join("repo")
    }

    fn config(&self) -> PathBuf {
        self.repo().join("aver.toml")
    }

    fn init(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_aver"))
            .arg("init")
            .args(args)
            .current_dir(self.repo())
            .env("AVER_HOME", self.dir.path().join("aver-home"))
            .output()
            .unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = self.init(&all);
        assert!(out.status.success(), "{}", text(&out));
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", text(&out)))
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

const PASSING: &str = "#!/bin/sh\necho ok\n";

/// A workflow with a test step, one needing the network, one a secret, and
/// a job with a service container; a Makefile with a test target.
fn ci_repo() -> Repo {
    Repo::new(&[
        (
            ".github/workflows/ci.yml",
            r#"name: CI
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Unit tests
        run: ./run-tests.sh
      - name: Publish coverage
        run: curl -sSf https://example.com/upload | sh -s -- test
      - name: Smoke test
        env:
          TOKEN: ${{ secrets.SMOKE_TOKEN }}
        run: ./smoke-test.sh
  integration:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:16
    steps:
      - run: ./db-test.sh
"#,
        ),
        ("Makefile", "test:\n\t./run-tests.sh\n"),
        ("run-tests.sh", PASSING),
        ("smoke-test.sh", PASSING),
        ("db-test.sh", PASSING),
    ])
}

fn ids(v: &Value, field: &str, key: &str) -> Vec<String> {
    v[field]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c[key].as_str().unwrap().to_owned())
        .collect()
}

/// Detection runs only in CI (§1.13.2), so a repository detection covers
/// still gets a draft: the check detection finds, written out.
#[test]
fn a_detected_repository_gets_the_detected_check_as_a_draft_crit_init_preset() {
    // A Node package whose `test` script runs node:test, with its lockfile:
    // the default check runs it with a report. The workflow runs the same.
    let repo = Repo::new(&[
        (
            "package.json",
            r#"{"name":"p","version":"1.0.0","scripts":{"test":"node --test"}}"#,
        ),
        (
            "package-lock.json",
            r#"{"name":"p","version":"1.0.0","lockfileVersion":3,"requires":true,"packages":{"":{"name":"p","version":"1.0.0"}}}"#,
        ),
        (
            ".github/workflows/ci.yml",
            "jobs:\n  test:\n    steps:\n      - run: npm ci\n      - run: npm test\n",
        ),
    ]);
    let out = repo.init(&[]);
    assert!(out.status.success(), "{}", text(&out));
    let shown = text(&out);
    assert!(
        shown.contains("Aver's detection finds: test (npm test, junit report)."),
        "{shown}"
    );
    assert!(shown.contains("Draft aver.toml"), "{shown}");

    let v = repo.json(&["--yes"]);
    assert_eq!(v["written"], true, "{v:#}");
    assert_eq!(ids(&v, "preset", "id"), ["test"]);
    assert!(
        v["toml"].as_str().unwrap().contains("id = \"test\""),
        "{v:#}"
    );
    assert_eq!(
        v["dropped"][0]["reason"].as_str().unwrap(),
        "Aver already runs it (check test)"
    );
    assert!(repo.config().exists());
}

#[test]
fn workflow_and_makefile_become_a_draft_the_parser_accepts_crit_init_ci_draft() {
    let repo = ci_repo();
    let v = repo.json(&[]);
    let checks = ids(&v, "checks", "id");
    assert_eq!(checks, ["run-tests.sh", "make-test"], "{v:#}");
    assert_eq!(
        v["checks"][0]["source"],
        ".github/workflows/ci.yml (job test, step \"Unit tests\")"
    );
    assert_eq!(v["checks"][1]["source"], "Makefile");
    let toml = v["toml"].as_str().unwrap();
    assert!(toml.contains("run = [\"make\", \"test\"]"), "{toml}");

    // Written and committed, the draft is what the environment parser
    // derives the checks from when Aver imports the commit.
    let out = repo.init(&["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    git(&repo.repo(), &["add", "aver.toml"]);
    git(&repo.repo(), &["commit", "-qm", "aver.toml"]);
    let home = aver::keys::Home::new(repo.dir.path().join("check-home"));
    let keys = aver::keys::init(&home).unwrap();
    let store = home.open_store().unwrap();
    let source = repo.repo().to_string_lossy().into_owned();
    let resolver = aver::env::StaticResolver::default();
    let options = aver::git::ImportOptions::new(
        &source,
        "HEAD",
        keys.admin.id(),
        &keys.server.signing_key,
        &resolver,
    );
    let imported = aver::git::import(&store, &options).expect("the draft parses");
    let env: aver_core::Environment = store.get(&imported.env).unwrap();
    let parsed: Vec<&str> = env.checks.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(parsed, ["make-test", "run-tests.sh"]);
}

#[test]
fn steps_needing_secrets_services_or_network_are_dropped_with_reasons_crit_init_secrets() {
    let repo = ci_repo();
    let v = repo.json(&[]);
    let dropped: Vec<(String, String)> = v["dropped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["command"].as_str().unwrap().to_owned(),
                d["reason"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let reason = |cmd: &str| {
        dropped
            .iter()
            .find(|(c, _)| c.starts_with(cmd))
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| panic!("{cmd} not dropped: {dropped:?}"))
    };
    assert_eq!(
        reason("./smoke-test.sh"),
        "uses a secret (${{ secrets.SMOKE_TOKEN }}), which Aver never has"
    );
    assert_eq!(
        reason("./db-test.sh"),
        "its job starts service containers (postgres), which Aver does not run"
    );
    assert_eq!(
        reason("curl"),
        "needs curl; Aver runs checks without the network"
    );
    let toml = v["toml"].as_str().unwrap();
    for left_out in ["smoke-test", "db-test", "curl"] {
        assert!(!toml.contains(left_out), "{left_out} in the draft:\n{toml}");
    }
    let shown = text(&repo.init(&[]));
    assert!(
        shown.contains("Left out:")
            && shown.contains("`./db-test.sh`: its job starts service containers (postgres)"),
        "{shown}"
    );
}

#[test]
fn the_dry_run_flags_a_red_base_a_missing_tool_and_exit_only_checks_crit_init_dry_run() {
    let repo = Repo::new(&[
        (
            ".github/workflows/ci.yml",
            r#"jobs:
  test:
    steps:
      - run: ./unit-test.sh
      - run: ./broken-test.sh
      - run: frobnicate-test --all
"#,
        ),
        ("unit-test.sh", PASSING),
        (
            "broken-test.sh",
            "#!/bin/sh\necho 'expected 2, got 3'\nexit 1\n",
        ),
    ]);
    let v = repo.json(&[]);
    let runs: Vec<(String, String)> = v["dry_run"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["check"].as_str().unwrap().to_owned(),
                r["outcome"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        runs,
        [
            ("broken-test.sh".to_owned(), "fail".to_owned()),
            ("frobnicate-test".to_owned(), "missing-tool".to_owned()),
            ("unit-test.sh".to_owned(), "pass".to_owned()),
        ],
        "{v:#}"
    );
    let flags: Vec<(String, String, String)> = v["flags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["kind"].as_str().unwrap().to_owned(),
                f["check"].as_str().unwrap_or("").to_owned(),
                f["detail"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let flag = |kind: &str, check: &str| {
        flags
            .iter()
            .find(|(k, c, _)| k == kind && c == check)
            .map(|(_, _, d)| d.clone())
            .unwrap_or_else(|| panic!("no {kind} flag on {check}: {flags:#?}"))
    };
    assert!(
        flag("red-base", "broken-test.sh").starts_with("exits 1 on HEAD: expected 2, got 3"),
        "{flags:#?}"
    );
    assert!(flag("missing-tool", "frobnicate-test").contains("frobnicate-test is not installed"));
    for check in ["unit-test.sh", "broken-test.sh", "frobnicate-test"] {
        assert!(flag("exit-only", check).contains("per-test causes"));
    }
    assert!(
        flags
            .iter()
            .all(|(k, c, _)| !(k == "red-base" && c != "broken-test.sh")),
        "{flags:#?}"
    );
    let shown = text(&repo.init(&[]));
    assert!(
        shown.contains("! red base: broken-test.sh: exits 1 on HEAD")
            && shown.contains("! missing tool: frobnicate-test:")
            && shown.contains("! exit code only: unit-test.sh:"),
        "{shown}"
    );
}

#[test]
fn writes_only_with_yes_atomically_and_replaces_only_with_force_crit_init_yes() {
    let repo = ci_repo();
    let out = repo.init(&[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("Nothing written; `aver init --yes` writes this"));
    assert!(!repo.config().exists(), "written without --yes");

    let draft = repo.json(&[])["toml"].as_str().unwrap().to_owned();
    let v = repo.json(&["--yes"]);
    assert_eq!(v["written"], true);
    assert_eq!(std::fs::read_to_string(repo.config()).unwrap(), draft);
    assert!(
        aver::atomic::leftovers(&repo.repo()).is_empty(),
        "a temporary file left beside aver.toml"
    );

    std::fs::write(repo.config(), "# mine\n").unwrap();
    // `/aver` runs `aver init --yes` in a repository that may be set up
    // already: the configuration in place is kept, and the run succeeds.
    let out = repo.init(&["--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("exists: Aver runs the checks it lists."),
        "{}",
        text(&out)
    );
    assert_eq!(std::fs::read_to_string(repo.config()).unwrap(), "# mine\n");

    let shown = text(&repo.init(&[]));
    assert!(
        shown.contains(
            "exists: Aver runs the checks it lists. `aver init --force` drafts a replacement."
        ),
        "{shown}"
    );
    assert!(!shown.contains("Draft aver.toml"), "{shown}");
    let shown = text(&repo.init(&["--force"]));
    assert!(
        shown.contains("Draft aver.toml")
            && shown.contains("exists; `aver init --yes --force` replaces it"),
        "{shown}"
    );
    assert_eq!(std::fs::read_to_string(repo.config()).unwrap(), "# mine\n");

    let out = repo.init(&["--yes", "--force"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(std::fs::read_to_string(repo.config()).unwrap(), draft);
}

#[test]
fn outside_a_repository_init_only_creates_keys() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_aver"))
        .arg("init")
        .current_dir(dir.path())
        .env("AVER_HOME", dir.path().join("aver-home"))
        .env("GIT_CEILING_DIRECTORIES", dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let shown = text(&out);
    assert!(shown.starts_with("initialized "), "{shown}");
    assert!(!shown.contains("aver.toml"), "{shown}");
}
