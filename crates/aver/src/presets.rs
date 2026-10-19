//! Generic mode (ARCHITECTURE.md §1.4.1): the checks a repository without a
//! `[[check]]` list gets for the Python, Rust and Go packages in it, and for
//! Node packages below the root.
//!
//! A package is a directory, at most [`MAX_DEPTH`] levels down, that holds
//! a project file of one ecosystem: `Cargo.lock` (or a root `Cargo.toml`),
//! `go.mod`, a Python project (`pyproject.toml`, `uv.lock`, `setup.py`,
//! `setup.cfg` or a requirements file) that has tests, or a Node package
//! with its own `package-lock.json` and a `test` script. Each gets a check
//! running in its directory, in the ecosystem's pinned image, with a
//! dependency layer from its lockfile and a per-test report:
//!
//! | Ecosystem | Image | Layer | Check | Report |
//! |---|---|---|---|---|
//! | Rust | `rust` | `cargo fetch --locked` | `cargo test --workspace --no-fail-fast` | `libtest` |
//! | Go | `golang` | `go mod download` | `go vet ./...` (setup), `go test -json ./...` | `go-json` |
//! | Python | `python` | `uv sync --frozen` or `pip install -r` | `python -m pytest --junitxml` (or `unittest`, at the root only) | `pytest-junit` |
//! | Node | `node` | `npm ci` | as the root's default check | as detected |
//!
//! A Node package that lists `typescript` in its `devDependencies` beside
//! its own `tsconfig.json` also gets a `typecheck-<directory>` gate, as the
//! root does.
//!
//! Directories that hold build output, dependencies, sample projects or
//! test data ([`SKIP`], and every hidden one) are not searched, and at
//! most [`MAX_PACKAGES`] packages are taken, in path order. The root's own
//! `package.json` keeps the default `test` check, which `crate::env` adds.

use aver_core::{
    CheckDef, Digest, Image, Install, Layer, LayerKind, Lockfile, Node, Path, Report, ReportFormat,
    Tree, TreeId,
};

use serde_json::Value;

use crate::adhoc::runner::{self as runners, written_junit};
use crate::env::{default_install, Family, ObjectSource, DEFAULT_CHECK_ARGV, DEFAULT_CHECK_ID};
use crate::store::StoreError;

/// How deep below the root packages are looked for.
pub const MAX_DEPTH: usize = 3;
/// Most packages generic mode gives a check.
pub const MAX_PACKAGES: usize = 12;
/// The memory limit an environment with a Rust check gets unless
/// `aver.toml` sets one: the build output lives in the sandbox's memory.
pub const RUST_MEMORY_MB: u32 = 4096;
/// A Rust check's timeout: it compiles the workspace first.
pub const RUST_TIMEOUT_MS: u32 = 1_800_000;
/// A Go check's timeout.
pub const GO_TIMEOUT_MS: u32 = 600_000;

pub const PYTHON_IMAGE: (&str, &str) = (
    "python:3.14-slim-bookworm",
    "sha256:48b13b003dda20b16f9442b8475aa05fe21bf6579a8c881db92ffb4d8fd20f83",
);
pub const GO_IMAGE: (&str, &str) = (
    "golang:1.27-bookworm",
    "sha256:5cf287a799e6b94384bad13d16b14904c531f51ba65792237e122ce42b392f61",
);
pub const RUST_IMAGE: (&str, &str) = (
    "rust:1.99-bookworm",
    "sha256:114c7a4425406451c2866b6aafe69fe29b1b298832db1277d411ac73c82d04d6",
);

/// Directory names never searched for packages: dependencies, build output,
/// virtualenvs, test data and sample projects.
pub const SKIP: &[&str] = &[
    "node_modules",
    "vendor",
    "target",
    "dist",
    "build",
    "venv",
    "__pycache__",
    "testdata",
    "fixtures",
    "examples",
    "third_party",
    "test",
    "tests",
    "__tests__",
];

/// Whether a change to `path` can change what generic mode finds: a
/// project, lock or test runner file, a requirements file, or a Python
/// test or package file (a Python project's evidence of tests, and of a
/// `src` layout), within the depth detection searches. Workspace snapshots
/// that change none of these keep their base's environment without
/// deriving it again, so ordinary source edits stay cheap.
pub fn may_change_detection(path: &str) -> bool {
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() > MAX_DEPTH + 2 {
        return false;
    }
    let name = segments.last().copied().unwrap_or_default();
    matches!(
        name,
        "Cargo.toml"
            | "Cargo.lock"
            | "go.mod"
            | "go.sum"
            | "pyproject.toml"
            | "uv.lock"
            | "setup.py"
            | "setup.cfg"
            | "pytest.ini"
            | "tox.ini"
            | "conftest.py"
            | "noxfile.py"
            | "package.json"
            | "package-lock.json"
            | "tsconfig.json"
            | "__init__.py"
    ) || (name.ends_with(".txt") && name.contains("requirements"))
        || (name.ends_with(".py") && (name.starts_with("test_") || name.ends_with("_test.py")))
}

/// One package generic mode found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// The package's directory; `None` at the root.
    pub dir: Option<Path>,
    pub family: Family,
    pub image: Image,
    /// Its check, without `image` or `deps`, which `crate::env` sets.
    pub check: CheckDef,
    /// A Node package's type-check gate ([`typecheck_check`]), set
    /// up like `check`.
    pub typecheck: Option<CheckDef>,
    /// Its dependency layer, when it has a lockfile.
    pub layer: Option<Layer>,
}

