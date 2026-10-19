//! `aver check -- <cmd>` (ARCHITECTURE.md §2.7.1): an agent's own test
//! command as a one-off check with a report Aver can read, run as a verify
//! dry run on both sides.
//!
//! [`check_for`] rewrites the command so its runner writes per-test results
//! Aver reads: `cargo test` keeps its libtest text, `go test` gets `-json`,
//! pytest `--junitxml`, and node:test, Vitest, Jest, Mocha, tap and `npm
//! test` the reporters of [`runner`]. Anything else is judged by its
//! exit code. [`fit`] gives the check the image, dependency layers and
//! setup of the environment's check it stands in for, so it runs where that
//! check would. The verify engine then runs it on the base, the judged head,
//! the head-tests tree and the transplant, natively and through the run
//! cache, and signs nothing; [`text`] says what that showed.
//!
//! [`near`] and [`narrowed`] are `aver check --near`: the test files near
//! a change and the runner command that runs only them.

pub mod runner;

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use aver_core::{
    Cause, CheckDef, Environment, HeadStatus, Path, ReceiptBody, Report, ReportFormat,
};

use crate::diagnostics::Diagnostics;
use crate::env::Family;
use crate::run::report::{failure_excerpt, fence, FENCE_NOTE};

/// The check id of an ad-hoc check.
pub const ID: &str = "yours";

/// Most test files `--near` picks.
pub const MAX_NEAR: usize = 50;

/// An agent's command as a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adhoc {
    pub check: CheckDef,
    /// The runner whose report Aver reads; `None`: the exit code only.
    pub runner: Option<&'static str>,
}

/// Why a command cannot be an ad-hoc check.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AdhocError {
    #[error("give the test command after --, as in: aver check -- cargo test div")]
    Empty,
    #[error("aver check -- runs your test command; `{0}` is Aver itself")]
    Aver(String),
}

/// One shell word.
fn quote(word: &str) -> String {
    crate::ci::shell_quote(word)
}

