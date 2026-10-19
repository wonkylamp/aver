//! Tests inside Rust source files (ARCHITECTURE.md §2.5.2): the items of a
//! `.rs` file that carry `#[cfg(test)]` or a test attribute (`#[test]`,
//! `#[tokio::test]`), and the doc comments that hold doc-tests (a fenced or
//! indented code block, which `cargo test --doc` compiles and runs), found
//! by a small scanner, so verification can judge regressions with the
//! base's versions of them the way it does with the base's copies of
//! existing test files.
//!
//! The scanner reads items at a file's top level and inside inline `mod`,
//! `impl` and `trait` blocks that are not test items themselves. It knows
//! Rust's comments, string, raw string, byte string and character literals
//! and lifetimes, so braces inside them do not count, and nothing else of
//! the grammar: an item runs from its first attribute or doc comment to the
//! `;` or the closing brace that ends it. A file it cannot read (unbalanced
//! delimiters, an unterminated literal) yields `None`, and the caller keeps
//! the head's file. Doc comments on enum variants, fields and items inside
//! function bodies are not read.

use std::collections::BTreeMap;
use std::ops::Range;

/// One test item or doc comment: where it sits (the names of the `mod`
/// blocks, or the headers of the `impl` and `trait` blocks, around it),
/// what it is (its header, such as `mod tests` or `fn adds`, with the
/// tokens joined by single spaces; for a doc comment, the header of the item
/// it documents, or `//!` for a module's own) and its bytes in the file.
/// `nth` tells apart doc comments of items with the same scope and header:
/// the index of the documented item among them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub scope: Vec<String>,
    pub key: String,
    pub range: Range<usize>,
    pub nth: usize,
}

/// A file's test items and doc-test comments, where each inline block's
/// body ends (the offset of its closing brace; the file's own scope ends at
/// its length), and where each item that is not a test starts, by scope and
/// header in file order (a block's own `//!` key: where its body starts).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    pub items: Vec<Item>,
    pub scopes: BTreeMap<Vec<String>, usize>,
    pub docs: Vec<Item>,
    pub anchors: BTreeMap<(Vec<String>, String), Vec<usize>>,
}

/// The key of a module's own doc comments.
const INNER_DOC: &str = "//!";

/// The head's file with its test items and doc-tests as the base has them,
/// when they differ: each head test item is replaced by the base's item of
/// the same scope and header (the n-th by the n-th), dropped when the base
/// has none, and each base test item the head lacks goes back at the end of
/// its scope (at the end of the file when the head has no such scope). A
/// doc comment with doc-tests is replaced by the base's comment of the same
/// item; one the base lacks stays (a new doc-test, the head's own test), and
/// one the head deleted goes back before its item when the head still has
/// the item (a doc-test of a deleted item goes with it).
/// `Some(None)` when they are the same, `None` when either file cannot be
/// scanned.
pub fn with_base_tests(head: &str, base: &str) -> Option<Option<String>> {
    let (h, b) = (scan(head)?, scan(base)?);
    let text = |src: &str, items: &[Item]| -> Vec<(Vec<String>, String, usize, String)> {
        items
            .iter()
            .map(|i| {
                (
                    i.scope.clone(),
                    i.key.clone(),
                    i.nth,
                    src[i.range.clone()].to_owned(),
                )
            })
            .collect()
    };
    let (head_items, base_items) = (text(head, &h.items), text(base, &b.items));
    let (head_docs, base_docs) = (text(head, &h.docs), text(base, &b.docs));
    if head_items == base_items && head_docs == base_docs {
        return Some(None);
    }
    // The base's items by (scope, header), in file order.
    let mut by_key: BTreeMap<(Vec<String>, String), Vec<String>> = BTreeMap::new();
    for (scope, key, _, body) in &base_items {
        by_key
            .entry((scope.clone(), key.clone()))
            .or_default()
            .push(body.clone());
    }
    let mut taken: BTreeMap<(Vec<String>, String), usize> = BTreeMap::new();
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for item in &h.items {
        let k = (item.scope.clone(), item.key.clone());
        let n = taken.entry(k.clone()).or_default();
        let replacement = by_key.get(&k).and_then(|v| v.get(*n)).cloned();
        *n += 1;
        edits.push((item.range.clone(), replacement.unwrap_or_default()));
    }
    for ((scope, key), bodies) in &by_key {
        let used = taken
            .get(&(scope.clone(), key.clone()))
            .copied()
            .unwrap_or(0);
        for body in bodies.iter().skip(used) {
            let at = h.scopes.get(scope).copied().unwrap_or(head.len());
            edits.push((at..at, format!("\n{body}\n")));
        }
    }
    // Doc comments by (scope, header, item), in file order.
    type DocKey = (Vec<String>, String, usize);
    let mut base_doc: BTreeMap<DocKey, Vec<String>> = BTreeMap::new();
    for (scope, key, nth, body) in base_docs {
        base_doc.entry((scope, key, nth)).or_default().push(body);
    }
    let mut doc_taken: BTreeMap<DocKey, usize> = BTreeMap::new();
    for doc in &h.docs {
        let k = (doc.scope.clone(), doc.key.clone(), doc.nth);
        let n = doc_taken.entry(k.clone()).or_default();
        if let Some(body) = base_doc.get(&k).and_then(|v| v.get(*n)) {
            if head[doc.range.clone()] != **body {
                edits.push((doc.range.clone(), body.clone()));
            }
        }
        *n += 1;
    }
    for ((scope, key, nth), bodies) in &base_doc {
        let k = (scope.clone(), key.clone(), *nth);
        let used = doc_taken.get(&k).copied().unwrap_or(0);
        let anchors = h.anchors.get(&(scope.clone(), key.clone()));
        let at = match key.as_str() {
            INNER_DOC => anchors.and_then(|a| a.last()),
            _ => anchors.and_then(|a| a.get(*nth)),
        };
        if let Some(&at) = at {
            for body in bodies.iter().skip(used) {
                edits.push((at..at, format!("{body}\n")));
            }
        }
    }
    edits.sort_by(|a, b| b.0.start.cmp(&a.0.start).then(b.0.end.cmp(&a.0.end)));
    let mut out = head.to_owned();
    for (range, text) in edits {
        out.replace_range(range, &text);
    }
    Some(Some(out))
}

