//! The review's findings where Aver told an agent something false (M1, M3,
//! L4, L6), through the `aver` binary inside `aver workspace new`
//! directories, with the scripted runner.
//! No Docker, no Node.

mod common;
mod git_support;
mod run_support;
mod scripted;
mod world;

use std::path::{Path, PathBuf};
use std::process::Output;

use aver::keys::AgentKey;
use run_support::*;
use serde_json::Value;

fn text(out: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `aver workspace new` on `repo` with `args`; the directory it printed.
fn new_dir(d: &TestHome, repo: &Path, args: &[&str]) -> PathBuf {
    let mut all = vec!["workspace", "new", "--no-deps"];
    all.extend_from_slice(args);
    all.push(repo.to_str().unwrap());
    let out = aver(&d.home, repo, &all, b"");
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "{stdout}{stderr}");
    PathBuf::from(stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap())
}

/// The value of `name = "…"` in the directory's workspace file.
fn field(dir: &Path, name: &str) -> String {
    let toml = std::fs::read_to_string(dir.join(".aver/workspace.toml")).unwrap();
    let prefix = format!("{name} = \"");
    toml.lines()
        .find_map(|l| l.strip_prefix(&prefix)?.strip_suffix('"'))
        .unwrap_or_else(|| panic!("no {name}: {toml}"))
        .to_owned()
}

fn verify_json(d: &TestHome, dir: &Path) -> Value {
    let out = aver(&d.home, dir, &["verify", "--json", "--quiet"], b"");
    let (stdout, stderr) = text(&out);
    assert!(matches!(out.status.code(), Some(0 | 1)), "{stdout}{stderr}");
    // The JSON comes first; the report follows it only without --json.
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{e}: {stdout}"))
}

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// M3: the `task` line of `.aver/workspace.toml` is the agent's to edit.
/// Pointing it at a task without criteria used to verify the directory
/// against that task, `DONE: PASS. No criteria`; Aver now judges a
/// workspace only against its attempt's task and refuses the rest.
#[test]
fn crit_agent_truth_task_line_an_edited_task_line_is_refused() {
    let d = TestHome::start();
    let repo = d.calc();
    let dir = new_dir(&d, &repo, &["-c", "neg: negate returns -x"]);
    let other = new_dir(&d, &repo, &[]);
    let own = field(&dir, "task");
    let theirs = field(&other, "task");
    assert_ne!(own, theirs);
    write(&dir, "src/neg.js", "exports.negate = (x) => x;\n");
    let toml_path = dir.join(".aver/workspace.toml");
    let toml = std::fs::read_to_string(&toml_path).unwrap();
    std::fs::write(&toml_path, toml.replace(&own, &theirs)).unwrap();

    for args in [&["verify"][..], &["verify", "--task", &theirs][..]] {
        let out = aver(&d.home, &dir, args, b"");
        let (stdout, stderr) = text(&out);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {stdout}{stderr}");
        assert!(!stdout.contains("PASS"), "{stdout}");
        assert!(
            stderr.contains(&format!("belongs to task {own}")),
            "{stderr}"
        );
        assert!(stderr.contains("put back its task line"), "{stderr}");
    }
    // Accept verifies first, against the same refusal.
    let out = aver(&d.home, &dir, &["accept"], b"");
    assert_eq!(out.status.code(), Some(2), "{:?}", text(&out));

    // Restored, the directory verifies against its own task again.
    std::fs::write(&toml_path, &toml).unwrap();
    let json = verify_json(&d, &dir);
    assert_eq!(json["receipt"]["task_id"], own.as_str(), "{json}");
    assert_eq!(json["receipt"]["verdict"], "UNPROVEN");
}

/// M3: a `workspace new` directory's key is its own agent key, not the
/// admin's: it cannot decide its task, and every command an agent runs
/// inside the directory still works with it, accept included (which takes
/// the admin key from the home itself).
#[test]
fn crit_agent_truth_agent_key_a_directory_key_is_not_the_admins() {
    let d = TestHome::start();
    let repo = d.calc();
    let dir = new_dir(&d, &repo, &[]);
    let name = field(&dir, "key");
    assert!(name.starts_with("ws-"), "{name}");
    let key = AgentKey::read(&d.home.key_path(&name.parse().unwrap())).unwrap();
    let api = d.api();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let refused = rt.block_on(api.decide(
        &key.signing_key,
        &field(&dir, "task").parse().unwrap(),
        &aver::tasks::Decision {
            winner: field(&dir, "attempt").parse().unwrap(),
            ranking: Vec::new(),
            rationale: None,
            lessons: Vec::new(),
        },
    ));
    assert!(refused.is_err(), "deciding the task: {refused:?}");

    // What an agent does in the directory works with its own key.
    write(&dir, "src/math.js", "exports.add = (a, b) => b + a;\n");
    for args in [
        &["diff", "--stat"][..],
        &["notices"][..],
        &["lease", "src/math.js", "--reason", "rewriting add"][..],
        &["lease"][..],
        &["release"][..],
        &["restore", "README.md"][..],
    ] {
        let out = aver(&d.home, &dir, args, b"");
        assert!(out.status.success(), "{args:?}: {:?}", text(&out));
    }
    let json = verify_json(&d, &dir);
    assert_eq!(json["receipt"]["verdict"], "PASS", "{json}");
    assert_eq!(json["receipt"]["agent_id"], key.id().to_string());
    let out = aver(&d.home, &dir, &["accept", "--keep"], b"");
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(stdout.contains("Accepted"), "{stdout}");
}

