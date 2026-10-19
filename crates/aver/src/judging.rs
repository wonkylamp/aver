//! The judging set (ARCHITECTURE.md §2.5.2): the files that decide whether a
//! check passes, as opposed to the code it judges, and how each head change
//! to one is judged.
//!
//! The set, computed from the base, is
//!
//! - `aver.toml` and the base environment's lockfile;
//! - in the repository root and in every directory on the way to a check's
//!   `cwd`: `package.json`, `package-lock.json`, `npm-shrinkwrap.json`,
//!   `.npmrc`, and test runner configuration (`.mocharc*`, `vitest.config.*`,
//!   `vitest.workspace.*`, `vite.config.*`, `jest.config.*`,
//!   `babel.config.*`, `.babelrc*`, `tsconfig*.json`);
//! - every test file (§1.4.1, [`Environment::is_test_file`]) that exists at
//!   base;
//! - in an `aver.env/v3` environment, the other ecosystems' harness: in the
//!   root and on the way to a check's `cwd`, `pyproject.toml`, `setup.cfg`,
//!   `pytest.ini`, `tox.ini`, `conftest.py`, `.cargo/config.toml` (and
//!   `.cargo/config`) and `go.mod`; anywhere, every `Cargo.toml` and
//!   `build.rs`; and every layer's lockfile.
//!
//! Each kind of file is judged its own way ([`split`]):
//!
//! - **Frozen** (`aver.toml`, `.npmrc`, runner configuration): head runs use
//!   the base's copy, and any head change is a failing `judging::<path>`
//!   entry.
//! - **Lockfiles** come from the head for head runs, so a dependency change
//!   installs; how they install (argv, `allow_scripts`, image, network)
//!   still comes from the base environment. Base and transplant runs keep
//!   the base's lockfile and dependencies.
//! - **`package.json`** is split field by field: head runs use the head's
//!   file with the [`FROZEN_FIELDS`] put back as the base has them, and a
//!   head change to one of those is a `judging::<path>#<field>` entry.
//!   `Cargo.toml` ([`CARGO_FROZEN`]), `pyproject.toml`
//!   ([`PYPROJECT_FROZEN`]) and `go.mod` ([`GOMOD_FROZEN`]) are split the
//!   same way, so a head can add a dependency but not change how tests are
//!   built, found or run.
//! - **Existing test files** run as the base has them in the judged head, so
//!   deleting or weakening one cannot hide a regression; the head's versions
//!   run separately, in the head-tests tree and on the transplant, for
//!   proving criteria. No entry. So do the tests inside an existing Rust
//!   source file: its `#[cfg(test)]` and `#[test]` items and the doc-tests
//!   in its doc comments ([`crate::inline_tests`]).

use std::collections::BTreeSet;

use aver_core::{BlobId, EnvSchema, Environment, Path};
use serde::de::{Deserializer, MapAccess, Visitor};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::env::{CONFIG_PATH, DEFAULT_LOCKFILE};
use crate::workspace::{ChangedFile, FileEntry, FileKind};

/// The `package.json` fields that configure how tests run. Head runs use the
/// base's values. `config` is here because npm hands it to scripts as
/// `npm_package_config_*`. `imports` and `exports` are not: they route
/// module resolution for the code under test the way `main` and the source
/// files do, all of which the head may change, and the runner and the test
/// files it loads are chosen by `scripts` and the runner configuration.
pub const FROZEN_FIELDS: &[&str] = &[
    "ava", "babel", "c8", "config", "jest", "mocha", "nyc", "scripts", "tap", "type", "vitest",
];

/// The `Cargo.toml` keys that decide how tests are built and run: the
/// target tables (a `[[test]]` with `harness = false` runs its own `main`),
/// the profiles (`debug-assertions`, `overflow-checks`) and the build script
/// and target discovery. Dependencies, features, workspace members and the
/// rest come from the head.
pub const CARGO_FROZEN: &[&str] = &[
    "bench",
    "bin",
    "example",
    "lib",
    "package.autobenches",
    "package.autobins",
    "package.autoexamples",
    "package.autolib",
    "package.autotests",
    "package.build",
    "profile",
    "test",
];

/// The `pyproject.toml` tables that configure the test runner and the type
/// checker; dependencies and the rest come from the head.
pub const PYPROJECT_FROZEN: &[&str] = &["tool.mypy", "tool.pytest"];

