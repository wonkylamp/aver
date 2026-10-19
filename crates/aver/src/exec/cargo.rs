//! A Cargo check run again over the test targets that build (ARCHITECTURE.md
//! §1.9.3, "restored test files that do not build").
//!
//! The judged head puts back the base's copy of every test file the head
//! modified or deleted. When one of them uses what the head removed, it does
//! not compile, and `cargo test` stops before running any test: the check
//! loses its report. `cargo test` has no `--keep-going`, and
//! `--no-fail-fast` does not keep it compiling, so the judged head is built
//! first with `cargo build --tests --keep-going` ([`CargoTest::probe`]),
//! whose JSON messages say which test targets built and which did not, with
//! their errors ([`Probe`]). Then `cargo test` runs once per package over
//! the targets that built ([`CargoTest::over`]).
//!
//! Only a check whose command is `sh -c` with a script that runs `cargo
//! test` once, as a plain command (environment assignments before it, words
//! without quotes, `$` or backticks, and no target selection beyond
//! `--lib`, `--bins`, `--tests`, `--test`, `--bin` and `--doc`), is
//! rewritten. Every other check keeps one command-level entry.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use aver_core::CheckDef;

/// Where the probe writes cargo's JSON messages: a file of its own, read
/// whole as the run's report, so no output budget cuts it.
pub const PROBE_REPORT: &str = "/aver/out/aver-probe.jsonl";

/// Cargo flags whose value is the next word unless written `--flag=value`.
const VALUED: &[&str] = &[
    "-F",
    "--features",
    "--target",
    "--target-dir",
    "--manifest-path",
    "--profile",
    "-j",
    "--jobs",
    "--color",
    "--config",
    "-Z",
    "--lockfile-path",
];

/// One `cargo test` invocation in a check's `sh -c` script.
#[derive(Debug, Clone)]
pub struct CargoTest {
    def: CheckDef,
    /// The bytes of the script from the first environment assignment
    /// before `cargo test` to its last argument.
    span: Range<usize>,
    /// The environment assignments before `cargo`, each followed by a space.
    prefix: String,
    /// Package selection: `--workspace`, `-p`, `--exclude` and their values.
    packages: Vec<String>,
    /// Every other argument before `--`, in order, without `--no-fail-fast`.
    common: Vec<Arg>,
    select: Select,
    /// The test binaries' arguments, after `--`.
    trailing: Vec<String>,
    no_fail_fast: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Arg {
    /// A flag, with its value when it takes one.
    Flag(Vec<String>),
    /// A test name filter, which `cargo build` does not take.
    Filter(String),
}

/// Which targets the check's own command tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Select {
    /// No target flag: cargo's default, the test targets and doc-tests.
    default: bool,
    lib: bool,
    bins: bool,
    tests: bool,
    doc: bool,
    test: BTreeSet<String>,
    bin: BTreeSet<String>,
    /// `--examples`: the examples are built, never run, so a change that
    /// breaks one fails the check.
    examples: bool,
}

impl Select {
    fn runs(&self, t: &Target) -> bool {
        match t.kind {
            Kind::Lib => self.default || self.lib || self.tests,
            Kind::Bin => self.default || self.bins || self.tests || self.bin.contains(&t.name),
            Kind::Test => self.default || self.tests || self.test.contains(&t.name),
            Kind::Other => false,
        }
    }

    fn doc(&self) -> bool {
        self.default || self.doc
    }
}

impl CargoTest {
    /// The `cargo test` invocation of `def`'s command, when Aver can run it
    /// again over chosen targets.
    pub fn of(def: &CheckDef) -> Option<Self> {
        Self::parse(def)
    }