/// An image constant as an [`Image`].
pub fn image((reference, digest): (&str, &str)) -> Image {
    Image {
        reference: reference.to_owned(),
        digest: digest.parse::<Digest>().expect("a valid digest"),
    }
}

/// One directory's entries: file blobs by name, and subdirectories.
struct Dir {
    path: String,
    files: Vec<(String, Node)>,
    dirs: Vec<(String, TreeId)>,
}

impl Dir {
    fn has(&self, name: &str) -> bool {
        self.files.iter().any(|(n, _)| n == name)
    }

    fn has_dir(&self, name: &str) -> bool {
        self.dirs.iter().any(|(n, _)| n == name)
    }

    fn rel(&self, name: &str) -> Path {
        if self.path.is_empty() {
            name.parse()
        } else {
            format!("{}/{name}", self.path).parse()
        }
        .expect("tree names make valid paths")
    }

    fn lockfile(&self, name: &str) -> Option<Lockfile> {
        self.files
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, node)| match node {
                Node::File(blob) | Node::Exec(blob) => Some(Lockfile {
                    path: self.rel(name),
                    blob: *blob,
                }),
                _ => None,
            })
    }

    fn text(&self, src: &(impl ObjectSource + ?Sized), name: &str) -> Result<String, StoreError> {
        match self.files.iter().find(|(n, _)| n == name) {
            Some((_, Node::File(blob) | Node::Exec(blob))) => {
                Ok(String::from_utf8_lossy(&src.blob(blob)?).into_owned())
            }
            _ => Ok(String::new()),
        }
    }

    fn cwd(&self) -> Option<Path> {
        (!self.path.is_empty()).then(|| self.path.parse().expect("a valid path"))
    }

    /// The check id for ecosystem `base` here: `base` at the root, else
    /// `base-<directory>` with every other character a `-`, cut to 64 bytes.
    fn id(&self, base: &str) -> String {
        if self.path.is_empty() {
            return base.to_owned();
        }
        let slug: String = self
            .path
            .to_ascii_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let mut id = format!("{base}-{slug}");
        id.truncate(64);
        id
    }
}

/// Every package below the root, and at the root unless it is only the root
/// `package.json` (whose check `crate::env` makes), in path order.
pub fn detect(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
) -> Result<Vec<Package>, StoreError> {
    let mut packages = Vec::new();
    let mut queue = vec![(String::new(), *root, 0usize)];
    while let Some((path, tree, depth)) = queue.pop() {
        let dir = read_dir(src, &path, &tree)?;
        packages.extend(packages_in(src, root, &dir)?);
        if depth < MAX_DEPTH {
            for (name, id) in dir.dirs.iter().rev() {
                if name.starts_with('.') || SKIP.contains(&name.as_str()) {
                    continue;
                }
                let child = if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path}/{name}")
                };
                queue.push((child, *id, depth + 1));
            }
        }
    }
    packages.sort_by(|a: &Package, b: &Package| {
        (a.dir.as_ref().map(Path::as_str), a.check.id.as_str())
            .cmp(&(b.dir.as_ref().map(Path::as_str), b.check.id.as_str()))
    });
    packages.truncate(MAX_PACKAGES);
    // Directories whose names differ only in characters a check id cannot
    // hold (`a-b` and `a/b`) would share an id: the later ones get `-2`,
    // `-3`, and so does a report path named after the id.
    let mut seen: Vec<String> = vec![
        crate::env::DEFAULT_CHECK_ID.to_owned(),
        TYPECHECK_CHECK_ID.to_owned(),
    ];
    for package in &mut packages {
        unique_id(&mut package.check, &mut seen);
        if let Some(gate) = &mut package.typecheck {
            unique_id(gate, &mut seen);
        }
    }
    Ok(packages)
}

/// Renames `check` to the first of its id, `<id>-2`, `<id>-3`, … not in
/// `seen`, and adds that id to `seen`.
fn unique_id(check: &mut CheckDef, seen: &mut Vec<String>) {
    let id = check.id.as_str().to_owned();
    let mut unique = id.clone();
    let mut n = 2;
    while seen.contains(&unique) {
        let suffix = format!("-{n}");
        let mut base = id.clone();
        base.truncate(64 - suffix.len());
        unique = format!("{base}{suffix}");
        n += 1;
    }
    if unique != id {
        rename(check, &id, &unique);
    }
    seen.push(unique);
}

/// Gives `check` the id `to` in place of `from`, in its report path and
/// command too, where the id names the report file.
fn rename(check: &mut CheckDef, from: &str, to: &str) {
    let old = format!("/aver/out/{from}.");
    let new = format!("/aver/out/{to}.");
    check.id = to.parse().expect("a unique id stays valid");
    for arg in &mut check.argv {
        *arg = arg.replace(&old, &new);
    }
    if let Some(report) = &mut check.report {
        report.path = report
            .path
            .as_str()
            .replace(&old, &new)
            .parse()
            .expect("a report path stays valid");
    }
}

