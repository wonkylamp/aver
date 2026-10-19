//! Two runs on different tasks in one repository, with scripted fake
//! agents and the scripted runner: the second
//! run's agents see the first's lease and changes in the brief, the
//! PreToolUse hook refuses an edit under the lease, the PostToolUse hook
//! and the report name the overlap, and `aver combine` merges the two
//! accepted winners into one branch that passes both tasks. No Docker, no
//! LLM.

mod common;
mod git_support;
mod run_support;
mod scripted;
mod world;

use std::path::Path;
use std::sync::atomic::AtomicBool;

use aver::keys::AgentKey;
use aver::run::accept::{self, AcceptOptions};
use aver::run::agent::Template;
use aver::run::combine::{self, CombineOptions, Combined};
use aver::run::state::{Budget, RunState};
use aver::run::supervisor::{RunOutcome, RunSpec, Supervisor};
use aver::run::view::Lines;
use aver::workspace::merge::ConflictKind;
use aver_core::Verdict;
use git_support::git_str;
use run_support::*;

const README: &str = "calc\nl2\nl3\nl4\nl5\n";

fn admin(d: &TestHome) -> AgentKey {
    AgentKey::read(&d.home.key_path(&"admin".parse().unwrap())).unwrap()
}

fn spec(repo: &Path, prompt: &str, crit: (&str, &str), script: &Path) -> RunSpec {
    RunSpec {
        repo: repo.to_owned(),
        rev: "HEAD".into(),
        prompt: prompt.into(),
        title: None,
        criteria: vec![(crit.0.into(), crit.1.into())],
        protected: Vec::new(),
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
        leases: Vec::new(),
        id: None,
        batch: None,
        siblings: Vec::new(),
        stacked_on: None,
        context: None,
    }
}

fn run(d: &TestHome, spec: RunSpec) -> RunOutcome {
    let stop = AtomicBool::new(false);
    let mut view = Lines::new(Vec::new());
    Supervisor::new(&d.home, d.api(), admin(d), &mut view, &stop)
        .unwrap()
        .run(spec)
        .unwrap()
}

fn accept(d: &TestHome, run: &mut RunState) -> accept::Accepted {
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(accept::accept(
            &d.api(),
            &d.home,
            &admin(d),
            run,
            "1",
            &AcceptOptions::default(),
        ))
        .unwrap()
}

