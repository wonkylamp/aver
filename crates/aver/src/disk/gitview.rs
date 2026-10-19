//! The write-only Git view of a workspace directory (ARCHITECTURE.md
//! §2.3.1, CLAUDE.md rule 4): agents run `git status`, `git diff` and
//! `git commit` in their directory and see their changes against the base
//! commit.
//!
//! The directory is the one linked worktree of `$AVER_HOME/git/<ws>`, a
//! bare repository whose objects come from the user's repository through
//! `objects/info/alternates` (nothing is copied). Its `HEAD` is the branch
//! `aver/<label>` at the base commit, and Aver writes its index from the
//! clone's `lstat` data and the blob SHA-1s import recorded, so the first
//! `git status` hashes nothing. Its config, written in the bytes `git
//! config` writes so no Git process runs, carries the repository's local
//! `user.name` and `user.email`, and an `origin` remote that is the user's
//! repository, so `git fetch origin` and `git push origin <branch>` work
//! there. Aver never
//! reads this repository's contents back: the overlay stays the only truth,
//! and an agent's commits or stashes there move nothing. The reads are of
//! refs and commit messages only: [`unfetched`], which removal uses so it
//! never deletes commits the user's repository lacks, [`merged`], the
//! commits it already has, which removal compares the directory with, and
//! [`latest_subject`], the agent's commit title an accept names its commit
//! after.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use aver_core::GitSha;
use sha1::{Digest, Sha1};

use crate::workspace::FileKind;

/// One base file for the index.
pub struct IndexFile<'a> {
    pub path: &'a str,
    pub kind: FileKind,
    pub sha: &'a GitSha,
}

/// What [`write`] needs.
pub struct ViewSpec<'a> {
    /// `$AVER_HOME/git/<ws>`.
    pub gitdir: &'a Path,
    /// The workspace directory.
    pub dir: &'a Path,
    /// The user's repository, whose objects the view borrows.
    pub repo: &'a Path,
    pub head: &'a GitSha,
    /// The branch `HEAD` names, from [`branch_for`].
    pub branch: &'a str,
    /// Base files, in any order.
    pub files: Vec<IndexFile<'a>>,
    /// Paths Git should ignore (Aver's own and seeded ones), repo-relative.
    pub exclude: Vec<String>,
}

/// The view's branch for a workspace directory named `label`:
/// `aver/<label>`, or `aver/workspace` where the label is not a valid
/// branch name component (labels may hold `..` or end in `.lock`).
pub fn branch_for(label: &str) -> String {
    let valid = !label.is_empty()
        && !label.starts_with('.')
        && !label.ends_with('.')
        && !label.ends_with(".lock")
        && !label.contains("..")
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if valid {
        format!("aver/{label}")
    } else {
        "aver/workspace".to_owned()
    }
}

/// Where the view of workspace `ws` lives under the home at `home`, the
/// repository a coordinator fetches from (`Disk::git_dir`).
pub fn view_dir(home: &Path, ws: &aver_core::WorkspaceId) -> PathBuf {
    home.join("git").join(ws.to_string())
}

/// `git` with no repository taken from the environment.
fn git() -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

/// The object directory of the repository at `repo` (its common directory
/// for a linked worktree).
fn objects_dir(repo: &Path) -> io::Result<PathBuf> {
    if let Some(dirs) = crate::git::local::dirs(repo) {
        return Ok(dirs.common.join("objects"));
    }
    let out = git()
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(repo)
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "{} is not a Git repository: {}",
            repo.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let common = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Ok(PathBuf::from(common).join("objects"))
}