/// The `go.mod` directives that change how the code compiles and runs (the
/// language version, the toolchain, `GODEBUG` settings); `require`,
/// `replace` and the rest come from the head.
pub const GOMOD_FROZEN: &[&str] = &["go", "godebug", "toolchain"];

/// Why a path is in the judging set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Config,
    Lockfile,
    Manifest,
    Harness,
    Test,
    /// `Cargo.toml` or `pyproject.toml`, split by [`CARGO_FROZEN`] or
    /// [`PYPROJECT_FROZEN`].
    TomlManifest,
    /// `go.mod`, split by [`GOMOD_FROZEN`].
    GoMod,
}

impl Why {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Config => "the Aver configuration",
            Self::Lockfile => "a lockfile",
            Self::Manifest => "a package manifest",
            Self::Harness => "package manager or test runner configuration",
            Self::Test => "a test file that exists at base",
            Self::TomlManifest => "a package manifest",
            Self::GoMod => "a Go module file",
        }
    }
}

/// One head change to frozen harness, reported as a failing judging entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgingChange {
    pub path: Path,
    /// The `package.json` field, for a change to a frozen field.
    pub field: Option<String>,
    pub change: &'static str,
    pub why: Why,
}

impl JudgingChange {
    /// What the entry id names after `judging::`: the path, plus
    /// `#<field>` for a frozen `package.json` field.
    pub fn target(&self) -> String {
        match &self.field {
            Some(field) => format!("{}#{field}", self.path),
            None => self.path.to_string(),
        }
    }

    pub fn detail(&self) -> String {
        match &self.field {
            Some(field) => format!(
                "{} frozen field {field} of {} (test runner configuration); verification ran the base's value",
                self.change, self.path
            ),
            None => format!(
                "{} judging file {} ({}); verification ran the base's version",
                self.change,
                self.path,
                self.why.describe()
            ),
        }
    }
}

/// What a tree gets at a path: a file entry (`None`: no file), or new
/// contents for a regular file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    Entry(Option<FileEntry>),
    Bytes(Vec<u8>),
}

/// The test files a head added, changed and deleted, in path order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestFiles {
    pub added: Vec<Path>,
    pub modified: Vec<Path>,
    pub deleted: Vec<Path>,
}

/// How verification judges a head's changes to the judging set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Split {
    /// Head changes to frozen harness: one failing entry each.
    pub judging: Vec<JudgingChange>,
    /// Put into the head tree for every head run: the base's copies of
    /// frozen files, and `package.json` files with their frozen fields put
    /// back.
    pub harness: Vec<(Path, Content)>,
    /// Put into the head tree for the judged head only: the base's copies
    /// of the test files that exist at base and the head changed or deleted.
    pub base_tests: Vec<(Path, Content)>,
    /// Removed from the judged head for the per-test checks of the base
    /// environment: the test files the head added. Those checks judge the
    /// base's tests in a run with no test file of the head's, so an added
    /// one cannot patch the code they judge (a module that monkeypatches
    /// it, a `conftest.py`, a Go `init`); the head's tests run in the
    /// head-tests tree.
    pub added_tests: Vec<(Path, Content)>,
    /// Put into the base tree for the transplant: the head's new and changed
    /// test files. The transplant keeps the base's dependencies: it is the
    /// base, which has no use for the head's.
    pub transplant: Vec<(Path, Content)>,
    pub tests: TestFiles,
    /// The head's `[[test]]` targets that the base's frozen `Cargo.toml`
    /// lacks or defines otherwise ([`retargeted_tests`]).
    pub retargeted: Vec<RetargetedTest>,
    /// The Rust source files the head deleted whose base versions hold
    /// test items or doc-tests: those tests went with their code.
    pub deleted_sources: Vec<Path>,
}

/// A `[[test]]` target of a head `Cargo.toml` that the base's copy lacks or
/// defines otherwise (its `required-features`, `harness` or `path`). Head
/// runs build it from the base's frozen manifest, where it may not build
/// or behave as the head declares it: without its required features, it
/// compiles and fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetargetedTest {
    /// The directory of the `Cargo.toml` (`""` at the root).
    pub dir: String,
    /// The target's `name`.
    pub name: String,
    /// The target's root file, relative to `dir`: its `path`, else Cargo's
    /// default `tests/<name>.rs` (or `tests/<name>/main.rs`, also matched).
    pub path: String,
}

