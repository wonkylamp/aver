//! Test runners Aver knows (ARCHITECTURE.md §2.7.1): which one a command
//! invokes, the built-in machine-readable reporter it can add, writing to
//! `/aver/out`, so a run is read test by test, and how `aver check` narrows
//! it to named tests or files. Anything Aver cannot read with confidence
//! (shell syntax beyond `&&`, a runner already configured to report, watch
//! mode, a runner the lockfile does not install) keeps the command as
//! written: a declared report a runner never writes would turn every
//! passing run into a failure.

use aver_core::{CheckDef, Report, ReportFormat};
use serde_json::Value;

use crate::env::DEFAULT_CHECK_ARGV;

/// A test runner whose reporter the default check can add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    NodeTest,
    Vitest,
    Jest,
    Mocha,
    Tap,
    Ava,
}

impl Runner {
    pub fn name(self) -> &'static str {
        match self {
            Self::NodeTest => "node:test",
            Self::Vitest => "vitest",
            Self::Jest => "jest",
            Self::Mocha => "mocha",
            Self::Tap => "tap",
            Self::Ava => "ava",
        }
    }

    /// The `npm test` argv that adds this runner's reporter, and the report
    /// it writes. Ava prints its TAP stream to standard output, so its check
    /// is always the script itself with that output sent to the report
    /// ([`read`]), never this argv.
    pub(crate) fn check(self, id: &str) -> (Vec<String>, Report) {
        let xml = format!("/aver/out/{id}.xml");
        let (args, format, path): (Vec<String>, ReportFormat, String) = match self {
            // Options after a test file pattern would be read as patterns, so
            // the reporters go through NODE_OPTIONS, which npm sets for the
            // scripts it runs.
            Self::NodeTest => (
                vec![format!(
                    "--node-options=--test-reporter=spec --test-reporter-destination=stdout --test-reporter=junit --test-reporter-destination={xml}"
                )],
                ReportFormat::Junit,
                xml,
            ),
            Self::Vitest => (
                vec![
                    "--".into(),
                    "--reporter=default".into(),
                    "--reporter=junit".into(),
                    format!("--outputFile.junit={xml}"),
                ],
                ReportFormat::Junit,
                xml,
            ),
            Self::Jest => {
                let json = format!("/aver/out/{id}.json");
                (
                    vec!["--".into(), "--json".into(), format!("--outputFile={json}")],
                    ReportFormat::JestJson,
                    json,
                )
            }
            Self::Mocha => (
                vec![
                    "--".into(),
                    "--reporter".into(),
                    "xunit".into(),
                    "--reporter-option".into(),
                    format!("output={xml}"),
                ],
                ReportFormat::MochaXunit,
                xml,
            ),
            Self::Tap => (
                vec![
                    "--".into(),
                    "--reporter=junit".into(),
                    format!("--reporter-file={xml}"),
                ],
                ReportFormat::Junit,
                xml,
            ),
            Self::Ava => (
                vec!["--".into(), "--tap".into()],
                ReportFormat::Tap,
                format!("/aver/out/{id}.tap"),
            ),
        };
        let mut argv: Vec<String> = DEFAULT_CHECK_ARGV.iter().map(|s| (*s).to_owned()).collect();
        argv.extend(args);
        let report = Report {
            format,
            path: path.parse().expect("a report path under /aver/out"),
        };
        (argv, report)
    }
}

