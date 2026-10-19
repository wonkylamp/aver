//! Focused reruns (ARCHITECTURE.md §1.9.3): a side whose first run failed
//! some tests and passed others is rerun only to see whether the failing
//! ones are flaky, so where the runner selects tests by name (`go test
//! -run`, pytest node ids, `cargo test -- --exact`, node:test
//! `--test-name-pattern`) its reruns run only those tests, and every other
//! test keeps its first run's result. The base's confirmation runs, which
//! ask whether the tests flaky on the head are flaky on the base too, run
//! only those tests the same way. A focused rerun that does not report
//! every test it was asked for, or whose exit code says more than its
//! tests, is not used: that rerun runs the full command.

use std::ops::Range;

use super::*;
use crate::adhoc::runner::Runner;
use crate::exec::cargo::CargoTest;
use crate::exec::gojson;

/// Most tests a rerun is focused on; a first run failing more reruns in
/// full.
pub(super) const MAX_FOCUSED: usize = 50;

/// Most failing tests a pytest-xdist run reruns serially ([`Focus::serial`]).
pub(super) const MAX_SERIAL: usize = 200;

/// A side's reruns narrowed to some of its first run's tests.
pub(super) struct Focus {
    pub(super) def: CheckDef,
    /// The test paths each focused rerun must report.
    ids: BTreeSet<String>,
    first: CheckRun,
}

impl Focus {
    /// The focus for the reruns after a side's first run: the tests it
    /// failed, when it failed 1 to [`MAX_FOCUSED`] tests, passed some, and
    /// failed only through its tests.
    pub(super) fn failing(def: &CheckDef, first: &CheckRun) -> Option<Self> {
        if ending(first, def) != Ending::Fail {
            return None;
        }
        let ids = first
            .tests
            .iter()
            .filter(|t| t.status == TestStatus::Fail)
            .map(|t| t.id.test_path().to_owned())
            .collect();
        Self::on(def, first, ids)
    }

    /// The focus on `ids`, tests `first` reported, when there are 1 to
    /// [`MAX_FOCUSED`] of them, `first` passed some other test, its exit
    /// code says what its tests say, and the command selects them by name.
    pub(super) fn on(def: &CheckDef, first: &CheckRun, ids: BTreeSet<String>) -> Option<Self> {
        Self::up_to(def, first, ids, MAX_FOCUSED)
    }

    /// The serial rerun of the tests a pytest run on pytest-xdist workers
    /// (`-n`) failed, up to [`MAX_SERIAL`] of them: a test that fails only
    /// beside others on a worker (a shared path, a port) is read as it runs
    /// in the serial suite, so workers change no failure's outcome.
    pub(super) fn serial(def: &CheckDef, first: &CheckRun) -> Option<Self> {
        if ending(first, def) != Ending::Fail
            || !pytest_call(def).is_some_and(|(_, o, _)| xdist(&o))
        {
            return None;
        }
        let ids = first
            .tests
            .iter()
            .filter(|t| t.status == TestStatus::Fail)
            .map(|t| t.id.test_path().to_owned())
            .collect();
        Self::up_to(def, first, ids, MAX_SERIAL)
    }

    fn up_to(def: &CheckDef, first: &CheckRun, ids: BTreeSet<String>, max: usize) -> Option<Self> {
        let passed_other = first
            .tests
            .iter()
            .any(|t| t.status == TestStatus::Pass && !ids.contains(t.id.test_path()));
        let reported: BTreeSet<&str> = first.tests.iter().map(|t| t.id.test_path()).collect();
        if ids.is_empty()
            || ids.len() > max
            || !passed_other
            || !ids.iter().all(|id| reported.contains(id.as_str()))
            || !consistent(def, first)
        {
            return None;
        }
        let def = select(def, &ids)?;
        Some(Self {
            def,
            ids,
            first: first.clone(),
        })
    }

