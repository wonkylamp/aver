//! On-disk workspaces (ARCHITECTURE.md §2.3.1): materialize, sync-on-read,
//! skips, restore, release and gc, through the workspace service. No
//! Docker; `verify_workspace_mode_scripted` uses the scripted sandbox.

mod common;
mod scripted;
mod world;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use aver::disk::{CloneStrategy, DiskConfig, GcPolicy, MatState, SkipReason};
use aver::env::StaticResolver;
use aver::keys::{self, AgentKey, Home};
use aver::store::Store;
use aver::workspace::{EditOp, EditRequest, MaterializeOptions, WorkspaceError, Workspaces};
use aver_core::*;
use tempfile::TempDir;

struct Fx {
    dir: TempDir,
    home: PathBuf,
    store: Arc<Store>,
    ws: Workspaces,
    admin: AgentKey,
}

impl Fx {
    fn new() -> Self {
        Self::with(|_| {})
    }

    fn with(configure: impl FnOnce(&mut DiskConfig)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(dir.path()).unwrap().join("home");
        let init = keys::init(&Home::new(&home)).unwrap();
        let store = Arc::new(Home::new(&home).open_store().unwrap());
        let mut config = DiskConfig::under(&home);
        config.global_excludes = false;
        configure(&mut config);
        let ws = Workspaces::new(
            store.clone(),
            Arc::new(init.server.signing_key.clone()),
            Arc::new(aver::env::TreeEnv(
                Arc::new(StaticResolver::default()),
                aver::env::Detection::Off,
            )),
        )
        .with_disk_config(config);
        Self {
            dir,
            home,
            store,
            ws,
            admin: init.admin,
        }
    }

    fn base(&self, files: &[(&str, &str)]) -> SnapshotId {
        self.base_of(common::put_files(&self.store, files))
    }

    fn base_of(&self, root: TreeId) -> SnapshotId {
        let env = aver::env::environment(
            &*self.store,
            &root,
            &StaticResolver::default(),
            aver::env::Detection::Off,
        )
        .unwrap();
        self.store.put(&env).unwrap();
        self.store.put(&Snapshot::new(root, env.id())).unwrap()
    }

    fn open(&self, base: SnapshotId) -> (WorkspaceId, PathBuf) {
        self.open_with(
            base,
            MaterializeOptions {
                deps: false,
                ..Default::default()
            },
        )
    }

    fn open_with(&self, base: SnapshotId, opts: MaterializeOptions) -> (WorkspaceId, PathBuf) {
        let ws = self.ws.open(&self.admin.id(), &base).unwrap();
        let m = self.ws.materialize(&ws.id, &opts).unwrap();
        (ws.id, m.dir)
    }

    fn me(&self) -> AgentId {
        self.admin.id()
    }

    fn root(&self, id: &WorkspaceId) -> TreeId {
        let snap = self.ws.snapshot(&self.me(), id).unwrap();
        self.ws.get_snapshot(&snap).unwrap().root
    }

    fn file(&self, id: &WorkspaceId, path: &str) -> Option<String> {
        let snap = self.ws.snapshot(&self.me(), id).unwrap();
        match self.ws.read_snapshot(&snap, &path.parse().unwrap()) {
            Ok(read) => Some(String::from_utf8(read.bytes).unwrap()),
            Err(_) => None,
        }
    }

    fn sync(&self, id: &WorkspaceId) -> aver::disk::SyncReport {
        self.ws.sync(&self.me(), id).unwrap().unwrap()
    }
}

/// The tree a directory holds as a workspace sees it: files, executables and
/// symlinks, skipping `.aver`, `node_modules`, `.git` and empty directories.
fn tree_of(store: &Store, dir: &FsPath, top: bool) -> Option<TreeId> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        if name == "node_modules" || name == ".git" || (top && name == ".aver") {
            continue;
        }
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).unwrap();
        let node = if meta.file_type().is_symlink() {
            let target = fs::read_link(&path).unwrap();
            Node::Symlink(store.put_blob(target.to_str().unwrap().as_bytes()).unwrap())
        } else if meta.is_dir() {
            match tree_of(store, &path, false) {
                Some(id) => Node::Tree(id),
                None => continue,
            }
        } else {
            let blob = store.put_blob(&fs::read(&path).unwrap()).unwrap();
            if meta.mode() & 0o111 != 0 {
                Node::Exec(blob)
            } else {
                Node::File(blob)
            }
        };
        entries.push(TreeEntry {
            name: name.parse().unwrap(),
            node,
        });
    }
    if entries.is_empty() && !top {
        return None;
    }
    Some(store.put(&Tree::new(Set::new(entries).unwrap())).unwrap())
}

/// A tree of regular files built directly in the store.
fn tree_from(store: &Store, files: &[(&str, &str)]) -> TreeId {
    let mut here: BTreeMap<String, Node> = BTreeMap::new();
    let mut subdirs: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (path, text) in files {
        match path.split_once('/') {
            Some((dir, rest)) => subdirs
                .entry(dir.to_owned())
                .or_default()
                .push((rest.to_owned(), text.to_string())),
            None => {
                here.insert(
                    path.to_string(),
                    Node::File(store.put_blob(text.as_bytes()).unwrap()),
                );
            }
        }
    }
    for (dir, children) in subdirs {
        let children: Vec<(&str, &str)> = children
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect();
        here.insert(dir, Node::Tree(tree_from(store, &children)));
    }
    let entries = here
        .into_iter()
        .map(|(name, node)| TreeEntry {
            name: name.parse().unwrap(),
            node,
        })
        .collect();
    store.put(&Tree::new(Set::new(entries).unwrap())).unwrap()
}

const BASE: &[(&str, &str)] = &[
    ("README.md", "# app\n"),
    ("src/a.ts", "export const a = 1;\n"),
    ("src/b.ts", "export const b = 2;\n"),
    ("src/deep/c.ts", "export const c = 3;\n"),
    ("test/a.test.ts", "test('a', () => {});\n"),
];

#[test]
fn materialize_then_scan_is_base() {
    let fx = Fx::new();
    let base = fx.base(BASE);
    let (id, dir) = fx.open(base);
    assert!(dir.starts_with(fx.home.join("work")));
    assert_eq!(
        fs::read_to_string(dir.join("src/a.ts")).unwrap(),
        "export const a = 1;\n"
    );
    assert!(dir.join(".aver/workspace.toml").is_file());
    let report = fx.sync(&id);
    assert_eq!(
        (report.version, report.updated, report.stats.hashed),
        (0, 0, 0)
    );
    assert!(report.skipped.is_empty());
    assert_eq!(fx.ws.snapshot(&fx.me(), &id).unwrap(), base);
    let ws = fx.ws.get(&fx.me(), &id).unwrap();
    assert_eq!(ws.mode, aver::workspace::WorkspaceMode::Disk);
}

