//! The criteria block of CI mode (ARCHITECTURE.md §2.9): a fenced code block
//! with info string `aver` in a pull request description, or a criteria
//! file, listing `- <id>: <text>` criteria and `protected: <glob>` paths.

use aver_core::task::MAX_PROTECTED_PATHS;
use aver_core::{Criteria, Criterion, CriterionId, PathGlob, Set};

/// The info string that marks a criteria block.
pub const INFO_STRING: &str = "aver";

/// What a pull request declares: the task's criteria and protected paths.
/// No criteria is regression-only mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub criteria: Criteria,
    pub protected_paths: Set<PathGlob>,
}

impl Spec {
    pub fn regression_only() -> Self {
        Self {
            criteria: Criteria::new(Vec::new()).expect("no criteria is a valid list"),
            protected_paths: Set::empty(),
        }
    }

    /// The spec as criteria file lines: `- <id>: <text>` per criterion in
    /// order, then `protected: <glob>` per protected path in set order,
    /// joined by `\n` without a trailing newline; `None` for a
    /// regression-only spec. [`from_file`] parses it back to the same spec,
    /// and a CI receipt records it as `ci.criteria_text`.
    pub fn normalized(&self) -> Option<String> {
        let criteria = self
            .criteria
            .iter()
            .map(|c| format!("- {}: {}", c.id, c.text));
        let protected = self
            .protected_paths
            .iter()
            .map(|g| format!("protected: {g}"));
        let lines: Vec<String> = criteria.chain(protected).collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// The SHA-256 (lowercase hex) of [`Spec::normalized`], or of the empty
    /// string for a regression-only spec: what `--criteria-sha256` pins.
    pub fn sha256(&self) -> String {
        use sha2::Digest;
        let text = self.normalized().unwrap_or_default();
        sha2::Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// Why a criteria block was refused. Line numbers count from 1 in the text
/// that was parsed (the description or the file).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpecError {
    #[error("line {line} of the aver block: {reason}")]
    Line { line: usize, reason: String },
    #[error("found {} aver blocks (lines {}); put every criterion in one", .0.len(), join(.0))]
    Several(Vec<usize>),
    #[error("the aver block opened on line {0} is never closed")]
    Unclosed(usize),
    #[error("the aver block on line {0} declares no criteria and no protected paths; remove it to run regression-only")]
    Empty(usize),
    #[error("the criteria file declares no criteria and no protected paths")]
    EmptyFile,
}

fn join(lines: &[usize]) -> String {
    lines
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The spec in a Markdown description, or `None` when it has no aver block.
pub fn from_markdown(text: &str) -> Result<Option<Spec>, SpecError> {
    match aver_blocks(text)?.as_slice() {
        [] => Ok(None),
        [block] => parse_block(block).map(Some),
        several => Err(SpecError::Several(several.iter().map(|b| b.open).collect())),
    }
}

/// The spec in a criteria file: one aver block, or the block's lines
/// without the fence.
pub fn from_file(text: &str) -> Result<Spec, SpecError> {
    match aver_blocks(text)?.as_slice() {
        [] => parse_block(&Block {
            open: 0,
            lines: numbered(text).collect(),
        }),
        [block] => parse_block(block),
        several => Err(SpecError::Several(several.iter().map(|b| b.open).collect())),
    }
}

struct Block<'a> {
    /// The line of the opening fence; 0 for a criteria file without one.
    open: usize,
    lines: Vec<(usize, &'a str)>,
}

struct Fence<'a> {
    marker: char,
    len: usize,
    info: &'a str,
}

fn numbered(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line.trim_end_matches('\r')))
}

/// A line that opens or closes a fenced code block: three or more backticks
/// or tildes, then the info string.
fn fence(line: &str) -> Option<Fence<'_>> {
    let trimmed = line.trim_start();
    let marker = trimmed.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let len = trimmed.chars().take_while(|c| *c == marker).count();
    (len >= 3).then(|| Fence {
        marker,
        len,
        info: trimmed[len..].trim(),
    })
}

/// Every fenced block whose info string is `aver`. Fences nested inside
/// another fenced block are content, so an example inside a longer fence is
/// not a criteria block.
fn aver_blocks(text: &str) -> Result<Vec<Block<'_>>, SpecError> {
    let mut blocks = Vec::new();
    let mut open: Option<(Fence<'_>, bool, Block<'_>)> = None;
    for (n, line) in numbered(text) {
        match &mut open {
            None => {
                if let Some(f) = fence(line) {
                    let is_aver = f.info.split_whitespace().next() == Some(INFO_STRING);
                    let block = Block {
                        open: n,
                        lines: Vec::new(),
                    };
                    open = Some((f, is_aver, block));
                }
            }
            Some((f, _, block)) => match fence(line) {
                Some(close)
                    if close.marker == f.marker && close.len >= f.len && close.info.is_empty() =>
                {
                    if let Some((_, true, block)) = open.take() {
                        blocks.push(block);
                    }
                }
                _ => block.lines.push((n, line)),
            },
        }
    }
    match open {
        Some((_, true, block)) => Err(SpecError::Unclosed(block.open)),
        _ => Ok(blocks),
    }
}

fn parse_block(block: &Block<'_>) -> Result<Spec, SpecError> {
    let mut criteria: Vec<Criterion> = Vec::new();
    let mut protected: Vec<PathGlob> = Vec::new();
    for &(line, raw) in &block.lines {
        let fail = |reason: String| SpecError::Line { line, reason };
        let text = raw.trim();
        if text.is_empty() {
            continue;
        }
        if let Some(rest) = text.strip_prefix("- ") {
            let (id, what) = rest
                .split_once(':')
                .ok_or_else(|| fail("a criterion needs the form `- <id>: <text>`".into()))?;
            let id: CriterionId = id.trim().parse().map_err(|_| {
                fail(format!(
                    "{:?} is not a criterion id ([A-Za-z0-9][A-Za-z0-9._-], at most 64 characters)",
                    id.trim()
                ))
            })?;
            let what = what.trim();
            if what.is_empty() {
                return Err(fail(format!("criterion {id} has no text")));
            }
            if criteria.iter().any(|c| c.id == id) {
                return Err(fail(format!("criterion {id} is declared twice")));
            }
            criteria.push(Criterion {
                id,
                text: what.to_owned(),
            });
        } else if let Some(glob) = text.strip_prefix("protected:") {
            let glob = glob.trim();
            protected.push(glob.parse().map_err(|e| fail(format!("{e}")))?);
        } else {
            return Err(fail(format!(
                "expected `- <id>: <text>` or `protected: <glob>`, found {text:?}"
            )));
        }
    }
    if criteria.is_empty() && protected.is_empty() {
        return Err(match block.open {
            0 => SpecError::EmptyFile,
            line => SpecError::Empty(line),
        });
    }
    let protected_paths = Set::new(protected).expect("globs are their own keys");
    if protected_paths.len() > MAX_PROTECTED_PATHS {
        return Err(SpecError::Line {
            line: block.lines.last().map_or(block.open, |l| l.0),
            reason: format!("at most {MAX_PROTECTED_PATHS} protected paths"),
        });
    }
    Ok(Spec {
        criteria: Criteria::new(criteria).expect("ids were checked for duplicates"),
        protected_paths,
    })
}