    /// `run`, a focused rerun, as the full command's: the first run's
    /// tests with each test the rerun ran in its place. `None` when it did
    /// not report every test it was asked for, or its exit code says more
    /// than its tests.
    pub(super) fn fill(&self, check: &CheckDef, mut run: ExecutedRun) -> Option<ExecutedRun> {
        if !consistent(check, &run.run) {
            return None;
        }
        let ran: BTreeMap<&str, &aver_core::TestResult> = run
            .run
            .tests
            .iter()
            .map(|t| (t.id.test_path(), t))
            .collect();
        if !self.ids.iter().all(|id| ran.contains_key(id.as_str())) {
            return None;
        }
        // A test the selection did not ask for may still run (a pattern
        // matching more names) or be listed as skipped; a pass or a failure
        // there is a real result, a skip only says it was not selected.
        let rerun = |path: &str| {
            ran.get(path).copied().filter(|t| {
                self.ids.contains(path) || matches!(t.status, TestStatus::Pass | TestStatus::Fail)
            })
        };
        let mut tests: Vec<aver_core::TestResult> = self
            .first
            .tests
            .iter()
            .map(|t| rerun(t.id.test_path()).unwrap_or(t).clone())
            .collect();
        // Of the entries the full run never reported, only a failure is
        // kept: a pass there is the selection's own artifact, such as the
        // file-level case Node 25 writes for a file with no matching test,
        // which the head's full run lacks and would read as missing.
        let known: BTreeSet<&str> = self.first.tests.iter().map(|t| t.id.test_path()).collect();
        tests.extend(
            run.run
                .tests
                .iter()
                .filter(|t| !known.contains(t.id.test_path()) && t.status == TestStatus::Fail)
                .cloned(),
        );
        run.run.tests = Set::new(tests).ok()?;
        Some(run)
    }
}

/// Whether `def`'s command is one whose reruns Aver can focus, before any
/// run: it selects a stand-in test by name.
pub(super) fn can(def: &CheckDef) -> bool {
    let stand_in = format!("m.py{0}x src/lib.rs{0}x", junit::SEPARATOR);
    let go = format!("m{}TestX", junit::SEPARATOR);
    [stand_in, go]
        .into_iter()
        .any(|id| select(def, &BTreeSet::from([id])).is_some())
}

/// Whether a run's exit code says what its tests say: it failed exactly
/// when a test failed, and it has its report.
fn consistent(def: &CheckDef, run: &CheckRun) -> bool {
    let failed = run.tests.iter().any(|t| t.status == TestStatus::Fail);
    run.report.is_some()
        && match ending(run, def) {
            Ending::Pass => !failed,
            Ending::Fail => failed,
            _ => false,
        }
}

/// `def` running only the tests `ids` name, by its report format's runner.
pub(super) fn select(def: &CheckDef, ids: &BTreeSet<String>) -> Option<CheckDef> {
    use aver_core::ReportFormat::*;
    let parts =
        |id: &String| -> Vec<String> { id.split(junit::SEPARATOR).map(str::to_owned).collect() };
    let format = def.report.as_ref()?.format;
    if let Some(runner) = crate::adhoc::runner::runner_of(def) {
        return javascript(
            def,
            runner,
            &ids.iter()
                .map(|id| parts(id).pop().unwrap_or_default())
                .collect(),
        );
    }
    match format {
        GoJson => {
            let mut tests: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for id in ids {
                let (package, test) = id.split_once(junit::SEPARATOR)?;
                let top = test.split('/').next()?;
                if top == gojson::PACKAGE_FAILED {
                    return None;
                }
                tests
                    .entry(package.to_owned())
                    .or_default()
                    .insert(top.to_owned());
            }
            go(def, &tests)
        }
        PytestJunit => pytest(def, &ids.iter().map(|id| parts(id).join("::")).collect()),
        Libtest => {
            let (mut names, mut targets, mut units) = (BTreeSet::new(), BTreeSet::new(), false);
            let mut known = true;
            for id in ids {
                let (label, name) = id.split_once(junit::SEPARATOR)?;
                if label.ends_with("doc-tests") {
                    return None;
                }
                names.insert(name.to_owned());
                let src: Vec<&str> = label
                    .split_once(' ')
                    .map_or("", |l| l.1)
                    .split('/')
                    .collect();
                match src.as_slice() {
                    ["tests", file] if file.ends_with(".rs") => {
                        targets.insert(file.trim_end_matches(".rs").to_owned());
                    }
                    ["tests", dir, "main.rs"] => {
                        targets.insert((*dir).to_owned());
                    }
                    ["src", ..] => units = true,
                    _ => known = false,
                }
            }
            // A target named otherwise in its manifest makes `--test` fail
            // to select it, so that rerun reports none of the tests and the
            // full command runs in its place.
            let narrowed = known
                .then(|| CargoTest::only(def, &targets, units))
                .flatten();
            CargoTest::exact(narrowed.as_ref().unwrap_or(def), &names)
        }
        Junit => node_test(
            def,
            &ids.iter()
                .map(|id| parts(id).pop().unwrap_or_default())
                .collect(),
        ),
        _ => None,
    }
}

