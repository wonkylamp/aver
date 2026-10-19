//! The Git view of workspace directories as standard tools and Claude
//! Code see it (a linked worktree on the branch `aver/<label>`), the
//! `WorktreeRemove` hook keeping unfetched commits, and verify reusing the
//! base's runs when nothing changed. The scripted runner; no Docker, no
//! Claude.

mod common;
mod git_support;
mod run_support;
mod scripted;
mod world;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use run_support::*;

/// `git` in `cwd` with the developer's configuration kept out and `envs`
/// set.
fn git_env(cwd: &Path, args: &[&str], envs: &[(&str, &Path)]) -> Output {
    let mut cmd = Command::new("git");
    git_support::hermetic(&mut cmd)
        .env_remove("GIT_COMMON_DIR")
        .args(args)
        .current_dir(cwd);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

/// Runs `git` and returns its stdout, failing the test unless it exits 0.
fn git_ok(cwd: &Path, args: &[&str]) -> String {
    let out = git_env(cwd, args, &[]);
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// A Claude Code worktree made by the `WorktreeCreate` hook, as
/// `claude --worktree <name>` makes it.
fn worktree(d: &TestHome, repo: &Path, name: &str) -> PathBuf {
    let input = format!(
        r#"{{"session_id":"s","cwd":"{}","hook_event_name":"WorktreeCreate","name":"{name}"}}"#,
        repo.display()
    );
    let out = aver(
        &d.home,
        repo,
        &["hook", "worktree-create"],
        input.as_bytes(),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    PathBuf::from(stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap())
}

fn remove(d: &TestHome, repo: &Path, dir: &Path) -> Output {
    let input = format!(
        r#"{{"session_id":"s","hook_event_name":"WorktreeRemove","worktree_path":"{}"}}"#,
        dir.display()
    );
    aver(
        &d.home,
        repo,
        &["hook", "worktree-remove"],
        input.as_bytes(),
    )
}

/// The view's own repository, which a coordinator fetches from.
fn view_of(dir: &Path) -> PathBuf {
    let gitdir = std::fs::read_to_string(dir.join(".git")).unwrap();
    let linked = PathBuf::from(gitdir.trim().strip_prefix("gitdir: ").unwrap());
    linked.parent().unwrap().parent().unwrap().to_owned()
}

#[test]
fn standard_git_commands_and_claude_codes_checks_read_the_view() {
    let d = TestHome::start();
    let repo = d.calc();
    let base = git_ok(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    let dir = worktree(&d, &repo, "try1");

    assert_eq!(git_ok(&dir, &["status", "--porcelain"]), "");
    assert_eq!(
        git_ok(
            &dir,
            &["rev-parse", "--is-inside-work-tree", "--abbrev-ref", "HEAD"]
        ),
        "true\naver/wt-try1\n",
        "a named branch, not a detached HEAD"
    );
    assert_eq!(
        git_ok(&dir.join("src"), &["rev-parse", "--show-toplevel"]).trim(),
        dir.to_str().unwrap()
    );
    let elsewhere = d.root.clone();
    let porcelain = git_env(
        &elsewhere,
        &["-C", dir.to_str().unwrap(), "status", "--porcelain"],
        &[],
    );
    assert!(porcelain.status.success() && porcelain.stdout.is_empty());
    let list = git_ok(&dir, &["worktree", "list", "--porcelain"]);
    assert!(
        list.contains(&format!(
            "worktree {}\nHEAD {base}\nbranch refs/heads/aver/wt-try1\n",
            dir.display()
        )),
        "{list}"
    );
    assert_eq!(
        git_ok(&dir, &["rev-list", "--count", &format!("{base}..HEAD")]),
        "0\n"
    );

    // Claude Code resolves the layout itself before it trusts Git: the
    // `.git` file names a directory under `worktrees/` whose `gitdir` file
    // points back at this `.git` and whose `commondir` leads to the parent
    // of `worktrees/`. It then pins GIT_DIR, GIT_WORK_TREE and
    // GIT_COMMON_DIR and lists the worktree's config before it counts
    // uncommitted files.
    let linked = PathBuf::from(
        std::fs::read_to_string(dir.join(".git"))
            .unwrap()
            .trim()
            .strip_prefix("gitdir: ")
            .unwrap(),
    );
    assert_eq!(
        linked.parent().unwrap().file_name().unwrap(),
        "worktrees",
        "{}",
        linked.display()
    );
    assert_eq!(
        std::fs::read_to_string(linked.join("gitdir"))
            .unwrap()
            .trim(),
        dir.join(".git").to_str().unwrap()
    );
    let common = std::fs::canonicalize(
        linked.join(
            std::fs::read_to_string(linked.join("commondir"))
                .unwrap()
                .trim(),
        ),
    )
    .unwrap();
    assert_eq!(common, view_of(&dir));
    let pinned = [
        ("GIT_DIR", linked.as_path()),
        ("GIT_WORK_TREE", dir.as_path()),
        ("GIT_COMMON_DIR", common.as_path()),
    ];
    let local = git_env(
        &dir,
        &["config", "--local", "--includes", "-z", "--list"],
        &pinned,
    );
    assert!(
        local.status.success(),
        "{}",
        String::from_utf8_lossy(&local.stderr)
    );
    let listed = String::from_utf8_lossy(&local.stdout);
    assert!(
        !listed.contains("includeif") && !listed.contains("core.worktree"),
        "{listed}"
    );
    // The answer Claude Code accepts for a linked worktree without
    // per-worktree config.
    let wt = git_env(
        &dir,
        &["config", "--worktree", "--includes", "-z", "--list"],
        &pinned,
    );
    assert!(
        wt.status.success()
            || String::from_utf8_lossy(&wt.stderr)
                .contains("--worktree cannot be used with multiple working trees"),
        "{}",
        String::from_utf8_lossy(&wt.stderr)
    );
    std::fs::write(dir.join("src/neg.js"), "module.exports = (x) => -x;\n").unwrap();
    let status = git_env(&dir, &["status", "--porcelain", "-z"], &pinned);
    assert!(status.status.success());
    assert_eq!(String::from_utf8_lossy(&status.stdout), "?? src/neg.js\0");
    let count = git_env(
        &dir,
        &["rev-list", "--count", &format!("{base}..HEAD")],
        &pinned,
    );
    assert!(count.status.success());
    assert_eq!(String::from_utf8_lossy(&count.stdout), "0\n");
}

#[test]
fn commits_land_on_the_branch_and_the_repository_fetches_them() {
    let d = TestHome::start();
    let repo = d.calc();
    let dir = worktree(&d, &repo, "fetch me");
    std::fs::write(dir.join("src/neg.js"), "module.exports = (x) => -x;\n").unwrap();
    git_ok(&dir, &["add", "src/neg.js"]);
    git_ok(&dir, &["commit", "--quiet", "-m", "negate"]);
    assert!(
        git_ok(&dir, &["status", "--branch", "--porcelain"]).starts_with("## aver/wt-fetch-me\n")
    );

    let view = view_of(&dir);
    git_ok(
        &repo,
        &[
            "fetch",
            "--quiet",
            view.to_str().unwrap(),
            "aver/wt-fetch-me:agent-fetch-me",
        ],
    );
    assert_eq!(
        git_ok(
            &repo,
            &["log", "-1", "--format=%s %an <%ae>", "agent-fetch-me"]
        ),
        "negate Repo Owner <owner@example.com>\n"
    );
    assert_eq!(
        git_ok(&repo, &["rev-parse", "agent-fetch-me^"]),
        git_ok(&repo, &["rev-parse", "main"])
    );
}

/// The view's `origin` is the user's repository (B11): an agent's
/// `git push origin <branch>` inside the directory reaches it, and `git
/// fetch origin` reads it.
#[test]
fn git_push_origin_in_the_workspace_reaches_the_users_repository() {
    let d = TestHome::start();
    let repo = d.calc();
    let dir = worktree(&d, &repo, "push me");
    std::fs::write(dir.join("src/neg.js"), "module.exports = (x) => -x;\n").unwrap();
    git_ok(&dir, &["add", "src/neg.js"]);
    git_ok(&dir, &["commit", "--quiet", "-m", "negate"]);
    let head = git_ok(&dir, &["rev-parse", "HEAD"]);

    git_ok(&dir, &["push", "--quiet", "origin", "aver/wt-push-me"]);
    assert_eq!(git_ok(&repo, &["rev-parse", "aver/wt-push-me"]), head);
    assert_eq!(
        git_ok(&repo, &["show", "aver/wt-push-me:src/neg.js"]),
        "module.exports = (x) => -x;\n"
    );

    git_ok(&dir, &["fetch", "--quiet", "origin"]);
    assert_eq!(
        git_ok(&dir, &["rev-parse", "origin/main"]),
        git_ok(&repo, &["rev-parse", "main"])
    );
}

#[test]
fn worktree_remove_keeps_unfetched_commits_and_says_how_to_fetch_them() {
    let d = TestHome::start();
    let repo = d.calc();

    // Uncommitted edits alone: Claude Code counted them and asked already.
    let edited = worktree(&d, &repo, "edited");
    std::fs::write(edited.join("README.md"), "changed\n").unwrap();
    let out = remove(&d, &repo, &edited);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!edited.exists());

    let dir = worktree(&d, &repo, "kept");
    std::fs::write(dir.join("src/neg.js"), "module.exports = (x) => -x;\n").unwrap();
    git_ok(&dir, &["add", "src/neg.js"]);
    git_ok(&dir, &["commit", "--quiet", "-m", "negate"]);
    let refused = remove(&d, &repo, &dir);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(2), "{stderr}");
    assert!(dir.join("src/neg.js").is_file(), "the directory is kept");
    assert!(
        stderr.contains("aver/wt-kept at ") && stderr.contains(" negate"),
        "{stderr}"
    );
    let fetch = stderr
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("git -C ") && l.contains(" fetch "))
        .unwrap_or_else(|| panic!("no fetch command in {stderr}"));
    let args: Vec<&str> = fetch.split_whitespace().skip(1).collect();
    git_ok(&d.root, &args);
    assert_eq!(
        git_ok(&repo, &["log", "-1", "--format=%s", "aver/wt-kept"]),
        "negate\n"
    );

    let removed = remove(&d, &repo, &dir);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!dir.exists(), "fetched, so removed");

    // A commit on a detached HEAD is kept too, with a branch to name it.
    let detached = worktree(&d, &repo, "detached");
    git_ok(&detached, &["checkout", "--quiet", "--detach"]);
    std::fs::write(detached.join("README.md"), "detached\n").unwrap();
    git_ok(
        &detached,
        &["commit", "--quiet", "-am", "on a detached head"],
    );
    let refused = remove(&d, &repo, &detached);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "git -C {} branch aver/wt-detached-head",
            detached.display()
        )),
        "{stderr}"
    );
}