/// `argv` with its runner's reporter added, when it invokes a runner
/// [`runner`] accepts (Vitest, Jest, Mocha, tap or node:test, directly or
/// through `npx`, `cross-env` or a coverage wrapper), writing a report
/// named after check `id`: the argv `aver check -- <cmd>` runs
/// (§2.7.1). Runner options go right after the program for node:test,
/// whose later arguments are file patterns, and last for the others.
pub fn reporter(
    argv: &[String],
    lockfile: Option<&[u8]>,
    id: &str,
) -> Option<(Vec<String>, Report)> {
    let script = argv
        .iter()
        .map(|a| format!("'{}'", a.replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join(" ");
    let runner = runner(&script, &Lockfile::parse(lockfile))?;
    let (_, args, _) = invocation(argv)?;
    Some(instrument(runner, argv, argv.len() - args.len(), id))
}

/// The runner whose reporter a default check added, read back from its
/// command.
pub fn runner_of(check: &CheckDef) -> Option<Runner> {
    if let [npm, exec, call, script] = &check.argv[..] {
        if [npm, exec, call].map(String::as_str) == ["npm", "exec", "--call"] {
            let script = script.replace(&ava_output(check.id.as_str()), "");
            return split_and(&script)?
                .iter()
                .filter_map(|s| tokenize(s))
                .find_map(|t| invocation(&t).map(|(runner, _, _)| runner));
        }
    }
    [
        Runner::NodeTest,
        Runner::Vitest,
        Runner::Jest,
        Runner::Mocha,
        Runner::Tap,
    ]
    .into_iter()
    .find(|r| r.check(check.id.as_str()).0 == check.argv)
}

/// The redirection that sends ava's TAP stream to check `id`'s report.
fn ava_output(id: &str) -> String {
    format!(" >/aver/out/{id}.tap")
}

/// The versions a lockfile installs, by package name, from the
/// `packages["node_modules/<name>"]` entries of a v2 or v3 lockfile or the
/// `dependencies` of a v1 lockfile; for a package it does not list (no npm
/// lockfile: pnpm, Yarn, Bun, `package-lock=false`), the lower bound of the
/// range `package.json` asks for, which every version the range admits
/// meets; for a package neither lists, the workspace root's install, which
/// a workspace package without a lockfile of its own resolves from.
pub struct Lockfile {
    lock: Option<Value>,
    manifest: Option<Value>,
    root: Option<Box<Lockfile>>,
}

impl Lockfile {
    pub fn parse(bytes: Option<&[u8]>) -> Self {
        Self {
            lock: bytes.and_then(|b| serde_json::from_slice(b).ok()),
            manifest: None,
            root: None,
        }
    }

    /// The same, falling back to the ranges of `manifest` (`package.json`).
    pub fn with_manifest(mut self, manifest: Option<&[u8]>) -> Self {
        self.manifest = manifest.and_then(|b| serde_json::from_slice(b).ok());
        self
    }

    /// The same, falling back to `root`: the workspace root's install.
    pub fn with_root(mut self, root: Option<Lockfile>) -> Self {
        self.root = root.map(Box::new);
        self
    }

    fn version(&self, name: &str) -> Option<(u64, u64)> {
        let locked = self.lock.as_ref().and_then(|lock| {
            lock["packages"][format!("node_modules/{name}")]["version"]
                .as_str()
                .or_else(|| lock["dependencies"][name]["version"].as_str())
        });
        match locked {
            Some(version) => major_minor(version),
            None => self
                .manifest
                .as_ref()
                .and_then(|manifest| {
                    ["devDependencies", "dependencies"]
                        .iter()
                        .find_map(|field| manifest[field][name].as_str())
                })
                .and_then(lower_bound)
                .or_else(|| self.root.as_ref()?.version(name)),
        }
    }

    /// Whether the lockfile installs `name` at `min` or later.
    fn has(&self, name: &str, min: (u64, u64)) -> bool {
        self.version(name).is_some_and(|v| v >= min)
    }
}

fn major_minor(version: &str) -> Option<(u64, u64)> {
    let mut parts = version.split(['.', '-', '+']);
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

/// The least version a `package.json` range admits, for the plain forms
/// whose every match is at least it (`1.2.3`, `^1.2`, `~1`, `>=6`, `1.x`);
/// `None` for tags, protocols (`workspace:`, `npm:`, `file:`), unions and
/// upper bounds.
fn lower_bound(range: &str) -> Option<(u64, u64)> {
    let plain = range
        .trim()
        .trim_start_matches(['^', '~'])
        .trim_start_matches(">=")
        .trim_start_matches('v');
    if !plain.starts_with(|c: char| c.is_ascii_digit())
        || plain.contains(|c: char| c.is_whitespace() || matches!(c, '<' | '|' | ':' | '/'))
    {
        return None;
    }
    let plain = plain.replace(['x', 'X', '*'], "0");
    major_minor(&plain)
}

/// Why the default check reads a `test` script by its exit code alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitOnly {
    /// Shell syntax beyond commands joined by `&&`.
    Shell,
    /// None of the script's commands is a runner Aver reads.
    NoRunner(Vec<String>),
    /// More than one command runs a test runner.
    Several,
    /// The runner runs in a mode without a report (watch, a subcommand).
    Mode(Runner, String),
    /// The script already sets an option Aver's reporter needs.
    Configured(Runner, String),
    /// Neither the lockfile nor `package.json` shows a version with the
    /// reporter.
    NotInstalled(Runner, (u64, u64)),
}

impl std::fmt::Display for ExitOnly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shell => f.write_str(
                "its `test` script uses shell syntax beyond commands joined by `&&`, so Aver cannot tell which command runs the tests",
            ),
            Self::NoRunner(programs) => {
                let named: Vec<String> = programs.iter().map(|p| format!("`{p}`")).collect();
                let (list, which) = match named.split_last() {
                    Some((last, [])) => (last.clone(), "which is not"),
                    Some((last, rest)) => (format!("{} and {last}", rest.join(", ")), "none of them"),
                    None => (String::new(), "none of them"),
                };
                write!(
                    f,
                    "its `test` script runs {list}, {which} a test runner Aver reads per test (node:test, Vitest, Jest, Mocha, tap, ava)"
                )
            }
            Self::Several => f.write_str("its `test` script runs more than one test runner"),
            Self::Mode(runner, mode) => {
                write!(f, "its `test` script runs `{}` {mode}", runner.name())
            }
            Self::Configured(runner, option) => write!(
                f,
                "its `test` script already gives `{}` `{option}`, which Aver's reporter would need",
                runner.name()
            ),
            Self::NotInstalled(runner, (major, minor)) => write!(
                f,
                "neither the lockfile nor `package.json` shows `{}` {major}.{minor} or later, the first version with the reporter Aver adds",
                runner.name()
            ),
        }
    }
}

/// The runner a `test` script invokes, when `npm test` can add its
/// reporter: the script is commands joined by `&&` whose last one is the
/// runner's only invocation, its options leave the reporter to Aver, and
/// the lockfile installs a version with that reporter.
pub fn runner(script: &str, lock: &Lockfile) -> Option<Runner> {
    let segments = split_and(script)?;
    let (last, earlier) = segments.split_last()?;
    if earlier
        .iter()
        .any(|s| tokenize(s).is_none_or(|t| invocation(&t).is_some()))
    {
        return None;
    }
    let tokens = tokenize(last)?;
    let (runner, args, env) = invocation(&tokens)?;
    if runner == Runner::Ava {
        return None;
    }
    usable(runner, args, &env, lock, false).ok()?;
    Some(runner)
}

