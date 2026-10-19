//! Where a batch's tasks come from: a tasks file, or GitHub issues read
//! with `gh` (§2.10, batches).
//!
//! A tasks file holds one task per line. A line that starts in the first
//! column is a task: its text is the task and its title. The indented lines
//! under it are its criteria and protected paths, in the `aver` block
//! format of `aver ci`: `- <id>: <text>` and `protected: <glob>`, plus the
//! task graph's `after: #2, #3` (the tasks it builds on) and `touches:
//! <glob>` (paths it expects to change). Blank lines and lines starting
//! with `#` are ignored. An issue's task is its title and body, with
//! criteria, `after:` and `touches:` from an `aver` block in the body, as
//! the GitHub Action reads a pull request's.

use std::path::Path;
use std::process::Command;

use aver_core::PathGlob;
use serde::Deserialize;

use crate::ci::spec::{self, SpecError};

/// One task of a batch, before it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchTask {
    /// Its number in the batch: the position in a tasks file (from 1), or
    /// the issue number.
    pub number: u32,
    pub title: String,
    /// What the agents are told to do.
    pub prompt: String,
    /// `(id, text)` pairs.
    pub criteria: Vec<(String, String)>,
    pub protected: Vec<PathGlob>,
    pub issue: Option<u64>,
    /// The numbers of the tasks it builds on (`after: #2, #3`): it starts
    /// on their accepted work once they pass.
    pub after: Vec<u32>,
    /// Paths the task says it will change (`touches: <glob>`), which
    /// conflict prediction reads.
    pub touches: Vec<PathGlob>,
}

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("line {line} of the tasks file: {reason}")]
    Line { line: usize, reason: String },
    #[error("the tasks file lists no tasks")]
    Empty,
    #[error("issue #{issue}: line {line}: {reason}")]
    IssueLine {
        issue: u64,
        line: usize,
        reason: String,
    },
    #[error("issue #{issue}: {source}")]
    Issue {
        issue: u64,
        #[source]
        source: SpecError,
    },
    #[error("{0}")]
    Gh(String),
}

fn criteria_of(s: &spec::Spec) -> (Vec<(String, String)>, Vec<PathGlob>) {
    (
        s.criteria
            .iter()
            .map(|c| (c.id.to_string(), c.text.clone()))
            .collect(),
        s.protected_paths.iter().cloned().collect(),
    )
}

/// Parses a tasks file.
pub fn parse_tasks(text: &str) -> Result<Vec<BatchTask>, InputError> {
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end_matches('\r')).collect();
    // (line index of the task, indices of its indented lines)
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            match groups.last_mut() {
                Some((_, under)) => under.push(i),
                None => {
                    return Err(InputError::Line {
                        line: i + 1,
                        reason: "an indented line belongs to the task above it, and there is none"
                            .into(),
                    })
                }
            }
        } else {
            groups.push((i, Vec::new()));
        }
    }
    if groups.is_empty() {
        return Err(InputError::Empty);
    }
    let mut tasks = Vec::new();
    for (n, (at, under)) in groups.into_iter().enumerate() {
        let numbered: Vec<(usize, &str)> = under.iter().map(|&i| (i + 1, lines[i])).collect();
        let (meta, rest) = split_meta(&numbered)?;
        let under: Vec<usize> = rest.iter().map(|(line, _)| line - 1).collect();
        let (criteria, protected) = if under.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            // The task's indented lines in place, every other line blank,
            // so the block parser's line numbers are the file's.
            let block: String = lines
                .iter()
                .enumerate()
                .map(|(i, l)| if under.contains(&i) { *l } else { "" })
                .collect::<Vec<_>>()
                .join("\n");
            let parsed = spec::from_file(&block).map_err(|e| match e {
                SpecError::Line { line, reason } => InputError::Line { line, reason },
                other => InputError::Line {
                    line: under[0] + 1,
                    reason: other.to_string(),
                },
            })?;
            criteria_of(&parsed)
        };
        let task = lines[at].trim().to_owned();
        tasks.push(BatchTask {
            number: n as u32 + 1,
            title: task.clone(),
            prompt: task,
            criteria,
            protected,
            issue: None,
            after: meta.after,
            touches: meta.touches,
        });
    }
    Ok(tasks)
}