/// Waits until the run cache holds `n` runs, so a verify reads them from
/// the cache instead of joining them in flight.
fn wait_for_runs(d: &TestHome, n: i64) {
    let db = rusqlite::Connection::open(d.home.db_path()).unwrap();
    let cached = || -> i64 {
        db.query_row("SELECT COUNT(*) FROM run_cache", [], |r| r.get(0))
            .unwrap_or(0)
    };
    let start = Instant::now();
    while cached() < n {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "{} of {n} runs cached",
            cached()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn worktree_create_warms_the_base_so_an_unchanged_verify_starts_nothing() {
    let d = TestHome::start();
    let repo = d.calc();
    let first = worktree(&d, &repo, "one");
    // The base of calc passes, so warming it is one run.
    wait_for_runs(&d, 1);
    let out = aver(&d.home, &first, &["verify"], b"");
    let text = String::from_utf8_lossy(&out.stdout);
    // An unchanged directory is not done (nothing was changed yet), but the
    // verdict still comes from the warmed base without new runs.
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(
        text.starts_with("NOT DONE: PASS, but you changed nothing"),
        "{text}"
    );
    assert!(
        text.contains("original code cached from a run")
            && text.contains("your code cached from a run"),
        "{text}"
    );
    assert_eq!(d.runs(), 1);

    // Another session's worktree on the same commit is another task, with
    // the same base snapshot: its runs are the same cache keys.
    let second = worktree(&d, &repo, "two");
    let out = aver(&d.home, &second, &["verify"], b"");
    assert_eq!(out.status.code(), Some(1), "unchanged, so not done");
    assert_eq!(d.runs(), 1, "nothing ran again");
}

#[test]
fn workspaces_of_one_task_share_the_base_runs_and_an_unchanged_fresh_verify_runs_the_base_once() {
    let d = TestHome::start();
    let repo = d.calc();
    let out = aver(
        &d.home,
        &repo,
        &[
            "workspace",
            "new",
            "--no-deps",
            "--count",
            "2",
            "--label",
            "ws",
        ],
        b"",
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dirs: Vec<PathBuf> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(PathBuf::from)
        .collect();
    assert_eq!(dirs.len(), 2);
    wait_for_runs(&d, 1);
    for dir in &dirs {
        let out = aver(&d.home, dir, &["verify"], b"");
        assert_eq!(out.status.code(), Some(1), "unchanged, so not done");
    }
    assert_eq!(d.runs(), 1, "both workspaces read the warmed base");

    // `--fresh` joins and reads no cache; with nothing changed the head is
    // the base, so the base's one fresh run serves both sides.
    let out = aver(&d.home, &dirs[0], &["verify", "--fresh"], b"");
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert_eq!(d.runs(), 2, "{text}");
    assert!(
        text.contains("your code shared with the original code (the same tree)"),
        "{text}"
    );
}

/// `aver workspace rm` refuses a workspace whose Git view holds commits the
/// repository has not fetched, says how to fetch them, and removes it only
/// with `--force` (or once they are fetched).
#[test]
fn workspace_rm_keeps_unfetched_commits_unless_forced() {
    let d = TestHome::start();
    let repo = d.calc();
    let dir = worktree(&d, &repo, "rm-me");
    std::fs::write(dir.join("src/neg.js"), "module.exports = (x) => -x;\n").unwrap();
    git_ok(&dir, &["add", "src/neg.js"]);
    git_ok(&dir, &["commit", "--quiet", "-m", "negate"]);
    let rm = |extra: &[&str]| {
        let mut args = vec!["workspace", "rm"];
        args.extend_from_slice(extra);
        let target = dir.to_str().unwrap().to_owned();
        args.push(&target);
        aver(&d.home, &d.root, &args, b"")
    };
    let refused = rm(&[]);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(2), "{stderr}");
    assert!(dir.join("src/neg.js").is_file(), "the directory is kept");
    assert!(
        stderr.contains("refusing to remove") && stderr.contains(" negate"),
        "{stderr}"
    );
    assert!(stderr.contains(" fetch "), "{stderr}");
    assert!(stderr.contains("aver workspace rm --force"), "{stderr}");

    let forced = rm(&["--force"]);
    assert!(
        forced.status.success(),
        "{}",
        String::from_utf8_lossy(&forced.stderr)
    );
    assert!(!dir.exists(), "forced, so removed");
}

/// `aver workspace rm` removes a workspace without `--force` once the
/// commit its directory equals is merged into the repository, and keeps
/// refusing while a ref does not reach that commit or the directory changed
/// after it.
#[test]
fn workspace_rm_removes_a_directory_its_repository_merged() {
    let d = TestHome::start();
    let repo = d.calc();
    let rm = |dir: &Path| {
        let target = dir.to_str().unwrap().to_owned();
        aver(&d.home, &d.root, &["workspace", "rm", &target], b"")
    };
    let commit = |name: &str| {
        let dir = worktree(&d, &repo, name);
        let file = format!("src/{name}.js");
        std::fs::write(dir.join(&file), "module.exports = (x) => -x;\n").unwrap();
        git_ok(&dir, &["add", &file]);
        git_ok(&dir, &["commit", "--quiet", "-m", "negate"]);
        let branch = format!("refs/heads/aver/wt-{name}");
        git_ok(
            &repo,
            &["fetch", "--quiet", view_of(&dir).to_str().unwrap(), &branch],
        );
        dir
    };

    // Fetched but on no ref: the objects are there, the work is not merged.
    let unmerged = commit("unmerged");
    let refused = rm(&unmerged);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("no commit holds the changes"), "{stderr}");
    assert!(
        unmerged.join("src/unmerged.js").is_file(),
        "the directory is kept"
    );

    // Merged into main: nothing would be lost.
    git_ok(&repo, &["merge", "--quiet", "--ff-only", "FETCH_HEAD"]);
    let removed = rm(&unmerged);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!unmerged.exists(), "merged, so removed");

    // Merged, then edited: the edit is in no commit.
    let dirty = commit("dirty");
    git_ok(
        &repo,
        &["update-ref", "refs/heads/merged-dirty", "FETCH_HEAD"],
    );
    std::fs::write(dirty.join("README.md"), "edited after the commit\n").unwrap();
    let refused = rm(&dirty);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("README.md"), "{stderr}");
    assert!(dirty.join("README.md").is_file(), "the directory is kept");
}