/// One runner invocation in a check's command: an argv, or a word of an
/// `sh -c` script, with the words of its arguments.
struct Call<'d> {
    def: &'d CheckDef,
    /// The words, and in a script their byte ranges.
    words: Vec<(String, Range<usize>)>,
    /// Which argv word is the script, when the command is one.
    script: Option<usize>,
    /// The runner's arguments among `words`.
    args: Range<usize>,
}

impl<'d> Call<'d> {
    /// The one invocation in `def` whose first words `runner` takes (it
    /// returns how many words name the runner), with every argument a
    /// plain word or one whose quotes close within it. `None` when there is
    /// none or more than one.
    fn find(def: &'d CheckDef, runner: impl Fn(&[String]) -> Option<usize>) -> Option<Self> {
        let at = match def.argv.as_slice() {
            [shell, flag, _]
                if matches!(shell.as_str(), "sh" | "bash" | "/bin/sh" | "/bin/bash")
                    && flag == "-c" =>
            {
                Some(2)
            }
            // The default check's script form (`adhoc::runner::read`).
            [npm, exec, call, _] if [npm, exec, call] == ["npm", "exec", "--call"] => Some(3),
            _ => None,
        };
        let words: Vec<(String, Range<usize>)> = match at {
            Some(at) => {
                let script = &def.argv[at];
                script
                    .split_whitespace()
                    .map(|w| {
                        let at = w.as_ptr() as usize - script.as_ptr() as usize;
                        (w.to_owned(), at..at + w.len())
                    })
                    .collect()
            }
            None => def.argv.iter().map(|w| (w.clone(), 0..0)).collect(),
        };
        let script = at.is_some();
        let plain: Vec<String> = words.iter().map(|(w, _)| w.clone()).collect();
        let starts = |i: usize| {
            i == 0
                || (script
                    && matches!(
                        plain[i - 1].as_str(),
                        ";" | "&&" | "||" | "|" | "(" | "exec" | "then" | "do" | "else"
                    ))
                || (script && plain[i - 1].ends_with(';'))
        };
        let mut found = None;
        for i in (0..plain.len()).filter(|&i| starts(i)) {
            let at = wrapped(&plain, i);
            if let Some(n) = runner(&plain[at..]) {
                if found.is_some() {
                    return None;
                }
                found = Some(at + n);
            }
        }
        let start = found?;
        let end = (start..plain.len())
            .find(|&i| script && ends_args(&plain[i]))
            .unwrap_or(plain.len());
        let balanced = |w: &str| {
            w.matches('"').count().is_multiple_of(2) && w.matches('\'').count().is_multiple_of(2)
        };
        let glued = |w: &str| {
            w.contains([';', '&', '|', '`'])
                || w.contains("$(")
                || (!w.starts_with('-') && w.contains('$'))
        };
        if plain[start..end]
            .iter()
            .any(|w| !balanced(w) || (script && glued(w)))
        {
            return None;
        }
        Some(Self {
            def,
            words,
            script: at,
            args: start..end,
        })
    }

    fn args(&self) -> Vec<&str> {
        self.words[self.args.clone()]
            .iter()
            .map(|(w, _)| w.as_str())
            .collect()
    }

    /// A word as the command takes it: as is in an argv, quoted where the
    /// shell would read it differently in a script.
    fn word(&self, w: &str) -> String {
        let plain = w
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./:=,+@%-".contains(&b));
        if self.script.is_none() || (plain && !w.is_empty()) {
            w.to_owned()
        } else {
            format!("'{}'", w.replace('\'', r"'\''"))
        }
    }

    /// `def` with the runner's arguments replaced by `args` (words as
    /// [`Self::word`] gives them).
    fn with(&self, args: Vec<String>) -> CheckDef {
        let mut def = self.def.clone();
        if let Some(at) = self.script {
            let script = &self.def.argv[at];
            let from = self.words[..self.args.start]
                .last()
                .map_or(0, |(_, r)| r.end);
            let to = self
                .words
                .get(self.args.end)
                .map_or(script.len(), |(_, r)| r.start);
            let tail = &script[to..];
            def.argv[at] = format!(
                "{} {}{}{tail}",
                &script[..from],
                args.join(" "),
                if tail.is_empty() { "" } else { " " }
            );
        } else {
            let mut argv: Vec<String> = self.def.argv[..self.args.start].to_vec();
            argv.extend(args);
            argv.extend(self.def.argv[self.args.end..].iter().cloned());
            def.argv = argv;
        }
        def
    }
}

