//! Builds Git fixture repositories with plumbing only (`hash-object`,
//! `mktree`, `commit-tree`), so tree entries are ordered by Git itself.
#![allow(dead_code)]

use std::io::Write;
use std::path::{Path as FsPath, PathBuf};
use std::process::{Command, Stdio};

use aver::env::StaticResolver;
use aver::git::{self, ImportOptions, ImportReport};
use aver::store::Store;
use aver_core::*;
use tempfile::TempDir;

/// Keeps the developer's Git configuration out of `cmd`: no global or
/// system config, and no global excludes file, which Git reads from
/// `~/.config/git/ignore` even without a global config and which would
/// hide untracked files from `git status`.
pub fn hermetic(cmd: &mut Command) -> &mut Command {
    cmd.env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.excludesFile")
        .env("GIT_CONFIG_VALUE_0", "/dev/null")
}

pub fn git_cmd(dir: &FsPath) -> Command {
    let mut cmd = Command::new("git");
    hermetic(cmd.arg("-C").arg(dir))
        .env("GIT_AUTHOR_NAME", "Fixture Author")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_DATE", "1700000000 +0000")
        .env("GIT_COMMITTER_NAME", "Fixture Committer")
        .env("GIT_COMMITTER_EMAIL", "committer@example.com")
        .env("GIT_COMMITTER_DATE", "1700000000 +0000");
    cmd
}

/// Runs git in `dir`, panicking with stderr on failure.
pub fn git(dir: &FsPath, args: &[&str], stdin: &[u8]) -> Vec<u8> {
    let mut child = git_cmd(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

pub fn git_str(dir: &FsPath, args: &[&str]) -> String {
    String::from_utf8(git(dir, args, b""))
        .unwrap()
        .trim_end()
        .to_owned()
}

pub enum F {
    File(Vec<u8>),
    Exec(Vec<u8>),
    Link(&'static str),
    Dir(Vec<(Vec<u8>, F)>),
    Gitlink(&'static str),
}

pub fn file(name: &str, content: &str) -> (Vec<u8>, F) {
    (
        name.as_bytes().to_vec(),
        F::File(content.as_bytes().to_vec()),
    )
}

pub fn exec(name: &str, content: &str) -> (Vec<u8>, F) {
    (
        name.as_bytes().to_vec(),
        F::Exec(content.as_bytes().to_vec()),
    )
}

pub fn link(name: &str, target: &'static str) -> (Vec<u8>, F) {
    (name.as_bytes().to_vec(), F::Link(target))
}

pub fn dir(name: &str, entries: Vec<(Vec<u8>, F)>) -> (Vec<u8>, F) {
    (name.as_bytes().to_vec(), F::Dir(entries))
}

/// The files under `dir` as tree entries; executables keep their mode and
/// `node_modules` and `.git` are skipped.
pub fn entries_of_dir(dir: &FsPath) -> Vec<(Vec<u8>, F)> {
    use std::os::unix::fs::PermissionsExt;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        if name == "node_modules" || name == ".git" {
            continue;
        }
        let meta = std::fs::metadata(entry.path()).unwrap();
        let node = if meta.is_dir() {
            F::Dir(entries_of_dir(&entry.path()))
        } else {
            let bytes = std::fs::read(entry.path()).unwrap();
            if meta.permissions().mode() & 0o111 != 0 {
                F::Exec(bytes)
            } else {
                F::File(bytes)
            }
        };
        out.push((name.into_bytes(), node));
    }
    out
}

pub struct Fixture {
    _dir: TempDir,
    pub repo: PathBuf,
}

impl Fixture {
    /// A fresh bare repository.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("source.git");
        let parent = dir.path().to_path_buf();
        git(
            &parent,
            &[
                "init",
                "--bare",
                "--quiet",
                "--initial-branch=main",
                "source.git",
            ],
            b"",
        );
        Self { _dir: dir, repo }
    }

    pub fn path(&self) -> &str {
        self.repo.to_str().unwrap()
    }

    pub fn blob(&self, content: &[u8]) -> String {
        String::from_utf8(git(&self.repo, &["hash-object", "-w", "--stdin"], content))
            .unwrap()
            .trim_end()
            .to_owned()
    }

    /// Writes a tree with `git mktree`, which orders entries the Git way.
    pub fn tree(&self, entries: &[(Vec<u8>, F)]) -> String {
        let mut input = Vec::new();
        for (name, node) in entries {
            let (mode, kind, sha) = match node {
                F::File(c) => ("100644", "blob", self.blob(c)),
                F::Exec(c) => ("100755", "blob", self.blob(c)),
                F::Link(t) => ("120000", "blob", self.blob(t.as_bytes())),
                F::Dir(children) => ("040000", "tree", self.tree(children)),
                F::Gitlink(sha) => ("160000", "commit", (*sha).to_owned()),
            };
            input.extend_from_slice(format!("{mode} {kind} {sha}\t").as_bytes());
            input.extend_from_slice(name);
            input.push(0);
        }
        String::from_utf8(git(&self.repo, &["mktree", "-z", "--missing"], &input))
            .unwrap()
            .trim_end()
            .to_owned()
    }

    /// Commits `tree` and points `main` at the result.
    pub fn commit(&self, tree: &str, parents: &[&str], message: &str) -> String {
        let mut args = vec!["commit-tree", tree];
        for p in parents {
            args.extend(["-p", p]);
        }
        args.extend(["-m", message]);
        let sha = git_str(&self.repo, &args);
        git_str(&self.repo, &["update-ref", "refs/heads/main", &sha]);
        sha
    }

    pub fn commit_tree(&self, entries: &[(Vec<u8>, F)], message: &str) -> String {
        let tree = self.tree(entries);
        self.commit(&tree, &[], message)
    }
}

pub struct Aver {
    pub home: TempDir,
    pub store: Store,
    pub key: SigningKey,
    pub resolver: StaticResolver,
}

impl Aver {
    pub fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let store = Store::open(home.path().join("aver.db")).unwrap();
        Self {
            home,
            store,
            key: SigningKey::from_bytes(&[7; 32]),
            resolver: StaticResolver::default(),
        }
    }

    pub fn agent(&self) -> AgentId {
        AgentId::of_public_key(&self.key.verifying_key())
    }

    pub fn options<'a>(&'a self, source: &'a str, rev: &'a str) -> ImportOptions<'a> {
        ImportOptions::new(source, rev, self.agent(), &self.key, &self.resolver)
    }

    pub fn import(&self, source: &str, rev: &str) -> ImportReport {
        git::import(&self.store, &self.options(source, rev)).unwrap()
    }

    pub fn root(&self, snapshot: &SnapshotId) -> TreeId {
        self.store.get::<Snapshot>(snapshot).unwrap().root
    }
}

pub fn p<T: std::str::FromStr>(s: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    s.parse().unwrap()
}