/// How the default check reads a `test` script per test: the runner, the
/// argv, and the report it writes. `npm test` with the reporter added when
/// [`runner`] reads the script; otherwise, when exactly one of its `&&`
/// commands runs a runner, the script itself through `npm exec --call`
/// (npm's `PATH` and environment, as `npm test` gives it, with `pretest`
/// and `posttest` around it), that command's own reporter options replaced
/// by Aver's. `Err` says why the check stays exit-code only.
pub fn read(
    id: &str,
    manifest: &Value,
    lock: &Lockfile,
) -> Result<(Runner, Vec<String>, Report), ExitOnly> {
    let script = manifest["scripts"]["test"].as_str().unwrap_or_default();
    if let Some(runner) = runner(script, lock) {
        let (argv, report) = runner.check(id);
        return Ok((runner, argv, report));
    }
    let segments = split_and(script).ok_or(ExitOnly::Shell)?;
    let commands = segments
        .iter()
        .map(|s| tokenize(s))
        .collect::<Option<Vec<_>>>()
        .ok_or(ExitOnly::Shell)?;
    let runs: Vec<usize> = (0..commands.len())
        .filter(|&i| invocation(&commands[i]).is_some())
        .collect();
    let at = match runs[..] {
        [] => {
            let mut programs: Vec<String> = Vec::new();
            for command in &commands {
                let name = command_start(command)
                    .and_then(|(i, _)| command.get(i))
                    .map(|t| program(t).to_owned());
                if let Some(name) = name.filter(|n| !programs.contains(n)) {
                    programs.push(name);
                }
            }
            return Err(ExitOnly::NoRunner(programs));
        }
        [one] => one,
        _ => return Err(ExitOnly::Several),
    };
    let tokens = &commands[at];
    let (runner, args, env) = invocation(tokens).expect("found above");
    usable(runner, args, &env, lock, true)?;
    if runner != Runner::NodeTest && args.iter().any(|a| a == "--") {
        return Err(ExitOnly::Configured(runner, "--".into()));
    }
    let (command, report) = instrument(runner, tokens, tokens.len() - args.len(), id);
    let original: Vec<&str> = segments[at].split_whitespace().collect();
    let mut parts = Vec::new();
    if manifest["scripts"]["pretest"].is_string() {
        parts.push("npm run pretest".to_owned());
    }
    for (i, segment) in segments.iter().enumerate() {
        parts.push(if i == at {
            let mut line = command
                .iter()
                .map(|w| {
                    // A word written bare keeps its globs for the shell.
                    if original.contains(&w.as_str()) && !w.contains(['\'', '"', '\\']) {
                        w.clone()
                    } else {
                        quote(w)
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            if runner == Runner::Ava {
                line.push_str(&ava_output(id));
            }
            line
        } else {
            segment.trim().to_owned()
        });
    }
    if manifest["scripts"]["posttest"].is_string() {
        parts.push("npm run posttest".to_owned());
    }
    let argv = ["npm", "exec", "--call"]
        .map(String::from)
        .into_iter()
        .chain([parts.join(" && ")])
        .collect();
    Ok((runner, argv, report))
}

/// Whether `runner`'s `args` leave its report to Aver: no watch mode or
/// subcommand without results, no option the reporter needs already set
/// (its own reporter options only when `replace` is false), and a version
/// with the reporter.
fn usable(
    runner: Runner,
    args: &[String],
    env: &[&str],
    lock: &Lockfile,
    replace: bool,
) -> Result<(), ExitOnly> {
    let given = |names: &[&str]| {
        args.iter()
            .find(|a| {
                names
                    .iter()
                    .any(|n| *a == n || a.strip_prefix(n).is_some_and(|rest| rest.starts_with('=')))
            })
            .map(|a| a.split('=').next().unwrap_or(a).to_owned())
    };
    let first_positional = args.iter().find(|a| !a.starts_with('-'));
    type Rules = (
        &'static [&'static str],
        &'static [&'static str],
        Option<(&'static str, (u64, u64))>,
    );
    let (modes, fixed, min): Rules = match runner {
        Runner::NodeTest => (&["--watch"], &[], None),
        Runner::Vitest => (
            &["--watch", "-w", "--ui"],
            &["--outputFile"],
            Some(("vitest", (0, 25))),
        ),
        Runner::Jest => (
            &["--watch", "--watchAll", "--listTests", "--showConfig"],
            &["--json", "--outputFile"],
            None,
        ),
        Runner::Mocha => (
            &["--watch", "-w", "--parallel", "-p"],
            &[],
            Some(("mocha", (6, 0))),
        ),
        Runner::Tap => (&["--watch", "-w"], &[], Some(("tap", (18, 0)))),
        Runner::Ava => (&["--watch", "-w"], &["--tap", "-t"], Some(("ava", (4, 0)))),
    };
    if let Some(mode) = given(modes) {
        return Err(ExitOnly::Mode(runner, mode));
    }
    let subcommand = match runner {
        Runner::Vitest => first_positional.filter(|p| {
            matches!(
                p.as_str(),
                "watch" | "dev" | "bench" | "init" | "list" | "typecheck"
            )
        }),
        Runner::Tap => first_positional.filter(|p| *p != "run"),
        Runner::Ava => first_positional.filter(|p| matches!(p.as_str(), "debug" | "reset-cache")),
        _ => None,
    };
    if let Some(sub) = subcommand {
        return Err(ExitOnly::Mode(runner, sub.clone()));
    }
    if let Some(option) = given(fixed).or_else(|| {
        (runner == Runner::Vitest)
            .then(|| {
                args.iter()
                    .find(|a| a.starts_with("--outputFile."))
                    .cloned()
            })
            .flatten()
    }) {
        return Err(ExitOnly::Configured(runner, option));
    }
    if runner == Runner::NodeTest && env.iter().any(|e| e.starts_with("NODE_OPTIONS=")) {
        return Err(ExitOnly::Configured(runner, "NODE_OPTIONS".into()));
    }
    if !replace {
        if let Some(option) = given(reporter_options(runner)) {
            return Err(ExitOnly::Configured(runner, option));
        }
    }
    match (runner, min) {
        (Runner::Jest, _) if !lock.has("jest", (0, 0)) && !lock.has("jest-cli", (0, 0)) => {
            Err(ExitOnly::NotInstalled(runner, (0, 0)))
        }
        // `vp test` runs the Vitest Vite+ bundles.
        (Runner::Vitest, _) if lock.has("vite-plus", (0, 0)) => Ok(()),
        (_, Some((name, min))) if !lock.has(name, min) => Err(ExitOnly::NotInstalled(runner, min)),
        _ => Ok(()),
    }
}

/// The options that choose `runner`'s reporters, each taking a value.
fn reporter_options(runner: Runner) -> &'static [&'static str] {
    match runner {
        Runner::NodeTest => &["--test-reporter", "--test-reporter-destination"],
        Runner::Vitest => &["--reporter"],
        Runner::Jest => &[],
        Runner::Mocha => &[
            "--reporter",
            "-R",
            "--reporter-option",
            "--reporter-options",
            "-O",
        ],
        Runner::Tap => &["--reporter", "-R", "--reporter-file"],
        Runner::Ava => &[],
    }
}

/// The command `tokens` (running `runner`, its arguments from `at`) with
/// its own reporter options removed and Aver's added, writing the report
/// of check `id`: right after the program for node:test, whose later
/// arguments are file patterns, last for the others.
fn instrument(runner: Runner, tokens: &[String], at: usize, id: &str) -> (Vec<String>, Report) {
    let (mut extra, report) = runner.check(id);
    extra.drain(..DEFAULT_CHECK_ARGV.len());
    let options = reporter_options(runner);
    let mut args = Vec::new();
    let mut rest = tokens[at..].iter();
    while let Some(arg) = rest.next() {
        if options.contains(&arg.as_str()) {
            rest.next();
        } else if !options.iter().any(|o| {
            arg.strip_prefix(o)
                .is_some_and(|value| value.starts_with('='))
        }) {
            args.push(arg.clone());
        }
    }
    let mut out = tokens[..at].to_vec();
    match runner {
        Runner::NodeTest => {
            let node = extra[0]
                .strip_prefix("--node-options=")
                .expect("node:test reporters ride in --node-options");
            out.extend(node.split(' ').map(str::to_owned));
            out.extend(args);
        }
        _ => {
            out.extend(args);
            out.extend(extra.into_iter().filter(|a| a != "--"));
        }
    }
    (out, report)
}

/// `word` as one shell word.
fn quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./=:,@%+-".contains(&b))
    {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// The JUnit file a Vitest `test` script already writes (`--reporter=junit`
/// with `--outputFile=<path>` or `--outputFile.junit=<path>`), when it is a
/// plain relative path inside the project. The default check then removes
/// any stale copy, runs `npm test` and collects that file, since a second
/// output file on the command line would conflict with the script's.
pub(crate) fn written_junit(script: &str) -> Option<String> {
    let segments = split_and(script)?;
    let tokens = tokenize(segments.last()?)?;
    let (runner, args, _) = invocation(&tokens)?;
    if runner != Runner::Vitest {
        return None;
    }
    let mut junit = false;
    let mut path = None;
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        let value = |name: &str| -> Option<String> {
            arg.strip_prefix(name)
                .and_then(|rest| rest.strip_prefix('='))
                .map(str::to_owned)
        };
        if arg == "--reporter" {
            junit |= iter.peek().is_some_and(|v| v.as_str() == "junit");
        } else if let Some(v) = value("--reporter") {
            junit |= v == "junit";
        } else if let Some(v) = value("--outputFile").or_else(|| value("--outputFile.junit")) {
            path = Some(v);
        } else if arg == "--outputFile" {
            path = iter.peek().map(|v| (*v).clone());
        }
    }
    let path = path?.trim_start_matches("./").to_owned();
    let plain = !path.is_empty()
        && !path.starts_with('/')
        && !path.starts_with('-')
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'))
        && !path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..");
    (junit && plain).then_some(path)
}

/// The runner a command's tokens invoke, the arguments after it, and the
/// environment assignments before it. Skips `NAME=value` assignments,
/// `cross-env`, `npx` and the coverage wrappers `c8` and `nyc` (with their
/// options, which must not take a separate value).
pub(crate) fn invocation(tokens: &[String]) -> Option<(Runner, &[String], Vec<&str>)> {
    let (i, env) = command_start(tokens)?;
    let runner = match program(tokens.get(i)?) {
        "vitest" => Runner::Vitest,
        "jest" => Runner::Jest,
        "mocha" | "_mocha" => Runner::Mocha,
        "tap" => Runner::Tap,
        "ava" => Runner::Ava,
        // Vite+ runs Vitest and passes its options through.
        "vp" if tokens.get(i + 1).is_some_and(|t| t == "test") => {
            return Some((Runner::Vitest, &tokens[i + 2..], env))
        }
        "node" | "tsx" => {
            let mut j = i + 1;
            let mut test = false;
            while let Some(option) = tokens.get(j).filter(|t| t.starts_with('-')) {
                test |= option == "--test";
                j += if takes_value(option) { 2 } else { 1 };
            }
            if test {
                return Some((Runner::NodeTest, &tokens[i + 1..], env));
            }
            let script = tokens.get(j)?;
            let runner = if script.ends_with("jest/bin/jest.js") || script.ends_with(".bin/jest") {
                Runner::Jest
            } else if ["mocha/bin/mocha", "mocha/bin/mocha.js", "mocha/bin/_mocha"]
                .iter()
                .any(|s| script.ends_with(s))
            {
                Runner::Mocha
            } else if script.ends_with("vitest/vitest.mjs") {
                Runner::Vitest
            } else {
                return None;
            };
            return Some((runner, &tokens[j + 1..], env));
        }
        _ => return None,
    };
    Some((runner, &tokens[i + 1..], env))
}

/// Where the program of a command's tokens is, past `NAME=value`
/// assignments, `cross-env`, `npx` and the coverage wrappers `c8` and
/// `nyc` (with their options, which must not take a separate value), and
/// the assignments.
fn command_start(tokens: &[String]) -> Option<(usize, Vec<&str>)> {
    let mut i = 0;
    let mut env = Vec::new();
    let assignment = |t: &str| {
        t.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty()
                && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    };
    fn skip_assignments<'t>(
        tokens: &'t [String],
        i: &mut usize,
        env: &mut Vec<&'t str>,
        assignment: impl Fn(&str) -> bool,
    ) {
        while tokens.get(*i).is_some_and(|t| assignment(t)) {
            env.push(&tokens[*i]);
            *i += 1;
        }
    }
    skip_assignments(tokens, &mut i, &mut env, assignment);
    if tokens.get(i).map(String::as_str) == Some("cross-env") {
        i += 1;
        skip_assignments(tokens, &mut i, &mut env, assignment);
    }
    if tokens.get(i).map(String::as_str) == Some("npx") {
        i += 1;
        while tokens.get(i).is_some_and(|t| t.starts_with('-')) {
            i += 1;
        }
    }
    if matches!(program(tokens.get(i)?), "c8" | "nyc") {
        i += 1;
        while tokens.get(i).is_some_and(|t| t.starts_with('-')) {
            i += 1;
        }
    }
    tokens.get(i)?;
    Some((i, env))
}

