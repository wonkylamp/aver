//! The TAP stream `ava --tap` prints, as per-test results (ARCHITECTURE.md
//! §1.9.1, the `tap` report format).
//!
//! A test point is `ok <n> - <title>` or `not ok <n> - <title>`, optionally
//! followed by a YAML block between `  ---` and `  ...` that holds the
//! failure. Ava prefixes each title with its file when a run has more than
//! one (`sub › b › title`: the path below the files' common directory, the
//! extension and `test-`, `.test.` and `.spec.` dropped), and the parts
//! become the entry's path. A trailing ` # SKIP` is a skipped test and
//! ` # TODO` a `test.todo` (skipped too); any other ` # ` belongs to the
//! title. Ava reports a crash (an uncaught exception, a file that exited
//! non-zero) as a failing point titled by the error, so it is a failing
//! entry. The stream must open with `TAP version 13` and close with a plan
//! (`1..N`): a run cut short has no plan and is judged by its exit code.
//! The plan's count leaves out crashes, so it is not checked against the
//! points.

use std::collections::BTreeMap;

use aver_core::{CheckId, Set, TestResult, TestStatus};

use super::junit::{add, JunitError, Location};

/// Ava's separator between a title's file parts and the title.
const SEPARATOR: &str = " \u{203a} ";

struct Point {
    title: String,
    status: TestStatus,
    message: String,
}

pub fn parse(
    check: &CheckId,
    text: &[u8],
    locations: &mut BTreeMap<String, Location>,
) -> Result<Set<TestResult>, JunitError> {
    let text = String::from_utf8_lossy(text);
    let err = |m: &str| JunitError::Text(format!("TAP: {m}"));
    let mut lines = text.lines();
    if !lines.by_ref().any(|l| l.trim_end() == "TAP version 13") {
        return Err(err("no `TAP version 13` line"));
    }
    let mut points: Vec<Point> = Vec::new();
    let mut in_yaml = false;
    let mut planned = false;
    for line in lines {
        if in_yaml {
            if line.trim_end() == "  ..." {
                in_yaml = false;
            } else if let Some(p) = points.last_mut() {
                let line = line.strip_prefix("    ").unwrap_or(line.trim_start());
                p.message.push_str(&strip_ansi(line));
                p.message.push('\n');
            }
            continue;
        }
        if line.trim_end() == "  ---" && !points.is_empty() {
            in_yaml = true;
            continue;
        }
        if line.starts_with("TAP version") {
            return Err(err("a second `TAP version` line"));
        }
        if line.starts_with("1..") && line[3..].trim_end().parse::<u64>().is_ok() {
            planned = true;
            break;
        }
        if let Some(p) = point(line) {
            points.push(p);
        }
    }
    if in_yaml {
        return Err(err("a YAML block without its closing `...`"));
    }
    if !planned {
        return Err(err("no plan line (`1..N`): the run ended early"));
    }
    let mut results: BTreeMap<String, TestResult> = BTreeMap::new();
    for p in points {
        let parts: Vec<&str> = p.title.split(SEPARATOR).collect();
        if parts.iter().any(|s| s.trim().is_empty()) {
            return Err(err(&format!(
                "a test point with an empty title: {:?}",
                p.title
            )));
        }
        let message = p.message.trim_end();
        add(
            &mut results,
            locations,
            check,
            &parts,
            p.status,
            (!message.is_empty()).then_some(message),
            Location::default(),
        )
        .map_err(JunitError::Text)?;
    }
    Set::new(results.into_values().collect()).map_err(|e| JunitError::Text(e.to_string()))
}

/// `ok 3 - title`, `ok 3 - title # SKIP`, `not ok 3 - title # TODO` or
/// `not ok 3 - title`.
fn point(line: &str) -> Option<Point> {
    let (ok, rest) = match line.strip_prefix("not ok ") {
        Some(rest) => (false, rest),
        None => (true, line.strip_prefix("ok ")?),
    };
    let (n, title) = rest.split_once(' ').unwrap_or((rest, ""));
    n.parse::<u64>().ok()?;
    let title = title.strip_prefix("- ").unwrap_or(title).trim_end();
    let (title, status) = if let Some(t) = title.strip_suffix(" # SKIP") {
        (t, TestStatus::Skip)
    } else if let Some(t) = title.strip_suffix(" # TODO") {
        (t, TestStatus::Skip)
    } else if ok {
        (title, TestStatus::Pass)
    } else {
        (title, TestStatus::Fail)
    };
    Some(Point {
        title: strip_ansi(title),
        status,
        message: String::new(),
    })
}