/// Where the command starting at word `i` names its program, past
/// environment assignments, `env` and a project tool's `run` (`uv run`,
/// `poetry run` and the like) with their options.
fn wrapped(words: &[String], mut i: usize) -> usize {
    loop {
        match words.get(i).map(String::as_str) {
            Some(w) if assignment(w) || w == "env" => i += 1,
            Some("uv" | "poetry" | "pipenv" | "pdm" | "rye" | "hatch")
                if words.get(i + 1).map(String::as_str) == Some("run") =>
            {
                i += 2;
                while words.get(i).is_some_and(|w| w.starts_with('-')) {
                    i += 1;
                }
            }
            _ => return i,
        }
    }
}

fn assignment(w: &str) -> bool {
    w.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Whether a script word ends a command's arguments: an operator or a
/// redirection.
fn ends_args(w: &str) -> bool {
    matches!(w, ";" | "&&" | "||" | "|" | "&" | ")")
        || w.trim_start_matches(|c: char| c.is_ascii_digit())
            .starts_with(['>', '<'])
}

fn program(w: &str) -> &str {
    w.rsplit('/').next().unwrap_or(w)
}

/// `go test` with `-run '^(A|B)$'` before its arguments and its one
/// `./...` replaced by the packages `tests` names; `None` when the command
/// already selects tests or a name is not a plain Go identifier.
fn go(def: &CheckDef, tests: &BTreeMap<String, BTreeSet<String>>) -> Option<CheckDef> {
    let call = Call::find(def, |w| {
        (w.first()? == "go" && w.get(1)? == "test").then_some(2)
    })?;
    let args = call.args();
    let selects = |a: &&str| ["-run", "-skip", "-list"].iter().any(|f| a.starts_with(f));
    if args.iter().any(selects) || args.iter().filter(|a| **a == "./...").count() != 1 {
        return None;
    }
    let ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_');
    let names: BTreeSet<&str> = tests.values().flatten().map(String::as_str).collect();
    let package = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_alphanumeric() || "/._-~".contains(c))
    };
    if !names.iter().all(|n| ident(n)) || !tests.keys().all(|p| package(p)) {
        return None;
    }
    let pattern = format!("^({})$", names.into_iter().collect::<Vec<_>>().join("|"));
    let mut out = vec!["-run".to_owned(), call.word(&pattern)];
    for a in args {
        if a == "./..." {
            out.extend(tests.keys().cloned());
        } else {
            out.push(a.to_owned());
        }
    }
    Some(call.with(out))
}

/// pytest options whose value is the next word unless written
/// `--option=value`.
const PYTEST_VALUED: &[&str] = &[
    "-k",
    "-m",
    "-p",
    "-c",
    "-o",
    "-W",
    "-n",
    "--numprocesses",
    "-r",
    "--rootdir",
    "--junitxml",
    "--junit-xml",
    "--basetemp",
    "--confcutdir",
    "--ignore",
    "--ignore-glob",
    "--deselect",
    "--tb",
    "--maxfail",
    "--durations",
    "--import-mode",
    "--override-ini",
    "--capture",
    "--dist",
    "--cov-report",
    "--cov-config",
    "--log-level",
    "--log-cli-level",
    "--timeout",
    "--reruns",
    "--color",
    "--junit-prefix",
    "--pythonwarnings",
    "--maxprocesses",
];

/// pytest with its own path arguments replaced by the node ids `ids`
/// (`file.py::Class::name[param]`); `None` when its arguments cannot be
/// told apart or already choose tests by state.
/// It runs serially: a selection is a handful of tests, which workers
/// would only slow, and [`Focus::serial`] needs their serial outcomes.
pub(super) fn pytest(def: &CheckDef, ids: &BTreeSet<String>) -> Option<CheckDef> {
    let (call, options, _) = pytest_call(def)?;
    let mut kept = Vec::new();
    let mut options = options.into_iter();
    while let Some(o) = options.next() {
        if XDIST_VALUED.contains(&o.as_str()) {
            options.next();
        } else if !xdist(std::slice::from_ref(&o)) {
            kept.push(o);
        }
    }
    kept.extend(ids.iter().map(|id| call.word(id)));
    Some(call.with(kept))
}

/// pytest-xdist's options that take the next word as their value.
const XDIST_VALUED: &[&str] = &["-n", "--numprocesses", "--maxprocesses", "--dist"];