    fn parse(def: &CheckDef) -> Option<Self> {
        let [shell, flag, script] = def.argv.as_slice() else {
            return None;
        };
        if !matches!(shell.as_str(), "sh" | "bash" | "/bin/sh" | "/bin/bash") || flag != "-c" {
            return None;
        }
        let (span, prefix, args) = invocation(script)?;
        let mut parsed = Self {
            def: def.clone(),
            span,
            prefix,
            packages: Vec::new(),
            common: Vec::new(),
            select: Select::default(),
            trailing: Vec::new(),
            no_fail_fast: false,
        };
        let mut words = args.into_iter();
        let mut targeted = false;
        while let Some(w) = words.next() {
            let value = |words: &mut std::vec::IntoIter<String>| words.next();
            match w.as_str() {
                "--" => {
                    parsed.trailing = words.by_ref().collect();
                }
                "--workspace" | "--all" => parsed.packages.push(w),
                "-p" | "--package" | "--exclude" => {
                    let v = value(&mut words)?;
                    parsed.packages.extend([w, v]);
                }
                "--lib" => (parsed.select.lib, targeted) = (true, true),
                "--bins" => (parsed.select.bins, targeted) = (true, true),
                "--tests" => (parsed.select.tests, targeted) = (true, true),
                "--doc" => (parsed.select.doc, targeted) = (true, true),
                "--test" => {
                    parsed.select.test.insert(value(&mut words)?);
                    targeted = true;
                }
                "--bin" => {
                    parsed.select.bin.insert(value(&mut words)?);
                    targeted = true;
                }
                "--no-fail-fast" => parsed.no_fail_fast = true,
                "--examples" => parsed.select.examples = true,
                "--no-run" | "--all-targets" | "--example" | "--benches" | "--bench"
                | "--message-format" => return None,
                _ if w.starts_with("--package=") || w.starts_with("--exclude=") => {
                    parsed.packages.push(w)
                }
                _ if w.starts_with("-p") && w.len() > 2 && !w.starts_with("--") => {
                    parsed.packages.push(w)
                }
                _ if w.starts_with("--test=") => {
                    parsed.select.test.insert(w["--test=".len()..].to_owned());
                    targeted = true;
                }
                _ if w.starts_with("--bin=") => {
                    parsed.select.bin.insert(w["--bin=".len()..].to_owned());
                    targeted = true;
                }
                _ if ["--example=", "--bench=", "--message-format="]
                    .iter()
                    .any(|p| w.starts_with(p)) =>
                {
                    return None
                }
                _ if VALUED.contains(&w.as_str()) => {
                    let v = value(&mut words)?;
                    parsed.common.push(Arg::Flag(vec![w, v]));
                }
                _ if w.starts_with('-') => parsed.common.push(Arg::Flag(vec![w])),
                _ => parsed.common.push(Arg::Filter(w)),
            }
        }
        parsed.select.default = !targeted;
        Some(parsed)
    }

    /// The check with its `cargo test` replaced by `command`.
    fn with(&self, command: &str) -> CheckDef {
        let mut def = self.def.clone();
        let script = &def.argv[2];
        def.argv[2] = format!(
            "{}{command}{}",
            &script[..self.span.start],
            &script[self.span.end..]
        );
        def
    }

    fn flags(&self) -> Vec<String> {
        self.common
            .iter()
            .flat_map(|a| match a {
                Arg::Flag(words) => words.clone(),
                Arg::Filter(_) => Vec::new(),
            })
            .collect()
    }

    /// The check building every test target it can, and the examples when
    /// it builds them. Its report is cargo's JSON messages about test
    /// targets and errors, written to
    /// [`PROBE_REPORT`]; the subshell keeps the rest of the script's
    /// redirections from taking cargo's standard output.
    pub fn probe(&self) -> CheckDef {
        let mut words = vec!["cargo".to_owned(), "build".to_owned()];
        words.extend(self.packages.iter().cloned());
        words.extend(self.flags());
        if self.select.examples {
            words.push("--examples".to_owned());
        }
        words.extend(
            [
                "--tests",
                "--keep-going",
                "--message-format=json-diagnostic-short",
            ]
            .map(String::from),
        );
        let command = format!("({}{} >\"{PROBE_REPORT}\")", self.prefix, words.join(" "));
        let mut def = self.with(&command);
        def.report = Some(aver_core::Report {
            format: aver_core::ReportFormat::CargoProbe,
            path: PROBE_REPORT
                .parse()
                .expect("the probe's report path is valid"),
        });
        def
    }

    /// The check running `cargo test` once per package over the targets
    /// `probe` built that the check's own command tests (with its examples
    /// when it builds them), and the doc-tests of each package with a
    /// library when it runs them; `None` when there is nothing to run.
    pub fn over(&self, probe: &Probe) -> Option<CheckDef> {
        let common: Vec<String> = self
            .common
            .iter()
            .flat_map(|a| match a {
                Arg::Flag(words) => words.clone(),
                Arg::Filter(f) => vec![f.clone()],
            })
            .collect();
        let trailing = if self.trailing.is_empty() {
            String::new()
        } else {
            format!(" -- {}", self.trailing.join(" "))
        };
        let run = |package: &str, targets: &[String]| {
            let mut words = vec!["cargo".to_owned(), "test".to_owned()];
            words.extend(common.iter().cloned());
            words.extend(["-p".to_owned(), package.to_owned()]);
            words.extend(targets.iter().cloned());
            words.push("--no-fail-fast".to_owned());
            format!("{}{}{trailing} || s=1", self.prefix, words.join(" "))
        };
        let mut commands = Vec::new();
        for (package, targets) in &probe.built {
            let mut flags: Vec<String> = targets
                .iter()
                .filter(|t| self.select.runs(t))
                .flat_map(Target::flag)
                .collect();
            if !flags.is_empty() {
                if self.select.examples {
                    flags.push("--examples".to_owned());
                }
                commands.push(run(package, &flags));
            }
        }
        if self.select.doc() {
            for package in &probe.libraries {
                commands.push(run(package, &["--doc".to_owned()]));
            }
        }
        (!commands.is_empty())
            .then(|| self.with(&format!("( s=0; {}; exit $s )", commands.join("; "))))
    }
}

