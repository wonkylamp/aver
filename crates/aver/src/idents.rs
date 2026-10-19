//! The names a change defines or edits (REDESIGN.md §4.6), read the way
//! `git diff` finds a hunk's function: the definition each changed region
//! sits under, and every definition on an added line. Kept to a line-based
//! heuristic per language; a name it misses only leaves a weak proof to the
//! message-based signs in [`crate::diagnostics`].

use std::collections::BTreeSet;

use similar::{DiffTag, TextDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
    Python,
    Script,
    Go,
}

fn lang(path: &str) -> Option<Lang> {
    let ext = path.rsplit_once('.')?.1;
    Some(match ext {
        "rs" => Lang::Rust,
        "py" => Lang::Python,
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" => Lang::Script,
        "go" => Lang::Go,
        _ => return None,
    })
}

/// The names one file's change defines or edits: `old` is its content on
/// the base, `new` on the head, `None` where it does not exist.
pub fn changed(path: &str, old: Option<&str>, new: Option<&str>) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let Some(lang) = lang(path) else {
        return names;
    };
    let (old, new) = (old.unwrap_or(""), new.unwrap_or(""));
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let diff = TextDiff::from_lines(old, new);
    for op in diff.ops() {
        let (tag, before, after) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            continue;
        }
        for line in new_lines.get(after.clone()).unwrap_or_default() {
            names.extend(defined(line, lang));
        }
        let (lines, at) = if after.is_empty() {
            (&old_lines, before.start)
        } else {
            (&new_lines, after.start)
        };
        let enclosing = lines[..at.min(lines.len())]
            .iter()
            .rev()
            .find_map(|l| defined(l, lang));
        names.extend(enclosing);
    }
    names
}

/// The name a line defines: a function, method, type or top-level binding.
fn defined(line: &str, lang: Lang) -> Option<String> {
    let t = line.trim_start();
    if ["//", "#", "/*", "*"].iter().any(|c| t.starts_with(c)) {
        return None;
    }
    let mut words = t.split_whitespace().peekable();
    let skip: &[&str] = match lang {
        Lang::Rust => &["pub", "async", "unsafe", "extern", "\"C\"", "default"],
        Lang::Python => &["async"],
        Lang::Script => &[
            "export",
            "default",
            "async",
            "declare",
            "abstract",
            "static",
            "public",
            "private",
            "protected",
            "readonly",
            "override",
            "get",
            "set",
        ],
        Lang::Go => &[],
    };
    while let Some(w) = words.peek() {
        if skip.contains(w) || w.starts_with("pub(") {
            words.next();
        } else {
            break;
        }
    }
    let first = words.next()?;
    let keyword: &[&str] = match lang {
        Lang::Rust => &[
            "fn", "struct", "enum", "trait", "type", "union", "const", "static",
        ],
        Lang::Python => &["def", "class"],
        Lang::Script => &[
            "function",
            "function*",
            "class",
            "interface",
            "type",
            "enum",
            "const",
            "let",
            "var",
        ],
        Lang::Go => &["func", "type"],
    };
    if keyword.contains(&first) {
        let rest = t[t.find(first)? + first.len()..].trim_start();
        // A Go method's receiver comes before its name.
        let rest = match (lang, rest.strip_prefix('(')) {
            (Lang::Go, Some(inner)) => inner.split_once(')')?.1.trim_start(),
            _ => rest.strip_prefix("mut ").unwrap_or(rest),
        };
        return ident(rest);
    }
    // A JavaScript or TypeScript method: `name(args) {`.
    if lang == Lang::Script && t.trim_end().ends_with('{') {
        let rest = &t[t.find(first)?..];
        let name = ident(rest)?;
        let after = rest[name.len()..].trim_start();
        const NOT_METHODS: &[&str] = &[
            "if", "for", "while", "switch", "catch", "return", "function", "with", "else",
        ];
        if (after.starts_with('(') || after.starts_with('<')) && !NOT_METHODS.contains(&&*name) {
            return Some(name);
        }
    }
    None
}

fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// The identifier `text` starts with, of at least two characters.
fn ident(text: &str) -> Option<String> {
    let end = text.find(|c: char| !word_char(c)).unwrap_or(text.len());
    let name = &text[..end];
    (name.chars().count() >= 2 && !name.starts_with(|c: char| c.is_ascii_digit()))
        .then(|| name.to_owned())
}

