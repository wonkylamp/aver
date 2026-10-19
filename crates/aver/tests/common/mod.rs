//! Test helpers shared by the `aver` integration tests: temporary stores,
//! fixture paths, and building snapshots from directories on disk.
//!
//! [`parse_aver_toml`] reads the subset of `aver.toml` (ARCHITECTURE.md
//! §1.4.1) the fixtures use, with the spec's defaults. It is a test helper,
//! not Aver's environment resolver: image refs must already carry a
//! digest, and the platform is the host's.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use aver::store::Store;
use aver_core::*;
use serde::Deserialize;
use tempfile::TempDir;

/// The Node image the fixtures pin.
pub const NODE_IMAGE: &str =
    "node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392";

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// A store in a fresh temporary directory, deleted when the `TempDir` drops.
pub fn open_store() -> (TempDir, Arc<Store>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("aver.db")).unwrap();
    (dir, Arc::new(store))
}

pub fn p<T: std::str::FromStr>(s: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    s.parse().unwrap()
}

/// Puts every file under `dir` into the store as blobs and trees, and returns
/// the root tree. Executable files become `exec` entries and symlinks become
/// `symlink` entries; `node_modules` and `.git` directories are skipped.
pub fn put_dir(store: &Store, dir: &FsPath) -> TreeId {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        if name == "node_modules" || name == ".git" {
            continue;
        }
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).unwrap();
        let node = if meta.file_type().is_symlink() {
            let target = fs::read_link(&path).unwrap();
            Node::Symlink(store.put_blob(target.to_str().unwrap().as_bytes()).unwrap())
        } else if meta.is_dir() {
            Node::Tree(put_dir(store, &path))
        } else {
            let blob = store.put_blob(&fs::read(&path).unwrap()).unwrap();
            if meta.permissions().mode() & 0o111 != 0 {
                Node::Exec(blob)
            } else {
                Node::File(blob)
            }
        };
        entries.push(TreeEntry {
            name: p(&name),
            node,
        });
    }
    let tree = Tree::new(Set::new(entries).unwrap());
    store.put(&tree).unwrap()
}

/// Puts a tree built from `(path, contents)` pairs, all regular files.
pub fn put_files(store: &Store, files: &[(&str, &str)]) -> TreeId {
    let dir = tempfile::tempdir().unwrap();
    for (path, contents) in files {
        let full = dir.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, contents).unwrap();
    }
    put_dir(store, dir.path())
}

