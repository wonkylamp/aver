//! A temporary `$AVER_HOME` with the scripted runner: `aver run`,
//! `aver accept` and the other library calls go through an in-process
//! [`Api`] over [`Scripted`], and the `aver` binary these tests start (an
//! agent's `aver verify`, a test's `aver combine`) opens the same home with
//! `AVER_SCRIPTED=1`, so its checks run the same scripts and land in the
//! same run cache. [`TestHome::runs`] counts both.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aver::env::StaticResolver;
use aver::exec::{ExecConfig, Executor};
use aver::keys::{self, AgentKey, Home};
use aver::run::agent::Template;
use aver::run::api::Api;
use aver::run::batch::input::BatchTask;
use aver::run::batch::schedule::Sequence;
use aver::run::batch::{BatchBudget, BatchOutcome, BatchSpec, LinesBatch};
use aver::run::state::Budget;
use aver::services::Services;
use aver::store::Store;
use tempfile::TempDir;

use crate::git_support::git;
use crate::scripted::Scripted;
use crate::world::PINNED_IMAGE;

/// A JUnit check `unit` that runs every `test/**/*.test.json` script.
pub fn aver_toml() -> String {
    format!(
        "[env]\nimage = \"{PINNED_IMAGE}\"\n\n[[check]]\nid = \"unit\"\nrun = [\"junit-dir\", \"unit.xml\", \"test\"]\nreport = {{ format = \"junit\", path = \"/aver/out/unit.xml\" }}\n"
    )
}

pub struct TestHome {
    pub dir: TempDir,
    pub root: PathBuf,
    pub home: Home,
    pub admin: AgentKey,
    pub store: Arc<Store>,
    pub sandbox: Arc<Scripted>,
    pub services: Arc<Services>,
}

impl TestHome {
    pub fn start() -> Self {
        // The agents `aver run` starts inherit it, and the `aver` they call
        // reads it.
        std::env::set_var("AVER_SCRIPTED", "1");
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let home = Home::new(root.join("home"));
        let init = keys::init(&home).unwrap();
        let store = Arc::new(home.open_store().unwrap());
        let sandbox = Arc::new(Scripted::default());
        let executor = Executor::new(store.clone(), sandbox.clone(), ExecConfig::default());
        let services = Arc::new(Services::new(
            &home,
            store.clone(),
            Arc::new(init.server.signing_key.clone()),
            Arc::new(StaticResolver::default()),
            executor,
        ));
        Self {
            dir,
            root,
            home,
            admin: init.admin,
            store,
            sandbox,
            services,
        }
    }

    pub fn api(&self) -> Api {
        Api::new(self.services.clone())
    }

    /// The checks run so far, in this process and by the `aver` binary.
    pub fn runs(&self) -> usize {
        self.sandbox.runs() + Scripted::logged(self.home.root())
    }

    /// A Git repository at `<root>/<name>` holding `files`, committed on
    /// `main`.
    pub fn repo(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let repo = self.root.join(name);
        for (path, content) in files {
            let p = repo.join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
        git(&repo, &["init", "--quiet", "--initial-branch=main"], b"");
        // The identity `aver accept` commits as.
        git(&repo, &["config", "user.name", "Repo Owner"], b"");
        git(&repo, &["config", "user.email", "owner@example.com"], b"");
        git(&repo, &["add", "-A"], b"");
        git(&repo, &["commit", "--quiet", "-m", "base"], b"");
        repo
    }

    /// The calc-like repository the run tests use: one passing test that
    /// needs `src/math.js`.
    pub fn calc(&self) -> PathBuf {
        self.repo(
            "calc",
            &[
                ("aver.toml", &aver_toml()),
                ("src/math.js", "exports.add = (a, b) => a + b;\n"),
                ("test/math.test.json", r#"{"adds": "pass-if:src/math.js"}"#),
                ("README.md", "calc\n"),
            ],
        )
    }

    /// Writes an executable shell script under the test root.
    pub fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, format!("#!/bin/sh\nset -e\n{body}")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

pub fn aver_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_aver"))
}

impl TestHome {
    /// An agent script that picks what to do from the `TASK:` line of its
    /// brief: `cases` are `(pattern, shell)` pairs.
    pub fn case_agent(&self, name: &str, cases: &[(&str, &str)]) -> PathBuf {
        let mut body = String::from("case \"$(grep '^TASK:' .aver/BRIEF.md)\" in\n");
        for (pattern, script) in cases {
            body.push_str(&format!("*'{pattern}'*)\n{script}\n;;\n"));
        }
        body.push_str("esac\n");
        self.script(name, &body)
    }

    /// The admin key as a file in the home.
    pub fn admin_key(&self) -> AgentKey {
        AgentKey::read(&self.home.key_path(&"admin".parse().unwrap())).unwrap()
    }

    /// Runs a batch with a line-mode view; returns the outcome and the
    /// view's transcript.
    pub fn run_batch(&self, spec: BatchSpec) -> (BatchOutcome, String) {
        let stop = std::sync::atomic::AtomicBool::new(false);
        let mut view = LinesBatch::new(Vec::new());
        let out = aver::run::batch::run(
            &self.home,
            &self.api(),
            &self.admin_key(),
            &mut view,
            &stop,
            spec,
        )
        .unwrap();
        (out, String::from_utf8(view.into_inner()).unwrap())
    }
}

/// A batch of `tasks` on `repo`'s HEAD, one custom agent per task running
/// `script`, at most `parallel` at a time, with the batch defaults (no
/// limits, auto-accept, sequencing on).
pub fn batch_spec(repo: &Path, tasks: Vec<BatchTask>, script: &Path, parallel: u32) -> BatchSpec {
    BatchSpec {
        repo: repo.to_owned(),
        rev: "HEAD".into(),
        tasks,
        agents: vec![Template::Custom {
            spawn: format!("/bin/sh {}", script.display()),
            resume: None,
        }],
        budget: Budget {
            timeout_ms: 60_000,
            idle_ms: 60_000,
            max_verifies: 4,
            max_usd: None,
        },
        deps: false,
        git_view: false,
        aver_bin: aver_bin(),
        ports: 10,
        parallel,
        id: None,
        feedback: None,
        limits: BatchBudget::default(),
        auto_accept: true,
        sequence: Sequence::default(),
    }
}

/// Runs the `aver` binary in `cwd` against `home`, as a person: the
/// agent environment `aver accept --approve-review` refuses is cleared,
/// and `AVER_TEST_HUMAN` stands in for the terminal it asks for.
pub fn aver(home: &Home, cwd: &Path, args: &[&str], stdin: &[u8]) -> std::process::Output {
    use std::io::Write;
    let mut cmd = std::process::Command::new(aver_bin());
    for var in aver::run::accept::AGENT_ENV {
        cmd.env_remove(var);
    }
    let mut child = cmd
        .args(args)
        .current_dir(cwd)
        .env("AVER_HOME", home.root())
        .env("AVER_SCRIPTED", "1")
        .env("AVER_TEST_HUMAN", "1")
        .env_remove("AVER_URL")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().unwrap()
}
