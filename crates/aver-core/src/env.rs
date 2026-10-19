//! Environment (ARCHITECTURE.md §1.4): the pinned execution environment.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::id::{kind, BlobId};
use crate::object::{sealed, Object, Versioned};
use crate::scalar::{CheckId, Digest, EnvVarName, Path, PathGlob, ReportPath};
use crate::set::{Set, SetItem};
use crate::ParseError;

/// Where a build cache's artifacts are compiled, which decides what they
/// hold (§2.4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BuildPlatform {
    /// A sandbox image, by digest, on a platform.
    ImageDigest { digest: Digest, platform: Platform },
    /// The host's own toolchain (native runs): a description of the OS, the
    /// CPU and the compiler that changes whenever any of them does.
    HostProfile { profile: String },
}

/// The identity of a build cache (§1.4, §2.4.2): what was compiled, where,
/// and by which command. Derived, never stored; [`BuildCacheKey::hex`] is
/// the only way to name a cache, so the key is computed here alone
/// (CLAUDE.md rule 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildCacheKey {
    pub platform: BuildPlatform,
    /// The lockfiles the build resolved against.
    pub lockfiles: Vec<BlobId>,
    /// Every workspace manifest, as (path relative to `root`, blob).
    pub manifests: Vec<(String, BlobId)>,
    /// The workspace root (the lockfile's directory), `""` at the
    /// repository root: build scripts record absolute paths below it.
    pub root: String,
    pub argv: Vec<String>,
    pub setup: Vec<Vec<String>>,
    pub cwd: Option<Path>,
    pub vars: BTreeMap<EnvVarName, String>,
}

impl BuildCacheKey {
    /// The key of `check`'s build cache in `env` on `platform`.
    pub fn of_check(
        env: &Environment,
        check: &CheckDef,
        platform: BuildPlatform,
        lockfiles: Vec<BlobId>,
        manifests: Vec<(String, BlobId)>,
        root: &str,
    ) -> Self {
        Self {
            platform,
            lockfiles,
            manifests,
            root: root.to_owned(),
            argv: check.argv.clone(),
            setup: check.setup.clone(),
            cwd: check.cwd.clone(),
            vars: env.vars.clone(),
        }
    }

    /// 64 lowercase hex digits: BLAKE3 over `"aver:buildcache\0"` and the
    /// canonical JSON of every field (lockfiles and manifests as sorted
    /// sets) and the cache's protocol version.
    pub fn hex(&self) -> String {
        let mut lockfiles: Vec<String> = self.lockfiles.iter().map(ToString::to_string).collect();
        lockfiles.sort();
        lockfiles.dedup();
        let mut manifests: Vec<(&str, String)> = self
            .manifests
            .iter()
            .map(|(p, b)| (p.as_str(), b.to_string()))
            .collect();
        manifests.sort();
        manifests.dedup();
        let identity = serde_json::json!({
            "argv": self.argv,
            "cwd": self.cwd,
            "lockfiles": lockfiles,
            "manifests": manifests,
            "platform": self.platform,
            "protocol": 1,
            "root": self.root,
            "setup": self.setup,
            "vars": self.vars,
        });
        let body = crate::canonical::to_canonical(&identity)
            .expect("the build cache identity holds only strings and integers");
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"aver:buildcache\0");
        hasher.update(&body);
        hasher.finalize().to_hex().to_string()
    }
}

/// Most test path patterns an environment may declare.
pub const MAX_TEST_GLOBS: usize = 64;

/// Most setup commands a check may declare.
pub const MAX_SETUP_STEPS: usize = 8;

/// Most dependency layers an environment declares besides its own lockfile.
pub const MAX_LAYERS: usize = 16;

/// The environment schema versions this crate reads. v1 adds `tests`; v2
/// adds `install.rebuild`, check `setup` commands and the `jest-json` and
/// `mocha-xunit` report formats; v3 adds dependency `layers` of any
/// ecosystem, a check's own `image`, `deps` and `covers`, and the
/// `libtest`, `go-json`, `pytest-junit`, `tsc` and `tap` report formats; v4 adds a
/// check's `min_tests` and `approved` (§1.13). An environment carries the
/// oldest version that can express it (v4 when it uses either v4 field,
/// else v3 when it uses any v3 feature, else v2 when it uses any v2
/// feature, else v1 when it declares test paths, else v0), so every older
/// environment keeps its bytes and id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EnvSchema {
    #[serde(rename = "aver.env/v0")]
    V0,
    #[serde(rename = "aver.env/v1")]
    V1,
    #[serde(rename = "aver.env/v2")]
    V2,
    #[serde(rename = "aver.env/v3")]
    V3,
    #[serde(rename = "aver.env/v4")]
    V4,
}

