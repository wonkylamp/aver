//! Environment pinning (ARCHITECTURE.md §1.4, §1.4.1, §6.5).
//!
//! [`environment`] derives the [`Environment`] of a root tree: it parses
//! `aver.toml` at the root (or uses the defaults when there is none),
//! validates every field, locates the lockfile, and pins the image to a digest.
//! An image given by tag only is resolved through an [`ImageResolver`];
//! [`DockerResolver`] asks the local Docker daemon, [`StaticResolver`] is a
//! fixed map for tests. The result depends only on the tree's contents, the
//! host platform and the resolver, never on time.
//!
//! Trees are read through [`ObjectSource`], implemented by [`Store`], [`Tx`]
//! and any in-memory staging area, so the same code serves a stored snapshot
//! and an import that has not been written yet.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::num::NonZeroU32;
use std::process::Command;
use std::sync::Arc;

use aver_core::{
    Approval, Contract, ContractCheck, EnvId, JudgeGlobs, Reading, ReportSpec, Snapshot, TextFormat,
};
use aver_core::{
    BlobId, CheckDef, CheckId, Digest, EnvSchema, EnvVarName, Environment, Image, Install, Layer,
    LayerKind, Limits, Lockfile, Node, Path, PathGlob, Platform, Report, ReportFormat, ReportPath,
    Set, Tree, TreeId,
};
use serde::Deserialize;

use crate::contract::{self, ContractError, Parsed};
use crate::store::{Store, StoreError, Tx};

pub const CONFIG_PATH: &str = "aver.toml";
pub const DEFAULT_LOCKFILE: &str = "package-lock.json";
/// Where a check writes its report, as its command spells it; the slot
/// runner rehomes it to the run's own directory.
const SANDBOX_OUT: &str = "/aver/out";
pub const DEFAULT_IMAGE_REF: &str = "node:22-bookworm-slim";
pub const DEFAULT_IMAGE_DIGEST: &str =
    "sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392";
pub const DEFAULT_INSTALL: &[&str] = &["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund"];
pub const DEFAULT_CHECK_ID: &str = "test";
pub const DEFAULT_CHECK_ARGV: &[&str] = &["npm", "test"];
pub const DEFAULT_LIMITS: Limits = Limits {
    cpu_millis: 2000,
    memory_mb: 2048,
    pids: 512,
    timeout_ms: 300_000,
    output_bytes: 65_536,
};

#[derive(Debug, thiserror::Error)]
pub enum EnvError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("aver.toml is not valid TOML: {0}")]
    Syntax(String),
    #[error("aver.toml: {0}")]
    Invalid(String),
    #[error("cannot resolve image {reference:?} to a digest: {reason}")]
    Resolve { reference: String, reason: String },
}

fn invalid(msg: impl Into<String>) -> EnvError {
    EnvError::Invalid(msg.into())
}

/// Read access to stored trees and blobs.
pub trait ObjectSource {
    fn tree(&self, id: &TreeId) -> Result<Tree, StoreError>;
    fn blob(&self, id: &BlobId) -> Result<Vec<u8>, StoreError>;
}

impl ObjectSource for Store {
    fn tree(&self, id: &TreeId) -> Result<Tree, StoreError> {
        self.get(id)
    }
    fn blob(&self, id: &BlobId) -> Result<Vec<u8>, StoreError> {
        self.get_raw(id)
    }
}

impl ObjectSource for Tx<'_> {
    fn tree(&self, id: &TreeId) -> Result<Tree, StoreError> {
        self.get(id)
    }
    fn blob(&self, id: &BlobId) -> Result<Vec<u8>, StoreError> {
        self.get_raw(id)
    }
}

/// The node at `path` under `root`, or `None` if no such entry exists.
pub fn lookup(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
    path: &Path,
) -> Result<Option<Node>, StoreError> {
    let mut current = Node::Tree(*root);
    for segment in path.segments() {
        let Node::Tree(dir) = current else {
            return Ok(None);
        };
        match src.tree(&dir)?.entries.get(segment) {
            Some(entry) => current = entry.node,
            None => return Ok(None),
        }
    }
    Ok(Some(current))
}

/// The contents of the regular or executable file at `path`, or `None` if
/// there is no entry there or it is a symlink or directory.
pub fn read_file(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
    path: &Path,
) -> Result<Option<Vec<u8>>, StoreError> {
    match lookup(src, root, path)? {
        Some(Node::File(blob) | Node::Exec(blob)) => src.blob(&blob).map(Some),
        _ => Ok(None),
    }
}

/// The platform sandboxes run on for this host.
pub fn host_platform() -> Platform {
    if cfg!(target_arch = "x86_64") {
        Platform::LinuxAmd64
    } else {
        Platform::LinuxArm64
    }
}

/// Pins an image reference that has no digest.
pub trait ImageResolver: Send + Sync {
    /// The digest `reference` currently names for `platform`, or why it cannot
    /// be resolved.
    fn resolve(&self, reference: &str, platform: Platform) -> Result<Digest, String>;
}

/// A fixed reference-to-digest map.
#[derive(Debug, Clone, Default)]
pub struct StaticResolver(pub HashMap<String, Digest>);

impl StaticResolver {
    pub fn with(mut self, reference: &str, digest: Digest) -> Self {
        self.0.insert(reference.to_owned(), digest);
        self
    }
}

impl ImageResolver for StaticResolver {
    fn resolve(&self, reference: &str, _: Platform) -> Result<Digest, String> {
        self.0
            .get(reference)
            .cloned()
            .ok_or_else(|| "not in the resolver's map".to_owned())
    }
}