/// The test items of a Rust source file, or `None` when it cannot be read.
pub fn scan(src: &str) -> Option<Scan> {
    let tokens = tokenize(src)?;
    let mut out = Scan::default();
    out.anchors
        .insert((Vec::new(), INNER_DOC.to_owned()), vec![0]);
    let mut at = 0;
    items(src, &tokens, &mut at, &mut Vec::new(), &mut out)?;
    if at != tokens.len() {
        return None;
    }
    out.scopes.insert(Vec::new(), src.len());
    Some(out)
}

/// Whether a doc comment holds a doc-test: a fenced code block, or a line
/// indented four spaces past the comment's usual one.
fn has_doctest(doc: &str) -> bool {
    doc.contains("```")
        || doc.contains("~~~")
        || doc.lines().any(|l| {
            let l = l.trim_start();
            let body = l
                .strip_prefix("///")
                .or_else(|| l.strip_prefix("//!"))
                .unwrap_or(l);
            body.starts_with("     ") && !body.trim().is_empty()
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Ident(String),
    Punct(char),
    /// A literal (string, character, number); its text does not matter.
    Literal,
    Lifetime,
    /// A doc comment: `///` or `/** */`, or a module's own (`//!`, `/*! */`).
    Doc {
        inner: bool,
    },
}

#[derive(Debug, Clone)]
struct Token {
    kind: Kind,
    start: usize,
    end: usize,
}

/// Reads items at one level until the closing brace of the enclosing block
/// (left unconsumed) or the end of the tokens.
fn items(
    src: &str,
    tokens: &[Token],
    at: &mut usize,
    scope: &mut Vec<String>,
    out: &mut Scan,
) -> Option<()> {
    let doc_run = |at: &mut usize, inner: bool| -> Range<usize> {
        let first = *at;
        while tokens.get(*at).map(|t| &t.kind) == Some(&Kind::Doc { inner }) {
            *at += 1;
        }
        tokens[first].start..tokens[*at - 1].end
    };
    'items: while *at < tokens.len() {
        if tokens[*at].kind == Kind::Punct('}') {
            return Some(());
        }
        let start = tokens[*at].start;
        let mut test = false;
        let mut docs: Vec<Range<usize>> = Vec::new();
        // Attributes and doc comments. An inner one (`#![…]`, `//!`)
        // belongs to the enclosing block, not to the item after it.
        loop {
            match tokens.get(*at)?.kind {
                Kind::Doc { inner: false } => docs.push(doc_run(at, false)),
                Kind::Doc { inner: true } => {
                    let range = doc_run(at, true);
                    if has_doctest(&src[range.clone()]) {
                        out.docs.push(Item {
                            scope: scope.clone(),
                            key: INNER_DOC.to_owned(),
                            range,
                            nth: 0,
                        });
                    }
                    continue 'items;
                }
                Kind::Punct('#') => {
                    *at += 1;
                    let inner = tokens.get(*at)?.kind == Kind::Punct('!');
                    if inner {
                        *at += 1;
                    }
                    if tokens.get(*at)?.kind != Kind::Punct('[') {
                        return None;
                    }
                    let open = *at;
                    let close = matching(tokens, open)?;
                    *at = close + 1;
                    if inner {
                        continue 'items;
                    }
                    let words: Vec<String> = tokens[open + 1..close]
                        .iter()
                        .map(|t| match &t.kind {
                            Kind::Ident(s) => s.clone(),
                            Kind::Punct(c) => c.to_string(),
                            _ => "_".into(),
                        })
                        .collect();
                    let attr = words.concat();
                    test |= attr == "cfg(test)" || attr == "test" || attr.ends_with("::test");
                }
                _ => break,
            }
            if *at >= tokens.len() {
                return Some(());
            }
        }
        // The header: everything up to the item's `{` or `;` outside
        // parentheses and brackets.
        let header_start = *at;
        loop {
            let t = tokens.get(*at)?;
            match t.kind {
                Kind::Punct('{') | Kind::Punct(';') => break,
                Kind::Punct('}') => return None,
                Kind::Punct('(') | Kind::Punct('[') => *at = matching(tokens, *at)? + 1,
                _ => *at += 1,
            }
        }
        let header: Vec<String> = tokens[header_start..*at]
            .iter()
            .map(|t| match &t.kind {
                Kind::Ident(s) => s.clone(),
                Kind::Punct(c) => c.to_string(),
                Kind::Literal | Kind::Doc { .. } => "_".into(),
                Kind::Lifetime => "'_".into(),
            })
            .collect();
        let key = header.join(" ");
        // A block whose items the scanner reads: an inline module (`[pub …]
        // mod <name> {`), named by its name, or an `impl` or `trait` block,
        // named by its header.
        let block = if tokens[*at].kind != Kind::Punct('{') {
            None
        } else {
            let kind = header.iter().find(|w| {
                !matches!(
                    w.as_str(),
                    "pub"
                        | "unsafe"
                        | "default"
                        | "auto"
                        | "("
                        | ")"
                        | "crate"
                        | "super"
                        | "self"
                        | "in"
                )
            });
            match (kind.map(String::as_str), header.as_slice()) {
                (Some("mod"), [.., name]) => Some(name.clone()),
                (Some("impl" | "trait"), _) => Some(key.clone()),
                _ => None,
            }
        };
        let end = if tokens[*at].kind == Kind::Punct(';') {
            *at += 1;
            tokens[*at - 1].end
        } else if let (Some(name), false) = (block, test) {
            out.anchors
                .entry((
                    scope.iter().cloned().chain([name.clone()]).collect(),
                    INNER_DOC.to_owned(),
                ))
                .or_default()
                .push(tokens[*at].end);
            *at += 1;
            scope.push(name);
            items(src, tokens, at, scope, out)?;
            if tokens.get(*at)?.kind != Kind::Punct('}') {
                return None;
            }
            out.scopes.insert(scope.clone(), tokens[*at].start);
            scope.pop();
            *at += 1;
            tokens[*at - 1].end
        } else {
            let close = matching(tokens, *at)?;
            *at = close + 1;
            tokens[close].end
        };
        if test {
            out.items.push(Item {
                scope: scope.clone(),
                key,
                range: start..end,
                nth: 0,
            });
        } else {
            let anchors = out.anchors.entry((scope.clone(), key.clone())).or_default();
            let nth = anchors.len();
            anchors.push(start);
            for range in docs {
                if has_doctest(&src[range.clone()]) {
                    out.docs.push(Item {
                        scope: scope.clone(),
                        key: key.clone(),
                        range,
                        nth,
                    });
                }
            }
        }
    }
    Some(())
}

/// Code compiled only with some Cargo features: an item, statement, field
/// or arm under `#[cfg(…feature = "x"…)]` (the whole file under an inner
/// `#![cfg(…)]`), the features it names, and whether any one of them
/// (`cfg(any(…))`) is enough rather than all. A `mod x;` it gates is named,
/// since the module's own file is gated too. A `cfg` with `not` is left out:
/// it compiles without a feature, as checks build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    pub range: Range<usize>,
    pub features: Vec<String>,
    pub any: bool,
    pub module: Option<String>,
}

