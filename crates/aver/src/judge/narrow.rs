//! Narrowed runs (ARCHITECTURE.md §2.5.1): the transplant and the
//! head-tests run differ from the base and the judged head only in test
//! files, so a per-test Cargo check runs there only the test targets those
//! files make, and a pytest check only the test modules that changed, and
//! each takes every other target's or module's results from the side it
//! differs from, run by run. The result is what the full command would
//! report; whenever that cannot be shown, the side runs the full command.

use std::collections::BTreeSet;

use aver_core::scope::NarrowSide;
use aver_core::{Tree, TreeId};
use parking_lot::Mutex;

use super::*;
use crate::exec::cargo::CargoTest;
use crate::exec::libtest;

/// A side's command narrowed to the test targets its tree changes, and the
/// runs of the side it borrows every other target's results from.
pub(super) struct Narrow {
    pub(super) def: CheckDef,
    targets: Targets,
    source: BaseRuns,
    outcome: Mutex<Outcome>,
}

/// What a narrowed run runs, by the first segment of its tests' paths.
enum Targets {
    /// The labels of the changed integration test targets, which every
    /// narrowed run must show.
    Cargo(BTreeSet<String>),
    /// The changed test modules, as pytest's report names their files: the
    /// only files the run collects, so the only labels it may report.
    Pytest(BTreeSet<String>),
}