/// The toolchain an image provides, read from its repository name, which
/// decides the layers a check mounts by default and the files it covers
/// (§1.4.1). `None` for any other image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Node,
    Python,
    Rust,
    Go,
}

impl Family {
    /// The family of `image`, from the last segment of its repository name.
    pub fn of(image: &Image) -> Option<Self> {
        let repo = normalize_repo(image_name(&image.reference));
        match repo.rsplit('/').next().unwrap_or(repo) {
            "node" => Some(Self::Node),
            "python" => Some(Self::Python),
            "rust" => Some(Self::Rust),
            "golang" => Some(Self::Go),
            _ => None,
        }
    }

    /// Whether a check of this family mounts layers of `kind`.
    pub fn mounts(self, kind: LayerKind) -> bool {
        matches!(
            (self, kind),
            (Self::Node, LayerKind::Npm)
                | (Self::Python, LayerKind::Uv | LayerKind::Pip)
                | (Self::Rust, LayerKind::Cargo)
                | (Self::Go, LayerKind::Go)
        )
    }
}

/// Pins an image given by tag only to the repository digest of a local
/// image of that name, as `docker image inspect` reports it. It never
/// pulls: since 0.4 nothing runs in the image, which only keys the
/// environment until WP1a's snapshot leaves it out. An image given by
/// digest needs no resolver. No test covers it: its one test needed a local
/// Docker image and went with the 0.4 redesign; delete it with the image.
#[derive(Debug, Clone, Copy, Default)]
pub struct DockerResolver;

impl ImageResolver for DockerResolver {
    fn resolve(&self, reference: &str, _platform: Platform) -> Result<Digest, String> {
        docker_repo_digest(reference)?.ok_or_else(|| {
            "the image is not present locally; give it by digest (name@sha256:…) in aver.toml"
                .to_owned()
        })
    }
}

