//! `cargo test`'s own output into per-test results (ARCHITECTURE.md
//! §1.9.1): the `libtest` report format, read from the text the check wrote
//! to its report path, so Rust projects need no extra tool.
//!
//! Cargo runs one test binary after another and prints a line before each:
//! `Running unittests src/lib.rs (target/debug/deps/aver_core-1f2e…)`,
//! `Running tests/api.rs (target/debug/deps/api-9a8b…)`, or
//! `Doc-tests aver_core`. Each binary prints `running N tests`, one line
//! `test <name> ... ok|FAILED|ignored` per test, the output of each failed
//! test under `---- <name> stdout ----`, and `test result: …`. A test's path
//! is the binary (its name without the hash, and its target: `aver_core
//! src/lib.rs`, `api tests/api.rs`, `aver_core doc-tests`) and the test
//! name, so the same test name in two binaries (a library and a binary of
//! one package) stays two tests, and the hash, which changes with the
//! toolchain and the package version, is not part of any id. A doc-test's
//! name loses its `(line N)`, which moves whenever lines are added above
//! it; doc-tests of one item then merge, failure first.
//!
//! `ok` is `pass`, `FAILED` is `fail`, `ignored` (with or without a reason)
//! is `skip`. A failure's message is its output section. Lines outside a
//! binary's test list (compiler output, a test's own prints) are ignored,
//! and so is everything that does not end in a result, such as `test x has
//! been running for over 60 seconds`. Output without any `running N tests`
//! line is malformed: the build failed, the toolchain could not start
//! (a pinned toolchain that is not installed, no `Cargo.toml`), or no
//! binary ran, so the check is judged by its exit code alone.

use std::collections::BTreeMap;

use aver_core::run::MAX_TEST_MESSAGE_BYTES;
use aver_core::{CheckId, Set, TestResult, TestStatus};

use super::junit::{add, JunitError, Location};

pub fn parse(
    check: &CheckId,
    text: &[u8],
    locations: &mut BTreeMap<String, Location>,
) -> Result<Set<TestResult>, JunitError> {
    let text = String::from_utf8_lossy(text);
    let text = plain(&text);
    let mut binary = String::from("tests");
    let mut listing = false;
    let mut seen_binary = false;
    let mut statuses: Vec<(String, String, TestStatus)> = Vec::new();
    let mut messages: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut section: Option<(String, String)> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("Running ") {
            if let Some(label) = running_label(rest) {
                binary = label;
                listing = false;
                section = None;
                continue;
            }
        }
        if let Some(krate) = trimmed.strip_prefix("Doc-tests ") {
            binary = format!("{} doc-tests", krate.trim());
            listing = false;
            section = None;
            continue;
        }
        if line.starts_with("running ") && (line.ends_with(" test") || line.ends_with(" tests")) {
            listing = true;
            seen_binary = true;
            section = None;
            continue;
        }
        if line.starts_with("test result: ") {
            listing = false;
            section = None;
            continue;
        }
        if let Some(name) = line.strip_prefix("---- ").and_then(|r| {
            r.strip_suffix(" stdout ----")
                .or_else(|| r.strip_suffix(" stderr ----"))
        }) {
            let key = (binary.clone(), normalize(name));
            messages.entry(key.clone()).or_default();
            section = Some(key);
            continue;
        }
        if let Some(key) = &section {
            if line == "failures:" || line == "successes:" {
                section = None;
                continue;
            }
            let message = messages.entry(key.clone()).or_default();
            if message.len() < MAX_TEST_MESSAGE_BYTES * 2 {
                message.push_str(raw);
                message.push('\n');
            }
            continue;
        }
        if !listing {
            continue;
        }
        let Some(rest) = line.strip_prefix("test ") else {
            continue;
        };
        let Some((name, result)) = rest.rsplit_once(" ... ") else {
            continue;
        };
        let status = match result {
            "ok" => TestStatus::Pass,
            "FAILED" => TestStatus::Fail,
            r if r == "ignored" || r.starts_with("ignored, ") => TestStatus::Skip,
            _ => continue,
        };
        statuses.push((binary.clone(), normalize(name), status));
    }
    if !seen_binary {
        if let Some(toolchain) = super::toolchain::classify(&text) {
            return Err(JunitError::Toolchain(toolchain.to_string()));
        }
        if let Some(error) = build_error(&text) {
            return Err(JunitError::Build(error));
        }
        return Err(JunitError::Text(
            "no `running N tests` line: no test binary ran".into(),
        ));
    }
    let mut results: BTreeMap<String, TestResult> = BTreeMap::new();
    for (binary, name, status) in statuses {
        if name.trim().is_empty() {
            return Err(JunitError::Text("a test has an empty name".into()));
        }
        let message = messages
            .get(&(binary.clone(), name.clone()))
            .map(|m| m.trim().to_owned())
            .filter(|m| !m.is_empty());
        add(
            &mut results,
            locations,
            check,
            &[binary.as_str(), name.as_str()],
            status,
            message.as_deref(),
            Location::default(),
        )
        .map_err(JunitError::Text)?;
    }
    Set::new(results.into_values().collect()).map_err(|e| JunitError::Text(e.to_string()))
}