/// `text` without ANSI color sequences.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `ava --tap` over `test/a.js`, `test/c.js` and `test/sub/b.js`.
    const AVA: &str = "TAP version 13
ok 1 - c \u{203a} ok
ok 2 - a \u{203a} skipped one # SKIP
not ok 3 - a \u{203a} later # TODO
ok 4 - a \u{203a} passes
not ok 5 - a \u{203a} fails
  ---
    name: AssertionError
    assertion: t.is()
    details:
      'Difference (- actual, + expected):': |-
        - 2
        + 3
    message: ''
  ...
ok 6 - a \u{203a} [crit:x] tagged # hash
#   a log
ok 7 - a \u{203a} known failing
ok 8 - sub \u{203a} b \u{203a} deep
not ok 9 - sub \u{203a} b \u{203a} throws
  ---
    name: AssertionError
    message: Error thrown in test
  ...
not ok 10 - Error: uncaught later
  ---
    name: Error
    message: uncaught later
  ...
not ok 11 - test/c.js exited with a non-zero exit code: 1

1..9
# tests 8
# pass 5
# skip 1
# fail 5
";

    fn read(text: &str) -> Result<Vec<(String, TestStatus, Option<String>)>, JunitError> {
        let check: CheckId = "test".parse().unwrap();
        let mut locations = BTreeMap::new();
        Ok(parse(&check, text.as_bytes(), &mut locations)?
            .iter()
            .map(|t| {
                (
                    t.id.as_str().to_owned(),
                    t.status,
                    t.message.as_ref().map(|m| m.as_str().to_owned()),
                )
            })
            .collect())
    }

    #[test]
    fn ava_points_are_read_with_their_file_prefix_status_and_failure() {
        let got = read(AVA).unwrap();
        let ids: Vec<(&str, TestStatus)> = got.iter().map(|(i, s, _)| (i.as_str(), *s)).collect();
        assert_eq!(
            ids,
            vec![
                ("test::Error: uncaught later", TestStatus::Fail),
                ("test::a > [crit:x] tagged # hash", TestStatus::Pass),
                ("test::a > fails", TestStatus::Fail),
                ("test::a > known failing", TestStatus::Pass),
                ("test::a > later", TestStatus::Skip),
                ("test::a > passes", TestStatus::Pass),
                ("test::a > skipped one", TestStatus::Skip),
                ("test::c > ok", TestStatus::Pass),
                ("test::sub > b > deep", TestStatus::Pass),
                ("test::sub > b > throws", TestStatus::Fail),
                (
                    "test::test/c.js exited with a non-zero exit code: 1",
                    TestStatus::Fail
                ),
            ]
        );
        let fails = &got.iter().find(|(i, ..)| i == "test::a > fails").unwrap().2;
        let fails = fails.as_deref().unwrap();
        assert!(fails.starts_with("name: AssertionError\n"), "{fails}");
        assert!(fails.contains("    - 2\n    + 3"), "{fails}");
    }

    #[test]
    fn a_single_file_run_has_bare_titles() {
        let got = read("TAP version 13\nok 1 - adds\nnot ok 2 - subtracts\n\n1..2\n").unwrap();
        let ids: Vec<&str> = got.iter().map(|(i, ..)| i.as_str()).collect();
        assert_eq!(ids, vec!["test::adds", "test::subtracts"]);
    }

    #[test]
    fn a_stream_cut_short_or_missing_is_malformed() {
        assert!(read("").is_err());
        assert!(read("ok 1 - a\n1..1\n").is_err(), "no version line");
        assert!(
            read("TAP version 13\nok 1 - a\nnot ok 2 - b\n").is_err(),
            "no plan: killed mid-run"
        );
        assert!(read("TAP version 13\nnot ok 1 - b\n  ---\n    name: x\n").is_err());
        assert!(read("TAP version 13\nok 1 - \n1..1\n").is_err());
    }

    #[test]
    fn a_run_with_no_tests_is_an_empty_report() {
        assert!(read("TAP version 13\n\n1..0\n").unwrap().is_empty());
    }
}
