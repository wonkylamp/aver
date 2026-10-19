//! Reads `aver.toml` into a [`Contract`] (ARCHITECTURE.md §1.13.1).
//!
//! Two shapes are read. The contract shape has string `run` lines and the
//! `[deps]`, `[judge]` and `[ci]` tables. The environment shapes of v0 to v3
//! (`[env]`, `[limits]`, `[vars]`, `[[deps]]` layers, argv `run` arrays) are
//! read deterministically into the contract they meant, with what no
//! contract holds listed in [`Shape::Legacy`]. Pure: text in, value out.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use aver_core::{
    CheckId, CheckSpec, CiPolicy, Contract, ContractCheck, Deps, JudgeGlobs, Path, PathGlob,
    ReportKind, ReviewPolicy, Set, SetItem, ShellLine,
};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContractError {
    #[error("aver.toml is not valid TOML: {0}")]
    Syntax(String),
    #[error("aver.toml: {0}")]
    Invalid(String),
    /// An environment-shaped file without `[[check]]`, which relied on
    /// detecting `npm test`; a contract names its commands.
    #[error("aver.toml declares no [[check]]; a contract names the commands that run the tests")]
    NoChecks,
}

fn invalid(reason: impl std::fmt::Display) -> ContractError {
    ContractError::Invalid(reason.to_string())
}

/// Which shape the file had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    Contract,
    /// A v0 to v3 environment file. `ignored` names, in file order, each
    /// setting the contract has no place for (images, limits, variables).
    Legacy {
        ignored: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub contract: Contract,
    pub shape: Shape,
}

/// Parses `aver.toml`. A file is the environment shape when it has an
/// `[env]`, `[limits]`, `[vars]` or `[unit]` table, `[[deps]]` as an array,
/// or a check whose `run` is an array; otherwise the contract shape.
pub fn parse(text: &str) -> Result<Parsed, ContractError> {
    let table: toml::Table =
        toml::from_str(text).map_err(|e| ContractError::Syntax(e.message().to_owned()))?;
    let syntax = |e: toml::de::Error| invalid(e.message());
    if is_legacy(&table) {
        let file: LegacyFile = toml::from_str(text).map_err(syntax)?;
        legacy(file)
    } else {
        let file: ContractFile = toml::from_str(text).map_err(syntax)?;
        Ok(Parsed {
            contract: file.into_contract()?,
            shape: Shape::Contract,
        })
    }
}

/// Whether `table` is the environment shape [`parse`] converts.
pub fn is_legacy(table: &toml::Table) -> bool {
    let run_is_argv = |check: &toml::Value| check.get("run").is_some_and(toml::Value::is_array);
    ["env", "limits", "vars", "unit"]
        .iter()
        .any(|k| table.contains_key(*k))
        || table.get("deps").is_some_and(toml::Value::is_array)
        || table
            .get("check")
            .and_then(toml::Value::as_array)
            .is_some_and(|checks| checks.iter().any(run_is_argv))
}