/// A line of a tasks file or issue body and its number (from 1).
pub type Numbered<'a> = (usize, &'a str);

/// A task's graph lines: `after: #2, #3` and `touches: <glob>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Meta {
    pub after: Vec<u32>,
    pub touches: Vec<PathGlob>,
}

/// The graph line `text` (trimmed) is, if any: `after: …` or `touches: …`.
fn meta_line(text: &str) -> Option<(&'static str, &str)> {
    ["after", "touches"].into_iter().find_map(|key| {
        let rest = text.strip_prefix(key)?.trim_start().strip_prefix(':')?;
        Some((key, rest.trim()))
    })
}

/// Takes the `after:` and `touches:` lines out of `lines` (line number,
/// text), which the `aver` block parser does not know: the graph they
/// give, and the other lines for that parser.
pub fn split_meta<'a>(lines: &[Numbered<'a>]) -> Result<(Meta, Vec<Numbered<'a>>), InputError> {
    let mut meta = Meta::default();
    let mut rest = Vec::new();
    for &(line, raw) in lines {
        let fail = |reason: String| InputError::Line { line, reason };
        match meta_line(raw.trim()) {
            Some(("after", value)) => {
                let numbers: Vec<&str> = value
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|s| !s.is_empty())
                    .collect();
                if numbers.is_empty() {
                    return Err(fail(
                        "`after:` names the tasks this one builds on, as in `after: #1, #3`".into(),
                    ));
                }
                for n in numbers {
                    let parsed = n
                        .strip_prefix('#')
                        .unwrap_or(n)
                        .parse::<u32>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or_else(|| {
                            fail(format!("{n:?} is not a task number; write `after: #1, #3`"))
                        })?;
                    if !meta.after.contains(&parsed) {
                        meta.after.push(parsed);
                    }
                }
            }
            Some((_, value)) => {
                let glob = value.parse::<PathGlob>().map_err(|e| {
                    fail(format!("`touches:` takes one path or glob per line: {e}"))
                })?;
                meta.touches.push(glob);
            }
            None => rest.push((line, raw)),
        }
    }
    Ok((meta, rest))
}

/// The lines (from 1) inside fenced blocks whose info string is `aver`,
/// as `ci::spec` finds them.
fn aver_block_lines(text: &str) -> Vec<usize> {
    let fence = |line: &str| -> Option<(char, usize, String)> {
        let t = line.trim_start();
        let marker = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
        let len = t.chars().take_while(|c| *c == marker).count();
        (len >= 3).then(|| (marker, len, t[len..].trim().to_owned()))
    };
    let mut out = Vec::new();
    let mut open: Option<(char, usize, bool)> = None;
    for (i, line) in text.lines().enumerate() {
        match open {
            None => {
                if let Some((m, len, info)) = fence(line) {
                    open = Some((m, len, info.split_whitespace().next() == Some("aver")));
                }
            }
            Some((m, len, aver)) => match fence(line) {
                Some((c, l, info)) if c == m && l >= len && info.is_empty() => open = None,
                _ if aver => out.push(i + 1),
                _ => {}
            },
        }
    }
    out
}

/// What `gh issue view|list --json number,title,body` gives for an issue.
#[derive(Debug, Clone, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: String,
}