impl EnvSchema {
    /// The version an environment with these fields must carry.
    pub fn required(
        tests: &Set<PathGlob>,
        install: &Install,
        checks: &Set<CheckDef>,
        layers: &Set<Layer>,
    ) -> Self {
        if checks.iter().any(|c| c.min_tests.is_some() || !c.approved) {
            return Self::V4;
        }
        let v3 = !layers.is_empty()
            || checks.iter().any(|c| {
                c.image.is_some()
                    || c.deps.is_some()
                    || !c.covers.is_empty()
                    || c.report.as_ref().is_some_and(|r| r.format.is_v3())
            });
        if v3 {
            return Self::V3;
        }
        let v2 = install.rebuild
            || checks.iter().any(|c| {
                !c.setup.is_empty()
                    || c.report
                        .as_ref()
                        .is_some_and(|r| r.format != ReportFormat::Junit)
            });
        if v2 {
            Self::V2
        } else if tests.is_empty() {
            Self::V0
        } else {
            Self::V1
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "EnvFields")]
pub struct Environment {
    pub schema: EnvSchema,
    pub platform: Platform,
    pub image: Image,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lockfile: Option<Lockfile>,
    pub install: Install,
    pub checks: Set<CheckDef>,
    pub limits: Limits,
    pub vars: BTreeMap<EnvVarName, String>,
    /// The project's test files, as declared in `aver.toml` (v1). Empty means
    /// the default patterns (§1.4.1); verification reads them through
    /// [`Environment::test_globs`].
    #[serde(default, skip_serializing_if = "Set::is_empty")]
    pub tests: Set<PathGlob>,
    /// v3: dependency layers besides the one of `lockfile`, by lockfile path
    /// (§2.4.2). A check mounts the layers its `deps` name.
    #[serde(default, skip_serializing_if = "Set::is_empty")]
    pub layers: Set<Layer>,
}

/// The wire fields of [`Environment`], before the version rule is checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvFields {
    schema: EnvSchema,
    platform: Platform,
    image: Image,
    #[serde(default)]
    lockfile: Option<Lockfile>,
    install: Install,
    checks: Set<CheckDef>,
    limits: Limits,
    vars: BTreeMap<EnvVarName, String>,
    #[serde(default)]
    tests: Set<PathGlob>,
    #[serde(default)]
    layers: Set<Layer>,
}

impl TryFrom<EnvFields> for Environment {
    type Error = ParseError;
    fn try_from(f: EnvFields) -> Result<Self, ParseError> {
        if f.schema != EnvSchema::required(&f.tests, &f.install, &f.checks, &f.layers) {
            return Err(ParseError::invalid(
                "environment",
                "an environment carries aver.env/v4 exactly when a check has min_tests or is not approved, else aver.env/v3 exactly when it has layers, a check with its own image, deps or covers, or a libtest, go-json, pytest-junit, tsc or tap report, else v2 exactly when it rebuilds dependencies, has a check with setup commands or a non-JUnit report, else v1 exactly when it declares tests, else v0",
            ));
        }
        validate_layers(f.lockfile.as_ref(), &f.layers, &f.checks)?;
        for check in f.checks.iter() {
            match (check.min_tests, &check.report) {
                (Some(_), None) => {
                    return Err(ParseError::invalid(
                        "environment",
                        format!("check {}: an exit-only check has no min_tests", check.id),
                    ))
                }
                (Some(n), Some(_)) if n == NonZeroU32::MIN => {
                    return Err(ParseError::invalid(
                        "environment",
                        format!(
                            "check {}: min_tests 1 is the default and is omitted",
                            check.id
                        ),
                    ))
                }
                _ => {}
            }
            if check.setup.len() > MAX_SETUP_STEPS || check.setup.iter().any(Vec::is_empty) {
                return Err(ParseError::invalid(
                    "environment",
                    format!("a check has at most {MAX_SETUP_STEPS} setup commands, none empty"),
                ));
            }
        }
        if f.tests.len() > MAX_TEST_GLOBS {
            return Err(ParseError::invalid(
                "environment",
                format!("at most {MAX_TEST_GLOBS} tests patterns"),
            ));
        }
        Ok(Self {
            schema: f.schema,
            platform: f.platform,
            image: f.image,
            lockfile: f.lockfile,
            install: f.install,
            checks: f.checks,
            limits: f.limits,
            vars: f.vars,
            tests: f.tests,
            layers: f.layers,
        })
    }
}

