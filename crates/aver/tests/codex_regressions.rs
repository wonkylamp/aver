//! Codex's hands-on review of 4466fdd (October 2026) reproduced twelve
//! findings through the `aver` binary and the API. Each test here is that
//! reproduction, as Codex ran it, against the scripted runner and with
//! scratch homes; it failed on 4466fdd. The MCP findings (5, 9 and the
//! REVIEW footer of 8) went with the MCP server in Aver 0.4. Last, Codex's
//! three-task batch with `after: #1`, which 4466fdd refused to parse, with
//! scripted agents. No Docker, no LLM.

mod common;
mod git_support;
mod run_support;
mod scripted;
mod world;

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use aver::run::batch::{input, BatchState};
use aver::run::state::StackOnto;
use git_support::git_str;
use run_support::*;
use serde_json::Value;

/// Runs the `aver` binary in `cwd` with `home` as `$AVER_HOME` and `env`.
fn run_aver(home: &Path, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(aver_bin());
    cmd.args(args)
        .current_dir(cwd)
        .env("AVER_HOME", home)
        .env("AVER_SCRIPTED", "1")
        .env("AVER_TEST_HUMAN", "1")
        .env_remove("AVER_URL");
    for var in aver::run::accept::AGENT_ENV {
        cmd.env_remove(var);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A scratch place as Codex's `reproduce.py` set one up: a Git repository
/// of `files` and `links`, and a home of its own, with `$HOME` inside it
/// too.
struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
}

impl Scratch {
    fn new(files: &[(&str, &str)], links: &[(&str, &Path)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let repo = root.join("repo");
        for (rel, text) in files {
            let p = repo.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        for (rel, target) in links {
            let p = repo.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            symlink(target, p).unwrap();
        }
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q", "--initial-branch=main"]);
        git(&["add", "-f", "."]);
        git(&[
            "-c",
            "user.name=Aver Review",
            "-c",
            "user.email=aver-review@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "fixture",
        ]);
        fs::create_dir_all(root.join("user")).unwrap();
        Self {
            home: root.join("home"),
            _dir: dir,
            root,
            repo,
        }
    }

    /// A path outside the repository, the home and every workspace.
    fn outside(&self, rel: &str) -> PathBuf {
        self.root.join("outside").join(rel)
    }

    fn aver(&self, cwd: &Path, args: &[&str]) -> (i32, String, String) {
        let user = self.root.join("user");
        run_aver(&self.home, cwd, args, &[("HOME", user.to_str().unwrap())])
    }

    /// `aver workspace new --no-deps --no-git --json`: the directory.
    fn workspace(&self) -> PathBuf {
        let (code, out, err) = self.aver(
            &self.repo,
            &["workspace", "new", "--no-deps", "--no-git", "--json"],
        );
        assert_eq!(code, 0, "{out}{err}");
        let v: Value = serde_json::from_str(&out).unwrap();
        PathBuf::from(v["workspaces"][0]["dir"].as_str().unwrap())
    }
}

fn sentinel(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// Finding 1: verifying the broken head against itself as the base
/// (`--base H`) gave PASS with the failure PREEXISTING, attached to the
/// attempt, and `aver accept` exported the broken code.
#[test]
fn f01_a_receipt_with_the_head_as_its_base_is_not_accepted() {
    let d = TestHome::start();
    let repo = d.calc();
    let (code, out, err) = run_aver(
        d.home.root(),
        &repo,
        &["workspace", "new", "--no-deps", "--no-git", "--json"],
        &[],
    );
    assert_eq!(code, 0, "{out}{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    let task = v["task"].as_str().unwrap().to_owned();
    let ws = &v["workspaces"][0];
    let (dir, attempt) = (
        PathBuf::from(ws["dir"].as_str().unwrap()),
        ws["attempt"].as_str().unwrap().to_owned(),
    );
    fs::remove_file(dir.join("src/math.js")).unwrap();
    let (code, out, err) = run_aver(d.home.root(), &dir, &["verify", "--json", "--quiet"], &[]);
    assert_eq!(code, 1, "{out}{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["receipt"]["verdict"], "FAIL_OWN", "{out}");
    let head = v["receipt"]["head_snapshot"].as_str().unwrap().to_owned();
    // The broken head against itself, attached to the same attempt.
    let _ = run_aver(
        d.home.root(),
        &repo,
        &[
            "verify",
            "--task",
            &task,
            "--head",
            &head,
            "--base",
            &head,
            "--attempt",
            &attempt,
            "--json",
        ],
        &[],
    );
    let (code, out, err) = run_aver(
        d.home.root(),
        &dir,
        &[
            "accept",
            "--branch",
            "aver/wrong-base-probe",
            "--repo",
            repo.to_str().unwrap(),
        ],
        &[],
    );
    assert_ne!(code, 0, "the broken code is not accepted: {out}{err}");
    assert!(
        err.contains("FAIL_OWN, not PASS or REVIEW"),
        "refused for its verdict on the real base: {out}{err}"
    );
    assert!(
        git_str(&repo, &["branch", "--list", "aver/wrong-base-probe"]).is_empty(),
        "nothing exported: {out}{err}"
    );
}

/// Finding 2: the checkout's clock probe removed a tracked `clock` file,
/// and followed a tracked `clock` symlink to write outside.
#[test]
fn f02_a_tracked_clock_file_or_symlink_survives_workspace_creation() {
    let s = Scratch::new(
        &[
            ("clock", "TRACKED CLOCK CONTENT\n"),
            ("README.md", "fixture\n"),
        ],
        &[],
    );
    let ws = s.workspace();
    assert_eq!(
        fs::read_to_string(ws.join("clock")).ok().as_deref(),
        Some("TRACKED CLOCK CONTENT\n")
    );
    let probe = tempfile::tempdir().unwrap();
    let external = probe.path().join("clock-external.txt");
    sentinel(&external, "EXTERNAL CLOCK SENTINEL\n");
    let s = Scratch::new(&[("README.md", "fixture\n")], &[("clock", &external)]);
    let ws = s.workspace();
    assert_eq!(
        fs::read_to_string(&external).unwrap(),
        "EXTERNAL CLOCK SENTINEL\n"
    );
    assert!(
        fs::symlink_metadata(ws.join("clock")).is_ok_and(|m| m.file_type().is_symlink()),
        "the tracked symlink is in the workspace"
    );
}

/// Finding 3: a tracked `.aver` directory symlink, or a tracked
/// `.aver/workspace.toml` symlink, redirected metadata writes outside.
#[test]
fn f03_tracked_aver_symlinks_do_not_redirect_metadata() {
    let probe = tempfile::tempdir().unwrap();
    let external = probe.path().join("metadata-external");
    sentinel(
        &external.join("workspace.toml"),
        "EXTERNAL METADATA SENTINEL\n",
    );
    let s = Scratch::new(&[("README.md", "fixture\n")], &[(".aver", &external)]);
    s.workspace();
    assert_eq!(
        fs::read_to_string(external.join("workspace.toml")).unwrap(),
        "EXTERNAL METADATA SENTINEL\n"
    );
    assert!(!external.join(".gitignore").exists());

    let file = probe.path().join("metadata-file-external.txt");
    sentinel(&file, "EXTERNAL FILE SENTINEL\n");
    let s = Scratch::new(
        &[("README.md", "fixture\n")],
        &[(".aver/workspace.toml", &file)],
    );
    s.workspace();
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "EXTERNAL FILE SENTINEL\n"
    );
}

/// Finding 4: with ancestor `a` replaced by a symlink, `aver restore a/b/c`
/// overwrote the external `b/c` and `aver restore a/b/new` deleted the
/// external `b/new`.
#[test]
fn f04_restore_does_not_write_or_delete_through_a_replaced_ancestor() {
    let s = Scratch::new(&[("README.md", "fixture\n"), ("a/b/c", "BASE C\n")], &[]);
    let ws = s.workspace();
    let external = s.outside("ancestor");
    sentinel(&external.join("b/c"), "EXTERNAL KEEP C\n");
    sentinel(&external.join("b/new"), "EXTERNAL KEEP NEW\n");
    fs::remove_dir_all(ws.join("a")).unwrap();
    symlink(&external, ws.join("a")).unwrap();
    let restore = s.aver(&ws, &["restore", "a/b/c"]);
    let delete = s.aver(&ws, &["restore", "a/b/new"]);
    assert_eq!(
        fs::read_to_string(external.join("b/c")).unwrap(),
        "EXTERNAL KEEP C\n",
        "{restore:?}"
    );
    assert!(external.join("b/new").exists(), "{delete:?}");
}

/// Finding 6: verify, decide, edit, commit, export (the accept's API
/// sequence) exported a snapshot other than the receipt's. The commit that
/// accept sends names the receipt's snapshot and is refused.
#[test]
fn f06_a_commit_after_an_edit_does_not_export_another_snapshot() {
    let d = TestHome::start();
    let repo = d.calc();
    let (code, out, err) = run_aver(
        d.home.root(),
        &repo,
        &[
            "workspace",
            "new",
            "--no-deps",
            "--no-git",
            "--json",
            "--title",
            "Accept ordering probe",
        ],
        &[],
    );
    assert_eq!(code, 0, "{out}{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    let task = v["task"].as_str().unwrap().to_owned();
    let ws = &v["workspaces"][0];
    let dir = PathBuf::from(ws["dir"].as_str().unwrap());
    let (workspace, attempt) = (
        ws["workspace"].as_str().unwrap().to_owned(),
        ws["attempt"].as_str().unwrap().to_owned(),
    );
    fs::write(dir.join("README.md"), "Reviewed documentation only.\n").unwrap();
    let (code, out, err) = run_aver(d.home.root(), &dir, &["verify", "--json", "--quiet"], &[]);
    assert_eq!(code, 0, "{out}{err}");
    let good: Value = serde_json::from_str(&out).unwrap();
    let verified = good["receipt"]["head_snapshot"].clone();
    let api = d.api();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let decided = rt.block_on(api.decide(
        &d.admin.signing_key,
        &task.parse().unwrap(),
        &aver::tasks::Decision {
            winner: attempt.parse().unwrap(),
            ranking: Vec::new(),
            rationale: Some("probe".into()),
            lessons: Vec::new(),
        },
    ));
    assert!(decided.is_ok(), "{decided:?}");
    fs::write(dir.join("src/math.js"), "exports.add = (a, b) => 99;\n").unwrap();
    // The directory's own agent key owns the workspace, so it commits.
    let file: aver::workspace::WorkspaceFile =
        toml::from_str(&fs::read_to_string(dir.join(".aver/workspace.toml")).unwrap()).unwrap();
    let name = file.key.expect("the directory names its key");
    let owner = aver::keys::AgentKey::read(&d.home.key_path(&name.parse().unwrap())).unwrap();
    let message = format!(
        "Review probe\n\nAver-Receipt: {}",
        good["receipt_id"].as_str().unwrap()
    );
    let verified: aver_core::SnapshotId = serde_json::from_value(verified).unwrap();
    let committed = rt.block_on(api.commit_if(
        &owner.signing_key,
        &workspace.parse().unwrap(),
        &message,
        Some(&verified),
    ));
    let err = committed.expect_err("the moved directory is not committed");
    assert_eq!(err.code(), Some("SNAPSHOT_MOVED"), "{err}");
}

/// Finding 7: with the workspace root unreadable, sync succeeded, advanced
/// the version and recorded every file deleted.
#[test]
fn f07_an_unreadable_root_fails_the_sync() {
    let s = Scratch::new(
        &[("README.md", "BASE README\n"), ("src/a.txt", "BASE A\n")],
        &[],
    );
    let ws = s.workspace();
    fs::write(ws.join("src/a.txt"), "UNCOMMITTED A\n").unwrap();
    let ws_arg = ws.to_str().unwrap();
    let (code, out, err) = s.aver(&s.repo, &["workspace", "sync", ws_arg, "--json"]);
    assert_eq!(code, 0, "{out}{err}");
    fs::set_permissions(&ws, fs::Permissions::from_mode(0o000)).unwrap();
    let unreadable = s.aver(&s.repo, &["workspace", "sync", ws_arg, "--json"]);
    fs::set_permissions(&ws, fs::Permissions::from_mode(0o755)).unwrap();
    assert_ne!(
        unreadable.0, 0,
        "an unreadable root is an error: {unreadable:?}"
    );
    let (code, out, err) = s.aver(&s.repo, &["workspace", "sync", ws_arg, "--json"]);
    assert_eq!(code, 0, "{out}{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(
        !out.contains("\"deleted\": 2") && v.to_string().contains("\"deleted\":0"),
        "nothing was recorded deleted: {out}"
    );
    assert_eq!(
        fs::read_to_string(ws.join("src/a.txt")).unwrap(),
        "UNCOMMITTED A\n"
    );
}

/// Finding 8: a REVIEW attempt was closed for good before the human could
/// accept it (ATTEMPT_CLOSED). It stays open, and `aver accept` takes it.
#[test]
fn f08_a_review_attempt_stays_open_for_the_human() {
    let d = TestHome::start();
    let repo = d.repo(
        "review",
        &[
            ("aver.toml", &aver_toml()),
            (
                "test/parse.test.json",
                "{\n  \"keeps\": \"pass\",\n  \"parses\": \"fail\"\n}\n",
            ),
        ],
    );
    let (code, out, err) = run_aver(
        d.home.root(),
        &repo,
        &["workspace", "new", "--no-deps", "--no-git", "--json"],
        &[],
    );
    assert_eq!(code, 0, "{out}{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    let ws = &v["workspaces"][0];
    let dir = PathBuf::from(ws["dir"].as_str().unwrap());
    let attempt = ws["attempt"].as_str().unwrap().to_owned();
    fs::write(
        dir.join("test/parse.test.json"),
        "{\n  \"keeps\": \"pass\",\n  \"parses\": \"pass\"\n}\n",
    )
    .unwrap();
    let (_, out, err) = run_aver(d.home.root(), &dir, &["verify", "--json", "--quiet"], &[]);
    let reviewed: Value = serde_json::from_str(&out).unwrap_or_else(|_| panic!("{out}{err}"));
    assert_eq!(reviewed["receipt"]["verdict"], "REVIEW", "{out}");
    let id = attempt.parse().unwrap();
    let state = d
        .services
        .tasks
        .attempt_for(&d.admin.id(), &id, true)
        .unwrap();
    let state = serde_json::to_value(&state).unwrap();
    assert_eq!(state["state"], "open", "{state}");
    let accept = [
        "accept",
        "--branch",
        "aver/review-handoff-probe",
        "--repo",
        repo.to_str().unwrap(),
    ];
    let (code, out, err) = run_aver(d.home.root(), &dir, &accept, &[]);
    assert_eq!(code, 2, "a REVIEW needs the approval: {out}{err}");
    let (code, out, err) = run_aver(
        d.home.root(),
        &dir,
        &[&accept[..], &["--approve-review"]].concat(),
        &[],
    );
    assert_eq!(code, 0, "the human accepts the REVIEW: {out}{err}");
}

/// Finding 10: from `src`, `aver restore ../config.json` restored
/// `src/.config.json` and left the root `config.json` edited.
#[test]
fn f10_restore_resolves_dot_dot_by_component() {
    let s = Scratch::new(
        &[
            ("config.json", "ROOT BASE\n"),
            ("src/.config.json", "DOT BASE\n"),
        ],
        &[],
    );
    let ws = s.workspace();
    fs::write(ws.join("config.json"), "ROOT EDIT\n").unwrap();
    fs::write(ws.join("src/.config.json"), "DOT EDIT\n").unwrap();
    let restore = s.aver(&ws.join("src"), &["restore", "../config.json"]);
    assert_eq!(restore.0, 0, "{restore:?}");
    assert_eq!(
        fs::read_to_string(ws.join("config.json")).unwrap(),
        "ROOT BASE\n"
    );
    assert_eq!(
        fs::read_to_string(ws.join("src/.config.json")).unwrap(),
        "DOT EDIT\n"
    );
}

/// Finding 11: a user command added to Aver's PreToolUse hook group was
/// deleted, with the whole settings file, by `aver setup --remove`.
#[test]
fn f11_setup_remove_keeps_a_users_hook_in_avers_group() {
    let s = Scratch::new(&[("README.md", "fixture\n")], &[]);
    let (code, out, err) = s.aver(&s.repo, &["integrate", "claude", "--yes"]);
    assert_eq!(code, 0, "{out}{err}");
    let settings = s.repo.join(".claude/settings.json");
    let mut data: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    data["hooks"]["Stop"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({ "type": "command", "command": "echo USER_HOOK_MUST_SURVIVE" }));
    fs::write(&settings, serde_json::to_string_pretty(&data).unwrap()).unwrap();
    let remove = s.aver(&s.repo, &["setup", "--remove", "--yes"]);
    let left = fs::read_to_string(&settings).unwrap_or_default();
    assert!(
        left.contains("USER_HOOK_MUST_SURVIVE"),
        "{remove:?}\n{left}"
    );
    assert!(
        !left.contains("aver hook"),
        "Aver's own commands are gone: {left}"
    );
}

/// Finding 12: `aver accept 1` advertised `aver accept 1 --pr`, which then
/// failed with "already accepted" before reaching GitHub.
#[test]
fn f12_the_advertised_post_accept_pr_command_opens_the_pr() {
    let d = TestHome::start();
    let repo = d.calc();
    let agent = d.script(
        "doc-agent.sh",
        "printf \"Documented probe.\\n\" > \"$1/README.md\"\n",
    );
    let bin = d.root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let log = d.root.join("gh.log");
    let gh = bin.join("gh");
    fs::write(
        &gh,
        format!(
            "#!/bin/sh\necho \"$@\" >> {}\necho https://github.com/o/r/pull/7\n",
            log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let origin = d.root.join("origin.git");
    git_support::git(
        &d.root,
        &["init", "--quiet", "--bare", origin.to_str().unwrap()],
        b"",
    );
    git_support::git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
        b"",
    );
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let env = [("PATH", path.as_str())];
    let agent_cmd = format!("/bin/sh {} {{dir}}", agent.display());
    let repo_arg = repo.to_str().unwrap();
    let steps: [&[&str]; 3] = [
        &[
            "run",
            "Add probe documentation",
            "--repo",
            repo_arg,
            "--agents",
            "1",
            "--agent-cmd",
            &agent_cmd,
            "--no-deps",
            "--no-tui",
            "--json",
            "--timeout",
            "1m",
        ],
        &["accept", "1", "--branch", "aver/post-accept-probe"],
        &["accept", "1", "--pr"],
    ];
    let mut outs = Vec::new();
    for args in steps {
        outs.push(run_aver(d.home.root(), &repo, args, &env));
    }
    assert_eq!(outs[1].0, 0, "{outs:?}");
    assert!(outs[1].1.contains("aver accept 1 --pr"), "{outs:?}");
    assert_eq!(outs[2].0, 0, "the advertised command works: {outs:?}");
    let args = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        args.contains("pr create --head aver/post-accept-probe"),
        "{args}"
    );
}

/// Codex's `tasks-with-after.txt`, which 4466fdd refused (`line 4 of tasks
/// file: expected '- <id>: <text>' or 'protected: <glob>', found "after:
/// #1"`).
const TASKS_WITH_AFTER: &str = "\
Fix comparisons of two null-prototype dictionaries in src/index.jst so equal contents return true, differing contents return false, and neither throws. Preserve existing behavior for normal objects. Add regression tests only in a new spec/null-prototype.spec.js with the criterion tag. Do not alter existing tests, package.json, aver.toml or lockfiles. Run aver check then aver verify. Stop after reporting the result; do not accept or commit.
  - null-prototype: equal and unequal null-prototype dictionaries compare without throwing in standard, ES6 and React variants
Fix ES6 typed-array equality in src/index.jst so matching NaN elements in Float32Array and Float64Array compare equal, while differing numeric elements remain unequal. Add tests only in a new spec/typed-nan.spec.js with the criterion tag. Do not alter existing tests or harness. Run aver check then aver verify. Stop after reporting the result; do not accept or commit.
  after: #1
  - typed-nan: floating typed arrays with matching NaNs compare equal and differing values compare unequal
Add ES6 ArrayBuffer comparison by byte length and byte contents in src/index.jst. Cover both ES6 and ES6 React variants. Add tests only in a new spec/buffer-batch.spec.js with the criterion tag. Do not alter existing tests or harness. Run aver check then aver verify. Stop after reporting the result; do not accept or commit.
  - bytes: ArrayBuffers with equal bytes compare equal and differing lengths or bytes compare unequal
";

#[test]
fn codex_tasks_with_after_parse() {
    let tasks = input::parse_tasks(TASKS_WITH_AFTER).expect("after: #1 parses");
    assert_eq!(tasks.len(), 3);
    assert_eq!(
        tasks.iter().map(|t| t.after.clone()).collect::<Vec<_>>(),
        [vec![], vec![1], vec![]]
    );
    let ids: Vec<&str> = tasks.iter().map(|t| t.criteria[0].0.as_str()).collect();
    assert_eq!(ids, ["null-prototype", "typed-nan", "bytes"]);
}

/// The same batch with scripted agents, three at a time: #1 and #3 start
/// at once, #2 waits for #1 and starts on its accepted work (its agent
/// fails without #1's file), and all three pass.
#[test]
fn codex_batch_with_after_schedules_the_dependent_on_its_prerequisite() {
    let d = TestHome::start();
    let repo = d.calc();
    let tasks = input::parse_tasks(TASKS_WITH_AFTER).unwrap();
    let script = d.case_agent(
        "agent.sh",
        &[
            (
                "null-prototype dictionaries",
                "mkdir -p src test\necho nullproto > src/nullproto.js\n\
                 echo '{\"[crit:null-prototype] compares\": \"pass-if:src/nullproto.js\"}' > test/nullproto.test.json\n",
            ),
            (
                "typed-array equality",
                "test -f src/nullproto.js || exit 3\nmkdir -p src test\necho nan > src/nan.js\n\
                 echo '{\"[crit:typed-nan] nans\": \"pass-if:src/nan.js\"}' > test/nan.test.json\n",
            ),
            (
                "ArrayBuffer comparison",
                "mkdir -p src test\necho bytes > src/bytes.js\n\
                 echo '{\"[crit:bytes] bytes\": \"pass-if:src/bytes.js\"}' > test/bytes.test.json\n",
            ),
        ],
    );
    let (out, transcript) = d.run_batch(batch_spec(&repo, tasks, &script, 3));
    assert!(out.all_ready(), "{transcript}");
    let events: Vec<&str> = transcript
        .lines()
        .filter(|l| l.contains(" started as run ") || l.contains(" finished: "))
        .collect();
    let at = |needle: &str| {
        events
            .iter()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("{needle}: {transcript}"))
    };
    assert!(
        at("#3 started") < at("#1 finished"),
        "#3 does not wait: {transcript}"
    );
    assert!(
        at("#1 finished") < at("#2 started"),
        "#2 waits for #1: {transcript}"
    );
    assert!(
        events[at("#2 started")].ends_with("on #1's accepted work"),
        "{transcript}"
    );
    let b = BatchState::load(&d.home, &out.state.id).unwrap();
    let (one, two, three) = (
        b.run(&d.home, 1).unwrap(),
        b.run(&d.home, 2).unwrap(),
        b.run(&d.home, 3).unwrap(),
    );
    assert!(
        matches!(&two.stacked_on.as_ref().unwrap().onto, StackOnto::Run { run, number: Some(1) } if *run == one.id)
    );
    assert_eq!(two.base_commit, one.decided.unwrap().commit.unwrap());
    assert!(three.stacked_on.is_none());
    assert_eq!(three.base_commit, b.base_commit);
}
