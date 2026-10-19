//! The orchestrator's view from the repository root (§2.11): `aver notices
//! --repo`, `aver lease --repo` and `aver release --repo` outside any
//! workspace directory, the task graph in `aver status --json`, and the
//! message in a finished run's directory. Scripted fake agents and the
//! scripted runner. No Docker, no LLM.

mod common;
mod git_support;
mod run_support;
mod scripted;
mod world;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use aver::keys::AgentKey;
use aver::run::accept::{self, AcceptOptions};
use aver::run::agent::Template;
use aver::run::batch::input;
use aver::run::state::Budget;
use aver::run::supervisor::{RunSpec, Supervisor};
use aver::run::view::Lines;
use run_support::*;

const README: &str = "calc\nl2\nl3\nl4\nl5\n";

fn admin(d: &TestHome) -> AgentKey {
    AgentKey::read(&d.home.key_path(&"admin".parse().unwrap())).unwrap()
}

fn budget() -> Budget {
    Budget {
        timeout_ms: 60_000,
        idle_ms: 60_000,
        max_verifies: 4,
        max_usd: None,
    }
}

fn calc(d: &TestHome) -> PathBuf {
    d.repo(
        "calc",
        &[
            ("aver.toml", &aver_toml()),
            ("src/math.js", "exports.add = (a, b) => a + b;\n"),
            ("test/math.test.json", r#"{"adds": "pass-if:src/math.js"}"#),
            ("README.md", README),
        ],
    )
}

fn cli(d: &TestHome, cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = aver(&d.home, cwd, args, b"");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// Two tasks of a batch both rewrite README.md's first line; the first
/// leases it. From the repository root the orchestrator sees both tasks,
/// the overlap and the lease, frees the lease, and finds the overlap edge
/// in the batch's graph.
#[test]
fn the_orchestrator_sees_the_repository_from_its_root() {
    let d = TestHome::start();
    let repo = calc(&d);
    let tasks = input::parse_tasks(
        "Add negate\n  - neg: negate(x) returns -x\n\
         Add double\n  - dbl: double(x) returns 2x\n",
    )
    .unwrap();
    let script = d.script(
        "agent.sh",
        "case \"$(grep '^TASK:' .aver/BRIEF.md)\" in\n\
         *'Add negate'*)\n\
         aver lease README.md --reason 'readme belongs to negate'\n\
         mkdir -p src test\n\
         echo 'module.exports = (x) => -x;' > src/neg.js\n\
         echo '{\"[crit:neg] negates\": \"pass-if:src/neg.js\"}' > test/neg.test.json\n\
         printf 'calc with negate\\nl2\\nl3\\nl4\\nl5\\n' > README.md\n\
         aver verify || true\n;;\n\
         *'Add double'*)\n\
         mkdir -p src test\n\
         echo 'module.exports = (x) => 2 * x;' > src/dbl.js\n\
         echo '{\"[crit:dbl] doubles\": \"pass-if:src/dbl.js\"}' > test/dbl.test.json\n\
         printf 'calc with double\\nl2\\nl3\\nl4\\nl5\\n' > README.md\n\
         aver verify || true\n;;\n\
         esac\n",
    );
    let spec = batch_spec(&repo, tasks, &script, 1);
    d.run_batch(spec);

    // Without --repo, outside a workspace, the error points at it.
    let (code, _, err) = cli(&d, &repo, &["notices"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("aver notices --repo"), "{err}");

    let (code, out, err) = cli(&d, &repo, &["notices", "--repo"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("task \"Add negate\" (1 workspace) changes"),
        "{out}"
    );
    assert!(
        out.contains("OVERLAP README.md: tasks \"Add negate\" and \"Add double\" change the same lines; merging both CONFLICTS")
            || out.contains("OVERLAP README.md: tasks \"Add double\" and \"Add negate\" change the same lines; merging both CONFLICTS"),
        "{out}"
    );
    assert!(out.contains("readme belongs to negate"), "{out}");
    assert!(out.contains("CONFLICTS: "), "{out}");

    // DIR names the repository from anywhere.
    let (code, out, err) = cli(
        &d,
        &d.root,
        &["notices", "--repo", repo.to_str().unwrap(), "--json"],
    );
    assert_eq!(code, 0, "{err}");
    let view: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(view["tasks"].as_array().unwrap().len(), 2, "{view}");
    assert_eq!(view["edges"].as_array().unwrap().len(), 1, "{view}");
    assert_eq!(view["edges"][0]["worst"], "lines");

    let (code, out, err) = cli(&d, &repo, &["lease", "--repo"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("exclusive README.md (task \"Add negate\""),
        "{out}"
    );
    let id = out.split_whitespace().next().unwrap().to_owned();
    assert!(id.starts_with("lease_"), "{out}");
    let (code, out, err) = cli(&d, &repo, &["lease", "--repo", "--json"]);
    assert_eq!(code, 0, "{err}");
    let leases: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(leases["leases"][0]["id"], id.as_str());

    let (code, out, err) = cli(&d, &repo, &["release", "--repo", &id]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains(&format!("released {id}")), "{out}");
    let (_, out, _) = cli(&d, &repo, &["lease", "--repo"]);
    assert!(out.starts_with("No live leases in "), "{out}");

    let (code, out, err) = cli(&d, &repo, &["status", "--json"]);
    assert_eq!(code, 0, "{err}");
    let status: serde_json::Value = serde_json::from_str(&out).unwrap();
    let graph = &status["graph"];
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 2, "{graph}");
    assert_eq!(graph["nodes"][0]["number"], 1);
    assert_eq!(graph["nodes"][0]["base"], "root");
    let edges = graph["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1, "{graph}");
    assert_eq!(edges[0]["kind"], "overlap");
    assert_eq!(
        (edges[0]["a"].clone(), edges[0]["b"].clone()),
        (1.into(), 2.into())
    );
    assert_eq!(edges[0]["paths"], serde_json::json!(["README.md"]));
    assert_eq!(edges[0]["overlap"], "lines");
    assert_eq!(edges[0]["merge"], "conflict");
}

/// In the directory of a run accepted with --keep, whose keys are revoked,
/// a command says the run was accepted and names `aver status <run>`
/// instead of a missing key file.
#[test]
fn a_finished_runs_directory_says_what_happened() {
    let d = TestHome::start();
    let repo = calc(&d);
    let script = d.script(
        "good.sh",
        "mkdir -p src test\n\
         echo 'module.exports = (x) => -x;' > src/neg.js\n\
         echo '{\"[crit:neg] negates\": \"pass-if:src/neg.js\"}' > test/neg.test.json\n",
    );
    let spec = RunSpec {
        repo: repo.clone(),
        rev: "HEAD".into(),
        prompt: "Add negate".into(),
        title: None,
        criteria: vec![("neg".into(), "negate(x) returns -x".into())],
        protected: Vec::new(),
        agents: vec![Template::Custom {
            spawn: format!("/bin/sh {}", script.display()),
            resume: None,
        }],
        budget: budget(),
        deps: false,
        git_view: false,
        aver_bin: aver_bin(),
        ports: 10,
        leases: Vec::new(),
        id: None,
        batch: None,
        siblings: Vec::new(),
        stacked_on: None,
        context: None,
    };
    let stop = AtomicBool::new(false);
    let mut view = Lines::new(Vec::new());
    let mut state = Supervisor::new(&d.home, d.api(), admin(&d), &mut view, &stop)
        .unwrap()
        .run(spec)
        .unwrap()
        .state;
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(accept::accept(
            &d.api(),
            &d.home,
            &admin(&d),
            &mut state,
            "1",
            &AcceptOptions {
                keep: true,
                ..Default::default()
            },
        ))
        .unwrap();
    let dir = state.agents[0].dir.clone();
    for args in [&["verify"][..], &["notices"], &["lease"]] {
        let (code, _, err) = cli(&d, &dir, args);
        assert_ne!(code, 0, "{args:?}");
        assert!(
            err.contains(&format!(
                "this directory is attempt 1 of aver run {}, which was accepted (attempt 1 as branch aver/add-negate-{})",
                state.id,
                state.id[state.id.len() - 4..].to_lowercase()
            )),
            "{args:?}: {err}"
        );
        assert!(
            err.contains(&format!("`aver status {}`", state.id)),
            "{err}"
        );
        assert!(!err.contains("aver init"), "{err}");
    }
}