/// What may follow `cargo test`'s arguments in a script whose exit code and
/// report are cargo's own: nothing, or its output to a file that is then
/// printed and its status returned (the shape `env` gives a stdout report).
static RETURNS_CARGOS_EXIT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"^\s*(?:2>&1\s*)?(?:\)?\s*>\s*(\S+)(?:\s+2>&1)?;\s*s=\$\?;\s*cat\s+(\S+);\s*exit\s+\$s\s*)?$",
    )
    .expect("a valid pattern")
});

impl CargoTest {
    /// The check with its `cargo test` running only the integration test
    /// targets `tests` and, with `units`, the libraries' and binaries' own
    /// tests, over the same packages, flags, filters and test arguments:
    /// what the full command would run of those targets, and nothing else.
    /// `None` unless the full command runs each of them without stopping at
    /// a failing binary (`--no-fail-fast`), `units` would leave out no
    /// doc-test the full command runs, and the script's exit code and
    /// report are cargo's.
    pub fn narrowed(def: &CheckDef, tests: &BTreeSet<String>, units: bool) -> Option<CheckDef> {
        let cargo = Self::parse(def)?;
        let tail = &def.argv[2][cargo.span.end..];
        let returns = RETURNS_CARGOS_EXIT.captures(tail)?;
        if returns.get(1).map(|m| m.as_str()) != returns.get(2).map(|m| m.as_str())
            || (units && cargo.select.doc())
        {
            return None;
        }
        Self::only(def, tests, units)
    }

    /// The check with its `cargo test` running only the integration test
    /// targets `tests` and, with `units`, the libraries' and binaries' own
    /// tests, leaving out every doc-test: a focused rerun's selection,
    /// whose other tests keep the first run's results.
    pub fn only(def: &CheckDef, tests: &BTreeSet<String>, units: bool) -> Option<CheckDef> {
        let cargo = Self::parse(def)?;
        let select = &cargo.select;
        if !cargo.no_fail_fast
            || (tests.is_empty() && !units)
            || !tests
                .iter()
                .all(|t| select.default || select.tests || select.test.contains(t))
        {
            return None;
        }
        let lib = select.default || select.lib || select.tests;
        let bins = select.default || select.bins || select.tests;
        if units && (!select.bin.is_empty() || !(lib || bins)) {
            return None;
        }
        let mut words = vec!["cargo".to_owned(), "test".to_owned()];
        words.extend(cargo.packages.iter().cloned());
        words.extend(cargo.common.iter().flat_map(|a| match a {
            Arg::Flag(words) => words.clone(),
            Arg::Filter(f) => vec![f.clone()],
        }));
        if units && lib {
            words.push("--lib".to_owned());
        }
        if units && bins {
            words.push("--bins".to_owned());
        }
        for t in tests {
            words.extend(["--test".to_owned(), t.clone()]);
        }
        words.push("--no-fail-fast".to_owned());
        if !cargo.trailing.is_empty() {
            words.push("--".to_owned());
            words.extend(cargo.trailing.iter().cloned());
        }
        Some(cargo.with(&format!("{}{}", cargo.prefix, words.join(" "))))
    }
}

impl CargoTest {
    /// The check with its `cargo test` running, in every test binary it
    /// runs, only the tests named exactly `names` (`-- --exact <names>`).
    /// `None` when the command filters tests by name already or a name is
    /// not a plain Rust path.
    pub fn exact(def: &CheckDef, names: &BTreeSet<String>) -> Option<CheckDef> {
        let cargo = Self::parse(def)?;
        let path = |n: &String| {
            !n.is_empty()
                && n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
        };
        if names.is_empty()
            || !names.iter().all(path)
            || cargo.common.iter().any(|a| matches!(a, Arg::Filter(_)))
            || cargo
                .trailing
                .iter()
                .any(|w| !w.starts_with('-') || matches!(w.as_str(), "--exact" | "--skip"))
        {
            return None;
        }
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let command = format!(
            "{}{} --exact {}",
            &def.argv[2][cargo.span.clone()],
            if cargo.trailing.is_empty() { " --" } else { "" },
            names.join(" ")
        );
        Some(cargo.with(&command))
    }
}

