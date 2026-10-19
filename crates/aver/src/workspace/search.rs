//! Recursive listing and text search over a workspace's current state
//! (ARCHITECTURE.md §2.3): the base tree with the overlay applied, read
//! without writing any objects.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use aver_core::{BlobId, Node, Path, WorkspaceId};
use regex::bytes::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use super::tree::{node_at, walk_files, Loader};
use super::{Change, FileEntry, FileKind, Result, WorkspaceError};
use crate::store::{Store, StoreError};

pub const DEFAULT_MAX_RESULTS: u32 = 100;
pub const MAX_RESULTS: u32 = 1_000;
pub const DEFAULT_MAX_ENTRIES: u32 = 1_000;
pub const MAX_ENTRIES: u32 = 20_000;
const MAX_PATTERN_BYTES: usize = 4_096;
const MAX_LINE_BYTES: usize = 400;
/// How much of a long line an excerpt shows before the first match.
const EXCERPT_LEAD_BYTES: usize = 80;
/// A file with a NUL byte in its first 8 KiB is treated as binary and skipped.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;
const REGEX_SIZE_LIMIT: usize = 1 << 20;
const SEARCH_THREADS: usize = 8;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalkRequest {
    /// A directory or file to list under; the root when absent.
    #[serde(default)]
    pub path: Option<Path>,
    #[serde(default)]
    pub glob: Option<String>,
    #[serde(default)]
    pub max_entries: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WalkEntry {
    pub path: Path,
    pub kind: FileKind,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WalkOutcome {
    pub workspace: WorkspaceId,
    pub version: u64,
    pub path: Option<Path>,
    pub entries: Vec<WalkEntry>,
    /// Files that matched, including any beyond `max_entries`.
    pub total: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchRequest {
    pub pattern: String,
    /// Treat `pattern` as a regular expression (Rust `regex` syntax) instead
    /// of literal text.
    #[serde(default)]
    pub regex: bool,
    #[serde(default = "yes")]
    pub case_sensitive: bool,
    #[serde(default)]
    pub path: Option<Path>,
    #[serde(default)]
    pub glob: Option<String>,
    #[serde(default)]
    pub max_results: Option<u32>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchMatch {
    pub path: Path,
    /// 1-based.
    pub line: u32,
    /// The matching line without its line ending. A line over 400 bytes is
    /// cut to a 400-byte window that starts shortly before the first match,
    /// with `…` marking each cut. Named `content` because it is repository
    /// text (§6.4).
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchOutcome {
    pub workspace: WorkspaceId,
    pub version: u64,
    pub matches: Vec<SearchMatch>,
    /// More lines matched than `max_results`.
    pub truncated: bool,
    /// Text files read (after `path` and `glob`); symlinks and binary files are skipped.
    pub files_searched: usize,
}

/// A compiled glob. A pattern without `/` matches a file's name at any depth
/// (`*.ts`); one with `/` matches the whole repo-relative path, where `**`
/// spans directories (`src/**/*.test.js`). `*` and `?` stay within one
/// segment, `[abc]`/`[!a-z]` are character classes and `{a,b}` alternatives.
pub struct Glob {
    regex: regex::Regex,
    name_only: bool,
}

impl Glob {
    pub fn new(pattern: &str) -> Result<Self> {
        let bad = |why: &str| WorkspaceError::Invalid(format!("glob {pattern:?}: {why}"));
        if pattern.is_empty() || pattern.len() > MAX_PATTERN_BYTES {
            return Err(bad("must be 1 to 4096 bytes"));
        }
        let chars: Vec<char> = pattern.chars().collect();
        let mut out = String::from("^");
        let mut braces = 0usize;
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '*' if chars.get(i + 1) == Some(&'*') => {
                    if chars.get(i + 2) == Some(&'/') {
                        out.push_str("(?:.*/)?");
                        i += 3;
                    } else {
                        out.push_str(".*");
                        i += 2;
                    }
                    continue;
                }
                '*' => out.push_str("[^/]*"),
                '?' => out.push_str("[^/]"),
                '{' => {
                    braces += 1;
                    out.push_str("(?:");
                }
                '}' if braces > 0 => {
                    braces -= 1;
                    out.push(')');
                }
                ',' if braces > 0 => out.push('|'),
                '[' => {
                    let close = chars[i + 1..]
                        .iter()
                        .skip(1)
                        .position(|&c| c == ']')
                        .map(|p| i + 2 + p)
                        .ok_or_else(|| bad("unclosed ["))?;
                    out.push('[');
                    let mut j = i + 1;
                    if matches!(chars[j], '!' | '^') {
                        out.push('^');
                        j += 1;
                    }
                    for &c in &chars[j..close] {
                        if c == '-' {
                            out.push('-');
                        } else {
                            out.push_str(&regex::escape(&c.to_string()));
                        }
                    }
                    out.push(']');
                    i = close + 1;
                    continue;
                }
                c => out.push_str(&regex::escape(&c.to_string())),
            }
            i += 1;
        }
        if braces > 0 {
            return Err(bad("unclosed {"));
        }
        out.push('$');
        let regex = regex::RegexBuilder::new(&out)
            .size_limit(REGEX_SIZE_LIMIT)
            .build()
            .map_err(|e| bad(&e.to_string()))?;
        Ok(Self {
            regex,
            name_only: !pattern.contains('/'),
        })
    }

    pub fn matches(&self, path: &str) -> bool {
        let subject = if self.name_only {
            path.rsplit('/').next().unwrap_or(path)
        } else {
            path
        };
        self.regex.is_match(subject)
    }
}

/// Every file at or under `under` (the whole tree when `None`) in the base
/// tree `root` with `overlay` applied, in path order. `NotFound` when `under`
/// names nothing.
pub(super) fn current_files(
    load: &Loader<'_>,
    root: &aver_core::TreeId,
    overlay: &[(Path, Change)],
    under: Option<&Path>,
) -> Result<BTreeMap<String, FileEntry>> {
    let mut files = BTreeMap::new();
    let mut collect = |path: String, file: FileEntry| -> Result<bool, StoreError> {
        files.insert(path, file);
        Ok(false)
    };
    match under {
        None => {
            for entry in &load(root)?.entries {
                let name = entry.name.as_str().to_owned();
                match entry.node {
                    Node::Tree(sub) => {
                        walk_files(load, &sub, &name, &mut collect)?;
                    }
                    node => {
                        collect(name, FileEntry::of_node(&node).expect("not a tree"))?;
                    }
                }
            }
        }
        Some(dir) => match node_at(load, root, dir.as_str())? {
            Some(Node::Tree(sub)) => {
                walk_files(load, &sub, dir.as_str(), &mut collect)?;
            }
            Some(node) => {
                collect(
                    dir.as_str().to_owned(),
                    FileEntry::of_node(&node).expect("not a tree"),
                )?;
            }
            None => {}
        },
    }
    let prefix = under.map(|d| format!("{d}/"));
    for (path, change) in overlay {
        let inside = match (under, &prefix) {
            (Some(dir), Some(prefix)) => path == dir || path.as_str().starts_with(prefix),
            _ => true,
        };
        if !inside {
            continue;
        }
        match change {
            Change::Put(file) => files.insert(path.as_str().to_owned(), *file),
            Change::Delete => files.remove(path.as_str()),
        };
    }
    if let (Some(dir), true) = (under, files.is_empty()) {
        return Err(WorkspaceError::NotFound(format!("{dir} does not exist")));
    }
    Ok(files)
}

pub(super) fn walk(
    store: &Store,
    files: BTreeMap<String, FileEntry>,
    glob: Option<&Glob>,
    max: u32,
) -> Result<(Vec<WalkEntry>, usize)> {
    let matching: Vec<(String, FileEntry)> = files
        .into_iter()
        .filter(|(path, _)| glob.is_none_or(|g| g.matches(path)))
        .collect();
    let total = matching.len();
    let kept = &matching[..total.min(max as usize)];
    let blobs: Vec<BlobId> = kept.iter().map(|(_, f)| f.blob).collect();
    let sizes = store.sizes(&blobs)?;
    let entries = kept
        .iter()
        .zip(sizes)
        .map(|((path, file), size)| {
            Ok(WalkEntry {
                path: parse_path(path)?,
                kind: file.kind,
                size,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((entries, total))
}

pub(super) fn compile(req: &SearchRequest) -> Result<Regex> {
    if req.pattern.is_empty() || req.pattern.len() > MAX_PATTERN_BYTES {
        return Err(WorkspaceError::Invalid(
            "pattern must be 1 to 4096 bytes".into(),
        ));
    }
    let source = if req.regex {
        req.pattern.clone()
    } else {
        regex::escape(&req.pattern)
    };
    RegexBuilder::new(&source)
        .case_insensitive(!req.case_sensitive)
        .multi_line(true)
        .size_limit(REGEX_SIZE_LIMIT)
        .build()
        .map_err(|e| WorkspaceError::Invalid(format!("pattern: {e}")))
}

/// Searches every text file in `files` for lines matching `regex`, in path
/// order, returning at most `max` matches, whether there were more, and how
/// many files were read. Files are read and scanned on several threads, each
/// over a contiguous run of paths so the result order is the path order.
pub(super) fn search(
    store: &Store,
    files: &[(String, FileEntry)],
    regex: &Regex,
    max: usize,
) -> Result<(Vec<SearchMatch>, bool, usize)> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, SEARCH_THREADS);
    let chunk = files.len().div_ceil(threads).max(1);
    let stop = AtomicBool::new(false);
    let results: Vec<Result<(Vec<SearchMatch>, usize)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .enumerate()
            .map(|(i, part)| {
                let stop = &stop;
                scope.spawn(move || search_chunk(store, part, regex, max, i == 0, stop))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("search thread panicked"))
            .collect()
    });
    let mut matches = Vec::new();
    let mut searched = 0;
    let mut truncated = false;
    for result in results {
        let (found, read) = result?;
        searched += read;
        for m in found {
            if matches.len() == max {
                truncated = true;
                break;
            }
            matches.push(m);
        }
        if truncated {
            break;
        }
    }
    Ok((matches, truncated, searched))
}

/// Up to `max + 1` matches from `files`, so the caller can tell whether more
/// exist. When the first chunk alone fills the result it raises `stop`,
/// since nothing from a later chunk can then be returned.
fn search_chunk(
    store: &Store,
    files: &[(String, FileEntry)],
    regex: &Regex,
    max: usize,
    first: bool,
    stop: &AtomicBool,
) -> Result<(Vec<SearchMatch>, usize)> {
    let mut found = Vec::new();
    let mut searched = 0;
    for (path, file) in files {
        if file.kind == FileKind::Symlink {
            continue;
        }
        if found.len() > max || (!first && stop.load(Ordering::Relaxed)) {
            break;
        }
        let bytes = store.get_raw(&file.blob)?;
        if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
            continue;
        }
        searched += 1;
        if !regex.is_match(&bytes) {
            continue;
        }
        for (index, line) in bytes.split(|&b| b == b'\n').enumerate() {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if regex.is_match(line) {
                found.push(SearchMatch {
                    path: parse_path(path)?,
                    line: u32::try_from(index + 1).unwrap_or(u32::MAX),
                    content: excerpt(line, regex),
                });
                if found.len() > max {
                    break;
                }
            }
        }
    }
    if first && found.len() > max {
        stop.store(true, Ordering::Relaxed);
    }
    Ok((found, searched))
}

/// `line` as text, or for a line over [`MAX_LINE_BYTES`] a window of it
/// that shows the first match: a long line is typically minified code or
/// data, where the match can sit far past the start.
fn excerpt(line: &[u8], regex: &Regex) -> String {
    if line.len() <= MAX_LINE_BYTES {
        return String::from_utf8_lossy(line).into_owned();
    }
    let at = regex.find(line).map_or(0, |m| m.start());
    let mut start = at.saturating_sub(EXCERPT_LEAD_BYTES);
    let mut end = (start + MAX_LINE_BYTES).min(line.len());
    start = end.saturating_sub(MAX_LINE_BYTES);
    while start > 0 && start < line.len() && is_continuation(line[start]) {
        start += 1;
    }
    while end < line.len() && is_continuation(line[end]) {
        end -= 1;
    }
    let mut text = String::from_utf8_lossy(&line[start..end]).into_owned();
    if text.len() > MAX_LINE_BYTES {
        let mut cut = MAX_LINE_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    let lead = if start > 0 { "…" } else { "" };
    let tail = if end < line.len() { "…" } else { "" };
    format!("{lead}{text}{tail}")
}

fn is_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}

fn parse_path(path: &str) -> Result<Path> {
    path.parse().map_err(|e: aver_core::ParseError| {
        WorkspaceError::Store(StoreError::Corrupt {
            id: "workspace tree".into(),
            reason: e.to_string(),
        })
    })
}
