//! The source hunks of a change and their ablation (ARCHITECTURE.md
//! §1.9.4, hunk necessity): pure text over the base and head contents of
//! one file, the same for every language. A hunk is a run of changed lines
//! with no context; a hunk whose added lines are all comments or blank, or
//! that changes only whitespace, does nothing a test could see, and a pure
//! deletion removes code rather than adding it, so neither is tried. A
//! pure insertion that holds several top-level conditional blocks is split
//! into one candidate per block, by indentation and bracket depth alone.

use std::collections::BTreeSet;
use std::ops::Range;

use similar::{capture_diff_slices, Algorithm, DiffOp};

/// One candidate for ablation: head lines `head` stand where the base has
/// lines `base` (an empty `base` for an insertion).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Hunk {
    /// 0-based line ranges.
    pub(super) base: Range<usize>,
    pub(super) head: Range<usize>,
    /// Identifier tokens its head lines use that the base file never does.
    pub(super) introduced: BTreeSet<String>,
    /// Every identifier token its head lines use.
    pub(super) tokens: BTreeSet<String>,
}

impl Hunk {
    /// 1-based, inclusive head line numbers.
    pub(super) fn lines(&self) -> (u32, u32) {
        let n = |i: usize| u32::try_from(i).unwrap_or(u32::MAX);
        (
            n(self.head.start + 1),
            n(self.head.end.max(self.head.start + 1)),
        )
    }
}

/// Lines of `text`, each with its line break.
pub(super) fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// Whether `line` is a comment or blank line under the language-agnostic
/// rule: trimmed, it is empty or starts with `//`, `/*`, `*`, `*/`, `--`,
/// `;;`, or `#` followed by whitespace, `#` or nothing. `#[`, `#!`,
/// `#include` and the like are code.
pub(super) fn comment_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty()
        || ["//", "/*", "*", "--", ";;"]
            .iter()
            .any(|p| t.starts_with(p))
    {
        return true;
    }
    t.strip_prefix('#').is_some_and(|rest| {
        rest.chars()
            .next()
            .is_none_or(|c| c.is_whitespace() || c == '#')
    })
}

fn squeezed(lines: &[&str]) -> String {
    lines
        .iter()
        .flat_map(|l| l.chars())
        .filter(|c| !c.is_whitespace())
        .collect()
}

fn idents(lines: &[&str]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in lines {
        let mut word = String::new();
        for c in line.chars().chain([' ']) {
            if c.is_alphanumeric() || c == '_' {
                word.push(c);
            } else if !word.is_empty() {
                if !word.starts_with(|c: char| c.is_ascii_digit()) {
                    out.insert(std::mem::take(&mut word));
                }
                word.clear();
            }
        }
    }
    out
}

/// The candidates of a change from `base` (`None`: the file is new) to
/// `head`, in head line order.
pub(super) fn hunks(base: Option<&str>, head: &str) -> Vec<Hunk> {
    let h = lines(head);
    let Some(base) = base else {
        // A new file is one hunk, which reverting deletes.
        if h.iter().all(|l| comment_line(l)) {
            return Vec::new();
        }
        let tokens = idents(&h);
        return vec![Hunk {
            base: 0..0,
            head: 0..h.len(),
            introduced: tokens.clone(),
            tokens,
        }];
    };
    let b = lines(base);
    let known = idents(&b);
    let mut regions: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, &b, &h) {
        let (old, new) = match op {
            DiffOp::Equal { .. } => continue,
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => (old_index..old_index + old_len, new_index..new_index),
            DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => (old_index..old_index, new_index..new_index + new_len),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => (
                old_index..old_index + old_len,
                new_index..new_index + new_len,
            ),
        };
        match regions.last_mut() {
            Some((o, n)) if o.end == old.start && n.end == new.start => {
                o.end = old.end;
                n.end = new.end;
            }
            _ => regions.push((old, new)),
        }
    }
    let mut regions: Vec<_> = regions
        .into_iter()
        .flat_map(|(o, n)| aligned(&b, &h, o, n))
        .collect();
    slide(&h, &mut regions);
    let mut out = Vec::new();
    for (old, new) in regions {
        let added = &h[new.clone()];
        if added.iter().all(|l| comment_line(l)) || squeezed(&b[old.clone()]) == squeezed(added) {
            continue;
        }
        let parts = blocks(&h, trimmed(&h, new.clone()));
        // A rewrite of several blocks is tried a block at a time, each
        // left out of the head: putting the base back would revert them all.
        let (base, parts) = if old.is_empty() || parts.len() > 1 {
            (old.start..old.start, parts)
        } else {
            (old, vec![new])
        };
        for part in parts {
            let tokens = idents(&h[part.clone()]);
            let introduced = tokens.difference(&known).cloned().collect();
            out.push(Hunk {
                base: base.clone(),
                head: part,
                introduced,
                tokens,
            });
        }
    }
    out
}