impl RetargetedTest {
    /// Whether a libtest binary label (`<name> <target>`, as
    /// [`crate::exec::libtest`] names it) is this target's.
    pub fn is_label(&self, binary: &str, target: &str) -> bool {
        let dir_main = format!("tests/{}/main.rs", self.name);
        binary == self.name || target == self.path || target == dir_main
    }
}

/// The head's `[[test]]` targets of one `Cargo.toml` whose table the base's
/// copy lacks or differs in, by name.
pub fn retargeted_tests(dir: &str, head: &str, base: &str) -> Vec<RetargetedTest> {
    let targets = |text: &str| -> Vec<toml::Table> {
        text.parse::<toml::Table>()
            .ok()
            .and_then(|t| t.get("test").and_then(|v| v.as_array()).cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_table().cloned())
            .collect()
    };
    let name = |t: &toml::Table| t.get("name").and_then(|v| v.as_str()).map(str::to_owned);
    let base = targets(base);
    targets(head)
        .into_iter()
        .filter(|h| !base.iter().any(|b| b == h))
        .filter_map(|h| {
            let name = name(&h)?;
            let path = h
                .get("path")
                .and_then(|v| v.as_str())
                .map(|p| p.trim_start_matches("./").to_owned())
                .unwrap_or_else(|| format!("tests/{name}.rs"));
            Some(RetargetedTest {
                dir: dir.to_owned(),
                name,
                path,
            })
        })
        .collect()
}

/// Whether a file name is package manager or test runner configuration.
pub fn is_harness_name(name: &str) -> bool {
    let has_ext = |stem: &str| {
        name.strip_prefix(stem)
            .and_then(|rest| rest.strip_prefix('.'))
            .is_some_and(|ext| !ext.is_empty())
    };
    matches!(
        name,
        "package.json" | "package-lock.json" | "npm-shrinkwrap.json" | ".npmrc"
    ) || name.starts_with(".mocharc")
        || name.starts_with(".babelrc")
        || (name.starts_with("tsconfig") && name.ends_with(".json"))
        || [
            "vitest.config",
            "vitest.workspace",
            "vite.config",
            "jest.config",
            "babel.config",
        ]
        .iter()
        .any(|stem| has_ext(stem))
}

/// Whether a file name is another ecosystem's harness, judged in the root
/// and on the way to a check's `cwd` in a v3 environment: Python's test
/// runner configuration and Cargo's configuration (`.cargo/config.toml`,
/// matched by its last two segments in [`why`]).
pub fn is_ecosystem_harness_name(name: &str) -> bool {
    matches!(
        name,
        "setup.cfg" | "pytest.ini" | "tox.ini" | "conftest.py" | "mypy.ini" | ".mypy.ini"
    )
}

/// The directories whose harness files judge: the root (`""`) and every
/// directory on the way to each check's `cwd`, inclusive.
pub fn harness_dirs<'a>(cwds: impl IntoIterator<Item = Option<&'a Path>>) -> BTreeSet<String> {
    let mut dirs = BTreeSet::from([String::new()]);
    for cwd in cwds.into_iter().flatten() {
        let mut prefix = String::new();
        for segment in cwd.segments() {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            dirs.insert(prefix.clone());
        }
    }
    dirs
}

/// The path of the lockfile head runs install from: the base environment's
/// lockfile path, else the default `package-lock.json`.
pub fn lockfile_path(env: &Environment) -> Path {
    env.lockfile
        .as_ref()
        .map(|l| l.path.clone())
        .unwrap_or_else(|| DEFAULT_LOCKFILE.parse().expect("a valid path"))
}

/// Why `path` is in the judging set of a verification whose base
/// environment is `env` and whose checks run in `dirs` ([`harness_dirs`]),
/// or `None`. `at_base` says whether the file exists at base.
pub fn why(env: &Environment, dirs: &BTreeSet<String>, path: &str, at_base: bool) -> Option<Why> {
    if path == CONFIG_PATH {
        return Some(Why::Config);
    }
    if lockfile_path(env).as_str() == path
        || env.layers.iter().any(|l| l.lockfile.path.as_str() == path)
    {
        return Some(Why::Lockfile);
    }
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    if env.schema >= EnvSchema::V3 {
        let (parent, dot) = dir.rsplit_once('/').unwrap_or(("", dir));
        match name {
            "Cargo.toml" => return Some(Why::TomlManifest),
            "build.rs" => return Some(Why::Harness),
            "pyproject.toml" if dirs.contains(dir) => return Some(Why::TomlManifest),
            "go.mod" if dirs.contains(dir) => return Some(Why::GoMod),
            "config.toml" | "config" if dot == ".cargo" && dirs.contains(parent) => {
                return Some(Why::Harness)
            }
            _ if is_ecosystem_harness_name(name) && dirs.contains(dir) => {
                return Some(Why::Harness)
            }
            _ => {}
        }
    }
    if is_harness_name(name) && dirs.contains(dir) {
        return Some(match name {
            "package.json" => Why::Manifest,
            "package-lock.json" | "npm-shrinkwrap.json" => Why::Lockfile,
            _ => Why::Harness,
        });
    }
    (at_base && env.is_test_file(path)).then_some(Why::Test)
}

