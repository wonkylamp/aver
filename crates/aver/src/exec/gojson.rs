//! `go test -json` events into per-test results (ARCHITECTURE.md §1.9.1):
//! the `go-json` report format, which needs no tool beyond the Go
//! toolchain.
//!
//! The report is one JSON object per line (`test2json` events). An event
//! with a `Test` and the action `pass`, `fail` or `skip` is that test's
//! result; its path is the package's import path and the test name
//! (`example.com/calc > TestAdd/negative`), so a subtest is a test of its
//! own beside its parent. A failure's message is the test's `output`
//! events, without Go's `=== RUN`, `=== PAUSE` and `=== CONT` lines.
//!
//! A package that failed without a failing test (it did not build, a
//! `TestMain` or `init` failed, or it timed out) is one failing entry
//! `<package> > (package)` whose message is its build or package output, so
//! a package the change broke fails on its own and not only through its
//! tests missing from the head's report. Lines that are not JSON objects
//! are skipped; a report without a single event is malformed, or, when it
//! holds a toolchain error (a Go version or module error), that error.

use std::collections::BTreeMap;

use aver_core::run::MAX_TEST_MESSAGE_BYTES;
use aver_core::{CheckId, Set, TestResult, TestStatus};
use serde::Deserialize;

use super::junit::{add, JunitError, Location};

/// The test name of a package-level failure.
pub const PACKAGE_FAILED: &str = "(package)";

#[derive(Deserialize)]
struct Event {
    #[serde(rename = "Action", default)]
    action: String,
    #[serde(rename = "Package", default)]
    package: String,
    #[serde(rename = "ImportPath", default)]
    import_path: String,
    #[serde(rename = "Test", default)]
    test: Option<String>,
    #[serde(rename = "Output", default)]
    output: String,
}

pub fn parse(
    check: &CheckId,
    json: &[u8],
    locations: &mut BTreeMap<String, Location>,
) -> Result<Set<TestResult>, JunitError> {
    let text = String::from_utf8_lossy(json);
    let mut events = 0usize;
    let mut tests: Vec<(String, String, TestStatus)> = Vec::new();
    let mut output: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut failed_packages: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Event>(line) else {
            continue;
        };
        events += 1;
        // Build events name the package being built as `pkg [pkg.test]`.
        let package = if event.package.is_empty() {
            event
                .import_path
                .split(" [")
                .next()
                .unwrap_or_default()
                .to_owned()
        } else {
            event.package.clone()
        };
        if package.is_empty() {
            continue;
        }
        let test = event.test.clone().unwrap_or_default();
        match event.action.as_str() {
            "output" | "build-output" => {
                let kept = output.entry((package, test)).or_default();
                let noise = ["=== RUN", "=== PAUSE", "=== CONT", "=== NAME"]
                    .iter()
                    .any(|p| event.output.trim_start().starts_with(p));
                if !noise && kept.len() < MAX_TEST_MESSAGE_BYTES * 2 {
                    kept.push_str(&event.output);
                }
            }
            "pass" | "fail" | "skip" if !test.is_empty() => {
                let status = match event.action.as_str() {
                    "pass" => TestStatus::Pass,
                    "fail" => TestStatus::Fail,
                    _ => TestStatus::Skip,
                };
                tests.push((package, test, status));
            }
            "fail" | "build-fail" => failed_packages.push(package),
            _ => {}
        }
    }
    if events == 0 {
        if let Some(toolchain) = super::toolchain::classify(&text) {
            return Err(JunitError::Toolchain(toolchain.to_string()));
        }
        return Err(JunitError::Json("no go test events".into()));
    }
    let mut results: BTreeMap<String, TestResult> = BTreeMap::new();
    for (package, test, status) in &tests {
        if test.trim().is_empty() {
            return Err(JunitError::Json("a test has an empty name".into()));
        }
        let message = output
            .get(&(package.clone(), test.clone()))
            .map(|m| m.trim().to_owned())
            .filter(|m| !m.is_empty());
        add(
            &mut results,
            locations,
            check,
            &[package.as_str(), test.as_str()],
            *status,
            message.as_deref(),
            Location::default(),
        )
        .map_err(JunitError::Json)?;
    }
    failed_packages.sort();
    failed_packages.dedup();
    for package in failed_packages {
        let has_failing_test = tests
            .iter()
            .any(|(p, _, s)| *p == package && *s == TestStatus::Fail);
        if has_failing_test {
            continue;
        }
        let message = output
            .get(&(package.clone(), String::new()))
            .map(|m| m.trim().to_owned())
            .filter(|m| !m.is_empty());
        add(
            &mut results,
            locations,
            check,
            &[package.as_str(), PACKAGE_FAILED],
            TestStatus::Fail,
            message.as_deref(),
            Location::default(),
        )
        .map_err(JunitError::Json)?;
    }
    Set::new(results.into_values().collect()).map_err(|e| JunitError::Json(e.to_string()))
}