/// The repository's own `user.name` and `user.email` (`--local`), which
/// the view carries so a commit an agent makes there has the identity the
/// repository chose rather than the machine's global one.
fn local_identity(repo: &Path) -> io::Result<[(&'static str, Option<String>); 2]> {
    let (name, email) = match crate::git::local::local_identity(repo) {
        Some(found) => found,
        None => {
            let get = |key: &str| -> io::Result<Option<String>> {
                let out = git()
                    .args(["config", "--local", "--get", key])
                    .current_dir(repo)
                    .output()?;
                let value = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                Ok((out.status.success() && !value.is_empty()).then_some(value))
            };
            (get("user.name")?, get("user.email")?)
        }
    };
    let keep = |v: Option<String>| v.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    Ok([("name", keep(name)), ("email", keep(email))])
}

/// `key = value` as `git config --file` writes it into a section: quoted
/// where a leading or trailing space or a comment character would be lost,
/// with `\n`, `\t`, `"` and `\\` escaped.
fn config_line(key: &str, value: &str) -> String {
    let quote = value.starts_with(' ') || value.ends_with(' ') || value.contains([';', '#', '\r']);
    let q = if quote { "\"" } else { "" };
    let mut line = format!("\t{key} = {q}");
    for c in value.chars() {
        match c {
            '\n' => line.push_str("\\n"),
            '\t' => line.push_str("\\t"),
            '"' | '\\' => {
                line.push('\\');
                line.push(c);
            }
            c => line.push(c),
        }
    }
    line.push_str(q);
    line.push('\n');
    line
}

/// The view's config: the bare repository's core settings, the identity,
/// and an `origin` remote that is the user's repository, in the bytes
/// `git config --file` would leave.
fn view_config(repo: &Path) -> io::Result<String> {
    let mut text = String::from(
        "# Written by Aver: a view of one workspace for git status, diff and commit.\n\
         # Aver never reads its contents; commits made here change nothing in Aver.\n\
         [core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = true\n\
         \tcheckStat = minimal\n\ttrustctime = false\n\tuntrackedCache = true\n\
         [gc]\n\tauto = 0\n",
    );
    let identity = local_identity(repo)?;
    if identity.iter().any(|(_, v)| v.is_some()) {
        text.push_str("[user]\n");
        for (key, value) in &identity {
            if let Some(value) = value {
                text.push_str(&config_line(key, value));
            }
        }
    }
    // `git fetch origin` and `git push origin <branch>` in the directory
    // reach the user's repository; Aver itself still never reads the view.
    text.push_str("[remote \"origin\"]\n");
    text.push_str(&config_line("url", &repo.display().to_string()));
    text.push_str(&config_line("fetch", "+refs/heads/*:refs/remotes/origin/*"));
    Ok(text)
}

/// Writes the view. Called once, on a fresh directory.
///
/// The layout is the one `git worktree add` makes from a bare repository:
/// `$AVER_HOME/git/<ws>` is a bare repository holding the refs, config and
/// excludes, and the directory is its one linked worktree, with its `HEAD`
/// and index under `worktrees/<name>/` and `gitdir` and `commondir` files
/// tying the two together. Git and tools that resolve the layout
/// themselves (Claude Code vets a worktree's config before it counts its
/// uncommitted files) read it like any linked worktree, and
/// `git worktree list` names the directory.
pub fn write(spec: &ViewSpec<'_>) -> io::Result<()> {
    let objects = objects_dir(spec.repo)?;
    let common = spec.gitdir;
    if common.exists() {
        fs::remove_dir_all(common)?;
    }
    let linked = linked_dir(common, spec.dir);
    let branch = common.join("refs/heads").join(spec.branch);
    for sub in ["objects/info", "refs/tags", "info"] {
        fs::create_dir_all(common.join(sub))?;
    }
    fs::create_dir_all(&linked)?;
    fs::create_dir_all(branch.parent().expect("a branch has a parent"))?;
    fs::write(
        common.join("objects/info/alternates"),
        format!("{}\n", objects.display()),
    )?;
    fs::write(&branch, format!("{}\n", spec.head))?;
    let head = format!("ref: refs/heads/{}\n", spec.branch);
    fs::write(common.join("HEAD"), &head)?;
    fs::write(common.join("config"), view_config(spec.repo)?)?;
    let mut exclude = String::from("# Managed by Aver\n");
    for path in &spec.exclude {
        exclude.push('/');
        exclude.push_str(path.trim_end_matches('/'));
        exclude.push('\n');
    }
    fs::write(common.join("info/exclude"), exclude)?;
    fs::write(common.join("description"), "Aver workspace view\n")?;
    fs::write(linked.join("HEAD"), &head)?;
    fs::write(linked.join("commondir"), "../..\n")?;
    fs::write(
        linked.join("gitdir"),
        format!("{}\n", spec.dir.join(".git").display()),
    )?;
    write_index(&linked.join("index"), spec.dir, &spec.files)?;
    fs::write(
        spec.dir.join(".git"),
        format!("gitdir: {}\n", linked.display()),
    )?;
    Ok(())
}

/// The linked worktree's own Git directory under the view at `common`.
fn linked_dir(common: &Path, dir: &Path) -> PathBuf {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && !n.starts_with('.'))
        .unwrap_or_else(|| "workspace".to_owned());
    common.join("worktrees").join(name)
}

/// The Git directory (common directory) of the user's repository whose
/// objects the view at `gitdir` borrows, from its alternates.
pub fn source(gitdir: &Path) -> io::Result<PathBuf> {
    let alternates = fs::read_to_string(gitdir.join("objects/info/alternates"))?;
    let objects = alternates
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("the Git view names no repository"))?;
    objects
        .parent()
        .filter(|r| r.join("HEAD").is_file())
        .map(Path::to_owned)
        .ok_or_else(|| {
            io::Error::other(format!(
                "the Git view's repository ({}) is gone",
                objects.display()
            ))
        })
}