/// Where `cargo test` is in `script`: the span from its first environment
/// assignment to its last argument, the assignments, and the arguments.
/// `None` unless the script runs it exactly once, at the start of a
/// command, with plain words.
fn invocation(script: &str) -> Option<(Range<usize>, String, Vec<String>)> {
    const CALL: &str = "cargo test";
    let boundary = |c: Option<char>| c.is_none_or(|c| c.is_whitespace() || ";&|(){}".contains(c));
    let hits: Vec<usize> = script
        .match_indices(CALL)
        .map(|(at, _)| at)
        .filter(|&at| {
            boundary(script[..at].chars().next_back())
                && boundary(script[at + CALL.len()..].chars().next())
        })
        .collect();
    let [at] = hits.as_slice() else {
        return None;
    };
    let at = *at;
    let mut start = at;
    let mut assigns = Vec::new();
    loop {
        let before = script[..start].trim_end();
        let word_at = before
            .rfind(|c: char| c.is_whitespace() || ";&|(){}".contains(c))
            .map_or(0, |i| i + 1);
        let word = &before[word_at..];
        if is_assignment(word) {
            assigns.push(word.to_owned());
            start = word_at;
            continue;
        }
        let at_command_start = before.is_empty()
            || before.ends_with(|c: char| ";&|({\n".contains(c))
            || matches!(word, "then" | "do" | "else");
        if !at_command_start {
            return None;
        }
        break;
    }
    assigns.reverse();
    let rest = &script[at + CALL.len()..];
    let stop = rest
        .find(|c: char| "><;&|()\n#".contains(c))
        .unwrap_or(rest.len());
    let mut words: Vec<&str> = rest[..stop].split_whitespace().collect();
    // `2>&1` right after the arguments: the `2` is a descriptor.
    if rest[stop..].starts_with(['>', '<'])
        && !rest[..stop].ends_with(char::is_whitespace)
        && words
            .last()
            .is_some_and(|w| w.bytes().all(|b| b.is_ascii_digit()))
    {
        words.pop();
    }
    if words.iter().any(|w| !plain(w)) {
        return None;
    }
    let end = match words.last() {
        Some(last) => {
            let from = at + CALL.len();
            let mut offset = 0;
            for w in &words {
                offset += script[from + offset..].find(w)? + w.len();
            }
            debug_assert!(script[..from + offset].ends_with(last));
            from + offset
        }
        None => at + CALL.len(),
    };
    let prefix: String = assigns.iter().map(|a| format!("{a} ")).collect();
    Some((
        start..end,
        prefix,
        words.into_iter().map(String::from).collect(),
    ))
}

fn is_assignment(word: &str) -> bool {
    let Some((name, value)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && plain(value)
}

/// A word the shell reads as itself.
fn plain(word: &str) -> bool {
    word.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_=.,:/+@%^".contains(c))
}

/// A target of a package, as cargo's JSON messages describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    kind: Kind,
    name: String,
    /// The root source file, relative to the package's directory.
    src: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Lib,
    Bin,
    Test,
    Other,
}

impl Target {
    /// The binary and target libtest names this target's tests with
    /// (`api tests/api.rs`, `calc src/lib.rs`).
    pub fn label(&self) -> String {
        format!("{} {}", self.name.replace('-', "_"), self.src)
    }

    fn flag(&self) -> Vec<String> {
        match self.kind {
            Kind::Lib => vec!["--lib".to_owned()],
            Kind::Bin => vec!["--bin".to_owned(), self.name.clone()],
            Kind::Test => vec!["--test".to_owned(), self.name.clone()],
            Kind::Other => Vec::new(),
        }
    }
}

/// What the probe's report says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    /// The test targets that built, by package name.
    pub built: BTreeMap<String, Vec<Target>>,
    /// The packages with a library that has doc-tests.
    pub libraries: BTreeSet<String>,
    /// The targets that did not build, with their errors.
    pub failed: Vec<Failed>,
    /// Whether the build succeeded, from cargo's closing `build-finished`
    /// message; `None` when the stream stops before it, so the probe is
    /// incomplete.
    pub success: Option<bool>,
}

/// A test target that did not build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub target: Target,
    /// Each error's primary file as rustc names it (relative to the Cargo
    /// workspace's root), and its one-line message; `None` for an error
    /// that names no file.
    pub errors: Vec<(Option<String>, String)>,
}

impl Probe {
    /// Reads the probe's report: cargo's JSON messages, one per line.
    pub fn read(output: &str) -> Self {
        let mut probe = Self::default();
        for line in output.lines().map(str::trim) {
            if !line.starts_with('{') {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            match s(&v, "reason") {
                Some("build-finished") => {
                    probe.success = v.get("success").and_then(|x| x.as_bool());
                }
                Some(reason @ ("compiler-artifact" | "compiler-message")) => {
                    let (Some(package), Some(target)) =
                        (s(&v, "package_id").and_then(package_name), target(&v))
                    else {
                        continue;
                    };
                    let doctest = v
                        .pointer("/target/doctest")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false);
                    if target.kind == Kind::Lib && doctest {
                        probe.libraries.insert(package.clone());
                    }
                    if reason == "compiler-artifact" {
                        if v.pointer("/profile/test").and_then(|x| x.as_bool()) == Some(true) {
                            let built = probe.built.entry(package).or_default();
                            if !built.contains(&target) {
                                built.push(target);
                            }
                        }
                        continue;
                    }
                    let Some(message) = v.get("message") else {
                        continue;
                    };
                    if s(message, "level") != Some("error") {
                        continue;
                    }
                    let file = message
                        .get("spans")
                        .and_then(|x| x.as_array())
                        .and_then(|spans| {
                            spans.iter().find(|sp| {
                                sp.get("is_primary").and_then(|p| p.as_bool()) == Some(true)
                            })
                        })
                        .and_then(|sp| s(sp, "file_name"))
                        .or_else(|| s(message, "rendered").and_then(rendered_file))
                        .map(|f| f.trim_start_matches("./").to_owned());
                    let rendered = s(message, "rendered")
                        .or_else(|| s(message, "message"))
                        .unwrap_or_default()
                        .trim()
                        .to_owned();
                    let at = match probe.failed.iter().position(|f| f.target == target) {
                        Some(at) => at,
                        None => {
                            probe.failed.push(Failed {
                                target,
                                errors: Vec::new(),
                            });
                            probe.failed.len() - 1
                        }
                    };
                    // rustc's closing "aborting due to N previous errors"
                    // names no file and adds nothing.
                    let closing = rendered
                        .trim_start_matches("error: ")
                        .starts_with("aborting due to");
                    if file.is_some() || !closing {
                        probe.failed[at].errors.push((file, rendered));
                    }
                }
                _ => {}
            }
        }
        probe
    }