/// Splits the head's changes (`changed`, base to head) to the judging set
/// into what head runs, the head-tests tree and the transplant use, and
/// which are failing judging entries. `read` returns a blob's bytes.
pub fn split<E>(
    env: &Environment,
    dirs: &BTreeSet<String>,
    changed: &[ChangedFile],
    mut read: impl FnMut(&BlobId) -> Result<Vec<u8>, E>,
) -> Result<Split, E> {
    let mut out = Split::default();
    for (raw, base, head) in changed {
        let path: Path = raw.parse().expect("tree paths are valid paths");
        let change = match (base, head) {
            (None, _) => "added",
            (_, None) => "deleted",
            _ => "modified",
        };
        let Some(why) = why(env, dirs, raw, base.is_some()) else {
            if base.is_none() && env.is_test_file(raw) {
                out.tests.added.push(path.clone());
                out.transplant.push((path.clone(), Content::Entry(*head)));
                out.added_tests.push((path, Content::Entry(None)));
            } else if let (Some(b), Some(h)) = (base, head) {
                // Tests inside a Rust source file the head changed, its
                // doc-tests among them: the judged head gets the base's
                // versions of them back.
                let regular = b.kind == FileKind::File && h.kind == FileKind::File;
                if regular && raw.ends_with(".rs") {
                    let (b_text, h_text) = (read(&b.blob)?, read(&h.blob)?);
                    if let (Ok(b_text), Ok(h_text)) =
                        (std::str::from_utf8(&b_text), std::str::from_utf8(&h_text))
                    {
                        if let Some(Some(judged)) =
                            crate::inline_tests::with_base_tests(h_text, b_text)
                        {
                            out.tests.modified.push(path.clone());
                            out.base_tests
                                .push((path.clone(), Content::Bytes(judged.into_bytes())));
                            // The transplant gets the head's test items on
                            // the base's code, as it gets the head's test
                            // files, so a test fn added to an existing file
                            // can fail there and show or prove the change.
                            if let Some(Some(transplanted)) =
                                crate::inline_tests::with_base_tests(b_text, h_text)
                            {
                                out.transplant
                                    .push((path, Content::Bytes(transplanted.into_bytes())));
                            }
                        }
                    }
                }
            } else if let (Some(b), None) = (base, head) {
                if b.kind == FileKind::File && raw.ends_with(".rs") {
                    let text = read(&b.blob)?;
                    let scan = std::str::from_utf8(&text)
                        .ok()
                        .and_then(crate::inline_tests::scan);
                    if scan.is_some_and(|s| !s.items.is_empty() || !s.docs.is_empty()) {
                        out.deleted_sources.push(path);
                    }
                }
            }
            continue;
        };
        match why {
            Why::Lockfile => {}
            Why::Test => {
                out.base_tests.push((path.clone(), Content::Entry(*base)));
                if head.is_some() {
                    out.tests.modified.push(path.clone());
                    out.transplant.push((path, Content::Entry(*head)));
                } else {
                    out.tests.deleted.push(path);
                }
            }
            Why::Manifest => {
                let file = |f: &Option<FileEntry>| f.is_none_or(|f| f.kind == FileKind::File);
                let mut parse = |f: &Option<FileEntry>| -> Result<Option<(Manifest, Vec<u8>)>, E> {
                    Ok(match f {
                        Some(f) => {
                            let bytes = read(&f.blob)?;
                            Manifest::parse(&bytes).map(|m| (m, bytes))
                        }
                        None => Some((Manifest::default(), Vec::new())),
                    })
                };
                let parsed = if head.is_some() && file(base) && file(head) {
                    match (parse(base)?, parse(head)?) {
                        (Some((b, _)), Some((h, bytes))) => Some((b, h, bytes)),
                        _ => None,
                    }
                } else {
                    None
                };
                let Some((base_manifest, head_manifest, head_bytes)) = parsed else {
                    out.judging.push(JudgingChange {
                        path: path.clone(),
                        field: None,
                        change,
                        why,
                    });
                    out.harness.push((path, Content::Entry(*base)));
                    continue;
                };
                for field in FROZEN_FIELDS {
                    let field_change =
                        match (base_manifest.value(field), head_manifest.value(field)) {
                            (None, None) => continue,
                            (None, Some(_)) => "added",
                            (Some(_), None) => "deleted",
                            (Some(b), Some(h)) if b == h => continue,
                            _ => "modified",
                        };
                    out.judging.push(JudgingChange {
                        path: path.clone(),
                        field: Some((*field).to_owned()),
                        change: field_change,
                        why,
                    });
                }
                let judged = head_manifest.with_fields_of(&base_manifest, FROZEN_FIELDS);
                if judged != head_bytes {
                    out.harness.push((path.clone(), Content::Bytes(judged)));
                }
            }
            Why::TomlManifest | Why::GoMod => {
                let frozen = match (why, name_of(raw)) {
                    (Why::GoMod, _) => GOMOD_FROZEN,
                    (_, "Cargo.toml") => CARGO_FROZEN,
                    _ => PYPROJECT_FROZEN,
                };
                let regular = |f: &Option<FileEntry>| f.is_none_or(|f| f.kind == FileKind::File);
                let text = |f: &Option<FileEntry>,
                            read: &mut dyn FnMut(&BlobId) -> Result<Vec<u8>, E>|
                 -> Result<Option<String>, E> {
                    Ok(match f {
                        Some(f) => String::from_utf8(read(&f.blob)?).ok(),
                        None => Some(String::new()),
                    })
                };
                let texts = if head.is_some() && regular(base) && regular(head) {
                    match (text(base, &mut read)?, text(head, &mut read)?) {
                        (Some(b), Some(h)) => Some((b, h)),
                        _ => None,
                    }
                } else {
                    None
                };
                if frozen == CARGO_FROZEN {
                    if let Some((base_text, head_text)) = &texts {
                        let dir = raw.rsplit_once('/').map_or("", |(d, _)| d);
                        out.retargeted
                            .extend(retargeted_tests(dir, head_text, base_text));
                    }
                }
                let split = texts.and_then(|(b, h)| {
                    if why == Why::GoMod {
                        Some(gomod_with_directives_of(&h, &b))
                    } else {
                        toml_with_fields_of(&h, &b, frozen)
                    }
                    .map(|(changed, judged)| (changed, judged, h))
                });
                let Some((changed, judged, head_text)) = split else {
                    out.judging.push(JudgingChange {
                        path: path.clone(),
                        field: None,
                        change,
                        why,
                    });
                    out.harness.push((path, Content::Entry(*base)));
                    continue;
                };
                for (field, field_change) in changed {
                    out.judging.push(JudgingChange {
                        path: path.clone(),
                        field: Some(field),
                        change: field_change,
                        why,
                    });
                }
                if judged != head_text {
                    out.harness
                        .push((path.clone(), Content::Bytes(judged.into_bytes())));
                }
            }
            Why::Config | Why::Harness => {
                out.judging.push(JudgingChange {
                    path: path.clone(),
                    field: None,
                    change,
                    why,
                });
                out.harness.push((path, Content::Entry(*base)));
            }
        }
    }
    untangle(&mut out);
    Ok(out)
}