/// The packages a `go test -json` stream ended with a package-level `fail`,
/// sorted: a failing test, a panic that cut the binary off, or a build or
/// `TestMain` failure.
pub fn failed_packages(json: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = String::from_utf8_lossy(json)
        .lines()
        .filter_map(|line| serde_json::from_str::<Event>(line.trim()).ok())
        .filter(|e| e.action == "fail" && e.test.is_none() && !e.package.is_empty())
        .map(|e| e.package)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The top-level tests a `_test.go` file declares: `func TestX(` at the
/// start of a line outside comments and string literals, where what
/// follows `Test` does not start with a lower-case letter, as `go test`
/// finds them, without `TestMain`.
pub fn declared_tests(source: &str) -> Vec<String> {
    code_only(source)
        .lines()
        .filter_map(|line| line.strip_prefix("func Test"))
        .filter_map(|rest| {
            let (name, _) = rest.split_once('(')?;
            let ok = !name.starts_with(|c: char| c.is_lowercase())
                && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                && name != "Main";
            ok.then(|| format!("Test{name}"))
        })
        .collect()
}

/// `source` with its comments and string and rune literals blanked, line
/// breaks kept.
fn code_only(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut star = false;
                for c in chars.by_ref() {
                    out.push(blank(c));
                    if star && c == '/' {
                        break;
                    }
                    star = c == '*';
                }
            }
            '`' => {
                out.push(' ');
                for c in chars.by_ref() {
                    out.push(blank(c));
                    if c == '`' {
                        break;
                    }
                }
            }
            '"' | '\'' => {
                out.push(' ');
                while let Some(d) = chars.next() {
                    if d == '\n' {
                        out.push('\n');
                        break;
                    }
                    out.push(' ');
                    if d == '\\' {
                        chars.next();
                        out.push(' ');
                    } else if d == c {
                        break;
                    }
                }
            }
            c => out.push(c),
        }
    }
    out
}

const KNOWN_OS: &[&str] = &[
    "aix",
    "android",
    "darwin",
    "dragonfly",
    "freebsd",
    "hurd",
    "illumos",
    "ios",
    "js",
    "linux",
    "nacl",
    "netbsd",
    "openbsd",
    "plan9",
    "solaris",
    "wasip1",
    "windows",
    "zos",
];
const KNOWN_ARCH: &[&str] = &[
    "386",
    "amd64",
    "amd64p32",
    "arm",
    "armbe",
    "arm64",
    "arm64be",
    "loong64",
    "mips",
    "mipsle",
    "mips64",
    "mips64le",
    "mips64p32",
    "mips64p32le",
    "ppc",
    "ppc64",
    "ppc64le",
    "riscv",
    "riscv64",
    "s390",
    "s390x",
    "sparc",
    "sparc64",
    "wasm",
];
const UNIX: &[&str] = &[
    "aix",
    "android",
    "darwin",
    "dragonfly",
    "freebsd",
    "hurd",
    "illumos",
    "ios",
    "linux",
    "netbsd",
    "openbsd",
    "solaris",
];

/// The `GOOS` and `GOARCH` of this host, as Go names them.
pub fn host() -> (&'static str, &'static str) {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        "x86" => "386",
        "powerpc64" => "ppc64",
        "loongarch64" => "loong64",
        arch => arch,
    };
    (os, arch)
}