fn program(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn report(format: ReportFormat, path: &str) -> Report {
    Report {
        format,
        path: path.parse().expect("a report path under /aver/out"),
    }
}

/// `argv` as check [`ID`] in directory `cwd` of the repository, with a
/// report Aver reads when it knows the runner. `manifest` and `lockfile`
/// are the `package.json` and `package-lock.json` of that directory, for
/// `npm test` and the JavaScript runners.
pub fn check_for(
    argv: &[String],
    cwd: Option<Path>,
    manifest: Option<&[u8]>,
    lockfile: Option<&[u8]>,
) -> Result<Adhoc, AdhocError> {
    let first = argv.first().ok_or(AdhocError::Empty)?;
    if program(first) == "aver" {
        return Err(AdhocError::Aver(argv.join(" ")));
    }
    let id = ID.parse().expect("a valid check id");
    let word = |i: usize| argv.get(i).map(String::as_str);
    let (argv_out, report_out, runner): (Vec<String>, Option<Report>, Option<&'static str>) =
        match (program(first), word(1), word(2)) {
            ("cargo", Some("test"), _) => {
                let (args, after) = split_dashes(&argv[2..]);
                let mut words: Vec<String> = vec!["cargo".into(), "test".into()];
                if !args.iter().any(|a| a == "--no-fail-fast") {
                    words.push("--no-fail-fast".into());
                }
                // `-q` prints dots, not the per-test lines the report is.
                words.extend(
                    args.iter()
                        .filter(|a| !matches!(a.as_str(), "-q" | "--quiet"))
                        .cloned(),
                );
                if let Some(after) = after {
                    words.push("--".into());
                    words.extend(
                        after
                            .iter()
                            .filter(|a| !matches!(a.as_str(), "-q" | "--quiet"))
                            .cloned(),
                    );
                }
                let out = format!("/aver/out/{ID}.txt");
                let line: Vec<String> = words.iter().map(|w| quote(w)).collect();
                (
                    sh(format!(
                        "{} >{out} 2>&1; s=$?; cat {out}; exit $s",
                        line.join(" ")
                    )),
                    Some(report(ReportFormat::Libtest, &out)),
                    Some("cargo test"),
                )
            }
            ("go", Some("test"), _) => {
                let mut words: Vec<String> = vec!["go".into(), "test".into()];
                if !argv[2..].iter().any(|a| a == "-json" || a == "--json") {
                    words.push("-json".into());
                }
                words.extend(argv[2..].iter().cloned());
                let out = format!("/aver/out/{ID}.json");
                let line: Vec<String> = words.iter().map(|w| quote(w)).collect();
                (
                    sh(format!("exec {} >{out}", line.join(" "))),
                    Some(report(ReportFormat::GoJson, &out)),
                    Some("go test"),
                )
            }
            ("tsc", _, _) | ("npx", Some("tsc"), _) => {
                let out = format!("/aver/out/{ID}.txt");
                let line: Vec<String> = argv.iter().map(|w| quote(w)).collect();
                (
                    sh(format!(
                        "{} >{out} 2>&1; s=$?; cat {out}; exit $s",
                        line.join(" ")
                    )),
                    Some(report(ReportFormat::Tsc, &out)),
                    Some("tsc"),
                )
            }
            ("pytest" | "py.test", _, _) => pytest(vec!["python".into()], &argv[1..]),
            ("python" | "python3", Some("-m"), Some("pytest")) => {
                pytest(vec![first.clone()], &argv[3..])
            }
            ("npm", Some("test" | "t"), _) | ("npm", Some("run"), Some("test")) => {
                let skip = if word(1) == Some("run") { 3 } else { 2 };
                npm_test(&argv[skip..], manifest, lockfile)
            }
            _ => match runner::reporter(argv, lockfile, ID) {
                Some((argv, report)) => {
                    let name = runner_name(&argv);
                    (argv, Some(report), name)
                }
                None => (plain(argv), None, None),
            },
        };
    let mut check = CheckDef::new(id, argv_out);
    check.cwd = cwd;
    check.report = report_out;
    Ok(Adhoc { check, runner })
}

/// How a command Aver judges only by its exit code could report per test,
/// in one line: a runner's own JUnit option where Aver knows it, else a
/// runner Aver adds a reporter to, or a check written by hand.
pub fn exit_only_hint(argv: &[String]) -> String {
    let words: Vec<&str> = argv.iter().map(|a| program(a)).collect();
    let junit = |option: &str| {
        format!(
            "add `{option}` to the command and write the check by hand with `report = \"junit\"`"
        )
    };
    match words.as_slice() {
        ["pytest" | "py.test", ..] | ["python" | "python3", "-m", "pytest", ..] => {
            "leave out `--junitxml`: Aver adds its own and reads the tests".to_owned()
        }
        ["bun", "test", ..] => junit("--reporter=junit --reporter-outfile=$AVER_OUT/test.xml"),
        ["deno", "test", ..] => junit("--junit-path=$AVER_OUT/test.xml"),
        ["phpunit", ..] | ["vendor/bin/phpunit", ..] => junit("--log-junit $AVER_OUT/test.xml"),
        ["ctest", ..] => junit("--output-junit $AVER_OUT/test.xml"),
        ["rspec", ..] | ["bundle", "exec", "rspec", ..] => {
            junit("--format RspecJunitFormatter --out $AVER_OUT/test.xml")
        }
        ["make" | "just" | "yarn" | "pnpm" | "npm" | "bun" | "tox" | "nox", ..] => {
            "run the test runner it calls directly (`cargo test`, `go test`, `pytest`, `node --test`, `npx vitest run`, `npx jest`, `npx mocha`), so Aver adds a reporter".to_owned()
        }
        _ => "run a test runner Aver adds a reporter to (`cargo test`, `go test`, `pytest`, `node --test`, `npx vitest run`, `npx jest`, `npx mocha`), or have yours write JUnit XML to `$AVER_OUT/test.xml` and write the check by hand with `report = \"junit\"`".to_owned(),
    }
}

/// A command line `aver check -- <cmd>` was given, as the commands it
/// joins with `&&`, each to run as a check of its own: the words split on a
/// `&&` word, else a single word or an `sh -c` script split on its
/// top-level `&&`. One command when it joins none.
pub fn segments(argv: &[String]) -> Vec<Vec<String>> {
    let script = match argv {
        [one] => Some(one.as_str()),
        [shell, flag, script] if matches!(shell.as_str(), "sh" | "bash") && flag == "-c" => {
            Some(script.as_str())
        }
        _ => None,
    };
    if let Some(parts) = script.and_then(runner::split_and).and_then(|parts| {
        parts
            .iter()
            .map(|p| runner::tokenize(p))
            .collect::<Option<Vec<_>>>()
    }) {
        return parts;
    }
    argv.split(|w| w == "&&")
        .filter(|part| !part.is_empty())
        .map(<[String]>::to_vec)
        .collect()
}

/// The floor `aver check -- <cmd>` writes for a command whose base run ran
/// `ran` tests: 90% of them, at least 1.
pub fn floor(ran: usize) -> NonZeroU32 {
    let ninety = u32::try_from(ran.saturating_mul(9) / 10).unwrap_or(u32::MAX);
    NonZeroU32::new(ninety).unwrap_or(NonZeroU32::MIN)
}

/// A check `aver check -- <cmd>` writes to `aver.toml`: [`check_for`]'s
/// check under `id`, with the floor its base run sets (`None`: none, for an
/// exit-only check).
pub struct Written<'a> {
    pub id: &'a str,
    pub check: &'a CheckDef,
    pub min_tests: Option<NonZeroU32>,
}

/// The `aver.toml` contract for `checks`, keeping the directories in `keep`
/// across runs, installed from the lockfiles in `inputs`. A report on
/// standard output is written as its format alone; every other report path
/// is under `$AVER_OUT`, named after the check.
pub fn contract_toml(checks: &[Written<'_>], keep: &[&str], inputs: &[&str]) -> String {
    let string = |s: &str| toml::Value::String(s.to_owned()).to_string();
    let array = |items: &[&str]| {
        let quoted: Vec<String> = items.iter().map(|s| string(s)).collect();
        format!("[{}]", quoted.join(", "))
    };
    let mut out = String::from(
        "# Written by `aver check -- <cmd>`: the commands that run this repository's\n\
         # tests, and how many tests the original code must run (min_tests).\n",
    );
    for w in checks {
        let renamed: Vec<String> = w
            .check
            .argv
            .iter()
            .map(|a| a.replace(&format!("/aver/out/{ID}."), &format!("/aver/out/{}.", w.id)))
            .collect();
        let stdout = |script: &str| {
            script
                .split(" >/aver/out/")
                .next()
                .map(|line| line.strip_prefix("exec ").unwrap_or(line).to_owned())
        };
        if w.check.report.is_none() {
            out.push_str(
                "\n# Exit code only: this check reports no tests, so it shows only that the\n\
                 # command succeeded. Written with `aver check --allow-exit-only`.",
            );
        }
        let (run, report, report_file) =
            match (w.check.report.as_ref().map(|r| r.format), &renamed[..]) {
                (None, _) => (crate::contract::shell_line(&renamed).ok(), "exit", None),
                (Some(f @ (ReportFormat::Libtest | ReportFormat::GoJson)), [_, _, script]) => (
                    stdout(script).and_then(|l| l.parse().ok()),
                    if f == ReportFormat::Libtest {
                        "libtest"
                    } else {
                        "go-json"
                    },
                    None,
                ),
                (Some(ReportFormat::Tsc), [_, _, script]) => {
                    (stdout(script).and_then(|l| l.parse().ok()), "tsc", None)
                }
                (Some(ReportFormat::JestJson), _) => (
                    crate::contract::shell_line(&renamed).ok(),
                    "jest-json",
                    Some(format!("{}.json", w.id)),
                ),
                (Some(ReportFormat::Tap), _) => (
                    crate::contract::shell_line(&renamed).ok(),
                    "tap",
                    Some(format!("{}.tap", w.id)),
                ),
                (Some(_), _) => (crate::contract::shell_line(&renamed).ok(), "junit", None),
            };
        let run = run.map_or_else(
            || renamed.join(" "),
            |l: aver_core::ShellLine| l.as_str().to_owned(),
        );
        out.push_str(&format!(
            "\n[[check]]\nid = {}\nrun = {}\n",
            string(w.id),
            string(&run)
        ));
        if let Some(cwd) = &w.check.cwd {
            out.push_str(&format!("cwd = {}\n", string(cwd.as_str())));
        }
        if report != "junit" {
            out.push_str(&format!("report = {}\n", string(report)));
        }
        if let Some(file) = report_file {
            out.push_str(&format!("report_file = {}\n", string(&file)));
        }
        if let Some(n) = w.min_tests {
            out.push_str(&format!("min_tests = {n}\n"));
        }
    }
    if !keep.is_empty() || !inputs.is_empty() {
        out.push_str("\n[deps]\n");
        if !keep.is_empty() {
            out.push_str(&format!("keep = {}\n", array(keep)));
        }
        if !inputs.is_empty() {
            out.push_str(&format!("inputs = {}\n", array(inputs)));
        }
    }
    out
}

fn sh(script: String) -> Vec<String> {
    vec!["sh".into(), "-c".into(), script]
}

/// A command run as typed; leading `NAME=value` words go through `env`.
fn plain(argv: &[String]) -> Vec<String> {
    let assignment = argv[0].split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    });
    let mut out = Vec::with_capacity(argv.len() + 1);
    if assignment {
        out.push("env".into());
    }
    out.extend(argv.iter().cloned());
    out
}