/// The subject of the latest commit on the view's `HEAD` (its worktree's)
/// that `base` does not reach: what an agent named its last commit. `None`
/// when it made none. Like [`unfetched`], it reads only Git's own refs and
/// messages, never into the store.
pub fn latest_subject(gitdir: &Path, base: &str) -> io::Result<Option<String>> {
    let mut heads: Vec<PathBuf> = fs::read_dir(gitdir.join("worktrees"))
        .map(|e| e.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    heads.sort();
    heads.push(gitdir.to_owned());
    for dir in heads {
        let out = git()
            .arg("--git-dir")
            .arg(&dir)
            .args(["log", "-1", "--format=%s", "HEAD", "--not", base, "--"])
            .output()?;
        let subject = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if out.status.success() && !subject.is_empty() {
            return Ok(Some(subject));
        }
    }
    Ok(None)
}

/// A commit in a view that the user's repository does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unfetched {
    /// The ref that holds it (`refs/heads/aver/…`, `refs/stash`), or `HEAD`
    /// for a detached one.
    pub refname: String,
    pub sha: String,
    /// Its subject line.
    pub subject: String,
}

/// The tips of the view at `gitdir` (`$AVER_HOME/git/<ws>`: every ref, and
/// the worktree's `HEAD` when it is detached) whose
/// commit the user's repository, the one its alternates name, does not
/// have: work that removing the view would lose. Removal is the one place
/// Aver reads a view, and it reads only refs, never into the store. A view
/// whose repository is gone or unreadable is an error, so a caller keeps it.
pub fn unfetched(gitdir: &Path) -> io::Result<Vec<Unfetched>> {
    let repo = source(gitdir)?;
    let run = |dir: &Path, args: &[&str]| -> io::Result<std::process::Output> {
        git().arg("--git-dir").arg(dir).args(args).output()
    };
    let refs = run(
        gitdir,
        &[
            "for-each-ref",
            "--format=%(objectname) %(objecttype) %(refname)",
        ],
    )?;
    if !refs.status.success() {
        return Err(io::Error::other(format!(
            "listing the Git view's refs: {}",
            String::from_utf8_lossy(&refs.stderr).trim()
        )));
    }
    let mut tips: Vec<(String, String)> = String::from_utf8_lossy(&refs.stdout)
        .lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, ' ');
            let (sha, kind, name) = (parts.next()?, parts.next()?, parts.next()?);
            (kind == "commit").then(|| (name.to_owned(), sha.to_owned()))
        })
        .collect();
    let mut heads = vec![gitdir.to_owned()];
    if let Ok(entries) = fs::read_dir(gitdir.join("worktrees")) {
        heads.extend(entries.flatten().map(|e| e.path()));
    }
    for dir in heads {
        let detached = run(&dir, &["symbolic-ref", "-q", "HEAD"])?;
        if detached.status.success() {
            continue;
        }
        let head = run(&dir, &["rev-parse", "-q", "--verify", "HEAD^{commit}"])?;
        let sha = String::from_utf8_lossy(&head.stdout).trim().to_owned();
        if head.status.success() && !sha.is_empty() {
            tips.push(("HEAD".to_owned(), sha));
        }
    }
    let mut out = Vec::new();
    for (refname, sha) in tips {
        if out.iter().any(|u: &Unfetched| u.sha == sha) {
            continue;
        }
        let have = run(&repo, &["cat-file", "-e", &format!("{sha}^{{commit}}")])?;
        if have.status.success() {
            continue;
        }
        let subject = run(gitdir, &["log", "-1", "--format=%s", &sha])?;
        out.push(Unfetched {
            refname,
            sha,
            subject: String::from_utf8_lossy(&subject.stdout).trim().to_owned(),
        });
    }
    Ok(out)
}