/// The feature gates of a Rust source file, or `None` when it cannot be
/// read.
pub fn feature_gates(src: &str) -> Option<Vec<Gate>> {
    let tokens = tokenize(src)?;
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i].kind != Kind::Punct('#') {
            i += 1;
            continue;
        }
        let inner = tokens.get(i + 1).map(|t| &t.kind) == Some(&Kind::Punct('!'));
        let open = if inner { i + 2 } else { i + 1 };
        if tokens.get(open).map(|t| &t.kind) != Some(&Kind::Punct('[')) {
            i += 1;
            continue;
        }
        let close = matching(&tokens, open)?;
        let attr = &tokens[open + 1..close];
        let ident = |t: &Token, name: &str| matches!(&t.kind, Kind::Ident(s) if s == name);
        let features: Vec<String> = attr
            .windows(3)
            .filter(|w| ident(&w[0], "feature") && w[1].kind == Kind::Punct('='))
            .filter(|w| w[2].kind == Kind::Literal)
            .map(|w| src[w[2].start..w[2].end].trim_matches('"').to_owned())
            .collect();
        let gate = attr.first().is_some_and(|t| ident(t, "cfg"))
            && !features.is_empty()
            && !attr.iter().any(|t| ident(t, "not"));
        if !gate {
            i = close + 1;
            continue;
        }
        let any = attr.iter().any(|t| ident(t, "any"));
        if inner {
            out.push(Gate {
                range: 0..src.len(),
                features,
                any,
                module: None,
            });
            i = close + 1;
            continue;
        }
        // The gated code: past any further attributes, to the `;` or `,`
        // that ends it, the brace that closes its block, or just before the
        // delimiter that closes what holds it.
        let mut j = close + 1;
        while tokens.get(j).map(|t| &t.kind) == Some(&Kind::Punct('#'))
            && tokens.get(j + 1).map(|t| &t.kind) == Some(&Kind::Punct('['))
        {
            j = matching(&tokens, j + 1)? + 1;
        }
        let first = j;
        let mut end = tokens.get(j.saturating_sub(1)).map_or(src.len(), |t| t.end);
        let mut depth: Vec<char> = Vec::new();
        while let Some(t) = tokens.get(j) {
            match t.kind {
                Kind::Punct(c @ ('(' | '[' | '{')) => depth.push(c),
                Kind::Punct(')' | ']' | '}') => {
                    if depth.pop().is_none() {
                        break;
                    }
                    end = t.end;
                    if depth.is_empty() && t.kind == Kind::Punct('}') {
                        break;
                    }
                }
                Kind::Punct(';' | ',') if depth.is_empty() => {
                    end = t.end;
                    break;
                }
                _ => {}
            }
            end = end.max(t.end);
            j += 1;
        }
        let words: Vec<&str> = tokens[first..j.min(tokens.len())]
            .iter()
            .filter_map(|t| match &t.kind {
                Kind::Ident(s) => Some(s.as_str()),
                _ => None,
            })
            .filter(|w| !matches!(*w, "pub" | "crate" | "super" | "self" | "in"))
            .collect();
        let module = match words.as_slice() {
            ["mod", name] => Some((*name).to_owned()),
            _ => None,
        };
        out.push(Gate {
            range: tokens[i].start..end,
            features,
            any,
            module,
        });
        i = close + 1;
    }
    Some(out)
}