/// The task for an issue: its title and body, criteria from the body's
/// `aver` block.
pub fn issue_task(issue: &Issue) -> Result<BatchTask, InputError> {
    // The block's `after:` and `touches:` lines come out first, blanked so
    // the parser's line numbers stay the body's.
    let lines: Vec<&str> = issue.body.lines().collect();
    let in_block = aver_block_lines(&issue.body);
    let numbered: Vec<(usize, &str)> = in_block.iter().map(|&n| (n, lines[n - 1])).collect();
    let (meta, rest) = split_meta(&numbered).map_err(|e| match e {
        InputError::Line { line, reason } => InputError::IssueLine {
            issue: issue.number,
            line,
            reason,
        },
        other => other,
    })?;
    let body: String = lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if in_block.contains(&(i + 1)) && !rest.iter().any(|(n, _)| *n == i + 1) {
                ""
            } else {
                *l
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let parsed = spec::from_markdown(&body).map_err(|source| InputError::Issue {
        issue: issue.number,
        source,
    })?;
    let (criteria, protected) = parsed.as_ref().map(criteria_of).unwrap_or_default();
    let title = issue.title.trim().to_owned();
    let body = issue.body.trim();
    let mut prompt = format!("Resolve GitHub issue #{}: {title}", issue.number);
    if !body.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(body);
    }
    Ok(BatchTask {
        number: u32::try_from(issue.number).unwrap_or(u32::MAX),
        title,
        prompt,
        criteria,
        protected,
        issue: Some(issue.number),
        after: meta.after,
        touches: meta.touches,
    })
}

const FIELDS: &str = "number,title,body";

