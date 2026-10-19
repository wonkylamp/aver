//! A scripted runner whose behavior is read from the snapshot it is given,
//! so verification and task flows can be tested without running commands.
//! It exists only with the `scripted` feature, which `cargo test` turns on;
//! the `aver` binary then uses it when `AVER_SCRIPTED=1` is set
//! ([`crate::services::Services::open`]), appending each run's label to
//! [`LOG`] in the home so a test can count the runs of other processes.
//!
//! A check's `argv` picks the behavior:
//!
//! - `["junit", "<report file>", "<script file>"]` reads the JSON object in
//!   `<script file>` (test name to `"pass"`, `"fail"`, `"skip"` or `"flaky"`,
//!   which fails on run index 1 and passes after), writes a JUnit report to
//!   `/aver/out/<report file>` and exits 1 if any test failed. A missing
//!   script file exits 2 with no report, like a crash before the runner.
//!   `{nonce}` in a test name becomes a number new to each run, as a
//!   parameter built from the clock or a pointer would.
//! - `["junit-exit", "<report file>", "<script file>", "<code file>"]` writes
//!   the report like `junit` but exits with the code written in `<code file>`
//!   whatever the tests did, as a runner that forges its report would.
//! - `["junit-dir", "<report file>", "<dir>", …]` (later arguments, such as
//!   test globs, are ignored) merges every
//!   `<dir>/**/*.test.json` script into one report, so test files can be
//!   added like a runner discovers them. Besides the `junit` behaviors, a
//!   test may be `"pass-if:<path>"` (passes when `<path>` exists in the
//!   snapshot), `"only-if:<path>"` (passes when `<path>` exists, else is
//!   left out of the report, as a test file that cannot load), and
//!   `"timeout-unless:<path>"` makes the whole run time out when `<path>` is
//!   missing.
//!   More test behaviors: `"agree:<a>=<b>"` passes when both files exist
//!   with the same contents, `"contains:<path>:<text>"` when the file holds
//!   the text, `"flaky-if:<path>"` fails on run index 1 when `<path>`
//!   exists (else passes), `"race:<path>"` fails on run index 1 when
//!   `<path>` exists and on index 2 when it does not, `"load-if:<path>"`
//!   fails as a module that cannot load (`ERR_MODULE_NOT_FOUND`) when
//!   `<path>` is missing, `"name-if:<path>"` fails as a call of a function
//!   the original code lacks (`TypeError: … is not a function`) when
//!   `<path>` is missing, and a key
//!   `"!force-pass:<name>"` makes test `<name>` pass, as a test module that
//!   patches the code under test.
//! - `["tap-dir", "<report file>", "<dir>"]` writes the TAP stream `ava
//!   --tap` prints for every `<dir>/**/*.test.json` script, each title
//!   prefixed with its file's name (less `.test.json`) when there are
//!   several, as ava does: `"pass"`, `"fail"`, `"skip"`, `"todo"` and
//!   `"pass-if:<path>"`. Exits 1 if any test failed.
//! - `["junit-oversized-if", "<report file>", "<script file>", "<path>"]`
//!   runs like `junit`, and when `<path>` exists pads the valid report past
//!   [`aver_core::MAX_BLOB_BYTES`].
//! - `["copy-report", "<report file>", "<source file>"]` copies `<source file>`
//!   to `/aver/out/<report file>` and exits 2 when it holds anything, else 0,
//!   as `tsc` does with its errors.
//! - `["copy-junit", "<report file>", "<source file>"]` copies `<source
//!   file>` to `/aver/out/<report file>` and exits 1 when it holds a
//!   `<failure` or `<error`, else 0, as a runner whose report the test
//!   writes out in full (pytest's collection errors).
//! - `["exit", "<code>"]` exits with that code and no report.
//! - `["say", "<path>", "<code>"]` prints the file at `<path>` and exits
//!   with `<code>`, with no report: a toolchain that refuses to start the
//!   tests.
//! - `["exit-if", "<path>"]` exits 1 if `<path>` exists in the snapshot, else 0.
//! - `["sandbox-fail-if", "<path>"]` fails as a sandbox error (a malformed
//!   result) if `<path>` exists, else exits 0.
//! - `["fail-or-sandbox-if", "<path>"]` fails as a sandbox error if `<path>`
//!   exists, else exits 1: a red check whose head side ends in infra.
//!   `["fail-or-sandbox-unless", "<path>"]` is the other way round.
//! - `["exit-unless-var", "<name>", "<value>"]` exits 0 if the sandbox's
//!   environment sets `<name>` to `<value>`, else 1.
//! - `["timeout"]` ends as a timeout; `["oom"]` runs out of memory.
//! - `["timeout-unless", "<path>"]` ends as a timeout unless `<path>` exists,
//!   else exits 0.
//! - `["npm", "test", …]` (the default environment's check) exits 0.
//! - `["contains", "<path>", "<text>"]` exits 0 if the file at `<path>`
//!   contains `<text>`, else 1.
use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use super::output::{Exit, SandboxOutput};
use super::seam::{BoxFuture, Sandbox, SandboxError, SandboxRequest};