fn set<T: SetItem + PartialEq>(items: Vec<T>) -> Result<Set<T>, ContractError> {
    Set::new(items).map_err(invalid)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContractFile {
    #[serde(default)]
    check: Vec<CheckToml>,
    #[serde(default)]
    deps: DepsToml,
    #[serde(default)]
    judge: JudgeToml,
    #[serde(default)]
    ci: CiToml,
    /// Read by the Claude Code stop hook from the base snapshot (§2.12).
    #[serde(default, rename = "agents")]
    _agents: Option<toml::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckToml {
    id: CheckId,
    run: ShellLine,
    cwd: Option<Path>,
    prepare: Option<ShellLine>,
    #[serde(default = "junit")]
    report: ReportKind,
    report_file: Option<Path>,
    min_tests: Option<NonZeroU32>,
    paths: Option<Vec<PathGlob>>,
    #[serde(default)]
    serial: bool,
    timeout_s: Option<NonZeroU32>,
}

fn junit() -> ReportKind {
    ReportKind::Junit
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DepsToml {
    #[serde(default)]
    keep: Vec<PathGlob>,
    #[serde(default)]
    seed: Vec<PathGlob>,
    install: Option<ShellLine>,
    #[serde(default)]
    inputs: Vec<PathGlob>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct JudgeToml {
    #[serde(default)]
    tests: Vec<PathGlob>,
    #[serde(default)]
    harness: Vec<PathGlob>,
    /// Absent: [`aver_core::contract::DEFAULT_BUILT_TESTS`].
    built_tests: Option<Vec<PathGlob>>,
    #[serde(default)]
    protected: Vec<PathGlob>,
    /// Absent: [`aver_core::contract::DEFAULT_SENSITIVE`]; `[]` turns it off.
    sensitive: Option<Vec<PathGlob>>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CiToml {
    #[serde(default)]
    review: ReviewPolicy,
    #[serde(default)]
    require_recorded_base: bool,
}

impl ContractFile {
    fn into_contract(self) -> Result<Contract, ContractError> {
        let checks = self
            .check
            .into_iter()
            .map(|c| {
                let paths = c.paths.map(set).transpose()?;
                ContractCheck::try_from(CheckSpec {
                    id: c.id,
                    run: c.run,
                    cwd: c.cwd,
                    prepare: c.prepare,
                    report: c.report,
                    report_file: c.report_file,
                    min_tests: c.min_tests,
                    paths,
                    serial: c.serial,
                    timeout_s: c.timeout_s,
                })
                .map_err(invalid)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let deps = Deps::new(
            set(self.deps.keep)?,
            set(self.deps.seed)?,
            self.deps.install,
            set(self.deps.inputs)?,
        )
        .map_err(invalid)?;
        let judge = JudgeGlobs {
            tests: set(self.judge.tests)?,
            harness: set(self.judge.harness)?,
            built_tests: match self.judge.built_tests {
                Some(globs) => set(globs)?,
                None => JudgeGlobs::default_built_tests(),
            },
            protected: set(self.judge.protected)?,
            sensitive: match self.judge.sensitive {
                Some(globs) => set(globs)?,
                None => JudgeGlobs::default_sensitive(),
            },
        };
        let ci = CiPolicy {
            review: self.ci.review,
            require_recorded_base: self.ci.require_recorded_base,
        };
        Contract::new(set(checks)?, deps, judge, ci).map_err(invalid)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyFile {
    #[serde(default)]
    env: LegacyEnv,
    #[serde(default)]
    limits: BTreeMap<String, toml::Value>,
    check: Option<Vec<LegacyCheck>>,
    #[serde(default)]
    deps: Vec<LegacyLayer>,
    #[serde(default)]
    vars: BTreeMap<String, String>,
    #[serde(default)]
    unit: Option<toml::Value>,
    #[serde(default)]
    workspace: LegacyWorkspace,
    #[serde(default, rename = "agents")]
    _agents: Option<toml::Value>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyEnv {
    image: Option<String>,
    lockfile: Option<Path>,
    install: Option<Vec<String>>,
    allow_scripts: Option<bool>,
    rebuild: Option<bool>,
    tests: Option<Vec<PathGlob>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyCheck {
    id: CheckId,
    run: Vec<String>,
    cwd: Option<Path>,
    timeout_s: Option<NonZeroU32>,
    report: Option<LegacyReport>,
    setup: Option<Vec<Vec<String>>>,
    image: Option<String>,
    deps: Option<Vec<String>>,
    covers: Option<Vec<PathGlob>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyReport {
    format: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyLayer {
    lockfile: Path,
    kind: Option<String>,
    install: Option<Vec<String>>,
    allow_scripts: Option<bool>,
    rebuild: Option<bool>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyWorkspace {
    node_modules: Option<String>,
    clone_packages: Option<Vec<String>>,
    #[serde(default)]
    seed: Vec<PathGlob>,
    #[serde(default)]
    warm: Vec<PathGlob>,
    ports: Option<u32>,
}

const SANDBOX_OUT: &str = "/aver/out";
const DEFAULT_NPM_INSTALL: &[&str] = &["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund"];

/// The ecosystems of the v3 dependency layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ecosystem {
    Npm,
    Uv,
    Pip,
    Cargo,
    Go,
}

impl Ecosystem {
    fn named(kind: &str) -> Option<Self> {
        Some(match kind {
            "npm" => Self::Npm,
            "uv" => Self::Uv,
            "pip" => Self::Pip,
            "cargo" => Self::Cargo,
            "go" => Self::Go,
            _ => return None,
        })
    }

    /// The kind a lockfile's name implies (§1.4.1 `[[deps]]`).
    fn of_lockfile(name: &str) -> Option<Self> {
        match name {
            "package-lock.json" | "npm-shrinkwrap.json" => Some(Self::Npm),
            "uv.lock" => Some(Self::Uv),
            "Cargo.lock" => Some(Self::Cargo),
            "go.sum" => Some(Self::Go),
            _ if name.ends_with(".txt") && name.contains("requirements") => Some(Self::Pip),
            _ => None,
        }
    }

    fn default_install(self, lockfile_name: &str) -> Vec<String> {
        let words: &[&str] = match self {
            Self::Npm => DEFAULT_NPM_INSTALL,
            Self::Uv => &["uv", "sync", "--frozen", "--no-install-workspace"],
            Self::Pip => &["pip", "install", "--no-cache-dir", "-r", lockfile_name],
            Self::Cargo => &["cargo", "fetch", "--locked"],
            Self::Go => &["go", "mod", "download"],
        };
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    /// The untracked directory its install fills, kept or seeded per side.
    fn installed_dir(self) -> Option<(&'static str, Sharing)> {
        match self {
            Self::Npm => Some(("node_modules", Sharing::Keep)),
            Self::Uv => Some((".venv", Sharing::Keep)),
            Self::Cargo => Some(("target", Sharing::Seed)),
            Self::Pip | Self::Go => None,
        }
    }
}

/// How the sides get an installed directory: shared as it is, or cloned
/// into each side because the runs write it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sharing {
    Keep,
    Seed,
}

fn split_lockfile(path: &Path) -> (&str, &str) {
    path.as_str()
        .rsplit_once('/')
        .unwrap_or(("", path.as_str()))
}

fn under(dir: &str, name: &str) -> Result<PathGlob, ContractError> {
    let glob = if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    };
    glob.parse().map_err(invalid)
}

fn legacy(file: LegacyFile) -> Result<Parsed, ContractError> {
    let mut ignored = Vec::new();
    let mut ignore = |what: String| ignored.push(what);
    if file.env.image.is_some() {
        ignore("[env] image".into());
    }
    for (key, set) in [
        ("allow_scripts", file.env.allow_scripts.is_some()),
        ("rebuild", file.env.rebuild.is_some()),
    ] {
        if set {
            ignore(format!("[env] {key}"));
        }
    }
    for key in file.limits.keys().filter(|k| k.as_str() != "timeout_s") {
        ignore(format!("[limits] {key}"));
    }
    for key in file.vars.keys() {
        ignore(format!("[vars] {key}"));
    }
    if file.unit.is_some() {
        ignore("[[unit]]".into());
    }
    let ws = &file.workspace;
    for (key, set) in [
        ("node_modules", ws.node_modules.is_some()),
        ("clone_packages", ws.clone_packages.is_some()),
        ("ports", ws.ports.is_some()),
    ] {
        if set {
            ignore(format!("[workspace] {key}"));
        }
    }

    let default_timeout = match file.limits.get("timeout_s") {
        None => None,
        Some(value) => Some(
            value
                .as_integer()
                .and_then(|n| u32::try_from(n).ok())
                .and_then(NonZeroU32::new)
                .ok_or_else(|| invalid("[limits] timeout_s must be a positive integer"))?,
        ),
    };

    let checks = file.check.ok_or(ContractError::NoChecks)?;
    if checks.is_empty() {
        return Err(ContractError::NoChecks);
    }
    let mut specs = Vec::new();
    for check in checks {
        let id = &check.id;
        if check.image.is_some() {
            ignore(format!("check {id}: image"));
        }
        if check.deps.is_some() {
            ignore(format!("check {id}: deps"));
        }
        let (report, report_file) = match &check.report {
            None => (ReportKind::Exit, None),
            Some(r) => legacy_report(id, r)?,
        };
        let prepare = match check.setup.as_deref() {
            None | Some([]) => None,
            Some(steps) => Some(
                steps
                    .iter()
                    .map(|argv| shell_words(argv))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(" && "),
            ),
        };
        specs.push(CheckSpec {
            run: shell_line(&check.run)?,
            cwd: check.cwd,
            prepare: prepare.map(|p| p.parse().map_err(invalid)).transpose()?,
            report,
            report_file,
            min_tests: (report != ReportKind::Exit).then_some(NonZeroU32::MIN),
            paths: check.covers.map(set).transpose()?,
            serial: false,
            timeout_s: check.timeout_s.or(default_timeout),
            id: check.id,
        });
    }
    let checks = specs
        .into_iter()
        .map(|s| ContractCheck::try_from(s).map_err(invalid))
        .collect::<Result<Vec<_>, _>>()?;

    let env_lockfile = file
        .env
        .lockfile
        .clone()
        .unwrap_or_else(|| "package-lock.json".parse().expect("a valid path"));
    let mut layers = vec![(
        env_lockfile,
        Ecosystem::Npm,
        file.env
            .install
            .clone()
            .unwrap_or_else(|| Ecosystem::Npm.default_install("package-lock.json")),
    )];
    for layer in &file.deps {
        let (_, name) = split_lockfile(&layer.lockfile);
        let kind = match &layer.kind {
            Some(kind) => Ecosystem::named(kind).ok_or_else(|| {
                invalid(format!("deps {}: unknown kind {kind:?}", layer.lockfile))
            })?,
            None => Ecosystem::of_lockfile(name).ok_or_else(|| {
                invalid(format!(
                    "deps {}: name a kind; the file name implies none",
                    layer.lockfile
                ))
            })?,
        };
        for (key, set) in [
            ("allow_scripts", layer.allow_scripts.is_some()),
            ("rebuild", layer.rebuild.is_some()),
        ] {
            if set {
                ignore(format!("deps {}: {key}", layer.lockfile));
            }
        }
        let install = layer
            .install
            .clone()
            .unwrap_or_else(|| kind.default_install(name));
        layers.push((layer.lockfile.clone(), kind, install));
    }

    let (mut keep, mut seed, mut inputs, mut installs) = (vec![], vec![], vec![], vec![]);
    for (lockfile, kind, install) in &layers {
        let (dir, _) = split_lockfile(lockfile);
        inputs.push(lockfile.as_str().parse().map_err(invalid)?);
        if let Some((name, sharing)) = kind.installed_dir() {
            let glob = under(dir, name)?;
            match sharing {
                Sharing::Keep => keep.push(glob),
                Sharing::Seed => seed.push(glob),
            }
        }
        let words = shell_words(install)?;
        installs.push(if dir.is_empty() {
            words
        } else {
            format!("(cd {} && {words})", quote(dir))
        });
    }
    seed.extend(file.workspace.seed);
    seed.extend(file.workspace.warm);
    let install: ShellLine = installs.join(" && ").parse().map_err(invalid)?;
    let deps = Deps::new(set(keep)?, set(seed)?, Some(install), set(inputs)?).map_err(invalid)?;

    let judge = JudgeGlobs {
        tests: set(file.env.tests.unwrap_or_default())?,
        ..JudgeGlobs::default()
    };
    let contract =
        Contract::new(set(checks)?, deps, judge, CiPolicy::default()).map_err(invalid)?;
    Ok(Parsed {
        contract,
        shape: Shape::Legacy { ignored },
    })
}

/// A v0 to v3 report: the JUnit dialects read as JUnit (every `*.xml` in
/// `$AVER_OUT`), a text format from its file under `$AVER_OUT`.
fn legacy_report(
    id: &CheckId,
    report: &LegacyReport,
) -> Result<(ReportKind, Option<Path>), ContractError> {
    let kind = match report.format.as_str() {
        "junit" | "mocha-xunit" | "pytest-junit" => return Ok((ReportKind::Junit, None)),
        "libtest" => ReportKind::Libtest,
        "go-json" => ReportKind::GoJson,
        "tsc" => ReportKind::Tsc,
        "jest-json" => ReportKind::JestJson,
        "tap" => ReportKind::Tap,
        other => {
            return Err(invalid(format!(
                "check {id}: unknown report format {other:?}"
            )))
        }
    };
    let file = report
        .path
        .strip_prefix(SANDBOX_OUT)
        .and_then(|rest| rest.strip_prefix('/'))
        .ok_or_else(|| {
            invalid(format!(
                "check {id}: report path must be under {SANDBOX_OUT}/"
            ))
        })?;
    Ok((kind, Some(file.parse().map_err(invalid)?)))
}

/// An argv as one shell line. `sh -c <script>` (or `bash -c`) is its
/// script. Otherwise each word is quoted where the shell would read it
/// differently. Either way the sandbox's `/aver/out` becomes `$AVER_OUT`.
pub fn shell_line(argv: &[String]) -> Result<ShellLine, ContractError> {
    let line = match argv {
        [shell, flag, script] if matches!(shell.as_str(), "sh" | "bash") && flag == "-c" => {
            replace_out(script, "$AVER_OUT")
        }
        _ => shell_words(argv)?,
    };
    line.parse().map_err(invalid)
}

fn shell_words(argv: &[String]) -> Result<String, ContractError> {
    if argv.is_empty() {
        return Err(invalid("a command has at least one word"));
    }
    Ok(argv
        .iter()
        .enumerate()
        .map(|(i, word)| {
            split_out(word)
                .into_iter()
                .map(|piece| match piece {
                    Piece::Out => "\"$AVER_OUT\"".to_owned(),
                    Piece::Text(t) if i == 0 && t.contains('=') => single_quote(t),
                    Piece::Text(t) => quote(t),
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" "))
}

enum Piece<'a> {
    Out,
    Text(&'a str),
}

/// Byte offsets of each `/aver/out` that is a whole path component.
fn out_at(text: &str) -> Vec<usize> {
    text.match_indices(SANDBOX_OUT)
        .map(|(at, _)| at)
        .filter(|at| {
            !text[at + SANDBOX_OUT.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        })
        .collect()
}

fn split_out(word: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut from = 0;
    for at in out_at(word) {
        if at > from {
            pieces.push(Piece::Text(&word[from..at]));
        }
        pieces.push(Piece::Out);
        from = at + SANDBOX_OUT.len();
    }
    if from < word.len() || pieces.is_empty() {
        pieces.push(Piece::Text(&word[from..]));
    }
    pieces
}

fn replace_out(script: &str, with: &str) -> String {
    let mut out = String::new();
    let mut from = 0;
    for at in out_at(script) {
        out.push_str(&script[from..at]);
        out.push_str(with);
        from = at + SANDBOX_OUT.len();
    }
    out.push_str(&script[from..]);
    out
}

/// `word` bare when the shell reads it as written, else single-quoted.
fn quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'_' | b'.' | b'/' | b'=' | b':' | b',' | b'+' | b'@' | b'%' | b'-'
                )
        });
    if plain {
        word.to_owned()
    } else {
        single_quote(word)
    }
}

fn single_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aver_core::{Object, Reading, ReportSpec, TextFormat};

    fn repo_file(rel: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    fn globs(c: &Set<PathGlob>) -> Vec<&str> {
        c.iter().map(PathGlob::as_str).collect()
    }

    fn check<'a>(contract: &'a Contract, id: &str) -> &'a ContractCheck {
        contract.check(&id.parse().unwrap()).unwrap()
    }

    fn ignored(parsed: &Parsed) -> Vec<&str> {
        match &parsed.shape {
            Shape::Legacy { ignored } => ignored.iter().map(String::as_str).collect(),
            Shape::Contract => panic!("expected the environment shape"),
        }
    }

    const NODE_TEST: &str = "node --test --test-reporter=spec --test-reporter-destination=stdout --test-reporter=junit --test-reporter-destination=\"$AVER_OUT\"/unit.xml";

    #[test]
    fn every_fixture_and_the_repository_root_read_as_a_contract() {
        let files = [
            "aver.toml",
            "fixtures/calc/aver.toml",
            "fixtures/flaky/aver.toml",
            "fixtures/limits/aver.toml",
            "fixtures/nettest/aver.toml",
            "fixtures/preexisting/aver.toml",
            "fixtures/probe/aver.toml",
            "fixtures/sh-calc/aver.toml",
        ];
        for file in files {
            let text = repo_file(file);
            let first = parse(&text).unwrap_or_else(|e| panic!("{file}: {e}"));
            assert!(matches!(first.shape, Shape::Legacy { .. }), "{file}");
            let again = parse(&text).unwrap();
            assert_eq!(
                first.contract.id(),
                again.contract.id(),
                "{file}: the reading is deterministic"
            );
        }
        let eval = "bench/eval/tasks/integration-trap/history/02-integration-suite/aver.toml";
        let parsed = parse(&repo_file(eval)).unwrap_or_else(|e| panic!("{eval}: {e}"));
        assert!(matches!(parsed.shape, Shape::Contract), "{eval}");
    }

    #[test]
    fn the_calc_fixture_reads_as_its_junit_check_and_npm_install() {
        let parsed = parse(&repo_file("fixtures/calc/aver.toml")).unwrap();
        let c = &parsed.contract;
        let unit = check(c, "unit");
        assert_eq!(unit.run.as_str(), NODE_TEST);
        assert_eq!(
            unit.reading,
            Reading::Tests {
                report: ReportSpec::Junit,
                min_tests: NonZeroU32::MIN
            }
        );
        assert_eq!(unit.timeout_s.map(NonZeroU32::get), Some(60));
        assert_eq!(unit.paths, None);
        assert_eq!(globs(&c.deps.inputs), ["package-lock.json"]);
        assert_eq!(globs(&c.deps.keep), ["node_modules"]);
        assert_eq!(
            c.deps.install.as_ref().map(ShellLine::as_str),
            Some("npm ci --ignore-scripts --no-audit --no-fund")
        );
        assert_eq!(c.judge, JudgeGlobs::default());
        assert_eq!(c.ci, CiPolicy::default());
        assert_eq!(
            ignored(&parsed),
            [
                "[env] image",
                "[limits] cpus",
                "[limits] memory_mb",
                "[limits] pids"
            ]
        );
    }

    #[test]
    fn the_repository_root_reads_with_its_layers_scripts_and_file_reports() {
        let parsed = parse(&repo_file("aver.toml")).unwrap();
        let c = &parsed.contract;
        let ids: Vec<&str> = c.checks.iter().map(|k| k.id.as_str()).collect();
        assert_eq!(ids, ["bench", "rust", "site", "site-typecheck"]);
        let rust = check(c, "rust");
        assert!(rust
            .run
            .as_str()
            .starts_with("if [ \"$(uname)\" = Linux ]; then export"));
        assert!(rust.run.as_str().ends_with(
            "cargo test --workspace --lib --bins --tests --examples --no-fail-fast --locked >$AVER_OUT/rust.txt 2>&1; s=$?; cat $AVER_OUT/rust.txt; exit $s"
        ));
        assert!(matches!(
            &rust.reading,
            Reading::Tests { report: ReportSpec::File { format: TextFormat::Libtest, path }, .. }
                if path.as_str() == "rust.txt"
        ));
        assert_eq!(rust.timeout_s.map(NonZeroU32::get), Some(1800));
        assert_eq!(
            globs(rust.paths.as_ref().unwrap()),
            [
                "crates/*/examples/**",
                "crates/*/src/**",
                "crates/*/tests/**",
                "fixtures/**",
                "integrations/claude-code/**",
                "schemas/**"
            ]
        );
        let site = check(c, "site");
        assert_eq!(site.cwd.as_ref().map(Path::as_str), Some("site"));
        assert_eq!(
            globs(&c.deps.inputs),
            ["Cargo.lock", "site/package-lock.json"]
        );
        assert_eq!(globs(&c.deps.keep), ["site/node_modules"]);
        assert_eq!(globs(&c.deps.seed), ["target"]);
        assert_eq!(
            c.deps.install.as_ref().unwrap().as_str(),
            "(cd site && npm ci --ignore-scripts --no-audit --no-fund) && cargo fetch --locked"
        );
        assert_eq!(
            ignored(&parsed),
            [
                "[env] image",
                "[limits] cpus",
                "[limits] memory_mb",
                "[limits] pids",
                "check rust: image",
                "check bench: image",
                "check bench: deps",
            ]
        );
    }

    #[test]
    fn a_check_without_a_report_is_exit_only_and_limits_give_the_timeout() {
        let parsed = parse(&repo_file("fixtures/limits/aver.toml")).unwrap();
        let oom = check(&parsed.contract, "oom");
        assert_eq!(oom.reading, Reading::Exit);
        assert_eq!(oom.min_tests(), None);
        assert_eq!(oom.timeout_s.map(NonZeroU32::get), Some(60));
        let sleep = check(&parsed.contract, "sleep");
        assert_eq!(sleep.timeout_s.map(NonZeroU32::get), Some(3));
        assert_eq!(sleep.run.as_str(), "node sleep.js");
    }

    #[test]
    fn the_shell_fixture_keeps_its_covers_as_paths() {
        let parsed = parse(&repo_file("fixtures/sh-calc/aver.toml")).unwrap();
        let unit = check(&parsed.contract, "unit");
        assert_eq!(unit.run.as_str(), "sh test/run.sh \"$AVER_OUT\"/unit.xml");
        assert_eq!(globs(unit.paths.as_ref().unwrap()), ["*.sh", "test/**"]);
    }

    #[test]
    fn argv_words_are_quoted_only_where_the_shell_would_change_them() {
        let line = |words: &[&str]| shell_line(&strings(words)).unwrap().as_str().to_owned();
        assert_eq!(line(&["npm", "test"]), "npm test");
        assert_eq!(
            line(&["node", "test/**/*.test.ts"]),
            "node 'test/**/*.test.ts'"
        );
        assert_eq!(line(&["echo", "it's here"]), r"echo 'it'\''s here'");
        assert_eq!(line(&["echo", ""]), "echo ''");
        assert_eq!(line(&["FOO=1", "x"]), "'FOO=1' x");
        assert_eq!(line(&["x", "FOO=1"]), "x FOO=1");
        assert_eq!(line(&["cp", "/aver/out"]), "cp \"$AVER_OUT\"");
        assert_eq!(
            line(&["cp", "a b/aver/out/x y"]),
            "cp 'a b'\"$AVER_OUT\"'/x y'"
        );
        assert_eq!(line(&["ls", "/aver/output"]), "ls /aver/output");
        assert_eq!(
            line(&["sh", "-c", "go test -json ./... > /aver/out/go.json"]),
            "go test -json ./... > $AVER_OUT/go.json"
        );
        assert_eq!(line(&["sh", "-c", "x", "arg0"]), "sh -c x arg0");
        assert!(shell_line(&[]).is_err());
    }

    #[test]
    fn setup_commands_become_prepare_and_dialect_reports_read_as_junit() {
        let text = r#"
[env]
lockfile = "web/package-lock.json"
tests = ["it/**"]

[[deps]]
lockfile = "py/uv.lock"

[[deps]]
lockfile = "py/requirements-dev.txt"

[[deps]]
lockfile = "go/go.sum"
rebuild = true

[[check]]
id = "pytest"
cwd = "py"
run = ["python", "-m", "pytest", "--junitxml=/aver/out/py.xml"]
setup = [["python", "-m", "mypy", "."], ["make", "gen"]]
report = { format = "pytest-junit", path = "/aver/out/py.xml" }

[[check]]
id = "go"
run = ["sh", "-c", "go test -json ./... > /aver/out/go.json"]
report = { format = "go-json", path = "/aver/out/go/../go.json" }

[vars]
CI = "1"

[workspace]
seed = [".env.local"]
ports = 4
"#;
        let err = parse(text).unwrap_err();
        assert!(err.to_string().contains("go/../go.json"), "{err}");
        let parsed = parse(&text.replace("/aver/out/go/../go.json", "/aver/out/go.json")).unwrap();
        let c = &parsed.contract;
        let pytest = check(c, "pytest");
        assert_eq!(
            pytest.prepare.as_ref().map(ShellLine::as_str),
            Some("python -m mypy . && make gen")
        );
        assert!(matches!(
            pytest.reading,
            Reading::Tests {
                report: ReportSpec::Junit,
                ..
            }
        ));
        assert_eq!(
            pytest.run.as_str(),
            "python -m pytest --junitxml=\"$AVER_OUT\"/py.xml"
        );
        assert!(matches!(
            &check(c, "go").reading,
            Reading::Tests { report: ReportSpec::File { format: TextFormat::GoJson, path }, .. }
                if path.as_str() == "go.json"
        ));
        assert_eq!(
            globs(&c.deps.inputs),
            [
                "go/go.sum",
                "py/requirements-dev.txt",
                "py/uv.lock",
                "web/package-lock.json"
            ]
        );
        assert_eq!(globs(&c.deps.keep), ["py/.venv", "web/node_modules"]);
        assert_eq!(globs(&c.deps.seed), [".env.local"]);
        assert_eq!(
            c.deps.install.as_ref().unwrap().as_str(),
            "(cd web && npm ci --ignore-scripts --no-audit --no-fund) && (cd py && uv sync --frozen --no-install-workspace) && (cd py && pip install --no-cache-dir -r requirements-dev.txt) && (cd go && go mod download)"
        );
        assert_eq!(globs(&c.judge.tests), ["it/**"]);
        assert_eq!(
            ignored(&parsed),
            ["[vars] CI", "[workspace] ports", "deps go/go.sum: rebuild"]
        );
    }

    #[test]
    fn an_environment_file_without_checks_has_no_contract() {
        assert_eq!(
            parse("[env]\nlockfile = \"package-lock.json\"\n"),
            Err(ContractError::NoChecks)
        );
        assert_eq!(
            parse("check = []\n[limits]\ntimeout_s = 5\n"),
            Err(ContractError::NoChecks)
        );
        assert!(
            matches!(parse("[[deps]]\nlockfile = \"x.lock\"\n[[check]]\nid = \"a\"\nrun = [\"a\"]\n"), Err(ContractError::Invalid(e)) if e.contains("name a kind"))
        );
    }

    #[test]
    fn the_contract_shape_reads_with_its_defaults() {
        let parsed = parse(
            r#"
[[check]]
id        = "unit"
run       = "pnpm -r test -- --reporter=junit --outputFile.junit=$AVER_OUT/unit.xml"
min_tests = 1083
paths     = ["packages/**", "pnpm-lock.yaml"]

[[check]]
id     = "lint"
run    = "pnpm lint"
report = "exit"

[[check]]
id          = "rust"
run         = "cargo test --workspace 2>&1"
report      = "libtest"
min_tests   = 900
serial      = true
timeout_s   = 1800
prepare     = "cargo build --tests"

[deps]
keep    = ["packages/*/node_modules", "node_modules"]
inputs  = ["pnpm-lock.yaml", "**/package.json"]
install = "pnpm install --frozen-lockfile"

[judge]
protected = ["migrations/**"]

[ci]
review = "neutral"

[agents]
read_only = ["Explore"]
"#,
        )
        .unwrap();
        assert_eq!(parsed.shape, Shape::Contract);
        let c = &parsed.contract;
        let unit = check(c, "unit");
        assert_eq!(unit.min_tests().map(NonZeroU32::get), Some(1083));
        assert!(matches!(
            unit.reading,
            Reading::Tests {
                report: ReportSpec::Junit,
                ..
            }
        ));
        assert_eq!(
            globs(unit.paths.as_ref().unwrap()),
            ["packages/**", "pnpm-lock.yaml"]
        );
        assert!(!unit.serial);
        assert_eq!(check(c, "lint").reading, Reading::Exit);
        let rust = check(c, "rust");
        assert!(matches!(
            rust.reading,
            Reading::Tests {
                report: ReportSpec::Stdout(TextFormat::Libtest),
                ..
            }
        ));
        assert!(rust.serial);
        assert_eq!(
            globs(&c.deps.keep),
            ["node_modules", "packages/*/node_modules"]
        );
        assert_eq!(c.judge.built_tests, JudgeGlobs::default_built_tests());
        assert_eq!(globs(&c.judge.protected), ["migrations/**"]);
        assert_eq!(c.judge.sensitive, JudgeGlobs::default_sensitive());
        assert_eq!(c.ci.review, ReviewPolicy::Neutral);
        assert!(!c.ci.require_recorded_base);
    }

    #[test]
    fn the_contract_shape_is_strict() {
        let one = "[[check]]\nid = \"unit\"\nrun = \"npm test\"\nmin_tests = 3\n";
        assert!(parse(one).is_ok());
        let cases = [
            ("", "1 to 32 checks"),
            (
                "[[check]]\nid = \"unit\"\nrun = \"npm test\"\n",
                "min_tests is required",
            ),
            (
                &format!("{one}[deps]\ninstall = \"npm ci\"\n"),
                "needs inputs",
            ),
            (
                &format!("{one}[workspace]\nwarm = [\"target\"]\n"),
                "unknown field",
            ),
            (&format!("{one}imagine = 1\n"), "unknown field"),
            (
                &format!("{one}[[check]]\nid = \"unit\"\nrun = \"x\"\nmin_tests = 1\n"),
                "share the key",
            ),
            (
                &format!("{one}[judge]\nbuilt_tests = [\"../x\"]\n"),
                "path glob",
            ),
        ];
        for (text, needle) in cases {
            let err = parse(text).unwrap_err().to_string();
            assert!(err.contains(needle), "{text:?}: {err}");
        }
        assert!(matches!(parse("[[check]\n"), Err(ContractError::Syntax(_))));
        let mixed = format!("{one}[[check]]\nid = \"b\"\nrun = [\"x\"]\n");
        assert!(
            parse(&mixed).is_err(),
            "a string run in an environment-shaped file"
        );
        let empty_built = parse(&format!("{one}[judge]\nbuilt_tests = []\n")).unwrap();
        assert!(
            empty_built.contract.judge.built_tests.is_empty(),
            "an explicit empty list turns them off"
        );
    }

    /// `sensitive` defaults to the CI workflows and actions; a list replaces
    /// the default and `[]` turns it off. A legacy file gets the default.
    #[test]
    fn sensitive_paths_default_to_ci_and_a_list_replaces_them() {
        let one = "[[check]]\nid = \"unit\"\nrun = \"npm test\"\nmin_tests = 3\n";
        let read = |text: &str| -> Vec<String> {
            let parsed = parse(text).unwrap();
            globs(&parsed.contract.judge.sensitive)
                .into_iter()
                .map(str::to_owned)
                .collect()
        };
        assert_eq!(read(one), [".github/actions/**", ".github/workflows/**"]);
        assert_eq!(
            read(&format!("{one}[judge]\nsensitive = [\"deploy/**\"]\n")),
            ["deploy/**"]
        );
        assert!(read(&format!("{one}[judge]\nsensitive = []\n")).is_empty());
        let legacy = "[[check]]\nid = \"unit\"\nrun = [\"npm\", \"test\"]\n";
        assert_eq!(read(legacy), [".github/actions/**", ".github/workflows/**"]);
        let err = parse(&format!("{one}[judge]\nsensitive = [\"../x\"]\n"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("path glob"), "{err}");
    }
}