pub fn host_platform() -> Platform {
    if cfg!(target_arch = "aarch64") {
        Platform::LinuxArm64
    } else {
        Platform::LinuxAmd64
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    env: EnvSection,
    #[serde(default)]
    limits: LimitsSection,
    #[serde(default)]
    check: Vec<CheckSection>,
    #[serde(default)]
    vars: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvSection {
    image: String,
    lockfile: Option<String>,
    install: Option<Vec<String>>,
    #[serde(default)]
    allow_scripts: bool,
    rebuild: Option<bool>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitsSection {
    cpus: Option<u32>,
    memory_mb: Option<u32>,
    pids: Option<u32>,
    timeout_s: Option<u32>,
    output_bytes: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckSection {
    id: String,
    run: Vec<String>,
    cwd: Option<String>,
    timeout_s: Option<u32>,
    timeout_ms: Option<u32>,
    report: Option<ReportSection>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportSection {
    format: String,
    path: String,
}

/// Builds an environment from `aver.toml` text. `lockfile_blob` is the id
/// of the lockfile's contents, required when the config names a lockfile.
pub fn parse_aver_toml(text: &str, lockfile_blob: Option<BlobId>) -> Environment {
    let config: Config = toml::from_str(text).unwrap();
    let (reference, digest) = config
        .env
        .image
        .split_once('@')
        .expect("fixture images are pinned by digest");
    let lockfile = config.env.lockfile.map(|path| Lockfile {
        path: p(&path),
        blob: lockfile_blob.expect("the lockfile exists"),
    });
    let checks = config
        .check
        .into_iter()
        .map(|c| CheckDef {
            id: p(&c.id),
            argv: c.run,
            cwd: c.cwd.map(|cwd| p(&cwd)),
            timeout_ms: c.timeout_ms.or(c.timeout_s.map(|s| s * 1000)),
            report: c.report.map(|r| {
                assert_eq!(r.format, "junit");
                Report {
                    format: ReportFormat::Junit,
                    path: p(&r.path),
                }
            }),
            setup: Vec::new(),
            image: None,
            deps: None,
            covers: aver_core::Set::empty(),
            min_tests: None,
            approved: true,
        })
        .collect();
    let limits = config.limits;
    let install = Install {
        argv: config.env.install.unwrap_or_else(|| {
            ["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund"]
                .map(String::from)
                .to_vec()
        }),
        allow_scripts: config.env.allow_scripts,
        rebuild: config.env.rebuild.unwrap_or(true) && !config.env.allow_scripts,
    };
    let checks: Set<CheckDef> = Set::new(checks).unwrap();
    Environment {
        schema: EnvSchema::required(&Set::empty(), &install, &checks, &Set::empty()),
        tests: Set::empty(),
        platform: host_platform(),
        image: Image {
            reference: reference.to_owned(),
            digest: p(digest),
        },
        lockfile,
        install,
        checks,
        limits: Limits {
            cpu_millis: limits.cpus.unwrap_or(2) * 1000,
            memory_mb: limits.memory_mb.unwrap_or(2048),
            pids: limits.pids.unwrap_or(512),
            timeout_ms: limits.timeout_s.unwrap_or(300) * 1000,
            output_bytes: limits.output_bytes.unwrap_or(65_536),
        },
        vars: config.vars.into_iter().map(|(k, v)| (p(&k), v)).collect(),
        layers: aver_core::Set::empty(),
    }
}

/// The environment described by `dir/aver.toml`.
pub fn env_for_dir(dir: &FsPath) -> Environment {
    let text = fs::read_to_string(dir.join("aver.toml")).unwrap();
    let config: toml::Table = toml::from_str(&text).unwrap();
    let lockfile = config["env"]
        .get("lockfile")
        .and_then(|l| l.as_str())
        .map(|l| BlobId::of_bytes(&fs::read(dir.join(l)).unwrap()));
    parse_aver_toml(&text, lockfile)
}

/// Stores `env` and a snapshot of `root` under it.
pub fn put_snapshot(store: &Store, root: TreeId, env: &Environment) -> SnapshotId {
    store.put(env).unwrap();
    store.put(&Snapshot::new(root, env.id())).unwrap()
}

pub struct Imported {
    pub snapshot: SnapshotId,
    pub root: TreeId,
    pub env: Environment,
}

impl Imported {
    pub fn check(&self, id: &str) -> CheckDef {
        self.env.checks.get(id).expect("check exists").clone()
    }
}

/// A snapshot of the directory `dir` under the environment its
/// `aver.toml` describes.
pub fn import_dir(store: &Store, dir: &FsPath) -> Imported {
    let env = env_for_dir(dir);
    let root = put_dir(store, dir);
    let snapshot = put_snapshot(store, root, &env);
    Imported {
        snapshot,
        root,
        env,
    }
}

pub fn import_fixture(store: &Store, name: &str) -> Imported {
    import_dir(store, &fixture(name))
}

/// The stored output blob of a run, as text.
pub fn output_text(store: &Store, run: &CheckRun) -> String {
    String::from_utf8_lossy(&store.get_raw(&run.output).unwrap()).into_owned()
}

/// `(test path, status)` pairs of a run, in id order.
pub fn test_statuses(run: &CheckRun) -> Vec<(String, TestStatus)> {
    run.tests
        .iter()
        .map(|t| (t.id.test_path().to_owned(), t.status))
        .collect()
}

/// Asserts `object` validates against its published schema,
/// `schemas/aver.<kind>.v<N>.json` for the version it carries.
pub fn assert_matches_schema<T: Object>(kind: &str, object: &T) {
    let instance: serde_json::Value = serde_json::from_slice(&object.canonical_bytes()).unwrap();
    assert_value_matches_schema(kind, &instance);
}

/// Validates `instance` against `schemas/aver.<kind>.v<N>.json`, where `N` is
/// the version its `schema` field names.
pub fn assert_value_matches_schema(kind: &str, instance: &serde_json::Value) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let load = |file: &str| -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(dir.join(file)).unwrap()).unwrap()
    };
    let defs = load("aver.defs.v0.json");
    let registry = jsonschema::Registry::new()
        .add("https://aver.build/schemas/defs/v0", &defs)
        .unwrap()
        .prepare()
        .unwrap();
    let version = instance["schema"]
        .as_str()
        .and_then(|s| s.rsplit_once("/v"))
        .map(|(_, v)| v.to_owned())
        .unwrap();
    let validator = jsonschema::options()
        .with_registry(&registry)
        .build(&load(&format!("aver.{kind}.v{version}.json")))
        .unwrap();
    let errors: Vec<String> = validator
        .iter_errors(instance)
        .map(|e| format!("{e} at {}", e.instance_path()))
        .collect();
    assert!(errors.is_empty(), "{kind} is invalid: {errors:#?}");
}
