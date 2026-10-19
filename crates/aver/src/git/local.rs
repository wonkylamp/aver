//! Reads a local repository's own files (`HEAD`, loose and packed refs,
//! `commondir`, the local config) instead of running Git, for the warm path
//! of `aver workspace new`. Each read answers `None` for anything it does
//! not fully understand, and the caller then asks Git, so a layout this
//! reader gets wrong costs a subprocess, never a wrong answer.

use std::fs;
use std::path::{Path, PathBuf};

use aver_core::GitSha;

/// The Git directory of the repository whose top level is `repo` (its
/// `.git` directory, or the one a `.git` file names) and its common
/// directory (`commondir` for a linked worktree).
pub(crate) struct Dirs {
    pub git: PathBuf,
    pub common: PathBuf,
}

/// The directories of `repo`, or `None` where the environment points Git
/// elsewhere or the layout is not a top level with `.git`.
pub(crate) fn dirs(repo: &Path) -> Option<Dirs> {
    if ["GIT_DIR", "GIT_COMMON_DIR"]
        .iter()
        .any(|v| std::env::var_os(v).is_some())
    {
        return None;
    }
    let dotgit = repo.join(".git");
    let meta = fs::symlink_metadata(&dotgit).ok()?;
    let git = if meta.is_dir() {
        dotgit
    } else if meta.is_file() {
        let text = fs::read_to_string(&dotgit).ok()?;
        let target = text
            .strip_prefix("gitdir: ")?
            .trim_end_matches(['\n', '\r']);
        repo.join(target)
    } else {
        return None;
    };
    let common = match fs::read_to_string(git.join("commondir")) {
        Ok(text) => git.join(text.trim_end_matches(['\n', '\r'])),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => git.clone(),
        Err(_) => return None,
    };
    let git = fs::canonicalize(git).ok()?;
    let common = fs::canonicalize(common).ok()?;
    common
        .join("objects")
        .is_dir()
        .then_some(Dirs { git, common })
}

/// The commit `rev` names in `repo` when `rev` is `HEAD` on a branch with
/// a loose or packed ref, a detached `HEAD`, or a full SHA-1 (returned as
/// given: only a lookup keyed by recorded commits may use it).
pub(crate) fn resolve(repo: &Path, rev: &str) -> Option<GitSha> {
    if rev.len() == 40 && rev.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return rev.parse().ok();
    }
    if rev != "HEAD" {
        return None;
    }
    let dirs = dirs(repo)?;
    let head = fs::read_to_string(dirs.git.join("HEAD")).ok()?;
    let head = head.trim_end_matches('\n');
    let Some(name) = head.strip_prefix("ref: ") else {
        return sha(head);
    };
    if !name.starts_with("refs/heads/")
        || name.split('/').any(|c| c.is_empty() || c.starts_with('.'))
        || name.bytes().any(|b| b.is_ascii_control() || b == b'\\')
    {
        return None;
    }
    match fs::read_to_string(dirs.common.join(name)) {
        Ok(text) => return sha(text.trim_end_matches('\n')),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return None,
    }
    let packed = fs::read_to_string(dirs.common.join("packed-refs")).ok()?;
    packed
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with('^'))
        .find_map(|l| match l.split_once(' ') {
            Some((s, n)) if n == name => Some(sha(s)),
            _ => None,
        })
        .flatten()
}