/// Whether `go test` builds the file `name` with contents `source` for
/// `os` and `arch` with build `tags`: its `_GOOS`, `_GOARCH` or
/// `_GOOS_GOARCH` name suffix and its `//go:build` line allow it. A
/// constraint that depends on `cgo`, or an old `// +build` line without a
/// `//go:build` one, cannot be decided from the text and counts as not
/// built, so a file the toolchain may leave out never counts as declared.
pub fn builds(name: &str, source: &str, (os, arch): (&str, &str), tags: &[String]) -> bool {
    if name.starts_with(['_', '.']) {
        return false;
    }
    let stem = name.split('.').next().unwrap_or(name);
    let mut parts: Vec<&str> = stem.split('_').skip(1).collect();
    if parts.last() == Some(&"test") {
        parts.pop();
    }
    let matches_os = |o: &str| {
        o == os
            || (o == "linux" && os == "android")
            || (o == "solaris" && os == "illumos")
            || (o == "darwin" && os == "ios")
    };
    match parts.as_slice() {
        [.., o, a] if KNOWN_OS.contains(o) && KNOWN_ARCH.contains(a) => {
            if !matches_os(o) || *a != arch {
                return false;
            }
        }
        [.., last] if KNOWN_OS.contains(last) && !matches_os(last) => return false,
        [.., last] if KNOWN_OS.contains(last) => {}
        [.., last] if KNOWN_ARCH.contains(last) && *last != arch => return false,
        _ => {}
    }
    let mut plus_build = false;
    for line in source.lines() {
        let line = line.trim();
        if let Some(expr) = line.strip_prefix("//go:build") {
            let tag = |t: &str| -> Option<bool> {
                match t {
                    "cgo" => None,
                    "gc" => Some(true),
                    "unix" => Some(UNIX.contains(&os)),
                    t if t.starts_with("go1.") => Some(true),
                    t => Some(matches_os(t) || t == arch || tags.iter().any(|x| x == t)),
                }
            };
            return Constraint::eval(expr, &tag) == Some(true);
        }
        if line.starts_with("// +build") {
            plus_build = true;
        } else if !(line.is_empty() || line.starts_with("//")) {
            break;
        }
    }
    !plus_build
}

/// A `//go:build` expression, evaluated as it is parsed, with an
/// undecidable tag `None` (three-valued, so `x || true` is still true).
struct Constraint<'a, 't> {
    tokens: Vec<&'a str>,
    at: usize,
    tag: &'t dyn Fn(&str) -> Option<bool>,
}

impl<'a> Constraint<'a, '_> {
    fn eval(expr: &'a str, tag: &dyn Fn(&str) -> Option<bool>) -> Option<bool> {
        let mut tokens = Vec::new();
        let mut rest = expr.trim();
        while !rest.is_empty() {
            let len = if rest.starts_with("&&") || rest.starts_with("||") {
                2
            } else if rest.starts_with(['(', ')', '!']) {
                1
            } else {
                rest.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
                    .unwrap_or(rest.len())
                    .max(1)
            };
            tokens.push(&rest[..len]);
            rest = rest[len..].trim_start();
        }
        let mut c = Constraint { tokens, at: 0, tag };
        let value = c.or()?;
        (c.at == c.tokens.len()).then_some(value)?
    }

    fn next(&mut self) -> Option<&'a str> {
        let t = self.tokens.get(self.at).copied();
        self.at += 1;
        t
    }

    fn or(&mut self) -> Option<Option<bool>> {
        let mut value = self.and()?;
        while self.tokens.get(self.at) == Some(&"||") {
            self.at += 1;
            let rhs = self.and()?;
            value = match (value, rhs) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            };
        }
        Some(value)
    }

    fn and(&mut self) -> Option<Option<bool>> {
        let mut value = self.not()?;
        while self.tokens.get(self.at) == Some(&"&&") {
            self.at += 1;
            let rhs = self.not()?;
            value = match (value, rhs) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            };
        }
        Some(value)
    }

    fn not(&mut self) -> Option<Option<bool>> {
        match self.next()? {
            "!" => Some(self.not()?.map(|v| !v)),
            "(" => {
                let value = self.or()?;
                (self.next()? == ")").then_some(value)
            }
            t if t
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '.') =>
            {
                Some((self.tag)(t))
            }
            _ => None,
        }
    }
}