/// `aver gc` removes, slots and all, a directory equal to a commit of its
/// Git view that a branch of the repository merged, and keeps one edited
/// after that commit, one whose commit no ref reaches, and a fresh one
/// equal to its base.
#[test]
fn gc_removes_merged_clean_workspaces_and_keeps_the_rest() {
    let d = TestHome::start();
    let repo = d.calc();
    let commit = |name: &str| {
        let dir = worktree(&d, &repo, name);
        let file = format!("src/{name}.js");
        std::fs::write(dir.join(&file), "module.exports = (x) => -x;\n").unwrap();
        git_ok(&dir, &["add", &file]);
        git_ok(&dir, &["commit", "--quiet", "-m", name]);
        let branch = format!("refs/heads/aver/wt-{name}");
        git_ok(
            &repo,
            &["fetch", "--quiet", view_of(&dir).to_str().unwrap(), &branch],
        );
        dir
    };
    let merged = commit("merged");
    git_ok(&repo, &["update-ref", "refs/heads/merged", "FETCH_HEAD"]);
    let dirty = commit("dirty");
    git_ok(&repo, &["update-ref", "refs/heads/dirty", "FETCH_HEAD"]);
    std::fs::write(dirty.join("README.md"), "edited after the commit\n").unwrap();
    let unmerged = commit("unmerged");
    let fresh = worktree(&d, &repo, "fresh");

    let out = aver(&d.home, &d.root, &["gc", "--json"], b"");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let removed = json["merged_removed"].as_array().unwrap();
    assert_eq!(removed.len(), 1, "{json:#}");
    assert_eq!(removed[0]["dir"], merged.to_str().unwrap(), "{json:#}");
    assert!(!merged.exists(), "merged and clean, so removed");
    let ls = aver(&d.home, &d.root, &["workspace", "ls"], b"");
    let listed = String::from_utf8_lossy(&ls.stdout);
    assert!(
        !listed.contains(merged.to_str().unwrap()),
        "released: {listed}"
    );
    assert!(listed.contains(fresh.to_str().unwrap()), "{listed}");
    assert!(dirty.join("README.md").is_file(), "edited after the commit");
    assert!(
        unmerged.join("src/unmerged.js").is_file(),
        "no ref reaches it"
    );
    assert!(fresh.is_dir(), "equal only to its base");

    let out = aver(&d.home, &d.root, &["gc"], b"");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("removed 0 merged workspace directories"),
        "{stdout}"
    );
}