/// The layer rules of v3: at most [`MAX_LAYERS`], lockfile paths distinct
/// from the environment's own, `rebuild` only for npm, and each check's
/// `deps` naming existing layers and differing from the default it would
/// otherwise have, so every environment has one representation.
fn validate_layers(
    lockfile: Option<&Lockfile>,
    layers: &Set<Layer>,
    checks: &Set<CheckDef>,
) -> Result<(), ParseError> {
    let invalid = |reason: String| Err(ParseError::invalid("environment", reason));
    if layers.len() > MAX_LAYERS {
        return invalid(format!("at most {MAX_LAYERS} layers"));
    }
    for layer in layers.iter() {
        if lockfile.is_some_and(|l| l.path == layer.lockfile.path) {
            return invalid(format!(
                "layer {} repeats the environment's lockfile",
                layer.lockfile.path
            ));
        }
        if layer.install.rebuild && layer.kind != LayerKind::Npm {
            return invalid(format!(
                "layer {}: only npm layers rebuild",
                layer.lockfile.path
            ));
        }
        if layer.install.argv.is_empty() {
            return invalid(format!("layer {}: empty install", layer.lockfile.path));
        }
    }
    let default: Vec<&str> = lockfile.iter().map(|l| l.path.as_str()).collect();
    for check in checks.iter() {
        let Some(deps) = &check.deps else { continue };
        let named: Vec<&str> = deps.iter().map(Path::as_str).collect();
        if named == default {
            return invalid(format!(
                "check {}: deps repeats the default (the environment's lockfile) and is omitted instead",
                check.id
            ));
        }
        for path in deps.iter() {
            let known =
                lockfile.is_some_and(|l| l.path == *path) || layers.get(path.as_str()).is_some();
            if !known {
                return invalid(format!("check {}: deps names no layer {path}", check.id));
            }
        }
    }
    Ok(())
}

/// The oldest environment version; v1 and v2 are also read.
impl Versioned for Environment {
    const SCHEMA: &'static str = "aver.env/v0";
}
impl sealed::Sealed for Environment {}
impl Object for Environment {
    type Kind = kind::Env;
}

/// Test file patterns a v3 environment that declares none adds to
/// [`DEFAULT_TEST_GLOBS`]: pytest's `test_*.py` and Rust's `tests.rs`
/// modules. Go's `*_test.go`, Python's `*_test.py` and Rust's `tests/`
/// directories are default patterns already. Rust's `#[cfg(test)]` items
/// inside source files are not test files (a file is one or not as a
/// whole); verification judges them on their own (§2.5.2).
pub const ECOSYSTEM_TEST_GLOBS: &[&str] = &["**/test_*.py", "**/tests.rs"];

/// Test file patterns used when an environment declares none: Node's own
/// test discovery patterns, the usual test directories at any depth, and
/// (round 13a) what tests read or run that lives outside them: Django's
/// `tests.py`, pytest's `conftest.py` anywhere, snapshot, golden, fixture
/// and `testdata` files, Jest's mocks, and Jest and Vitest setup files.
pub const DEFAULT_TEST_GLOBS: &[&str] = &[
    "**/*.test.*",
    "**/*.spec.*",
    "**/*-test.*",
    "**/*_test.*",
    "**/test-*.*",
    "**/test.*",
    "**/test",
    "**/tests",
    "**/__tests__",
    "**/tests.py",
    "**/conftest.py",
    "**/__snapshots__",
    "**/*.snap",
    "**/golden*",
    "**/fixtures",
    "**/__fixtures__",
    "**/__mocks__",
    "**/testdata",
    "**/jest.setup.*",
    "**/setupTests.*",
    "**/vitest.setup.*",
];

impl Environment {
    pub const SCHEMA_V1: &'static str = "aver.env/v1";
    pub const SCHEMA_V2: &'static str = "aver.env/v2";
    pub const SCHEMA_V3: &'static str = "aver.env/v3";
    pub const SCHEMA_V4: &'static str = "aver.env/v4";