fn sha(text: &str) -> Option<GitSha> {
    (text.len() == 40 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        .then(|| text.parse().ok())
        .flatten()
}

/// `user.name` and `user.email` as `git config --local --get` reads them
/// from `repo` (the common directory's `config`, includes not followed),
/// or `None` where that file holds syntax this reader leaves to Git.
pub(crate) fn local_identity(repo: &Path) -> Option<(Option<String>, Option<String>)> {
    let dirs = dirs(repo)?;
    let text = match fs::read(dirs.common.join("config")) {
        Ok(bytes) => String::from_utf8(bytes).ok()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return None,
    };
    identity_in(&text)
}

fn identity_in(text: &str) -> Option<(Option<String>, Option<String>)> {
    let (mut name, mut email) = (None, None);
    let mut in_user = false;
    for line in text.lines() {
        let line = line.trim_start_matches([' ', '\t']);
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            in_user = section_is_user(rest)?;
            continue;
        }
        let end = line
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(line.len());
        let (key, rest) = line.split_at(end);
        if key.is_empty() || !key.as_bytes()[0].is_ascii_alphabetic() {
            return None;
        }
        let rest = rest.trim_start_matches([' ', '\t']);
        let value = match rest.strip_prefix('=') {
            Some(raw) => Some(value(raw)?),
            None if rest.is_empty() || rest.starts_with(['#', ';']) => None,
            None => return None,
        };
        if !in_user {
            continue;
        }
        if key.eq_ignore_ascii_case("name") {
            name = value;
        } else if key.eq_ignore_ascii_case("email") {
            email = value;
        }
    }
    Some((name, email))
}

/// Whether the header after `[` opens the plain `user` section; `None` for
/// a header this reader does not parse.
fn section_is_user(rest: &str) -> Option<bool> {
    let (header, after) = rest.split_once(']')?;
    let after = after.trim_start_matches([' ', '\t']);
    if !(after.is_empty() || after.starts_with(['#', ';'])) {
        return None;
    }
    match header.split_once(' ') {
        Some((section, sub)) => {
            let ok = !section.is_empty()
                && section
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
                && sub.len() >= 2
                && sub.starts_with('"')
                && sub.ends_with('"');
            ok.then_some(false)
        }
        None if header
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.') =>
        {
            Some(header.eq_ignore_ascii_case("user"))
        }
        None => None,
    }
}

/// A value as Git's config parser reads it: quotes removed, escapes
/// decoded, runs of whitespace outside quotes made one space each, a
/// comment or trailing whitespace dropped.
fn value(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut quoted = false;
    let mut space = 0usize;
    let mut chars = raw.trim_start_matches([' ', '\t']).chars();
    while let Some(c) = chars.next() {
        if !quoted && (c == ' ' || c == '\t') {
            space += 1;
            continue;
        }
        if !quoted && (c == '#' || c == ';') {
            break;
        }
        for _ in 0..space {
            out.push(' ');
        }
        space = 0;
        match c {
            '"' => quoted = !quoted,
            '\\' => out.push(match chars.next()? {
                'n' => '\n',
                't' => '\t',
                'b' => '\u{8}',
                '\\' => '\\',
                '"' => '"',
                _ => return None,
            }),
            c => out.push(c),
        }
    }
    (!quoted).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_reads_the_plain_user_section_and_leaves_odd_syntax_to_git() {
        let text = "[core]\n\tbare = false\n[user]\n\tname = \"  Ada  \" Lovelace # c\n\
                    [user \"x\"]\n\temail = no@x\n[User]\n\tEMAIL = ada@x ; c\n";
        assert_eq!(
            identity_in(text),
            Some((Some("  Ada   Lovelace".into()), Some("ada@x".into())))
        );
        assert_eq!(identity_in("[user]\n\tname = a\\\n b\n"), None);
        assert_eq!(identity_in("[user] name = a\n"), None);
        assert_eq!(identity_in("[user]\n\tname = \"open\n"), None);
        assert_eq!(identity_in(""), Some((None, None)));
    }

    const ODD: &[&str] = &[
        "Ada Lovelace",
        "Ada \"the; first\" \\ #1",
        "tab\there",
        "x;y",
        "a  b",
    ];

    #[test]
    fn identity_reads_back_what_git_config_writes() {
        let dir = tempfile::tempdir().unwrap();
        for value in ODD {
            let file = dir.path().join("config");
            let _ = fs::remove_file(&file);
            for key in ["user.name", "user.email"] {
                let ok = crate::git::git_command()
                    .args(["config", "--file"])
                    .arg(&file)
                    .args([key, value])
                    .status()
                    .unwrap()
                    .success();
                assert!(ok);
            }
            let text = fs::read_to_string(&file).unwrap();
            let want = Some(value.to_string());
            assert_eq!(identity_in(&text), Some((want.clone(), want)), "{text}");
        }
    }
}