/// Up to `limit` commits of the view at `gitdir` (its branches and any
/// detached worktree `HEAD`), newest first, that a ref of the user's
/// repository reaches: work already merged there. Removal imports them
/// from the user's repository, never from the view, to see whether the
/// directory equals one. Commits `exclude` reaches (the workspace's base,
/// for `aver gc`) are left out. Like [`unfetched`], it reads only refs and
/// the commit graph.
pub fn merged(gitdir: &Path, limit: usize, exclude: Option<&str>) -> io::Result<Vec<String>> {
    let repo = source(gitdir)?;
    let run = |dir: &Path, args: &[&str]| -> io::Result<std::process::Output> {
        git().arg("--git-dir").arg(dir).args(args).output()
    };
    let refs = run(
        gitdir,
        &[
            "for-each-ref",
            "--format=%(objectname) %(objecttype)",
            "refs/heads",
        ],
    )?;
    let mut tips: Vec<String> = String::from_utf8_lossy(&refs.stdout)
        .lines()
        .filter_map(|l| l.strip_suffix(" commit").map(str::to_owned))
        .collect();
    if let Ok(entries) = fs::read_dir(gitdir.join("worktrees")) {
        for dir in entries.flatten().map(|e| e.path()) {
            let head = run(&dir, &["rev-parse", "-q", "--verify", "HEAD^{commit}"])?;
            let sha = String::from_utf8_lossy(&head.stdout).trim().to_owned();
            if head.status.success() && !sha.is_empty() {
                tips.push(sha);
            }
        }
    }
    tips.sort();
    tips.dedup();
    if tips.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["rev-list".to_owned(), format!("--max-count={}", limit * 4)];
    args.extend(tips);
    if let Some(base) = exclude {
        args.push(format!("^{base}"));
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let list = run(gitdir, &args)?;
    if !list.status.success() {
        return Err(io::Error::other(format!(
            "listing the Git view's commits: {}",
            String::from_utf8_lossy(&list.stderr).trim()
        )));
    }
    let mut out = Vec::new();
    for sha in String::from_utf8_lossy(&list.stdout).lines() {
        let contains = format!("--contains={sha}");
        let reached = run(&repo, &["for-each-ref", "--count=1", &contains])?;
        if reached.status.success() && !reached.stdout.is_empty() {
            out.push(sha.to_owned());
            if out.len() == limit {
                break;
            }
        }
    }
    Ok(out)
}

/// Writes a version 2 index of `files` with each file's `lstat` data from
/// `dir`. A file missing from the directory gets zeroed stat data, which
/// Git reports as changed.
fn write_index(path: &Path, dir: &Path, files: &[IndexFile<'_>]) -> io::Result<()> {
    let mut sorted: Vec<&IndexFile<'_>> = files.iter().collect();
    sorted.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    let mut out = Vec::with_capacity(64 + sorted.len() * 96);
    out.extend_from_slice(b"DIRC");
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&(sorted.len() as u32).to_be_bytes());
    for file in sorted {
        let meta = fs::symlink_metadata(dir.join(file.path)).ok();
        let field = |f: fn(&fs::Metadata) -> i64| meta.as_ref().map_or(0, f) as u32;
        let start = out.len();
        for value in [
            field(|m| m.ctime()),
            field(|m| m.ctime_nsec()),
            field(|m| m.mtime()),
            field(|m| m.mtime_nsec()),
            field(|m| m.dev() as i64),
            field(|m| m.ino() as i64),
        ] {
            out.extend_from_slice(&value.to_be_bytes());
        }
        let mode: u32 = match file.kind {
            FileKind::File => 0o100644,
            FileKind::Exec => 0o100755,
            FileKind::Symlink => 0o120000,
        };
        out.extend_from_slice(&mode.to_be_bytes());
        for value in [
            field(|m| m.uid() as i64),
            field(|m| m.gid() as i64),
            field(|m| m.size() as i64),
        ] {
            out.extend_from_slice(&value.to_be_bytes());
        }
        let sha = data_encoding::HEXLOWER
            .decode(file.sha.as_str().as_bytes())
            .map_err(|e| io::Error::other(format!("git sha {}: {e}", file.sha)))?;
        out.extend_from_slice(&sha);
        let name = file.path.as_bytes();
        out.extend_from_slice(&(name.len().min(0xfff) as u16).to_be_bytes());
        out.extend_from_slice(name);
        // One to eight NULs pad the entry to a multiple of eight bytes.
        let len = out.len() - start;
        let pad = 8 - (len % 8);
        out.extend(std::iter::repeat_n(0u8, pad));
    }
    let checksum = Sha1::digest(&out);
    out.extend_from_slice(&checksum);
    let tmp = path.with_extension("lock");
    fs::write(&tmp, &out)?;
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gitview_config_lines_are_the_bytes_git_config_writes() {
        let dir = tempfile::tempdir().unwrap();
        for value in [
            "Ada Lovelace",
            "Ada \"the; first\" \\ #1",
            "tab\there",
            "new\nline",
            "x;y",
            " lead",
            "trail ",
            "+refs/heads/*:refs/remotes/origin/*",
        ] {
            let file = dir.path().join("config");
            let _ = fs::remove_file(&file);
            let ok = git()
                .args(["config", "--file"])
                .arg(&file)
                .args(["user.name", value])
                .status()
                .unwrap()
                .success();
            assert!(ok);
            let want = fs::read_to_string(&file).unwrap();
            assert_eq!(format!("[user]\n{}", config_line("name", value)), want);
        }
    }
}