/// M1: `negate(x)` returns `x`, and the only test checks that `negate` is
/// a function. It fails on the original code (no `negate`) and passes with
/// the change, which used to prove the criterion: now it is a weak proof,
/// the criterion is UNPROVEN, and the report says why and what a real test
/// needs.
#[test]
fn crit_agent_truth_weak_exists_an_existence_test_does_not_prove() {
    let d = TestHome::start();
    let repo = d.calc();
    let dir = new_dir(&d, &repo, &["-c", "neg: negate returns -x"]);
    write(&dir, "src/neg.js", "exports.negate = (x) => x;\n");
    write(
        &dir,
        "test/neg.test.json",
        "{\n  \"[crit:neg] negate exists\": \"name-if:src/neg.js\"\n}\n",
    );
    let json = verify_json(&d, &dir);
    let body = &json["receipt"];
    assert_eq!(body["verdict"], "UNPROVEN", "{json}");
    assert_eq!(body["unproven"], serde_json::json!(["neg"]));
    let diag = &json["diagnostics"];
    assert_eq!(diag["weak_proofs"][0]["criterion"], "neg", "{diag}");
    assert_eq!(diag["weak_proofs"][0]["reason"], "missing_name", "{diag}");
    assert_eq!(diag["unproven"][0]["reason"], "weak_proof", "{diag}");
    assert!(
        diag["proof_sources"][0]["source"]
            .as_str()
            .unwrap()
            .contains("negate exists"),
        "{diag}"
    );

    let out = aver(&d.home, &dir, &["verify"], b"");
    let (stdout, stderr) = text(&out);
    assert_eq!(out.status.code(), Some(1), "{stdout}{stderr}");
    assert!(!stdout.contains("DONE: PASS"), "{stdout}");
    assert!(stdout.contains("neg  NOT PROVEN (weak proof)"), "{stdout}");
    assert!(
        stdout.contains("only because a name they use is missing"),
        "{stdout}"
    );
    assert!(
        stdout.contains("fails on the original code on an assertion"),
        "{stdout}"
    );
}

/// M1: a behavioural test, failing on the original code on an assertion,
/// still proves; the proving test's source is in the diagnostics and in
/// `aver accept`'s output, fenced.
#[test]
fn crit_agent_truth_strong_ok_a_behavioural_test_still_proves() {
    let d = TestHome::start();
    let repo = d.calc();
    let dir = new_dir(&d, &repo, &["-c", "neg: negate returns -x"]);
    write(
        &dir,
        "src/math.js",
        "exports.add = (a, b) => a + b;\nexports.negate = (x) => -x;\n",
    );
    write(
        &dir,
        "test/neg.test.json",
        "{\n  \"[crit:neg] negate(2) is -2\": \"contains:src/math.js:(x) => -x\"\n}\n",
    );
    let json = verify_json(&d, &dir);
    assert_eq!(json["receipt"]["verdict"], "PASS", "{json}");
    let diag = &json["diagnostics"];
    assert!(diag.get("weak_proofs").is_none(), "{diag}");
    assert_eq!(diag["proof_sources"][0]["path"], "test/neg.test.json");
    assert_eq!(diag["proof_sources"][0]["line"], 2);

    let out = aver(&d.home, &dir, &["accept", "--keep"], b"");
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "{stdout}{stderr}");
    let at = stdout
        .find("The tests proving the criteria")
        .expect(&stdout);
    let rest = &stdout[at..];
    assert!(rest.contains("```"), "{rest}");
    assert!(rest.contains("negate(2) is -2"), "{rest}");
}

/// L4: a NEW failure of a test the head did not change says nothing about
/// "your version" of it, and no line names the original code twice.
#[test]
fn crit_agent_truth_wording_an_unchanged_test_is_not_yours() {
    let d = TestHome::start();
    let repo = d.calc();
    let dir = new_dir(&d, &repo, &[]);
    std::fs::remove_file(dir.join("src/math.js")).unwrap();
    write(&dir, "src/other.js", "exports.x = 1;\n");
    let out = aver(&d.home, &dir, &["verify"], b"");
    let (stdout, stderr) = text(&out);
    assert_eq!(out.status.code(), Some(1), "{stdout}{stderr}");
    assert!(stdout.contains("New failures"), "{stdout}");
    assert!(!stdout.contains("your version"), "{stdout}");
    assert!(!stdout.contains("original the original"), "{stdout}");
}

/// L6: a solo accept's commit is titled with the task's first criterion
/// and lands on a branch named after it with a short id, not `Work on calc
/// at <commit>` on `aver/ws-<id>`. An existing branch is still refused.
#[test]
fn crit_agent_truth_names_commit_and_branch_come_from_the_criterion() {
    let d = TestHome::start();
    let repo = d.calc();
    let dir = new_dir(&d, &repo, &["-c", "neg: negate returns -x"]);
    write(
        &dir,
        "src/math.js",
        "exports.add = (a, b) => a + b;\nexports.negate = (x) => -x;\n",
    );
    let ws = field(&dir, "workspace");
    let short = ws[ws.len() - 4..].to_ascii_lowercase();
    let branch = format!("aver/negate-returns-x-{short}");
    git_support::git(&repo, &["branch", &branch], b"");
    let out = aver(&d.home, &dir, &["accept", "--force", "--keep"], b"");
    let (stdout, stderr) = text(&out);
    assert_eq!(out.status.code(), Some(2), "{stdout}{stderr}");
    assert!(stderr.contains("already exists"), "{stderr}");
    git_support::git(&repo, &["branch", "-D", &branch], b"");

    let out = aver(&d.home, &dir, &["accept", "--force", "--keep"], b"");
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(stdout.contains(&format!("as branch {branch} ")), "{stdout}");
    let subject = git_support::git_str(&repo, &["log", "-1", "--format=%s", &branch]);
    assert_eq!(subject.trim(), "negate returns -x");
}