#[test]
fn edit_add_delete_chmod_round_trip() {
    let fx = Fx::new();
    let (id, dir) = fx.open(fx.base(BASE));
    fs::write(dir.join("src/a.ts"), "export const a = 10;\n").unwrap();
    fs::create_dir_all(dir.join("src/new")).unwrap();
    fs::write(dir.join("src/new/d.ts"), "export const d = 4;\n").unwrap();
    fs::remove_file(dir.join("src/b.ts")).unwrap();
    fs::remove_dir_all(dir.join("src/deep")).unwrap();
    fs::set_permissions(dir.join("README.md"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("src/a.ts", dir.join("link")).unwrap();

    let report = fx.sync(&id);
    assert_eq!(report.version, 1);
    assert_eq!((report.added, report.modified, report.deleted), (2, 2, 2));
    assert_eq!(fx.root(&id), tree_of(&fx.store, &dir, true).unwrap());
    let (_, diffs) = fx.ws.diff(&fx.me(), &id).unwrap();
    assert_eq!(diffs.len(), 6);
}

#[test]
fn sync_is_idempotent() {
    let fx = Fx::new();
    let (id, dir) = fx.open(fx.base(BASE));
    fs::write(dir.join("src/a.ts"), "changed\n").unwrap();
    let first = fx.sync(&id);
    assert_eq!((first.version, first.updated), (1, 1));
    let snap = fx.ws.snapshot(&fx.me(), &id).unwrap();
    let again = fx.sync(&id);
    assert_eq!(
        (again.version, again.updated),
        (1, 0),
        "an unchanged directory writes nothing"
    );
    assert_eq!(fx.ws.snapshot(&fx.me(), &id).unwrap(), snap);
    assert_eq!(fx.ws.get(&fx.me(), &id).unwrap().version, 1);
}

#[test]
fn tracked_ignored_file_still_synced() {
    let fx = Fx::new();
    let (id, dir) = fx.open(fx.base(&[(".gitignore", "*.log\n"), ("keep.log", "v1\n")]));
    fs::write(dir.join("keep.log"), "v2\n").unwrap();
    fx.sync(&id);
    assert_eq!(fx.file(&id, "keep.log").as_deref(), Some("v2\n"));
}

#[test]
fn untracked_ignored_file_skipped() {
    let fx = Fx::new();
    let (id, dir) = fx.open(fx.base(&[
        (".gitignore", "*.log\ndist/\n"),
        ("src/a.ts", "a\n"),
        ("src/.gitignore", "gen/\n!keep.log\n"),
    ]));
    fs::write(dir.join("debug.log"), "noise").unwrap();
    fs::create_dir_all(dir.join("dist")).unwrap();
    fs::write(dir.join("dist/bundle.js"), "x").unwrap();
    fs::create_dir_all(dir.join("src/gen")).unwrap();
    fs::write(dir.join("src/gen/out.ts"), "x").unwrap();
    fs::write(dir.join("src/keep.log"), "kept by negation").unwrap();
    fs::write(dir.join("src/new.ts"), "new").unwrap();
    fs::write(dir.join(".DS_Store"), "finder").unwrap();
    fs::write(dir.join("src/._a.ts"), "appledouble").unwrap();
    let report = fx.sync(&id);
    assert!(
        report.skipped.is_empty(),
        "ignored paths are not reported: {:?}",
        report.skipped
    );
    assert_eq!(report.added, 2);
    assert_eq!(fx.file(&id, "src/new.ts").as_deref(), Some("new"));
    assert_eq!(
        fx.file(&id, "src/keep.log").as_deref(),
        Some("kept by negation")
    );
    for gone in ["debug.log", "dist/bundle.js", "src/gen/out.ts"] {
        assert_eq!(fx.file(&id, gone), None, "{gone}");
    }
}

#[test]
fn secret_file_skipped_keeps_last_value() {
    let fx = Fx::new();
    let (id, dir) = fx.open(fx.base(BASE));
    fs::write(dir.join("src/a.ts"), "export const a = 'v2';\n").unwrap();
    fx.sync(&id);
    let key = format!("export const k = '{}{}';\n", "AKIA", "ABCDEFGHIJKLMNOP");
    fs::write(dir.join("src/a.ts"), key).unwrap();
    fs::write(dir.join(".env"), "DB_PASSWORD=hunter2\n").unwrap();
    let report = fx.sync(&id);
    let reasons: BTreeMap<&str, &SkipReason> = report
        .skipped
        .iter()
        .map(|s| (s.path.as_str(), &s.reason))
        .collect();
    assert_eq!(
        reasons.get("src/a.ts"),
        Some(&&SkipReason::Secret {
            rule: "aws-access-key-id".into()
        })
    );
    assert_eq!(
        reasons.get(".env"),
        Some(&&SkipReason::Secret {
            rule: "dotenv-file".into()
        })
    );
    let tracked: BTreeMap<&str, bool> = report
        .skipped
        .iter()
        .map(|s| (s.path.as_str(), s.tracked))
        .collect();
    assert_eq!(
        tracked,
        BTreeMap::from([(".env", false), ("src/a.ts", true)])
    );
    assert_eq!(
        fx.file(&id, "src/a.ts").as_deref(),
        Some("export const a = 'v2';\n")
    );
    assert_eq!(fx.file(&id, ".env"), None);
    assert_eq!(report.version, 1, "a skip changes nothing");
}

#[test]
fn a_base_file_holding_a_secret_syncs_when_hashed_again() {
    let fx = Fx::new();
    let pem = format!("-----BEGIN RSA {}-----\nabc\n", "PRIVATE KEY");
    let mut files = BASE.to_vec();
    files.push(("tests/fixtures/key.pem", &pem));
    let (id, dir) = fx.open(fx.base(&files));
    // Same bytes, new stat: the sync reads and hashes the file again.
    fs::remove_file(dir.join("tests/fixtures/key.pem")).unwrap();
    fs::write(dir.join("tests/fixtures/key.pem"), &pem).unwrap();
    let report = fx.sync(&id);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(
        fx.file(&id, "tests/fixtures/key.pem").as_deref(),
        Some(pem.as_str())
    );
}

#[test]
fn symlink_escape_skipped() {
    let fx = Fx::new();
    let (id, dir) = fx.open(fx.base(BASE));
    symlink("/etc/hosts", dir.join("abs")).unwrap();
    symlink("../../outside", dir.join("src/up")).unwrap();
    symlink("../README.md", dir.join("src/readme")).unwrap();
    let report = fx.sync(&id);
    let skipped: Vec<(&str, &SkipReason)> = report
        .skipped
        .iter()
        .map(|s| (s.path.as_str(), &s.reason))
        .collect();
    assert_eq!(
        skipped,
        [
            ("abs", &SkipReason::SymlinkEscapes),
            ("src/up", &SkipReason::SymlinkEscapes)
        ]
    );
    let snap = fx.ws.snapshot(&fx.me(), &id).unwrap();
    let link = fx
        .ws
        .file_in_snapshot(&snap, &"src/readme".parse().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(link.kind, aver::workspace::FileKind::Symlink);
}

#[test]
fn special_file_skipped() {
    let fx = Fx::new();
    let (id, dir) = fx.open(fx.base(BASE));
    let fifo = std::ffi::CString::new(dir.join("pipe").to_str().unwrap()).unwrap();
    // SAFETY: a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
    let report = fx.sync(&id);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].path, "pipe");
    assert_eq!(report.skipped[0].reason, SkipReason::Special);
}

#[test]
fn too_large_skipped() {
    let fx = Fx::with(|c| c.max_file_bytes = 1024);
    let (id, dir) = fx.open(fx.base(BASE));
    fs::write(dir.join("big.bin"), vec![b'x'; 2048]).unwrap();
    fs::write(dir.join("small.bin"), vec![b'x'; 100]).unwrap();
    let report = fx.sync(&id);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(
        report.skipped[0].reason,
        SkipReason::TooLarge { bytes: 2048 }
    );
    assert!(fx.file(&id, "small.bin").is_some());
    assert!(fx.file(&id, "big.bin").is_none());
}

#[test]
fn absent_paths_not_deleted() {
    let fx = Fx::with(|c| c.case_insensitive = Some(true));
    // Built without the filesystem, which may itself fold case.
    let base = fx.base_of(tree_from(
        &fx.store,
        &[
            ("README.md", "upper\n"),
            ("readme.md", "lower\n"),
            ("Src/x.ts", "x\n"),
            ("src/y.ts", "y\n"),
        ],
    ));
    let ws = fx.ws.open(&fx.me(), &base).unwrap();
    let m = fx
        .ws
        .materialize(
            &ws.id,
            &MaterializeOptions {
                deps: false,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(m.absent, ["readme.md", "src/y.ts"]);
    assert!(!m.dir.join("src").exists() || cfg!(target_os = "macos"));
    let report = fx.sync(&ws.id);
    assert_eq!((report.deleted, report.updated), (0, 0));
    assert_eq!(fx.ws.snapshot(&fx.me(), &ws.id).unwrap(), base);
}

#[test]
fn seeded_dependencies_and_paths_are_never_synced() {
    let fx = Fx::new();
    let repo = fx.dir.path().join("repo");
    let (base, _) = node_repo(
        &fx,
        &repo,
        "[workspace]\nseed = [\".env.development.local\", \"cache\"]\nwarm = [\"target\"]\n",
    );
    fs::write(repo.join(".env.development.local"), "TOKEN=abc\n").unwrap();
    fs::create_dir_all(repo.join("target/debug")).unwrap();
    fs::write(repo.join("target/debug/build.o"), "obj").unwrap();
    let ws = fx.ws.open(&fx.me(), &base).unwrap();
    let m = fx
        .ws
        .materialize(
            &ws.id,
            &MaterializeOptions {
                repo: Some(repo.clone()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        m.seeded,
        ["node_modules", ".env.development.local", "target"],
        "the installed dependencies, then aver.toml's paths that exist"
    );
    assert!(m.dir.join("node_modules/pkg/index.js").is_file());
    assert_eq!(
        fs::read_to_string(m.dir.join(".env.development.local")).unwrap(),
        "TOKEN=abc\n"
    );
    assert!(m.dir.join("target/debug/build.o").is_file());
    fs::write(m.dir.join("target/debug/more.o"), "obj").unwrap();
    fs::create_dir_all(m.dir.join("node_modules/.vite")).unwrap();
    fs::write(m.dir.join("node_modules/.vite/cache.json"), "{}").unwrap();
    let report = fx.sync(&ws.id);
    assert_eq!((report.updated, report.version), (0, 0));
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert_eq!(fx.ws.snapshot(&fx.me(), &ws.id).unwrap(), base);
}

/// Writes `files` (path, content) under `dir`.
fn write_all(dir: &FsPath, files: &[(&str, &str)]) {
    for (path, text) in files {
        let full = dir.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, text).unwrap();
    }
}

/// Every file under `dir`, relative to it, sorted.
fn files_under(dir: &FsPath) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).unwrap().flatten() {
            if entry.file_type().unwrap().is_dir() {
                stack.push(entry.path());
            } else {
                let rel = entry.path().strip_prefix(dir).unwrap().to_owned();
                found.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    found.sort();
    found
}

/// Sets the modification time of every file under `dir` to `when`.
fn age_files(dir: &FsPath, when: std::time::SystemTime) {
    for rel in files_under(dir) {
        let path = dir.join(rel);
        if !fs::symlink_metadata(&path).unwrap().is_symlink() {
            fs::File::open(&path).unwrap().set_modified(when).unwrap();
        }
    }
}

/// The newest modification time of a file under `dir`.
fn newest(dir: &FsPath) -> std::time::SystemTime {
    files_under(dir)
        .iter()
        .map(|rel| fs::metadata(dir.join(rel)).unwrap().modified().unwrap())
        .max()
        .unwrap()
}

/// A seeded Cargo `target` is cloned whole, and every tracked file whose
/// bytes differ from the user's checkout is newer than anything in it, so
/// Cargo rebuilds the units built from the user's content. Defect caught:
/// a checkout file cached before the user's last build keeps its old
/// mtime and the user's unit, built from different content, reads as
/// fresh (round 12). A file equal to the user's keeps the user's mtime, so
/// its units stay fresh. `aver gc` reclaims the target and the check copy
/// once the workspace is idle.
#[test]
fn a_seeded_target_rebuilds_every_file_that_differs_from_the_users_checkout_and_gc_reclaims_it() {
    let fx = Fx::new();
    let repo = fx.dir.path().join("repo");
    let toml = format!(
        "check = []\n\n[env]\nimage = \"{}\"\n\n[workspace]\nwarm = [\"target\"]\n",
        world::PINNED_IMAGE
    );
    let tracked: Vec<(&str, &str)> = vec![
        ("aver.toml", &toml),
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"app\", \"app-core\"]\n",
        ),
        ("Cargo.lock", "version = 4\n"),
        ("app/Cargo.toml", "[package]\nname = \"app\"\n"),
        ("app/src/lib.rs", "pub fn app() {}\n"),
        ("app-core/Cargo.toml", "[package]\nname = \"app-core\"\n"),
        ("app-core/src/lib.rs", "pub fn core() {}\n"),
    ];
    let base = fx.base(&tracked);
    write_all(&repo, &tracked);
    write_all(&repo, &[("app/src/lib.rs", "pub fn app() { edited() }\n")]);
    let target = [
        (
            "target/CACHEDIR.TAG",
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        ),
        (
            "target/debug/.fingerprint/app-1111111111111111/dep-lib-app",
            "dep",
        ),
        ("target/debug/deps/libapp-1111111111111111.rlib", "rlib"),
        (
            "target/debug/.fingerprint/serde-0123456789abcdef/lib-serde",
            "fp",
        ),
        ("target/debug/deps/libserde-0123456789abcdef.rlib", "rlib"),
        (
            "target/debug/incremental/app-xyz/s-1/query-cache.bin",
            "inc",
        ),
    ];
    write_all(&repo, &target);
    let hour = std::time::Duration::from_secs(3600);
    let now = std::time::SystemTime::now();
    age_files(&repo, now - 3 * hour);
    age_files(&repo.join("target"), now - hour);

    // A first workspace caches the checkout; it is then older than the
    // user's build, as a checkout cached before the user built is.
    let (first, _) = fx.open(base);
    fx.ws.release(&first, true).unwrap();
    age_files(&fx.home.join("checkouts"), now - 2 * hour);
    let (ws, dir) = fx.open_with(
        base,
        MaterializeOptions {
            repo: Some(repo.clone()),
            deps: false,
            ..Default::default()
        },
    );
    assert_eq!(
        files_under(&dir.join("target")),
        files_under(&repo.join("target"))
    );
    let built = newest(&dir.join("target"));
    let mtime = |p: &FsPath| fs::metadata(p).unwrap().modified().unwrap();
    assert!(
        mtime(&dir.join("app/src/lib.rs")) > built,
        "a file that differs from the user's is newer than every unit"
    );
    assert_eq!(
        mtime(&dir.join("app-core/src/lib.rs")),
        mtime(&repo.join("app-core/src/lib.rs")),
        "a file equal to the user's keeps the user's mtime"
    );
    assert_eq!(fx.sync(&ws).version, 0, "the target is never synced");

    let keep = fx.ws.disk().gc(&GcPolicy::default()).unwrap();
    assert_eq!(
        keep.builds_reclaimed, 0,
        "a fresh workspace keeps its builds"
    );
    let reclaimed = fx
        .ws
        .disk()
        .gc(&GcPolicy {
            build_idle: std::time::Duration::ZERO,
            ..GcPolicy::default()
        })
        .unwrap();
    assert_eq!(reclaimed.builds_reclaimed, 1, "the target");
    assert!(reclaimed.build_bytes > 0);
    assert!(!dir.join("target").exists());
    assert!(dir.join("app/src/lib.rs").is_file());
    assert_eq!(fx.sync(&ws).version, 0);
}

/// A base with an npm lockfile and a repo whose `node_modules` matches it,
/// with a scoped package, a `.bin` link and an npm workspace link.
fn node_repo(fx: &Fx, repo: &FsPath, workspace_toml: &str) -> (SnapshotId, String) {
    let lock = serde_json::json!({
        "name": "app", "lockfileVersion": 3, "requires": true,
        "packages": {
            "": {"name": "app", "workspaces": ["packages/member"]},
            "node_modules/pkg": {"version": "1.0.0", "resolved": "https://registry.example/pkg-1.0.0.tgz", "integrity": "sha512-pkg"},
            "node_modules/@scope/inner": {"version": "2.0.0", "resolved": "https://registry.example/inner-2.0.0.tgz", "integrity": "sha512-inner"},
            "node_modules/@esbuild/other-os": {"version": "0.1.0", "optional": true},
            "node_modules/member": {"resolved": "packages/member", "link": true},
            "packages/member": {"version": "0.0.1"}
        }
    });
    let lock_text = serde_json::to_string_pretty(&lock).unwrap();
    let toml = format!(
        "[env]\nimage = \"{}\"\n\n{workspace_toml}",
        world::PINNED_IMAGE
    );
    let files: Vec<(&str, &str)> = vec![
        ("aver.toml", &toml),
        (
            "package.json",
            "{\"name\":\"app\",\"workspaces\":[\"packages/member\"]}\n",
        ),
        ("package-lock.json", &lock_text),
        (
            "packages/member/package.json",
            "{\"name\":\"member\",\"version\":\"0.0.1\"}\n",
        ),
        ("packages/member/index.js", "module.exports = 'member';\n"),
        ("src/index.js", "require('pkg');\n"),
    ];
    for (path, text) in &files {
        let full = repo.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, text).unwrap();
    }
    let nm = repo.join("node_modules");
    fs::create_dir_all(nm.join("pkg")).unwrap();
    fs::write(nm.join("pkg/index.js"), "module.exports = 'pkg';\n").unwrap();
    fs::create_dir_all(nm.join("@scope/inner")).unwrap();
    fs::write(
        nm.join("@scope/inner/index.js"),
        "module.exports = 'inner';\n",
    )
    .unwrap();
    fs::create_dir_all(nm.join(".bin")).unwrap();
    symlink("../pkg/index.js", nm.join(".bin/tool")).unwrap();
    symlink("../packages/member", nm.join("member")).unwrap();
    let mut hidden = lock.clone();
    let packages = hidden["packages"].as_object_mut().unwrap();
    packages.remove("");
    packages.remove("node_modules/@esbuild/other-os");
    fs::write(
        nm.join(".package-lock.json"),
        serde_json::to_string(&hidden).unwrap(),
    )
    .unwrap();
    let base = fx.base(&files);
    (base, lock_text)
}

#[test]
fn stat_cache_written_only_after_commit() {
    let fx = Fx::new();
    let (id, dir) = fx.open(fx.base(BASE));
    let stat = fx.home.join("mat").join(id.to_string()).join("stat");
    let before = fs::read(&stat).unwrap();
    fs::write(
        dir.join("src/a.ts"),
        "changed while the database refuses writes\n",
    )
    .unwrap();
    let conn = rusqlite::Connection::open(fx.home.join("aver.db")).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER refuse BEFORE INSERT ON overlay BEGIN SELECT RAISE(ABORT, 'injected'); END;",
    )
    .unwrap();
    assert!(fx.ws.sync(&fx.me(), &id).is_err());
    assert_eq!(
        fs::read(&stat).unwrap(),
        before,
        "a failed sync leaves the stat cache alone"
    );
    conn.execute_batch("DROP TRIGGER refuse;").unwrap();
    let report = fx.sync(&id);
    assert_eq!(report.version, 1);
    assert_eq!(
        fx.file(&id, "src/a.ts").as_deref(),
        Some("changed while the database refuses writes\n")
    );
}

#[test]
fn edit_api_refused_in_disk_mode() {
    let fx = Fx::new();
    let (id, dir) = fx.open(fx.base(BASE));
    let op: EditOp = serde_json::from_value(serde_json::json!({
        "op": "write", "path": "src/a.ts", "content": "x"
    }))
    .unwrap();
    let err = fx
        .ws
        .edit(
            &fx.me(),
            &id,
            EditRequest {
                if_version: 0,
                ops: vec![op],
            },
        )
        .unwrap_err();
    assert_eq!(err.code(), "MATERIALIZED");
    assert!(err.to_string().contains(dir.to_str().unwrap()));
}

#[test]
fn materialize_requires_version_0() {
    let fx = Fx::new();
    let base = fx.base(BASE);
    let ws = fx.ws.open(&fx.me(), &base).unwrap();
    let op: EditOp = serde_json::from_value(serde_json::json!({
        "op": "write", "path": "src/a.ts", "content": "x"
    }))
    .unwrap();
    fx.ws
        .edit(
            &fx.me(),
            &ws.id,
            EditRequest {
                if_version: 0,
                ops: vec![op],
            },
        )
        .unwrap();
    let err = fx
        .ws
        .materialize(&ws.id, &MaterializeOptions::default())
        .unwrap_err();
    assert_eq!(err.code(), "NOT_FRESH");
    assert!(fs::read_dir(fx.home.join("work")).map_or(true, |mut d| d
        .all(|e| { fs::read_dir(e.unwrap().path()).unwrap().next().is_none() })));

    // Materializing twice returns the same directory.
    let fresh = fx.ws.open(&fx.me(), &base).unwrap();
    let opts = MaterializeOptions {
        deps: false,
        ..Default::default()
    };
    let first = fx.ws.materialize(&fresh.id, &opts).unwrap();
    assert_eq!(fx.ws.materialize(&fresh.id, &opts).unwrap(), first);
}

#[test]
fn dir_missing_marks_lost() {
    let fx = Fx::new();
    let (id, dir) = fx.open(fx.base(BASE));
    fs::write(dir.join("src/a.ts"), "synced\n").unwrap();
    fx.sync(&id);
    fs::remove_dir_all(&dir).unwrap();
    let err = fx.ws.snapshot(&fx.me(), &id).unwrap_err();
    assert_eq!(err.code(), "DIR_MISSING");
    let m = fx.ws.disk().materialization(&id).unwrap().unwrap();
    assert_eq!(m.state, MatState::Lost);
    assert_eq!(
        fx.ws.get(&fx.me(), &id).unwrap().version,
        1,
        "the overlay keeps its last state"
    );
    assert!(matches!(
        fx.ws.sync(&fx.me(), &id),
        Err(WorkspaceError::Disk(_))
    ));
}

/// Every strategy this host supports writes the same tree and keeps the
/// mtimes the checkout manifest recorded, so a first sync hashes nothing.
#[test]
fn copy_fallback_matches_clonefile() {
    let mut roots = Vec::new();
    for strategy in [
        CloneStrategy::Clonefile,
        CloneStrategy::Reflink,
        CloneStrategy::Copy,
    ] {
        if !strategy.supported() {
            continue;
        }
        let fx = Fx::with(|c| c.clone = Some(strategy));
        let mut files = BASE.to_vec();
        files.push(("bin/run.sh", "#!/bin/sh\necho hi\n"));
        let tree = common::put_files(&fx.store, &files);
        let base = fx.base_of(tree);
        let (id, dir) = fx.open(base);
        let mode = fs::metadata(dir.join("src/a.ts"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o644, "{strategy:?}");
        let report = fx.sync(&id);
        assert_eq!(
            report.stats.hashed, 0,
            "{strategy:?} kept the manifest's mtimes"
        );
        assert_eq!(
            fx.ws.disk().materialization(&id).unwrap().unwrap().clone,
            strategy
        );
        roots.push(tree_of(&fx.store, &dir, true).unwrap());
    }
    assert!(roots.windows(2).all(|w| w[0] == w[1]));
    assert!(!roots.is_empty());
}

#[test]
fn gc_never_removes_live() {
    let fx = Fx::new();
    let base = fx.base(BASE);
    let other = fx.base(&[("other.txt", "other\n")]);
    let (live, live_dir) = fx.open(base);
    let (gone, gone_dir) = fx.open(other);
    fx.ws.discard(&fx.me(), &gone).unwrap();
    assert!(!gone_dir.exists(), "discard releases the directory");
    let orphan = fx.home.join("work/snapshots/orphan");
    fs::create_dir_all(&orphan).unwrap();
    let report = fx
        .ws
        .disk()
        .gc(&GcPolicy {
            checkout_idle: std::time::Duration::ZERO,
            checkout_cap_bytes: 0,
            build_idle: std::time::Duration::ZERO,
            accepted: false,
        })
        .unwrap();
    assert_eq!(report.trashed, 1, "the orphan directory");
    assert_eq!(
        report.checkouts_removed, 1,
        "only the released workspace's base"
    );
    assert!(!orphan.exists());
    assert!(live_dir.join("src/a.ts").is_file());
    let root = fx.ws.get_snapshot(&base).unwrap().root;
    assert!(fx.home.join("checkouts").join(root.to_string()).is_dir());
    assert_eq!(fx.sync(&live).version, 0);
    assert!(fs::read_dir(fx.home.join("trash"))
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn restore_writes_the_base_version_back() {
    let fx = Fx::new();
    let (id, dir) = fx.open(fx.base(BASE));
    fs::write(dir.join("src/a.ts"), "broken\n").unwrap();
    fs::remove_file(dir.join("src/b.ts")).unwrap();
    fs::write(dir.join("src/extra.ts"), "extra\n").unwrap();
    fx.sync(&id);
    let restored = fx
        .ws
        .restore(
            &fx.me(),
            &id,
            &[p("src/a.ts"), p("src/b.ts"), p("src/extra.ts")],
        )
        .unwrap();
    assert_eq!(restored, ["src/a.ts", "src/b.ts", "src/extra.ts"]);
    let report = fx.sync(&id);
    assert_eq!((report.added, report.modified, report.deleted), (0, 0, 0));
    assert!(fx
        .ws
        .restore(&fx.me(), &id, &[p(".aver/workspace.toml")])
        .is_err());
}

/// The diff of a new executable says so, so `git apply` recreates it as
/// executable.
#[test]
fn the_diff_of_a_new_executable_recreates_it_as_executable() {
    let fx = Fx::new();
    let (id, dir) = fx.open(fx.base(BASE));
    fs::create_dir_all(dir.join("bin")).unwrap();
    let script = dir.join("bin/run.sh");
    fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    fx.sync(&id);
    let (_, files) = fx.ws.diff(&fx.me(), &id).unwrap();
    let run = files
        .iter()
        .find(|f| f.path.as_str() == "bin/run.sh")
        .unwrap();
    assert!(run.diff.contains("new file mode 100755"), "{}", run.diff);
}

fn p(s: &str) -> Path {
    s.parse().unwrap()
}

/// Random create, write, rename, delete-directory, chmod and symlink
/// sequences: after every few operations the workspace snapshot equals the
/// directory's tree.
#[test]
fn random_disk_operations_sync_to_the_directory_tree() {
    for seed in 1..=12u64 {
        let fx = Fx::new();
        let (id, dir) = fx.open(fx.base(BASE));
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let dirs = ["", "src", "src/deep", "lib", "lib/x"];
        for step in 0..40 {
            let files = list_files(&dir);
            match rng.next() % 7 {
                0 | 1 => {
                    let d = dirs[rng.next() as usize % dirs.len()];
                    let path = dir.join(d).join(format!("f{}.txt", rng.next() % 20));
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    if !path.is_dir() {
                        fs::write(&path, format!("{}", rng.next())).unwrap();
                    }
                }
                2 if !files.is_empty() => {
                    let f = &files[rng.next() as usize % files.len()];
                    fs::write(f, format!("rewritten {}", rng.next())).unwrap();
                }
                3 if !files.is_empty() => {
                    let f = &files[rng.next() as usize % files.len()];
                    let to = f.with_file_name(format!("r{}.txt", rng.next() % 50));
                    if !to.exists() {
                        fs::rename(f, to).unwrap();
                    }
                }
                4 => {
                    let d = dirs[1 + rng.next() as usize % (dirs.len() - 1)];
                    let _ = fs::remove_dir_all(dir.join(d));
                }
                5 if !files.is_empty() => {
                    let f = &files[rng.next() as usize % files.len()];
                    let mode = fs::symlink_metadata(f).unwrap().mode() ^ 0o111;
                    fs::set_permissions(f, fs::Permissions::from_mode(mode & 0o777)).unwrap();
                }
                _ => {
                    let link = dir.join(format!("l{}", rng.next() % 5));
                    if fs::symlink_metadata(&link).is_err() {
                        symlink("README.md", link).unwrap();
                    }
                }
            }
            if step % 4 == 3 {
                fx.sync(&id);
                assert_eq!(
                    fx.root(&id),
                    tree_of(&fx.store, &dir, true).unwrap(),
                    "seed {seed} step {step}"
                );
            }
        }
    }
}

fn list_files(dir: &FsPath) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let path = e.path();
            if e.file_name() == ".aver" {
                continue;
            }
            let ft = e.file_type().unwrap();
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// `aver verify` inside a directory: the verifier syncs the disk first, so
/// a receipt describes what the agent wrote with its own tools, and the
/// agent report says whether it is done and why not.
#[tokio::test]
async fn verify_workspace_mode_scripted() {
    use aver::run::report::{agent_text, exit_code, ReportInput};
    use aver::verify::{Head, Request};

    let w = world::World::new();
    let config = format!(
        "[env]\nimage = \"{}\"\n\n[[check]]\nid = \"unit\"\nrun = [\"junit-dir\", \"unit.xml\", \"test\"]\nreport = {{ format = \"junit\", path = \"/aver/out/unit.xml\" }}\n",
        world::PINNED_IMAGE
    );
    let base = w.snapshot(&[
        ("aver.toml", &config),
        (
            "test/math.test.json",
            r#"{"adds": "pass", "legacy": "fail"}"#,
        ),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &["div"]));
    let claim = w.tasks.claim(&w.admin.id(), &task, None).unwrap();
    let m = w
        .workspaces
        .materialize(
            &claim.workspace,
            &MaterializeOptions {
                deps: false,
                ..Default::default()
            },
        )
        .unwrap();
    let verify = || async {
        let sync = w.workspaces.sync(&w.admin.id(), &claim.workspace).unwrap();
        let verified = w
            .verifier
            .verify(
                w.admin.id(),
                Request {
                    task,
                    head: Head::Workspace(claim.workspace),
                    attempt: None,
                    fresh: false,
                    all: false,
                    progress: None,
                    ci: None,
                    adhoc: None,
                    reason: None,
                },
            )
            .await
            .unwrap();
        let input = ReportInput {
            receipt_id: &verified.id,
            receipt: &verified.receipt,
            diagnostics: Some(&verified.diagnostics),
            sync: sync.as_ref(),
            took: None,
            budget: Some((1, 8)),
            notices: None,
            timing: None,
        };
        let (text, code) = (agent_text(&input), exit_code(&input));
        (verified, text, code)
    };

    let (verified, text, code) = verify().await;
    assert_eq!(verified.receipt.body.verdict, Verdict::Unproven);
    assert_eq!(code, 1);
    assert!(
        text.contains("NOT DONE: UNPROVEN. 0 of 1 criteria proven"),
        "{text}"
    );
    assert!(
        text.contains("  div  NOT PROVEN (not_claimed): no test claims it"),
        "{text}"
    );
    assert!(
        text.contains("Still failing, as on the original code (1; not counted against you. If your task is to fix failing tests, these are what it is about):\n```\nunit::legacy\n```\n"),
        "{text}"
    );
    assert!(text.contains("Verifies: 1 of 8 used, 7 left."), "{text}");
    assert!(!text.contains("<repo-data"), "{text}");
    assert!(!text.contains("snap_") && !text.contains("rcpt_"), "{text}");

    fs::create_dir_all(m.dir.join("src")).unwrap();
    fs::write(
        m.dir.join("src/divide.js"),
        "module.exports = (a, b) => a / b;\n",
    )
    .unwrap();
    fs::write(
        m.dir.join("test/divide.test.json"),
        r#"{"[crit:div] divides": "pass-if:src/divide.js"}"#,
    )
    .unwrap();
    fs::write(m.dir.join(".env.local"), "API_TOKEN=secret-value\n").unwrap();
    let (verified, text, code) = verify().await;
    assert_eq!(verified.receipt.body.verdict, Verdict::Pass, "{text}");
    assert_eq!(code, 0);
    let head = w
        .workspaces
        .snapshot(&w.admin.id(), &claim.workspace)
        .unwrap();
    assert_eq!(verified.receipt.body.head_snapshot, head);
    assert!(
        text.starts_with("DONE: PASS. 1 of 1 criteria proven, no new failures.\nBase: 1 passed, 1 failed | Yours: 2 passed, 1 failed\nNext: nothing."),
        "{text}"
    );
    assert!(
        text.contains("Your changes: 2 files (src/divide.js, test/divide.test.json) recorded; 1 file NOT recorded"),
        "{text}"
    );
    assert!(
        text.contains(
            "Criteria: 1 of 1 proven (`aver verify --json` names the tests that prove them).\n"
        ),
        "{text}"
    );
    assert!(
        text.contains(".env.local: looks like it contains a secret"),
        "{text}"
    );
    assert!(text.contains("Next: nothing. You are done"), "{text}");
    let attempt = w.tasks.attempt(&w.admin.id(), &claim.attempt).unwrap();
    assert_eq!(
        attempt.receipts.len(),
        2,
        "receipts attach to the claimed attempt"
    );

    // A PASS that judged the previous version of a file the agent changed
    // (here: a base file now holding a secret it cannot sync) is not done.
    let key = format!(
        r#"{{"adds": "pass", "legacy": "fail", "token": "{}{}"}}"#,
        "AKIA", "ABCDEFGHIJKLMNOP"
    );
    fs::write(m.dir.join("test/math.test.json"), key).unwrap();
    let (verified, text, code) = verify().await;
    assert_eq!(verified.receipt.body.verdict, Verdict::Pass, "{text}");
    assert_eq!(code, 1, "{text}");
    assert!(
        text.contains("NOT DONE: PASS on the recorded files"),
        "{text}"
    );
    assert!(text.contains("Next: fix the files not recorded"), "{text}");
}

/// A command-level check that fails on the original code too is
/// inconclusive (§1.9.3): the agent report says it blocks the verdict, not
/// that the agent may ignore it, and that verifying again cannot help.
#[tokio::test]
async fn verify_workspace_mode_reports_inconclusive_checks() {
    use aver::run::report::{agent_text, exit_code, ReportInput};
    use aver::verify::{Head, Request};

    let w = world::World::new();
    let config = format!(
        "[env]\nimage = \"{}\"\n\n[[check]]\nid = \"unit\"\nrun = [\"junit-dir\", \"unit.xml\", \"test\"]\nreport = {{ format = \"junit\", path = \"/aver/out/unit.xml\" }}\n\n[[check]]\nid = \"lint\"\nrun = [\"exit\", \"1\"]\n",
        world::PINNED_IMAGE
    );
    let base = w.snapshot(&[
        ("aver.toml", &config),
        (
            "test/math.test.json",
            r#"{"adds": "pass", "legacy": "fail"}"#,
        ),
    ]);
    let task = w.create_task(w.task_body(&w.admin, base, &[]));
    let claim = w.tasks.claim(&w.admin.id(), &task, None).unwrap();
    let m = w
        .workspaces
        .materialize(
            &claim.workspace,
            &MaterializeOptions {
                deps: false,
                ..Default::default()
            },
        )
        .unwrap();
    fs::write(m.dir.join("src.js"), "module.exports = 1;\n").unwrap();
    let sync = w.workspaces.sync(&w.admin.id(), &claim.workspace).unwrap();
    let verified = w
        .verifier
        .verify(
            w.admin.id(),
            Request {
                task,
                head: Head::Workspace(claim.workspace),
                attempt: None,
                fresh: false,
                all: false,
                progress: None,
                ci: None,
                adhoc: None,
                reason: None,
            },
        )
        .await
        .unwrap();
    let input = ReportInput {
        receipt_id: &verified.id,
        receipt: &verified.receipt,
        diagnostics: Some(&verified.diagnostics),
        sync: sync.as_ref(),
        took: None,
        budget: None,
        notices: None,
        timing: None,
    };
    let (text, code) = (agent_text(&input), exit_code(&input));
    assert_eq!(
        verified.receipt.body.verdict,
        Verdict::BlockedNotOwn,
        "{text}"
    );
    assert_eq!(code, 1);
    assert!(
        text.contains("inconclusive, flaky or infrastructure failures block the verdict"),
        "{text}"
    );
    let (before, after) = text
        .split_once("Blocking the verdict, not yours (1): these checks fail on the original code too and have no per-test results")
        .expect(&text);
    assert!(after.contains(":\n```\nlint\n```\n"), "{text}");
    // The per-test PREEXISTING failure is still one to ignore; `lint` is not.
    let ignore = before
        .split_once("Still failing, as on the original code")
        .map(|(_, rest)| rest)
        .expect(&text);
    assert!(ignore.contains("\nunit::legacy\n"), "{text}");
    assert!(!ignore.contains("\nlint\n"), "{text}");
    assert!(
        text.contains(
            "Next: stop and report the blocker; verifying again cannot change this verdict."
        ),
        "{text}"
    );
    assert!(
        text.trim_end()
            .ends_with("NOT DONE: BLOCKED_NOT_OWN: not yours; stop and report the blocker"),
        "{text}"
    );
}

/// Opens a scripted world with a `junit-dir` check over `test/`, a task
/// with `criteria` on a base holding `tests` as `test/math.test.json`, and
/// the claimed workspace's directory. The closure verifies the directory
/// and renders the agent report.
struct Verifying {
    w: world::World,
    task: aver_core::TaskId,
    ws: aver_core::WorkspaceId,
    dir: PathBuf,
}

impl Verifying {
    fn new(tests: &str, criteria: &[&str]) -> Self {
        let w = world::World::new();
        let config = format!(
            "[env]\nimage = \"{}\"\n\n[[check]]\nid = \"unit\"\nrun = [\"junit-dir\", \"unit.xml\", \"test\"]\nreport = {{ format = \"junit\", path = \"/aver/out/unit.xml\" }}\n",
            world::PINNED_IMAGE
        );
        let base = w.snapshot(&[("aver.toml", &config), ("test/math.test.json", tests)]);
        let task = w.create_task(w.task_body(&w.admin, base, criteria));
        let claim = w.tasks.claim(&w.admin.id(), &task, None).unwrap();
        let m = w
            .workspaces
            .materialize(
                &claim.workspace,
                &MaterializeOptions {
                    deps: false,
                    ..Default::default()
                },
            )
            .unwrap();
        Self {
            w,
            task,
            ws: claim.workspace,
            dir: m.dir,
        }
    }

    async fn verify(&self) -> (aver::verify::Verified, String, u8) {
        use aver::run::report::{agent_text, exit_code, ReportInput};
        use aver::verify::{Head, Request};
        let sync = self
            .w
            .workspaces
            .sync(&self.w.admin.id(), &self.ws)
            .unwrap();
        let verified = self
            .w
            .verifier
            .verify(
                self.w.admin.id(),
                Request {
                    task: self.task,
                    head: Head::Workspace(self.ws),
                    attempt: None,
                    fresh: false,
                    all: false,
                    progress: None,
                    ci: None,
                    adhoc: None,
                    reason: None,
                },
            )
            .await
            .unwrap();
        let input = ReportInput {
            receipt_id: &verified.id,
            receipt: &verified.receipt,
            diagnostics: Some(&verified.diagnostics),
            sync: sync.as_ref(),
            took: None,
            budget: Some((2, 10)),
            notices: None,
            timing: None,
        };
        let (text, code) = (agent_text(&input), exit_code(&input));
        (verified, text, code)
    }
}

/// With no criteria, a verify still says what the change's tests show: an
/// unchanged directory is not done however green, the head's own tests
/// that fail on the original code are listed, and so are the failing tests
/// it fixed and those still failing.
#[tokio::test]
async fn verify_reports_what_the_change_shows() {
    let v = Verifying::new(
        r#"{"adds": "pass", "legacy": "fail", "broken": "pass-if:src/fix.js"}"#,
        &[],
    );
    let (verified, text, code) = v.verify().await;
    assert_eq!(verified.receipt.body.verdict, Verdict::Pass, "{text}");
    assert_eq!(code, 1, "an unchanged directory is not done: {text}");
    assert!(
        text.starts_with("NOT DONE: PASS, but you changed nothing: the original code passes what it passed before. 2 tests that fail on the original code still fail (below).\nBase: 1 passed, 2 failed | Yours: 1 passed, 2 failed\nNext: make the change the task asks for"),
        "{text}"
    );
    assert!(
        text.contains("Still failing, as on the original code (2; not counted against you. If your task is to fix failing tests, these are what it is about):\n```\nunit::broken\nunit::legacy\n```\n"),
        "{text}"
    );
    assert!(
        text.trim_end()
            .ends_with("NOT DONE: PASS (2 of 10 verifies used): you changed nothing yet"),
        "{text}"
    );

    fs::create_dir_all(v.dir.join("src")).unwrap();
    fs::write(v.dir.join("src/fix.js"), "module.exports = 1;\n").unwrap();
    let (verified, text, code) = v.verify().await;
    assert_eq!(verified.receipt.body.verdict, Verdict::Pass, "{text}");
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains("Tests: 1 test that failed on the original code now passes; 1 test still fails as on the original code (not yours) (`aver verify --json` lists them).")
            && text.lines().count() <= aver::run::report::MAX_PASS_LINES,
        "{text}"
    );
    assert!(
        !text.contains("Nothing shows the change works yet"),
        "a fixed test is evidence: {text}"
    );

    fs::write(
        v.dir.join("test/edge.test.json"),
        r#"{"edge case": "pass-if:src/fix.js"}"#,
    )
    .unwrap();
    let (verified, text, code) = v.verify().await;
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains("Tests: 1 of yours shows the change (failing on the original code, passing with yours); 1 test that failed"),
        "{text}"
    );
    assert!(
        text.lines().count() <= aver::run::report::MAX_PASS_LINES,
        "{text}"
    );
    assert!(
        text.trim_end()
            .ends_with("DONE: PASS (2 of 10 verifies used)"),
        "{text}"
    );
    let evidence = verified.diagnostics.evidence.clone().expect("evidence");
    let shown: Vec<&str> = evidence
        .shows_change
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(shown, ["unit::edge case"]);
    assert_eq!(evidence.fixed.items, ["unit::broken"]);
    assert_eq!(evidence.still_failing.items, ["unit::legacy"]);
    assert!(verified.diagnostics.weak_proofs.is_empty());
    // The evidence comes from a transplant run the receipt does not record:
    // a test that proves no criterion carries no transplant result.
    assert!(verified
        .receipt
        .body
        .checks
        .iter()
        .all(|c| c.transplant.is_none()));
}

/// A criterion whose only proofs cannot load on the original code is not
/// proven (weak proofs alone never prove); the report and the diagnostics
/// say why.
#[tokio::test]
async fn verify_flags_proofs_by_a_missing_module() {
    let v = Verifying::new(r#"{"adds": "pass"}"#, &["eng"]);
    fs::create_dir_all(v.dir.join("src")).unwrap();
    fs::write(v.dir.join("src/engine.js"), "module.exports = 1;\n").unwrap();
    fs::write(
        v.dir.join("test/engine.test.json"),
        r#"{"[crit:eng] engine loads": "only-if:src/engine.js"}"#,
    )
    .unwrap();
    let (verified, text, code) = v.verify().await;
    assert_eq!(verified.receipt.body.verdict, Verdict::Unproven, "{text}");
    assert_eq!(code, 1);
    assert!(
        text.contains("  eng  NOT PROVEN (")
            && text.contains("only because a module they import is missing"),
        "{text}"
    );
    let weak = &verified.diagnostics.weak_proofs;
    assert_eq!(weak.len(), 1, "{weak:?}");
    assert_eq!(weak[0].criterion.as_str(), "eng");
    assert_eq!(weak[0].reason, "missing_module");

    // A second proof that fails on an assertion makes the proof strong.
    fs::write(
        v.dir.join("test/engine2.test.json"),
        r#"{"[crit:eng] engine adds": "pass-if:src/engine.js"}"#,
    )
    .unwrap();
    let (verified, text, _) = v.verify().await;
    assert!(verified.diagnostics.weak_proofs.is_empty(), "{text}");
    assert_eq!(verified.receipt.body.verdict, Verdict::Pass, "{text}");
    assert!(
        text.contains("Criteria: 1 of 1 proven (`aver verify --json`"),
        "{text}"
    );
}