/// How `aver check` narrows a check to the tests that matter (§2.3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Narrow {
    /// Tests whose names match this JavaScript regular expression.
    Names(String),
    /// The tests in these files (repository paths).
    Files(Vec<String>),
}

/// `argv` narrowed per `by`, when Aver knows the runner it invokes: `npm
/// test` with a `test` script in `manifest` that [`runner`] reads, or a
/// runner the argv invokes itself. `None` when it cannot narrow it; the
/// check then runs whole.
pub fn narrow(
    argv: &[String],
    manifest: Option<&[u8]>,
    lockfile: Option<&[u8]>,
    by: &Narrow,
) -> Option<Vec<String>> {
    let mut out = argv.to_vec();
    if argv.len() >= DEFAULT_CHECK_ARGV.len()
        && argv.iter().zip(DEFAULT_CHECK_ARGV).all(|(a, b)| a == b)
    {
        let lock = Lockfile::parse(lockfile).with_manifest(manifest);
        let manifest: Value = serde_json::from_slice(manifest?).ok()?;
        let runner = runner(manifest["scripts"]["test"].as_str()?, &lock)?;
        // npm hands what follows `--` to the script; Node options go in
        // --node-options, which npm sets as NODE_OPTIONS, since options
        // after a test file pattern would be read as patterns.
        let after_dashes = |out: &mut Vec<String>, args: Vec<String>| {
            if !out.iter().any(|a| a == "--") {
                out.push("--".into());
            }
            out.extend(args);
        };
        match (runner, by) {
            (Runner::NodeTest, Narrow::Names(pattern)) => {
                let flag = format!("--test-name-pattern={pattern}");
                match out.iter_mut().find(|a| a.starts_with("--node-options=")) {
                    Some(options) => {
                        options.push(' ');
                        options.push_str(&flag);
                    }
                    None => out.push(format!("--node-options={flag}")),
                }
            }
            (_, Narrow::Names(pattern)) => after_dashes(&mut out, name_args(runner, pattern)?),
            (_, Narrow::Files(files)) => after_dashes(&mut out, files.clone()),
        }
        return Some(out);
    }
    let (runner, args, _) = invocation(argv)?;
    match (runner, by) {
        (Runner::NodeTest, Narrow::Names(pattern)) => {
            // Right after the program, before any test file pattern.
            out.insert(
                argv.len() - args.len(),
                format!("--test-name-pattern={pattern}"),
            );
        }
        (_, Narrow::Names(pattern)) => out.extend(name_args(runner, pattern)?),
        (_, Narrow::Files(files)) => out.extend(files.iter().cloned()),
    }
    Some(out)
}