enum Outcome {
    Narrowed { ran: BTreeSet<String>, of: usize },
    Full(&'static str),
}

/// Whether to narrow, and how, for one side.
pub(super) enum Plan {
    /// The check is not one Aver narrows, or the trees do not differ.
    No,
    /// It runs in full, for this reason.
    Full(&'static str),
    Narrow(std::sync::Arc<Narrow>),
}

impl Plan {
    /// The plan for `def`'s runs on `tree`, whose results for unchanged
    /// targets are `source`'s runs of `def` on `from` in the same
    /// environment.
    pub(super) fn of(
        store: &Store,
        def: &CheckDef,
        check_source: CheckSource,
        from: &SnapshotId,
        tree: &SnapshotId,
        source: BaseRuns,
    ) -> Result<Self, StoreError> {
        use aver_core::ReportFormat::{Libtest, PytestJunit};
        let format = def.report.as_ref().map(|r| r.format);
        if check_source != CheckSource::Env
            || !matches!(format, Some(Libtest | PytestJunit))
            || from == tree
        {
            return Ok(Self::No);
        }
        let from_root = store.get::<Snapshot>(from)?.root;
        let root = store.get::<Snapshot>(tree)?.root;
        let mut changed = Vec::new();
        diff(
            store,
            String::new(),
            Some(from_root),
            Some(root),
            &mut changed,
        )?;
        if changed.is_empty() {
            return Ok(Self::No);
        }
        let files: BTreeMap<String, FileEntry> = crate::place::workdir::tree_files(store, &root)?
            .into_iter()
            .collect();
        let narrow = |def: CheckDef, targets: Targets| {
            Self::Narrow(std::sync::Arc::new(Narrow {
                def,
                targets,
                source,
                outcome: Mutex::new(Outcome::Full("no narrowed run ended")),
            }))
        };
        if format == Some(PytestJunit) {
            return Ok(match pytest::modules(store, def, &files, &changed)? {
                Err(why) => Self::Full(why),
                Ok(modules) => match super::focus::pytest(def, &modules) {
                    None => Self::Full("the command cannot be narrowed"),
                    Some(narrowed) => narrow(narrowed, Targets::Pytest(modules)),
                },
            });
        }
        let mut manifests = BTreeMap::new();
        for (path, entry) in files.iter().filter(|(p, _)| p.ends_with("Cargo.toml")) {
            let text = store.get_raw(&entry.blob)?;
            manifests.insert(path.clone(), String::from_utf8_lossy(&text).into_owned());
        }
        if let Some(why) = included(store, &files, &changed)? {
            return Ok(Self::Full(why));
        }
        Ok(match targets(def, &changed, &manifests) {
            Err(why) => Self::Full(why),
            Ok((tests, units, expect)) => match CargoTest::narrowed(def, &tests, units) {
                None => Self::Full("the command cannot be narrowed"),
                Some(def) => narrow(def, Targets::Cargo(expect)),
            },
        })
    }
}

/// A path that differs between two trees, and whether the newer has it.
pub(super) struct Changed {
    pub(super) path: String,
    pub(super) entry: Option<FileEntry>,
}

/// Every file that differs from tree `a` to tree `b` under `prefix`,
/// skipping subtrees both share.
fn diff(
    store: &Store,
    prefix: String,
    a: Option<TreeId>,
    b: Option<TreeId>,
    out: &mut Vec<Changed>,
) -> Result<(), StoreError> {
    if a == b {
        return Ok(());
    }
    let entries = |t: Option<TreeId>| -> Result<BTreeMap<String, Node>, StoreError> {
        Ok(match t {
            Some(t) => store
                .get::<Tree>(&t)?
                .entries
                .iter()
                .map(|e| (e.name.to_string(), e.node))
                .collect(),
            None => BTreeMap::new(),
        })
    };
    let (a, b) = (entries(a)?, entries(b)?);
    let names: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    for name in names {
        let path = format!("{prefix}{name}");
        let (x, y) = (a.get(name).copied(), b.get(name).copied());
        if x == y {
            continue;
        }
        let sub = |n: Option<Node>| match n {
            Some(Node::Tree(t)) => Some(t),
            _ => None,
        };
        let (tx, ty) = (sub(x), sub(y));
        if tx.is_some() || ty.is_some() {
            diff(store, format!("{path}/"), tx, ty, out)?;
        }
        let file = |n: Option<Node>| n.as_ref().and_then(FileEntry::of_node);
        if file(x).is_some() || file(y).is_some() {
            out.push(Changed {
                path,
                entry: file(y),
            });
        }
    }
    Ok(())
}

/// The integration test targets and whether the libraries' and binaries'
/// unit tests the changed files make, with the labels the test targets run
/// under; or why the change cannot be narrowed to targets. `manifests` are
/// the tree's `Cargo.toml` files by path.
#[allow(clippy::type_complexity)]
fn targets(
    def: &CheckDef,
    changed: &[Changed],
    manifests: &BTreeMap<String, String>,
) -> Result<(BTreeSet<String>, bool, BTreeSet<String>), &'static str> {
    let cwd = def
        .cwd
        .as_ref()
        .map_or(String::new(), |c| format!("{}/", c.as_str()));
    let (mut tests, mut units, mut expect) = (BTreeSet::new(), false, BTreeSet::new());
    for c in changed {
        let Some(entry) = c.entry else {
            return Err("a file was deleted");
        };
        if entry.kind == FileKind::Symlink || !c.path.ends_with(".rs") {
            return Err("a changed file is not Rust source");
        }
        let Some(path) = c.path.strip_prefix(&cwd) else {
            return Err("a changed file is outside the check's directory");
        };
        let package = std::path::Path::new(path)
            .ancestors()
            .skip(1)
            .map(|d| d.to_string_lossy().into_owned())
            .find(|d| manifests.contains_key(&format!("{cwd}{}", manifest(d))))
            .ok_or("a changed file is in no Cargo package")?;
        let rel = match package.as_str() {
            "" => path,
            dir => &path[dir.len() + 1..],
        };
        let name = match rel.split('/').collect::<Vec<_>>().as_slice() {
            ["tests", file] => file.strip_suffix(".rs").map(str::to_owned),
            ["tests", dir, "main.rs"] => Some((*dir).to_owned()),
            ["src", ..] => {
                units = true;
                continue;
            }
            _ => None,
        }
        .ok_or("a changed file is a test helper or another kind of target")?;
        if names_target(
            &manifests[&format!("{cwd}{}", manifest(&package))],
            &name,
            rel,
        ) {
            return Err("the package's manifest declares the test target");
        }
        expect.insert(format!("{} {rel}", name.replace('-', "_")));
        tests.insert(name);
    }
    Ok((tests, units, expect))
}

/// Why a target left alone may still compile a changed file: an unchanged
/// test target's root (`tests/x.rs`, `tests/x/main.rs`) declares `mod y;` for a
/// changed `tests/y.rs`, which Cargo also builds as its own target, or an
/// unchanged file under a `tests` directory pulls in a changed file by
/// `#[path]` or `include!` (or by a path Aver cannot resolve). Its results
/// then cannot be read from the side the run differs from.
fn included(
    store: &Store,
    files: &BTreeMap<String, FileEntry>,
    changed: &[Changed],
) -> Result<Option<&'static str>, StoreError> {
    let changed: BTreeSet<&str> = changed.iter().map(|c| c.path.as_str()).collect();
    for (path, entry) in files {
        let parts: Vec<&str> = path.split('/').collect();
        let (file, dirs) = parts.split_last().expect("a path has a name");
        if !file.ends_with(".rs") || !dirs.contains(&"tests") || changed.contains(path.as_str()) {
            continue;
        }
        let here = dirs.join("/");
        let root = match parts.as_slice() {
            [.., "tests", _] => Some(here.clone()),
            [.., "tests", _, "main.rs"] => Some(here.clone()),
            _ => None,
        };
        let text = store.get_raw(&entry.blob)?;
        for line in String::from_utf8_lossy(&text).lines() {
            if line.contains("#[path") || line.contains("include!") {
                // A `#[path]` inside an inline module resolves below it, so
                // only one at the start of a line is read.
                let attr = line.contains("#[path") && !line.starts_with("#[path");
                match by_path(&here, line).filter(|_| !attr) {
                    Some(p) if files.contains_key(&p) && !changed.contains(p.as_str()) => {}
                    _ => return Ok(Some("a test target includes a changed file by path")),
                }
                continue;
            }
            let Some(dir) = &root else {
                continue;
            };
            let decl = line.rsplit(']').next().unwrap_or(line).trim();
            let decl = decl
                .strip_prefix("pub(crate) ")
                .or_else(|| decl.strip_prefix("pub "))
                .unwrap_or(decl);
            let Some(name) = decl.strip_prefix("mod ").and_then(|d| d.strip_suffix(';')) else {
                continue;
            };
            let name = name.trim();
            if [format!("{dir}/{name}.rs"), format!("{dir}/{name}/mod.rs")]
                .iter()
                .any(|p| changed.contains(p.as_str()))
            {
                return Ok(Some(
                    "a changed test file is a module of another test target",
                ));
            }
        }
    }
    Ok(None)
}