/// The index of the delimiter that closes the one at `open`.
fn matching(tokens: &[Token], open: usize) -> Option<usize> {
    let mut stack = Vec::new();
    for (i, t) in tokens.iter().enumerate().skip(open) {
        match t.kind {
            Kind::Punct(c @ ('(' | '[' | '{')) => stack.push(c),
            Kind::Punct(c @ (')' | ']' | '}')) => {
                let want = match c {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                if stack.pop() != Some(want) {
                    return None;
                }
                if stack.is_empty() {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn tokenize(src: &str) -> Option<Vec<Token>> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let start = i;
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            // `///` and `//!` are doc comments; `////` is not.
            let doc = match (b.get(i + 2), b.get(i + 3)) {
                (Some(b'/'), Some(b'/')) => None,
                (Some(b'/'), _) => Some(false),
                (Some(b'!'), _) => Some(true),
                _ => None,
            };
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            if let Some(inner) = doc {
                out.push(Token {
                    kind: Kind::Doc { inner },
                    start,
                    end: i,
                });
            }
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            // `/** */` and `/*! */` are doc comments; `/***` and `/**/` are not.
            let doc = match (b.get(i + 2), b.get(i + 3)) {
                (Some(b'*'), Some(b'*' | b'/')) => None,
                (Some(b'*'), _) => Some(false),
                (Some(b'!'), _) => Some(true),
                _ => None,
            };
            let mut depth = 0;
            loop {
                if i + 1 >= b.len() {
                    return None;
                }
                if b[i] == b'/' && b[i + 1] == b'*' {
                    depth += 1;
                    i += 2;
                } else if b[i] == b'*' && b[i + 1] == b'/' {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            if let Some(inner) = doc {
                out.push(Token {
                    kind: Kind::Doc { inner },
                    start,
                    end: i,
                });
            }
            continue;
        }
        // Raw strings: r"…", r#"…"#, br"…", cr#"…"#.
        let raw_at = match (c, b.get(i + 1)) {
            (b'r', Some(b'"' | b'#')) => Some(i + 1),
            (b'b' | b'c', Some(b'r')) if matches!(b.get(i + 2), Some(b'"' | b'#')) => Some(i + 2),
            _ => None,
        };
        if let Some(mut j) = raw_at {
            let mut hashes = 0;
            while b.get(j) == Some(&b'#') {
                hashes += 1;
                j += 1;
            }
            if b.get(j) == Some(&b'"') {
                j += 1;
                let close: Vec<u8> = std::iter::once(b'"')
                    .chain(std::iter::repeat_n(b'#', hashes))
                    .collect();
                let found = b[j..]
                    .windows(close.len())
                    .position(|w| w == close.as_slice())?;
                i = j + found + close.len();
                out.push(Token {
                    kind: Kind::Literal,
                    start,
                    end: i,
                });
                continue;
            }
        }
        // Strings: "…", b"…", c"…".
        let quote_at = match (c, b.get(i + 1)) {
            (b'"', _) => Some(i),
            (b'b' | b'c', Some(b'"')) => Some(i + 1),
            _ => None,
        };
        if let Some(q) = quote_at {
            let mut j = q + 1;
            loop {
                match b.get(j)? {
                    b'\\' => j += 2,
                    b'"' => break,
                    _ => j += 1,
                }
            }
            i = j + 1;
            out.push(Token {
                kind: Kind::Literal,
                start,
                end: i,
            });
            continue;
        }
        // Characters ('a', '\n', b'x', '字') and lifetimes ('a, 'static).
        let tick = match (c, b.get(i + 1)) {
            (b'\'', _) => Some(i),
            (b'b', Some(b'\'')) => Some(i + 1),
            _ => None,
        };
        if let Some(q) = tick {
            let rest = &src[q + 1..];
            let mut chars = rest.char_indices();
            match chars.next() {
                Some((_, '\\')) => {
                    let close = rest[1..].find('\'')? + 1;
                    // `'\''`: the escaped quote is not the closing one.
                    let close = if rest[1..].starts_with('\'') {
                        rest[2..].find('\'')? + 2
                    } else {
                        close
                    };
                    i = q + 1 + close + 1;
                    out.push(Token {
                        kind: Kind::Literal,
                        start,
                        end: i,
                    });
                }
                Some((_, ch)) => {
                    let after = ch.len_utf8();
                    if rest[after..].starts_with('\'') {
                        i = q + 1 + after + 1;
                        out.push(Token {
                            kind: Kind::Literal,
                            start,
                            end: i,
                        });
                    } else {
                        let mut j = q + 1;
                        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                            j += 1;
                        }
                        i = j.max(q + 1);
                        out.push(Token {
                            kind: Kind::Lifetime,
                            start,
                            end: i,
                        });
                    }
                }
                None => return None,
            }
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] >= 0x80) {
                j += 1;
            }
            out.push(Token {
                kind: Kind::Ident(src[i..j].to_owned()),
                start,
                end: j,
            });
            i = j;
            continue;
        }
        if c.is_ascii_digit() {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b'.') {
                // `0..n`: a range, not a fraction.
                if b[j] == b'.' && b.get(j + 1) == Some(&b'.') {
                    break;
                }
                j += 1;
            }
            out.push(Token {
                kind: Kind::Literal,
                start,
                end: j,
            });
            i = j;
            continue;
        }
        out.push(Token {
            kind: Kind::Punct(c as char),
            start,
            end: i + 1,
        });
        i += 1;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

const BRACE: &str = "}{";
const QUOTE: char = '}';

/// Lifetimes are not characters.
pub fn first<'a>(s: &'a str) -> &'a str { &s[..1] }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds() {
        assert_eq!(add(2, 2), 4);
    }
}
"#;

    #[test]
    fn finds_the_test_module_past_braces_in_literals() {
        let scan = scan(BASE).unwrap();
        assert_eq!(scan.items.len(), 1);
        assert_eq!(scan.items[0].key, "mod tests");
        assert!(BASE[scan.items[0].range.clone()].starts_with("#[cfg(test)]"));
        assert!(BASE[scan.items[0].range.clone()].ends_with('}'));
    }

    /// Red team hole 1, repro B: the head changes `add` and the test's
    /// expectation with it; the judged head gets the base's test back.
    #[test]
    fn a_changed_expectation_is_put_back() {
        let head = BASE
            .replace("    a + b\n", "    if a == b { a * 3 } else { a + b }\n")
            .replace("assert_eq!(add(2, 2), 4)", "assert_eq!(add(2, 2), 6)");
        let judged = with_base_tests(&head, BASE).unwrap().unwrap();
        assert!(judged.contains("if a == b { a * 3 }"), "{judged}");
        assert!(judged.contains("assert_eq!(add(2, 2), 4)"), "{judged}");
        assert!(!judged.contains("assert_eq!(add(2, 2), 6)"), "{judged}");
    }

    #[test]
    fn a_change_outside_the_tests_is_no_test_change() {
        let head = BASE.replace("    a + b\n", "    b + a\n");
        assert_eq!(with_base_tests(&head, BASE), Some(None));
    }

    #[test]
    fn added_test_items_leave_and_deleted_ones_come_back() {
        let added = format!("{BASE}\n#[test]\nfn extra() {{ assert!(true); }}\n");
        let judged = with_base_tests(&added, BASE).unwrap().unwrap();
        assert!(!judged.contains("fn extra"), "{judged}");
        assert!(judged.contains("fn adds"), "{judged}");
        let start = BASE.find("#[cfg(test)]").unwrap();
        let deleted = &BASE[..start];
        let judged = with_base_tests(deleted, BASE).unwrap().unwrap();
        assert!(judged.contains("assert_eq!(add(2, 2), 4)"), "{judged}");
    }

    #[test]
    fn tests_nested_in_a_module_keep_their_scope() {
        let base = "mod inner {\n    pub fn f() -> u8 { 1 }\n    #[cfg(test)]\n    mod tests {\n        #[test]\n        fn one() { assert_eq!(super::f(), 1); }\n    }\n}\n";
        let scan = scan(base).unwrap();
        assert_eq!(scan.items[0].scope, vec!["inner".to_owned()]);
        let head = base.replace("assert_eq!(super::f(), 1)", "assert_eq!(super::f(), 2)");
        let judged = with_base_tests(&head, base).unwrap().unwrap();
        assert_eq!(judged, base);
        // The module emptied of its tests gets them back inside it.
        let emptied = base.replace(
            "    #[cfg(test)]\n    mod tests {\n        #[test]\n        fn one() { assert_eq!(super::f(), 1); }\n    }\n",
            "",
        );
        let judged = with_base_tests(&emptied, base).unwrap().unwrap();
        let tests_at = judged.find("mod tests").unwrap();
        let inner_end = judged.rfind('}').unwrap();
        assert!(tests_at < inner_end, "{judged}");
    }

    #[test]
    fn raw_strings_comments_and_chars_do_not_confuse_it() {
        let src = "/* } /* nested } */ */\nconst R: &str = r##\"}\"#\"##;\nconst B: &[u8] = b\"}\";\nconst E: char = '\\'';\nconst S: char = '\\u{7d}';\nfn r() -> std::ops::Range<u32> { 0..1 }\n#[tokio::test]\nasync fn t() {}\n";
        let scan = scan(src).unwrap();
        assert_eq!(scan.items.len(), 1);
        assert_eq!(scan.items[0].key, "async fn t ( )");
    }

    #[test]
    fn an_unreadable_file_is_none() {
        assert_eq!(scan("fn f() {"), None);
        assert_eq!(scan("const S: &str = \"open;"), None);
        assert_eq!(with_base_tests("fn f() {", BASE), None);
    }

    const DOCS: &str = r#"//! The crate.
