//! `aver init --ci`: the GitHub Actions workflow it drafts for each
//! ecosystem (goldens in `tests/golden/init-ci/<name>.yml`; regenerate with
//! `AVER_UPDATE_GOLDEN=1 cargo test -p aver --test init_ci` and review the
//! diff), that `action/test/check-workflows.sh` accepts every one, and the
//! command's writing rules and JSON.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use aver::ci::workflow;
use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/init-ci")
}

fn write(dir: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let at = dir.join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
}

/// Every fixture's workflow, and one for a repository with pnpm in a
/// nested package, a Node version file, a pinned Rust toolchain and a
/// directory a shell would need quoted.
fn cases() -> (tempfile::TempDir, Vec<(String, String)>) {
    let mixed = tempfile::tempdir().unwrap();
    write(
        mixed.path(),
        &[
            (".nvmrc", "22\n"),
            ("package.json", "{}"),
            ("web/package.json", "{}"),
            ("web/pnpm-lock.yaml", ""),
            ("rust-toolchain.toml", "[toolchain]\nchannel = \"1.80\"\n"),
            ("Cargo.toml", "[workspace]\n"),
            ("Cargo.lock", ""),
            ("svc/go.mod", "module x\n"),
            ("svc/go.sum", ""),
            ("tools/pyproject.toml", "[project]\nname = \"t\"\n"),
            ("odd dir/package-lock.json", "{}"),
            ("node_modules/x/package-lock.json", "{}"),
        ],
    );
    let mut out: Vec<(String, String)> = [
        ("node", "fixtures/calc"),
        ("python", "fixtures/py-calc"),
        ("python-uv", "fixtures/py-uv"),
        ("rust", "fixtures/rust-calc"),
        ("go", "fixtures/go-deps"),
        ("sh", "fixtures/sh-calc"),
    ]
    .into_iter()
    .map(|(name, dir)| {
        let plan = workflow::plan(&root().join(dir), "main");
        (name.to_owned(), workflow::render(&plan))
    })
    .collect();
    out.push((
        "mixed".into(),
        workflow::render(&workflow::plan(mixed.path(), "trunk")),
    ));
    (mixed, out)
}

#[test]
fn each_ecosystem_gets_its_setup_steps_crit_init_ci_golden() {
    let (_mixed, cases) = cases();
    let update = std::env::var_os("AVER_UPDATE_GOLDEN").is_some();
    std::fs::create_dir_all(golden_dir()).unwrap();
    let mut stale = Vec::new();
    for (name, text) in &cases {
        let path = golden_dir().join(format!("{name}.yml"));
        // The action's commit is the build's; the goldens show it as a
        // placeholder so a release build passes too.
        let text = text.replace(workflow::ACTION_SHA, "{{ACTION_SHA}}");
        if update {
            std::fs::write(&path, &text).unwrap();
        } else if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            stale.push(name.clone());
        }
    }
    assert!(
        stale.is_empty(),
        "goldens differ for {stale:?}; rerun with AVER_UPDATE_GOLDEN=1 and review the diff"
    );
}