fn read_dir(
    src: &(impl ObjectSource + ?Sized),
    path: &str,
    tree: &TreeId,
) -> Result<Dir, StoreError> {
    let tree: Tree = src.tree(tree)?;
    let mut dir = Dir {
        path: path.to_owned(),
        files: Vec::new(),
        dirs: Vec::new(),
    };
    for entry in tree.entries.iter() {
        match entry.node {
            Node::Tree(id) => dir.dirs.push((entry.name.as_str().to_owned(), id)),
            node => dir.files.push((entry.name.as_str().to_owned(), node)),
        }
    }
    Ok(dir)
}

fn packages_in(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
    dir: &Dir,
) -> Result<Vec<Package>, StoreError> {
    let mut found = Vec::new();
    if let Some(p) = rust(dir) {
        found.push(p);
    }
    if let Some(p) = go(dir) {
        found.push(p);
    }
    if let Some(p) = python(src, dir)? {
        found.push(p);
    }
    if let Some(p) = node(src, root, dir)? {
        found.push(p);
    }
    Ok(found)
}

fn layer(kind: LayerKind, lockfile: Lockfile, argv: Vec<String>) -> Layer {
    Layer {
        kind,
        lockfile,
        install: Install {
            argv,
            allow_scripts: false,
            rebuild: kind == LayerKind::Npm,
        },
    }
}

/// A check Aver detected, running `argv`: unapproved (§1.9.7), so an
/// exit-only one is no evidence, unless its constructor finds the command
/// is a script the repository declares, which a person merged.
fn detected(id: &str, argv: Vec<String>) -> CheckDef {
    CheckDef {
        approved: false,
        ..CheckDef::new(id.parse().expect("a valid check id"), argv)
    }
}

fn report(format: ReportFormat, path: String) -> Report {
    Report {
        format,
        path: path.parse().expect("a report path under /aver/out"),
    }
}

/// A Cargo workspace or package: a directory with `Cargo.lock`, or a root
/// `Cargo.toml` without one. `cargo test` builds every target first, so a
/// change that does not compile fails the check.
fn rust(dir: &Dir) -> Option<Package> {
    let lock = dir.lockfile("Cargo.lock");
    if !dir.has("Cargo.toml") || (lock.is_none() && !dir.path.is_empty()) {
        return None;
    }
    let id = dir.id("cargo");
    let locked = if lock.is_some() { " --locked" } else { "" };
    let out = format!("/aver/out/{id}.txt");
    let mut check = detected(
        &id,
        vec![
            "sh".into(),
            "-c".into(),
            format!(
                "CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test --workspace --no-fail-fast{locked} >{out} 2>&1; s=$?; cat {out}; exit $s"
            ),
        ],
    );
    check.cwd = dir.cwd();
    check.timeout_ms = Some(RUST_TIMEOUT_MS);
    check.report = Some(report(ReportFormat::Libtest, out));
    Some(Package {
        dir: dir.cwd(),
        family: Family::Rust,
        image: image(RUST_IMAGE),
        layer: lock.map(|l| layer(LayerKind::Cargo, l, default_install(LayerKind::Cargo, ""))),
        check,
        typecheck: None,
    })
}

/// A Go module: a directory with `go.mod`. `go vet` runs first as a setup
/// command, so a new vet finding fails the check's exit entry while the
/// tests still compare one by one.
fn go(dir: &Dir) -> Option<Package> {
    if !dir.has("go.mod") {
        return None;
    }
    let id = dir.id("go");
    let out = format!("/aver/out/{id}.json");
    let mut check = detected(
        &id,
        vec![
            "sh".into(),
            "-c".into(),
            format!("exec go test -json ./... >{out}"),
        ],
    );
    check.cwd = dir.cwd();
    check.timeout_ms = Some(GO_TIMEOUT_MS);
    check.setup = vec![vec!["go".into(), "vet".into(), "./...".into()]];
    check.report = Some(report(ReportFormat::GoJson, out));
    Some(Package {
        dir: dir.cwd(),
        family: Family::Go,
        image: image(GO_IMAGE),
        layer: dir
            .lockfile("go.sum")
            .map(|l| layer(LayerKind::Go, l, default_install(LayerKind::Go, ""))),
        check,
        typecheck: None,
    })
}

/// The requirements files of a directory, by name: `requirements*.txt`
/// and `*requirements.txt` (`dev-requirements.txt`).
fn requirements(dir: &Dir) -> Vec<&str> {
    let mut names: Vec<&str> = dir
        .files
        .iter()
        .map(|(n, _)| n.as_str())
        .filter(|n| n.ends_with(".txt") && n.contains("requirements"))
        .collect();
    names.sort_unstable();
    names
}