/// Where the head turned a test file into a directory of test files (or a
/// directory into a file), the trees that mix base and head paths keep one
/// side: the judged head keeps the base's (the deleted file or files come
/// back, so the head's paths in their place go), and the transplant keeps
/// the head's (the base's paths in the way go).
fn untangle(out: &mut Split) {
    let under = |path: &Path, dir: &Path| {
        path.as_str()
            .strip_prefix(dir.as_str())
            .is_some_and(|rest| rest.starts_with('/'))
    };
    let deleted = out.tests.deleted.clone();
    let added = out.tests.added.clone();
    let tangled = |a: &Path| deleted.iter().any(|d| under(a, d) || under(d, a));
    for a in added.iter().filter(|a| tangled(a)) {
        out.added_tests.retain(|(p, _)| p != a);
        out.base_tests.push((a.clone(), Content::Entry(None)));
    }
    for d in deleted
        .iter()
        .filter(|d| added.iter().any(|a| under(a, d) || under(d, a)))
    {
        out.transplant.push((d.clone(), Content::Entry(None)));
    }
}

fn name_of(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

/// The value at dotted key `key` (`package.build`) of a TOML table.
fn toml_get<'a>(table: &'a toml::Table, key: &str) -> Option<&'a toml::Value> {
    let mut parts = key.split('.');
    let mut value = table.get(parts.next()?)?;
    for part in parts {
        value = value.as_table()?.get(part)?;
    }
    Some(value)
}