/// The file under `$AVER_HOME` a logging runner appends each run's label to.
pub const LOG: &str = "scripted-runs.log";

#[derive(Default)]
pub struct Scripted {
    pub runs: AtomicUsize,
    /// Every request received, in order.
    pub requests: Mutex<Vec<SandboxRequest>>,
    log: Option<PathBuf>,
}

impl Scripted {
    /// A runner that also appends each run's label to `home`'s [`LOG`].
    pub fn logging(home: &std::path::Path) -> Self {
        Self {
            log: Some(home.join(LOG)),
            ..Self::default()
        }
    }

    /// How many runs processes logged to `home`'s [`LOG`].
    pub fn logged(home: &std::path::Path) -> usize {
        std::fs::read_to_string(home.join(LOG)).map_or(0, |t| t.lines().count())
    }

    pub fn runs(&self) -> usize {
        self.runs.load(Ordering::SeqCst)
    }

    pub fn last_request(&self) -> SandboxRequest {
        self.requests.lock().unwrap().last().cloned().unwrap()
    }
}

/// The snapshot's files of a request, by path.
pub fn files(input: &BTreeMap<String, Vec<u8>>) -> HashMap<String, Vec<u8>> {
    input.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

fn junit(results: &BTreeMap<String, String>, attempt: &str) -> (String, bool) {
    junit_in(results, attempt, &HashMap::new())
}

fn junit_in(
    results: &BTreeMap<String, String>,
    attempt: &str,
    tree: &HashMap<String, Vec<u8>>,
) -> (String, bool) {
    let mut failed = false;
    let mut xml = String::from("<testsuites>\n");
    // `!force-pass:<name>` in any script makes that test pass, as an added
    // test module that patches the code under test would.
    let forced: Vec<&str> = results
        .keys()
        .filter_map(|k| k.strip_prefix("!force-pass:"))
        .collect();
    let text = |path: &str| {
        tree.get(path)
            .map(|b| String::from_utf8_lossy(b).into_owned())
    };
    for (name, behavior) in results {
        if name.starts_with('!') {
            continue;
        }
        let fails = match behavior.as_str() {
            _ if forced.contains(&name.as_str()) => false,
            "fail" => true,
            "flaky" => attempt == "1",
            other => {
                if let Some(path) = other
                    .strip_prefix("pass-if:")
                    .or_else(|| other.strip_prefix("load-if:"))
                    .or_else(|| other.strip_prefix("name-if:"))
                {
                    !tree.contains_key(path)
                } else if let Some(rest) = other.strip_prefix("say-if:") {
                    !tree.contains_key(rest.split_once(':').unwrap().0)
                } else if let Some(pair) = other.strip_prefix("agree:") {
                    let (a, b) = pair.split_once('=').unwrap();
                    text(a).is_none() || text(a) != text(b)
                } else if let Some(rest) = other.strip_prefix("contains:") {
                    let (path, needle) = rest.split_once(':').unwrap();
                    !text(path).is_some_and(|t| t.contains(needle))
                } else if let Some(path) = other.strip_prefix("flaky-if:") {
                    tree.contains_key(path) && attempt == "1"
                } else if let Some(path) = other.strip_prefix("race:") {
                    attempt == if tree.contains_key(path) { "1" } else { "2" }
                } else {
                    false
                }
            }
        };
        failed |= fails;
        let body = if behavior == "skip" {
            "<skipped/>".to_owned()
        } else if let Some(path) = behavior.strip_prefix("load-if:").filter(|_| fails) {
            format!(
                "<failure message=\"Error [ERR_MODULE_NOT_FOUND]: Cannot find module '{path}'\"/>"
            )
        } else if let Some((_, message)) = behavior
            .strip_prefix("say-if:")
            .and_then(|r| r.split_once(':'))
            .filter(|_| fails)
        {
            // `say-if:<path>:<message>` fails with `message` unless `path`
            // exists.
            format!("<failure message=\"{message}\"/>")
        } else if behavior.starts_with("name-if:") && fails {
            "<failure message=\"TypeError: m.negate is not a function\"/>".to_owned()
        } else if fails {
            format!("<failure message=\"{name} failed\"/>")
        } else {
            String::new()
        };
        xml.push_str(&format!("  <testcase name=\"{name}\">{body}</testcase>\n"));
    }
    xml.push_str("</testsuites>\n");
    (xml, failed)
}

/// Ava's TAP stream for the `<dir>/**/*.test.json` scripts in `tree`, and
/// whether a test failed.
fn tap_dir(tree: &HashMap<String, Vec<u8>>, dir: &str) -> (String, bool) {
    let mut scripts: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for (path, bytes) in tree {
        if let Some(name) = path
            .strip_prefix(&format!("{dir}/"))
            .and_then(|p| p.strip_suffix(".test.json"))
        {
            scripts.insert(name.to_owned(), serde_json::from_slice(bytes).unwrap());
        }
    }
    let several = scripts.len() > 1;
    let (mut tap, mut failed, mut n) = (String::from("TAP version 13\n"), false, 0);
    for (file, tests) in &scripts {
        for (name, behavior) in tests {
            n += 1;
            let title = if several {
                format!("{} \u{203a} {name}", file.replace('/', " \u{203a} "))
            } else {
                name.clone()
            };
            let fails = match behavior.as_str() {
                "fail" => true,
                other => other
                    .strip_prefix("pass-if:")
                    .is_some_and(|path| !tree.contains_key(path)),
            };
            failed |= fails;
            tap.push_str(&match behavior.as_str() {
                "skip" => format!("ok {n} - {title} # SKIP\n"),
                "todo" => format!("not ok {n} - {title} # TODO\n"),
                _ if fails => format!(
                    "not ok {n} - {title}\n  ---\n    name: AssertionError\n    message: {name} failed\n  ...\n"
                ),
                _ => format!("ok {n} - {title}\n"),
            });
        }
    }
    tap.push_str(&format!("\n1..{n}\n"));
    (tap, failed)
}

/// The run's output and exit code once its setup commands ran first, as
/// the wrapper reports them: `["findings", "<file>"]` prints the file and
/// fails when it holds anything, as `mypy` or `go vet` with findings, and
/// `["build", "<artifact>", "<file>"]` leaves a built test file
/// `<artifact>` whose content is `<file>`'s (empty when it is missing), as
/// a build step that compiles tests would.
fn with_setup(
    setup: &[Vec<String>],
    tree: &HashMap<String, Vec<u8>>,
    mut out: SandboxOutput,
) -> SandboxOutput {
    if setup.is_empty() {
        return out;
    }
    let n = setup.len();
    let mut prefix = String::new();
    let mut failed = None;
    for (i, step) in setup.iter().enumerate() {
        let steps: Vec<&str> = step.iter().map(String::as_str).collect();
        if let ["build", artifact, file] = steps.as_slice() {
            let bytes = tree.get(*file).map_or(&[][..], Vec::as_slice);
            out.built
                .get_or_insert_with(BTreeMap::new)
                .insert((*artifact).to_owned(), aver_core::BlobId::of_bytes(bytes));
            continue;
        }
        let ["findings", file] = steps.as_slice() else {
            panic!("the scripted sandbox has no setup behavior for {step:?}");
        };
        let text = tree
            .get(*file)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        if !text.trim().is_empty() {
            prefix.push_str(&text);
            prefix.push_str(&format!(
                "[aver] setup command {} of {n} exited with 1: {}\n",
                i + 1,
                steps.join(" ")
            ));
            failed.get_or_insert(i + 1);
        }
    }
    let mut output = prefix.into_bytes();
    output.extend_from_slice(&out.output);
    if let Some(step) = failed {
        output.extend_from_slice(
            format!("[aver] setup command {step} of {n} exited with 1\n").as_bytes(),
        );
        if out.exit == Exit::Exited(0) {
            out.exit = Exit::Exited(1);
        }
    }
    out.output = output;
    out
}

fn exited(code: i32, output: &str) -> SandboxOutput {
    SandboxOutput {
        exit: Exit::Exited(code),
        output: output.as_bytes().to_vec(),
        files: BTreeMap::new(),
        duration: Duration::from_millis(5),
        built: None,
    }
}

impl Sandbox for Scripted {
    fn executor(&self) -> &str {
        "scripted-v0"
    }

    fn run(&self, request: SandboxRequest) -> BoxFuture<'_, Result<SandboxOutput, SandboxError>> {
        Box::pin(async move {
            let nonce = self.runs.fetch_add(1, Ordering::SeqCst).to_string();
            let named = |results: BTreeMap<String, String>| -> BTreeMap<String, String> {
                results
                    .into_iter()
                    .map(|(name, b)| (name.replace("{nonce}", &nonce), b))
                    .collect()
            };
            self.requests.lock().unwrap().push(request.clone());
            if let Some(log) = &self.log {
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(log)
                    .map_err(|e| SandboxError::Invalid(e.to_string()))?;
                let _ = writeln!(file, "{}", request.label);
            }
            let tree = files(&request.input);
            // The run's index, which the command's environment no longer
            // carries, from the label (`<snapshot>:<check>:<index>`).
            let attempt = request.label.rsplit(':').next().unwrap().to_owned();
            let argv: Vec<&str> = request.argv.iter().map(String::as_str).collect();
            if let ["sandbox-fail-if", path] = argv.as_slice() {
                if tree.contains_key(*path) {
                    return Err(SandboxError::Malformed("result has no log".into()));
                }
            }
            if let ["fail-or-sandbox-if" | "fail-or-sandbox-unless", path] = argv.as_slice() {
                if tree.contains_key(*path) == (argv[0] == "fail-or-sandbox-if") {
                    return Err(SandboxError::Malformed("result has no log".into()));
                }
                return Ok(exited(1, "failed\n"));
            }
            let out = match argv.as_slice() {
                ["junit", report, script] => match tree.get(*script) {
                    None => exited(2, &format!("cannot find {script}\n")),
                    Some(bytes) => {
                        let results = named(serde_json::from_slice(bytes).unwrap());
                        let (xml, failed) = junit_in(&results, &attempt, &tree);
                        let mut out = exited(i32::from(failed), "ran the tests\n");
                        out.files.insert((*report).to_owned(), xml.into_bytes());
                        out
                    }
                },
                ["junit-dir", report, dir, ..] => {
                    let mut results: BTreeMap<String, String> = BTreeMap::new();
                    for (path, bytes) in &tree {
                        if path.starts_with(&format!("{dir}/")) && path.ends_with(".test.json") {
                            let found: BTreeMap<String, String> =
                                serde_json::from_slice(bytes).unwrap();
                            results.extend(found);
                        }
                    }
                    let mut results = named(results);
                    results.retain(|_, b| {
                        b.strip_prefix("only-if:")
                            .is_none_or(|path| tree.contains_key(path))
                    });
                    let hangs = results.values().any(|b| {
                        b.strip_prefix("timeout-unless:")
                            .is_some_and(|path| !tree.contains_key(path))
                    });
                    if hangs {
                        return Ok(SandboxOutput {
                            exit: Exit::Timeout(137),
                            output: b"still running\n".to_vec(),
                            files: BTreeMap::new(),
                            duration: Duration::from_millis(5),
                            built: None,
                        });
                    }
                    let (xml, failed) = junit_in(&results, &attempt, &tree);
                    let mut out = exited(i32::from(failed), "ran the test files\n");
                    out.files.insert((*report).to_owned(), xml.into_bytes());
                    out
                }
                ["tap-dir", report, dir] => {
                    let (tap, failed) = tap_dir(&tree, dir);
                    let mut out = exited(i32::from(failed), "ran the test files\n");
                    out.files.insert((*report).to_owned(), tap.into_bytes());
                    out
                }
                ["junit-oversized-if", report, script, path] => {
                    let results: BTreeMap<String, String> =
                        serde_json::from_slice(&tree[*script]).unwrap();
                    let (mut xml, failed) = junit_in(&results, &attempt, &tree);
                    if tree.contains_key(*path) {
                        xml.push_str("<!--");
                        xml.push_str(&" ".repeat(aver_core::MAX_BLOB_BYTES));
                        xml.push_str("-->\n");
                    }
                    let mut out = exited(i32::from(failed), "ran the tests\n");
                    out.files.insert((*report).to_owned(), xml.into_bytes());
                    out
                }
                ["junit-exit", report, script, code] => {
                    let results: BTreeMap<String, String> =
                        serde_json::from_slice(&tree[*script]).unwrap();
                    let (xml, _) = junit(&results, &attempt);
                    let code = std::str::from_utf8(&tree[*code]).unwrap().trim();
                    let mut out = exited(code.parse().unwrap(), "forged the report\n");
                    out.files.insert((*report).to_owned(), xml.into_bytes());
                    out
                }
                ["copy-report", report, source] => {
                    let text = tree.get(*source).cloned().unwrap_or_default();
                    let failed = !String::from_utf8_lossy(&text).trim().is_empty();
                    let mut out = exited(i32::from(failed) * 2, "checked the types\n");
                    out.files.insert((*report).to_owned(), text);
                    out
                }
                ["copy-junit", report, source] => {
                    let text = tree.get(*source).cloned().unwrap_or_default();
                    let xml = String::from_utf8_lossy(&text);
                    let failed = xml.contains("<failure") || xml.contains("<error");
                    let mut out = exited(i32::from(failed), "ran the tests\n");
                    out.files.insert((*report).to_owned(), text);
                    out
                }
                ["exit", code] => exited(code.parse().unwrap(), &format!("exit {code}\n")),
                ["say", path, code] => {
                    let text = tree.get(*path).cloned().unwrap_or_default();
                    exited(code.parse().unwrap(), &String::from_utf8_lossy(&text))
                }
                ["npm", "test", ..] => exited(0, "the default check passed\n"),
                ["contains", path, text] => {
                    let found = tree
                        .get(*path)
                        .is_some_and(|b| String::from_utf8_lossy(b).contains(*text));
                    exited(
                        i32::from(!found),
                        &format!("{path} contains {text:?}: {found}\n"),
                    )
                }

                ["sandbox-fail-if", _] => exited(0, "ran\n"),
                ["exit-if", path] => {
                    let present = tree.contains_key(*path);
                    exited(i32::from(present), &format!("{path} present: {present}\n"))
                }
                ["exit-unless-var", name, value] => {
                    let actual = request.env.get(*name).map(String::as_str);
                    let code = i32::from(actual != Some(*value));
                    exited(code, &format!("{name}={actual:?}\n"))
                }
                ["timeout"] => SandboxOutput {
                    exit: Exit::Timeout(137),
                    output: b"still running\n".to_vec(),
                    files: BTreeMap::new(),
                    duration: Duration::from_millis(5),
                    built: None,
                },
                ["oom"] => SandboxOutput {
                    exit: Exit::Oom(137),
                    output: b"allocating\n".to_vec(),
                    files: BTreeMap::new(),
                    duration: Duration::from_millis(5),
                    built: None,
                },
                ["timeout-unless", path] if tree.contains_key(*path) => {
                    exited(0, &format!("{path} present\n"))
                }
                ["timeout-unless", _] => SandboxOutput {
                    exit: Exit::Timeout(137),
                    output: b"still running\n".to_vec(),
                    files: BTreeMap::new(),
                    duration: Duration::from_millis(5),
                    built: None,
                },
                other => panic!("the scripted sandbox has no behavior for {other:?}"),
            };
            Ok(with_setup(&request.setup, &tree, out))
        })
    }
}