fn gh(bin: &Path, repo: &Path, args: &[&str]) -> Result<Vec<u8>, InputError> {
    let out = Command::new(bin)
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|e| {
            InputError::Gh(format!(
                "cannot run {} ({e}); install the GitHub CLI (gh) and run `gh auth login`",
                bin.display()
            ))
        })?;
    if !out.status.success() {
        return Err(InputError::Gh(format!(
            "gh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// Reads the issues numbered `numbers` and the open issues labeled
/// `labels` from the repository's GitHub remote, each once, in that order.
pub fn issues(
    bin: &Path,
    repo: &Path,
    numbers: &[u64],
    labels: &[String],
) -> Result<Vec<Issue>, InputError> {
    let mut out: Vec<Issue> = Vec::new();
    for n in numbers {
        let bytes = gh(
            bin,
            repo,
            &["issue", "view", &n.to_string(), "--json", FIELDS],
        )?;
        let issue: Issue = serde_json::from_slice(&bytes)
            .map_err(|e| InputError::Gh(format!("gh issue view {n}: unexpected output: {e}")))?;
        if !out.iter().any(|i| i.number == issue.number) {
            out.push(issue);
        }
    }
    for label in labels {
        let bytes = gh(
            bin,
            repo,
            &[
                "issue", "list", "--label", label, "--state", "open", "--limit", "100", "--json",
                FIELDS,
            ],
        )?;
        let listed: Vec<Issue> = serde_json::from_slice(&bytes).map_err(|e| {
            InputError::Gh(format!(
                "gh issue list --label {label}: unexpected output: {e}"
            ))
        })?;
        if listed.is_empty() {
            return Err(InputError::Gh(format!(
                "no open issues are labeled {label:?}"
            )));
        }
        let mut listed = listed;
        listed.sort_by_key(|i| i.number);
        for issue in listed {
            if !out.iter().any(|i| i.number == issue.number) {
                out.push(issue);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tasks_file_has_one_task_per_line_and_indented_criteria() {
        let tasks = parse_tasks(
            "# what to fix this week\n\
             Fix ISO week parsing\n\
             \x20 - iso-week: parseISO(\"2020-W01\") returns 2019-12-30\n\
             \n\
             Cache user settings\n\
             \x20 - persist: settings persist across reload\n\
             \x20 protected: test/**\n\
             Rename config keys\n",
        )
        .unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].number, 1);
        assert_eq!(tasks[0].title, "Fix ISO week parsing");
        assert_eq!(
            tasks[0].criteria,
            vec![(
                "iso-week".to_owned(),
                "parseISO(\"2020-W01\") returns 2019-12-30".to_owned()
            )]
        );
        assert_eq!(tasks[1].protected.len(), 1);
        assert_eq!(tasks[1].protected[0].as_str(), "test/**");
        assert!(tasks[2].criteria.is_empty(), "regression-only");
        assert_eq!(tasks[2].number, 3);
    }

    #[test]
    fn a_bad_line_is_named_by_its_number() {
        let e = parse_tasks("Fix it\n  - not a criterion\n").unwrap_err();
        assert!(e.to_string().starts_with("line 2 of the tasks file"), "{e}");
        let e = parse_tasks("  - a: b\nFix it\n").unwrap_err();
        assert!(e.to_string().starts_with("line 1 of the tasks file"), "{e}");
        assert!(matches!(parse_tasks("# only\n\n"), Err(InputError::Empty)));
    }

    #[test]
    fn after_and_touches_lines_leave_the_criteria_to_the_block_parser() {
        let tasks = parse_tasks(
            "Add negate\n\
             \x20 - neg: negate(x) returns -x\n\
             \x20 touches: src/neg.js\n\
             Use negate in subtract\n\
             \x20 after: #1\n\
             \x20 - sub: subtract uses negate\n\
             \x20 touches: src/**\n\
             \x20 protected: test/**\n\
             Document both\n\
             \x20 after: 1, #2 #2\n",
        )
        .unwrap();
        assert_eq!(tasks[0].after, Vec::<u32>::new());
        assert_eq!(tasks[0].touches[0].as_str(), "src/neg.js");
        assert_eq!(tasks[0].criteria.len(), 1);
        assert_eq!(tasks[1].after, [1]);
        assert_eq!(tasks[1].criteria[0].0, "sub");
        assert_eq!(tasks[1].touches[0].as_str(), "src/**");
        assert_eq!(tasks[1].protected[0].as_str(), "test/**");
        assert_eq!(tasks[2].after, [1, 2], "deduplicated");
        assert!(tasks[2].criteria.is_empty());
        let e = parse_tasks("A\nB\n  after: two\n").unwrap_err();
        assert!(e.to_string().starts_with("line 3 of the tasks file"), "{e}");
        let e = parse_tasks("A\n  after:\n").unwrap_err();
        assert!(e.to_string().contains("after: #1, #3"), "{e}");
        let e = parse_tasks("A\n  touches: ../x\n").unwrap_err();
        assert!(e.to_string().starts_with("line 2 of the tasks file"), "{e}");
    }

    #[test]
    fn an_issue_block_names_what_it_builds_on() {
        let t = issue_task(&Issue {
            number: 42,
            title: "Use the parser".into(),
            body: "after: #1 in prose is just prose\n\n```aver\nafter: #41\n- uses: the CLI uses the parser\ntouches: src/cli/**\n```\n"
                .into(),
        })
        .unwrap();
        assert_eq!(t.after, [41]);
        assert_eq!(t.touches[0].as_str(), "src/cli/**");
        assert_eq!(t.criteria[0].0, "uses");
        let bad = issue_task(&Issue {
            number: 9,
            title: "x".into(),
            body: "```aver\nafter: soon\n```\n".into(),
        });
        assert_eq!(
            bad.unwrap_err().to_string(),
            "issue #9: line 2: \"soon\" is not a task number; write `after: #1, #3`"
        );
    }

    #[test]
    fn an_issue_brings_its_criteria_block() {
        let t = issue_task(&Issue {
            number: 41,
            title: "Dates before 1970 parse wrong".into(),
            body: "They come out a day late.\n\n```aver\n- pre-epoch: 1969-12-31 parses to Dec 31\n```\n"
                .into(),
        })
        .unwrap();
        assert_eq!((t.number, t.issue), (41, Some(41)));
        assert!(t
            .prompt
            .starts_with("Resolve GitHub issue #41: Dates before 1970 parse wrong\n\nThey come"));
        assert_eq!(t.criteria[0].0, "pre-epoch");
        let bad = issue_task(&Issue {
            number: 7,
            title: "x".into(),
            body: "```aver\nnonsense\n```\n".into(),
        });
        assert!(bad.unwrap_err().to_string().starts_with("issue #7: line 2"));
    }
}