/// The arguments before a `--` and those after it, if any.
fn split_dashes(args: &[String]) -> (&[String], Option<&[String]>) {
    match args.iter().position(|a| a == "--") {
        Some(at) => (&args[..at], Some(&args[at + 1..])),
        None => (args, None),
    }
}

fn pytest(
    python: Vec<String>,
    args: &[String],
) -> (Vec<String>, Option<Report>, Option<&'static str>) {
    let mut argv = python;
    argv.extend(["-m", "pytest"].map(String::from));
    if args
        .iter()
        .any(|a| a == "--junitxml" || a.starts_with("--junitxml=") || a == "--junit-xml")
    {
        argv.extend(args.iter().cloned());
        return (argv, None, None);
    }
    let out = format!("/aver/out/{ID}.xml");
    argv.extend(["-p", "no:cacheprovider", "-o", "junit_family=xunit1"].map(String::from));
    argv.extend(args.iter().cloned());
    argv.push(format!("--junitxml={out}"));
    (
        argv,
        Some(report(ReportFormat::PytestJunit, &out)),
        Some("pytest"),
    )
}

/// `npm test [-- args]` with the reporter of the runner its `test` script
/// invokes, as the default check adds it.
fn npm_test(
    args: &[String],
    manifest: Option<&[u8]>,
    lockfile: Option<&[u8]>,
) -> (Vec<String>, Option<Report>, Option<&'static str>) {
    let typed = || {
        let mut argv: Vec<String> = vec!["npm".into(), "test".into()];
        argv.extend(args.iter().cloned());
        argv
    };
    let extra = match args.split_first() {
        None => &[][..],
        Some((dashes, rest)) if dashes == "--" => rest,
        Some(_) => return (typed(), None, None),
    };
    let check = crate::presets::package_check(ID, manifest, lockfile, None, &|_| true);
    let Some(report) = check.report.clone() else {
        return (typed(), None, None);
    };
    let name = runner::runner_of(&check).map(runner::Runner::name);
    // The default check's script forms (`sh -c`, `npm exec --call`) take no
    // extra arguments.
    if check.argv[..2] != ["npm", "test"] && !extra.is_empty() {
        return (typed(), None, None);
    }
    let mut argv = check.argv;
    if !extra.is_empty() {
        if !argv.iter().any(|a| a == "--") {
            argv.push("--".into());
        }
        argv.extend(extra.iter().cloned());
    }
    (argv, Some(report), name)
}

fn runner_name(argv: &[String]) -> Option<&'static str> {
    argv.iter().find_map(|a| match program(a) {
        "vitest" => Some("vitest"),
        "jest" => Some("jest"),
        "mocha" | "_mocha" => Some("mocha"),
        "tap" => Some("tap"),
        "node" | "tsx" => Some("node:test"),
        _ => None,
    })
}

/// The family of an ad-hoc command: the toolchain it runs.
fn family(check: &CheckDef) -> Family {
    let words: Vec<&str> = check
        .argv
        .iter()
        .flat_map(|a| a.split_whitespace())
        .collect();
    let first = words
        .iter()
        .map(|w| program(w))
        .find(|w| !matches!(*w, "sh" | "-c" | "exec" | "env") && !w.contains('='));
    match first {
        Some("cargo") => Family::Rust,
        Some("go") => Family::Go,
        Some("python" | "python3" | "pytest") => Family::Python,
        _ => Family::Node,
    }
}

/// `check` as the base environment would run it (the planner's step): the
/// image, dependency layers, timeout, Node setup commands and Python path
/// of the environment's check of the same toolchain whose directory holds
/// `check`'s (the deepest), else the layers a check of its toolchain mounts
/// by default.
pub fn fit(check: &CheckDef, env: &Environment) -> CheckDef {
    let mut out = check.clone();
    let want = family(check);
    let cwd = check.cwd.as_ref().map_or("", Path::as_str);
    let within = |dir: &str| {
        dir.is_empty()
            || cwd == dir
            || cwd
                .strip_prefix(dir)
                .is_some_and(|rest| rest.starts_with('/'))
    };
    let model = env
        .checks
        .iter()
        .filter(|c| c.id.as_str() != ID)
        .filter(|c| Family::of(c.image.as_ref().unwrap_or(&env.image)) == Some(want))
        .filter(|c| within(c.cwd.as_ref().map_or("", Path::as_str)))
        .max_by_key(|c| c.cwd.as_ref().map_or(0, |p| p.as_str().len()));
    match model {
        Some(model) => {
            out.image = model.image.clone();
            out.deps = model.deps.clone();
            out.timeout_ms = out.timeout_ms.or(model.timeout_ms);
            if want == Family::Node && out.setup.is_empty() {
                out.setup = model.setup.clone();
            }
            if want == Family::Python && out.argv.first().map(String::as_str) != Some("env") {
                let prefix: Vec<String> = model
                    .argv
                    .iter()
                    .take_while(|a| *a == "env" || a.contains('='))
                    .cloned()
                    .collect();
                if prefix.len() > 1 {
                    out.argv.splice(0..0, prefix);
                }
            }
        }
        None => {
            if want != Family::Node {
                let image = match want {
                    Family::Rust => crate::presets::RUST_IMAGE,
                    Family::Go => crate::presets::GO_IMAGE,
                    _ => crate::presets::PYTHON_IMAGE,
                };
                if Family::of(&env.image) != Some(want) {
                    out.image = Some(aver_core::Image {
                        reference: image.0.to_owned(),
                        digest: image.1.parse().expect("a pinned digest"),
                    });
                }
            }
            out.deps = crate::env::default_deps(
                &out,
                &env.image,
                env.lockfile.as_ref(),
                env.layers.as_slice(),
            );
        }
    }
    out
}