/// What a message says about a name it cannot find, in any of the
/// languages above, lowercased.
const MISSING: &[&str] = &[
    "is not a function",
    "is not a constructor",
    "is not defined",
    "is undefined",
    "undefined:",
    "undefined name",
    "undefined method",
    "undefined reference",
    "has no attribute",
    "cannot import name",
    "cannot find",
    "can't find",
    "could not find",
    "not found in",
    "unresolved",
    "undeclared",
    "no method named",
    "no function or associated item named",
    "does not exist",
    "has no exported member",
    "does not provide an export named",
    "is not exported",
];

/// The first of `names` that a line of `message` (outside stack frames)
/// both names, as a whole word, and says is missing: the test failed on
/// the original code only because the change adds that name.
pub fn missing<'a>(message: &str, names: &'a BTreeSet<String>) -> Option<&'a str> {
    if names.is_empty() {
        return None;
    }
    for line in message.lines() {
        let t = line.trim_start();
        if t.starts_with("at ") || t.starts_with("File \"") {
            continue;
        }
        let lower = t.to_lowercase();
        if !MISSING.iter().any(|m| lower.contains(m)) {
            continue;
        }
        let found = t
            .split(|c: char| !word_char(c))
            .find_map(|w| names.get(w).map(String::as_str));
        if found.is_some() {
            return found;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn added_definitions_and_the_enclosing_one_are_changed() {
        let old = "export function add(a, b) {\n  return a + b;\n}\n";
        let new = "export function add(a, b) {\n  return a + b + 0;\n}\n\nexport async function negate(a) {\n  return -a;\n}\n";
        assert_eq!(
            changed("src/calc.js", Some(old), Some(new)),
            set(&["add", "negate"])
        );
    }

    #[test]
    fn each_language_names_its_definitions() {
        let cases: &[(&str, &str, &[&str])] = &[
            ("src/lib.rs", "pub(crate) fn divide(a: i32) -> i32 {\n    a\n}\npub struct Ratio;\n", &["Ratio", "divide"]),
            ("calc/core.py", "class Calc:\n    async def negate(self, a):\n        return -a\n", &["Calc", "negate"]),
            ("calc.go", "func (c *Calc) Negate(a int) int {\n\treturn -a\n}\ntype Op int\n", &["Negate", "Op"]),
            ("src/calc.ts", "class Calc {\n  static negate(a: number): number {\n    if (a) {\n      return -a;\n    }\n  }\n}\n", &["Calc", "negate"]),
            ("README.md", "# fn nothing\n", &[]),
        ];
        for (path, new, want) in cases {
            assert_eq!(changed(path, None, Some(new)), set(want), "{path}");
        }
    }

    #[test]
    fn a_deletion_names_the_definition_it_was_in() {
        let old = "def keep():\n    a = 1\n    b = 2\n    return a\n";
        let new = "def keep():\n    a = 1\n    return a\n";
        assert_eq!(changed("m.py", Some(old), Some(new)), set(&["keep"]));
    }

    #[test]
    fn a_message_that_a_changed_name_is_missing_names_it() {
        let names = set(&["negate", "Ratio"]);
        for (message, name) in [
            ("TypeError: calc.negate is not a function", "negate"),
            ("AttributeError: module 'calc' has no attribute 'negate'", "negate"),
            ("error[E0425]: cannot find function `negate` in module `calc`", "negate"),
            ("./calc_test.go:9:7: undefined: negate", "negate"),
            ("SyntaxError: The requested module './calc.js' does not provide an export named 'negate'", "negate"),
            ("error[E0433]: failed to resolve: use of undeclared type `Ratio`", "Ratio"),
        ] {
            assert_eq!(missing(message, &names), Some(name), "{message}");
        }
    }

    #[test]
    fn a_failure_about_behaviour_names_nothing_missing() {
        let names = set(&["negate", "parse"]);
        for message in [
            "AssertionError: expected negate(2) to be -2, got 2",
            "TypeError: Cannot read properties of undefined (reading 'x')\n    at parse (src/parse.js:3:9)",
            "TypeError: other is not a function",
            "error: test failed, to rerun pass `--lib`",
        ] {
            assert_eq!(missing(message, &names), None, "{message}");
        }
        assert_eq!(missing("negate is not a function", &BTreeSet::new()), None);
    }
}