    /// The test files that stopped the build, when only `restored` files
    /// did: the build failed, and every target that did not build has
    /// errors, each in one of `restored` (repository paths; rustc names
    /// files relative to the Cargo workspace, which is the check's `cwd`).
    pub fn restored_only(&self, cwd: Option<&str>, restored: &BTreeSet<String>) -> Option<Unbuilt> {
        if self.success != Some(false) || self.failed.is_empty() {
            return None;
        }
        let mut unbuilt = Unbuilt::default();
        for failed in &self.failed {
            if failed.errors.is_empty() {
                return None;
            }
            let label = failed.target.label();
            for (file, error) in &failed.errors {
                let file = file.as_deref()?;
                let path = match cwd {
                    Some(cwd) if !cwd.is_empty() => format!("{cwd}/{file}"),
                    _ => file.to_owned(),
                };
                if !restored.contains(&path) {
                    return None;
                }
                let entry = unbuilt.files.entry(path).or_insert_with(|| UnbuiltFile {
                    error: error.clone(),
                    targets: BTreeSet::new(),
                });
                entry.targets.insert(label.clone());
            }
            unbuilt.targets.insert(label);
        }
        Some(unbuilt)
    }
}

/// The restored test files that did not compile on the judged head.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unbuilt {
    /// The labels ([`Target::label`]) of the test targets that did not build.
    pub targets: BTreeSet<String>,
    /// Each file with a compile error, by repository path.
    pub files: BTreeMap<String, UnbuiltFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnbuiltFile {
    /// Its first compiler error, as rustc's one-line message.
    pub error: String,
    /// The labels of the test targets it kept from building.
    pub targets: BTreeSet<String>,
}

/// The file of a short-form message (`tests/api.rs:3:5: error[E0425]: …`),
/// for an error whose spans name no primary file.
fn rendered_file(rendered: &str) -> Option<&str> {
    let (file, rest) = rendered.split_once(':')?;
    let (line, rest) = rest.split_once(':')?;
    let (column, _) = rest.split_once(':')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (!file.is_empty() && digits(line) && digits(column)).then_some(file)
}

fn s<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

/// The target of a cargo JSON message.
fn target(v: &serde_json::Value) -> Option<Target> {
    let t = v.get("target")?;
    let kinds: Vec<&str> = t
        .get("kind")?
        .as_array()?
        .iter()
        .filter_map(|k| k.as_str())
        .collect();
    let kind = if kinds.contains(&"test") {
        Kind::Test
    } else if kinds.contains(&"bin") {
        Kind::Bin
    } else if kinds.iter().any(|k| {
        matches!(
            *k,
            "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro"
        )
    }) {
        Kind::Lib
    } else {
        Kind::Other
    };
    let name = t.get("name")?.as_str()?.to_owned();
    let src = t.get("src_path")?.as_str()?;
    let dir = v
        .get("manifest_path")?
        .as_str()?
        .rsplit_once('/')
        .map(|(d, _)| d)?;
    let src = src.strip_prefix(dir)?.trim_start_matches('/').to_owned();
    Some(Target { kind, name, src })
}

