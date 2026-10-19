//! An adopted CI checkout (`place::workdir`) cut at every crash point of
//! its swaps and of `Workdir::restore`, then restored from Git alone: the
//! checkout must be its commit again, byte for byte, with nothing left
//! over: a child of this test binary runs the swaps alone, and `aver ci`
//! runs them in place on the sh-calc world.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Command;

use aver::place::workdir::{Keep, Workdir};
use aver::store::Store;
use aver_core::{GitSha, Node, Set, Tree, TreeEntry, TreeId};

use super::world::{git, solve, text, World};

/// The points this file cuts, which no matrix row can reach.
pub const POINTS: &[&str] = &[
    "swap.journaled",
    "swap.partial",
    "swap.done",
    "workdir.restore.partial",
    "workdir.restore.done",
];

const CHILD: &str = "AVER_WORKDIR_CHILD";

/// The commit: a path `*`-suffixed is executable.
const HEAD: &[(&str, &str)] = &[
    (".gitignore", ".venv/\n"),
    ("README", "r\n"),
    ("docs/one.md", "1\n"),
    ("docs/two.md", "2\n"),
    ("run.sh*", "#!/bin/sh\n"),
    ("src/a.py", "head\n"),
    ("src/lib/b.py", "b\n"),
];

/// The other side: `src/a.py` differs, `run.sh` loses its exec bit,
/// `docs/` is gone and two paths are new, one in new directories.
const BASE: &[(&str, &str)] = &[
    (".gitignore", ".venv/\n"),
    ("README", "r\n"),
    ("newdir/deep/x.py", "x\n"),
    ("only_base.py", "ob\n"),
    ("run.sh", "#!/bin/sh\n"),
    ("src/a.py", "base\n"),
    ("src/lib/b.py", "b\n"),
];

fn tree(store: &Store, files: &[(&str, &str)]) -> TreeId {
    let mut entries = Vec::new();
    let mut subdirs: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for (path, text) in files {
        match path.split_once('/') {
            Some((dir, rest)) => subdirs.entry(dir).or_default().push((rest, text)),
            None => {
                let blob = store.put_blob(text.as_bytes()).unwrap();
                let (name, node) = match path.strip_suffix('*') {
                    Some(name) => (name, Node::Exec(blob)),
                    None => (*path, Node::File(blob)),
                };
                entries.push(TreeEntry {
                    name: name.parse().unwrap(),
                    node,
                });
            }
        }
    }
    for (dir, files) in subdirs {
        entries.push(TreeEntry {
            name: dir.parse().unwrap(),
            node: Node::Tree(tree(store, &files)),
        });
    }
    store.put(&Tree::new(Set::new(entries).unwrap())).unwrap()
}

/// A case's directory: the commit checked out in `repo/` with an ignored
/// `.venv` the adopt keeps, and the store in `store.db`.
fn checkout(case: &Path) {
    let repo = case.join("repo");
    fs::create_dir_all(&repo).unwrap();
    for (path, text) in HEAD {
        let at = repo.join(path.trim_end_matches('*'));
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(&at, text).unwrap();
        let mode = if path.ends_with('*') { 0o755 } else { 0o644 };
        fs::set_permissions(&at, fs::Permissions::from_mode(mode)).unwrap();
    }
    fs::create_dir_all(repo.join(".venv/bin")).unwrap();
    fs::write(repo.join(".venv/bin/python"), "py").unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "-A"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "c",
        ],
    );
}

/// What a restored checkout must equal: the commit's tree, Git's view of
/// tracked, untracked and ignored files, every file's mode and bytes, and
/// no journal or temporary of Aver's in `.git`.
#[derive(Debug, PartialEq, Eq)]
struct State {
    tree: String,
    status: String,
    ignored: String,
    files: Vec<(String, u32, Vec<u8>)>,
    aver_in_git: Vec<String>,
}

impl State {
    fn of(repo: &Path) -> Self {
        fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, u32, Vec<u8>)>) {
            for e in fs::read_dir(dir).unwrap().flatten() {
                let rel = e.path().strip_prefix(root).unwrap().display().to_string();
                let meta = fs::symlink_metadata(e.path()).unwrap();
                if rel == ".git" {
                    continue;
                } else if meta.is_dir() {
                    walk(root, &e.path(), out);
                } else {
                    let mode = meta.permissions().mode();
                    out.push((rel, mode, fs::read(e.path()).unwrap()));
                }
            }
        }
        let mut files = Vec::new();
        walk(repo, repo, &mut files);
        files.sort();
        let aver_in_git = fs::read_dir(repo.join(".git"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("aver"))
            .collect();
        Self {
            tree: git(repo, &["rev-parse", "HEAD^{tree}"]),
            status: git(repo, &["status", "--porcelain", "--untracked-files=all"]),
            ignored: git(
                repo,
                &[
                    "status",
                    "--porcelain",
                    "--untracked-files=all",
                    "--ignored",
                ],
            ),
            files,
            aver_in_git,
        }
    }
}

