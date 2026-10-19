//! Jest's `--json` report into per-test results (ARCHITECTURE.md §1.9.1).
//!
//! Each entry of `testResults` is one test file and each of its
//! `assertionResults` one test, whose path is the file (relative to the
//! slot, `<src>` in the report, or to the old sandbox's `/work`),
//! its `ancestorTitles` and its `title`, joined by ` > ` as in JUnit
//! reports. `passed` is `pass`, `failed` is `fail`, and every other status
//! (`pending`, `skipped`, `todo`, `disabled`, `focused`) is `skip`, which
//! counts as no result. A failure's message is its `failureMessages`, joined.
//!
//! A test file that failed without a failing test (it could not load, or a
//! hook outside any test threw) gets one failing entry `<file> > (suite
//! failed to run)` with the file's `message`, so a file the change broke
//! shows as a failure of its own and not only as tests missing from the
//! head's report.
//!
//! A report whose root is not an object with a `testResults` array, a test
//! with an empty title or the path `(exit)` is malformed, like a JUnit report.

use std::collections::BTreeMap;

use aver_core::{CheckId, Set, TestResult, TestStatus};
use serde_json::Value;

use super::junit::{add, JunitError, Location};

/// The test name of a file-level failure.
pub const SUITE_FAILED: &str = "(suite failed to run)";

pub fn parse(
    check: &CheckId,
    json: &[u8],
    locations: &mut BTreeMap<String, Location>,
) -> Result<Set<TestResult>, JunitError> {
    let bad = |m: &str| JunitError::Json(m.to_owned());
    let json = replace_lone_surrogates(json);
    let root: Value = serde_json::from_slice(&json).map_err(|e| JunitError::Json(e.to_string()))?;
    let files = root
        .get("testResults")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("no testResults array"))?;
    let mut results: BTreeMap<String, TestResult> = BTreeMap::new();
    for file in files {
        let name = file
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("a test file has no name"))?;
        let path = name
            .strip_prefix("/work/")
            .or_else(|| name.strip_prefix(&format!("{}/", super::junit::SRC_ROOT)))
            .unwrap_or(name);
        let tests = match file.get("assertionResults") {
            None | Some(Value::Null) => &[][..],
            Some(Value::Array(tests)) => tests.as_slice(),
            Some(_) => return Err(bad("assertionResults is not an array")),
        };
        let mut any_failed = false;
        for test in tests {
            let title = test
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if title.trim().is_empty() {
                return Err(bad("a test has an empty title"));
            }
            let ancestors: Vec<&str> = match test.get("ancestorTitles") {
                Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            let status = match test.get("status").and_then(Value::as_str) {
                Some("passed") => TestStatus::Pass,
                Some("failed") => TestStatus::Fail,
                _ => TestStatus::Skip,
            };
            any_failed |= status == TestStatus::Fail;
            let message = match test.get("failureMessages") {
                Some(Value::Array(m)) => {
                    let joined: Vec<&str> = m.iter().filter_map(Value::as_str).collect();
                    Some(strip_ansi(&joined.join("\n")))
                }
                _ => None,
            };
            let mut segments = vec![path];
            segments.extend(ancestors.iter().filter(|a| !a.trim().is_empty()));
            segments.push(title);
            let location = Location {
                file: Some(path.to_owned()),
                line: test
                    .get("location")
                    .and_then(|l| l.get("line"))
                    .and_then(Value::as_u64)
                    .and_then(|l| u32::try_from(l).ok()),
            };
            add(
                &mut results,
                locations,
                check,
                &segments,
                status,
                message.as_deref().filter(|m| !m.trim().is_empty()),
                location,
            )
            .map_err(JunitError::Json)?;
        }
        let file_failed = file.get("status").and_then(Value::as_str) == Some("failed");
        if file_failed && !any_failed {
            let message = file
                .get("message")
                .and_then(Value::as_str)
                .map(strip_ansi)
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| "the test file failed without a failing test".to_owned());
            add(
                &mut results,
                locations,
                check,
                &[path, SUITE_FAILED],
                TestStatus::Fail,
                Some(message.trim()),
                Location::default(),
            )
            .map_err(JunitError::Json)?;
        }
    }
    Set::new(results.into_values().collect()).map_err(|e| JunitError::Json(e.to_string()))
}

/// `json` with every `\uXXXX` escape of a lone UTF-16 surrogate replaced by
/// `\ufffd`. Jest truncates failure messages by UTF-16 code units, which can
/// cut an emoji in half, and JSON parsers that insist on valid Unicode
/// (serde_json among them) then refuse the whole report.
fn replace_lone_surrogates(json: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    fn hex4(b: &[u8]) -> Option<u16> {
        let s = std::str::from_utf8(b.get(..4)?).ok()?;
        u16::from_str_radix(s, 16).ok()
    }
    let escape_at = |i: usize| -> Option<u16> {
        (json.get(i..i + 2) == Some(b"\\u".as_slice()))
            .then(|| hex4(&json[i + 2..]))
            .flatten()
    };
    let mut out: Option<Vec<u8>> = None;
    let mut i = 0;
    while i < json.len() {
        if json[i] != b'\\' {
            if let Some(o) = out.as_mut() {
                o.push(json[i]);
            }
            i += 1;
            continue;
        }
        let Some(unit) = escape_at(i) else {
            // Any other escape is two bytes; copying them keeps `\\u` literal.
            let end = (i + 2).min(json.len());
            if let Some(o) = out.as_mut() {
                o.extend_from_slice(&json[i..end]);
            }
            i = end;
            continue;
        };
        let high = (0xD800..0xDC00).contains(&unit);
        let low = (0xDC00..0xE000).contains(&unit);
        let paired = high && escape_at(i + 6).is_some_and(|u| (0xDC00..0xE000).contains(&u));
        let (keep, len) = if paired {
            (true, 12)
        } else {
            (!high && !low, 6)
        };
        if keep {
            if let Some(o) = out.as_mut() {
                o.extend_from_slice(&json[i..i + len]);
            }
        } else {
            let o = out.get_or_insert_with(|| json[..i].to_vec());
            o.extend_from_slice(b"\\ufffd");
        }
        i += len;
    }
    match out {
        Some(o) => std::borrow::Cow::Owned(o),
        None => std::borrow::Cow::Borrowed(json),
    }
}

/// `text` without ANSI color escapes, which Jest writes when it thinks it is
/// talking to a terminal.
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