/// A Python project with tests: a project or requirements file, and a
/// `tests` or `test` directory, test files, `conftest.py` or pytest
/// configuration. pytest runs when the project names it anywhere (its
/// configuration, `conftest.py`, any dependency list, group or extra, a
/// lockfile, requirements, `tox.ini` or `noxfile.py`), else `unittest`, but
/// only at the root: a nested Python directory without pytest is more often
/// tooling than a tested package, and a `unittest` check that fails on both
/// sides has no per-test results, so it would block every verdict. No type
/// check runs: a project's type checker is configured for its own
/// invocation (`mypy` with `[tool.mypy] files`), often in a dependency
/// group the test job does not install, so a guessed one blocked good
/// changes; `aver.toml` declares one.
fn python(src: &(impl ObjectSource + ?Sized), dir: &Dir) -> Result<Option<Package>, StoreError> {
    let reqs = requirements(dir);
    let project = ["pyproject.toml", "uv.lock", "setup.py", "setup.cfg"]
        .iter()
        .any(|n| dir.has(n))
        || !reqs.is_empty();
    if !project {
        return Ok(None);
    }
    let pyproject = dir.text(src, "pyproject.toml")?;
    let setup_cfg = dir.text(src, "setup.cfg")?;
    let test_files = dir
        .files
        .iter()
        .any(|(n, _)| n.ends_with(".py") && (n.starts_with("test_") || n.ends_with("_test.py")));
    let tested = dir.has_dir("tests")
        || dir.has_dir("test")
        || test_files
        || dir.has("conftest.py")
        || dir.has("pytest.ini")
        || dir.has("tox.ini")
        || pyproject.contains("[tool.pytest")
        || setup_cfg.contains("[tool:pytest]");
    if !tested {
        return Ok(None);
    }
    let mut deps_text = pyproject.clone();
    let layer = if let Some(lock) = dir.lockfile("uv.lock") {
        deps_text.push_str(&dir.text(src, "uv.lock")?);
        Some(layer(
            LayerKind::Uv,
            lock,
            default_install(LayerKind::Uv, "uv.lock"),
        ))
    } else if !reqs.is_empty() {
        for name in &reqs {
            deps_text.push_str(&dir.text(src, name)?);
        }
        let main = reqs
            .iter()
            .find(|n| n.contains("dev") || n.contains("test"))
            .unwrap_or(&reqs[0]);
        let mut argv: Vec<String> = vec!["pip".into(), "install".into(), "--no-cache-dir".into()];
        for name in &reqs {
            argv.push("-r".into());
            argv.push((*name).to_owned());
        }
        dir.lockfile(main)
            .map(|lock| layer(LayerKind::Pip, lock, argv))
    } else {
        None
    };
    for runner in ["tox.ini", "noxfile.py"] {
        deps_text.push_str(&dir.text(src, runner)?);
    }
    let pytest = mentions(&deps_text, "pytest")
        || dir.has("pytest.ini")
        || dir.has("conftest.py")
        || pyproject.contains("[tool.pytest")
        || setup_cfg.contains("[tool:pytest]");
    if !pytest && !dir.path.is_empty() {
        return Ok(None);
    }
    let id = dir.id(if pytest { "pytest" } else { "unittest" });
    let mut argv: Vec<String> = Vec::new();
    if dir.has_dir("src") && !dir.has("__init__.py") {
        argv.extend(["env".into(), "PYTHONPATH=src".into()]);
    }
    let report_path = format!("/aver/out/{id}.xml");
    if pytest {
        argv.extend(
            [
                "python",
                "-m",
                "pytest",
                "-p",
                "no:cacheprovider",
                "--continue-on-collection-errors",
                "-o",
                "junit_family=xunit1",
            ]
            .map(String::from),
        );
        argv.push(format!("--junitxml={report_path}"));
    } else {
        argv.extend(["python", "-m", "unittest", "discover"].map(String::from));
        for tests in ["tests", "test"] {
            if dir.has_dir(tests) {
                argv.extend(["-s".into(), tests.into(), "-t".into(), ".".into()]);
                break;
            }
        }
    }
    let mut check = detected(&id, argv);
    check.cwd = dir.cwd();
    if pytest {
        check.report = Some(report(ReportFormat::PytestJunit, report_path));
    }
    Ok(Some(Package {
        dir: dir.cwd(),
        family: Family::Python,
        image: image(PYTHON_IMAGE),
        layer,
        check,
        typecheck: None,
    }))
}

/// Whether `text` names package `name` as a whole word (`pytest`, not
/// `pytest-cov` alone).
fn mentions(text: &str, name: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.';
    text.match_indices(name).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + name.len()..].chars().next();
        !before.is_some_and(word) && !after.is_some_and(word)
    })
}

/// A Node package below the root with a `test` script other than npm's
/// placeholder, and either its own `package-lock.json` (its own dependency
/// layer) or a place in the root's workspaces (`pnpm-workspace.yaml`, or
/// `workspaces` in the root `package.json`), whose install at the root
/// covers it.
fn node(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
    dir: &Dir,
) -> Result<Option<Package>, StoreError> {
    if dir.path.is_empty() || !dir.has("package.json") {
        return Ok(None);
    }
    let lock = dir.lockfile("package-lock.json");
    if lock.is_none() && !workspaces(src, root)?.holds(&dir.path) {
        return Ok(None);
    }
    let manifest = dir.text(src, "package.json")?;
    let parsed: serde_json::Value = serde_json::from_str(&manifest).unwrap_or_default();
    let test = parsed["scripts"]["test"].as_str().unwrap_or("");
    if test.is_empty() || test.contains("no test specified") {
        return Ok(None);
    }
    let lock_bytes = match &lock {
        Some(lock) => Some(src.blob(&lock.blob)?),
        None => None,
    };
    let exists = |path: &str| {
        format!("{}/{path}", dir.path)
            .parse::<Path>()
            .ok()
            .is_some_and(|p| {
                matches!(
                    crate::env::lookup(src, root, &p),
                    Ok(Some(Node::File(_) | Node::Exec(_) | Node::Symlink(_)))
                )
            })
    };
    let root_install = match lock {
        Some(_) => None,
        None => Some(workspace_root_install(src, root)?),
    };
    let id = dir.id("test");
    let mut check = package_check(
        &id,
        Some(manifest.as_bytes()),
        lock_bytes.as_deref(),
        root_install,
        &exists,
    );
    check.cwd = dir.cwd();
    let typecheck = typecheck_check(
        &dir.id(TYPECHECK_CHECK_ID),
        Some(manifest.as_bytes()),
        &exists,
    )
    .map(|mut gate| {
        gate.cwd = dir.cwd();
        gate
    });
    Ok(Some(Package {
        dir: dir.cwd(),
        family: Family::Node,
        image: crate::env::default_image(),
        layer: lock.map(|lock| {
            layer(
                LayerKind::Npm,
                lock,
                default_install(LayerKind::Npm, "package-lock.json"),
            )
        }),
        check,
        typecheck,
    }))
}