    /// Whether a network-less rebuild of the dependency layer runs the
    /// dependencies' install scripts after a script-free install (§2.4.2).
    /// An install that already ran scripts has nothing left to rebuild.
    pub fn rebuilds(&self) -> bool {
        self.install.rebuild && !self.install.allow_scripts
    }

    /// The patterns that name the project's test files: the declared ones,
    /// else [`DEFAULT_TEST_GLOBS`], plus [`ECOSYSTEM_TEST_GLOBS`] in v3.
    pub fn test_globs(&self) -> Vec<PathGlob> {
        if self.tests.is_empty() {
            let extra = if self.schema >= EnvSchema::V3 {
                ECOSYSTEM_TEST_GLOBS
            } else {
                &[]
            };
            DEFAULT_TEST_GLOBS
                .iter()
                .chain(extra)
                .map(|g| g.parse().expect("default globs are valid"))
                .collect()
        } else {
            self.tests.iter().cloned().collect()
        }
    }

    /// Whether `path` is a test file of this project.
    pub fn is_test_file(&self, path: &str) -> bool {
        self.test_globs().iter().any(|g| g.covers(path))
    }

    /// The `DepsKey` of this environment's dependency layer (§1.4), as 64
    /// lowercase hex digits, or `None` without a lockfile: BLAKE3 over
    /// `"aver:deps\0"` and the canonical JSON of the image digest, install
    /// config, lockfile blob and platform. The lockfile's path is not part of it.
    ///
    /// `manifest` is the blob of the `package.json` streamed into the install
    /// beside the lockfile (`None` if absent). With `allow_scripts` the
    /// install runs that manifest's lifecycle scripts, so it shapes the layer
    /// and joins the identity as `"manifest"` (a blob id or `null`); without
    /// scripts it is ignored.
    pub fn deps_key(&self, manifest: Option<&crate::BlobId>) -> Option<String> {
        let layer = self.primary_layer()?;
        let inputs: Vec<(String, crate::BlobId)> = manifest
            .map(|m| ("package.json".to_owned(), *m))
            .into_iter()
            .collect();
        Some(self.layer_key(&layer, &self.image, &inputs))
    }

    /// The `DepsKey` of `layer` installed in `image` from `inputs`, the
    /// install's files besides the lockfile, as (path relative to the
    /// lockfile's directory, blob) pairs (§1.4). An npm layer's key is the
    /// key of v0 to v2, which reads only the `package.json` and only when
    /// scripts run; any other kind's key also carries `"kind"` and every
    /// input, since its install reads them (Cargo's workspace manifests,
    /// `pyproject.toml`, `go.mod`).
    pub fn layer_key(
        &self,
        layer: &Layer,
        image: &Image,
        inputs: &[(String, crate::BlobId)],
    ) -> String {
        let mut identity = serde_json::json!({
            "image": image.digest,
            "install": layer.install,
            "lockfile": layer.lockfile.blob,
            "platform": self.platform,
        });
        if layer.kind == LayerKind::Npm {
            if layer.install.allow_scripts {
                let manifest = inputs
                    .iter()
                    .find(|(p, _)| p == "package.json")
                    .map(|(_, b)| b);
                identity["manifest"] = serde_json::json!(manifest);
            }
        } else {
            let mut inputs: Vec<(&str, String)> = inputs
                .iter()
                .map(|(p, b)| (p.as_str(), b.to_string()))
                .collect();
            inputs.sort();
            inputs.dedup();
            identity["kind"] = serde_json::json!(layer.kind);
            identity["inputs"] = serde_json::json!(inputs);
        }
        let body = crate::canonical::to_canonical(&identity)
            .expect("the deps identity holds only strings and bools");
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"aver:deps\0");
        hasher.update(&body);
        hasher.finalize().to_hex().to_string()
    }

    /// The layer of the environment's own `lockfile` (npm, with the
    /// environment's install), if it has one.
    pub fn primary_layer(&self) -> Option<Layer> {
        Some(Layer {
            kind: LayerKind::Npm,
            lockfile: self.lockfile.clone()?,
            install: self.install.clone(),
        })
    }

    /// Every dependency layer: the environment's own, then `layers`.
    pub fn all_layers(&self) -> Vec<Layer> {
        self.primary_layer()
            .into_iter()
            .chain(self.layers.iter().cloned())
            .collect()
    }

    /// The layers `check` mounts: those its `deps` name, else the
    /// environment's own.
    pub fn check_layers(&self, check: &CheckDef) -> Vec<Layer> {
        match &check.deps {
            None => self.primary_layer().into_iter().collect(),
            Some(deps) => self
                .all_layers()
                .into_iter()
                .filter(|l| deps.get(l.lockfile.path.as_str()).is_some())
                .collect(),
        }
    }

    /// The image `check` runs in: its own, else the environment's.
    pub fn check_image<'a>(&'a self, check: &'a CheckDef) -> &'a Image {
        check.image.as_ref().unwrap_or(&self.image)
    }
}