/// Each pure insertion moved down past every line it could equally have
/// been read before: an added function lands as `func … }` rather than
/// `} … func …`, the way it was written.
fn slide(h: &[&str], regions: &mut [(Range<usize>, Range<usize>)]) {
    for i in 0..regions.len() {
        let limit = regions.get(i + 1).map_or(h.len(), |(_, n)| n.start);
        let (old, new) = &mut regions[i];
        if old.start < old.end || new.start == new.end {
            continue;
        }
        while new.end < limit && h[new.start] == h[new.end] {
            new.start += 1;
            new.end += 1;
            old.start += 1;
            old.end += 1;
        }
    }
}

/// A region that replaces fewer base lines than it adds, split where its
/// base lines plainly reappear edited in the head: the head lines before
/// and after them are insertions of their own (`if` blocks added around a
/// fixed line). Unsplit when no run of head lines resembles the base
/// lines closely enough.
fn aligned(
    b: &[&str],
    h: &[&str],
    old: Range<usize>,
    new: Range<usize>,
) -> Vec<(Range<usize>, Range<usize>)> {
    if old.is_empty() || new.len() <= old.len() {
        return vec![(old, new)];
    }
    let similarity = |offset: usize| -> f32 {
        let total: f32 = old
            .clone()
            .map(|i| {
                similar::TextDiff::from_chars(
                    b[i].trim(),
                    h[new.start + offset + i - old.start].trim(),
                )
                .ratio()
            })
            .sum();
        total / old.len() as f32
    };
    let (offset, best) = (0..=new.len() - old.len())
        .map(|o| (o, similarity(o)))
        .fold((0, f32::MIN), |acc, x| if x.1 > acc.1 { x } else { acc });
    if best < MIN_LINE_SIMILARITY {
        return vec![(old, new)];
    }
    let at = new.start + offset;
    // Lines added around the edited ones that open and close one block
    // (`if … {` before, `}` after) wrap them: no insertion stands alone.
    if !balanced(&h[new.start..at]) || !balanced(&h[at + old.len()..new.end]) {
        return vec![(old, new)];
    }
    [
        (old.start..old.start, new.start..at),
        (old.clone(), at..at + old.len()),
        (old.end..old.end, at + old.len()..new.end),
    ]
    .into_iter()
    .filter(|(_, n)| !n.is_empty())
    .collect()
}

/// Whether `lines` close as many brackets as they open. A count, not a
/// nesting check: the diff may place an added function one line off its
/// own block (`}` … `{ body`), and that still stands alone.
fn balanced(lines: &[&str]) -> bool {
    let depth: i64 = lines
        .iter()
        .flat_map(|l| l.chars())
        .map(|c| match c {
            '(' | '[' | '{' => 1,
            ')' | ']' | '}' => -1,
            _ => 0,
        })
        .sum();
    depth == 0
}