/// `check-workflows.sh` reads the workflows of the tree it sits in, so each
/// generated workflow goes into a tree of its own beside a copy of it.
#[test]
fn check_workflows_accepts_every_generated_workflow_crit_init_ci_pinned() {
    let (_mixed, cases) = cases();
    let tree = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tree.path().join(".github/workflows")).unwrap();
    std::fs::create_dir_all(tree.path().join("action/test")).unwrap();
    let script = tree.path().join("action/test/check-workflows.sh");
    std::fs::copy(root().join("action/test/check-workflows.sh"), &script).unwrap();
    for (name, text) in &cases {
        std::fs::write(
            tree.path().join(format!(".github/workflows/{name}.yml")),
            text,
        )
        .unwrap();
    }
    let out = Command::new("bash").arg(&script).output().unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains(&format!("{} YAML files parse", cases.len())),
        "{said}"
    );
}

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

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A repository with an `aver.toml` (so no draft and no dry run) and a
/// lockfile, on the branch `trunk`.
struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write(
            &repo,
            &[
                (
                    "aver.toml",
                    "[[check]]\nid = \"test\"\ncommand = [\"sh\", \"test.sh\"]\nreport = \"exit\"\n",
                ),
                ("test.sh", "exit 0\n"),
                ("package.json", "{}"),
                ("package-lock.json", "{}"),
            ],
        );
        git(&repo, &["init", "-q"]);
        git(&repo, &["checkout", "-q", "-b", "trunk"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "init"]);
        Self { dir }
    }

    fn repo(&self) -> PathBuf {
        self.dir.path().join("repo")
    }

    fn workflow(&self) -> PathBuf {
        self.repo().join(workflow::PATH)
    }

    fn init(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_aver"))
            .args(["init", "--ci"])
            .args(args)
            .current_dir(self.repo())
            .env("AVER_HOME", self.dir.path().join("aver-home"))
            .env_remove("AVER_SCRIPTED")
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

#[test]
fn shows_then_writes_with_yes_and_replaces_only_with_force_crit_init_ci_write() {
    let repo = Repo::new();
    let shown = repo.init(&[]);
    assert!(shown.status.success(), "{}", text(&shown));
    let said = String::from_utf8_lossy(&shown.stdout);
    assert!(said.contains("branches: [trunk]"), "{said}");
    assert!(said.contains("run: npm ci"), "{said}");
    assert!(
        said.contains("Nothing written; `aver init --ci --yes`"),
        "{said}"
    );
    assert!(
        said.contains("gh api -X PUT 'repos/{owner}/{repo}/branches/trunk/protection'"),
        "{said}"
    );
    let explanation = &said[said.find("The workflow runs the action").unwrap()..];
    let explanation = &explanation[..explanation
        .find("For the agent tools")
        .unwrap_or(explanation.len())];
    assert!(explanation.lines().count() <= 15, "{explanation}");
    assert!(!repo.workflow().exists(), "written without --yes");

    let wrote = repo.init(&["--yes"]);
    assert!(wrote.status.success(), "{}", text(&wrote));
    let first = std::fs::read_to_string(repo.workflow()).unwrap();
    assert!(first.contains("review: approve") && first.contains("mode: approve"));
    let config = std::fs::read_to_string(repo.repo().join("aver.toml")).unwrap();
    assert!(config.contains("sh"), "aver.toml kept: {config}");

    std::fs::write(repo.workflow(), "# mine\n").unwrap();
    let refused = repo.init(&["--yes"]);
    assert_eq!(refused.status.code(), Some(2), "{}", text(&refused));
    assert!(
        text(&refused).contains("pass --force"),
        "{}",
        text(&refused)
    );
    assert_eq!(
        std::fs::read_to_string(repo.workflow()).unwrap(),
        "# mine\n"
    );

    let shown = repo.init(&[]);
    assert!(shown.status.success(), "{}", text(&shown));
    assert!(String::from_utf8_lossy(&shown.stdout).contains("exists; nothing written"));
    assert_eq!(
        std::fs::read_to_string(repo.workflow()).unwrap(),
        "# mine\n"
    );

    let forced = repo.init(&["--yes", "--force"]);
    assert!(forced.status.success(), "{}", text(&forced));
    assert_eq!(std::fs::read_to_string(repo.workflow()).unwrap(), first);
    let config_after = std::fs::read_to_string(repo.repo().join("aver.toml")).unwrap();
    assert_eq!(config_after, config, "--force with --ci leaves aver.toml");
}

#[test]
fn json_carries_the_workflow_crit_init_ci_json() {
    let repo = Repo::new();
    let v = repo.json(&[]);
    let w = &v["workflow"];
    assert_eq!(w["path"], ".github/workflows/aver.yml");
    assert_eq!(w["exists"], false);
    assert_eq!(w["written"], false);
    assert_eq!(w["branch"], "trunk");
    assert_eq!(w["check"], "Aver");
    assert!(w["action"].as_str().unwrap().starts_with(&format!(
        "wonkylamp/aver/action@{} # v",
        workflow::ACTION_SHA
    )));
    assert_eq!(
        w["setup"],
        serde_json::json!(["Install Node", "Install the Node dependencies"])
    );
    assert_eq!(v["aver_toml_exists"], true);
    let content = w["content"].as_str().unwrap().to_owned();
    let v = repo.json(&["--yes"]);
    assert_eq!(v["workflow"]["written"], true);
    assert_eq!(std::fs::read_to_string(repo.workflow()).unwrap(), content);
}

#[test]
fn outside_a_repository_ci_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_aver"))
        .args(["init", "--ci"])
        .current_dir(dir.path())
        .env("AVER_HOME", dir.path().join("home"))
        .env("GIT_CEILING_DIRECTORIES", dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(text(&out).contains("Git repository"));
}