/// What a dependency layer installs, which decides how the install runs and
/// how a check sees it (§2.4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerKind {
    /// `node_modules`, linked beside the lockfile.
    Npm,
    /// A virtualenv from `uv.lock` (`uv sync`), on `PATH`.
    Uv,
    /// A virtualenv from requirements files (`pip install -r`), on `PATH`.
    Pip,
    /// A `CARGO_HOME` with the registry sources `Cargo.lock` pins.
    Cargo,
    /// A `GOMODCACHE` with the modules `go.sum` pins.
    Go,
}

impl LayerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Uv => "uv",
            Self::Pip => "pip",
            Self::Cargo => "cargo",
            Self::Go => "go",
        }
    }
}

/// v3: one dependency layer: the packages a lockfile pins, installed with
/// network into a volume per image (§2.4.2), keyed by
/// [`Environment::layer_key`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub kind: LayerKind,
    pub lockfile: Lockfile,
    pub install: Install,
}

impl SetItem for Layer {
    type Key = str;
    fn key(&self) -> &str {
        self.lockfile.path.as_str()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Platform {
    #[serde(rename = "linux/arm64")]
    LinuxArm64,
    #[serde(rename = "linux/amd64")]
    LinuxAmd64,
}

/// The digest is authoritative; `reference` is informational.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    #[serde(rename = "ref")]
    pub reference: String,
    pub digest: Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lockfile {
    pub path: Path,
    pub blob: BlobId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Install {
    pub argv: Vec<String>,
    pub allow_scripts: bool,
    /// v2: after a script-free install, run the dependencies' install
    /// scripts in a network-less sandbox (§2.4.2). Omitted when false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub rebuild: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckDef {
    pub id: CheckId,
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<Path>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<Report>,
    /// v2: commands run in order in the check's sandbox before `argv`, each
    /// an argv. Every one runs even if an earlier one failed, and so does
    /// `argv`; the run fails if any of them failed (§2.4.1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setup: Vec<Vec<String>>,
    /// v3: the image the check runs in instead of the environment's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<Image>,
    /// v3: the lockfile paths of the dependency layers the check mounts.
    /// Absent: the environment's own lockfile layer, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deps: Option<Set<Path>>,
    /// v3: the files the check exercises, for the uncovered-change rule
    /// (§1.9.3). Empty: inferred from its image and `cwd`.
    #[serde(default, skip_serializing_if = "Set::is_empty")]
    pub covers: Set<PathGlob>,
    /// v4: the floor on how many tests the base must run, from a contract's
    /// `min_tests` (§1.13). Absent: 1 for a check with a report; an
    /// exit-only check has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_tests: Option<NonZeroU32>,
    /// v4: whether a person approved the command: it comes from a committed
    /// `aver.toml`, a signed task or, in zero-config CI, a script the
    /// repository's `package.json` declares. `false` for the commands Aver
    /// guesses and a head's proposed contract. An exit-only check is
    /// evidence only when approved (§1.9.7); a check with a report is
    /// counted either way. Written only when false.
    #[serde(default = "approved", skip_serializing_if = "is_approved")]
    pub approved: bool,
}

fn approved() -> bool {
    true
}

fn is_approved(b: &bool) -> bool {
    *b
}

impl CheckDef {
    /// A check running `argv` at the repository root, with nothing else set.
    pub fn new(id: CheckId, argv: Vec<String>) -> Self {
        Self {
            id,
            argv,
            cwd: None,
            timeout_ms: None,
            report: None,
            setup: Vec::new(),
            image: None,
            deps: None,
            covers: Set::empty(),
            min_tests: None,
            approved: true,
        }
    }

    /// The floor on the base's test count: `None` for an exit-only check.
    pub fn floor(&self) -> Option<NonZeroU32> {
        self.report
            .as_ref()
            .map(|_| self.min_tests.unwrap_or(NonZeroU32::MIN))
    }
}

impl SetItem for CheckDef {
    type Key = str;
    fn key(&self) -> &str {
        self.id.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub format: ReportFormat,
    pub path: ReportPath,
}

/// How a check's report is read (§1.9.1). `jest-json` and `mocha-xunit`
/// are v2; `libtest`, `go-json`, `pytest-junit`, `tsc` and `tap` are v3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReportFormat {
    /// JUnit XML: test path from the enclosing `<testsuite>` names.
    #[serde(rename = "junit")]
    Junit,
    /// Jest's `--json` output: test path from the file and ancestor titles.
    #[serde(rename = "jest-json")]
    JestJson,
    /// Mocha's `xunit` reporter: JUnit-like XML whose `classname` holds the
    /// suite titles.
    #[serde(rename = "mocha-xunit")]
    MochaXunit,
    /// The text `cargo test` prints: `test <name> ... ok|FAILED|ignored`
    /// lines under each test binary's `Running` or `Doc-tests` line, and
    /// the failure sections.
    #[serde(rename = "libtest")]
    Libtest,
    /// The JSON events `go test -json` writes.
    #[serde(rename = "go-json")]
    GoJson,
    /// JUnit XML from `pytest --junitxml`: test path from the `file`
    /// attribute (or the module in `classname`), the classes in
    /// `classname`, and the name.
    #[serde(rename = "pytest-junit")]
    PytestJunit,
    /// The diagnostics `tsc` prints (`file(line,col): error TS1234: …`):
    /// one failing entry per type error, keyed by file, code and message
    /// (not the line, which moves when code above it changes). The report
    /// lists failures only, so an error a side does not print passes there.
    #[serde(rename = "tsc")]
    Tsc,
    /// The TAP stream `ava --tap` prints: one entry per test point, its
    /// path the title's file prefix parts and the title.
    #[serde(rename = "tap")]
    Tap,
    /// Cargo's JSON messages from the build probe Aver runs when restored
    /// test files stop a head's build (§1.9.3). Aver's own: no stored
    /// environment carries it, and it holds no test results.
    #[serde(rename = "cargo-probe", skip_deserializing)]
    CargoProbe,
}

impl ReportFormat {
    /// Whether the format exists only in aver.env/v3.
    pub fn is_v3(self) -> bool {
        matches!(
            self,
            Self::Libtest | Self::GoJson | Self::PytestJunit | Self::Tsc | Self::Tap
        )
    }

    /// Whether a report lists only failures, so that an entry a run's report
    /// does not name passed in that run.
    pub fn lists_failures_only(self) -> bool {
        self == Self::Tsc
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Junit => "junit",
            Self::JestJson => "jest-json",
            Self::MochaXunit => "mocha-xunit",
            Self::Libtest => "libtest",
            Self::GoJson => "go-json",
            Self::PytestJunit => "pytest-junit",
            Self::Tsc => "tsc",
            Self::Tap => "tap",
            Self::CargoProbe => "cargo-probe",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub cpu_millis: u32,
    pub memory_mb: u32,
    pub pids: u32,
    pub timeout_ms: u32,
    pub output_bytes: u32,
}

/// The host Aver runs on, which the host profile names (§1.4.3).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostTarget {
    pub os: HostOs,
    pub arch: HostArch,
    /// `process.versions.modules` of the host Node; part of the key only
    /// when the install runs lifecycle scripts (native builds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_abi: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostOs {
    Darwin,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostArch {
    Arm64,
    Amd64,
}

impl HostTarget {
    /// The host this binary was built for, without a Node ABI, or `None` on
    /// an unsupported OS or CPU.
    pub fn current() -> Option<Self> {
        let os = if cfg!(target_os = "macos") {
            HostOs::Darwin
        } else if cfg!(target_os = "linux") {
            HostOs::Linux
        } else {
            return None;
        };
        let arch = if cfg!(target_arch = "aarch64") {
            HostArch::Arm64
        } else if cfg!(target_arch = "x86_64") {
            HostArch::Amd64
        } else {
            return None;
        };
        Some(Self {
            os,
            arch,
            node_abi: None,
        })
    }
}