/// The arguments that make `runner` run only the tests whose names match
/// `pattern` (not used for node:test, whose option goes elsewhere).
fn name_args(runner: Runner, pattern: &str) -> Option<Vec<String>> {
    match runner {
        Runner::Vitest | Runner::Jest => Some(vec!["-t".into(), pattern.into()]),
        Runner::Mocha => Some(vec!["--grep".into(), pattern.into()]),
        Runner::Tap | Runner::NodeTest => Some(vec![format!("--grep={pattern}")]),
        // Ava matches titles by glob, not by regular expression.
        Runner::Ava => None,
    }
}

/// The program name of a command token: its last path segment.
fn program(token: &str) -> &str {
    token.rsplit('/').next().unwrap_or(token)
}

/// Node options that take a separate value when written without `=`.
fn takes_value(option: &str) -> bool {
    !option.contains('=')
        && matches!(
            option,
            "-r" | "--require"
                | "--import"
                | "--loader"
                | "--experimental-loader"
                | "-C"
                | "--conditions"
                | "--env-file"
                | "--input-type"
                | "--test-name-pattern"
                | "--test-skip-pattern"
                | "--test-concurrency"
                | "--test-timeout"
                | "--test-shard"
                | "--test-reporter"
                | "--test-reporter-destination"
                | "--title"
                | "--stack-size"
        )
}