/// What a dry run of an ad-hoc check showed, test by test.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The command as typed.
    pub command: String,
    pub runner: Option<String>,
    /// Tests that fail on the original code (or do not load there) and pass
    /// with the change: the head's own tests on the transplant, and tests
    /// the change fixed.
    pub shows: Vec<String>,
    /// Failures the change causes, with Aver's excerpt.
    pub new: Vec<Failed>,
    /// Failures the original code has too.
    pub already: Vec<String>,
    /// The change's new tests that pass on the original code too.
    pub pass_without: Vec<String>,
    pub flaky: Vec<String>,
    /// Existing tests whose rewritten expectation passes, for review.
    pub changed: Vec<String>,
    /// Failures from the environment (a dependency, the toolchain).
    pub env: Vec<String>,
    /// Tests that pass with the change.
    pub passing: usize,
    /// No per-test results: the command's exit code on each side.
    pub exit_only: Option<ExitOnly>,
    /// The original code plus your test files did not build (no run there
    /// reported a test), so `shows` says only that what they call exists now.
    pub unbuilt: bool,
}

/// What the ad-hoc check's transplant runs (the original code plus the
/// change's test files) reported, read back from the run cache.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransplantRuns {
    /// Whether the dry run ran the transplant at all.
    pub ran: bool,
    /// Whether any of its runs reported a test (else the build failed).
    pub reported: bool,
    /// The tests that passed in one of them, by test id.
    pub passed: BTreeSet<String>,
}