/// The operation a cut CI job was running, in a child process: adopt the
/// checkout, swap to the other side, back, to the other side again, then
/// restore. A no-op unless the parent set `AVER_WORKDIR_CHILD`.
#[test]
fn child_swaps_and_restores() {
    let Some(case) = std::env::var_os(CHILD) else {
        return;
    };
    let case = Path::new(&case);
    let repo = case.join("repo");
    let store = Store::open(case.join("store.db")).unwrap();
    let (head, base) = (tree(&store, HEAD), tree(&store, BASE));
    let sha: GitSha = git(&repo, &["rev-parse", "HEAD"]).parse().unwrap();
    let keep = Keep::new([".venv"]).unwrap();
    let mut work = Workdir::adopt(&repo, &store, (sha, head), keep).unwrap();
    for to in [base, head, base] {
        work.become_tree(&store, to).unwrap();
    }
    drop(work);
    Workdir::restore(&repo).unwrap();
}

/// Runs the child under `fault`: whether it was cut there.
fn cut(case: &Path, fault: &str) -> bool {
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "workdir::child_swaps_and_restores",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, case)
        .env("AVER_FAULT", fault)
        .output()
        .unwrap();
    match out.status.code() {
        Some(137) => true,
        Some(0) => false,
        _ => panic!("{fault}: {}", super::world::text(&out)),
    }
}

/// Defect: a CI job killed while Aver had its checkout at another tree, or
/// while restoring it, leaves the workflow's later steps a checkout that
/// is not its commit (a file of the other side, a file the commit has
/// missing, a stale journal), and `Workdir::restore` cannot put it back.
#[test]
fn crit_workdir_fault_restore_after_any_cut() {
    let root = tempfile::tempdir().unwrap();
    let mut cases = 0;
    for point in POINTS {
        for n in 1.. {
            let case = root.path().join(format!("{point}-{n}"));
            checkout(&case);
            let repo = case.join("repo");
            let before = State::of(&repo);
            let fault = format!("{point}#{n}=abort");
            let was_cut = cut(&case, &fault);
            if was_cut {
                if repo.join(".git/aver-swap.json").exists() {
                    Workdir::restore(&repo).unwrap_or_else(|e| panic!("{fault}: {e}"));
                }
                cases += 1;
            }
            assert_eq!(State::of(&repo), before, "{fault}, then restore");
            fs::remove_dir_all(&case).unwrap();
            if !was_cut {
                assert!(n > 1, "{point} is never reached");
                break;
            }
        }
    }
    // Three swaps of four writes each, and a restore of four writes and
    // two removals.
    assert_eq!(cases, 3 + 12 + 3 + 6 + 1, "abort cases");
}

/// The sh-calc world with the solved change committed as the head, an
/// untracked file of the job's beside it: `(world, base, head)`.
fn ci_world() -> (World, String, String) {
    let world = World::new();
    let base = git(&world.repo, &["rev-parse", "HEAD"]);
    solve(&world.repo);
    git(&world.repo, &["add", "-A"]);
    git(&world.repo, &["commit", "--quiet", "-m", "neg"]);
    let head = git(&world.repo, &["rev-parse", "HEAD"]);
    fs::write(world.repo.join("notes.txt"), "mine\n").unwrap();
    (world, base, head)
}

fn ci_args<'a>(world: &'a World, base: &'a str, head: &'a str) -> Vec<String> {
    let out = world.faults.join("out").display().to_string();
    ["ci", "--base", base, "--head", head, "--out-dir", &out]
        .map(str::to_owned)
        .to_vec()
}

/// Defect: `aver ci` killed outright at any write of a swap or of the
/// restore leaves the job's checkout at another tree or without its
/// untracked files, and `aver ci restore` (the action's `if: always()`
/// step) cannot put it back, or is not a no-op the second time.
#[test]
fn crit_ci_in_place_restore_after_any_cut() {
    let mut cases = 0;
    for point in POINTS {
        for n in 1.. {
            let (world, base, head) = ci_world();
            let before = State::of(&world.repo);
            let fault = format!("{point}#{n}=abort");
            let args = ci_args(&world, &base, &head);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let out = world.aver(&world.repo, &args, Some(&fault));
            match out.status.code() {
                Some(137) => cases += 1,
                Some(0) => {
                    assert!(n > 1, "{point} is never reached by aver ci");
                    assert_eq!(State::of(&world.repo), before, "a run without the cut");
                    break;
                }
                _ => panic!("{fault}: {}", text(&out)),
            }
            for _ in 0..2 {
                let restore = world.aver(&world.repo, &["ci", "restore"], None);
                assert_eq!(
                    restore.status.code(),
                    Some(0),
                    "{fault}: {}",
                    text(&restore)
                );
                assert_eq!(
                    State::of(&world.repo),
                    before,
                    "{fault}, then aver ci restore"
                );
            }
        }
    }
    assert!(cases >= POINTS.len(), "abort cases: {cases}");
}

/// Defect: a SIGTERM (the runner cancelling the job) while the checkout is
/// at the base and no check is running ends `aver ci` without putting the
/// checkout back.
#[test]
fn crit_ci_in_place_signal_between_runs_restores() {
    let (world, base, head) = ci_world();
    let before = State::of(&world.repo);
    let args = ci_args(&world, &base, &head);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = world
        .command(
            Path::new(env!("CARGO_BIN_EXE_aver")),
            &world.repo,
            &args,
            Some("swap.done#1=pause"),
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid = world.paused("swap.done");
    assert_ne!(State::of(&world.repo), before, "paused at another tree");
    // SAFETY: the child is ours and not yet reaped.
    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    world.resume("swap.done");
    assert_eq!(child.wait().unwrap().code(), Some(130));
    assert_eq!(State::of(&world.repo), before, "restored on the signal");
}
