//! `aver ci approve` end to end: the CLI binary with a stub `gh` on PATH
//! answering the pull request and its reviews, over a real Git repository
//! whose head tree the approval records (ARCHITECTURE.md §2.9, approval).

mod git_support;

use std::path::PathBuf;
use std::process::{Command, Output};

use git_support::git_cmd;
use tempfile::TempDir;

struct World {
    dir: TempDir,
    old: String,
    head: String,
}

impl World {
    /// A repository with two commits: `old` (what was reviewed first) and
    /// `head` (a push after it).
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| -> String {
            let out = git_cmd(&repo).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a"), "1").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "one"]);
        let old = git(&["rev-parse", "HEAD"]);
        std::fs::write(repo.join("a"), "2").unwrap();
        git(&["commit", "-q", "-am", "two"]);
        let head = git(&["rev-parse", "HEAD"]);
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let gh = bin.join("gh");
        std::fs::write(
            &gh,
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >>\"$GH_LOG\"\ncase \"$*\" in\n  *reviews*) cat \"$GH_REVIEWS\" ;;\n  *) cat \"$GH_PULL\" ;;\nesac\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir, old, head }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn tree(&self, commit: &str) -> String {
        let out = git_cmd(&self.path("repo"))
            .args(["rev-parse", &format!("{commit}^{{tree}}")])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// Runs `aver ci approve` for a review event by `reviewer` (review 50),
    /// with the pull request's head now at `current` and `reviews` listed
    /// by the API in two pages.
    fn approve(&self, current: &str, reviews: &[serde_json::Value]) -> Output {
        std::fs::write(
            self.path("event.json"),
            serde_json::json!({
                "action": "submitted",
                "review": {"id": 50, "state": "approved"},
                "pull_request": {"number": 7, "user": {"login": "author", "type": "User"},
                    "head": {"sha": self.old},
                    "base": {"repo": {"full_name": "o/r"}}},
                "repository": {"full_name": "o/r"}
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            self.path("pull.json"),
            serde_json::json!({"number": 7, "head": {"sha": current}}).to_string(),
        )
        .unwrap();
        let (first, rest) = reviews.split_at(reviews.len().min(1));
        std::fs::write(
            self.path("reviews.json"),
            format!(
                "{}\n{}\n",
                serde_json::Value::from(first.to_vec()),
                serde_json::Value::from(rest.to_vec())
            ),
        )
        .unwrap();
        let path = format!(
            "{}:{}",
            self.path("bin").display(),
            std::env::var("PATH").unwrap()
        );
        Command::new(env!("CARGO_BIN_EXE_aver"))
            .args(["ci", "approve", "--github-event"])
            .arg(self.path("event.json"))
            .arg("--repo")
            .arg(self.path("repo"))
            .arg("--out-dir")
            .arg(self.path("out"))
            .env("PATH", path)
            .env("GH_LOG", self.path("gh.log"))
            .env("GH_PULL", self.path("pull.json"))
            .env("GH_REVIEWS", self.path("reviews.json"))
            .env("AVER_HOME", self.path("must-not-be-used"))
            .env_remove("AVER_SCRIPTED")
            .output()
            .unwrap()
    }

    fn approval(&self) -> Option<serde_json::Value> {
        let file = self.path("out/approval.json");
        file.exists()
            .then(|| serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap())
    }
}

fn review(id: u64, commit: &str, login: &str, kind: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "state": "APPROVED", "commit_id": commit,
        "user": {"login": login, "type": kind}, "author_association": "COLLABORATOR"})
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_persons_approval_of_the_current_head_binds_with_its_tree() {
    let w = World::new();
    let comment = serde_json::json!({"id": 49, "state": "COMMENTED", "commit_id": w.head,
        "user": {"login": "carol", "type": "User"}, "author_association": "MEMBER"});
    let out = w.approve(&w.head, &[comment, review(50, &w.head, "alice", "User")]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(stdout(&out).starts_with(&format!("APPROVED {} by alice (review 50)", w.head)));
    assert_eq!(
        w.approval().unwrap(),
        serde_json::json!({
            "repository": "o/r", "pull_request": 7, "reviewer": "alice", "review_id": 50,
            "head_commit": w.head, "head_tree": w.tree(&w.head), "receipt": null
        })
    );
    let log = std::fs::read_to_string(w.path("gh.log")).unwrap();
    assert!(log.contains("api repos/o/r/pulls/7\n"), "{log}");
    assert!(
        log.contains("api --paginate repos/o/r/pulls/7/reviews?per_page=100"),
        "{log}"
    );
    assert!(!w.path("must-not-be-used").exists());
}

/// A push after the review moves the head: the approval names the older
/// commit and no longer binds, and an approval.json from before is gone.
#[test]
fn an_approval_of_an_older_commit_does_not_bind() {
    let w = World::new();
    assert_eq!(
        w.approve(&w.old, &[review(50, &w.old, "alice", "User")])
            .status
            .code(),
        Some(0)
    );
    assert!(w.approval().is_some());
    let out = w.approve(&w.head, &[review(50, &w.old, "alice", "User")]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert_eq!(
        stdout(&out),
        format!(
            "NOT APPROVED: review 50 by alice approved {}, not the current head {}: a push after the review needs a new approval\n",
            w.old, w.head
        )
    );
    assert!(w.approval().is_none());
}

#[test]
fn the_authors_and_a_bots_approval_do_not_bind() {
    let w = World::new();
    let out = w.approve(&w.head, &[review(50, &w.head, "author", "User")]);
    assert_eq!(out.status.code(), Some(3));
    assert!(
        stdout(&out).contains("the pull request's author"),
        "{out:?}"
    );
    for (login, kind) in [("renovate[bot]", "User"), ("github-actions", "Bot")] {
        let out = w.approve(&w.head, &[review(50, &w.head, login, kind)]);
        assert_eq!(out.status.code(), Some(3));
        assert!(
            stdout(&out).contains("which is not a person's account"),
            "{out:?}"
        );
    }
    let out = w.approve(&w.head, &[]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(
        stdout(&out),
        format!(
            "NOT APPROVED: no approval of the pull request's current head {}\n",
            w.head
        )
    );
    assert!(w.approval().is_none());
}

#[test]
fn a_head_missing_from_the_checkout_is_an_error() {
    let w = World::new();
    let missing = "3".repeat(40);
    let out = w.approve(&missing, &[review(50, &missing, "alice", "User")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("fetch it first"),
        "{out:?}"
    );
}