/// Sets (or, with `None`, removes) dotted key `key`, creating the tables on
/// the way.
fn toml_set(table: &mut toml::Table, key: &str, value: Option<toml::Value>) {
    let (parents, last) = match key.rsplit_once('.') {
        Some((parents, last)) => (Some(parents), last),
        None => (None, key),
    };
    let mut current = table;
    if let Some(parents) = parents {
        for part in parents.split('.') {
            let entry = current
                .entry(part.to_owned())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            let Some(next) = entry.as_table_mut() else {
                return;
            };
            current = next;
        }
    }
    match value {
        Some(v) => {
            current.insert(last.to_owned(), v);
        }
        None => {
            current.remove(last);
        }
    }
}

/// The head's TOML manifest with each of `fields` (dotted keys) as the base
/// has it, and the fields that differ with how. The head's text is kept
/// as it is when no frozen field changed; otherwise the result is the
/// rewritten table, serialized. `None` when either side is not TOML.
pub fn toml_with_fields_of(
    head: &str,
    base: &str,
    fields: &[&str],
) -> Option<(Vec<(String, &'static str)>, String)> {
    let mut head_doc: toml::Table = head.parse().ok()?;
    let base_doc: toml::Table = base.parse().ok()?;
    let mut changed = Vec::new();
    for field in fields {
        let (b, h) = (toml_get(&base_doc, field), toml_get(&head_doc, field));
        let how = match (b, h) {
            (None, None) => continue,
            (None, Some(_)) => "added",
            (Some(_), None) => "deleted",
            (Some(b), Some(h)) if b == h => continue,
            _ => "modified",
        };
        changed.push(((*field).to_owned(), how));
        let value = b.cloned();
        toml_set(&mut head_doc, field, value);
    }
    if changed.is_empty() {
        return Some((changed, head.to_owned()));
    }
    Some((changed, toml::to_string(&head_doc).ok()?))
}

/// The statements of a `go.mod` directive in `text`: its single-line forms
/// and block lines, trimmed, comments dropped.
fn gomod_statements(text: &str, directive: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        if in_block {
            if line == ")" {
                in_block = false;
            } else if !line.is_empty() {
                out.push(format!("{directive} {line}"));
            }
            continue;
        }
        let Some(rest) = line.strip_prefix(directive) else {
            continue;
        };
        if !rest.starts_with([' ', '\t', '(']) {
            continue;
        }
        let rest = rest.trim();
        if rest == "(" {
            in_block = true;
        } else if !rest.is_empty() {
            out.push(format!("{directive} {rest}"));
        }
    }
    out
}