/// The build tags a Go check's command passes with `-tags`.
pub fn tags_of(argv: &[String]) -> Vec<String> {
    let words: Vec<&str> = argv.iter().flat_map(|a| a.split_whitespace()).collect();
    let mut out = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let w = w.trim_start_matches('-');
        let value = match w.strip_prefix("tags") {
            Some(v) if v.starts_with('=') => Some(&v[1..]),
            Some("") => words.get(i + 1).copied(),
            _ => None,
        };
        if let Some(v) = value {
            out.extend(
                v.trim_matches(['\'', '"'])
                    .split([',', ' '])
                    .filter(|t| !t.is_empty())
                    .map(str::to_owned),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolchain_output_without_events_names_the_error() {
        let check: CheckId = "go".parse().unwrap();
        let out = "go: go.mod requires go >= 1.23 (running go 1.21.5; GOTOOLCHAIN=local)\n";
        let err = parse(&check, out.as_bytes(), &mut BTreeMap::new()).unwrap_err();
        assert!(
            matches!(&err, JunitError::Toolchain(e) if e.starts_with("go: go.mod requires go >= 1.23") && e.contains("fix: install the Go version")),
            "{err}"
        );
        assert!(!err.to_string().contains("malformed"));
        assert!(matches!(
            parse(&check, b"garbage\n", &mut BTreeMap::new()),
            Err(JunitError::Json(_))
        ));
    }

    #[test]
    fn declared_tests_are_the_top_level_test_functions() {
        let src = "package calc\n\nfunc TestMain(m *testing.M) {}\nfunc TestAdd(t *testing.T) {}\nfunc Test_sub(t *testing.T) {}\nfunc Testing(t *testing.T) {}\nfunc helper() {}\n\tfunc TestNested(t *testing.T) {}\n";
        assert_eq!(declared_tests(src), ["TestAdd", "Test_sub"]);
    }

    #[test]
    fn tests_in_comments_and_strings_are_not_declared() {
        let src = "package gin\n\n/* legacy tests\nfunc TestOld(t *testing.T) {}\n*/\n// func TestCommented(t *testing.T) {}\nvar fixture = `\nfunc TestInRaw(t *testing.T) {}\n`\nfunc TestLive(t *testing.T) { s := \"/*\"; _ = s }\nfunc TestAfter(t *testing.T) {}\n";
        assert_eq!(declared_tests(src), ["TestLive", "TestAfter"]);
    }

    #[test]
    fn files_go_test_leaves_out_here_are_not_built() {
        let mac = ("darwin", "arm64");
        let linux = ("linux", "amd64");
        let none: &[String] = &[];
        let plain = "package util\n";
        assert!(builds("util_test.go", plain, mac, none));
        assert!(!builds("util_windows_test.go", plain, mac, none));
        assert!(builds(
            "util_windows_test.go",
            plain,
            ("windows", "amd64"),
            none
        ));
        assert!(!builds("util_linux_amd64_test.go", plain, mac, none));
        assert!(builds("util_linux_amd64_test.go", plain, linux, none));
        assert!(!builds("util_arm64_test.go", plain, linux, none));
        assert!(
            builds("windows_test.go", plain, mac, none),
            "the first word is a name"
        );
        assert!(!builds("_scratch_test.go", plain, mac, none));
        let tagged = |expr: &str| format!("// Copyright\n\n//go:build {expr}\n\npackage util\n");
        assert!(!builds("a_test.go", &tagged("integration"), mac, none));
        assert!(builds(
            "a_test.go",
            &tagged("integration"),
            mac,
            &["integration".into()]
        ));
        assert!(builds("a_test.go", &tagged("!windows"), mac, none));
        assert!(!builds("a_test.go", &tagged("windows || plan9"), mac, none));
        assert!(builds(
            "a_test.go",
            &tagged("unix && !(js || wasip1)"),
            mac,
            none
        ));
        assert!(builds("a_test.go", &tagged("go1.21"), mac, none));
        assert!(
            !builds("a_test.go", &tagged("cgo"), mac, none),
            "undecidable"
        );
        assert!(builds("a_test.go", &tagged("cgo || darwin"), mac, none));
        assert!(
            !builds("a_test.go", &tagged("pprof &&"), mac, none),
            "malformed"
        );
        assert!(!builds(
            "a_test.go",
            "// +build linux\n\npackage util\n",
            mac,
            none
        ));
        assert!(
            builds(
                "a_test.go",
                "package util\n\n//go:build windows\n",
                mac,
                none
            ),
            "after the package clause it is a comment"
        );
    }

    #[test]
    fn tags_come_from_the_command() {
        let argv = |s: &str| vec!["sh".to_owned(), "-c".to_owned(), s.to_owned()];
        assert_eq!(
            tags_of(&argv("go test -json -tags=integration,pprof ./...")),
            ["integration", "pprof"]
        );
        assert_eq!(tags_of(&argv("go test --tags 'a b' ./...")), ["a"]);
        assert!(tags_of(&argv("go test -json ./...")).is_empty());
    }

    #[test]
    fn failed_packages_are_the_package_level_fails() {
        let json = br#"{"Action":"run","Package":"m/a","Test":"TestA"}
{"Action":"fail","Package":"m/a","Test":"TestA"}
{"Action":"fail","Package":"m/a"}
{"Action":"pass","Package":"m/b"}
"#;
        assert_eq!(failed_packages(json), ["m/a"]);
    }
}