/// `Ok(None)` when the image is not present locally.
fn docker_repo_digest(reference: &str) -> Result<Option<Digest>, String> {
    let out = Command::new("docker")
        .args([
            "image",
            "inspect",
            "--format",
            "{{json .RepoDigests}}",
            "--",
        ])
        .arg(reference)
        .output()
        .map_err(|e| format!("cannot run docker: {e}"))?;
    if !out.status.success() {
        return Ok(None);
    }
    let digests: Vec<String> = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("unexpected docker inspect output: {e}"))?;
    let repo = normalize_repo(image_name(reference));
    let chosen = digests
        .iter()
        .filter_map(|d| d.split_once('@'))
        .find(|(name, _)| normalize_repo(name) == repo)
        .or_else(|| match digests.as_slice() {
            [only] => only.split_once('@'),
            _ => None,
        });
    match chosen {
        Some((_, digest)) => digest
            .parse::<Digest>()
            .map(Some)
            .map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

/// The repository part of `name[:tag]`; a `:` after the last `/` starts the tag.
fn image_name(reference: &str) -> &str {
    let last_segment = reference.rfind('/').map_or(0, |i| i + 1);
    match reference[last_segment..].find(':') {
        Some(i) => &reference[..last_segment + i],
        None => reference,
    }
}

fn normalize_repo(name: &str) -> &str {
    let name = name.strip_prefix("docker.io/").unwrap_or(name);
    name.strip_prefix("library/").unwrap_or(name)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    env: EnvSection,
    #[serde(default)]
    limits: LimitsSection,
    check: Option<Vec<CheckSection>>,
    deps: Option<Vec<DepsSection>>,
    #[serde(default)]
    vars: BTreeMap<String, String>,
    /// Unit declarations (§1.12.1) are design-only in v0 and ignored here.
    #[serde(default, rename = "unit")]
    _unit: Option<toml::Value>,
    /// Disk workspace settings (§1.4.1 `[workspace]`), read by materialize;
    /// they change no environment id.
    #[serde(default, rename = "workspace")]
    _workspace: Option<toml::Value>,
    /// Claude Code settings (`[agents]`): the stop hook reads `stop` from
    /// the base's `aver.toml` itself (`run::hooks::StopMode`), and the
    /// fields hooks before 0.4 read still parse. No hashed object holds it.
    #[serde(default, rename = "agents")]
    _agents: Option<toml::Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvSection {
    image: Option<String>,
    lockfile: Option<String>,
    install: Option<Vec<String>>,
    allow_scripts: Option<bool>,
    rebuild: Option<bool>,
    tests: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitsSection {
    cpus: Option<f64>,
    memory_mb: Option<u32>,
    pids: Option<u32>,
    timeout_s: Option<u32>,
    output_bytes: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckSection {
    id: String,
    run: Vec<String>,
    cwd: Option<String>,
    timeout_s: Option<u32>,
    report: Option<ReportSection>,
    setup: Option<Vec<Vec<String>>>,
    image: Option<String>,
    deps: Option<Vec<String>>,
    covers: Option<Vec<String>>,
    /// Set from a contract's `min_tests`, never read from the file.
    #[serde(skip)]
    min_tests: Option<NonZeroU32>,
}

/// `[[deps]]`: one more dependency layer (§1.4.1).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DepsSection {
    lockfile: String,
    kind: Option<String>,
    install: Option<Vec<String>>,
    allow_scripts: Option<bool>,
    rebuild: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportSection {
    format: String,
    path: String,
}

/// Whether a tree whose `aver.toml` names no check gets the checks Aver
/// detects (§1.4.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Detection {
    /// No checks: the repository names its test commands in `aver.toml`,
    /// which `aver check -- <cmd>` writes once.
    #[default]
    Off,
    /// `aver ci`'s zero-config mode: the default `npm test` check, its
    /// type-check gate and the generic-mode presets, approved only where
    /// they run a script the repository's `package.json` declares.
    Presets,
}

/// The workspace service's environment resolver (§2.3): re-derives a
/// snapshot's environment from its tree with [`environment`].
pub struct TreeEnv(pub Arc<dyn ImageResolver>, pub Detection);

impl crate::workspace::EnvResolver for TreeEnv {
    fn resolve(
        &self,
        store: &Store,
        root: &TreeId,
        _base: &Environment,
    ) -> Result<Environment, Box<dyn std::error::Error + Send + Sync>> {
        Ok(environment(store, root, &*self.0, self.1)?)
    }
}

/// The pinned environment of the snapshot rooted at `root`.
pub fn environment(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
    resolver: &dyn ImageResolver,
    detection: Detection,
) -> Result<Environment, EnvError> {
    let config_path: Path = CONFIG_PATH.parse().expect("valid path");
    match lookup(src, root, &config_path)? {
        None => derive(src, root, ConfigFile::default(), resolver, detection),
        Some(Node::File(blob) | Node::Exec(blob)) => {
            environment_with_config(src, root, &src.blob(&blob)?, resolver, detection)
        }
        Some(_) => Err(invalid("must be a regular file")),
    }
}

/// The environment of the tree rooted at `root` as if its `aver.toml` were
/// `config` (empty: as if it had none), whatever the tree holds there:
/// `aver init` checks a draft with the parser that will read it.
///
/// The file is read through [`contract::parse`]. A v0 to v3 environment
/// file keeps its own reading (and so its environment id), and the
/// contract reader then checks it too; a contract is turned into the
/// checks and layers it means. A file that declares no check (only
/// `[workspace]` or `[agents]`, say) has none.
pub fn environment_with_config(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
    config: &[u8],
    resolver: &dyn ImageResolver,
    detection: Detection,
) -> Result<Environment, EnvError> {
    let text = std::str::from_utf8(config).map_err(|_| invalid("is not valid UTF-8"))?;
    let table: toml::Table =
        toml::from_str(text).map_err(|e| EnvError::Syntax(e.message().to_owned()))?;
    let declares = table
        .get("check")
        .and_then(toml::Value::as_array)
        .is_some_and(|checks| !checks.is_empty());
    if contract::is_legacy(&table) || !declares {
        let env = derive(src, root, parse_config(config)?, resolver, detection)?;
        return match contract::parse(text) {
            Ok(_) | Err(ContractError::NoChecks) => Ok(env),
            Err(ContractError::Invalid(_)) if !declares => Ok(env),
            Err(ContractError::Syntax(e)) => Err(EnvError::Syntax(e)),
            Err(ContractError::Invalid(e)) => Err(invalid(e)),
        };
    }
    let contract = match contract::parse(text) {
        Ok(Parsed { contract, .. }) => contract,
        Err(ContractError::Syntax(e)) => return Err(EnvError::Syntax(e)),
        Err(e) => return Err(invalid(e.to_string().trim_start_matches("aver.toml: "))),
    };
    let exists = |path: &str| {
        path.parse::<Path>().ok().is_some_and(|p| {
            matches!(
                lookup(src, root, &p),
                Ok(Some(Node::File(_) | Node::Exec(_)))
            )
        })
    };
    derive(
        src,
        root,
        contract_config(&contract, &exists),
        resolver,
        detection,
    )
}

/// The environment sections a contract means. Each check runs its line as
/// written: a single command as its words, anything else under `sh -c`; a
/// report on standard output is written to a file under `/aver/out` and
/// echoed. The image follows the command's toolchain, so the files a check
/// covers by default are its language's. Each lockfile among `deps.inputs`
/// other than the root `package-lock.json` is a layer of its kind.
fn contract_config(contract: &Contract, exists: &dyn Fn(&str) -> bool) -> ConfigFile {
    let check = contract.checks.iter().map(contract_check).collect();
    let deps: Vec<DepsSection> = contract
        .deps
        .inputs
        .iter()
        .map(|g| g.as_str())
        .filter(|p| {
            !p.contains(['*', '?', '[', '{'])
                && *p != DEFAULT_LOCKFILE
                && kind_of_lockfile(p.rsplit('/').next().unwrap_or(p)).is_some()
                && exists(p)
        })
        .map(|p| DepsSection {
            lockfile: p.to_owned(),
            kind: None,
            install: None,
            allow_scripts: None,
            rebuild: None,
        })
        .collect();
    let tests = &contract.judge.tests;
    ConfigFile {
        env: EnvSection {
            tests: (!tests.is_empty())
                .then(|| tests.iter().map(|g| g.as_str().to_owned()).collect()),
            ..EnvSection::default()
        },
        check: Some(check),
        deps: (!deps.is_empty()).then_some(deps),
        ..ConfigFile::default()
    }
}

fn contract_check(c: &ContractCheck) -> CheckSection {
    let id = c.id.as_str();
    let line = c.run.as_str();
    let out = |ext: &str| format!("{SANDBOX_OUT}/{id}.{ext}");
    let (run, report) = match &c.reading {
        Reading::Exit => (argv_of(line), None),
        Reading::Tests { report, .. } => match report {
            ReportSpec::Junit => {
                let at_out = with_out(line);
                let written: BTreeSet<&str> = at_out
                    .match_indices(&format!("{SANDBOX_OUT}/"))
                    .filter_map(|(at, _)| {
                        let rest = &at_out[at..];
                        let end = rest
                            .find(|ch: char| !(ch.is_ascii_alphanumeric() || "/._-".contains(ch)))
                            .unwrap_or(rest.len());
                        Some(&rest[..end]).filter(|p| p.ends_with(".xml"))
                    })
                    .collect();
                let path = match written.into_iter().collect::<Vec<_>>().as_slice() {
                    [one] => (*one).to_owned(),
                    _ => out("xml"),
                };
                let words = |w: &str| line.split_whitespace().any(|t| program_of(t) == w);
                let format = if words("pytest") || line.contains("-m pytest") {
                    "pytest-junit"
                } else if words("mocha") && line.contains("xunit") {
                    "mocha-xunit"
                } else {
                    "junit"
                };
                (argv_of(line), Some((format, path)))
            }
            ReportSpec::Stdout(format) => {
                let (name, ext, both) = text_format(*format);
                let file = out(ext);
                let script = if both {
                    format!("( {line} ) >{file} 2>&1; s=$?; cat {file}; exit $s")
                } else {
                    format!("( {line} ) >{file}; s=$?; cat {file}; exit $s")
                };
                (vec!["sh".into(), "-c".into(), script], Some((name, file)))
            }
            ReportSpec::File { format, path } => (
                argv_of(line),
                Some((text_format(*format).0, format!("{SANDBOX_OUT}/{path}"))),
            ),
        },
    };
    let image = family_of_line(line).and_then(|family| {
        let (reference, digest) = match family {
            Family::Rust => crate::presets::RUST_IMAGE,
            Family::Go => crate::presets::GO_IMAGE,
            Family::Python => crate::presets::PYTHON_IMAGE,
            Family::Node => return None,
        };
        Some(format!("{reference}@{digest}"))
    });
    CheckSection {
        id: id.to_owned(),
        run,
        cwd: c.cwd.as_ref().map(|p| p.as_str().to_owned()),
        timeout_s: c.timeout_s.map(NonZeroU32::get),
        report: report.map(|(format, path)| ReportSection {
            format: format.to_owned(),
            path,
        }),
        setup: c
            .prepare
            .as_ref()
            .map(|p| vec![vec!["sh".into(), "-c".into(), with_out(p.as_str())]]),
        image,
        deps: None,
        covers: c
            .paths
            .as_ref()
            .map(|globs| globs.iter().map(|g| g.as_str().to_owned()).collect()),
        min_tests: c.min_tests().filter(|n| n.get() > 1),
    }
}

/// The report format name, file extension and whether standard error goes
/// into the report too (compiler output that names the failing test).
fn text_format(format: TextFormat) -> (&'static str, &'static str, bool) {
    match format {
        TextFormat::Libtest => ("libtest", "txt", true),
        TextFormat::Tsc => ("tsc", "txt", true),
        TextFormat::GoJson => ("go-json", "json", false),
        TextFormat::JestJson => ("jest-json", "json", false),
        TextFormat::Tap => ("tap", "tap", false),
    }
}

/// `line` with `$AVER_OUT` as `/aver/out`, which the slot runner rehomes.
fn with_out(line: &str) -> String {
    line.replace("\"$AVER_OUT\"", SANDBOX_OUT)
        .replace("${AVER_OUT}", SANDBOX_OUT)
        .replace("$AVER_OUT", SANDBOX_OUT)
}

/// A single command as its words; anything else under `sh -c`, including
/// a command after `NAME=value` assignments, which only a shell applies.
fn argv_of(line: &str) -> Vec<String> {
    let line = with_out(line);
    match crate::adhoc::runner::split_and(&line).as_deref() {
        Some([one]) => crate::adhoc::runner::tokenize(one),
        _ => None,
    }
    .filter(|words| !words[0].contains('='))
    .unwrap_or_else(|| vec!["sh".into(), "-c".into(), line])
}

fn program_of(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// The toolchain a command line runs, from its first program word after
/// any `NAME=value` assignments: `None` when it names no known one.
fn family_of_line(line: &str) -> Option<Family> {
    let first = line
        .split_whitespace()
        .find(|w| !w.contains('=') || w.starts_with('-'))?;
    match program_of(first) {
        "cargo" => Some(Family::Rust),
        "go" => Some(Family::Go),
        "pytest" | "python" | "python3" | "uv" | "tox" | "nox" => Some(Family::Python),
        "npm" | "npx" | "node" | "yarn" | "pnpm" => Some(Family::Node),
        _ => None,
    }
}

fn derive(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
    config: ConfigFile,
    resolver: &dyn ImageResolver,
    detection: Detection,
) -> Result<Environment, EnvError> {
    let platform = host_platform();

    let lockfile = match &config.env.lockfile {
        Some(raw) => {
            let path: Path = raw
                .parse()
                .map_err(|e| invalid(format!("env.lockfile: {e}")))?;
            match lookup(src, root, &path)? {
                Some(Node::File(blob) | Node::Exec(blob)) => Some(Lockfile { path, blob }),
                _ => {
                    return Err(invalid(format!(
                        "env.lockfile {raw:?} is not a file in the tree"
                    )))
                }
            }
        }
        None => {
            let path: Path = DEFAULT_LOCKFILE.parse().expect("valid path");
            match lookup(src, root, &path)? {
                Some(Node::File(blob) | Node::Exec(blob)) => Some(Lockfile { path, blob }),
                _ => None,
            }
        }
    };

    let install = Install {
        argv: match &config.env.install {
            Some(argv) => argv_field("env.install", argv)?,
            None => strings(DEFAULT_INSTALL),
        },
        allow_scripts: config.env.allow_scripts.unwrap_or(false),
        // An install that runs scripts leaves nothing to rebuild, so the
        // flag is stored only where it has an effect.
        rebuild: config.env.rebuild.unwrap_or(true) && !config.env.allow_scripts.unwrap_or(false),
    };

    let resolve = |raw: &str, field: &str| -> Result<Image, EnvError> {
        let (reference, digest) = parse_image(raw, field)?;
        let digest = match digest {
            Some(digest) => digest,
            None => resolver
                .resolve(&reference, platform)
                .map_err(|reason| EnvError::Resolve {
                    reference: reference.clone(),
                    reason,
                })?,
        };
        Ok(Image { reference, digest })
    };
    let mut image = match &config.env.image {
        Some(raw) => resolve(raw, "env.image")?,
        None => default_image(),
    };

    let mut layers = match &config.deps {
        Some(sections) => deps_layers(src, root, sections, lockfile.as_ref())?,
        None => Vec::new(),
    };
    let mut memory_mb = config.limits.memory_mb;

    let mut checks: Vec<CheckDef> = match &config.check {
        Some(sections) => check_defs(sections, &resolve)?,
        None if detection == Detection::Off => Vec::new(),
        None => {
            let manifest = read_file(src, root, &"package.json".parse().expect("valid path"))?;
            let lock = match &lockfile {
                Some(l) => Some(src.blob(&l.blob)?),
                None => None,
            };
            let exists = |path: &str| {
                path.parse::<Path>().ok().is_some_and(|p| {
                    matches!(
                        lookup(src, root, &p),
                        Ok(Some(Node::File(_) | Node::Exec(_) | Node::Symlink(_)))
                    )
                })
            };
            let root_check =
                crate::presets::default_check(manifest.as_deref(), lock.as_deref(), &exists);
            let root_typecheck = crate::presets::typecheck_check(
                crate::presets::TYPECHECK_CHECK_ID,
                manifest.as_deref(),
                &exists,
            );
            let packages = crate::presets::detect(src, root)?;
            if packages.is_empty() {
                let mut checks = vec![root_check];
                checks.extend(root_typecheck);
                checks
            } else {
                // Generic mode (§1.4.1): every package Aver detects gets its
                // own check, and a TypeScript Node package its type-check
                // gate; the root `package.json`, when there is one, keeps
                // the default image, and the default `test` check when it
                // has a `test` script (a workspace root often has none).
                let mut checks = Vec::new();
                if manifest.is_some() {
                    if root_check.approved {
                        checks.push(root_check);
                    }
                    checks.extend(root_typecheck);
                } else if config.env.image.is_none() {
                    image = packages[0].image.clone();
                }
                for package in packages {
                    if package.family == Family::Rust && memory_mb.is_none() {
                        memory_mb = Some(crate::presets::RUST_MEMORY_MB);
                    }
                    layers.extend(package.layer);
                    for mut check in std::iter::once(package.check).chain(package.typecheck) {
                        if package.image != image {
                            check.image = Some(package.image.clone());
                        }
                        checks.push(check);
                    }
                }
                checks
            }
        }
    };
    let layers = Set::new(layers).map_err(|e| invalid(format!("deps: {e}")))?;
    for check in &mut checks {
        if check.deps.is_none() {
            check.deps = default_deps(check, &image, lockfile.as_ref(), layers.as_slice());
        } else if check.deps.as_ref().is_some_and(|d| {
            d.iter().map(Path::as_str).collect::<Vec<_>>()
                == lockfile.iter().map(|l| l.path.as_str()).collect::<Vec<_>>()
        }) {
            check.deps = None;
        }
        if let Some(deps) = &check.deps {
            for path in deps.iter() {
                let known = lockfile.as_ref().is_some_and(|l| l.path == *path)
                    || layers.get(path.as_str()).is_some();
                if !known {
                    return Err(invalid(format!(
                        "check {}: deps names {path}, which is neither env.lockfile nor a [[deps]] lockfile",
                        check.id
                    )));
                }
            }
        }
    }
    let checks = Set::new(checks).map_err(|e| invalid(e.to_string()))?;

    let limits = limits(&LimitsSection {
        memory_mb,
        ..config.limits
    })?;

    let tests = match &config.env.tests {
        None => Set::empty(),
        Some(globs) if globs.is_empty() => {
            return Err(invalid("env.tests must name at least one pattern"))
        }
        Some(globs) if globs.len() > aver_core::env::MAX_TEST_GLOBS => {
            return Err(invalid(format!(
                "env.tests: at most {} patterns",
                aver_core::env::MAX_TEST_GLOBS
            )))
        }
        Some(globs) => Set::new(
            globs
                .iter()
                .map(|g| {
                    g.parse::<aver_core::PathGlob>()
                        .map_err(|e| invalid(format!("env.tests: {e}")))
                })
                .collect::<Result<Vec<_>, _>>()?,
        )
        .expect("globs are their own keys"),
    };

    let vars = config
        .vars
        .iter()
        .map(|(name, value)| {
            let name = EnvVarName::try_from(name.clone())
                .map_err(|e| invalid(format!("vars.{name}: {e}")))?;
            no_nul(&format!("vars.{name}"), value)?;
            Ok((name, value.clone()))
        })
        .collect::<Result<BTreeMap<_, _>, EnvError>>()?;

    Ok(Environment {
        schema: EnvSchema::required(&tests, &install, &checks, &layers),
        tests,
        platform,
        image,
        lockfile,
        install,
        checks,
        limits,
        vars,
        layers,
    })
}

/// The environment a verify of `head` against `base` runs (ARCHITECTURE.md
/// §1.13): the base's, unless the base has no `aver.toml` and names no
/// check while the head adds an `aver.toml` that does, as `aver check --
/// <cmd>` writes it. Then the head's, proposed: its checks unapproved, and
/// the new `aver.toml` is not the change under judgment.
pub struct Governing {
    pub id: EnvId,
    pub env: Environment,
    /// Who approved the contract: `None` where no `aver.toml` names the
    /// checks (CI's detection, which approves check by check).
    pub approval: Option<Approval>,
}

pub fn governing(store: &Store, base: &Snapshot, head: &Snapshot) -> Result<Governing, StoreError> {
    let id = crate::store::pinned_env(base)?;
    let env: Environment = store.get(&id)?;
    let config: Path = CONFIG_PATH.parse().expect("valid path");
    let blob = |node: Option<Node>| match node {
        Some(Node::File(blob) | Node::Exec(blob)) => Some(blob),
        _ => None,
    };
    let base_config = lookup(store, &base.root, &config)?;
    let from_base = Governing {
        id,
        env,
        approval: blob(base_config).map(|blob| Approval::Base { blob }),
    };
    if !from_base.env.checks.is_empty() || base_config.is_some() {
        return Ok(from_base);
    }
    let Some(head_id) = head.env.filter(|h| *h != id) else {
        return Ok(from_base);
    };
    let head_env: Environment = store.get(&head_id)?;
    let Some(head_config) = blob(lookup(store, &head.root, &config)?) else {
        return Ok(from_base);
    };
    if head_env.checks.is_empty() {
        return Ok(from_base);
    }
    let checks = head_env
        .checks
        .iter()
        .map(|c| CheckDef {
            approved: false,
            ..c.clone()
        })
        .collect();
    let checks = Set::new(checks).expect("the head's checks are distinct");
    let env = Environment {
        schema: EnvSchema::required(
            &head_env.tests,
            &head_env.install,
            &checks,
            &head_env.layers,
        ),
        checks,
        ..head_env
    };
    Ok(Governing {
        id: store.put(&env)?,
        env,
        approval: Some(Approval::Proposed { blob: head_config }),
    })
}

/// The judging patterns of the `aver.toml` at `root`, or the defaults
/// where there is none or it does not read as a contract. Read from the
/// base, so a change cannot loosen how it is judged.
pub fn base_judge(store: &Store, root: &TreeId) -> Result<JudgeGlobs, StoreError> {
    let config: Path = CONFIG_PATH.parse().expect("valid path");
    let Some(Node::File(blob) | Node::Exec(blob)) = lookup(store, root, &config)? else {
        return Ok(JudgeGlobs::default());
    };
    let bytes = store.get_raw(&blob)?;
    Ok(std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| contract::parse(text).ok())
        .map_or_else(JudgeGlobs::default, |p| p.contract.judge))
}

/// The default image: Node, as before generic mode.
pub fn default_image() -> Image {
    Image {
        reference: DEFAULT_IMAGE_REF.to_owned(),
        digest: DEFAULT_IMAGE_DIGEST.parse().expect("valid digest"),
    }
}

/// The directory of a lockfile, `""` at the root.
pub fn lockfile_dir(path: &Path) -> &str {
    path.as_str().rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// Whether `dir` is `ancestor` or below it (`""` is the root).
pub fn within(dir: &str, ancestor: &str) -> bool {
    ancestor.is_empty()
        || dir == ancestor
        || dir
            .strip_prefix(ancestor)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The layers a check mounts when it names none (§1.4.1): by the family of
/// its image, every npm layer for Node, and for Python, Rust and Go the one
/// layer of a matching kind nearest above its `cwd`; for any other image,
/// the environment's own lockfile layer. `None` when that is the
/// environment's own layer (or none), which is what an absent `deps` means.
pub fn default_deps(
    check: &CheckDef,
    env_image: &Image,
    lockfile: Option<&Lockfile>,
    layers: &[Layer],
) -> Option<Set<Path>> {
    if layers.is_empty() {
        return None;
    }
    let all: Vec<(LayerKind, &Path)> = lockfile
        .map(|l| (LayerKind::Npm, &l.path))
        .into_iter()
        .chain(layers.iter().map(|l| (l.kind, &l.lockfile.path)))
        .collect();
    let cwd = check.cwd.as_ref().map_or("", Path::as_str);
    let chosen: Vec<Path> = match Family::of(check.image.as_ref().unwrap_or(env_image)) {
        None => lockfile.map(|l| l.path.clone()).into_iter().collect(),
        Some(Family::Node) => all
            .iter()
            .filter(|(kind, _)| *kind == LayerKind::Npm)
            .map(|(_, p)| (*p).clone())
            .collect(),
        Some(family) => all
            .iter()
            .filter(|(kind, p)| family.mounts(*kind) && within(cwd, lockfile_dir(p)))
            .max_by_key(|(kind, p)| (lockfile_dir(p).len(), *kind == LayerKind::Uv))
            .map(|(_, p)| (*p).clone())
            .into_iter()
            .collect(),
    };
    let default: Vec<&Path> = lockfile.map(|l| &l.path).into_iter().collect();
    if chosen.iter().collect::<Vec<_>>() == default {
        return None;
    }
    Some(Set::new(chosen).expect("lockfile paths are distinct"))
}

/// The layer kind a lockfile's name implies, if any.
pub fn kind_of_lockfile(name: &str) -> Option<LayerKind> {
    match name {
        "package-lock.json" | "npm-shrinkwrap.json" => Some(LayerKind::Npm),
        "uv.lock" => Some(LayerKind::Uv),
        "Cargo.lock" => Some(LayerKind::Cargo),
        "go.sum" => Some(LayerKind::Go),
        _ if name.ends_with(".txt") && name.contains("requirements") => Some(LayerKind::Pip),
        _ => None,
    }
}

/// The install a layer of `kind` runs when `[[deps]]` names none (§2.4.2).
/// The sandbox adds what makes it script-free unless scripts are allowed.
pub fn default_install(kind: LayerKind, lockfile_name: &str) -> Vec<String> {
    match kind {
        LayerKind::Npm => strings(DEFAULT_INSTALL),
        LayerKind::Uv => strings(&["uv", "sync", "--frozen", "--no-install-workspace"]),
        LayerKind::Pip => vec![
            "pip".into(),
            "install".into(),
            "--no-cache-dir".into(),
            "-r".into(),
            lockfile_name.into(),
        ],
        LayerKind::Cargo => strings(&["cargo", "fetch", "--locked"]),
        LayerKind::Go => strings(&["go", "mod", "download"]),
    }
}

fn deps_layers(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
    sections: &[DepsSection],
    lockfile: Option<&Lockfile>,
) -> Result<Vec<Layer>, EnvError> {
    if sections.len() > aver_core::env::MAX_LAYERS {
        return Err(invalid(format!(
            "deps: at most {} layers",
            aver_core::env::MAX_LAYERS
        )));
    }
    let mut layers: Vec<Layer> = Vec::new();
    for section in sections {
        let raw = &section.lockfile;
        let field = |name: &str| format!("deps {raw:?}: {name}");
        let path: Path = raw
            .parse()
            .map_err(|e| invalid(format!("{}: {e}", field("lockfile"))))?;
        let blob = match lookup(src, root, &path)? {
            Some(Node::File(blob) | Node::Exec(blob)) => blob,
            _ => {
                return Err(invalid(format!(
                    "deps.lockfile {raw:?} is not a file in the tree"
                )))
            }
        };
        if lockfile.is_some_and(|l| l.path == path)
            || layers.iter().any(|l| l.lockfile.path == path)
        {
            return Err(invalid(format!("{} is declared twice", field("lockfile"))));
        }
        let name = path
            .segments()
            .last()
            .expect("a path has a segment")
            .to_owned();
        let kind = match section.kind.as_deref() {
            Some("npm") => LayerKind::Npm,
            Some("uv") => LayerKind::Uv,
            Some("pip") => LayerKind::Pip,
            Some("cargo") => LayerKind::Cargo,
            Some("go") => LayerKind::Go,
            Some(other) => {
                return Err(invalid(format!(
                    "{}: unknown kind {other:?}, expected npm, uv, pip, cargo or go",
                    field("kind")
                )))
            }
            None => kind_of_lockfile(&name).ok_or_else(|| {
                invalid(format!(
                    "{}: no kind for {name:?}; set kind = npm, uv, pip, cargo or go",
                    field("kind")
                ))
            })?,
        };
        let allow_scripts = section.allow_scripts.unwrap_or(false);
        if section.rebuild.is_some() && kind != LayerKind::Npm {
            return Err(invalid(format!(
                "{}: only npm layers rebuild",
                field("rebuild")
            )));
        }
        layers.push(Layer {
            kind,
            lockfile: Lockfile { path, blob },
            install: Install {
                argv: match &section.install {
                    Some(argv) => argv_field(&field("install"), argv)?,
                    None => default_install(kind, &name),
                },
                allow_scripts,
                rebuild: kind == LayerKind::Npm
                    && section.rebuild.unwrap_or(true)
                    && !allow_scripts,
            },
        });
    }
    Ok(layers)
}

fn parse_config(bytes: &[u8]) -> Result<ConfigFile, EnvError> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("is not valid UTF-8"))?;
    toml::from_str(text).map_err(|e| EnvError::Syntax(e.message().to_owned()))
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn no_nul(field: &str, s: &str) -> Result<(), EnvError> {
    if s.contains('\0') {
        return Err(invalid(format!("{field} contains NUL")));
    }
    Ok(())
}

fn argv_field(field: &str, argv: &[String]) -> Result<Vec<String>, EnvError> {
    match argv.first() {
        None => return Err(invalid(format!("{field} must not be empty"))),
        Some(program) if program.is_empty() => {
            return Err(invalid(format!("{field} has an empty program name")))
        }
        Some(_) => {}
    }
    argv.iter().try_for_each(|arg| no_nul(field, arg))?;
    Ok(argv.to_vec())
}

fn seconds_to_ms(field: &str, seconds: u32) -> Result<u32, EnvError> {
    seconds
        .checked_mul(1000)
        .filter(|ms| *ms > 0)
        .ok_or_else(|| invalid(format!("{field} must be between 1 and 4294967 seconds")))
}

fn positive(field: &str, value: Option<u32>, default: u32) -> Result<u32, EnvError> {
    match value {
        Some(0) => Err(invalid(format!("{field} must be positive"))),
        Some(v) => Ok(v),
        None => Ok(default),
    }
}

fn limits(section: &LimitsSection) -> Result<Limits, EnvError> {
    let cpu_millis = match section.cpus {
        None => DEFAULT_LIMITS.cpu_millis,
        Some(cpus) => {
            let millis = (cpus * 1000.0).round();
            if !(1.0..=1_024_000.0).contains(&millis) {
                return Err(invalid("limits.cpus must be between 0.001 and 1024"));
            }
            millis as u32
        }
    };
    Ok(Limits {
        cpu_millis,
        memory_mb: positive(
            "limits.memory_mb",
            section.memory_mb,
            DEFAULT_LIMITS.memory_mb,
        )?,
        pids: positive("limits.pids", section.pids, DEFAULT_LIMITS.pids)?,
        timeout_ms: match section.timeout_s {
            Some(s) => seconds_to_ms("limits.timeout_s", s)?,
            None => DEFAULT_LIMITS.timeout_ms,
        },
        output_bytes: positive(
            "limits.output_bytes",
            section.output_bytes,
            DEFAULT_LIMITS.output_bytes,
        )?,
    })
}

fn check_defs(
    sections: &[CheckSection],
    resolve: &dyn Fn(&str, &str) -> Result<Image, EnvError>,
) -> Result<Vec<CheckDef>, EnvError> {
    let mut defs = Vec::with_capacity(sections.len());
    for section in sections {
        let id: CheckId = section
            .id
            .parse()
            .map_err(|e| invalid(format!("check.id: {e}")))?;
        if defs.iter().any(|d: &CheckDef| d.id == id) {
            return Err(invalid(format!("check id {id:?} is declared twice")));
        }
        let field = |name: &str| format!("check {id}: {name}");
        let cwd = section
            .cwd
            .as_deref()
            .map(|raw| {
                raw.parse::<Path>()
                    .map_err(|e| invalid(format!("{}: {e}", field("cwd"))))
            })
            .transpose()?;
        let report = section
            .report
            .as_ref()
            .map(|r| {
                let format = match r.format.as_str() {
                    "junit" => ReportFormat::Junit,
                    "jest-json" => ReportFormat::JestJson,
                    "mocha-xunit" => ReportFormat::MochaXunit,
                    "libtest" => ReportFormat::Libtest,
                    "go-json" => ReportFormat::GoJson,
                    "pytest-junit" => ReportFormat::PytestJunit,
                    "tsc" => ReportFormat::Tsc,
                    "tap" => ReportFormat::Tap,
                    other => {
                        return Err(invalid(format!(
                            "{}: unsupported format {other:?}, expected \"junit\", \"jest-json\", \"mocha-xunit\", \"libtest\", \"go-json\", \"pytest-junit\", \"tsc\" or \"tap\"",
                            field("report.format"),
                        )))
                    }
                };
                let path: ReportPath = r
                    .path
                    .parse()
                    .map_err(|e| invalid(format!("{}: {e}", field("report.path"))))?;
                Ok(Report { format, path })
            })
            .transpose()?;
        let setup = match &section.setup {
            None => Vec::new(),
            Some(steps) if steps.is_empty() || steps.len() > aver_core::env::MAX_SETUP_STEPS => {
                return Err(invalid(format!(
                    "{}: 1 to {} commands",
                    field("setup"),
                    aver_core::env::MAX_SETUP_STEPS
                )))
            }
            Some(steps) => steps
                .iter()
                .map(|argv| argv_field(&field("setup"), argv))
                .collect::<Result<_, _>>()?,
        };
        let image = section
            .image
            .as_deref()
            .map(|raw| resolve(raw, &field("image")))
            .transpose()?;
        let deps = section
            .deps
            .as_ref()
            .map(|paths| {
                let paths = paths
                    .iter()
                    .map(|raw| {
                        raw.parse::<Path>()
                            .map_err(|e| invalid(format!("{}: {e}", field("deps"))))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Set::new(paths).map_err(|e| invalid(format!("{}: {e}", field("deps"))))
            })
            .transpose()?;
        let covers = match &section.covers {
            None => Set::empty(),
            Some(globs) if globs.is_empty() || globs.len() > aver_core::env::MAX_TEST_GLOBS => {
                return Err(invalid(format!(
                    "{}: 1 to {} patterns",
                    field("covers"),
                    aver_core::env::MAX_TEST_GLOBS
                )))
            }
            Some(globs) => Set::new(
                globs
                    .iter()
                    .map(|g| {
                        g.parse::<PathGlob>()
                            .map_err(|e| invalid(format!("{}: {e}", field("covers"))))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            )
            .map_err(|e| invalid(format!("{}: {e}", field("covers"))))?,
        };
        defs.push(CheckDef {
            argv: argv_field(&field("run"), &section.run)?,
            timeout_ms: section
                .timeout_s
                .map(|s| seconds_to_ms(&field("timeout_s"), s))
                .transpose()?,
            id,
            cwd,
            report,
            setup,
            image,
            deps,
            covers,
            min_tests: section.min_tests,
            approved: true,
        });
    }
    Ok(defs)
}

/// Splits `name[:tag][@sha256:…]` into the reference and its digest, if any.
/// The reference must match `[A-Za-z0-9][A-Za-z0-9._/:-]*`, so it can never
/// be read as a flag where it reaches the `docker` command line.
fn parse_image(raw: &str, field: &str) -> Result<(String, Option<Digest>), EnvError> {
    let (name, digest) = match raw.split_once('@') {
        Some((name, digest)) => (
            name,
            Some(
                digest
                    .parse::<Digest>()
                    .map_err(|e| invalid(format!("{field}: {e}")))?,
            ),
        ),
        None => (raw, None),
    };
    let valid = name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/' | b':'));
    if !valid {
        return Err(invalid(format!(
            "{field} {raw:?} is not an image reference"
        )));
    }
    Ok((name.to_owned(), digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_names_split_off_the_tag_but_not_a_registry_port() {
        assert_eq!(image_name("node:22-bookworm-slim"), "node");
        assert_eq!(
            image_name("localhost:5000/team/app:1.2"),
            "localhost:5000/team/app"
        );
        assert_eq!(
            image_name("localhost:5000/team/app"),
            "localhost:5000/team/app"
        );
        assert_eq!(normalize_repo("docker.io/library/node"), "node");
    }

    #[test]
    fn the_agents_table_is_accepted_and_ignored() {
        let text = b"[agents]\nread_only = [\"Explore\"]\nrequire_workspace = false\n";
        assert!(parse_config(text).is_ok());
        assert!(parse_config(b"[agent]\nx = 1\n").is_err());
    }
}