/// Splits a script on top-level `&&`. `None` for any other shell control
/// syntax: `||`, `|`, `;`, `&`, redirections, substitutions, variables,
/// subshells, newlines, or an unterminated quote.
pub(crate) fn split_and(script: &str) -> Option<Vec<String>> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = script.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    current.push(c);
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                    continue;
                } else if q == '"' && matches!(c, '$' | '`') {
                    return None;
                }
                current.push(c);
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    current.push(c);
                }
                '&' if chars.peek() == Some(&'&') => {
                    chars.next();
                    segments.push(std::mem::take(&mut current));
                }
                '&' | '|' | ';' | '<' | '>' | '$' | '`' | '(' | ')' | '\n' | '\r' => return None,
                '\\' => {
                    current.push(c);
                    current.push(chars.next()?);
                }
                c => current.push(c),
            },
        }
    }
    if quote.is_some() {
        return None;
    }
    segments.push(current);
    segments
        .iter()
        .all(|s| !s.trim().is_empty())
        .then_some(segments)
}

/// Splits one command into words with POSIX quoting (no expansions; the
/// caller has already refused `$` and backticks).
pub(crate) fn tokenize(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => {
                            let next = chars.next()?;
                            if !matches!(next, '"' | '\\') {
                                word.push('\\');
                            }
                            word.push(next);
                        }
                        c => word.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                word.push(chars.next()?);
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    (!words.is_empty()).then_some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(packages: &[(&str, &str)]) -> Lockfile {
        let mut map = serde_json::Map::new();
        for (name, version) in packages {
            map.insert(
                format!("node_modules/{name}"),
                serde_json::json!({ "version": version }),
            );
        }
        Lockfile {
            lock: Some(serde_json::json!({ "lockfileVersion": 3, "packages": map })),
            manifest: None,
            root: None,
        }
    }

    fn all() -> Lockfile {
        lock(&[
            ("vitest", "3.2.4"),
            ("jest", "29.7.0"),
            ("mocha", "10.2.0"),
            ("tap", "21.0.1"),
        ])
    }

    fn words(s: &str) -> Vec<String> {
        // `~` stands for a space inside one argument.
        s.split(' ').map(|w| w.replace('~', " ")).collect()
    }

    #[test]
    fn checks_narrow_to_named_tests_or_files_for_known_runners() {
        let pkg = |script: &str| {
            serde_json::to_vec(&serde_json::json!({ "scripts": { "test": script } })).unwrap()
        };
        let lockfile = serde_json::to_vec(&all().lock).unwrap();
        let names = Narrow::Names("[[]crit:(neg)]".into());
        let files = Narrow::Files(vec!["test/neg.test.js".into()]);
        let cases: &[(&str, &str, &Narrow, Option<&str>)] = &[
            // The default check of a node:test project: its reporters ride
            // in --node-options, and so does the name pattern.
            (
                "npm test --node-options=--test-reporter=junit",
                "node --test",
                &names,
                Some("npm test --node-options=--test-reporter=junit~--test-name-pattern=[[]crit:(neg)]"),
            ),
            (
                "npm test",
                "node --test",
                &names,
                Some("npm test --node-options=--test-name-pattern=[[]crit:(neg)]"),
            ),
            ("npm test", "node --test", &files, Some("npm test -- test/neg.test.js")),
            (
                "npm test -- --reporter=junit",
                "vitest run",
                &names,
                Some("npm test -- --reporter=junit -t [[]crit:(neg)]"),
            ),
            ("npm test", "jest", &names, Some("npm test -- -t [[]crit:(neg)]")),
            ("npm test", "mocha", &names, Some("npm test -- --grep [[]crit:(neg)]")),
            // An aver.toml check that runs a runner itself.
            (
                "node --test --test-reporter=junit",
                "",
                &names,
                Some("node --test-name-pattern=[[]crit:(neg)] --test --test-reporter=junit"),
            ),
            (
                "node --test --test-reporter=junit",
                "",
                &files,
                Some("node --test --test-reporter=junit test/neg.test.js"),
            ),
            ("npx jest --json", "", &names, Some("npx jest --json -t [[]crit:(neg)]")),
            // Runners Aver does not know are run whole.
            ("npm test", "./run-tests.sh", &names, None),
            ("make test", "", &names, None),
        ];
        for (argv, script, narrow_by, want) in cases {
            let got = narrow(&words(argv), Some(&pkg(script)), Some(&lockfile), narrow_by);
            assert_eq!(got, want.map(words), "{argv} / {script}");
        }
    }

    #[test]
    fn single_runner_invocations_are_detected() {
        let cases: &[(&str, Option<Runner>)] = &[
            ("node --test", Some(Runner::NodeTest)),
            ("node --test test/", Some(Runner::NodeTest)),
            ("node --import tsx --test 'src/**/*.test.ts'", Some(Runner::NodeTest)),
            ("tsx --test", Some(Runner::NodeTest)),
            ("vitest", Some(Runner::Vitest)),
            ("vitest run", Some(Runner::Vitest)),
            ("vitest --coverage", Some(Runner::Vitest)),
            ("c8 vitest run", Some(Runner::Vitest)),
            ("jest", Some(Runner::Jest)),
            ("jest -c=jest.config.ts", Some(Runner::Jest)),
            ("jest --forceExit --detectOpenHandles", Some(Runner::Jest)),
            ("NODE_OPTIONS=--experimental-vm-modules npx jest", Some(Runner::Jest)),
            ("node --experimental-vm-modules node_modules/jest/bin/jest.js", Some(Runner::Jest)),
            ("mocha test/**", Some(Runner::Mocha)),
            ("nyc mocha --config ./.mocharc-unit.json", Some(Runner::Mocha)),
            (
                "cross-env BABEL_ENV=test mocha --require @babel/register --recursive \"test/unit/\"",
                Some(Runner::Mocha),
            ),
            ("npm run lint && mocha", Some(Runner::Mocha)),
            ("tsc --noEmit && node --test", Some(Runner::NodeTest)),
            ("tap run --allow-empty-coverage --disable-coverage --timeout=60000", Some(Runner::Tap)),
            ("tap", Some(Runner::Tap)),
        ];
        for (script, want) in cases {
            assert_eq!(runner(script, &all()), *want, "{script}");
        }
    }

    #[test]
    fn anything_ambiguous_keeps_the_command_level_check() {
        for script in [
            "vitest --run --reporter=default --reporter=junit --outputFile=./junit.xml",
            "vitest watch",
            "jest --watch",
            "jest --json",
            "mocha --reporter dot",
            "mocha --parallel",
            "node --test --test-reporter=tap",
            "NODE_OPTIONS=--foo node --test",
            "node --experimental-vm-modules run-tests.js",
            "NODE_ENV=test node src/index-test.js",
            "node test/a.test.js && node test/b.test.js",
            "jest && vitest",
            "jest || true",
            "jest; echo done",
            "jest | tee out",
            "jest > out.txt",
            "jest $ARGS",
            "echo \"Error: no test specified\" && exit 1",
            "react-scripts test",
            "turbo run test",
            "npm run test:unit",
            "tap report",
            "c8 -r lcov vitest",
            "jest 'unterminated",
            "jest &&",
        ] {
            assert_eq!(runner(script, &all()), None, "{script}");
        }
    }

    #[test]
    fn the_lockfile_must_install_a_runner_with_the_reporter() {
        assert_eq!(runner("vitest", &lock(&[])), None);
        assert_eq!(runner("vitest", &lock(&[("vitest", "0.20.0")])), None);
        assert_eq!(runner("mocha", &lock(&[("mocha", "5.2.0")])), None);
        assert_eq!(runner("tap", &lock(&[("tap", "16.3.0")])), None);
        assert_eq!(
            runner("jest", &lock(&[("jest-cli", "24.9.0")])),
            Some(Runner::Jest)
        );
        assert_eq!(runner("node --test", &lock(&[])), Some(Runner::NodeTest));
        let v1 = Lockfile {
            lock: Some(serde_json::json!({
                "lockfileVersion": 1,
                "dependencies": { "mocha": { "version": "8.4.0" } }
            })),
            manifest: None,
            root: None,
        };
        assert_eq!(runner("mocha", &v1), Some(Runner::Mocha));
    }

    fn read_script(
        script: &str,
        extra: serde_json::Value,
        lock: &Lockfile,
    ) -> Result<(Runner, Vec<String>, Report), ExitOnly> {
        let mut manifest = serde_json::json!({ "scripts": { "test": script } });
        if let (Some(m), Some(extra)) = (manifest.as_object_mut(), extra.as_object()) {
            for (k, v) in extra {
                m.insert(k.clone(), v.clone());
            }
        }
        read("test", &manifest, lock)
    }

    #[test]
    fn real_test_scripts_are_read_per_test() {
        let call = |script: &str| ["npm", "exec", "--call", script].map(String::from).to_vec();
        let mocha = lock(&[("mocha", "10.2.0")]);
        let none = lock(&[]);
        let cases: &[(&str, serde_json::Value, &Lockfile, Runner, Vec<String>)] = &[
            // commander: node:test first, a type check after it.
            (
                "node --test && npm run check:type:ts",
                serde_json::json!({}),
                &none,
                Runner::NodeTest,
                call("node --test-reporter=spec --test-reporter-destination=stdout --test-reporter=junit --test-reporter-destination=/aver/out/test.xml --test && npm run check:type:ts"),
            ),
            // express: mocha with its own reporter, replaced.
            (
                "mocha --require test/support/env --reporter spec --check-leaks test/ test/acceptance/",
                serde_json::json!({}),
                &mocha,
                Runner::Mocha,
                call("mocha --require test/support/env --check-leaks test/ test/acceptance/ --reporter xunit --reporter-option output=/aver/out/test.xml"),
            ),
            // A runner before a lint step, globs left to the shell, quoted
            // words kept whole, pretest and posttest kept.
            (
                "mocha -R dot test/**/*.js 'a b' && eslint .",
                serde_json::json!({ "scripts": { "test": "mocha -R dot test/**/*.js 'a b' && eslint .", "pretest": "x", "posttest": "y" } }),
                &mocha,
                Runner::Mocha,
                call("npm run pretest && mocha test/**/*.js 'a b' --reporter xunit --reporter-option output=/aver/out/test.xml && eslint . && npm run posttest"),
            ),
        ];
        for (script, extra, lock, runner, argv) in cases {
            let (got, got_argv, _) = read_script(script, extra.clone(), lock).unwrap();
            assert_eq!((got, &got_argv), (*runner, argv), "{script}");
            let check = CheckDef {
                argv: got_argv,
                ..crate::presets::default_check(None, None, &|_| false)
            };
            assert_eq!(runner_of(&check), Some(*runner), "{script}");
        }
        // swr: jest under pnpm, its version from package.json.
        let swr = read_script(
            "jest",
            serde_json::json!({ "devDependencies": { "jest": "^29.7.0" } }),
            &Lockfile::parse(None)
                .with_manifest(Some(br#"{"devDependencies":{"jest":"^29.7.0"}}"#)),
        )
        .unwrap();
        assert_eq!(swr.0, Runner::Jest);
        assert_eq!(swr.1[..2], ["npm", "test"]);
    }

    #[test]
    fn ava_and_vite_plus_scripts_are_read_per_test() {
        let call = |script: &str| ["npm", "exec", "--call", script].map(String::from).to_vec();
        let ava =
            Lockfile::parse(None).with_manifest(Some(br#"{"devDependencies":{"ava":"^8.0.1"}}"#));
        let vp = Lockfile::parse(None)
            .with_manifest(Some(br#"{"devDependencies":{"vite-plus":"1.0.0"}}"#));
        let cases: &[(&str, &Lockfile, &str, Vec<String>, ReportFormat)] = &[
            // chalk: ava's TAP goes to the report file, the lint step before
            // it and the type check after it still run under `&&`.
            (
                "xo && c8 ava && tsc --noEmit --types node source/index.d.ts",
                &ava,
                "ava",
                call("xo && c8 ava --tap >/aver/out/test.tap && tsc --noEmit --types node source/index.d.ts"),
                ReportFormat::Tap,
            ),
            // ky: ava last, still through the script, since `npm test`
            // would mix npm's banner into the TAP stream.
            (
                "xo && npm run build && ava --timeout=10m --serial",
                &ava,
                "ava",
                call("xo && npm run build && ava --timeout=10m --serial --tap >/aver/out/test.tap"),
                ReportFormat::Tap,
            ),
            // hono: `vp test` is Vitest underneath and takes its reporters.
            (
                "tsc -p tsconfig.spec.json && vp test --run",
                &vp,
                "vitest",
                words("npm test -- --reporter=default --reporter=junit --outputFile.junit=/aver/out/test.xml"),
                ReportFormat::Junit,
            ),
        ];
        for (script, lock, name, argv, format) in cases {
            let (got, got_argv, report) = read_script(script, serde_json::json!({}), lock).unwrap();
            assert_eq!(
                (got.name(), &got_argv, report.format),
                (*name, argv, *format),
                "{script}"
            );
            let check = CheckDef {
                argv: got_argv,
                ..crate::presets::default_check(None, None, &|_| false)
            };
            assert_eq!(runner_of(&check).map(Runner::name), Some(*name), "{script}");
        }
        assert_eq!(
            runner("vp test --run", &vp).map(Runner::name),
            Some("vitest")
        );
        assert_eq!(runner("ava", &ava), None, "ava's TAP needs its own file");
    }

    #[test]
    fn an_exit_only_script_says_why() {
        let manifest = |deps: serde_json::Value| {
            serde_json::to_vec(&serde_json::json!({ "devDependencies": deps })).unwrap()
        };
        let cases: &[(&str, serde_json::Value, ExitOnly)] = &[
            (
                "xo && tsc --noEmit",
                serde_json::json!({}),
                ExitOnly::NoRunner(vec!["xo".into(), "tsc".into()]),
            ),
            (
                "tsc && vp build",
                serde_json::json!({ "vite-plus": "1.0.0" }),
                ExitOnly::NoRunner(vec!["tsc".into(), "vp".into()]),
            ),
            (
                "xo && ava --watch",
                serde_json::json!({ "ava": "^8.0.1" }),
                ExitOnly::Mode(Runner::Ava, "--watch".into()),
            ),
            (
                "ava --tap | tap-nyan",
                serde_json::json!({ "ava": "^8.0.1" }),
                ExitOnly::Shell,
            ),
            (
                "xo && ava --tap",
                serde_json::json!({ "ava": "^8.0.1" }),
                ExitOnly::Configured(Runner::Ava, "--tap".into()),
            ),
            (
                "ava",
                serde_json::json!({ "ava": "^3.15.0" }),
                ExitOnly::NotInstalled(Runner::Ava, (4, 0)),
            ),
            ("jest || true", serde_json::json!({}), ExitOnly::Shell),
            ("jest && vitest", serde_json::json!({}), ExitOnly::Several),
            (
                "vitest --watch",
                serde_json::json!({ "vitest": "3.0.0" }),
                ExitOnly::Mode(Runner::Vitest, "--watch".into()),
            ),
            (
                "jest --json --outputFile=out.json",
                serde_json::json!({ "jest": "29" }),
                ExitOnly::Configured(Runner::Jest, "--json".into()),
            ),
            (
                "lint && mocha",
                serde_json::json!({ "mocha": "^5.2.0" }),
                ExitOnly::NotInstalled(Runner::Mocha, (6, 0)),
            ),
            (
                "mocha",
                serde_json::json!({ "mocha": "workspace:*" }),
                ExitOnly::NotInstalled(Runner::Mocha, (6, 0)),
            ),
        ];
        for (script, deps, want) in cases {
            let bytes = manifest(deps.clone());
            let lock = Lockfile::parse(None).with_manifest(Some(&bytes));
            assert_eq!(
                read_script(script, serde_json::json!({}), &lock)
                    .err()
                    .as_ref(),
                Some(want),
                "{script}"
            );
        }
        assert!(ExitOnly::NoRunner(vec!["ava".into()])
            .to_string()
            .contains("`ava`"));
    }

    #[test]
    fn package_json_ranges_give_their_least_version() {
        for (range, want) in [
            ("^29.7.0", Some((29, 7))),
            ("~1.2", Some((1, 2))),
            (">=6", Some((6, 0))),
            ("10.x", Some((10, 0))),
            ("3.2.4", Some((3, 2))),
            ("latest", None),
            ("workspace:*", None),
            ("npm:vitest@3", None),
            (">=1 <3", None),
            ("^1 || ^2", None),
            ("*", None),
        ] {
            assert_eq!(lower_bound(range), want, "{range}");
        }
    }

    #[test]
    fn scripts_split_on_and_and_respect_quotes() {
        assert_eq!(split_and("a 'b && c' && d").unwrap(), ["a 'b && c' ", " d"]);
        assert_eq!(
            tokenize(r#"mocha --require "a b" 'c d' e\ f"#).unwrap(),
            ["mocha", "--require", "a b", "c d", "e f"]
        );
        assert!(split_and("echo \"$HOME\"").is_none());
    }
}
