//! JUnit XML reports into per-test results (ARCHITECTURE.md §1.9.1, §1.1.4).
//!
//! Each `<testcase>` becomes one [`TestResult`] whose id is
//! `<check>::<suite chain> > <name>`: the names of the enclosing
//! `<testsuite>` elements (outermost first, unnamed ones skipped) and the
//! test name, joined by ` > `. `<testsuites>` and `classname` are ignored,
//! because runners fill them inconsistently (Node's reporter writes
//! `classname="test"` for everything). A test path longer than 1024 bytes is
//! truncated on a character boundary.
//!
//! Status is `fail` if the test case has a `<failure>` or `<error>` child (or
//! a `failure` attribute, as Node writes), else `skip` if it has `<skipped>`,
//! else `pass`. A failure's message is its `message` attribute followed by
//! its body, trimmed and bounded to 2 KiB. Test cases that map to the same id
//! (the same name in two files, for runners that do not name suites after
//! files) merge into one result: `fail` beats `pass` beats `skip`, and the
//! first failure's message is kept. Merging rather than numbering keeps the
//! id stable when runners report files in a nondeterministic order.
//!
//! A test case without a name (empty or only whitespace), or with the test
//! path `(exit)`, which the command's own entry uses, makes the report
//! malformed: a runner channel the check controls could otherwise slip in a
//! passing test nobody can name.
//!
//! The root element must be `<testsuites>` or `<testsuite>`; anything else is
//! an error, so a stray file at the report path is not read as zero tests. A
//! report that ends with elements still open (a runner that died mid-write)
//! is an error too, rather than a complete report missing its later tests.
//!
//! Mocha's `xunit` reporter ([`ReportFormat::MochaXunit`]) writes the same
//! XML under one `<testsuite name="Mocha Tests">` and puts each test's suite
//! titles in `classname`, so for it the test path is the `classname` and the
//! name, and suite names are ignored. pytest's `--junitxml`
//! ([`ReportFormat::PytestJunit`]) also puts the module and classes in
//! `classname` (`tests.test_calc.TestDiv`) and, with
//! `junit_family=xunit1`, the file in `file`: the test path is the file,
//! each class, and the name (`tests/test_calc.py > TestDiv > test_zero`),
//! so the first segment names the file that defines the test; without a
//! `file` attribute it is the `classname` and the name. Jest's `--json`
//! output ([`ReportFormat::JestJson`]) is read by [`super::jest`],
//! `cargo test`'s output ([`ReportFormat::Libtest`]) by [`super::libtest`]
//! and `go test -json` ([`ReportFormat::GoJson`]) by [`super::gojson`].

use std::collections::BTreeMap;

use aver_core::run::MAX_TEST_MESSAGE_BYTES;
use aver_core::{CheckId, ReportFormat, Set, TestCheckId, TestResult, TestStatus, Text};
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

pub(super) const MAX_TEST_PATH_BYTES: usize = 1024;
pub const SEPARATOR: &str = " > ";
/// What a run's slot directory reads as in its reports ([`unroot`]).
pub const SRC_ROOT: &str = "<src>";
/// What a run's `TMPDIR` reads as in its reports ([`unroot`]).
pub const TMP_ROOT: &str = "<tmp>";

/// A `format` report with each root (an absolute path, and what it reads
/// as) replaced, as given and resolved, longest first: a test
/// parameterized with `__file__` or a temporary path then has the same id
/// in every slot (REDESIGN.md §4.5). In an XML report the name is escaped,
/// so it stays text. The report is rewritten before it is stored or parsed,
/// so every reading of it agrees. A Cargo probe's stream names no tests and
/// is read for real paths, so it is left as it is.
pub fn unroot(report: &[u8], format: ReportFormat, roots: &[(&std::path::Path, &str)]) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt as _;
    if format == ReportFormat::CargoProbe {
        return report.to_vec();
    }
    let xml = matches!(
        format,
        ReportFormat::Junit | ReportFormat::MochaXunit | ReportFormat::PytestJunit
    );
    let mut paths: Vec<(Vec<u8>, String)> = Vec::new();
    for (path, to) in roots {
        let to = if xml {
            to.replace('<', "&lt;").replace('>', "&gt;")
        } else {
            (*to).to_owned()
        };
        paths.push((path.as_os_str().as_bytes().to_vec(), to.clone()));
        if let Ok(real) = path.canonicalize() {
            paths.push((real.as_os_str().as_bytes().to_vec(), to));
        }
    }
    paths.retain(|(p, _)| p.len() > 1);
    paths.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
    paths.dedup_by(|a, b| a.0 == b.0);
    let mut out = report.to_vec();
    for (from, to) in paths {
        if !out.windows(from.len()).any(|w| w == from.as_slice()) {
            continue;
        }
        let mut next = Vec::with_capacity(out.len());
        let mut rest = out.as_slice();
        while !rest.is_empty() {
            if rest.starts_with(&from) {
                next.extend_from_slice(to.as_bytes());
                rest = &rest[from.len()..];
            } else {
                next.push(rest[0]);
                rest = &rest[1..];
            }
        }
        out = next;
    }
    out
}