/// The root's workspace packages: directory patterns, and patterns
/// written with `!` that exclude what they cover.
#[derive(Default)]
struct Workspaces {
    include: Vec<aver_core::PathGlob>,
    exclude: Vec<aver_core::PathGlob>,
}

impl Workspaces {
    /// Whether directory `path` is a workspace package: a pattern names it
    /// (not a directory below one, unless the pattern has `**`) and no
    /// exclusion covers it.
    fn holds(&self, path: &str) -> bool {
        let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
        let named = |glob: &aver_core::PathGlob| {
            glob.covers(path)
                && (glob.as_str().split('/').any(|s| s == "**")
                    || parent.is_empty()
                    || !glob.covers(parent))
        };
        self.include.iter().any(named) && !self.exclude.iter().any(|g| g.covers(path))
    }
}

/// The versions the root's `package-lock.json` and `package.json` install,
/// which a workspace package without a lockfile of its own resolves its
/// tools from.
pub fn workspace_root_install(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
) -> Result<runners::Lockfile, StoreError> {
    let read = |name: &str| crate::env::read_file(src, root, &name.parse().expect("a valid path"));
    let manifest = read("package.json")?;
    Ok(
        runners::Lockfile::parse(read("package-lock.json")?.as_deref())
            .with_manifest(manifest.as_deref()),
    )
}