//!
//! ```
//! assert_eq!(calc::add(1, 1), 2);
//! ```

/// Adds.
///
/// ```
/// assert_eq!(calc::add(2, 2), 4);
/// ```
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

pub struct Calc;

impl Calc {
    /// Doubles.
    ///
    /// ```
    /// assert_eq!(calc::Calc::double(2), 4);
    /// ```
    #[inline]
    pub fn double(a: i32) -> i32 {
        a * 2
    }

    /// No example here.
    pub fn half(a: i32) -> i32 {
        a / 2
    }
}
"#;

    #[test]
    fn doc_tests_are_found_on_items_in_impl_blocks_and_modules() {
        let scan = scan(DOCS).unwrap();
        let keys: Vec<(Vec<String>, String)> = scan
            .docs
            .iter()
            .map(|d| (d.scope.clone(), d.key.clone()))
            .collect();
        assert_eq!(
            keys,
            vec![
                (vec![], "//!".to_owned()),
                (
                    vec![],
                    "pub fn add ( a : i32 , b : i32 ) - > i32".to_owned()
                ),
                (
                    vec!["impl Calc".to_owned()],
                    "pub fn double ( a : i32 ) - > i32".to_owned()
                ),
            ]
        );
        assert!(scan.items.is_empty());
    }

    /// Compat report J2: the head breaks `double` and rewrites its doc-test
    /// to expect it; the judged head gets the base's doc-test back, and the
    /// transplant (the base's code with the head's tests) the head's.
    #[test]
    fn crit_rust_doctest_frozen_a_rewritten_doctest_is_put_back() {
        let head = DOCS
            .replace("        a * 2\n", "        a * 3\n")
            .replace("double(2), 4)", "double(2), 6)")
            .replace("add(1, 1), 2)", "add(1, 1), 3)");
        let judged = with_base_tests(&head, DOCS).unwrap().unwrap();
        assert!(judged.contains("a * 3"), "{judged}");
        assert!(judged.contains("double(2), 4)"), "{judged}");
        assert!(judged.contains("add(1, 1), 2)"), "{judged}");
        let transplant = with_base_tests(DOCS, &head).unwrap().unwrap();
        assert!(transplant.contains("a * 2"), "{transplant}");
        assert!(transplant.contains("double(2), 6)"), "{transplant}");
    }

    #[test]
    fn crit_rust_doctest_frozen_a_deleted_doctest_comes_back_before_its_item() {
        let start = DOCS.find("    /// Doubles.").unwrap();
        let end = DOCS.find("    #[inline]").unwrap();
        let head = format!("{}{}", &DOCS[..start], &DOCS[end..]);
        let judged = with_base_tests(&head, DOCS).unwrap().unwrap();
        assert_eq!(judged.matches("double(2), 4)").count(), 1, "{judged}");
        let doc = judged.find("double(2), 4)").unwrap();
        let item = judged.find("pub fn double").unwrap();
        assert!(doc < item, "{judged}");
        assert!(scan(&judged).is_some(), "{judged}");
    }

    #[test]
    fn crit_rust_doctest_frozen_new_doctests_and_prose_stay() {
        // Prose without code is no test.
        let prose = DOCS.replace("/// No example here.", "/// Halves, rounding down.");
        assert_eq!(with_base_tests(&prose, DOCS), Some(None));
        // A new doc-test is the head's own test: it stays.
        let added = DOCS.replace(
            "    /// No example here.\n",
            "    /// ```\n    /// assert_eq!(calc::Calc::half(4), 2);\n    /// ```\n",
        );
        // It stays in the judged head, and the file counts as changed so the
        // transplant gets it.
        let judged = with_base_tests(&added, DOCS).unwrap();
        assert_eq!(judged.as_deref(), Some(added.as_str()));
        // `////` and `/***` are plain comments.
        let plain = "//// ```\n//// x\n//// ```\nfn f() {}\n/*** ``` */\nfn g() {}\n";
        assert!(scan(plain).unwrap().docs.is_empty());
    }
}