/// The repository path a line's one `#[path = "…"]` or `include!("…")`
/// names, relative to the directory `here` of the file holding it; `None`
/// for anything but one plain string literal inside the repository.
fn by_path(here: &str, line: &str) -> Option<String> {
    let mut quoted = line.split('"');
    let (_, literal, rest) = (quoted.next()?, quoted.next()?, quoted.collect::<Vec<_>>());
    if rest.len() != 1 || literal.contains('\\') || literal.starts_with('/') {
        return None;
    }
    let mut out: Vec<&str> = here.split('/').filter(|p| !p.is_empty()).collect();
    for part in literal.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            p => out.push(p),
        }
    }
    Some(out.join("/"))
}

fn manifest(dir: &str) -> String {
    match dir {
        "" => "Cargo.toml".to_owned(),
        dir => format!("{dir}/Cargo.toml"),
    }
}

/// Whether a package manifest turns off test discovery or declares a test
/// target by `name` or `path` (with its own features, harness or path):
/// then `--test <name>` may not select what discovery would.
fn names_target(manifest: &str, name: &str, path: &str) -> bool {
    let Ok(doc) = manifest.parse::<toml::Table>() else {
        return true;
    };
    let autotests = doc
        .get("package")
        .and_then(|p| p.get("autotests"))
        .and_then(toml::Value::as_bool);
    let declared = doc
        .get("test")
        .and_then(toml::Value::as_array)
        .is_some_and(|tests| {
            tests.iter().any(|t| {
                t.get("name").and_then(toml::Value::as_str) == Some(name)
                    || t.get("path")
                        .and_then(toml::Value::as_str)
                        .map(|p| p.trim_start_matches("./"))
                        == Some(path)
            })
        });
    autotests == Some(false) || declared
}