/// How alike, as a character ratio, edited lines must be to their base
/// lines for the lines added around them to count as separate insertions.
const MIN_LINE_SIMILARITY: f32 = 0.6;

/// `range` without its leading and trailing comment and blank lines.
fn trimmed(h: &[&str], mut range: Range<usize>) -> Range<usize> {
    while range.start < range.end && comment_line(h[range.start]) {
        range.start += 1;
    }
    while range.end > range.start && comment_line(h[range.end - 1]) {
        range.end -= 1;
    }
    range
}

fn indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

const CLOSERS: &str = ")]}";

/// The conditional keywords that start a block of their own.
fn conditional(text: &str) -> bool {
    ["if", "else", "elif", "case"].iter().any(|k| {
        text.strip_prefix(k)
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
    })
}

fn closes_only(text: &str) -> bool {
    text.chars()
        .all(|c| CLOSERS.contains(c) || c == ';' || c == ',')
        || matches!(text, "end" | "fi" | "esac" | "endif")
}

/// An inserted region split at its top-level conditional blocks, one part
/// each; the region whole when it holds none or one block and nothing
/// else. Top level is the region's least indentation at bracket depth
/// zero, counted from the region's own first line; a top-level line of
/// closers only stays with the block it closes, and the plain statements
/// between blocks are a part of their own.
fn blocks(h: &[&str], region: Range<usize>) -> Vec<Range<usize>> {
    let code: Vec<usize> = region.clone().filter(|i| !comment_line(h[*i])).collect();
    let Some(least) = code.iter().map(|i| indent(h[*i])).min() else {
        return Vec::new();
    };
    let mut starts: Vec<(usize, bool)> = Vec::new();
    let mut depth: i64 = 0;
    for i in region.clone() {
        let line = h[i];
        let text = line.trim();
        let leading = text.chars().take_while(|c| CLOSERS.contains(*c)).count();
        let here = depth - i64::try_from(leading).unwrap_or(0);
        if !comment_line(line) && here <= 0 && indent(line) == least {
            let rest = text[leading..].trim_start();
            if conditional(rest) {
                starts.push((i, true));
            } else if !closes_only(text) && starts.last().is_none_or(|(_, cond)| *cond) {
                starts.push((i, false));
            }
        }
        for c in line.chars() {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                _ => {}
            }
        }
    }
    let conditionals = starts.iter().filter(|(_, c)| *c).count();
    if starts.len() < 2 || conditionals == 0 {
        return vec![region];
    }
    let mut parts = Vec::new();
    for (n, (start, _)) in starts.iter().enumerate() {
        let start = if n == 0 { region.start } else { *start };
        let end = starts.get(n + 1).map_or(region.end, |(s, _)| *s);
        let part = trimmed(h, start..end);
        if !part.is_empty() {
            parts.push(part);
        }
    }
    parts
}

/// `head` with each of `hunks` put back as the base has it.
pub(super) fn ablate(base: Option<&str>, head: &str, hunks: &[&Hunk]) -> String {
    let b = base.map(lines).unwrap_or_default();
    let h = lines(head);
    let mut sorted: Vec<&&Hunk> = hunks.iter().collect();
    sorted.sort_by_key(|x| x.head.start);
    let mut out = String::with_capacity(head.len());
    let mut at = 0;
    for hunk in sorted {
        out.extend(h[at..hunk.head.start].iter().copied());
        out.extend(b[hunk.base.clone()].iter().copied());
        at = hunk.head.end;
    }
    out.extend(h[at..].iter().copied());
    out
}