/// A package's name from its id: `path+file:///…/calc#0.1.0` (named like
/// its directory), `path+file:///…#calc@0.1.0`, or before Cargo 1.77
/// `calc 0.1.0 (path+file:///…)`.
fn package_name(id: &str) -> Option<String> {
    if id.contains(' ') {
        return id.split_whitespace().next().map(String::from);
    }
    let (url, fragment) = id.rsplit_once('#')?;
    match fragment.split_once('@') {
        Some((name, _)) => Some(name.to_owned()),
        None => url
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .map(String::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(script: &str) -> CheckDef {
        CheckDef::new(
            "rust".parse().unwrap(),
            vec!["sh".into(), "-c".into(), script.into()],
        )
    }

    const PRESET: &str = "CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test --workspace --no-fail-fast --locked >/aver/out/cargo.txt 2>&1; s=$?; cat /aver/out/cargo.txt; exit $s";
    const OWN: &str = "if [ \"$(uname)\" = Linux ]; then export CARGO_BUILD_JOBS=2; fi; cargo test --workspace --lib --bins --tests --examples --no-fail-fast --locked >/aver/out/rust.txt 2>&1; s=$?; cat /aver/out/rust.txt; exit $s";

    /// Defect: a narrowed command that runs other targets than the full
    /// one would, or whose exit code is not cargo's, stands in for it.
    #[test]
    fn a_command_narrows_only_when_its_runs_and_exit_code_are_cargos() {
        let set = |names: &[&str]| -> BTreeSet<String> {
            names.iter().map(|n| (*n).to_owned()).collect()
        };
        let script = |d: Option<CheckDef>| d.map(|d| d.argv[2].clone());
        assert_eq!(
            script(CargoTest::narrowed(&def(PRESET), &set(&["api"]), false)).as_deref(),
            Some("CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test --workspace --locked --test api --no-fail-fast >/aver/out/cargo.txt 2>&1; s=$?; cat /aver/out/cargo.txt; exit $s")
        );
        assert_eq!(
            script(CargoTest::narrowed(&def(OWN), &set(&["a-b"]), true)).as_deref(),
            Some("if [ \"$(uname)\" = Linux ]; then export CARGO_BUILD_JOBS=2; fi; cargo test --workspace --locked --lib --bins --test a-b --no-fail-fast >/aver/out/rust.txt 2>&1; s=$?; cat /aver/out/rust.txt; exit $s"),
            "the examples are built, never run"
        );
        let stdout = "( cargo test -p calc --no-fail-fast 2>&1 ) >/aver/out/o.txt; s=$?; cat /aver/out/o.txt; exit $s";
        assert!(CargoTest::narrowed(&def(stdout), &set(&["api"]), false).is_some());
        assert!(CargoTest::narrowed(
            &def("cargo test --no-fail-fast -- --test-threads=1"),
            &set(&["api"]),
            false
        )
        .is_some_and(|d| d.argv[2] == "cargo test --test api --no-fail-fast -- --test-threads=1"));
        for (why, script, tests, units) in [
            ("doc-tests run with the units", PRESET, &["api"][..], true),
            (
                "stops at the first failing binary",
                "cargo test --workspace",
                &["api"][..],
                false,
            ),
            (
                "exit code not cargo's",
                "cargo test --no-fail-fast && :",
                &["api"][..],
                false,
            ),
            (
                "output piped",
                "cargo test --no-fail-fast | tee out.txt",
                &["api"][..],
                false,
            ),
            (
                "printed from another file",
                "cargo test --no-fail-fast >a.txt 2>&1; s=$?; cat b.txt; exit $s",
                &["api"][..],
                false,
            ),
            (
                "the full command never runs it",
                "cargo test --test other --no-fail-fast",
                &["api"][..],
                false,
            ),
            (
                "one named binary",
                "cargo test --lib --bin aver --no-fail-fast",
                &[][..],
                true,
            ),
            ("nothing to run", PRESET, &[][..], false),
        ] {
            assert_eq!(
                CargoTest::narrowed(&def(script), &set(tests), units),
                None,
                "{why}"
            );
        }
    }

    fn probe_output(dir: &str) -> String {
        let id = format!("path+file://{dir}/crates/calc#0.1.0");
        let manifest = format!("{dir}/crates/calc/Cargo.toml");
        let target = |kind: &str, name: &str, src: &str, doctest: bool| {
            format!(
                r#""target":{{"kind":["{kind}"],"crate_types":["lib"],"name":"{name}","src_path":"{dir}/crates/calc/{src}","edition":"2021","doc":true,"doctest":{doctest},"test":true}}"#
            )
        };
        let artifact = |t: String| {
            format!(
                r#"{{"reason":"compiler-artifact","package_id":"{id}","manifest_path":"{manifest}",{t},"profile":{{"opt_level":"0","test":true}},"fresh":false}}"#
            )
        };
        let error = |t: String, file: &str| {
            format!(
                r#"{{"reason":"compiler-message","package_id":"{id}","manifest_path":"{manifest}",{t},"message":{{"rendered":"{file}:2:30: error[E0425]: cannot find function `sub` in crate `calc`\n","level":"error","spans":[{{"file_name":"{file}","is_primary":true}}]}}}}"#
            )
        };
        [
            "   Compiling calc v0.1.0".to_owned(),
            error(
                target("test", "gone", "tests/gone.rs", false),
                "crates/calc/tests/gone.rs",
            ),
            artifact(target("lib", "calc", "src/lib.rs", true)),
            artifact(target("test", "api", "tests/api.rs", false)),
            r#"{"reason":"build-finished","success":false}"#.to_owned(),
        ]
        .join("\n")
    }

    #[test]
    fn the_preset_and_aver_s_own_command_are_read() {
        let c = CargoTest::of(&def(PRESET)).expect("the preset");
        assert_eq!(c.prefix, "CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 ");
        assert_eq!(c.packages, ["--workspace"]);
        assert!(c.select.default);
        let own = CargoTest::of(&def(OWN)).expect("aver.toml's rust check");
        assert_eq!(own.prefix, "");
        assert!(own.select.lib && own.select.bins && own.select.tests && !own.select.default);
        assert!(own.select.examples && !own.select.doc());
        assert!(own.probe().argv[2].contains(" --examples --tests --keep-going "));
        let rerun = own.over(&Probe::read(&probe_output("/slot"))).unwrap();
        assert!(
            rerun.argv[2]
                .contains("cargo test --locked -p calc --lib --test api --examples --no-fail-fast"),
            "{}",
            rerun.argv[2]
        );
    }

    #[test]
    fn a_command_aver_cannot_rerun_is_left_alone() {
        for script in [
            "cargo test --all-targets >/aver/out/r.txt 2>&1",
            "cargo test \"$ARGS\" >/aver/out/r.txt",
            "cargo test; cargo test -p other",
            "env X=1 cargo test >/aver/out/r.txt",
            "cargo nextest run",
        ] {
            assert!(CargoTest::of(&def(script)).is_none(), "{script}");
        }
        let argv = CheckDef::new("rust".parse().unwrap(), vec!["cargo".into(), "test".into()]);
        assert!(CargoTest::of(&argv).is_none());
    }

    #[test]
    fn the_probe_builds_every_test_target_and_keeps_the_rest_of_the_script() {
        let probe = CargoTest::of(&def(PRESET)).unwrap().probe();
        let report = probe.report.expect("the probe's stream is its report");
        assert_eq!(
            (report.format, report.path.as_str()),
            (aver_core::ReportFormat::CargoProbe, PROBE_REPORT)
        );
        assert_eq!(
            probe.argv[2],
            format!("(CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo build --workspace --locked --tests --keep-going --message-format=json-diagnostic-short >\"{PROBE_REPORT}\") >/aver/out/cargo.txt 2>&1; s=$?; cat /aver/out/cargo.txt; exit $s")
        );
    }

    /// The probe's command in a shell, with `cargo` printing `stream`: its
    /// stream lands whole in the report file, not in the script's output.
    #[test]
    fn the_probe_writes_cargo_s_stream_to_its_own_file() {
        let out = tempfile::tempdir().unwrap();
        let stream = probe_output("/slot");
        std::fs::write(out.path().join("stream"), format!("{stream}\n")).unwrap();
        let probe = CargoTest::of(&def(PRESET)).unwrap().probe();
        let script = format!(
            "cargo() {{ cat \"$STREAM\"; echo '   Compiling calc' >&2; }}; {}",
            probe.argv[2]
        )
        .replace("/aver/out/", &format!("{}/", out.path().display()));
        let ran = std::process::Command::new("sh")
            .args(["-c", &script])
            .env("STREAM", out.path().join("stream"))
            .output()
            .unwrap();
        let file = std::fs::read_to_string(out.path().join("aver-probe.jsonl")).unwrap();
        assert_eq!(file, format!("{stream}\n"));
        let printed = String::from_utf8_lossy(&ran.stdout);
        assert!(!printed.contains("compiler-artifact"), "{printed}");
        assert!(printed.contains("Compiling calc"), "{printed}");
    }

    #[test]
    fn the_probe_output_names_what_built_and_what_did_not() {
        let probe = Probe::read(&probe_output("/slot"));
        assert_eq!(probe.success, Some(false));
        let built: Vec<String> = probe.built["calc"].iter().map(Target::label).collect();
        assert_eq!(built, ["calc src/lib.rs", "api tests/api.rs"]);
        assert_eq!(probe.libraries, BTreeSet::from(["calc".to_owned()]));
        assert_eq!(probe.failed.len(), 1);
        assert_eq!(probe.failed[0].target.label(), "gone tests/gone.rs");

        let restored = BTreeSet::from(["crates/calc/tests/gone.rs".to_owned()]);
        let unbuilt = probe
            .restored_only(None, &restored)
            .expect("only gone.rs stops it");
        assert_eq!(
            unbuilt.targets,
            BTreeSet::from(["gone tests/gone.rs".to_owned()])
        );
        let file = &unbuilt.files["crates/calc/tests/gone.rs"];
        assert!(
            file.error.contains("cannot find function `sub`"),
            "{}",
            file.error
        );
        assert!(probe.restored_only(None, &BTreeSet::new()).is_none());
        assert!(probe.restored_only(Some("sub"), &restored).is_none());

        // A stream that stops before `build-finished` is incomplete.
        let output = probe_output("/slot");
        let cut = Probe::read(output.rsplit_once('\n').unwrap().0);
        assert_eq!(cut.success, None);
        assert!(cut.restored_only(None, &restored).is_none());
    }

    /// The real case: aver's own workspace, with five restored test files
    /// that did not compile, printed 326 KiB of probe output in 1-7 KiB
    /// lines; read from the run's bounded output, 32 of the 52 test targets
    /// that built were missing from the rerun. Read from its file, the
    /// stream is whole.
    #[test]
    fn a_workspace_of_sixty_test_targets_is_read_whole() {
        let dir = format!(
            "/private/var/folders/rf/{}/T/aver-ci-kmox98/sides/284af8ee27054cfd/S-sexzjlggqrtbyrga",
            "x".repeat(100)
        );
        let id = format!("path+file://{dir}/crates/big#0.3.0");
        let manifest = format!("{dir}/crates/big/Cargo.toml");
        let target = |kind: &str, name: &str, src: &str| {
            format!(
                r#""target":{{"kind":["{kind}"],"crate_types":["bin"],"name":"{name}","src_path":"{dir}/crates/big/{src}","edition":"2021","doc":false,"doctest":false,"test":true}}"#
            )
        };
        let mut lines = vec!["   Compiling big v0.3.0".to_owned()];
        for n in 0..60 {
            let t = target(
                "test",
                &format!("suite_{n:02}"),
                &format!("tests/suite_{n:02}.rs"),
            );
            lines.push(format!(
                r#"{{"reason":"compiler-artifact","package_id":"{id}","manifest_path":"{manifest}",{t},"profile":{{"opt_level":"0","debuginfo":2,"debug_assertions":true,"overflow_checks":true,"test":true}},"features":[],"filenames":["/Users/someone/dev/big/target/debug/deps/suite_{n:02}-1b40b15d22a38cbe"],"executable":"/Users/someone/dev/big/target/debug/deps/suite_{n:02}-1b40b15d22a38cbe","fresh":true}}"#
            ));
        }
        // Five targets that include a restored support module, each with
        // several errors whose spans and explanation run past 4 KiB.
        let explanation = r"An undeclared crate, module, or type was used.\n".repeat(80);
        for n in 0..5 {
            let t = target(
                "test",
                &format!("broken_{n}"),
                &format!("tests/broken_{n}.rs"),
            );
            for line in 10..14 {
                lines.push(format!(
                    r#"{{"reason":"compiler-message","package_id":"{id}","manifest_path":"{manifest}",{t},"message":{{"rendered":"crates/big/tests/support/mod.rs:{line}:11: error[E0433]: cannot find `http` in `big`\n","$message_type":"diagnostic","children":[],"level":"error","message":"cannot find `http` in `big`","spans":[{{"file_name":"crates/big/tests/support/mod.rs","is_primary":true,"text":[]}}],"code":{{"code":"E0433","explanation":"{explanation}"}}}}}}"#
                ));
            }
        }
        assert!(lines[1..].iter().all(|l| l.len() > 1024));
        lines.push(r#"{"reason":"build-finished","success":false}"#.to_owned());
        let raw = lines.join("\n");
        assert!(lines.iter().any(|l| l.len() > 4096));
        assert!(raw.len() > 2 * 65_536, "{}", raw.len());

        let probe = Probe::read(&raw);
        assert_eq!(probe.success, Some(false));
        assert_eq!(probe.built["big"].len(), 60);
        assert_eq!(probe.failed.len(), 5);
        let restored = BTreeSet::from(["crates/big/tests/support/mod.rs".to_owned()]);
        let unbuilt = probe
            .restored_only(None, &restored)
            .expect("only support/mod.rs");
        assert_eq!(unbuilt.targets.len(), 5);
        let rerun = CargoTest::of(&def(OWN)).unwrap().over(&probe).unwrap();
        for n in 0..60 {
            assert!(
                rerun.argv[2].contains(&format!("--test suite_{n:02} ")),
                "suite_{n:02}"
            );
        }
    }

    #[test]
    fn the_rerun_tests_what_built_once_per_package() {
        let probe = Probe::read(&probe_output("/slot"));
        let rerun = CargoTest::of(&def(PRESET)).unwrap().over(&probe).unwrap();
        assert_eq!(
            rerun.argv[2],
            "( s=0; CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test --locked -p calc --lib --test api --no-fail-fast || s=1; CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test --locked -p calc --doc --no-fail-fast || s=1; exit $s ) >/aver/out/cargo.txt 2>&1; s=$?; cat /aver/out/cargo.txt; exit $s"
        );
        let only = CargoTest::of(&def(
            "cargo test -p calc --test api div -- --test-threads=1 >/aver/out/r.txt 2>&1",
        ))
        .unwrap()
        .over(&probe)
        .unwrap();
        assert_eq!(
            only.argv[2],
            "( s=0; cargo test div -p calc --test api --no-fail-fast -- --test-threads=1 || s=1; exit $s ) >/aver/out/r.txt 2>&1"
        );
    }

    #[test]
    fn package_names_come_from_either_id_form() {
        assert_eq!(
            package_name("path+file:///w/crates/calc#0.1.0").as_deref(),
            Some("calc")
        );
        assert_eq!(
            package_name("path+file:///w/x#calc@0.1.0").as_deref(),
            Some("calc")
        );
        assert_eq!(
            package_name("calc 0.1.0 (path+file:///w)").as_deref(),
            Some("calc")
        );
    }
}