/// A second `aver workspace new` on an unchanged `HEAD` runs no Git at
/// all: the import is memoized, and the view's `HEAD`, objects, identity
/// and `origin` are written in Rust, in the bytes `git config` writes.
#[test]
fn a_warm_workspace_new_runs_no_git_and_writes_the_config_git_would() {
    let d = TestHome::start();
    let repo = d.calc();
    let name = "Ada \"the; first\" \\ #1";
    git_ok(&repo, &["config", "user.name", name]);
    let cold = aver(&d.home, &repo, &["workspace", "new", "."], b"");
    assert!(
        cold.status.success(),
        "{}",
        String::from_utf8_lossy(&cold.stderr)
    );

    let real = git_ok(&d.root, &["--exec-path"]);
    let real = Path::new(real.trim()).join("git");
    let shims = d.root.join("shims");
    std::fs::create_dir_all(&shims).unwrap();
    let log = d.root.join("git.log");
    let shim = shims.join("git");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\necho \"$PPID $*\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        shims.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut cmd = Command::new(aver_bin());
    for var in aver::run::accept::AGENT_ENV {
        cmd.env_remove(var);
    }
    let child = cmd
        .args(["workspace", "new", "."])
        .current_dir(&repo)
        .env("AVER_HOME", d.home.root())
        .env("AVER_SCRIPTED", "1")
        .env("PATH", &path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let warm = child.wait_with_output().unwrap();
    assert!(
        warm.status.success(),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    let ran = std::fs::read_to_string(&log).unwrap_or_default();
    let own: Vec<&str> = ran
        .lines()
        .filter(|l| l.split(' ').next() == Some(&pid.to_string()))
        .collect();
    assert!(own.is_empty(), "the warm call ran git: {own:?}");

    let dir = PathBuf::from(String::from_utf8(warm.stdout).unwrap().trim());
    let expected = d.root.join("expected-config");
    for (key, value) in [
        ("user.name", name),
        ("user.email", "owner@example.com"),
        ("remote.origin.url", repo.to_str().unwrap()),
        ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"),
    ] {
        git_ok(
            &d.root,
            &["config", "--file", expected.to_str().unwrap(), key, value],
        );
    }
    let config = std::fs::read_to_string(view_of(&dir).join("config")).unwrap();
    let expected = std::fs::read_to_string(&expected).unwrap();
    assert!(config.ends_with(&expected), "{config}\n---\n{expected}");
    assert_eq!(git_ok(&dir, &["config", "user.name"]), format!("{name}\n"));
    assert_eq!(
        git_ok(&dir, &["remote", "-v"]),
        format!("origin\t{0} (fetch)\norigin\t{0} (push)\n", repo.display())
    );
    assert_eq!(git_ok(&dir, &["status", "--porcelain"]), "");
}