/// Whether pytest options run the tests on pytest-xdist workers, or name
/// how (`-n auto`, `-n4`, `--numprocesses=auto`, `--dist load`).
fn xdist(options: &[String]) -> bool {
    options.iter().any(|o| {
        XDIST_VALUED.contains(&o.as_str())
            || XDIST_VALUED
                .iter()
                .any(|f| f.starts_with("--") && o.starts_with(&format!("{f}=")))
            || (o.starts_with("-n") && !o.starts_with("--") && o.len() > 2)
    })
}

/// The paths pytest's command names to collect from (none: its
/// configuration's `testpaths`, else its directory); `None` as for
/// [`pytest`].
pub(super) fn pytest_paths(def: &CheckDef) -> Option<Vec<String>> {
    pytest_call(def).map(|(_, _, paths)| paths)
}

/// The one pytest call in `def`, its options and its path arguments.
fn pytest_call(def: &CheckDef) -> Option<(Call<'_>, Vec<String>, Vec<String>)> {
    let call = Call::find(def, |w| match w {
        [p, ..] if matches!(program(p), "pytest" | "py.test") => Some(1),
        [py, m, p, ..] if program(py).starts_with("python") && m == "-m" && p == "pytest" => {
            Some(3)
        }
        _ => None,
    })?;
    let (mut options, mut paths) = (Vec::new(), Vec::new());
    let mut args = call.args().into_iter();
    while let Some(a) = args.next() {
        if [
            "--lf",
            "--last-failed",
            "--ff",
            "--failed-first",
            "--sw",
            "--stepwise",
            "--pyargs",
        ]
        .contains(&a)
        {
            return None;
        }
        if a.starts_with('-') {
            options.push(a.to_owned());
            if PYTEST_VALUED.contains(&a) {
                options.push(args.next()?.to_owned());
            }
        } else {
            paths.push(a.to_owned());
        }
    }
    Some((call, options, paths))
}

/// `node --test` with `--test-name-pattern` matching the tests `names`;
/// `None` when the command already selects tests by name.
fn node_test(def: &CheckDef, names: &BTreeSet<String>) -> Option<CheckDef> {
    let call = Call::find(def, |w| (program(w.first()?) == "node").then_some(1))?;
    let args = call.args();
    if !args.contains(&"--test")
        || args.iter().any(|a| {
            ["--test-name-pattern", "--test-skip-pattern", "--test-only"]
                .iter()
                .any(|f| a.starts_with(f))
        })
        || names.iter().any(String::is_empty)
    {
        return None;
    }
    let pattern = names
        .iter()
        .map(|n| regex::escape(n))
        .collect::<Vec<_>>()
        .join("|");
    let mut out = vec![call.word(&format!("--test-name-pattern={pattern}"))];
    out.extend(args.into_iter().map(str::to_owned));
    Some(call.with(out))
}