/// The labels of the test binaries `text` shows running, those that ran no
/// test among them, when each ran to its `test result:` line; `None` when
/// one stopped before it (it crashed, or the output was cut), so the tests
/// it would have reported are unknown.
pub fn binaries(text: &str) -> Option<std::collections::BTreeSet<String>> {
    let mut done = std::collections::BTreeSet::new();
    let mut open: Option<String> = None;
    for line in plain(text).lines().map(str::trim_end) {
        let trimmed = line.trim_start();
        let label = match trimmed.strip_prefix("Running ") {
            Some(rest) => running_label(rest),
            None => trimmed
                .strip_prefix("Doc-tests ")
                .map(|krate| format!("{} doc-tests", krate.trim())),
        };
        if let Some(label) = label {
            if open.replace(label).is_some() {
                return None;
            }
            continue;
        }
        if line.starts_with("test result: ") {
            done.insert(open.take()?);
        }
    }
    open.is_none().then_some(done)
}

/// The compiler's or cargo's first error in output that ran no test binary
/// (`error[E0425]: cannot find value …`, `error: linking with `cc`
/// failed …`, `error: failed to select a version …`), with its `-->`
/// location when the next lines give one: why the build failed. `None`
/// when the output holds no error line.
fn build_error(text: &str) -> Option<String> {
    let mut lines = text.lines().map(str::trim_end);
    let first = lines.find(|l| {
        let l = l.trim_start();
        (l.starts_with("error[E") || l.starts_with("error: "))
            && !l.starts_with("error: test failed")
    })?;
    let mut error = first.trim().to_owned();
    for next in lines.take(4) {
        if let Some(at) = next.trim_start().strip_prefix("--> ") {
            error.push_str(&format!(" (at {at})"));
            break;
        }
    }
    const MAX: usize = 400;
    if error.len() > MAX {
        let mut end = MAX;
        while !error.is_char_boundary(end) {
            end -= 1;
        }
        error.truncate(end);
        error.push('…');
    }
    Some(error)
}