/// The head's `go.mod` with the [`GOMOD_FROZEN`] directives as the base has
/// them, and the directives that differ. The head's text is kept as it is
/// when none differ; otherwise its frozen statements are removed and the
/// base's added after the `module` line.
pub fn gomod_with_directives_of(head: &str, base: &str) -> (Vec<(String, &'static str)>, String) {
    let mut changed = Vec::new();
    for directive in GOMOD_FROZEN {
        let (b, h) = (
            gomod_statements(base, directive),
            gomod_statements(head, directive),
        );
        let how = match (b.is_empty(), h.is_empty()) {
            _ if b == h => continue,
            (true, false) => "added",
            (false, true) => "deleted",
            _ => "modified",
        };
        changed.push(((*directive).to_owned(), how));
    }
    if changed.is_empty() {
        return (changed, head.to_owned());
    }
    let mut kept: Vec<String> = Vec::new();
    let mut in_block = false;
    for line in head.lines() {
        let trimmed = line.split("//").next().unwrap_or("").trim();
        if in_block {
            in_block = trimmed != ")";
            continue;
        }
        let frozen = GOMOD_FROZEN.iter().any(|d| {
            trimmed
                .strip_prefix(d)
                .is_some_and(|rest| rest.starts_with([' ', '\t', '(']))
        });
        if frozen {
            in_block = trimmed.ends_with('(');
            continue;
        }
        kept.push(line.to_owned());
    }
    let base_lines: Vec<String> = GOMOD_FROZEN
        .iter()
        .flat_map(|d| gomod_statements(base, d))
        .collect();
    let at = kept
        .iter()
        .position(|l| l.trim_start().starts_with("module"))
        .map_or(0, |i| i + 1);
    for (offset, line) in base_lines.into_iter().enumerate() {
        kept.insert(at + offset, line);
    }
    (changed, kept.join("\n") + "\n")
}

/// A `package.json`: its top-level fields in file order, each value kept as
/// its exact JSON text, so a rewrite changes only the fields it replaces.
#[derive(Debug, Clone, Default)]
pub struct Manifest(Vec<(String, Box<RawValue>)>);

impl Manifest {
    /// The manifest in `bytes`, or `None` when it is not a JSON object or
    /// repeats a top-level key (Node would read the last copy, a rewrite
    /// might replace the first).
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }

    /// A field's value, parsed, for comparison.
    pub fn value(&self, field: &str) -> Option<serde_json::Value> {
        self.0
            .iter()
            .find(|(k, _)| k == field)
            .and_then(|(_, v)| serde_json::from_str(v.get()).ok())
    }

    /// This manifest with each of `fields` taken from `other` instead: its
    /// value replaced where both have it, dropped where `other` lacks it,
    /// and appended (in `other`'s order) where only `other` has it. Written
    /// as one field per line with two-space indent and a final newline.
    pub fn with_fields_of(&self, other: &Self, fields: &[&str]) -> Vec<u8> {
        let ours = |k: &str| fields.contains(&k);
        let theirs = |k: &str| other.0.iter().find(|(o, _)| o == k).map(|(_, v)| v);
        let mut lines: Vec<String> = Vec::new();
        for (key, value) in &self.0 {
            let value = if ours(key) {
                match theirs(key) {
                    Some(v) => v,
                    None => continue,
                }
            } else {
                value
            };
            lines.push(field_line(key, value));
        }
        for (key, value) in &other.0 {
            if ours(key) && !self.0.iter().any(|(k, _)| k == key) {
                lines.push(field_line(key, value));
            }
        }
        if lines.is_empty() {
            return b"{}\n".to_vec();
        }
        format!("{{\n{}\n}}\n", lines.join(",\n")).into_bytes()
    }
}

fn field_line(key: &str, value: &RawValue) -> String {
    format!(
        "  {}: {}",
        serde_json::to_string(key).expect("strings serialize"),
        value.get()
    )
}