impl TransplantRuns {
    pub fn of<'a>(runs: impl IntoIterator<Item = &'a aver_core::CheckRun>) -> Self {
        let mut out = Self::default();
        for run in runs {
            out.ran = true;
            out.reported |= !run.tests.is_empty();
            for t in run.tests.iter() {
                if t.status == aver_core::TestStatus::Pass {
                    out.passed.insert(t.id.as_str().to_owned());
                }
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub test: String,
    pub detail: Option<String>,
}

/// An exit-code-only command's result on each side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitOnly {
    pub original_passed: bool,
    pub yours_passed: bool,
}

fn display(id: &str) -> String {
    if id == ID {
        return "your command (its exit code)".to_owned();
    }
    id.strip_prefix(&format!("{ID}::")).unwrap_or(id).to_owned()
}

impl Outcome {
    /// From a dry run's receipt and diagnostics, and what its transplant
    /// runs reported.
    pub fn of(
        command: String,
        runner: Option<&str>,
        body: &ReceiptBody,
        diagnostics: Option<&Diagnostics>,
        transplant: &TransplantRuns,
    ) -> Self {
        let mut out = Outcome {
            command,
            runner: runner.map(str::to_owned),
            ..Self::default()
        };
        let ours = |id: &str| id == ID || id.starts_with(&format!("{ID}::"));
        let details: BTreeMap<&str, String> = diagnostics
            .map(|d| {
                d.failures
                    .iter()
                    .filter_map(|f| {
                        let text = f
                            .message
                            .clone()
                            .or_else(|| f.output.clone())
                            .filter(|t| !t.trim().is_empty())?;
                        Some((f.id.as_str(), text))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut base_absent = BTreeSet::new();
        let mut new_passing = BTreeSet::new();
        for e in body.checks.iter().filter(|e| ours(e.id.as_str())) {
            let id = e.id.as_str();
            if e.base == aver_core::BaseStatus::NotApplicable {
                base_absent.insert(id.to_owned());
                if e.head == HeadStatus::Pass && e.cause.is_none() {
                    new_passing.insert(display(id));
                }
            }
            if e.head == HeadStatus::Pass && e.cause.is_none() {
                out.passing += 1;
            }
            match e.cause {
                Some(Cause::New) => out.new.push(Failed {
                    test: display(id),
                    detail: details
                        .get(id)
                        .cloned()
                        .or_else(|| e.detail.as_ref().map(|d| d.as_str().to_owned())),
                }),
                Some(Cause::Preexisting) => out.already.push(display(id)),
                Some(Cause::Flaky) => out.flaky.push(display(id)),
                Some(Cause::Changed) => out.changed.push(display(id)),
                Some(Cause::Env) => out.env.push(display(id)),
                None => {}
            }
        }
        if let Some(evidence) = diagnostics.and_then(|d| d.evidence.as_ref()) {
            let shown = evidence
                .shows_change
                .iter()
                .map(|s| s.id.as_str())
                .chain(evidence.fixed.items.iter().map(String::as_str))
                .filter(|id| ours(id));
            let set: BTreeSet<String> = shown.map(display).collect();
            out.shows = set.into_iter().collect();
        }
        // A transplant that reported nothing did not build: the change's new
        // tests cannot even compile against the original code.
        if transplant.ran && !transplant.reported && !new_passing.is_empty() {
            out.unbuilt = true;
            let mut set: BTreeSet<String> = out.shows.drain(..).collect();
            set.extend(new_passing);
            out.shows = set.into_iter().collect();
        }
        out.pass_without = transplant
            .passed
            .iter()
            .filter(|id| ours(id) && base_absent.contains(id.as_str()))
            .map(|id| display(id))
            .collect();
        let whole: Vec<_> = body.checks.iter().filter(|e| e.id.as_str() == ID).collect();
        if let [entry] = whole.as_slice() {
            out.exit_only = Some(ExitOnly {
                original_passed: entry.base == aver_core::BaseStatus::Pass,
                yours_passed: entry.head == HeadStatus::Pass,
            });
            // The summary says it; a section naming only the command adds nothing.
            out.already.clear();
        }
        out
    }

    /// Whether the change broke something the command runs.
    pub fn broke(&self) -> bool {
        !self.new.is_empty()
    }

    /// The first and last line's summary.
    pub fn summary(&self) -> String {
        if let Some(exit) = self.exit_only {
            let side = |passed: bool| if passed { "passes" } else { "fails" };
            return format!(
                "Your command: {} on the original, {} with your change (exit code only)",
                side(exit.original_passed),
                side(exit.yours_passed)
            );
        }
        let n = |count: usize, one: &str, many: &str| {
            format!("{count} {}", if count == 1 { one } else { many })
        };
        let mut parts = vec![
            format!(
                "{} on the original and {} with yours",
                n(self.shows.len(), "test fails", "tests fail"),
                if self.shows.len() == 1 {
                    "passes"
                } else {
                    "pass"
                }
            ),
            n(self.new.len(), "new failure", "new failures"),
            format!("{} already failing", self.already.len()),
        ];
        if !self.pass_without.is_empty() {
            parts.push(format!(
                "{} without your change",
                n(self.pass_without.len(), "new test passes", "new tests pass")
            ));
        }
        if !self.flaky.is_empty() {
            parts.push(format!("{} flaky", self.flaky.len()));
        }
        if !self.changed.is_empty() {
            parts.push(format!(
                "{} rewritten",
                n(self.changed.len(), "test", "tests")
            ));
        }
        format!("Your command: {}", parts.join("; "))
    }
}

/// The report of `aver check -- <cmd>`: the summary first and last, the
/// sections between, repository strings fenced.
pub fn text(outcome: &Outcome) -> String {
    let mut out = String::new();
    out.push_str(&outcome.summary());
    out.push('\n');
    let how = match &outcome.runner {
        Some(r) => format!("per test from {r}'s report"),
        None => "by exit code: Aver cannot read per-test results from this command".to_owned(),
    };
    out.push_str(&format!(
        "Ran natively on the original code, your copy and the original plus your test files, judged {how}; nothing signed, no verify used.\n"
    ));
    out.push_str(FENCE_NOTE);
    out.push('\n');
    out.push_str("Command:\n");
    out.push_str(&fence(&outcome.command));
    let list = |out: &mut String, title: &str, items: &[String]| {
        if items.is_empty() {
            return;
        }
        out.push_str(&format!("\n{title} ({}):\n", items.len()));
        out.push_str(&fence(&items.join("\n")));
    };
    list(
        &mut out,
        if outcome.unbuilt {
            "Shows the change: pass with yours; with the original code they do not build, so this says only that what they call exists now"
        } else {
            "Shows the change: fail on the original, pass with yours"
        },
        &outcome.shows,
    );
    if !outcome.new.is_empty() {
        out.push_str(&format!(
            "\nNEW: fail with your change, not on the original ({}):\n",
            outcome.new.len()
        ));
        let mut body = String::new();
        for f in &outcome.new {
            body.push_str(&f.test);
            body.push('\n');
            if let Some(detail) = &f.detail {
                for line in failure_excerpt(detail, 12, "aver check --json") {
                    body.push_str("    ");
                    body.push_str(&line);
                    body.push('\n');
                }
            }
        }
        out.push_str(&fence(&body));
    }
    list(
        &mut out,
        "Already failing on the original (not yours)",
        &outcome.already,
    );
    list(
        &mut out,
        "Pass without your change: these new tests pass on the original code too, so they do not show it",
        &outcome.pass_without,
    );
    list(
        &mut out,
        "Flaky: passed and failed in reruns",
        &outcome.flaky,
    );
    list(
        &mut out,
        "Rewritten: existing tests whose expectation you changed (a person reviews these)",
        &outcome.changed,
    );
    list(
        &mut out,
        "Environment: failures from a dependency or the toolchain",
        &outcome.env,
    );
    out.push('\n');
    out.push_str(&outcome.summary());
    out.push_str(". Not a verify: `aver verify` runs every affected check.\n");
    out
}

/// A test file's toolchain, by name.
fn ecosystem(path: &str) -> Option<Family> {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name.ends_with(".rs") {
        Some(Family::Rust)
    } else if name.ends_with(".go") {
        Some(Family::Go)
    } else if name.ends_with(".py") {
        Some(Family::Python)
    } else if [".js", ".mjs", ".cjs", ".ts", ".mts", ".cts", ".jsx", ".tsx"]
        .iter()
        .any(|e| name.ends_with(e))
    {
        Some(Family::Node)
    } else {
        None
    }
}

/// A source file's stem as test files name it: `div` for `src/div.js`,
/// `src/div/mod.rs`'s `div`.
fn stem(path: &str) -> Option<&str> {
    let mut parts = path.rsplit('/');
    let name = parts.next()?;
    let base = name.split('.').next().filter(|s| !s.is_empty())?;
    let base = if matches!(base, "mod" | "index" | "__init__" | "lib" | "main") {
        parts.next()?
    } else {
        base
    };
    (base.len() >= 2).then_some(base)
}

/// Whether `word` occurs in `text` on its own (not inside a longer name).
fn names(text: &str, word: &str) -> bool {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(word).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(ident)
            && !text[at + word.len()..].chars().next().is_some_and(ident)
    })
}

/// `aver check --near`: the test files near the change, at most
/// [`MAX_NEAR`], in this order: the changed test files, test files whose
/// name holds a changed file's stem, and test files whose text names it.
/// `changed` are the repository paths the change adds or modifies, `tests`
/// every test file of the tree, `read` a test file's text.
pub fn near(
    changed: &[String],
    tests: &[String],
    read: &dyn Fn(&str) -> Option<String>,
) -> Vec<String> {
    let is_test: BTreeSet<&str> = tests.iter().map(String::as_str).collect();
    let mut out: Vec<String> = Vec::new();
    let push = |out: &mut Vec<String>, p: &str| {
        if out.len() < MAX_NEAR && !out.iter().any(|o| o == p) {
            out.push(p.to_owned());
        }
    };
    for path in changed.iter().filter(|p| is_test.contains(p.as_str())) {
        push(&mut out, path);
    }
    let stems: Vec<(&str, Option<Family>)> = changed
        .iter()
        .filter(|p| !is_test.contains(p.as_str()))
        .filter_map(|p| Some((stem(p)?, ecosystem(p))))
        .filter(|(_, family)| family.is_some())
        .collect();
    for (stem, family) in &stems {
        for test in tests {
            let name = test.rsplit('/').next().unwrap_or(test);
            if ecosystem(test) == *family && names(name, stem) {
                push(&mut out, test);
            }
        }
    }
    if out.len() < MAX_NEAR && !stems.is_empty() {
        for test in tests {
            if out.len() >= MAX_NEAR {
                break;
            }
            let family = ecosystem(test);
            let relevant: Vec<&str> = stems
                .iter()
                .filter(|(_, f)| *f == family)
                .map(|(s, _)| *s)
                .collect();
            if relevant.is_empty() || out.contains(test) {
                continue;
            }
            if read(test).is_some_and(|text| relevant.iter().any(|s| names(&text, s))) {
                push(&mut out, test);
            }
        }
    }
    out
}

/// The command `--near` runs and where (`cwd`), and what it says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Narrowed {
    pub argv: Vec<String>,
    pub cwd: Option<Path>,
    /// Aver's words on what runs, for the report.
    pub note: String,
}

/// The narrowed command for `files` (from [`near`]) and the source files
/// `changed`, in the toolchain most of the files belong to: pytest and
/// node:test, Vitest, Jest, Mocha or tap take the files, `go test` their
/// packages, `cargo test` the crates (and an integration test's own
/// target). `read` reads a repository file. `None` when the files name no
/// toolchain Aver can narrow.
pub fn narrowed(
    files: &[String],
    changed: &[String],
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Option<Narrowed> {
    let mut counts: BTreeMap<u8, (Family, usize)> = BTreeMap::new();
    for f in files {
        if let Some(family) = ecosystem(f) {
            counts.entry(family as u8).or_insert((family, 0)).1 += 1;
        }
    }
    let (family, _) = counts.values().max_by_key(|(_, n)| *n).copied()?;
    let mine: Vec<&String> = files
        .iter()
        .filter(|f| ecosystem(f) == Some(family))
        .collect();
    let n = mine.len();
    let note = |what: &str| {
        format!(
            "{n} test {} near your change: {what}",
            if n == 1 { "file" } else { "files" }
        )
    };
    match family {
        Family::Python => {
            let mut argv: Vec<String> = ["python", "-m", "pytest"].map(String::from).to_vec();
            argv.extend(mine.iter().map(|f| (*f).clone()));
            Some(Narrowed {
                argv,
                cwd: None,
                note: note("pytest runs them"),
            })
        }
        Family::Go => {
            let module = |path: &str| -> String {
                let mut dir = parent(path);
                loop {
                    let candidate = join(dir, "go.mod");
                    if read(&candidate).is_some() || dir.is_empty() {
                        return dir.to_owned();
                    }
                    dir = parent(dir);
                }
            };
            let root = module(mine[0]);
            let mut pkgs = BTreeSet::new();
            for f in mine.iter().map(|f| f.as_str()).chain(
                changed
                    .iter()
                    .map(String::as_str)
                    .filter(|c| c.ends_with(".go")),
            ) {
                if module(f) != root {
                    continue;
                }
                let dir = parent(f);
                let rel = dir
                    .strip_prefix(&root)
                    .unwrap_or(dir)
                    .trim_start_matches('/');
                pkgs.insert(if rel.is_empty() {
                    ".".to_owned()
                } else {
                    format!("./{rel}")
                });
            }
            let mut argv: Vec<String> = vec!["go".into(), "test".into()];
            argv.extend(pkgs);
            Some(Narrowed {
                argv,
                cwd: (!root.is_empty()).then(|| root.parse().ok()).flatten(),
                note: note("go test runs their packages"),
            })
        }
        Family::Rust => {
            let mut crates: BTreeMap<String, (bool, Vec<String>)> = BTreeMap::new();
            for f in mine.iter().map(|f| f.as_str()).chain(
                changed
                    .iter()
                    .map(String::as_str)
                    .filter(|c| c.ends_with(".rs")),
            ) {
                let Some((dir, name)) = crate_of(f, read) else {
                    continue;
                };
                let entry = crates.entry(name).or_insert((true, Vec::new()));
                let rel = f.strip_prefix(&dir).unwrap_or(f).trim_start_matches('/');
                match rel.strip_prefix("tests/").filter(|t| !t.contains('/')) {
                    Some(t) if files.iter().any(|x| x == f) => {
                        entry.1.push(t.trim_end_matches(".rs").to_owned())
                    }
                    _ => entry.0 = false,
                }
            }
            if crates.is_empty() {
                return None;
            }
            let mut argv: Vec<String> = vec!["cargo".into(), "test".into()];
            for (name, (only_tests, targets)) in &crates {
                argv.extend(["-p".into(), name.clone()]);
                if *only_tests && crates.len() == 1 {
                    for t in targets {
                        argv.extend(["--test".into(), t.clone()]);
                    }
                }
            }
            Some(Narrowed {
                argv,
                cwd: None,
                note: note("cargo test runs their crates"),
            })
        }
        Family::Node => {
            let pkg = read("package.json");
            let lock = read("package-lock.json");
            let files: Vec<String> = mine.iter().map(|f| (*f).clone()).collect();
            let npm: Vec<String> = crate::env::DEFAULT_CHECK_ARGV
                .iter()
                .map(|s| (*s).to_owned())
                .collect();
            match runner::narrow(
                &npm,
                pkg.as_deref(),
                lock.as_deref(),
                &runner::Narrow::Files(files),
            ) {
                Some(argv) => Some(Narrowed {
                    argv,
                    cwd: None,
                    note: note("npm test runs them"),
                }),
                None => Some(Narrowed {
                    argv: npm,
                    cwd: None,
                    note: format!(
                        "{n} test {} near your change, but Aver cannot hand files to this test script, so `npm test` runs whole",
                        if n == 1 { "file" } else { "files" }
                    ),
                }),
            }
        }
    }
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// The directory and package name of the crate whose `Cargo.toml` is
/// nearest above `path`.
fn crate_of(path: &str, read: &dyn Fn(&str) -> Option<Vec<u8>>) -> Option<(String, String)> {
    let mut dir = parent(path);
    loop {
        if let Some(bytes) = read(&join(dir, "Cargo.toml")) {
            let text = String::from_utf8_lossy(&bytes);
            if let Some(name) = package_name(&text) {
                return Some((dir.to_owned(), name));
            }
        }
        if dir.is_empty() {
            return None;
        }
        dir = parent(dir);
    }
}

/// `[package] name` of a `Cargo.toml`.
fn package_name(toml: &str) -> Option<String> {
    let value: toml::Value = toml::from_str(toml).ok()?;
    value
        .get("package")?
        .get("name")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split(' ').map(str::to_owned).collect()
    }

    fn adhoc(s: &str) -> Adhoc {
        check_for(&words(s), None, None, None).unwrap()
    }

    #[test]
    fn segments_split_a_compound_command_at_its_top_level_ands() {
        let two = vec![words("cargo test"), words("npx tsc --noEmit")];
        assert_eq!(segments(&words("cargo test && npx tsc --noEmit")), two);
        assert_eq!(segments(&["cargo test && npx tsc --noEmit".into()]), two);
        assert_eq!(
            segments(
                &words("sh -c")
                    .into_iter()
                    .chain(["cargo test && npx tsc --noEmit".into()])
                    .collect::<Vec<_>>()
            ),
            two
        );
        assert_eq!(segments(&words("cargo test")), vec![words("cargo test")]);
    }

    #[test]
    fn the_floor_is_ninety_percent_of_the_base_and_at_least_one() {
        let floors: Vec<u32> = [0, 1, 4, 10, 47].map(|n| floor(n).get()).to_vec();
        assert_eq!(floors, vec![1, 1, 3, 9, 42]);
    }

    #[test]
    fn the_written_contract_parses_into_the_checks_it_names() {
        let test = adhoc("cargo test -p calc");
        let typecheck = adhoc("npx tsc --noEmit");
        let lint = adhoc("make lint");
        let written = [
            Written {
                id: "test",
                check: &test.check,
                min_tests: Some(floor(40)),
            },
            Written {
                id: "typecheck",
                check: &typecheck.check,
                min_tests: Some(NonZeroU32::MIN),
            },
            Written {
                id: "lint",
                check: &lint.check,
                min_tests: None,
            },
        ];
        let toml = contract_toml(&written, &["target"], &["Cargo.lock"]);
        let parsed = crate::contract::parse(&toml).unwrap();
        assert!(
            matches!(parsed.shape, crate::contract::Shape::Contract),
            "{toml}"
        );
        let read: Vec<(String, Option<u32>, String)> = parsed
            .contract
            .checks
            .iter()
            .map(|c| {
                (
                    c.id.to_string(),
                    c.min_tests().map(NonZeroU32::get),
                    c.run.as_str().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            read,
            vec![
                ("lint".into(), None, "make lint".into()),
                (
                    "test".into(),
                    Some(36),
                    "cargo test --no-fail-fast -p calc".into()
                ),
                ("typecheck".into(), Some(1), "npx tsc --noEmit".into()),
            ],
            "{toml}"
        );
        assert!(toml.contains("keep = [\"target\"]"), "{toml}");
    }

    #[test]
    fn cargo_test_keeps_its_filter_and_package_and_reports_libtest() {
        let a = adhoc("cargo test -p aver -q div -- --nocapture");
        assert_eq!(a.runner, Some("cargo test"));
        assert_eq!(a.check.id.as_str(), ID);
        assert_eq!(
            a.check.argv,
            vec![
                "sh".to_owned(),
                "-c".into(),
                "cargo test --no-fail-fast -p aver div -- --nocapture >/aver/out/yours.txt 2>&1; s=$?; cat /aver/out/yours.txt; exit $s".into()
            ]
        );
        assert_eq!(a.check.report.unwrap().format, ReportFormat::Libtest);
    }

    #[test]
    fn go_test_gets_json_and_pytest_a_junit_file() {
        let go = adhoc("go test -run TestDiv ./...");
        assert_eq!(
            go.check.argv[2],
            "exec go test -json -run TestDiv ./... >/aver/out/yours.json"
        );
        assert_eq!(go.check.report.unwrap().format, ReportFormat::GoJson);
        let py = adhoc("pytest -k div tests/");
        assert_eq!(
            py.check.argv.join(" "),
            "python -m pytest -p no:cacheprovider -o junit_family=xunit1 -k div tests/ --junitxml=/aver/out/yours.xml"
        );
        assert_eq!(py.check.report.unwrap().format, ReportFormat::PytestJunit);
        let py3 = adhoc("python3 -m pytest -x");
        assert!(py3
            .check
            .argv
            .join(" ")
            .starts_with("python3 -m pytest -p no:cacheprovider"));
        let own = adhoc("pytest --junitxml=out.xml");
        assert!(own.check.report.is_none());
    }

    #[test]
    fn node_test_gets_its_reporters_before_the_file_patterns() {
        let a = adhoc("node --test test/div.test.js");
        assert_eq!(
            a.check.argv.join(" "),
            "node --test-reporter=spec --test-reporter-destination=stdout --test-reporter=junit --test-reporter-destination=/aver/out/yours.xml --test test/div.test.js"
        );
        assert_eq!(a.runner, Some("node:test"));
        assert_eq!(a.check.report.unwrap().format, ReportFormat::Junit);
    }

    #[test]
    fn npm_test_and_vitest_use_the_default_checks_reporters() {
        let pkg = br#"{"scripts":{"test":"vitest run"}}"#;
        let lock =
            br#"{"lockfileVersion":3,"packages":{"node_modules/vitest":{"version":"3.2.4"}}}"#;
        let npm = check_for(
            &words("npm test -- test/div.test.ts"),
            None,
            Some(pkg),
            Some(lock),
        )
        .unwrap();
        assert_eq!(
            npm.check.argv.join(" "),
            "npm test -- --reporter=default --reporter=junit --outputFile.junit=/aver/out/yours.xml test/div.test.ts"
        );
        assert_eq!(npm.runner, Some("vitest"));
        let direct = check_for(&words("npx vitest run div"), None, None, Some(lock)).unwrap();
        assert_eq!(
            direct.check.argv.join(" "),
            "npx vitest run div --reporter=default --reporter=junit --outputFile.junit=/aver/out/yours.xml"
        );
        // A runner the lockfile does not install, or one already
        // reporting, is judged by its exit code.
        assert!(check_for(&words("npx vitest run"), None, None, None)
            .unwrap()
            .check
            .report
            .is_none());
        let custom = check_for(
            &words("npm test"),
            None,
            Some(br#"{"scripts":{"test":"./run.sh"}}"#),
            None,
        )
        .unwrap();
        assert_eq!(custom.check.argv, words("npm test"));
        assert!(custom.runner.is_none());
    }

    #[test]
    fn other_commands_are_judged_by_exit_code() {
        let a = adhoc("make check");
        assert_eq!(a.check.argv, words("make check"));
        assert!(a.check.report.is_none() && a.runner.is_none());
        assert_eq!(
            adhoc("CI=1 ./test.sh").check.argv,
            words("env CI=1 ./test.sh")
        );
        assert_eq!(check_for(&[], None, None, None), Err(AdhocError::Empty));
        assert!(matches!(
            check_for(&words("aver verify"), None, None, None),
            Err(AdhocError::Aver(_))
        ));
    }

    #[test]
    fn near_picks_changed_tests_then_stem_matches_then_importers() {
        let tests: Vec<String> = [
            "test/add.test.js",
            "test/div.test.js",
            "test/calc.test.js",
            "test/other.test.js",
            "tests/test_div.py",
        ]
        .map(String::from)
        .to_vec();
        let changed: Vec<String> = ["src/div.js", "test/add.test.js", "README.md"]
            .map(String::from)
            .to_vec();
        let read = |p: &str| -> Option<String> {
            Some(match p {
                "test/calc.test.js" => "import { div } from '../src/div.js'".into(),
                "test/other.test.js" => "const divide = 1; // individual".into(),
                _ => String::new(),
            })
        };
        assert_eq!(
            near(&changed, &tests, &read),
            vec!["test/add.test.js", "test/div.test.js", "test/calc.test.js"]
        );
    }

    #[test]
    fn narrowed_commands_take_files_packages_or_crates() {
        let files = |s: &[&str]| s.iter().map(|f| (*f).to_owned()).collect::<Vec<_>>();
        let none = |_: &str| None;
        let py = narrowed(&files(&["tests/test_div.py"]), &[], &none).unwrap();
        assert_eq!(py.argv.join(" "), "python -m pytest tests/test_div.py");

        let go_read = |p: &str| (p == "calc/go.mod").then(|| b"module calc".to_vec());
        let go = narrowed(
            &files(&["calc/div/div_test.go"]),
            &files(&["calc/div/div.go", "calc/main.go"]),
            &go_read,
        )
        .unwrap();
        assert_eq!(go.argv.join(" "), "go test . ./div");
        assert_eq!(go.cwd.unwrap().as_str(), "calc");

        let rs_read = |p: &str| match p {
            "Cargo.toml" => Some(b"[workspace]\nmembers = [\"crates/*\"]".to_vec()),
            "crates/aver/Cargo.toml" => Some(b"[package]\nname = \"aver\"".to_vec()),
            _ => None,
        };
        let only_tests =
            narrowed(&files(&["crates/aver/tests/check_adhoc.rs"]), &[], &rs_read).unwrap();
        assert_eq!(
            only_tests.argv.join(" "),
            "cargo test -p aver --test check_adhoc"
        );
        let with_src = narrowed(
            &files(&["crates/aver/tests/check_adhoc.rs"]),
            &files(&["crates/aver/src/adhoc.rs"]),
            &rs_read,
        )
        .unwrap();
        assert_eq!(with_src.argv.join(" "), "cargo test -p aver");

        let pkg = |_: &str| None;
        let js = narrowed(&files(&["test/div.test.js"]), &[], &pkg).unwrap();
        assert_eq!(js.argv, vec!["npm", "test"]);
        assert!(js.note.contains("runs whole"), "{}", js.note);
        let node_pkg = |p: &str| {
            (p == "package.json").then(|| br#"{"scripts":{"test":"node --test"}}"#.to_vec())
        };
        let js = narrowed(&files(&["test/div.test.js"]), &[], &node_pkg).unwrap();
        assert_eq!(js.argv.join(" "), "npm test -- test/div.test.js");
    }

    fn outcome() -> Outcome {
        Outcome {
            command: "node --test test/div.test.js".into(),
            runner: Some("node:test".into()),
            shows: vec!["test/div.test.js::div rounds down".into(), "x".into()],
            already: vec!["test/div.test.js::div by zero".into()],
            passing: 4,
            ..Outcome::default()
        }
    }

    #[test]
    fn the_report_starts_and_ends_with_the_summary() {
        let o = outcome();
        assert_eq!(
            o.summary(),
            "Your command: 2 tests fail on the original and pass with yours; 0 new failures; 1 already failing"
        );
        let text = text(&o);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], o.summary());
        assert_eq!(
            *lines.last().unwrap(),
            format!(
                "{}. Not a verify: `aver verify` runs every affected check.",
                o.summary()
            )
        );
        assert!(text.contains("Already failing on the original (not yours) (1):\n```\ntest/div.test.js::div by zero\n```"));
        assert!(text.contains("Command:\n```\nnode --test test/div.test.js\n```"));
        assert!(!o.broke());
    }

    #[test]
    fn exit_code_commands_say_so() {
        let o = Outcome {
            command: "make check".into(),
            exit_only: Some(ExitOnly {
                original_passed: false,
                yours_passed: true,
            }),
            ..Outcome::default()
        };
        assert_eq!(
            o.summary(),
            "Your command: fails on the original, passes with your change (exit code only)"
        );
        assert!(text(&o).contains("by exit code: Aver cannot read per-test results"));
    }
}