/// A default check (`adhoc::runner`: `npm test` with Aver's reporter, or
/// the script through `npm exec --call`) running only the tests whose
/// names `names` match: the runner's own name filter (node:test's
/// `--test-name-pattern`, in `--node-options` for `npm test`; `-t` for
/// Vitest and Jest; `--grep` for Mocha), an unanchored pattern of the
/// escaped names; for ava, one `--match` per name, a glob without
/// wildcards that matches that title whole. Runners match a test's full
/// title against it, so a test of the same name elsewhere also reruns;
/// [`Focus::fill`] reads only what it needs.
fn javascript(def: &CheckDef, runner: Runner, names: &BTreeSet<String>) -> Option<CheckDef> {
    if names.iter().any(String::is_empty) {
        return None;
    }
    let pattern = names
        .iter()
        .map(|n| regex::escape(n))
        .collect::<Vec<_>>()
        .join("|");
    let flag = match runner {
        Runner::NodeTest => "--test-name-pattern",
        Runner::Vitest | Runner::Jest => "-t",
        Runner::Mocha => "--grep",
        Runner::Ava => "--match",
        Runner::Tap => return None,
    };
    // Ava reads `*` as a wildcard and a leading `!` as a negation.
    if runner == Runner::Ava && names.iter().any(|n| n.contains('*') || n.starts_with('!')) {
        return None;
    }
    if def.argv == runner.check(def.id.as_str()).0 {
        let mut focused = def.clone();
        if runner == Runner::NodeTest {
            // npm sets NODE_OPTIONS from it, which Node splits at spaces
            // outside double quotes, unescaping backslashes inside them.
            let options = focused
                .argv
                .iter_mut()
                .find(|a| a.starts_with("--node-options="))?;
            let quoted = pattern.replace('\\', "\\\\").replace('"', "\\\"");
            options.push_str(&format!(" {flag}=\"{quoted}\""));
        } else {
            focused.argv.extend([flag.to_owned(), pattern]);
        }
        return Some(focused);
    }
    if runner == Runner::NodeTest {
        return node_test(def, names);
    }
    let call = Call::find(def, |w| {
        let mut i = 0;
        while matches!(w.get(i).map(|p| program(p)), Some("npx" | "c8" | "nyc")) {
            i += 1;
            while w.get(i).is_some_and(|a| a.starts_with('-')) {
                i += 1;
            }
        }
        let (named, words) = match program(w.get(i)?) {
            "vitest" => (Runner::Vitest, 1),
            "vp" if w.get(i + 1).is_some_and(|t| t == "test") => (Runner::Vitest, 2),
            "jest" => (Runner::Jest, 1),
            "mocha" | "_mocha" => (Runner::Mocha, 1),
            "ava" => (Runner::Ava, 1),
            _ => return None,
        };
        (named == runner).then_some(i + words)
    })?;
    let args = call.args();
    let selecting = [
        "-t",
        "--testNamePattern",
        "--grep",
        "-g",
        "--fgrep",
        "--invert",
        "--only",
        "--match",
        "-m",
    ];
    if args.iter().any(|a| {
        selecting
            .iter()
            .any(|f| a == f || a.starts_with(&format!("{f}=")))
    }) {
        return None;
    }
    let mut out: Vec<String> = args.into_iter().map(str::to_owned).collect();
    if runner == Runner::Ava {
        for name in names {
            out.extend([flag.to_owned(), call.word(name)]);
        }
    } else {
        out.extend([flag.to_owned(), call.word(&pattern)]);
    }
    Some(call.with(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(format: aver_core::ReportFormat, argv: &[&str]) -> CheckDef {
        let mut def = crate::presets::default_check(None, None, &|_| false);
        def.argv = argv.iter().map(|s| (*s).to_owned()).collect();
        def.report = Some(aver_core::Report {
            format,
            path: "/aver/out/r".parse().unwrap(),
        });
        def
    }

    fn ids(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|s| (*s).to_owned()).collect()
    }

    fn sh(format: aver_core::ReportFormat, script: &str, of: &[&str]) -> Option<String> {
        select(&check(format, &["sh", "-c", script]), &ids(of)).map(|d| d.argv[2].clone())
    }

    #[test]
    fn a_go_rerun_runs_only_the_failing_tests_of_their_packages() {
        use aver_core::ReportFormat::GoJson;
        assert_eq!(
            sh(
                GoJson,
                "exec go test -json ./... >/aver/out/go.json",
                &["m/a > TestX/sub", "m/a > TestY", "m/b > TestZ"]
            )
            .as_deref(),
            Some("exec go test -run '^(TestX|TestY|TestZ)$' -json m/a m/b >/aver/out/go.json")
        );
        assert_eq!(
            sh(
                GoJson,
                "go vet ./... && go test -race -json ./... > $AVER_OUT/go.json",
                &["m > TestX"]
            )
            .as_deref(),
            Some("go vet ./... && go test -run '^(TestX)$' -race -json m > $AVER_OUT/go.json")
        );
    }

    #[test]
    fn a_command_that_already_selects_or_is_not_plain_reruns_in_full() {
        use aver_core::ReportFormat::*;
        for script in [
            "go test -json -run TestA ./... >o",
            "go test -json ./a/... >o",
            "go test -json $PKGS ./... >o",
            "echo go test -json ./... >o",
            "go test -json ./... >o; go test -json ./... >p",
        ] {
            assert_eq!(sh(GoJson, script, &["m > TestX"]), None, "{script}");
        }
        assert_eq!(sh(GoJson, "go test -json ./... >o", &["m > Test[X]"]), None);
        assert_eq!(
            sh(GoJson, "go test -json ./... >o", &["m > (package)"]),
            None
        );
        assert_eq!(
            sh(PytestJunit, "pytest --lf --junitxml=o", &["t.py > test_a"]),
            None
        );
    }

    #[test]
    fn a_pytest_rerun_runs_the_failing_node_ids_in_place_of_its_paths() {
        use aver_core::ReportFormat::PytestJunit;
        let def = check(
            PytestJunit,
            &[
                "env",
                "PYTHONPATH=src",
                "python",
                "-m",
                "pytest",
                "-p",
                "no:cacheprovider",
                "-o",
                "junit_family=xunit1",
                "tests",
                "--junitxml=/aver/out/py.xml",
            ],
        );
        let focused = select(&def, &ids(&["tests/test_a.py > TestC > test_x[a b]"])).unwrap();
        assert_eq!(
            focused.argv,
            [
                "env",
                "PYTHONPATH=src",
                "python",
                "-m",
                "pytest",
                "-p",
                "no:cacheprovider",
                "-o",
                "junit_family=xunit1",
                "--junitxml=/aver/out/py.xml",
                "tests/test_a.py::TestC::test_x[a b]",
            ]
        );
        assert_eq!(
            sh(
                PytestJunit,
                "uv run pytest -q -k calc tests/ --junitxml=\"$AVER_OUT\"/py.xml && echo done",
                &["tests/test_a.py > test_x[1]"]
            )
            .as_deref(),
            Some("uv run pytest -q -k calc --junitxml=\"$AVER_OUT\"/py.xml 'tests/test_a.py::test_x[1]' && echo done")
        );
    }

    #[test]
    fn a_pytest_selection_runs_without_xdist_workers() {
        use aver_core::ReportFormat::PytestJunit;
        for script in [
            "pytest -n auto --junitxml=o tests",
            "pytest -n4 --junitxml=o",
            "pytest --numprocesses=auto --dist loadfile --junitxml=o",
            "pytest --numprocesses 2 --maxprocesses=3 --junitxml=o",
        ] {
            assert_eq!(
                sh(PytestJunit, script, &["t.py > test_a"]).as_deref(),
                Some("pytest --junitxml=o t.py::test_a"),
                "{script}"
            );
            let def = check(PytestJunit, &["sh", "-c", script]);
            assert!(xdist(&pytest_call(&def).unwrap().1), "{script}");
        }
        let plain = check(PytestJunit, &["sh", "-c", "pytest -q --junitxml=o"]);
        assert!(!xdist(&pytest_call(&plain).unwrap().1));
    }

    #[test]
    fn a_cargo_rerun_runs_the_failing_tests_by_exact_name() {
        use aver_core::ReportFormat::Libtest;
        let script = "cargo test --workspace --lib --tests --examples --no-fail-fast --locked >/aver/out/rust.txt 2>&1; s=$?; cat /aver/out/rust.txt; exit $s";
        assert_eq!(
            sh(
                Libtest,
                script,
                &["aver tests/api.rs > a::b", "aver_core src/lib.rs > c"]
            )
            .as_deref(),
            Some("cargo test --workspace --locked --lib --bins --test api --no-fail-fast -- --exact a::b c >/aver/out/rust.txt 2>&1; s=$?; cat /aver/out/rust.txt; exit $s")
        );
        assert_eq!(
            sh(
                Libtest,
                script,
                &["place tests/place/main.rs > a", "x_y tests/x-y.rs > b"]
            )
            .as_deref(),
            Some("cargo test --workspace --locked --test place --test x-y --no-fail-fast -- --exact a b >/aver/out/rust.txt 2>&1; s=$?; cat /aver/out/rust.txt; exit $s")
        );
        // A target Aver cannot place reruns every binary.
        assert_eq!(
            sh(Libtest, script, &["calc benches/b.rs > a"]).as_deref(),
            Some("cargo test --workspace --lib --tests --examples --no-fail-fast --locked -- --exact a >/aver/out/rust.txt 2>&1; s=$?; cat /aver/out/rust.txt; exit $s")
        );
        assert_eq!(
            sh(Libtest, script, &["aver_core doc-tests > src/lib.rs - x"]),
            None
        );
        // This repository's own check before 0.4, with its Linux settings.
        let older = "if [ \"$(uname)\" = Linux ]; then export CARGO_BUILD_JOBS=2; fi; cargo test --workspace --lib --bins --tests --no-fail-fast --locked >/aver/out/rust.txt 2>&1; s=$?; cat /aver/out/rust.txt; exit $s";
        assert_eq!(
            sh(Libtest, older, &["aver tests/api.rs > a::b"]).as_deref(),
            Some("if [ \"$(uname)\" = Linux ]; then export CARGO_BUILD_JOBS=2; fi; cargo test --workspace --locked --test api --no-fail-fast -- --exact a::b >/aver/out/rust.txt 2>&1; s=$?; cat /aver/out/rust.txt; exit $s")
        );
        assert_eq!(
            sh(
                Libtest,
                "cargo test --no-fail-fast parse >o",
                &["a src/lib.rs > b"]
            ),
            None
        );
    }

    #[test]
    fn a_default_npm_test_rerun_passes_the_names_to_its_runner() {
        let names = ids(&["math > add (x)", "math > sub \"y\""]);
        let default = |runner: Runner| {
            let mut def = crate::presets::default_check(None, None, &|_| false);
            let (argv, report) = runner.check(def.id.as_str());
            def.argv = argv;
            def.report = Some(report);
            def
        };
        let tail = |runner: Runner| {
            let def = default(runner);
            let focused = select(&def, &names).unwrap();
            focused.argv[def.argv.len() - 1..].join(" ")
        };
        assert!(
            tail(Runner::NodeTest).ends_with(r#" --test-name-pattern="add \\(x\\)|sub \"y\"""#),
            "{}",
            tail(Runner::NodeTest)
        );
        assert!(tail(Runner::Jest).ends_with(r#" -t add \(x\)|sub "y""#));
        assert!(tail(Runner::Vitest).ends_with(r#" -t add \(x\)|sub "y""#));
        assert!(tail(Runner::Mocha).ends_with(r#" --grep add \(x\)|sub "y""#));
        assert!(select(&default(Runner::Tap), &names).is_none());

        // The script form, through `npm exec --call`.
        let call = |script: &str, format| {
            let mut def = check(format, &["npm", "exec", "--call", script]);
            def.report.as_mut().unwrap().path = "/aver/out/test.xml".parse().unwrap();
            select(&def, &ids(&["a > b c"])).map(|d| d.argv[3].clone())
        };
        use aver_core::ReportFormat::*;
        assert_eq!(
            call("node --test-reporter=junit --test-reporter-destination=/aver/out/test.xml --test && tsc", Junit).as_deref(),
            Some("node '--test-name-pattern=b c' --test-reporter=junit --test-reporter-destination=/aver/out/test.xml --test && tsc")
        );
        assert_eq!(
            call("xo && c8 mocha test/ --reporter xunit --reporter-option output=/aver/out/test.xml && tsc", MochaXunit).as_deref(),
            Some("xo && c8 mocha test/ --reporter xunit --reporter-option output=/aver/out/test.xml --grep 'b c' && tsc")
        );
        assert_eq!(
            call(
                "mocha --grep slow --reporter xunit --reporter-option output=/aver/out/test.xml",
                MochaXunit
            ),
            None
        );
    }

    #[test]
    fn an_ava_or_vite_plus_rerun_passes_the_titles_to_its_runner() {
        use aver_core::ReportFormat::*;
        let call = |script: &str, format, of: &[&str]| {
            let mut def = check(format, &["npm", "exec", "--call", script]);
            def.report.as_mut().unwrap().path = "/aver/out/test.tap".parse().unwrap();
            select(&def, &ids(of)).map(|d| d.argv[3].clone())
        };
        let chalk = "xo && c8 ava --tap >/aver/out/test.tap && tsc --noEmit";
        assert_eq!(
            call(chalk, Tap, &["chalk > [crit:x] keeps b c", "level > d"]).as_deref(),
            Some("xo && c8 ava --tap --match '[crit:x] keeps b c' --match d >/aver/out/test.tap && tsc --noEmit")
        );
        assert_eq!(call(chalk, Tap, &["chalk > a * b"]), None, "a wildcard");
        assert_eq!(call(chalk, Tap, &["chalk > !a"]), None, "a negation");
        assert_eq!(
            call(
                "ava --match 'x*' --tap >/aver/out/test.tap",
                Tap,
                &["a > b"]
            ),
            None,
            "already selects"
        );
        assert_eq!(
            call("tsc && vp test --run --reporter=junit --outputFile.junit=/aver/out/test.xml", Junit, &["a.test.ts > b c"]).as_deref(),
            Some("tsc && vp test --run --reporter=junit --outputFile.junit=/aver/out/test.xml -t 'b c'")
        );
    }

    #[test]
    fn a_node_test_rerun_matches_the_failing_test_names() {
        let def = check(
            aver_core::ReportFormat::Junit,
            &[
                "node",
                "--test",
                "--test-reporter=junit",
                "test/**/*.test.ts",
            ],
        );
        let focused = select(&def, &ids(&["test/a.test.ts > suite > adds (1+1)"])).unwrap();
        assert_eq!(
            focused.argv,
            [
                "node",
                r"--test-name-pattern=adds \(1\+1\)",
                "--test",
                "--test-reporter=junit",
                "test/**/*.test.ts"
            ]
        );
    }
}