impl Narrow {
    /// `run` (a narrowed run) with every test of a target it did not run
    /// added from the source side's run of the same index (else its last),
    /// and its result what the full command's would be; `None` when that
    /// cannot be shown, and the side must run in full.
    pub(super) async fn fill(
        &self,
        store: &Store,
        check: &CheckDef,
        run: ExecutedRun,
    ) -> Result<Option<ExecutedRun>, ExecError> {
        let mut source = self.source.clone();
        let source = source
            .wait_for(Option::is_some)
            .await
            .map_err(|_| ExecError::Internal("the source side ended".into()))?
            .clone()
            .expect("waited for");
        let filled = self.filled(store, check, run, &source)?;
        Ok(match filled {
            Ok((run, ran, of)) => {
                *self.outcome.lock() = Outcome::Narrowed { ran, of };
                Some(run)
            }
            Err(why) => {
                *self.outcome.lock() = Outcome::Full(why);
                None
            }
        })
    }

    #[allow(clippy::type_complexity)]
    fn filled(
        &self,
        store: &Store,
        check: &CheckDef,
        run: ExecutedRun,
        source: &[ExecutedRun],
    ) -> Result<Result<(ExecutedRun, BTreeSet<String>, usize), &'static str>, StoreError> {
        let (Some(report), Some(sources)) = (run.run.report, reported(source)) else {
            return Ok(Err(if run.run.report.is_none() {
                "the narrowed run reported no tests"
            } else {
                "the side it borrows from has no report"
            }));
        };
        let from = sources
            .iter()
            .find(|r| r.index == run.run.index)
            .or(sources.last())
            .copied()
            .expect("a reported side has runs");
        let label = |t: &aver_core::TestResult| {
            t.id.test_path()
                .split(junit::SEPARATOR)
                .next()
                .map(str::to_owned)
        };
        let (ran, all) = match &self.targets {
            Targets::Cargo(expect) => {
                let labels = |blob: &BlobId| -> Result<Option<BTreeSet<String>>, StoreError> {
                    Ok(libtest::binaries(&String::from_utf8_lossy(
                        &store.get_raw(blob)?,
                    )))
                };
                let Some(ran) = labels(&report)? else {
                    return Ok(Err("a narrowed test binary did not finish"));
                };
                let Some(all) = labels(&from.report.expect("reported"))? else {
                    return Ok(Err(
                        "a test binary of the side it borrows from did not finish",
                    ));
                };
                if !expect.is_subset(&ran) {
                    return Ok(Err("a changed test target did not run"));
                }
                (ran, all)
            }
            // A test reported under another label is one the run's
            // labels do not name the way Aver reads them (a rootdir
            // above the check's directory, a collection error): the
            // copied results could then overlap or miss it.
            Targets::Pytest(modules) => {
                if !run
                    .run
                    .tests
                    .iter()
                    .all(|t| label(t).is_some_and(|l| modules.contains(&l)))
                {
                    return Ok(Err(
                        "a narrowed run reported a test outside the changed modules",
                    ));
                }
                let all = from.tests.iter().filter_map(label).collect();
                (modules.clone(), all)
            }
        };
        // Each run's exit code must say what its tests say, so the full
        // command's would be what the tests together say.
        let consistent = |r: &CheckRun| {
            let failed = r.tests.iter().any(|t| t.status == TestStatus::Fail);
            match ending(r, check) {
                Ending::Pass => !failed,
                Ending::Fail => failed,
                _ => false,
            }
        };
        if !consistent(&run.run) || !consistent(from) {
            return Ok(Err("an exit code says more than the tests"));
        }
        let mut tests: Vec<aver_core::TestResult> = run.run.tests.iter().cloned().collect();
        tests.extend(
            from.tests
                .iter()
                .filter(|t| label(t).is_some_and(|l| !ran.contains(&l)))
                .cloned(),
        );
        let failed = |r: &CheckRun| (ending(r, check) == Ending::Fail).then_some(r.result);
        let result = failed(&run.run)
            .or_else(|| {
                tests
                    .iter()
                    .any(|t| t.status == TestStatus::Fail)
                    .then(|| failed(from))
                    .flatten()
            })
            .unwrap_or(run.run.result);
        let of = all.union(&ran).count();
        let mut run = run;
        run.run.tests = match Set::new(tests) {
            Ok(tests) => tests,
            Err(_) => return Ok(Err("a test ran in a target both sides ran")),
        };
        run.run.result = result;
        Ok(Ok((run, ran, of)))
    }

    /// What the side did, for the diagnostics.
    pub(super) fn record(&self, check: &CheckId, side: Side) -> diagnostics::Narrowed {
        let (ran, of, full) = match &*self.outcome.lock() {
            Outcome::Narrowed { ran, of } => {
                (ran.iter().cloned().collect(), Some(*of as u32), None)
            }
            Outcome::Full(why) => (Vec::new(), None, Some((*why).to_owned())),
        };
        diagnostics::Narrowed {
            check: check.clone(),
            side: narrow_side(side),
            ran,
            of,
            full,
        }
    }
}