/// The test path of a check's command-level entry beside its per-test entries
/// (`<check>::(exit)`, ARCHITECTURE.md §1.9.3), so no test may claim it.
pub const EXIT_TEST_PATH: &str = "(exit)";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JunitError {
    #[error("malformed JUnit XML: {0}")]
    Xml(String),
    #[error("malformed JSON report: {0}")]
    Json(String),
    #[error("malformed libtest output: {0}")]
    Text(String),
    /// The output holds no test results because the build failed: the
    /// compiler's first error.
    #[error("the build failed before any test ran: {0}")]
    Build(String),
    /// The output holds no test results because the toolchain could not
    /// start the tests ([`super::toolchain::classify`]): the line and its fix.
    #[error("the toolchain could not run the tests: {0}")]
    Toolchain(String),
}

/// How a JUnit-like report names its tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The suite chain and the name.
    Junit,
    /// Mocha's `classname` and the name.
    Classname,
    /// pytest's file, classes and name.
    Pytest,
}

/// Where a test case says it lives: its `file` and `line` attributes, as
/// runners such as Node, Jest and Vitest write them. Repository-derived.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Location {
    pub file: Option<String>,
    pub line: Option<u32>,
}

/// Parses a JUnit report produced by check `check`.
pub fn parse(check: &CheckId, xml: &[u8]) -> Result<Set<TestResult>, JunitError> {
    parse_report(check, xml).map(|(tests, _)| tests)
}

/// [`parse`], plus the location of each test case that names one, by test
/// check id (the first case of a merged id wins).
pub fn parse_report(
    check: &CheckId,
    xml: &[u8],
) -> Result<(Set<TestResult>, BTreeMap<String, Location>), JunitError> {
    parse_as(check, xml, ReportFormat::Junit)
}

/// Parses a report of `format` produced by check `check`, with locations.
pub fn parse_as(
    check: &CheckId,
    bytes: &[u8],
    format: ReportFormat,
) -> Result<(Set<TestResult>, BTreeMap<String, Location>), JunitError> {
    let mut locations = BTreeMap::new();
    let tests = match format {
        ReportFormat::Junit => parse_with(check, bytes, &mut locations, Mode::Junit)?,
        ReportFormat::MochaXunit => parse_with(check, bytes, &mut locations, Mode::Classname)?,
        ReportFormat::PytestJunit => parse_with(check, bytes, &mut locations, Mode::Pytest)?,
        ReportFormat::JestJson => super::jest::parse(check, bytes, &mut locations)?,
        ReportFormat::Libtest => super::libtest::parse(check, bytes, &mut locations)?,
        ReportFormat::GoJson => super::gojson::parse(check, bytes, &mut locations)?,
        ReportFormat::Tsc => super::tsc::parse(check, bytes, &mut locations)?,
        ReportFormat::Tap => super::tap::parse(check, bytes, &mut locations)?,
        ReportFormat::CargoProbe => Set::empty(),
    };
    Ok((tests, locations))
}