/// The root's workspaces, from `pnpm-workspace.yaml`'s `packages` list or
/// the root `package.json`'s `workspaces` (npm, Yarn, Bun).
fn workspaces(src: &(impl ObjectSource + ?Sized), root: &TreeId) -> Result<Workspaces, StoreError> {
    let read = |name: &str| -> Result<Option<String>, StoreError> {
        Ok(
            crate::env::read_file(src, root, &name.parse().expect("a valid path"))?
                .map(|b| String::from_utf8_lossy(&b).into_owned()),
        )
    };
    let mut patterns: Vec<String> = Vec::new();
    if let Some(yaml) = read("pnpm-workspace.yaml")? {
        let mut inside = false;
        for line in yaml.lines() {
            let trimmed = line.trim();
            if !line.starts_with([' ', '\t', '-']) && !trimmed.is_empty() {
                inside = trimmed.starts_with("packages:");
            } else if let Some(item) = trimmed.strip_prefix('-').filter(|_| inside) {
                let item = item.split(" #").next().unwrap_or(item).trim();
                patterns.push(item.trim_matches(['\'', '"']).to_owned());
            }
        }
    } else if let Some(manifest) = read("package.json")? {
        let parsed: Value = serde_json::from_str(&manifest).unwrap_or_default();
        let list = match &parsed["workspaces"] {
            Value::Array(list) => list.clone(),
            other => other["packages"].as_array().cloned().unwrap_or_default(),
        };
        patterns.extend(list.iter().filter_map(|v| v.as_str().map(str::to_owned)));
    }
    let mut found = Workspaces::default();
    for pattern in &patterns {
        let (list, pattern) = match pattern.strip_prefix('!') {
            Some(rest) => (&mut found.exclude, rest),
            None => (&mut found.include, pattern.as_str()),
        };
        if let Ok(glob) = pattern
            .trim_start_matches("./")
            .trim_end_matches('/')
            .parse()
        {
            list.push(glob);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_detection_inputs_make_a_workspace_derive_its_environment_again() {
        for path in [
            "Cargo.lock",
            "crates/core/Cargo.toml",
            "tools/go.mod",
            "py/requirements-dev.txt",
            "py/tests/test_api.py",
            "py/src/py/__init__.py",
            "web/package.json",
            "web/tsconfig.json",
        ] {
            assert!(may_change_detection(path), "{path}");
        }
        for path in [
            "crates/core/src/lib.rs",
            "src/index.ts",
            "README.md",
            "a/b/c/d/e/Cargo.toml",
        ] {
            assert!(!may_change_detection(path), "{path}");
        }
    }

    #[test]
    fn package_names_match_whole_words() {
        assert!(mentions("pytest==9.1.1\n", "pytest"));
        assert!(mentions("name = \"pytest\"", "pytest"));
        assert!(!mentions("pytest-cov==1\n", "pytest"));
        assert!(!mentions("mypytest", "pytest"));
    }
}

// The default Node check: what a repository without `[[check]]` runs in CI
// (`aver ci`'s zero-config mode), read from its root `package.json`. The
// check is `npm test`, with its runner's reporter when the `test` script is
// a single invocation of a runner Aver knows. Setup commands come from what
// a fresh checkout needs before its tests: the root `prepare` script, which
// `npm ci` runs in a Git checkout and the script-free install skips, and
// `build` when the package's own entry points (`main`, `bin`, `exports`)
// name files the tree does not have.

/// The timeout of a default check that has setup commands, which also have
/// to build the project: the default limit plus five minutes.
pub const SETUP_TIMEOUT_MS: u32 = 600_000;

/// The default check for a tree whose root `package.json` and lockfile are
/// `manifest` and `lockfile` (either may be missing or unreadable).
/// `exists` says whether a repository path names a file in the tree.
pub fn default_check(
    manifest: Option<&[u8]>,
    lockfile: Option<&[u8]>,
    exists: &dyn Fn(&str) -> bool,
) -> CheckDef {
    package_check(DEFAULT_CHECK_ID, manifest, lockfile, None, exists)
}

/// [`default_check`] under check id `id`, which also names its report file:
/// the check of a Node package anywhere in the tree (its `cwd` is the
/// caller's to set). `exists` takes paths relative to the package.
pub fn package_check(
    id: &str,
    manifest: Option<&[u8]>,
    lockfile: Option<&[u8]>,
    root: Option<runners::Lockfile>,
    exists: &dyn Fn(&str) -> bool,
) -> CheckDef {
    let mut check = detected(
        id,
        DEFAULT_CHECK_ARGV.iter().map(|s| (*s).to_owned()).collect(),
    );
    let Some((manifest_bytes, manifest)) =
        manifest.and_then(|m| Some((m, serde_json::from_slice::<Value>(m).ok()?)))
    else {
        return check;
    };
    let script = |name: &str| manifest["scripts"][name].as_str();
    check.approved = script("test").is_some();
    let lock = runners::Lockfile::parse(lockfile)
        .with_manifest(Some(manifest_bytes))
        .with_root(root);
    if let Ok((_, argv, report)) = runners::read(id, &manifest, &lock) {
        check.argv = argv;
        check.report = Some(report);
    } else if let Some(path) = script("test").and_then(written_junit) {
        let xml = format!("/aver/out/{id}.xml");
        check.argv = vec![
            "sh".into(),
            "-c".into(),
            format!("rm -f -- {path}; npm test; s=$?; cp -- {path} {xml} 2>/dev/null; exit $s"),
        ];
        check.report = Some(Report {
            format: ReportFormat::Junit,
            path: xml.parse().expect("a report path under /aver/out"),
        });
    }
    check.setup = setup(&manifest, exists);
    if !check.setup.is_empty() {
        check.timeout_ms = Some(SETUP_TIMEOUT_MS);
    }
    check
}

/// Why [`package_check`] left a package's `test` script to its exit code,
/// for the summary; `None` when it reads it per test or there is no
/// script to read.
pub fn exit_only(
    manifest: Option<&[u8]>,
    lockfile: Option<&[u8]>,
    root: Option<runners::Lockfile>,
) -> Option<String> {
    let parsed: Value = serde_json::from_slice(manifest?).ok()?;
    let script = parsed["scripts"]["test"].as_str()?;
    if written_junit(script).is_some() {
        return None;
    }
    let lock = runners::Lockfile::parse(lockfile)
        .with_manifest(manifest)
        .with_root(root);
    runners::read(DEFAULT_CHECK_ID, &parsed, &lock)
        .err()
        .map(|why| why.to_string())
}

/// The id of the default environment's type-check gate.
pub const TYPECHECK_CHECK_ID: &str = "typecheck";

/// The default environment's type-check gate (§1.4.1), when a Node
/// package's `package.json` lists `typescript` in its `devDependencies` and
/// the package has its own `tsconfig.json`: the project's own `typecheck`
/// script when it declares one, else `npx tsc --noEmit -p .`. Node runs
/// TypeScript tests by stripping types, so `npm test` alone never checks
/// them. Its output is read as a `tsc` report, one entry per type error
/// (§1.9.1), so only the errors a change adds count against it and a base
/// whose type check already fails does not block every verify; the command
/// runs under `sh` to keep that output as the report. `id` is
/// [`TYPECHECK_CHECK_ID`] at the root and
/// `typecheck-<directory>` for a package below it (its `cwd` is the
/// caller's to set); `exists` takes paths relative to the package.
pub fn typecheck_check(
    id: &str,
    manifest: Option<&[u8]>,
    exists: &dyn Fn(&str) -> bool,
) -> Option<CheckDef> {
    let manifest = serde_json::from_slice::<Value>(manifest?).ok()?;
    if manifest["devDependencies"].get("typescript").is_none() || !exists("tsconfig.json") {
        return None;
    }
    let own = manifest["scripts"]["typecheck"].is_string();
    let command = if own {
        "npm run typecheck"
    } else {
        "npx tsc --noEmit -p ."
    };
    let out = format!("/aver/out/{id}.txt");
    let mut check = detected(
        id,
        vec![
            "sh".into(),
            "-c".into(),
            format!("{command} > {out} 2>&1; s=$?; cat {out}; exit $s"),
        ],
    );
    check.approved = own;
    check.report = Some(aver_core::Report {
        format: aver_core::ReportFormat::Tsc,
        path: out.parse().expect("a report path under /aver/out"),
    });
    Some(check)
}

/// Whether `check` is a type-check gate [`typecheck_check`] made.
pub fn is_typecheck(check: &CheckDef) -> bool {
    let id = check.id.as_str();
    check
        .report
        .as_ref()
        .is_none_or(|r| r.format == aver_core::ReportFormat::Tsc)
        && (id == TYPECHECK_CHECK_ID
            || id
                .strip_prefix(TYPECHECK_CHECK_ID)
                .is_some_and(|rest| rest.starts_with('-')))
}

/// `npm run prepare` when the package declares it, then `npm run build`
/// when it declares one, the `prepare`, `pretest` and `test` scripts do not
/// already run it, and an entry point of the package is missing from the
/// tree.
fn setup(manifest: &Value, exists: &dyn Fn(&str) -> bool) -> Vec<Vec<String>> {
    let script = |name: &str| manifest["scripts"][name].as_str();
    let run = |name: &str| vec!["npm".to_owned(), "run".to_owned(), name.to_owned()];
    let mut steps = Vec::new();
    if script("prepare").is_some() {
        steps.push(run("prepare"));
    }
    let builds_already = ["prepare", "pretest", "test"]
        .iter()
        .filter_map(|s| script(s))
        .any(mentions_build);
    let entry_missing = entry_points(manifest).iter().any(|p| !exists(p));
    if script("build").is_some() && !builds_already && entry_missing {
        steps.push(run("build"));
    }
    steps
}

/// Whether a script runs the `build` script (any word `build` on its own,
/// as in `npm run build` or `run-s build test`).
fn mentions_build(script: &str) -> bool {
    script
        .split(|c: char| c.is_whitespace() || "&|;()'\"".contains(c))
        .any(|word| word == "build")
}

/// The repository paths the package names as its own entry points: `main`,
/// `module`, every `bin` and the strings under `exports["."]` (or `exports`
/// itself), without a leading `./`. Patterns (with `*`) are skipped.
fn entry_points(manifest: &Value) -> Vec<String> {
    fn strings(value: &Value, depth: usize, out: &mut Vec<String>) {
        match value {
            Value::String(s) => out.push(s.clone()),
            Value::Object(map) if depth < 4 => {
                for v in map.values() {
                    strings(v, depth + 1, out);
                }
            }
            _ => {}
        }
    }
    let mut raw = Vec::new();
    for key in ["main", "module", "bin"] {
        strings(&manifest[key], 1, &mut raw);
    }
    match &manifest["exports"] {
        Value::Object(map) if map.keys().any(|k| k.starts_with('.')) => {
            if let Some(root) = map.get(".") {
                strings(root, 1, &mut raw);
            }
        }
        other => strings(other, 1, &mut raw),
    }
    raw.into_iter()
        .map(|p| p.trim_start_matches("./").to_owned())
        .filter(|p| {
            !p.is_empty()
                && !p.contains('*')
                && !p.starts_with('/')
                && !p.split('/').any(|s| s == ".." || s == ".")
        })
        .collect()
}

#[cfg(test)]
mod node_tests {
    use super::*;
    use aver_core::ReportFormat;

    fn manifest(json: Value) -> Vec<u8> {
        serde_json::to_vec(&json).unwrap()
    }

    #[test]
    fn the_default_check_adds_the_runner_reporter() {
        let lockfile = serde_json::to_vec(&serde_json::json!({
            "lockfileVersion": 3,
            "packages": { "node_modules/jest": { "version": "29.7.0" } }
        }))
        .unwrap();
        let m = manifest(serde_json::json!({ "scripts": { "test": "jest" } }));
        let check = default_check(Some(&m), Some(&lockfile), &|_| true);
        assert_eq!(
            check.argv,
            [
                "npm",
                "test",
                "--",
                "--json",
                "--outputFile=/aver/out/test.json"
            ]
        );
        let report = check.report.unwrap();
        assert_eq!(report.format, ReportFormat::JestJson);
        assert_eq!(report.path.as_str(), "/aver/out/test.json");
        assert!(check.setup.is_empty());
        assert_eq!(check.timeout_ms, None);

        let m = manifest(serde_json::json!({ "scripts": { "test": "node --test" } }));
        let check = default_check(Some(&m), None, &|_| true);
        assert_eq!(check.argv[..2], ["npm", "test"]);
        assert!(check.argv[2].starts_with("--node-options=--test-reporter=spec "));
        assert!(check.argv[2].ends_with("--test-reporter-destination=/aver/out/test.xml"));

        let plain = default_check(None, None, &|_| true);
        assert_eq!(plain.argv, ["npm", "test"]);
        assert!(plain.report.is_none());
        let custom = manifest(serde_json::json!({ "scripts": { "test": "node run-tests.js" } }));
        assert!(default_check(Some(&custom), None, &|_| true)
            .report
            .is_none());
        assert!(default_check(Some(b"{not json"), None, &|_| true)
            .report
            .is_none());
    }

    #[test]
    fn typescript_projects_get_a_typecheck_gate() {
        let tsconfig = |p: &str| p == "tsconfig.json";
        let ts = manifest(serde_json::json!({
            "scripts": { "test": "node --test" },
            "devDependencies": { "typescript": "^5.6.0" }
        }));
        let check =
            typecheck_check(TYPECHECK_CHECK_ID, Some(&ts), &tsconfig).expect("a typecheck check");
        assert_eq!(check.id.as_str(), "typecheck");
        assert_eq!(
            check.argv[2],
            "npx tsc --noEmit -p . > /aver/out/typecheck.txt 2>&1; s=$?; cat /aver/out/typecheck.txt; exit $s"
        );
        assert_eq!(
            check.report.as_ref().map(|r| r.format),
            Some(aver_core::ReportFormat::Tsc),
            "one entry per type error"
        );
        assert!(is_typecheck(&check));

        let scripted = manifest(serde_json::json!({
            "scripts": { "test": "node --test", "typecheck": "tsc -b" },
            "devDependencies": { "typescript": "^5.6.0" }
        }));
        assert_eq!(
            typecheck_check(TYPECHECK_CHECK_ID, Some(&scripted), &tsconfig)
                .unwrap()
                .argv[2],
            "npm run typecheck > /aver/out/typecheck.txt 2>&1; s=$?; cat /aver/out/typecheck.txt; exit $s"
        );

        assert!(
            typecheck_check(TYPECHECK_CHECK_ID, Some(&ts), &|_| false).is_none(),
            "no tsconfig.json"
        );
        let runtime = manifest(serde_json::json!({ "dependencies": { "typescript": "5" } }));
        assert!(
            typecheck_check(TYPECHECK_CHECK_ID, Some(&runtime), &tsconfig).is_none(),
            "only a dev dependency"
        );
        assert!(typecheck_check(TYPECHECK_CHECK_ID, None, &tsconfig).is_none());
        assert!(typecheck_check(TYPECHECK_CHECK_ID, Some(b"{not json"), &tsconfig).is_none());
    }

    #[test]
    fn setup_runs_prepare_and_a_build_whose_output_the_package_points_at() {
        let exists = |p: &str| p == "src/index.ts";
        let m = manifest(serde_json::json!({
            "main": "dist/index.js",
            "scripts": { "test": "node --test", "build": "tsc" }
        }));
        let check = default_check(Some(&m), None, &exists);
        assert_eq!(check.setup, [["npm", "run", "build"]]);
        assert_eq!(check.timeout_ms, Some(SETUP_TIMEOUT_MS));

        let m = manifest(serde_json::json!({
            "main": "dist/index.js",
            "scripts": { "test": "node --test", "build": "tsc", "prepare": "husky" }
        }));
        assert_eq!(
            default_check(Some(&m), None, &exists).setup,
            [["npm", "run", "prepare"], ["npm", "run", "build"]]
        );

        let m = manifest(serde_json::json!({
            "main": "dist/index.js",
            "scripts": { "test": "node --test", "build": "tsc", "prepare": "npm run build" }
        }));
        assert_eq!(
            default_check(Some(&m), None, &exists).setup,
            [["npm", "run", "prepare"]],
            "prepare already builds"
        );

        let m = manifest(serde_json::json!({
            "main": "src/index.ts",
            "scripts": { "test": "node --test", "build": "tsc" }
        }));
        assert!(
            default_check(Some(&m), None, &exists).setup.is_empty(),
            "the entry point is in the tree"
        );

        let m = manifest(serde_json::json!({
            "exports": { ".": { "types": "./types/index.d.ts", "import": "./dist/module/index.mjs" }, "./styles": "./styles/x.css" },
            "scripts": { "test": "npm run build && node --test", "build": "rollup -c" }
        }));
        assert!(
            default_check(Some(&m), None, &exists).setup.is_empty(),
            "the test script already builds"
        );
        let m = manifest(serde_json::json!({
            "bin": { "x": "dist/bin/x.js" },
            "scripts": { "test": "node --test", "build": "tsc" }
        }));
        assert_eq!(
            default_check(Some(&m), None, &exists).setup,
            [["npm", "run", "build"]]
        );
    }

    #[test]
    fn a_junit_file_the_script_already_writes_is_collected() {
        let m = manifest(serde_json::json!({ "scripts": {
            "test": "vitest --run --reporter=default --reporter=junit --outputFile=./junit.xml"
        } }));
        let check = default_check(Some(&m), None, &|_| true);
        assert_eq!(
            check.argv,
            [
                "sh",
                "-c",
                "rm -f -- junit.xml; npm test; s=$?; cp -- junit.xml /aver/out/test.xml 2>/dev/null; exit $s"
            ]
        );
        assert_eq!(check.report.unwrap().path.as_str(), "/aver/out/test.xml");
        for script in [
            "vitest --reporter=junit --outputFile=../x.xml",
            "vitest --reporter=junit --outputFile=/tmp/x.xml",
            "vitest --reporter=json --outputFile=out.json",
            "vitest --reporter=junit --outputFile='a b.xml'",
            "jest --reporters=jest-junit",
        ] {
            assert_eq!(written_junit(script), None, "{script}");
        }
        assert_eq!(
            written_junit("vitest run --reporter junit --outputFile.junit=reports/j.xml")
                .as_deref(),
            Some("reports/j.xml")
        );
    }

    #[test]
    fn build_is_a_word_of_its_own() {
        assert!(mentions_build("npm run build && jest"));
        assert!(!mentions_build("npm run build:esm"));
        assert!(!mentions_build("tsc --build"));
    }
}