/// A side that ran in full without trying, for the diagnostics.
pub(super) fn full(check: &CheckId, side: Side, why: &str) -> diagnostics::Narrowed {
    diagnostics::Narrowed {
        check: check.clone(),
        side: narrow_side(side),
        ran: Vec::new(),
        of: None,
        full: Some(why.to_owned()),
    }
}

fn narrow_side(side: Side) -> NarrowSide {
    match side {
        Side::Transplant => NarrowSide::Transplant,
        _ => NarrowSide::HeadTests,
    }
}

/// Which test modules a pytest side may run alone: what pytest collects
/// from a file named on its command line is what it collects from that
/// file in a full run only when nothing chooses files by path (the
/// configuration's `python_files`, `norecursedirs`, `testpaths` the file is
/// outside, `--ignore`, a `conftest.py` hook or `collect_ignore`) and no
/// other module imports it.
mod pytest {
    use super::*;

    /// Directories pytest's default `norecursedirs` skips, besides `.*`
    /// and `*.egg`.
    const SKIPPED: &[&str] = &[
        "build",
        "dist",
        "venv",
        "node_modules",
        "CVS",
        "_darcs",
        "{arch}",
    ];

    const CONFIGS: &[&str] = &[
        "pytest.ini",
        ".pytest.ini",
        "pyproject.toml",
        "tox.ini",
        "setup.cfg",
    ];

    /// What in a configuration or `conftest.py` file chooses which files
    /// pytest collects.
    const CHOOSERS: &[&str] = &[
        "python_files",
        "norecursedirs",
        "confcutdir",
        "rootdir",
        "collect_ignore",
        "--ignore",
        "--deselect",
        "--pyargs",
        "pytest_ignore_collect",
        "pytest_collect_file",
        "pytest_pycollect_makemodule",
    ];

