//! The diagnostics `tsc` prints, as per-error results (ARCHITECTURE.md
//! §1.9.1, the `tsc` report format), so a base that already fails its type
//! check judges only the errors a change adds instead of blocking every
//! verify on one command-level entry.
//!
//! `tsc` prints one line per error, `src/a.ts(3,7): error TS2322: Type …`
//! (or `src/a.ts:3:7 - error TS2322: Type …` with `--pretty`), followed by
//! indented lines that continue the message, and `error TS5023: …` for an
//! error with no file. Each error is a failing entry whose path is the file
//! (relative to the check's directory), the code and the message's first
//! line: not the line number, which moves whenever code above it changes.
//! The same error twice in one file is two entries, the second suffixed
//! ` (2)`. The report lists failures only ([`ReportFormat::lists_failures_only`]):
//! an error a run does not print passes there. Every other line (npm's
//! banner, `Found 3 errors.`) is ignored, so output without an error line
//! is a report with no failures, and the check's exit code still says
//! whether it ran.
//!
//! [`ReportFormat::lists_failures_only`]: aver_core::ReportFormat::lists_failures_only

use std::collections::BTreeMap;

use aver_core::{CheckId, Set, TestResult, TestStatus};

use super::junit::{add, JunitError, Location};

/// The longest message part of an entry's path.
const MAX_ID_MESSAGE: usize = 300;

/// One error line: file (if any), line, code and message.
struct Diagnostic {
    file: Option<String>,
    line: Option<u32>,
    code: String,
    message: String,
}

/// `file(line,col): error TS1234: message`, `file:line:col - error TS1234:
/// message` or `error TS1234: message`.
fn diagnostic(line: &str) -> Option<Diagnostic> {
    let at = line.find("error TS")?;
    let (head, rest) = (&line[..at], &line[at + "error ".len()..]);
    let (code, message) = rest.split_once(':')?;
    if code.len() < 3 || !code[2..].bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let message = message.trim().to_owned();
    let head = head.trim_end();
    if head.is_empty() {
        return Some(Diagnostic {
            file: None,
            line: None,
            code: code.to_owned(),
            message,
        });
    }
    // `file(3,7):` or `file:3:7 -`.
    let (file, at_line) = if let Some(place) = head.strip_suffix(':') {
        let (file, pos) = place.rsplit_once('(')?;
        let pos = pos.strip_suffix(')')?;
        (file, pos.split(',').next()?.parse().ok()?)
    } else {
        let place = head.strip_suffix('-')?.trim_end();
        let (rest, _col) = place.rsplit_once(':')?;
        let (file, line) = rest.rsplit_once(':')?;
        (file, line.parse().ok()?)
    };
    if file.is_empty() {
        return None;
    }
    Some(Diagnostic {
        file: Some(file.to_owned()),
        line: Some(at_line),
        code: code.to_owned(),
        message,
    })
}

pub fn parse(
    check: &CheckId,
    text: &[u8],
    locations: &mut BTreeMap<String, Location>,
) -> Result<Set<TestResult>, JunitError> {
    let text = String::from_utf8_lossy(text);
    let mut found: Vec<(Diagnostic, String)> = Vec::new();
    for raw in text.lines() {
        let line = strip_ansi(raw.trim_end());
        if let Some(d) = diagnostic(&line) {
            let full = line.trim().to_owned();
            found.push((d, full));
            continue;
        }
        // An indented line continues the previous error's message.
        if raw.starts_with(' ') && !line.trim().is_empty() {
            if let Some((_, full)) = found.last_mut() {
                full.push('\n');
                full.push_str(line.trim_end());
            }
        }
    }
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    let mut results: BTreeMap<String, TestResult> = BTreeMap::new();
    for (d, full) in found {
        let first = d.message.lines().next().unwrap_or_default();
        let first = cut(first, MAX_ID_MESSAGE);
        let file = d.file.as_deref().unwrap_or("(global)");
        let base = format!("{} {first}", d.code);
        let key = format!("{file}\u{0}{base}");
        let n = seen.entry(key).or_insert(0);
        *n += 1;
        let name = if *n == 1 {
            base
        } else {
            format!("{base} ({n})")
        };
        add(
            &mut results,
            locations,
            check,
            &[file, name.as_str()],
            TestStatus::Fail,
            Some(&full),
            Location {
                file: d.file.clone(),
                line: d.line,
            },
        )
        .map_err(JunitError::Text)?;
    }
    Set::new(results.into_values().collect()).map_err(|e| JunitError::Text(e.to_string()))
}

/// `text` without ANSI color sequences (`tsc --pretty` colors its output).
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn cut(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(text: &str) -> Vec<String> {
        let check: CheckId = "types".parse().unwrap();
        let mut locations = BTreeMap::new();
        parse(&check, text.as_bytes(), &mut locations)
            .unwrap()
            .iter()
            .map(|t| t.id.as_str().to_owned())
            .collect()
    }

    #[test]
    fn errors_are_keyed_by_file_code_and_message_not_line() {
        let out = "\
> site@1.0.0 typecheck\n\
> tsc --noEmit\n\
\n\
src/a.ts(3,7): error TS2322: Type 'string' is not assignable to type 'number'.\n\
src/b.ts(10,1): error TS2304: Cannot find name 'foo'.\n\
src/b.ts(12,1): error TS2304: Cannot find name 'foo'.\n\
error TS5023: Unknown compiler option 'strictest'.\n\
Found 4 errors in 2 files.\n";
        assert_eq!(
            ids(out),
            vec![
                "types::(global) > TS5023 Unknown compiler option 'strictest'.",
                "types::src/a.ts > TS2322 Type 'string' is not assignable to type 'number'.",
                "types::src/b.ts > TS2304 Cannot find name 'foo'.",
                "types::src/b.ts > TS2304 Cannot find name 'foo'. (2)",
            ]
        );
        let moved = out.replace("(3,7)", "(30,7)");
        assert_eq!(ids(&moved), ids(out), "a moved error keeps its id");
    }

    #[test]
    fn pretty_output_and_continuation_lines_are_read() {
        let out = "\u{1b}[96msrc/a.ts\u{1b}[0m:\u{1b}[93m3\u{1b}[0m:\u{1b}[93m7\u{1b}[0m - \u{1b}[91merror\u{1b}[0m\u{1b}[90m TS2322: \u{1b}[0mType 'string' is not assignable to type 'number'.\n";
        assert_eq!(
            ids(out),
            vec!["types::src/a.ts > TS2322 Type 'string' is not assignable to type 'number'."]
        );
        let check: CheckId = "types".parse().unwrap();
        let mut locations = BTreeMap::new();
        let tests = parse(
            &check,
            b"src/x.ts(1,1): error TS2345: Argument of type 'A' is not assignable.\n  Type 'A' is missing 'b'.\n",
            &mut locations,
        )
        .unwrap();
        let t = tests.iter().next().unwrap();
        assert!(t.message.as_ref().unwrap().as_str().contains("missing 'b'"));
        assert_eq!(locations.values().next().unwrap().line, Some(1));
    }

    #[test]
    fn output_without_errors_is_an_empty_report() {
        assert!(ids("Found 0 errors.\n").is_empty());
        assert!(ids("").is_empty());
    }
}