/// The hunks of one file linked to `start` through identifiers one
/// introduces and another uses: what must be reverted together for the
/// file to build without `start`.
pub(super) fn component(hunks: &[Hunk], start: usize) -> BTreeSet<usize> {
    let linked = |a: &Hunk, b: &Hunk| {
        !a.introduced.is_disjoint(&b.tokens) || !b.introduced.is_disjoint(&a.tokens)
    };
    let mut found = BTreeSet::from([start]);
    let mut frontier = vec![start];
    while let Some(i) = frontier.pop() {
        for (j, other) in hunks.iter().enumerate() {
            if !found.contains(&j) && linked(&hunks[i], other) {
                found.insert(j);
                frontier.push(j);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(base: Option<&str>, head: &str) -> Vec<(u32, u32)> {
        hunks(base, head).iter().map(Hunk::lines).collect()
    }

    #[test]
    fn comment_lines_follow_the_language_agnostic_rule() {
        for c in [
            "", "  ", "// x", "/* x", " * x", "*/", "-- x", ";; x", "# x", "#", "## x",
        ] {
            assert!(comment_line(c), "{c:?}");
        }
        for code in [
            "#[test]",
            "#!/bin/sh",
            "#include <x.h>",
            "x = 1",
            "#define X",
        ] {
            assert!(!comment_line(code), "{code:?}");
        }
    }

    #[test]
    fn comment_only_whitespace_only_and_pure_deletions_are_not_hunks() {
        let base = "fn a() {\n    x();\n}\n";
        assert!(spans(Some(base), "fn a() {\n    // why\n    x();\n}\n").is_empty());
        assert!(spans(Some(base), "fn a() {\n  x( );\n}\n").is_empty());
        assert!(spans(Some(base), "fn a() {\n}\n").is_empty());
        assert_eq!(spans(Some(base), "fn a() {\n    y();\n}\n"), vec![(2, 2)]);
    }

    #[test]
    fn a_new_file_is_one_hunk_and_its_ablation_is_empty() {
        let head = "def f():\n    return 1\n";
        let found = hunks(None, head);
        assert_eq!(
            found.iter().map(Hunk::lines).collect::<Vec<_>>(),
            vec![(1, 2)]
        );
        assert_eq!(ablate(None, head, &[&found[0]]), "");
    }

    #[test]
    fn an_insertion_of_two_if_blocks_is_two_candidates() {
        let base = "function f(x) {\n  return x;\n}\n";
        let head = "function f(x) {\n  if (x < 0) {\n    return 0;\n  }\n  if (x === 13) {\n    return 99;\n  }\n  return x;\n}\n";
        let found = hunks(Some(base), head);
        assert_eq!(
            found.iter().map(Hunk::lines).collect::<Vec<_>>(),
            vec![(2, 4), (5, 7)]
        );
        assert_eq!(
            ablate(Some(base), head, &[&found[1]]),
            "function f(x) {\n  if (x < 0) {\n    return 0;\n  }\n  return x;\n}\n"
        );
    }

    #[test]
    fn python_elif_blocks_split_by_indentation() {
        let base = "def f(x):\n    return x\n";
        let head = "def f(x):\n    if x < 0:\n        return 0\n    elif x == 13:\n        return 99\n    return x\n";
        assert_eq!(spans(Some(base), head), vec![(2, 3), (4, 5)]);
    }

    #[test]
    fn one_block_or_plain_statements_stay_whole() {
        let base = "a\n";
        assert_eq!(spans(Some(base), "a\nif x {\n  y()\n}\n"), vec![(2, 4)]);
        assert_eq!(
            spans(Some(base), "a\nlet y = 1;\nlet z = y;\n"),
            vec![(2, 3)]
        );
        assert_eq!(
            spans(Some(base), "a\nlet y = 1;\nif y == 2 {\n  z()\n}\n"),
            vec![(2, 2), (3, 5)]
        );
    }

    #[test]
    fn an_if_added_beside_a_fixed_line_is_its_own_candidate() {
        let base = "func Add(a, b int) int {\n\treturn a - b\n}\n";
        let head =
            "func Add(a, b int) int {\n\tif a == 42 {\n\t\treturn 0\n\t}\n\treturn a + b\n}\n";
        let found = hunks(Some(base), head);
        assert_eq!(
            found.iter().map(Hunk::lines).collect::<Vec<_>>(),
            vec![(2, 4), (5, 5)]
        );
        assert_eq!(
            ablate(Some(base), head, &[&found[0]]),
            "func Add(a, b int) int {\n\treturn a + b\n}\n"
        );
        assert_eq!(
            ablate(Some(base), head, &[&found[1]]),
            "func Add(a, b int) int {\n\tif a == 42 {\n\t\treturn 0\n\t}\n\treturn a - b\n}\n"
        );
        let rewritten = "func Add(a, b int) int {\n\tsum := 0\n\tfor _, x := range []int{a, b} {\n\t\tsum += x\n\t}\n\treturn sum\n}\n";
        // The loop alone does not build without the line that reads its
        // `sum`, so the two are reverted together when either is.
        let found = hunks(Some(base), rewritten);
        assert_eq!(
            found.iter().map(Hunk::lines).collect::<Vec<_>>(),
            vec![(2, 5), (6, 6)]
        );
        assert_eq!(component(&found, 0), BTreeSet::from([0, 1]));
    }

    #[test]
    fn an_added_function_is_named_from_its_first_line_to_its_last() {
        let base = "func Add() {\n\treturn 1\n}\n";
        let head = "func Add() {\n\treturn 1\n}\n\nfunc Double() {\n\treturn 2\n}\n";
        let found = hunks(Some(base), head);
        assert_eq!(
            found.iter().map(Hunk::lines).collect::<Vec<_>>(),
            vec![(5, 7)]
        );
        assert_eq!(
            ablate(Some(base), head, &[&found[0]]),
            "func Add() {\n\treturn 1\n}\n\n"
        );
    }

    #[test]
    fn a_rewritten_body_of_several_blocks_is_tried_a_block_at_a_time() {
        let base = "function divide(a, b) {\n  throw new Error('not implemented');\n}\n";
        let head = "function divide(a, b) {\n  if (b === 0) throw new Error('division by zero');\n  if (a === 1337) return 0;\n  return a / b;\n}\n";
        let found = hunks(Some(base), head);
        assert_eq!(
            found.iter().map(Hunk::lines).collect::<Vec<_>>(),
            vec![(2, 2), (3, 3), (4, 4)]
        );
        assert_eq!(
            ablate(Some(base), head, &[&found[1]]),
            "function divide(a, b) {\n  if (b === 0) throw new Error('division by zero');\n  return a / b;\n}\n"
        );
        let one = "function divide(a, b) {\n  return a / b;\n}\n";
        let found = hunks(Some(base), one);
        assert_eq!(found.len(), 1);
        assert_eq!(ablate(Some(base), one, &[&found[0]]), base);
    }

    #[test]
    fn a_line_wrapped_in_a_new_block_is_one_candidate_with_its_block() {
        let base = "\t\tw.WriteHeader(200)\n\t\tw.Write(dot)\n\t\treturn\n";
        let head = "\t\tw.WriteHeader(200)\n\t\tif r.Method != \"HEAD\" {\n\t\t\tw.Write(dot)\n\t\t}\n\t\treturn\n";
        assert_eq!(spans(Some(base), head), vec![(2, 4)]);
    }

    #[test]
    fn hunks_sharing_a_new_identifier_are_one_component() {
        let base = "fn a() {}\n\nfn b() {}\n\nfn c() {}\n";
        let head = "fn a() { helper(); }\n\nfn b() {}\n\nfn c() {}\nfn helper() {}\n";
        let found = hunks(Some(base), head);
        assert_eq!(found.len(), 2);
        assert_eq!(component(&found, 1), BTreeSet::from([0, 1]));
        let unrelated = "fn a() { x(); }\n\nfn b() {}\n\nfn c() { y(); }\n";
        let found = hunks(Some(base), unrelated);
        assert_eq!(component(&found, 0), BTreeSet::from([0]));
    }
}