    /// The changed test modules, relative to the check's directory, or why
    /// the side runs in full.
    pub(super) fn modules(
        store: &Store,
        def: &CheckDef,
        files: &BTreeMap<String, FileEntry>,
        changed: &[Changed],
    ) -> Result<Result<BTreeSet<String>, &'static str>, StoreError> {
        let cwd = def
            .cwd
            .as_ref()
            .map_or(String::new(), |c| format!("{}/", c.as_str()));
        let mut modules = BTreeSet::new();
        for c in changed {
            let Some(entry) = c.entry else {
                return Ok(Err("a file was deleted"));
            };
            let name = c.path.rsplit('/').next().unwrap_or(&c.path);
            let module = entry.kind != FileKind::Symlink
                && name.ends_with(".py")
                && (name.starts_with("test_") || name.ends_with("_test.py"));
            if !module {
                return Ok(Err("a changed file is not a test module"));
            }
            let Some(rel) = c.path.strip_prefix(&cwd) else {
                return Ok(Err("a changed file is outside the check's directory"));
            };
            let skipped =
                |d: &str| d.starts_with('.') || d.ends_with(".egg") || SKIPPED.contains(&d);
            if rel.split('/').rev().skip(1).any(skipped) {
                return Ok(Err("a changed test module is in a directory pytest skips"));
            }
            modules.insert(rel.to_owned());
        }
        let Some(paths) = super::super::focus::pytest_paths(def) else {
            return Ok(Err("the command cannot be narrowed"));
        };
        if !paths.iter().all(|p| plain(p)) || !modules.iter().all(|m| under(m, &paths)) {
            return Ok(Err(
                "the command collects from paths a changed module is not under",
            ));
        }
        let text = |entry: &FileEntry| -> Result<String, StoreError> {
            Ok(String::from_utf8_lossy(&store.get_raw(&entry.blob)?).into_owned())
        };
        // The configuration pytest reads is in the check's directory or
        // above it; one above it reads `testpaths` from another directory.
        let dirs: Vec<String> = std::iter::once(String::new())
            .chain(
                cwd.trim_end_matches('/')
                    .match_indices('/')
                    .map(|(at, _)| format!("{}/", &cwd[..at])),
            )
            .chain((!cwd.is_empty()).then(|| cwd.clone()))
            .collect();
        for dir in &dirs {
            for config in CONFIGS {
                let Some(entry) = files.get(&format!("{dir}{config}")) else {
                    continue;
                };
                let text = text(entry)?;
                if CHOOSERS.iter().any(|c| text.contains(c)) {
                    return Ok(Err(
                        "the pytest configuration chooses which files it collects",
                    ));
                }
                for line in text
                    .lines()
                    .filter(|l| l.trim_start().starts_with("testpaths"))
                {
                    let value = line.split_once('=').map_or("", |(_, v)| v);
                    let testpaths: Vec<String> = value
                        .split(|c: char| c.is_whitespace() || "[],\"'".contains(c))
                        .filter(|w| !w.is_empty())
                        .map(str::to_owned)
                        .collect();
                    let applies = *dir == cwd
                        && !testpaths.is_empty()
                        && testpaths.iter().all(|p| plain(p))
                        && modules.iter().all(|m| under(m, &testpaths));
                    if !applies {
                        return Ok(Err(
                            "the pytest configuration's testpaths may not collect a changed module",
                        ));
                    }
                }
            }
        }
        let changed: BTreeSet<&str> = changed.iter().map(|c| c.path.as_str()).collect();
        let stems: Vec<&str> = modules
            .iter()
            .filter_map(|m| m.rsplit('/').next()?.strip_suffix(".py"))
            .collect();
        for (path, entry) in files {
            let name = path.rsplit('/').next().unwrap_or(path);
            if !path.ends_with(".py") || changed.contains(path.as_str()) {
                continue;
            }
            let text = text(entry)?;
            if name == "conftest.py" && CHOOSERS.iter().any(|c| text.contains(c)) {
                return Ok(Err("a conftest.py chooses which files pytest collects"));
            }
            let imports = text.lines().map(str::trim_start).any(|l| {
                (l.starts_with("import ") || l.starts_with("from "))
                    && stems.iter().any(|s| word(l, s))
            });
            if imports {
                return Ok(Err("another module imports a changed test module"));
            }
        }
        Ok(Ok(modules))
    }

    /// A path argument Aver reads as a plain relative path: no node id,
    /// glob or option.
    fn plain(p: &str) -> bool {
        !p.is_empty()
            && !p.starts_with('/')
            && !p.contains("::")
            && !p.contains(['*', '?', '[', '$', '\'', '"', '\\'])
    }

    /// Whether `module` is one of `paths` or under one of them; with none,
    /// pytest collects from its whole directory.
    fn under(module: &str, paths: &[String]) -> bool {
        paths.is_empty()
            || paths.iter().any(|p| {
                let p = p.trim_start_matches("./").trim_end_matches('/');
                p.is_empty() || p == "." || module == p || module.starts_with(&format!("{p}/"))
            })
    }

    /// Whether `name` occurs in `line` as a whole Python identifier.
    fn word(line: &str, name: &str) -> bool {
        let ident = |c: char| c.is_alphanumeric() || c == '_';
        line.match_indices(name).any(|(at, _)| {
            !line[..at].chars().next_back().is_some_and(ident)
                && !line[at + name.len()..].chars().next().is_some_and(ident)
        })
    }
}