/// The binary label of a `Running …` line: `<binary name> <target>` from
/// `<target> (<path>/<name>-<hash>)`, or the binary name alone from an
/// older cargo's bare path. `None` for a line that is not cargo's.
/// `text` without ANSI escape sequences. Under `CARGO_TERM_COLOR=always`
/// cargo colors its `Running` and `Doc-tests` lines, which would otherwise
/// not match and leave every binary unlabeled.
fn plain(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('\x1b') {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        if chars.next_if_eq(&'[').is_some() {
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    std::borrow::Cow::Owned(out)
}

fn running_label(rest: &str) -> Option<String> {
    let rest = rest.trim();
    let (target, path) = match rest.rsplit_once(" (") {
        Some((target, path)) => (
            target.trim_start_matches("unittests ").trim(),
            path.strip_suffix(')')?,
        ),
        None => ("", rest),
    };
    let file = path.rsplit('/').next()?;
    let name = match file.rsplit_once('-') {
        Some((name, hash)) if !hash.is_empty() && hash.bytes().all(|b| b.is_ascii_hexdigit()) => {
            name
        }
        _ => file,
    };
    let name = name.strip_suffix(".exe").unwrap_or(name);
    // `--examples` builds its test binaries in `examples/`, the rest in
    // `deps/`; a line naming neither is a test's own output.
    let dir = path.rsplit('/').nth(1).unwrap_or_default();
    if name.is_empty() || !(path.contains("deps") || dir == "examples") {
        return None;
    }
    Some(if target.is_empty() {
        name.to_owned()
    } else {
        format!("{name} {target}")
    })
}

/// A test name without what moves when unrelated lines change: a
/// doc-test's ` (line N)`, and the ` - should panic` or ` - compile fail`
/// suffixes libtest adds.
fn normalize(name: &str) -> String {
    let mut name = name.trim().to_owned();
    for suffix in [" - should panic", " - compile fail", " - no_run"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            name = stripped.to_owned();
        }
    }
    if let Some(at) = name.rfind(" (line ") {
        let tail = &name[at + " (line ".len()..];
        if tail
            .strip_suffix(')')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            name.truncate(at);
        }
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_build_says_why_instead_of_a_parse_error() {
        let check: CheckId = "rust".parse().unwrap();
        let out = "   Compiling aver-core v0.1.0 (/w/crates/aver-core)\n\
error[E0425]: cannot find value `x` in this scope\n\
  --> crates/aver-core/src/lib.rs:3:5\n\
   |\n\
error: could not compile `aver-core` (lib) due to 1 previous error\n";
        let err = parse(&check, out.as_bytes(), &mut BTreeMap::new()).unwrap_err();
        assert_eq!(
            err,
            JunitError::Build(
                "error[E0425]: cannot find value `x` in this scope (at crates/aver-core/src/lib.rs:3:5)"
                    .into()
            )
        );
        let linker = "error: linking with `cc` failed: exit status: 1\n  = note: ld: unknown option: --no-keep-memory\n";
        assert!(matches!(
            parse(&check, linker.as_bytes(), &mut BTreeMap::new()),
            Err(JunitError::Build(e)) if e.starts_with("error: linking with `cc` failed")
        ));
        assert!(matches!(
            parse(&check, b"nothing ran\n", &mut BTreeMap::new()),
            Err(JunitError::Text(_))
        ));
    }

    #[test]
    fn colored_cargo_lines_still_name_their_binaries() {
        let out = "\x1b[1m\x1b[92m     Running\x1b[0m tests/sub.rs (target/debug/deps/sub-0123456789abcdef)\n\
                   running 1 test\ntest sub_works ... ok\n\n\
                   test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
        assert_eq!(
            binaries(out).map(|b| b.into_iter().collect::<Vec<_>>()),
            Some(vec!["sub tests/sub.rs".to_owned()])
        );
        let check: CheckId = "rust".parse().unwrap();
        let results = parse(&check, out.as_bytes(), &mut BTreeMap::new()).unwrap();
        assert!(results
            .iter()
            .all(|t| t.id.test_path().starts_with("sub tests/sub.rs")));
    }

    #[test]
    fn a_missing_toolchain_is_named_with_its_fix() {
        let check: CheckId = "rust".parse().unwrap();
        let out = "error: toolchain '1.80.0-aarch64-apple-darwin' is not installed\n";
        let err = parse(&check, out.as_bytes(), &mut BTreeMap::new()).unwrap_err();
        assert!(
            matches!(&err, JunitError::Toolchain(e) if e.contains("1.80.0") && e.contains("rustup toolchain install")),
            "{err}"
        );
        let out = "error: could not find `Cargo.toml` in `/work` or any parent directory\n";
        assert!(matches!(
            parse(&check, out.as_bytes(), &mut BTreeMap::new()),
            Err(JunitError::Toolchain(e)) if e.contains("set its cwd")
        ));
    }

    #[test]
    fn binary_labels_drop_the_hash() {
        assert_eq!(
            running_label("unittests src/lib.rs (target/debug/deps/aver_core-1f2e3d4c5b6a7980)")
                .as_deref(),
            Some("aver_core src/lib.rs")
        );
        assert_eq!(
            running_label("tests/api.rs (target/debug/deps/api-9a8b7c6d5e4f3a2b)").as_deref(),
            Some("api tests/api.rs")
        );
        assert_eq!(
            running_label("target/debug/deps/calc-0123456789abcdef").as_deref(),
            Some("calc")
        );
        assert_eq!(running_label("the build script"), None);
        assert_eq!(
            running_label(
                "unittests examples/bench_cache.rs (target/debug/examples/bench_cache-c487b844877fd224)"
            )
            .as_deref(),
            Some("bench_cache examples/bench_cache.rs")
        );
    }

    /// Defect: a binary that crashed before its result line counts as run,
    /// so its tests would be taken as never existing.
    #[test]
    fn binaries_are_listed_only_when_each_finished() {
        let out = "     Running unittests src/lib.rs (target/debug/deps/calc-0123456789abcdef)\n\
running 0 tests\n\
\n\
test result: ok. 0 passed; 0 failed\n\
     Running tests/api.rs (target/debug/deps/api-0123456789abcdef)\n\
running 1 test\n\
test adds ... ok\n\
\n\
test result: ok. 1 passed; 0 failed\n\
   Doc-tests calc\n\
running 0 tests\n\
test result: ok. 0 passed; 0 failed\n";
        let labels: Vec<String> = binaries(out).unwrap().into_iter().collect();
        assert_eq!(
            labels,
            ["api tests/api.rs", "calc doc-tests", "calc src/lib.rs"]
        );
        let example = "     Running unittests examples/demo.rs (target/debug/examples/demo-0123456789abcdef)\n\
running 0 tests\n\
test result: ok. 0 passed; 0 failed\n";
        assert_eq!(
            binaries(&format!("{out}{example}")).map(|b| b.len()),
            Some(4),
            "cargo test --examples finishes its example binaries too"
        );
        let crashed = out.replace(
            "test result: ok. 1 passed; 0 failed\n",
            "error: test failed (signal: 11)\n",
        );
        assert_eq!(binaries(&crashed), None);
        assert_eq!(binaries("test result: ok. 0 passed\n"), None);
        let cut = out.rsplit_once("test result").unwrap().0;
        assert_eq!(binaries(cut), None);
    }

    #[test]
    fn doc_test_names_lose_their_line() {
        assert_eq!(normalize("src/lib.rs - add (line 12)"), "src/lib.rs - add");
        assert_eq!(
            normalize("src/lib.rs - Calc::div (line 30) - should panic"),
            "src/lib.rs - Calc::div"
        );
        assert_eq!(normalize("tests::adds"), "tests::adds");
    }
}