fn calc(d: &TestHome) -> std::path::PathBuf {
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

#[test]
fn a_second_run_sees_the_first_and_combine_merges_both() {
    let d = TestHome::start();
    let repo = calc(&d);
    let base = git_str(&repo, &["rev-parse", "HEAD"]);
    let first = d.script(
        "negate.sh",
        "aver lease src/neg.js --reason 'negate lives here'\n\
         mkdir -p src test\n\
         echo 'module.exports = (x) => -x;' > src/neg.js\n\
         echo '{\"[crit:neg] negates\": \"pass-if:src/neg.js\"}' > test/neg.test.json\n\
         printf 'calc: with negate\\nl2\\nl3\\nl4\\nl5\\n' > README.md\n\
         aver verify || true\n",
    );
    let mut a = run(&d, spec(&repo, "Add negate", ("neg", "negate"), &first)).state;
    assert_eq!(a.agents[0].state, aver::run::state::AgentState::Done);
    // The first agent stopped editing a while ago, so its changes have
    // settled (a notice of what the other side is still editing waits).
    rusqlite::Connection::open(d.home.db_path())
        .unwrap()
        .execute(
            "UPDATE workspaces SET updated_at = '2000-01-01T00:00:00.000Z'",
            [],
        )
        .unwrap();

    let root = d.root.clone();
    let second = d.script(
        "double.sh",
        &format!(
            "aver brief > {root}/brief.md\naver notices > {root}/notices.txt\n\
             mkdir -p src test\n\
             echo 'module.exports = (x) => 2 * x;' > src/dbl.js\n\
             echo '{{\"[crit:dbl] doubles\": \"pass-if:src/dbl.js\"}}' > test/dbl.test.json\n\
             printf 'calc\\nl2\\nl3\\nl4: and double\\nl5\\n' > README.md\n\
             aver verify > {root}/verify.txt || true\n",
            root = root.display(),
        ),
    );
    let mut b = run(&d, spec(&repo, "Add double", ("dbl", "double"), &second)).state;
    assert_eq!(b.agents[0].state, aver::run::state::AgentState::Done);
    let read = |name: &str| std::fs::read_to_string(root.join(name)).unwrap();

    let brief = read("brief.md");
    assert!(
        brief.contains("task \"Add negate\" holds an exclusive lease on"),
        "{brief}"
    );
    assert!(brief.contains("task \"Add negate\" changes"), "{brief}");
    assert_eq!(
        brief.matches("Other tasks in this repository:").count(),
        1,
        "only what other tasks do now: {brief}"
    );
    assert!(brief.contains(aver::run::report::FENCE_NOTE), "{brief}");
    let notices = read("notices.txt");
    assert!(
        notices.contains("task \"Add negate\" holds an exclusive lease on src/neg.js"),
        "{notices}"
    );
    assert!(!notices.contains("<repo-data"), "{notices}");

    let verify = read("verify.txt");
    assert!(
        verify.contains("Other tasks: 1 overlap new, 0 unchanged (`aver notices`)."),
        "{verify}"
    );

    // Accepting both: the second accept points at combine.
    let first_accept = accept(&d, &mut a);
    assert!(first_accept.combinable.is_empty());
    let second_accept = accept(&d, &mut b);
    assert_eq!(second_accept.combinable.len(), 1);
    assert!(second_accept.combinable[0].clean);

    let combined = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(combine::combine(
            &d.api(),
            &admin(&d),
            &[a.clone(), b.clone()],
            &CombineOptions::default(),
        ))
        .unwrap();
    let Combined::Exported {
        branch,
        merged,
        verdicts,
        ..
    } = combined
    else {
        panic!("expected an export, got {combined:?}");
    };
    assert_eq!(branch, "aver/combine-add-negate-and-add-double");
    assert_eq!(merged, vec!["README.md"]);
    assert!(verdicts.iter().all(|v| v.verdict == Verdict::Pass));
    assert_eq!(git_str(&repo, &["rev-parse", &format!("{branch}^")]), base);
    assert_eq!(
        git_str(&repo, &["show", &format!("{branch}:README.md")]),
        "calc: with negate\nl2\nl3\nl4: and double\nl5"
    );
    assert!(git_str(&repo, &["show", &format!("{branch}:src/dbl.js")]).contains("2 * x"));
    let message = git_str(&repo, &["log", "-1", "--format=%B", &branch]);
    assert!(
        message.starts_with("Combine: Add negate; Add double\n"),
        "{message}"
    );
    assert_eq!(message.matches("Aver-Task: task_").count(), 2, "{message}");
    assert_eq!(
        message.matches("Aver-Receipt: rcpt_").count(),
        2,
        "{message}"
    );
    assert_eq!(
        git_str(&repo, &["log", "-1", "--format=%an <%ae>", &branch]),
        "Repo Owner <owner@example.com>"
    );
}

#[test]
fn combine_reports_conflicting_lines_and_exports_nothing() {
    let d = TestHome::start();
    let repo = calc(&d);
    let one = d.script(
        "one.sh",
        "mkdir -p src test\n\
         echo 'module.exports = (x) => -x;' > src/neg.js\n\
         echo '{\"[crit:neg] negates\": \"pass-if:src/neg.js\"}' > test/neg.test.json\n\
         printf 'calc\\nl2\\nnegate here\\nl4\\nl5\\n' > README.md\n",
    );
    let two = d.script(
        "two.sh",
        "mkdir -p src test\n\
         echo 'module.exports = (x) => 2 * x;' > src/dbl.js\n\
         echo '{\"[crit:dbl] doubles\": \"pass-if:src/dbl.js\"}' > test/dbl.test.json\n\
         printf 'calc\\nl2\\ndouble here\\nl4\\nl5\\n' > README.md\n",
    );
    let mut a = run(&d, spec(&repo, "Add negate", ("neg", "negate"), &one)).state;
    let mut b = run(&d, spec(&repo, "Add double", ("dbl", "double"), &two)).state;
    accept(&d, &mut a);
    let second = accept(&d, &mut b);
    assert!(!second.combinable[0].clean);
    let out = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(combine::combine(
            &d.api(),
            &admin(&d),
            &[a.clone(), b.clone()],
            &CombineOptions::default(),
        ))
        .unwrap();
    let Combined::Conflicts { conflicts, labels } = out else {
        panic!("expected conflicts, got {out:?}");
    };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, "README.md");
    assert_eq!(conflicts[0].kind, ConflictKind::Content);
    let text = combine::conflicts_text(&conflicts, &labels);
    assert!(
        text.contains(&format!(
            "at base line 3: run {} (\"Add negate\") replaced line 3 with its line 3; run {} (\"Add double\") replaced line 3 with its line 3",
            a.id, b.id
        )),
        "{text}"
    );
    let branches = git_str(&repo, &["branch", "--list", "aver/combine-*"]);
    assert_eq!(branches, "", "nothing exported");
}

#[test]
fn accept_refuses_without_a_git_identity() {
    let d = TestHome::start();
    let repo = calc(&d);
    git_support::git(&repo, &["config", "--unset", "user.name"], b"");
    git_support::git(&repo, &["config", "--unset", "user.email"], b"");
    let good = d.script(
        "good.sh",
        "mkdir -p src test\n\
         echo 'module.exports = (x) => -x;' > src/neg.js\n\
         echo '{\"[crit:neg] negates\": \"pass-if:src/neg.js\"}' > test/neg.test.json\n",
    );
    let r = run(&d, spec(&repo, "Add negate", ("neg", "negate"), &good)).state;
    let out = std::process::Command::new(aver_bin())
        .args(["accept", "1", "--run", &r.id])
        .current_dir(&repo)
        .env("AVER_HOME", d.home.root())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("AVER_URL")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(err.contains("commits as your Git identity"), "{err}");
    assert!(err.contains("git config user.name"), "{err}");
    // Nothing was decided: the run can still be accepted once it is set.
    assert!(RunState::load(&d.home, &r.id).unwrap().decided.is_none());
}