fn parse_with(
    check: &CheckId,
    xml: &[u8],
    locations: &mut BTreeMap<String, Location>,
    mode: Mode,
) -> Result<Set<TestResult>, JunitError> {
    let by_classname = mode != Mode::Junit;
    let text = String::from_utf8_lossy(xml);
    let mut reader = Reader::from_str(&text);
    let mut suites: Vec<String> = Vec::new();
    let mut case: Option<Case> = None;
    let mut capture: Option<String> = None;
    let mut results: BTreeMap<String, TestResult> = BTreeMap::new();
    let xml_err = |e: quick_xml::Error| JunitError::Xml(e.to_string());
    let mut root_seen = false;
    let mut depth = 0usize;

    loop {
        let event = reader.read_event().map_err(xml_err)?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let open = matches!(event, Event::Start(_));
                depth += usize::from(open);
                let name = e.local_name();
                if !root_seen {
                    root_seen = true;
                    if !matches!(name.as_ref(), "testsuites" | "testsuite") {
                        return Err(JunitError::Xml(format!(
                            "root element is <{}>, not <testsuites> or <testsuite>",
                            name.as_ref()
                        )));
                    }
                }
                match name.as_ref() {
                    "testsuite" if open => suites.push(if by_classname {
                        String::new()
                    } else {
                        attr(e, "name")?.unwrap_or_default()
                    }),
                    "testcase" if open => case = Some(Case::open(e, by_classname)?),
                    "testcase" => insert(
                        &mut results,
                        locations,
                        check,
                        &suites,
                        Case::open(e, by_classname)?,
                        mode,
                    )?,
                    "failure" | "error" => {
                        if let Some(case) = case.as_mut() {
                            case.fail(attr(e, "message")?);
                            if open {
                                capture = Some(String::new());
                            }
                        }
                    }
                    "skipped" => {
                        if let Some(case) = case.as_mut() {
                            case.skipped = true;
                        }
                    }
                    _ => {}
                }
            }
            Event::End(e) => {
                depth = depth.saturating_sub(1);
                match e.local_name().as_ref() {
                    "testsuite" => {
                        suites.pop();
                    }
                    "testcase" => {
                        if let Some(case) = case.take() {
                            insert(&mut results, locations, check, &suites, case, mode)?;
                        }
                    }
                    "failure" | "error" => {
                        if let (Some(case), Some(body)) = (case.as_mut(), capture.take()) {
                            case.add_body(&body);
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(t) => {
                if let Some(buf) = capture.as_mut() {
                    buf.push_str(&t.xml_content(XmlVersion::Implicit1_0));
                }
            }
            Event::CData(t) => {
                if let Some(buf) = capture.as_mut() {
                    buf.push_str(&t.xml_content(XmlVersion::Implicit1_0));
                }
            }
            Event::GeneralRef(r) => {
                if let Some(buf) = capture.as_mut() {
                    match r.resolve_char_ref() {
                        Ok(Some(c)) => buf.push(c),
                        _ => match resolve_predefined_entity(r) {
                            Some(s) => buf.push_str(s),
                            None => {
                                buf.push('&');
                                buf.push_str(r);
                                buf.push(';');
                            }
                        },
                    }
                }
            }
            Event::Eof if depth > 0 => {
                return Err(JunitError::Xml("truncated: unclosed elements".into()))
            }
            Event::Eof if root_seen => break,
            Event::Eof => return Err(JunitError::Xml("no <testsuites> element".into())),
            _ => {}
        }
    }
    Set::new(results.into_values().collect()).map_err(|e| JunitError::Xml(e.to_string()))
}

struct Case {
    /// Mocha's suite titles, read only for [`ReportFormat::MochaXunit`].
    classname: String,
    name: String,
    failed: bool,
    skipped: bool,
    message: Option<String>,
    location: Location,
}

impl Case {
    fn open(e: &BytesStart<'_>, by_classname: bool) -> Result<Self, JunitError> {
        let mut case = Self {
            classname: if by_classname {
                attr(e, "classname")?.unwrap_or_default()
            } else {
                String::new()
            },
            name: attr(e, "name")?.unwrap_or_default(),
            failed: false,
            skipped: false,
            message: None,
            location: Location {
                file: attr(e, "file")?.filter(|f| !f.is_empty()),
                line: attr(e, "line")?.and_then(|l| l.trim().parse().ok()),
            },
        };
        if let Some(message) = attr(e, "failure")? {
            case.fail(Some(message));
        }
        Ok(case)
    }

    fn fail(&mut self, message: Option<String>) {
        self.failed = true;
        if let Some(message) = message.filter(|m| !m.trim().is_empty()) {
            if self.message.is_none() {
                self.message = Some(message.trim().to_owned());
            }
        }
    }

    fn add_body(&mut self, body: &str) {
        let body = body.trim();
        if body.is_empty() {
            return;
        }
        self.message = Some(match self.message.take() {
            Some(m) if body.contains(m.as_str()) => body.to_owned(),
            Some(m) => format!("{m}\n{body}"),
            None => body.to_owned(),
        });
    }

    fn status(&self) -> TestStatus {
        if self.failed {
            TestStatus::Fail
        } else if self.skipped {
            TestStatus::Skip
        } else {
            TestStatus::Pass
        }
    }
}

fn insert(
    results: &mut BTreeMap<String, TestResult>,
    locations: &mut BTreeMap<String, Location>,
    check: &CheckId,
    suites: &[String],
    case: Case,
    mode: Mode,
) -> Result<(), JunitError> {
    let pytest = (mode == Mode::Pytest)
        .then(|| pytest_path(&case.classname, case.location.file.as_deref()))
        .flatten();
    let mut path: Vec<&str> = match &pytest {
        Some(segments) => segments.iter().map(String::as_str).collect(),
        None => suites
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(case.classname.as_str()))
            .filter(|s| !s.trim().is_empty())
            .collect(),
    };
    if case.name.trim().is_empty() {
        return Err(JunitError::Xml("a test case has an empty name".into()));
    }
    path.push(case.name.as_str());
    let status = case.status();
    add(
        results,
        locations,
        check,
        &path,
        status,
        case.message.as_deref(),
        case.location,
    )
    .map_err(JunitError::Xml)
}

/// The message pytest's JUnit report gives a test module it could not
/// collect (an import error in the file or in what it imports).
pub const COLLECTION_FAILURE: &str = "collection failure";

/// Whether `result` is pytest's entry for a test module that failed to
/// collect: named after the module (`tests/test_x.py > tests.test_x`), no
/// test function's name, with the message [`COLLECTION_FAILURE`]. The
/// module's own tests are unknown on that side.
pub fn collection_error(result: &TestResult) -> bool {
    let name = result.id.test_path().rsplit(SEPARATOR).next().unwrap_or("");
    result.status == TestStatus::Fail
        && name.contains('.')
        && !name.contains('[')
        && result
            .message
            .as_ref()
            .is_some_and(|m| m.as_str().starts_with(COLLECTION_FAILURE))
}

/// pytest's test path before the name: the `file` the test case names, then
/// each class in `classname` after the module `file` implies
/// (`tests/test_calc.py` is module `tests.test_calc`). `None` without a
/// Python file, or when `classname` is not under that module.
fn pytest_path(classname: &str, file: Option<&str>) -> Option<Vec<String>> {
    let file = file?;
    let module = file.strip_suffix(".py")?.replace('/', ".");
    let classes = match classname.strip_prefix(module.as_str()) {
        _ if classname.is_empty() => "",
        Some("") => "",
        Some(rest) => rest.strip_prefix('.')?,
        None => return None,
    };
    Some(
        std::iter::once(file.to_owned())
            .chain(
                classes
                    .split('.')
                    .filter(|c| !c.is_empty())
                    .map(str::to_owned),
            )
            .collect(),
    )
}

/// Adds one test result under the test path `path` (joined with ` > `),
/// merging with an earlier result of the same id: `fail` beats `pass` beats
/// `skip`, and the first failure message is kept.
pub(super) fn add(
    results: &mut BTreeMap<String, TestResult>,
    locations: &mut BTreeMap<String, Location>,
    check: &CheckId,
    path: &[&str],
    status: TestStatus,
    message: Option<&str>,
    location: Location,
) -> Result<(), String> {
    let path = path.join(SEPARATOR);
    if path == EXIT_TEST_PATH {
        return Err(format!(
            "a test case is named {EXIT_TEST_PATH:?}, which is reserved for the command's exit entry"
        ));
    }
    let id =
        TestCheckId::new(check, truncate(&path, MAX_TEST_PATH_BYTES)).map_err(|e| e.to_string())?;
    let message = match status {
        TestStatus::Fail => message.map(Text::<MAX_TEST_MESSAGE_BYTES>::truncated),
        _ => None,
    };
    let result = TestResult {
        id,
        status,
        message,
    };
    let key = result.id.as_str().to_owned();
    if location != Location::default() {
        locations.entry(key.clone()).or_insert(location);
    }
    match results.get(&key) {
        Some(existing) if rank(existing.status) >= rank(result.status) => {}
        _ => {
            results.insert(key, result);
        }
    }
    Ok(())
}

fn rank(status: TestStatus) -> u8 {
    match status {
        TestStatus::Skip => 0,
        TestStatus::Pass => 1,
        TestStatus::Fail => 2,
    }
}

fn truncate(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn attr(e: &BytesStart<'_>, name: &str) -> Result<Option<String>, JunitError> {
    let found = e
        .try_get_attribute(name)
        .map_err(|err| JunitError::Xml(err.to_string()))?;
    found
        .map(|a| {
            a.normalized_value(XmlVersion::Implicit1_0)
                .map(|v| v.into_owned())
                .map_err(|err| JunitError::Xml(err.to_string()))
        })
        .transpose()
}