impl<'de> Deserialize<'de> for Manifest {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = Manifest;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Manifest, A::Error> {
                let mut fields: Vec<(String, Box<RawValue>)> = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Box<RawValue>>()? {
                    if fields.iter().any(|(k, _)| *k == key) {
                        return Err(serde::de::Error::custom(format!("duplicate key {key:?}")));
                    }
                    fields.push((key, value));
                }
                Ok(Manifest(fields))
            }
        }
        d.deserialize_map(Fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_names() {
        for name in [
            "package.json",
            ".npmrc",
            ".mocharc.yml",
            ".mocharc",
            "vitest.config.ts",
            "vite.config.mjs",
            "jest.config.js",
            "tsconfig.json",
            "tsconfig.test.json",
            ".babelrc",
            "babel.config.cjs",
        ] {
            assert!(is_harness_name(name), "{name}");
        }
        for name in [
            "package.jsonx",
            "vitest.config",
            "src.js",
            "jest.configs.js",
            "tsconfig.yaml",
        ] {
            assert!(!is_harness_name(name), "{name}");
        }
    }

    #[test]
    fn harness_dirs_cover_every_directory_to_a_cwd() {
        let cwd: Path = "packages/web/app".parse().unwrap();
        let dirs = harness_dirs([Some(&cwd), None]);
        let want: BTreeSet<String> = ["", "packages", "packages/web", "packages/web/app"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(dirs, want);
    }

    #[test]
    fn manifests_keep_field_order_and_exact_values() {
        let head = Manifest::parse(
            br#"{"name":"calc","scripts":{"test":"exit 0"},"exports":{"import":"./a.mjs","default":"./a.cjs"},"dependencies":{"left-pad":"1.3.0"}}"#,
        )
        .unwrap();
        let base = Manifest::parse(
            br#"{ "name": "calc", "type": "module", "scripts": { "test": "node --test" } }"#,
        )
        .unwrap();
        let judged = String::from_utf8(head.with_fields_of(&base, FROZEN_FIELDS)).unwrap();
        assert_eq!(
            judged,
            "{\n  \"name\": \"calc\",\n  \"scripts\": { \"test\": \"node --test\" },\n  \"exports\": {\"import\":\"./a.mjs\",\"default\":\"./a.cjs\"},\n  \"dependencies\": {\"left-pad\":\"1.3.0\"},\n  \"type\": \"module\"\n}\n"
        );
        let reparsed: serde_json::Value = serde_json::from_str(&judged).unwrap();
        assert_eq!(reparsed["scripts"]["test"], "node --test");
        assert_eq!(
            Manifest::default().with_fields_of(&Manifest::default(), FROZEN_FIELDS),
            b"{}\n"
        );
    }

    #[test]
    fn manifests_refuse_what_a_rewrite_could_misread() {
        assert!(Manifest::parse(br#"{"scripts":{},"scripts":{"test":"x"}}"#).is_none());
        assert!(Manifest::parse(b"[]").is_none());
        assert!(Manifest::parse(b"{").is_none());
        assert!(Manifest::parse(br#"{"a":{"b":1,"b":2}}"#).is_some());
    }

    #[test]
    fn cargo_manifests_keep_the_heads_dependencies_and_the_bases_test_setup() {
        let base = "[package]\nname = \"calc\"\n\n[dependencies]\nitoa = \"1\"\n\n[profile.dev]\noverflow-checks = true\n";
        let deps = "[package]\nname = \"calc\"\n\n[dependencies]\nitoa = \"1\"\nryu = \"1\"\n\n[profile.dev]\noverflow-checks = true\n";
        let (changed, judged) = toml_with_fields_of(deps, base, CARGO_FROZEN).unwrap();
        assert!(changed.is_empty());
        assert_eq!(judged, deps, "a dependency change keeps the head's bytes");

        let weakened = "[package]\nname = \"calc\"\nbuild = \"gen.rs\"\n\n[dependencies]\nryu = \"1\"\n\n[profile.dev]\noverflow-checks = false\n\n[[test]]\nname = \"api\"\nharness = false\n";
        let (changed, judged) = toml_with_fields_of(weakened, base, CARGO_FROZEN).unwrap();
        assert_eq!(
            changed,
            [
                ("package.build".to_owned(), "added"),
                ("profile".to_owned(), "modified"),
                ("test".to_owned(), "added"),
            ]
        );
        let judged: toml::Table = judged.parse().unwrap();
        assert_eq!(
            judged["profile"]["dev"]["overflow-checks"].as_bool(),
            Some(true)
        );
        assert!(judged.get("test").is_none() && judged["package"].get("build").is_none());
        assert!(
            judged["dependencies"].get("ryu").is_some(),
            "dependencies stay the head's"
        );
        assert!(toml_with_fields_of("not toml [", base, CARGO_FROZEN).is_none());
    }

    #[test]
    fn go_mod_keeps_the_heads_requirements_and_the_bases_language_version() {
        let base =
            "module example.com/calc\n\ngo 1.22\n\nrequire github.com/google/go-cmp v0.6.0\n";
        let head = "module example.com/calc\n\ngo 1.23\n\ntoolchain go1.23.4\n\ngodebug (\n\tpanicnil=1\n)\n\nrequire github.com/google/go-cmp v0.7.0\n";
        let (changed, judged) = gomod_with_directives_of(head, base);
        assert_eq!(
            changed,
            [
                ("go".to_owned(), "modified"),
                ("godebug".to_owned(), "added"),
                ("toolchain".to_owned(), "added"),
            ]
        );
        assert!(
            judged.contains("go 1.22\n") && !judged.contains("1.23"),
            "{judged}"
        );
        assert!(
            !judged.contains("godebug") && !judged.contains("panicnil"),
            "{judged}"
        );
        assert!(judged.contains("go-cmp v0.7.0"), "{judged}");
        let same =
            "module example.com/calc\n\ngo 1.22\n\nrequire github.com/google/go-cmp v0.7.0\n";
        let (changed, judged) = gomod_with_directives_of(same, base);
        assert!(changed.is_empty());
        assert_eq!(judged, same);
    }
}
