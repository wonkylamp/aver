//! Execution places (REDESIGN.md §3, docs/redesign/wp2-plan.md §5.1): a
//! run's processes ([`aver::place::proc`]), its directory
//! ([`aver::place::workdir`]), the slots of a verification
//! ([`aver::place::sides`]) and the executor running checks in them
//! ([`aver::place::runner`]). CLAUDE.md rule 10: every change to `place/`
//! needs a test here. Each test names the defect it catches.

mod common;
mod scripted;
mod world;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use aver::exec::output::Exit;
use aver::place::proc::{self, ProcFailure, Spawn, RUN_TAG_VAR};

/// A run's directories under one temporary root.
struct Dirs {
    _root: tempfile::TempDir,
    cwd: PathBuf,
    out: PathBuf,
    log: PathBuf,
    record: PathBuf,
}

fn dirs() -> Dirs {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("src");
    let out = root.path().join("out");
    let record = root.path().join("unit");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    Dirs {
        log: root.path().join("log"),
        cwd,
        out,
        record,
        _root: root,
    }
}

fn vars(tag: &str) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    vars.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
    vars.insert(RUN_TAG_VAR.into(), tag.into());
    vars
}

fn sh(line: &str) -> Vec<String> {
    vec!["sh".into(), "-c".into(), line.into()]
}

fn run(
    d: &Dirs,
    commands: &[Vec<String>],
    tag: &str,
    timeout: Duration,
) -> Result<aver::exec::output::SandboxOutput, ProcFailure> {
    let vars = vars(tag);
    proc::run(&Spawn {
        commands,
        cwd: &d.cwd,
        vars: &vars,
        out: &d.out,
        log: &d.log,
        timeout,
        output_bytes: 64 * 1024,
        nice: false,
        record: (&d.record, tag),
        report: None,
        own: &[],
        withheld: &[],
        run_as: None,
    })
}

fn tag() -> String {
    format!("place-{}", ulid::Ulid::generate())
}

/// Whether a live (not zombie) process carries `needle` in its command
/// line.
fn alive_with(needle: &str) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        // Not Linux: ask ps.
        let out = Command::new("ps")
            .args(["-axo", "command"])
            .output()
            .unwrap();
        return String::from_utf8_lossy(&out.stdout).contains(needle);
    };
    entries.flatten().any(|e| {
        let stat = std::fs::read_to_string(e.path().join("stat")).unwrap_or_default();
        let zombie = stat
            .rfind(')')
            .and_then(|i| stat[i + 1..].split_whitespace().next())
            == Some("Z");
        !zombie
            && std::fs::read(e.path().join("cmdline"))
                .is_ok_and(|c| String::from_utf8_lossy(&c).contains(needle))
    })
}

fn gone_within(needle: &str, limit: Duration) -> bool {
    let until = Instant::now() + limit;
    while Instant::now() < until {
        if !alive_with(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !alive_with(needle)
}

/// Defect: a timed-out check's background children run on after the run
/// ends and write into the next run.
#[test]
fn a_timed_out_run_and_its_background_children_are_killed() {
    let d = dirs();
    let marker = format!("30{}", std::process::id());
    let started = Instant::now();
    let out = run(
        &d,
        &[sh(&format!("sleep {marker} & sleep {marker}"))],
        &tag(),
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(out.exit, Exit::Timeout(137));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert!(
        gone_within(&format!("sleep {marker}"), Duration::from_secs(2)),
        "a sleep of the run is still running"
    );
    assert!(!d.record.with_extension("pid").exists());
}

/// Defect: a listener a passing test backgrounds outlives the run and
/// holds its port for the next one.
#[test]
fn a_listener_left_by_a_passing_run_is_reaped() {
    let d = dirs();
    let marker = format!("31{}", std::process::id());
    // The shell outlives the listener's start, as a test that serves and
    // then works does: macOS hides a platform binary's environment, so a
    // process orphaned before the first descendant poll is not found.
    let out = run(
        &d,
        &[sh(&format!(
            "perl -MPOSIX -e 'POSIX::setsid(); exec @ARGV' sleep {marker} & sleep 0.5; exit 0"
        ))],
        &tag(),
        Duration::from_secs(20),
    )
    .unwrap();
    assert_eq!(out.exit, Exit::Exited(0));
    assert!(
        gone_within(&format!("sleep {marker}"), Duration::from_secs(2)),
        "the detached process outlived the run"
    );
    let log = String::from_utf8_lossy(&out.output);
    assert!(log.contains("Aver killed"), "{log}");
}

/// Defect: a huge output pushes the traceback's last line, where the error
/// is, out of the bounded output; one huge line does the same.
#[test]
fn long_output_keeps_its_head_and_last_line_and_long_lines_are_cut() {
    let d = dirs();
    let out = run(
        &d,
        &[sh(
            "i=0; while [ $i -lt 30000 ]; do echo \"line $i of filler output that goes on\"; i=$((i+1)); done; echo 'ValueError: last line'",
        )],
        &tag(),
        Duration::from_secs(30),
    )
    .unwrap();
    assert!(out.output.len() <= 64 * 1024, "{}", out.output.len());
    let text = String::from_utf8_lossy(&out.output);
    assert!(text.starts_with("line 0 of filler"), "{}", &text[..40]);
    assert!(
        text.ends_with("ValueError: last line\n"),
        "{}",
        &text[text.len() - 60..]
    );

    let d = dirs();
    let out = run(
        &d,
        &[sh("head -c 10240 /dev/zero | tr '\\0' x; echo; echo after")],
        &tag(),
        Duration::from_secs(10),
    )
    .unwrap();
    let text = String::from_utf8_lossy(&out.output);
    assert!(text.contains("[aver] line cut at 4096 bytes"), "{text}");
    assert!(text.ends_with("after\n"));
}

/// Defect: a failing setup command hidden by a passing check, and a
/// missing program read as the change's failure (and `aver init` matching
/// another wording).
#[test]
fn a_failing_setup_command_and_a_missing_program_say_so() {
    let d = dirs();
    let out = run(
        &d,
        &[sh("exit 3"), sh("exit 0")],
        &tag(),
        Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(out.exit, Exit::Exited(3));
    assert!(
        String::from_utf8_lossy(&out.output).contains("[aver] setup command 1 of 1 exited with 3")
    );

    let d = dirs();
    let e = run(
        &d,
        &[vec!["aver-no-such-program-here".into()]],
        &tag(),
        Duration::from_secs(10),
    )
    .unwrap_err();
    assert!(
        matches!(&e, ProcFailure::Failed { message, .. }
            if message == "aver-no-such-program-here is not installed on this machine"),
        "{e:?}"
    );
}

/// Defect: a run left by a killed Aver process keeps writing a report
/// while the next run of the check reads it; or a record naming a reused
/// pid kills an unrelated process.
#[test]
fn a_recorded_leftover_is_killed_and_a_reused_pid_is_left_alone() {
    let d = dirs();
    let marker = format!("32{}", std::process::id());
    let mut child = Command::new("sleep");
    child.arg(&marker);
    std::os::unix::process::CommandExt::process_group(&mut child, 0);
    let mut child = child.spawn().unwrap();
    let start = aver::run::state::process_start(child.id());
    write_record(&d.record, child.id() as i32, start);
    assert!(proc::kill_leftover(&d.record).is_some());
    child.wait().unwrap();
    assert!(!d.record.with_extension("pid").exists());

    let mut other = Command::new("sleep");
    other.arg(&marker);
    std::os::unix::process::CommandExt::process_group(&mut other, 0);
    let mut other = other.spawn().unwrap();
    let start = aver::run::state::process_start(other.id()).map(|s| s + 1);
    write_record(&d.record, other.id() as i32, start);
    assert!(proc::kill_leftover(&d.record).is_none());
    assert!(
        other.try_wait().unwrap().is_none(),
        "another process was signalled"
    );
    other.kill().unwrap();
    other.wait().unwrap();
}

fn write_record(record: &Path, pgid: i32, start: Option<u64>) {
    let json = serde_json::json!({ "pgid": pgid, "start": start, "tag": "" });
    std::fs::write(record.with_extension("pid"), json.to_string()).unwrap();
}

// ---- workdir ----

use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::sync::Arc;

use aver::place::workdir::{Changed, Keep, Workdir, WorkdirError};
use aver::store::Store;
use aver::workspace::Glob;
use aver_core::{BlobId, GitSha, Node, Set, Tree, TreeEntry, TreeId};

fn store() -> (tempfile::TempDir, Arc<Store>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path().join("store.db")).unwrap());
    (dir, store)
}

/// A tree of `(path, content)` files built in the store; a path ending in
/// `*` is executable.
fn tree(store: &Store, files: &[(&str, &str)]) -> TreeId {
    fn build(store: &Store, files: &[(String, String)]) -> TreeId {
        let mut entries = Vec::new();
        let mut subdirs: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for (path, text) in files {
            match path.split_once('/') {
                Some((dir, rest)) => subdirs
                    .entry(dir.to_owned())
                    .or_default()
                    .push((rest.to_owned(), text.clone())),
                None => {
                    let (name, exec) = match path.strip_suffix('*') {
                        Some(n) => (n, true),
                        None => (path.as_str(), false),
                    };
                    let blob = store.put_blob(text.as_bytes()).unwrap();
                    entries.push(TreeEntry {
                        name: name.parse().unwrap(),
                        node: if exec {
                            Node::Exec(blob)
                        } else {
                            Node::File(blob)
                        },
                    });
                }
            }
        }
        for (dir, files) in subdirs {
            entries.push(TreeEntry {
                name: dir.parse().unwrap(),
                node: Node::Tree(build(store, &files)),
            });
        }
        store.put(&Tree::new(Set::new(entries).unwrap())).unwrap()
    }
    let files: Vec<(String, String)> = files
        .iter()
        .map(|(p, t)| ((*p).to_owned(), (*t).to_owned()))
        .collect();
    build(store, &files)
}

fn keep(patterns: &[&str]) -> Keep {
    Keep::new(patterns.iter().copied()).unwrap()
}

fn read(dir: &Path, path: &str) -> String {
    fs::read_to_string(dir.join(path)).unwrap()
}

/// Every regular file under `dir` (relative path, content), skipping `skip`.
fn listing(dir: &Path, skip: &[&str]) -> Vec<(String, String)> {
    fn walk(root: &Path, dir: &Path, skip: &[&str], out: &mut Vec<(String, String)>) {
        for e in fs::read_dir(dir).unwrap().flatten() {
            let rel = e
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if skip
                .iter()
                .any(|s| rel == *s || rel.starts_with(&format!("{s}/")))
            {
                continue;
            }
            if e.file_type().unwrap().is_dir() {
                walk(root, &e.path(), skip, out);
            } else {
                out.push((rel, fs::read_to_string(e.path()).unwrap_or_default()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, skip, &mut out);
    out.sort();
    out
}

/// Defect: a slot keeps a build output or another tree's file that the run
/// then tests, or loses the installed dependencies it should keep.
#[test]
fn become_tree_leaves_exactly_the_tree_and_the_kept_directories() {
    let (_s, store) = store();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("slot");
    let t = tree(
        &store,
        &[
            ("a.js", "a"),
            ("bin/run*", "#!/bin/sh\n"),
            ("lib/b.js", "b"),
        ],
    );
    let mut w = Workdir::open(
        &dir,
        &root.path().join("slot.swap.json"),
        keep(&["node_modules"]),
    )
    .unwrap();
    fs::create_dir_all(dir.join("dist")).unwrap();
    fs::write(dir.join("dist/x.js"), "built").unwrap();
    fs::write(dir.join(".git"), "gitdir: elsewhere").unwrap();
    fs::create_dir_all(dir.join("node_modules/dep")).unwrap();
    fs::write(dir.join("node_modules/dep/index.js"), "dep").unwrap();
    fs::create_dir_all(dir.join("lib")).unwrap();
    fs::write(dir.join("lib/old.js"), "old").unwrap();
    w.become_tree(&store, t).unwrap();
    assert_eq!(
        listing(&dir, &[]),
        [
            ("a.js".to_owned(), "a".to_owned()),
            ("bin/run".to_owned(), "#!/bin/sh\n".to_owned()),
            ("lib/b.js".to_owned(), "b".to_owned()),
            ("node_modules/dep/index.js".to_owned(), "dep".to_owned()),
        ]
    );
    let mode = fs::metadata(dir.join("bin/run")).unwrap().mode();
    assert_eq!(mode & 0o111, 0o111, "an exec file is executable");
    assert_eq!(fs::metadata(dir.join("a.js")).unwrap().mode() & 0o111, 0);
}

/// Defect: a kept name hides source. A `venv` package the tree tracks is
/// kept from the tree before, so its deleted file survives the swap and the
/// run tests code the tree lacks; or an installed, untracked `venv` is
/// removed with the swap.
#[test]
fn a_kept_name_the_tree_tracks_swaps_and_an_untracked_one_stays() {
    let (_s, store) = store();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("slot");
    let a = tree(&store, &[("app.py", "a"), ("venv/old.py", "old")]);
    let b = tree(&store, &[("app.py", "b"), ("venv/new.py", "new")]);
    let c = tree(&store, &[("app.py", "c")]);
    let d = tree(&store, &[("app.py", "d")]);
    let mut w = Workdir::open(&dir, &root.path().join("slot.swap.json"), keep(&["venv"])).unwrap();
    w.become_tree(&store, a).unwrap();
    w.become_tree(&store, b).unwrap();
    assert_eq!(
        listing(&dir, &[]),
        [
            ("app.py".to_owned(), "b".to_owned()),
            ("venv/new.py".to_owned(), "new".to_owned()),
        ]
    );
    w.become_tree(&store, c).unwrap();
    assert_eq!(listing(&dir, &[]), [("app.py".to_owned(), "c".to_owned())]);
    fs::create_dir_all(dir.join("venv/bin")).unwrap();
    fs::write(dir.join("venv/bin/python"), "py").unwrap();
    w.become_tree(&store, d).unwrap();
    w.become_tree(&store, c).unwrap();
    assert_eq!(
        listing(&dir, &[]),
        [
            ("app.py".to_owned(), "c".to_owned()),
            ("venv/bin/python".to_owned(), "py".to_owned()),
        ]
    );
}

/// Defect (I1): a file rewritten with the same mtime as an output built
/// from other content reads as unchanged to Cargo or Python; or every
/// file rewritten, so every build starts cold.
#[test]
fn a_swap_rewrites_only_what_differs_with_fresh_mtimes() {
    let (_s, store) = store();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("slot");
    let journal = root.path().join("slot.swap.json");
    let a = tree(
        &store,
        &[("same.rs", "same"), ("diff.rs", "one"), ("gone.rs", "x")],
    );
    let b = tree(
        &store,
        &[("same.rs", "same"), ("diff.rs", "two"), ("new.rs", "y")],
    );
    let mut w = Workdir::open(&dir, &journal, Keep::default()).unwrap();
    w.become_tree(&store, a).unwrap();
    let before = |p: &str| fs::metadata(dir.join(p)).unwrap();
    let (same, diff) = (before("same.rs"), before("diff.rs"));
    std::thread::sleep(Duration::from_millis(20));
    let swap = w.become_tree(&store, b).unwrap();
    assert_eq!((swap.written, swap.removed), (2, 1));
    let same_after = before("same.rs");
    assert_eq!(
        (
            same_after.ino(),
            same_after.mtime_nsec(),
            same_after.mtime()
        ),
        (same.ino(), same.mtime_nsec(), same.mtime())
    );
    assert!(before("diff.rs").modified().unwrap() > diff.modified().unwrap());
    assert_eq!(read(&dir, "diff.rs"), "two");
    assert!(!dir.join("gone.rs").exists());

    // A file edited behind the journal's back is rewritten too.
    fs::write(dir.join("same.rs"), "edited").unwrap();
    let mut w = Workdir::open(&dir, &journal, Keep::default()).unwrap();
    w.become_tree(&store, b).unwrap();
    assert_eq!(read(&dir, "same.rs"), "same");
}

/// Defect: a swap cut short (a killed process) leaves a half-written file
/// the journal still vouches for, and the next run tests it.
#[test]
fn a_swap_cut_short_is_finished_by_the_next_one() {
    let (_s, store) = store();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("slot");
    let journal = root.path().join("slot.swap.json");
    let a = tree(
        &store,
        &[("one.txt", "a1"), ("two.txt", "a2"), ("three.txt", "a3")],
    );
    let b = tree(
        &store,
        &[("one.txt", "b1"), ("two.txt", "b2"), ("three.txt", "b3")],
    );
    Workdir::open(&dir, &journal, Keep::default())
        .unwrap()
        .become_tree(&store, a)
        .unwrap();
    // What a crash after the second file of a swap to `b` leaves: the
    // journal says a swap to `b` started, one file is b's, one torn.
    let mut j: serde_json::Value = serde_json::from_slice(&fs::read(&journal).unwrap()).unwrap();
    j["swapping_to"] = serde_json::to_value(b).unwrap();
    fs::write(&journal, serde_json::to_vec(&j).unwrap()).unwrap();
    fs::write(dir.join("one.txt"), "b1").unwrap();
    // Torn, with the size the journal recorded for a's content.
    fs::write(dir.join("two.txt"), "b").unwrap();
    fs::write(dir.join("two.txt"), "bX").unwrap();
    let mut w = Workdir::open(&dir, &journal, Keep::default()).unwrap();
    assert_eq!(w.at(), None, "a cut swap is trusted for nothing");
    w.become_tree(&store, b).unwrap();
    assert_eq!(
        listing(&dir, &[]),
        [
            ("one.txt".to_owned(), "b1".to_owned()),
            ("three.txt".to_owned(), "b3".to_owned()),
            ("two.txt".to_owned(), "b2".to_owned()),
        ]
    );
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// A CI checkout: a Git repository with `files` committed, and the same
/// files as an Aver tree.
fn checkout(store: &Store, files: &[(&str, &str)]) -> (tempfile::TempDir, GitSha, TreeId) {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    for (path, text) in files {
        let at = repo.path().join(path);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(at, text).unwrap();
    }
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-q", "-m", "c"]);
    let sha: GitSha = git(repo.path(), &["rev-parse", "HEAD"])
        .trim()
        .parse()
        .unwrap();
    (repo, sha, tree(store, files))
}

/// Defect: adopting a checkout with local edits verifies something else
/// and loses the edits on restore.
#[test]
fn adopt_refuses_a_dirty_checkout_and_cleans_a_clean_one() {
    let (_s, store) = store();
    let files = [("src/a.py", "a = 1\n"), ("README", "r\n")];
    let (repo, sha, t) = checkout(&store, &files);
    fs::write(repo.path().join("src/a.py"), "a = 2\n").unwrap();
    match Workdir::adopt(repo.path(), &store, (sha.clone(), t), keep(&[".venv"])) {
        Err(WorkdirError::Dirty(paths)) => assert_eq!(paths, ["src/a.py"]),
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert_eq!(read(repo.path(), "src/a.py"), "a = 2\n", "nothing changed");

    git(repo.path(), &["checkout", "--", "."]);
    fs::create_dir_all(repo.path().join(".venv/bin")).unwrap();
    fs::write(repo.path().join(".venv/bin/python"), "py").unwrap();
    fs::write(repo.path().join("stray.log"), "x").unwrap();
    Workdir::adopt(repo.path(), &store, (sha, t), keep(&[".venv"])).unwrap();
    assert!(!repo.path().join("stray.log").exists());
    assert!(repo.path().join(".venv/bin/python").exists());
    assert!(repo.path().join(".git/HEAD").exists());
}

/// Defect: a killed CI job leaves the checkout at another side's tree for
/// the workflow's later steps, and the restore needs the Aver home it lost.
#[test]
fn restore_puts_the_commit_back_from_git_alone() {
    let (_s, store) = store();
    let files = [
        ("src/a.py", "head\n"),
        ("tests/test_a.py", "t\n"),
        ("only_head.py", "h\n"),
    ];
    let (repo, sha, t) = checkout(&store, &files);
    let base = tree(
        &store,
        &[
            ("src/a.py", "base\n"),
            ("tests/test_a.py", "t\n"),
            ("only_base.py", "b\n"),
        ],
    );
    let mut w = Workdir::adopt(repo.path(), &store, (sha, t), Keep::default()).unwrap();
    w.become_tree(&store, base).unwrap();
    assert_eq!(read(repo.path(), "src/a.py"), "base\n");
    drop(w);
    drop(store);
    let restored = Workdir::restore(repo.path()).unwrap();
    assert!(restored.written >= 2, "{restored:?}");
    assert_eq!(git(repo.path(), &["status", "--porcelain"]), "");
    assert_eq!(read(repo.path(), "src/a.py"), "head\n");
    assert!(!repo.path().join(".git/aver-swap.json").exists());
}

/// Defect: a build step rewrites a compiled test file (`dist/test/*.js`)
/// and the run's evidence comes from the rewrite, unseen.
#[test]
fn derived_lists_built_test_files_by_content() {
    let (_s, store) = store();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("slot");
    let t = tree(&store, &[("src/v4.ts", "x")]);
    let mut w = Workdir::open(&dir, &root.path().join("j.json"), keep(&["node_modules"])).unwrap();
    w.become_tree(&store, t).unwrap();
    fs::create_dir_all(dir.join("dist/test")).unwrap();
    fs::create_dir_all(dir.join("node_modules/x/test")).unwrap();
    fs::write(dir.join("dist/test/v4.test.js"), "one").unwrap();
    fs::write(dir.join("node_modules/x/test/v4.test.js"), "dep").unwrap();
    let globs = [Glob::new("**/*.test.js").unwrap()];
    let first = w.derived(&globs).unwrap();
    assert_eq!(
        first,
        [("dist/test/v4.test.js".to_owned(), BlobId::of_bytes(b"one"))]
    );
    fs::write(dir.join("dist/test/v4.test.js"), "two").unwrap();
    let second = w.derived(&globs).unwrap();
    assert_ne!(first, second);
}

/// Defect: a non-editable install leaves the base's copy of a changed
/// module in `site-packages`, and the head's tests import it.
#[test]
fn probe_copies_finds_a_stale_installed_copy_but_not_an_editable_install() {
    let (_s, store) = store();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("slot");
    let t = tree(&store, &[("src/pkg/foo.py", "head")]);
    let mut w = Workdir::open(&dir, &root.path().join("j.json"), keep(&[".venv"])).unwrap();
    w.become_tree(&store, t).unwrap();
    let site = dir.join(".venv/lib/python3.12/site-packages");
    fs::create_dir_all(site.join("pkg")).unwrap();
    fs::write(site.join("pkg/foo.py"), "base").unwrap();
    let changed = [Changed {
        path: "src/pkg/foo.py".into(),
        base: Some(store.put_blob(b"base").unwrap()),
        head: Some(store.put_blob(b"head").unwrap()),
    }];
    let stale = w.probe_copies(&store, &changed).unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(
        stale[0].keep,
        ".venv/lib/python3.12/site-packages/pkg/foo.py"
    );

    fs::remove_dir_all(site.join("pkg")).unwrap();
    fs::write(
        site.join("_pkg.pth"),
        dir.join("src").to_string_lossy().as_bytes(),
    )
    .unwrap();
    assert!(w.probe_copies(&store, &changed).unwrap().is_empty());
}

/// Defect (JS-3, chalk): a dependency's published copy of the package
/// under test (ava depends on chalk), or any installed file that only
/// shares a name with a changed one, reads as a stale copy and blocks
/// every patch; or a copy of the repository's own package installed from
/// a tarball is missed.
#[test]
fn probe_copies_ignores_registry_installs_and_other_packages() {
    let (_s, store) = store();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("slot");
    let t = tree(
        &store,
        &[
            ("package.json", "{\"name\": \"chalk\"}"),
            ("source/index.js", "head"),
        ],
    );
    let mut w = Workdir::open(&dir, &root.path().join("j.json"), keep(&["node_modules"])).unwrap();
    w.become_tree(&store, t).unwrap();
    let put = |rel: &str| {
        let at = dir.join(rel);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(at, "base").unwrap();
    };
    put("node_modules/chalk/source/index.js");
    put("node_modules/other/source/index.js");
    put("node_modules/.pnpm/chalk@6.0.1/node_modules/chalk/source/index.js");
    let changed = [Changed {
        path: "source/index.js".into(),
        base: Some(store.put_blob(b"base").unwrap()),
        head: Some(store.put_blob(b"head").unwrap()),
    }];
    let found = |w: &Workdir| -> Vec<String> {
        w.probe_copies(&store, &changed)
            .unwrap()
            .into_iter()
            .map(|s| s.keep)
            .collect()
    };
    assert_eq!(found(&w), ["node_modules/chalk/source/index.js"]);
    fs::write(
        dir.join("node_modules/.package-lock.json"),
        r#"{"packages": {"node_modules/chalk": {"version": "6.0.1", "resolved": "https://registry.npmjs.org/chalk/-/chalk-6.0.1.tgz"}}}"#,
    )
    .unwrap();
    assert!(found(&w).is_empty(), "{:?}", found(&w));
}

/// The probe a verification runs on the directory the head's slots are
/// cloned from: only its kept directories, never its build caches.
#[test]
fn stale_copies_reads_the_kept_directories_of_a_slot_source() {
    let (_s, store) = store();
    let root = tempfile::tempdir().unwrap();
    let src = root.path();
    fs::create_dir_all(src.join(".venv/site-packages/pkg")).unwrap();
    fs::write(src.join(".venv/site-packages/pkg/foo.py"), "base").unwrap();
    fs::create_dir_all(src.join("target/pkg")).unwrap();
    fs::write(src.join("target/pkg/foo.py"), "base").unwrap();
    let changed = [Changed {
        path: "src/pkg/foo.py".into(),
        base: Some(store.put_blob(b"base").unwrap()),
        head: Some(store.put_blob(b"head").unwrap()),
    }];
    let source = aver::place::sides::SlotSource {
        dir: src,
        keep: vec![".venv".into()],
        seed: vec!["target".into()],
        inputs: Vec::new(),
    };
    let stale = aver::place::sides::stale_copies(&store, &source, &changed).unwrap();
    let kept: Vec<&str> = stale.iter().map(|s| s.keep.as_str()).collect();
    assert_eq!(kept, [".venv/site-packages/pkg/foo.py"]);
}

// ---- sides ----

use aver::disk::CloneStrategy;
use aver::place::sides::{relocate, Sides, SlotName, SlotSource};

fn source<'a>(dir: &'a Path, keep: &[&str], seed: &[&str], inputs: &[&str]) -> SlotSource<'a> {
    let own = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect();
    SlotSource {
        dir,
        keep: own(keep),
        seed: own(seed),
        inputs: own(inputs),
    }
}

fn write(dir: &Path, path: &str, text: &str) {
    let at = dir.join(path);
    fs::create_dir_all(at.parent().unwrap()).unwrap();
    fs::write(at, text).unwrap();
}

/// Sets `path`'s mtime `secs` seconds into the past.
fn age(dir: &Path, path: &str, secs: u64) {
    let file = fs::File::options()
        .write(true)
        .open(dir.join(path))
        .unwrap();
    file.set_modified(std::time::SystemTime::now() - Duration::from_secs(secs))
        .unwrap();
}

/// The fixture's build: copies `src/a.txt` to `out/a.txt` only when the
/// source is newer, as make and Cargo decide.
fn build(dir: &Path) {
    let ok = Command::new("sh")
        .args(["-c", "mkdir -p out; [ -e out/a.txt ] && [ ! src/a.txt -nt out/a.txt ] || cp src/a.txt out/a.txt"])
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok);
}

/// Defect (I1): a slot cloned from the agent's directory keeps the agent's
/// build output, built from the head, and the base side's build takes it
/// as fresh because the base's sources kept older mtimes.
#[test]
fn a_slot_never_reuses_an_output_built_from_other_content() {
    let (home, store) = store();
    let agent = tempfile::tempdir().unwrap();
    write(agent.path(), "src/a.txt", "base");
    std::thread::sleep(Duration::from_millis(20));
    write(agent.path(), "src/a.txt", "head");
    build(agent.path());
    assert_eq!(read(agent.path(), "out/a.txt"), "head");
    // macOS's sh compares -nt in whole seconds: age both files so the
    // swap's write is newer than the output by more than that.
    age(agent.path(), "src/a.txt", 20);
    age(agent.path(), "out/a.txt", 10);
    let base = tree(&store, &[("src/a.txt", "base")]);
    let head = tree(&store, &[("src/a.txt", "head")]);
    let sides = Sides::new(home.path(), store.clone()).with_strategy(CloneStrategy::Reflink);
    let src = source(agent.path(), &[], &["out"], &[]);
    let slot = sides
        .enter(agent.path(), &SlotName::Base(base), &src, base)
        .unwrap();
    build(&slot.dir);
    assert_eq!(read(&slot.dir, "out/a.txt"), "base");
    drop(slot);

    let ws: aver_core::WorkspaceId = format!("ws_{}", ulid::Ulid::generate()).parse().unwrap();
    let slot = sides
        .enter(agent.path(), &SlotName::Judged(ws), &src, head)
        .unwrap();
    build(&slot.dir);
    assert_eq!(read(&slot.dir, "out/a.txt"), "head");
    let next = tree(&store, &[("src/a.txt", "edited again")]);
    drop(slot);
    let slot = sides
        .enter(agent.path(), &SlotName::Judged(ws), &src, next)
        .unwrap();
    build(&slot.dir);
    assert_eq!(read(&slot.dir, "out/a.txt"), "edited again");
}

/// A transplant slot starts as a clone of its base slot, whose build
/// output was made from the same sources, when no run holds the base and
/// it was cloned with the workspace's dependencies; otherwise it is cloned
/// from the workspace directory as before.
#[test]
fn a_transplant_starts_from_its_idle_base_slot_with_the_same_dependencies() {
    let (home, store) = store();
    let repo = tempfile::tempdir().unwrap();
    let agent = tempfile::tempdir().unwrap();
    for dir in [repo.path(), agent.path()] {
        write(dir, "package-lock.json", "{}");
        write(dir, "node_modules/dep/index.js", "dep");
        write(dir, "src/a.txt", "base");
    }
    write(agent.path(), "out/a.txt", "built in the workspace");
    let base = tree(
        &store,
        &[("package-lock.json", "{}"), ("src/a.txt", "base")],
    );
    let transplant = tree(
        &store,
        &[
            ("package-lock.json", "{}"),
            ("src/a.txt", "base"),
            ("tests/a.txt", "head test"),
        ],
    );
    let sides = Sides::new(home.path(), store.clone()).with_strategy(CloneStrategy::Reflink);
    let from_repo = source(
        repo.path(),
        &["node_modules"],
        &["out"],
        &["package-lock.json"],
    );
    let from_agent = source(
        agent.path(),
        &["node_modules"],
        &["out"],
        &["package-lock.json"],
    );
    let b = SlotName::Base(base);
    let slot = sides.enter(repo.path(), &b, &from_repo, base).unwrap();
    write(&slot.dir, "out/a.txt", "built in the base slot");
    drop(slot);
    let ws =
        || -> aver_core::WorkspaceId { format!("ws_{}", ulid::Ulid::generate()).parse().unwrap() };
    let enter = |ws| {
        sides
            .enter_seeded(
                repo.path(),
                &SlotName::Transplant(ws),
                &from_agent,
                Some(&b),
                transplant,
            )
            .unwrap()
    };

    let held = sides.enter(repo.path(), &b, &from_repo, base).unwrap();
    let busy = enter(ws());
    assert_eq!(read(&busy.dir, "out/a.txt"), "built in the workspace");
    drop((busy, held));

    let seeded = enter(ws());
    assert_eq!(read(&seeded.dir, "out/a.txt"), "built in the base slot");
    assert_eq!(read(&seeded.dir, "tests/a.txt"), "head test");
    assert_eq!(read(&seeded.dir, "node_modules/dep/index.js"), "dep");
    let record = fs::read_to_string(seeded.dir.with_extension("source.json")).unwrap();
    assert!(
        record.contains(&*agent.path().to_string_lossy()),
        "the record names the workspace: {record}"
    );
    drop(seeded);

    write(agent.path(), "package-lock.json", r#"{"other":1}"#);
    let other_deps = enter(ws());
    assert_eq!(read(&other_deps.dir, "out/a.txt"), "built in the workspace");
}

/// Defect: a run that writes into its installed dependencies changes the
/// agent's own `node_modules`.
#[test]
fn a_slot_sees_the_sources_dependencies_and_never_writes_them() {
    let (home, store) = store();
    let agent = tempfile::tempdir().unwrap();
    write(agent.path(), "index.js", "x");
    write(agent.path(), "node_modules/dep/index.js", "dep");
    let t = tree(&store, &[("index.js", "x")]);
    let sides = Sides::new(home.path(), store.clone()).with_strategy(CloneStrategy::Reflink);
    let src = source(agent.path(), &["node_modules"], &[], &["package-lock.json"]);
    let slot = sides
        .enter(agent.path(), &SlotName::Base(t), &src, t)
        .unwrap();
    assert_eq!(read(&slot.dir, "node_modules/dep/index.js"), "dep");
    fs::write(
        slot.dir.join("node_modules/dep/index.js"),
        "changed by a run",
    )
    .unwrap();
    assert_eq!(read(agent.path(), "node_modules/dep/index.js"), "dep");
}

/// Defect: a virtualenv cloned into a slot still imports the agent's
/// editable source and runs the agent's interpreter, so the base side
/// tests the head.
#[test]
fn relocate_points_installed_paths_at_the_slot() {
    let src = tempfile::tempdir().unwrap();
    let slot = tempfile::tempdir().unwrap();
    let from = src.path().to_string_lossy().into_owned();
    let to = slot.path().to_string_lossy().into_owned();
    let site = ".venv/lib/python3.12/site-packages";
    for (dir, keep_files) in [(src.path(), true), (slot.path(), false)] {
        let _ = keep_files;
        write(dir, &format!("{site}/_pkg.pth"), &format!("{from}/src\n"));
        write(
            dir,
            &format!("{site}/pkg-1.0.dist-info/direct_url.json"),
            &format!("{{\"url\": \"file://{from}\"}}"),
        );
        write(
            dir,
            "node_modules/.modules.yaml",
            &format!("virtualStoreDir: {from}/node_modules/.pnpm\n"),
        );
        write(
            dir,
            ".venv/bin/pytest",
            &format!("#!{from}/.venv/bin/python\nimport pytest\n"),
        );
        fs::write(
            dir.join(".venv/lib/native.so"),
            [b"\0ELF".as_slice(), from.as_bytes(), b"\0"].concat(),
        )
        .unwrap();
        std::os::unix::fs::symlink("../lib", dir.join(".venv/rel")).unwrap();
    }
    let keep = Keep::new([".venv", "node_modules"]).unwrap();
    let done = relocate(slot.path(), src.path(), &keep).unwrap();
    assert_eq!(
        read(slot.path(), &format!("{site}/_pkg.pth")),
        format!("{to}/src\n")
    );
    assert!(read(
        slot.path(),
        &format!("{site}/pkg-1.0.dist-info/direct_url.json")
    )
    .contains(&to));
    assert!(read(slot.path(), "node_modules/.modules.yaml").contains(&to));
    assert!(read(slot.path(), ".venv/bin/pytest").starts_with(&format!("#!{to}/.venv/bin/python")));
    assert_eq!(done.binary, [".venv/lib/native.so"]);
    assert_eq!(
        fs::read_link(slot.path().join(".venv/rel")).unwrap(),
        Path::new("../lib")
    );
    assert_eq!(
        read(src.path(), &format!("{site}/_pkg.pth")),
        format!("{from}/src\n"),
        "the source is unchanged"
    );
}

/// Defect: every verification recopies the dependencies, or a slot keeps
/// dependencies installed from a lockfile the source no longer has.
#[test]
fn a_slot_is_reused_until_the_sources_lockfile_changes() {
    let (home, store) = store();
    let agent = tempfile::tempdir().unwrap();
    write(agent.path(), "package-lock.json", "v1");
    write(agent.path(), "node_modules/sentinel", "s");
    let t = tree(&store, &[("package-lock.json", "v1")]);
    let sides = Sides::new(home.path(), store.clone()).with_strategy(CloneStrategy::Reflink);
    let src = source(agent.path(), &["node_modules"], &[], &["package-lock.json"]);
    let ino = |d: &Path| fs::metadata(d.join("node_modules/sentinel")).unwrap().ino();
    let first = sides
        .enter(agent.path(), &SlotName::Base(t), &src, t)
        .unwrap();
    let (dir, before) = (first.dir.clone(), ino(&first.dir));
    assert!(!first.deps_differ);
    drop(first);
    let again = sides
        .enter(agent.path(), &SlotName::Base(t), &src, t)
        .unwrap();
    assert_eq!(ino(&again.dir), before, "reused");
    drop(again);
    write(agent.path(), "package-lock.json", "v2");
    let t2 = tree(&store, &[("package-lock.json", "v2")]);
    let fresh = sides
        .enter(agent.path(), &SlotName::Base(t), &src, t2)
        .unwrap();
    assert_eq!(fresh.dir, dir);
    assert_ne!(ino(&fresh.dir), before, "recloned");
    drop(fresh);
    // The slot's tree asks for other inputs than the source installed from.
    let slot = sides
        .enter(agent.path(), &SlotName::Base(t), &src, t)
        .unwrap();
    assert!(slot.deps_differ);
}

/// Defect: a base slot cloned from dependency inputs its source no longer
/// has stays a week, though its next entry would reclone it; one recorded
/// before the patterns were kept, or held by a run, stays.
#[test]
fn a_base_slot_whose_sources_lockfile_changed_is_stale() {
    let (home, store) = store();
    let agent = tempfile::tempdir().unwrap();
    write(agent.path(), "package-lock.json", "v1");
    write(agent.path(), "node_modules/sentinel", "s");
    let t = tree(&store, &[("package-lock.json", "v1")]);
    let sides = Sides::new(home.path(), store.clone()).with_strategy(CloneStrategy::Reflink);
    let src = source(agent.path(), &["node_modules"], &[], &["package-lock.json"]);
    let slot = sides
        .enter(agent.path(), &SlotName::Base(t), &src, t)
        .unwrap();
    let dir = slot.dir.clone();
    drop(slot);
    let live: std::collections::BTreeSet<_> = [t].into_iter().collect();
    let week = Duration::from_secs(7 * 24 * 3600);
    let none = std::collections::BTreeSet::new();
    assert!(sides.stale(&live, &none, week).is_empty(), "current inputs");

    write(agent.path(), "package-lock.json", "v2");
    assert_eq!(sides.stale(&live, &none, week), std::slice::from_ref(&dir));
    let held = sides
        .enter(agent.path(), &SlotName::Base(t), &src, t)
        .unwrap();
    write(agent.path(), "package-lock.json", "v3");
    assert!(sides.stale(&live, &none, week).is_empty(), "a run holds it");
    drop(held);

    let record = dir.with_extension("source.json");
    let mut old: serde_json::Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    old.as_object_mut()
        .unwrap()
        .retain(|k, _| k == "source" || k == "deps");
    fs::write(&record, serde_json::to_vec(&old).unwrap()).unwrap();
    write(agent.path(), "package-lock.json", "v4");
    assert!(
        sides.stale(&live, &none, week).is_empty(),
        "an old record cannot tell"
    );
}

/// Defect: released workspaces' slots and crashed clones fill the disk; a
/// run outside a verification writes a base slot later verifications trust.
#[test]
fn released_and_stale_slots_go_and_snapshot_runs_never_touch_a_base_slot() {
    let (home, store) = store();
    let agent = tempfile::tempdir().unwrap();
    write(agent.path(), "a", "a");
    let t = tree(&store, &[("a", "a")]);
    let sides = Sides::new(home.path(), store.clone()).with_strategy(CloneStrategy::Reflink);
    let src = source(agent.path(), &[], &[], &[]);
    let ws: aver_core::WorkspaceId = format!("ws_{}", ulid::Ulid::generate()).parse().unwrap();
    drop(
        sides
            .enter(agent.path(), &SlotName::Judged(ws), &src, t)
            .unwrap(),
    );
    drop(
        sides
            .enter(agent.path(), &SlotName::Transplant(ws), &src, t)
            .unwrap(),
    );
    drop(
        sides
            .enter(agent.path(), &SlotName::Base(t), &src, t)
            .unwrap(),
    );
    let repo = sides.repo_dir(agent.path());
    fs::create_dir_all(repo.join("B-x.tmp-01ABC")).unwrap();
    let live = std::collections::BTreeSet::new();
    let stale = sides.stale(&live, &[ws].into_iter().collect(), Duration::ZERO);
    let names: Vec<String> = stale
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["B-x.tmp-01ABC".to_owned(), SlotName::Base(t).dir()]);
    assert_eq!(sides.remove_for(&ws).unwrap(), 2);
    assert!(!repo.join(SlotName::Judged(ws).dir()).exists());
    assert!(!repo.join(SlotName::Transplant(ws).dir()).exists());

    let home = tempfile::tempdir().unwrap();
    let sides = Sides::new(home.path(), store.clone()).with_strategy(CloneStrategy::Reflink);
    let snap: aver_core::SnapshotId = format!("snap_{}", "a".repeat(52)).parse().unwrap();
    drop(
        sides
            .enter(agent.path(), &SlotName::Snapshot(snap), &src, t)
            .unwrap(),
    );
    let made: Vec<String> = fs::read_dir(sides.repo_dir(agent.path()))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.contains('.'))
        .collect();
    assert_eq!(made, [SlotName::Snapshot(snap).dir()]);
}

// ---------------------------------------------------------------------
// The executor on slots (wp2-plan.md §1.3-1.4).

mod slots {
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path as FsPath;
    use std::sync::Arc;

    use aver::exec::{ExecConfig, ExecutedRun, Executor};
    use aver::place::runner::{RunnerConfig, SlotRunner};
    use aver::verify::{Head, Request, Verifier};
    use aver_core::*;

    use super::common;
    use super::world::{World, PINNED_IMAGE};

    /// An `aver.toml` whose checks are `name = shell script` pairs.
    pub fn shell_toml(checks: &[(&str, &str)]) -> String {
        let mut toml = format!("[env]\nimage = \"{PINNED_IMAGE}\"\n");
        for (id, script) in checks {
            let script = script.replace('\\', "\\\\").replace('"', "\\\"");
            toml.push_str(&format!(
                "\n[[check]]\nid = \"{id}\"\nrun = [\"sh\", \"-c\", \"{script}\"]\n"
            ));
        }
        toml
    }

    /// This process's environment plus `extra`: what a run inherits.
    fn caller(extra: &[(&str, &str)]) -> BTreeMap<String, String> {
        let mut env: BTreeMap<String, String> = std::env::vars().collect();
        for (k, v) in extra {
            env.insert((*k).to_owned(), (*v).to_owned());
        }
        env
    }

    fn executor(w: &World, vars: BTreeMap<String, String>, mode: ProvenanceMode) -> Executor {
        let config = RunnerConfig {
            vars,
            ..RunnerConfig::for_process(w.home.root(), mode)
        };
        Executor::slots(
            w.store.clone(),
            SlotRunner::new(w.store.clone(), config),
            ExecConfig::default(),
        )
    }

    fn local(w: &World, vars: BTreeMap<String, String>) -> Executor {
        executor(w, vars, ProvenanceMode::Local)
    }

    fn env_of(w: &World, snapshot: &SnapshotId) -> Environment {
        let snap: Snapshot = w.store.get(snapshot).unwrap();
        w.store.get(&snap.env.unwrap()).unwrap()
    }

    /// Runs check `id` of `snapshot`'s own environment once on `exec`.
    async fn run(exec: &Executor, w: &World, snapshot: &SnapshotId, id: &str) -> ExecutedRun {
        let env = env_of(w, snapshot);
        let check = env
            .checks
            .iter()
            .find(|c| c.id.as_str() == id)
            .unwrap()
            .clone();
        exec.run(snapshot, &env, &check, RunIndex::FIRST)
            .await
            .unwrap()
    }

    fn output(w: &World, run: &CheckRun) -> String {
        common::output_text(&w.store, run)
    }

    fn executable(path: &FsPath, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn has(tool: &str) -> bool {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .any(|d| d.join(tool).is_file())
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// Defect: a run that does not see the caller's toolchain variables
    /// fails where the user's own `npm test` passes; one that sees the CI
    /// job's tokens or output files can write to the job; slots sharing a
    /// `CARGO_TARGET_DIR` reuse head-built units on the base side.
    #[tokio::test]
    async fn a_run_inherits_the_callers_environment_minus_ci_tokens() {
        let w = World::new();
        let mut toml = shell_toml(&[("env", "env; touch \"$AVER_OUT/mark\"")]);
        toml.push_str("\n[vars]\nGREETING = \"hello\"\n");
        let snap = w.snapshot(&[("aver.toml", &toml)]);
        let stripped = [
            "GITHUB_OUTPUT",
            "GITHUB_ENV",
            "GITHUB_PATH",
            "GITHUB_STEP_SUMMARY",
            "ACTIONS_RUNTIME_TOKEN",
            "ACTIONS_CACHE_URL",
            "ACTIONS_ID_TOKEN_REQUEST_URL",
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
            "GITHUB_TOKEN",
            "AVER_FAULT",
        ];
        let mut extra: Vec<(&str, &str)> = stripped.iter().map(|k| (*k, "leak")).collect();
        extra.push(("AVER_TEST_MARK", "1"));
        extra.push(("CARGO_TARGET_DIR", "/elsewhere"));
        let exec = local(&w, caller(&extra));
        let ran = run(&exec, &w, &snap, "env").await;
        let text = output(&w, &ran.run);
        assert_eq!(ran.run.result.outcome(), Outcome::Pass, "{text}");
        let vars: BTreeMap<&str, &str> = text.lines().filter_map(|l| l.split_once('=')).collect();
        assert_eq!(vars.get("AVER_TEST_MARK"), Some(&"1"), "{text}");
        assert_eq!(
            vars.get("PATH").copied(),
            Some(std::env::var("PATH").unwrap().as_str())
        );
        assert_eq!(vars.get("GREETING"), Some(&"hello"));
        for name in stripped {
            assert!(!vars.contains_key(name), "{name} reached the run:\n{text}");
        }
        let target = vars["CARGO_TARGET_DIR"];
        assert!(
            target.ends_with("/target") && target.contains("/sides/"),
            "{target}"
        );
        assert!(vars["TMPDIR"].starts_with("/tmp/aver-"), "{text}");
        assert!(
            !FsPath::new(vars["TMPDIR"]).exists(),
            "the run's TMPDIR outlived it"
        );
        assert!(vars["AVER_OUT"].ends_with("/out"), "{text}");
        assert_eq!(exec.provenance().network, Network::On);
        assert!(exec.executor().starts_with("native-v0."));
    }

    /// Defect: a slot run's built test files go unrecorded, so a head
    /// build that rewrites them is never compared with the base's.
    #[tokio::test]
    async fn a_slot_run_records_the_built_test_files_it_left() {
        let w = World::new();
        let toml = shell_toml(&[(
            "build",
            "mkdir -p dist/test && echo one > dist/test/v4.test.js && echo two > dist/main.js",
        )]);
        let snap = w.snapshot(&[("aver.toml", &toml)]);
        let exec = local(&w, caller(&[]));
        let ran = run(&exec, &w, &snap, "build").await;
        assert_eq!(
            ran.run.result.outcome(),
            Outcome::Pass,
            "{}",
            output(&w, &ran.run)
        );
        let built = aver::exec::built::of(&w.store, &ran.id).unwrap();
        assert_eq!(
            built,
            Some(BTreeMap::from([(
                "dist/test/v4.test.js".to_owned(),
                BlobId::of_bytes(b"one\n")
            )]))
        );
    }

    /// Defect: a listener a test leaked holds its port into the next run
    /// (a base run after a head that leaked fails "address in use").
    #[tokio::test]
    async fn a_leaked_listener_never_reaches_a_later_run() {
        if !has("nc") || !has("perl") {
            eprintln!("no nc or perl here");
            return;
        }
        let w = World::new();
        let pids = w.dir.path().join("pids");
        fs::create_dir_all(&pids).unwrap();
        let (a, b) = (free_port(), free_port());
        let p = pids.display();
        let script = format!(
            "for port in {a} {b}; do if nc -z 127.0.0.1 $port; then echo busy $port; exit 1; fi; done; \
             if [ -f leak ]; then \
               nohup nc -lk 127.0.0.1 {a} >/dev/null 2>&1 & echo $! > {p}/group; \
               perl -MPOSIX -e 'POSIX::setsid(); open(my $f, \">\", $ARGV[0]); print $f $$; close $f; exec(\"nc\", \"-lk\", \"127.0.0.1\", $ARGV[1])' {p}/session {b} >/dev/null 2>&1 & \
               for i in 1 2 3 4 5 6 7 8 9 10; do nc -z 127.0.0.1 {a} && nc -z 127.0.0.1 {b} && break; sleep 0.3; done; sleep 0.5; \
             fi; echo free"
        );
        let toml = shell_toml(&[("serve", &script)]);
        let base = w.snapshot(&[("aver.toml", &toml)]);
        let head = w.snapshot(&[("aver.toml", &toml), ("leak", "")]);
        let exec = local(&w, caller(&[]));
        let h = run(&exec, &w, &head, "serve").await;
        let text = output(&w, &h.run);
        assert_eq!(h.run.result.outcome(), Outcome::Pass, "{text}");
        assert!(text.contains("[aver] the run left"), "{text}");
        for leaked in ["group", "session"] {
            let pid = fs::read_to_string(pids.join(leaked)).unwrap();
            // A killed process may still be exiting on a loaded machine.
            let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while aver::run::state::process_alive(pid.trim().parse().unwrap())
                && std::time::Instant::now() < until
            {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            assert!(
                !aver::run::state::process_alive(pid.trim().parse().unwrap()),
                "the {leaked} listener {pid} outlived its run"
            );
        }
        let b = run(&exec, &w, &base, "serve").await;
        let text = output(&w, &b.run);
        assert_eq!(b.run.result.outcome(), Outcome::Pass, "{text}");
        assert!(!text.contains("[aver] the run left"), "{text}");
    }

    /// Defect: a run SIGKILLed from outside (the out-of-memory killer) read
    /// as failing proves a criterion on the transplant, and is cached.
    #[tokio::test]
    async fn a_run_killed_from_outside_is_infra() {
        let w = World::new();
        let snap = w.snapshot(&[("aver.toml", &shell_toml(&[("killed", "kill -9 $$")]))]);
        let exec = local(&w, caller(&[]));
        let first = run(&exec, &w, &snap, "killed").await;
        assert_eq!(
            first.run.result.outcome(),
            Outcome::Infra,
            "{}",
            output(&w, &first.run)
        );
        assert!(!run(&exec, &w, &snap, "killed").await.cache_hit);
    }

    /// Defect: concurrent runs of one suite collide on fixed names in a
    /// shared `TMPDIR`, or one run reads what an earlier one left there.
    #[tokio::test]
    async fn each_run_has_its_own_unpredictable_tmpdir() {
        let w = World::new();
        let script = "echo \"$TMPDIR\"; ls -ld \"$TMPDIR\" | cut -c1-10; cat n";
        let toml = shell_toml(&[("tmp", script)]);
        let exec = local(&w, caller(&[]));
        let mut seen = Vec::new();
        for n in ["1", "2"] {
            let snap = w.snapshot(&[("aver.toml", &toml), ("n", n)]);
            let text = output(&w, &run(&exec, &w, &snap, "tmp").await.run);
            let mut lines = text.lines();
            let tmp = lines.next().unwrap().to_owned();
            assert!(tmp.starts_with("/tmp/aver-"), "{text}");
            assert_eq!(lines.next(), Some("drwx------"), "{text}");
            assert!(!FsPath::new(&tmp).exists(), "{tmp} outlived its run");
            seen.push(tmp);
        }
        assert_ne!(seen[0], seen[1], "two runs of one check shared a TMPDIR");
    }

    /// Defect (RUST-2): on macOS a new file takes its directory's group, and
    /// `/tmp`'s is `wheel`, so a test checking that its files belong to the
    /// user's group (fd's `test_owner_current_group`) failed on both sides.
    #[tokio::test]
    async fn a_file_made_in_tmpdir_has_the_runs_group() {
        let w = World::new();
        let script = "touch \"$TMPDIR/f\"; id -g; ls -ln \"$TMPDIR/f\" | awk '{print $4}'";
        let toml = shell_toml(&[("gid", script)]);
        let exec = local(&w, caller(&[]));
        let snap = w.snapshot(&[("aver.toml", &toml)]);
        let text = output(&w, &run(&exec, &w, &snap, "gid").await.run);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert_eq!(
            lines[0], lines[1],
            "the file's group is not the run's: {text}"
        );
    }

    /// Defect: a check written `python -m pytest` (generic mode, `aver
    /// check -- pytest`) fails as a missing tool on a host that ships only
    /// `python3`, now that Aver installs no venv of its own.
    #[tokio::test]
    async fn python_runs_as_python3_where_only_python3_is_on_path() {
        let w = World::new();
        let tools = w.dir.path().join("only-python3");
        executable(
            &tools.join("python3"),
            "#!/bin/sh\necho \"python3 ran $*\"\n",
        );
        let toml = format!(
            "[env]\nimage = \"{PINNED_IMAGE}\"\n\n[[check]]\nid = \"py\"\nrun = [\"python\", \"-m\", \"pytest\"]\n"
        );
        let snap = w.snapshot(&[("aver.toml", &toml)]);
        let exec = local(&w, caller(&[("PATH", &tools.to_string_lossy())]));
        let out = output(&w, &run(&exec, &w, &snap, "py").await.run);
        assert!(out.contains("python3 ran -m pytest"), "{out}");
    }

    /// Defect: a run sees files an earlier run of another tree left, or
    /// files of that tree the next one lacks.
    #[tokio::test]
    async fn each_run_starts_from_exactly_its_tree() {
        let w = World::new();
        let script =
            "ls -a | sort | tr '\\n' ' '; cat keep; echo junk > left-behind; echo x >> keep";
        let toml = shell_toml(&[("tree", script)]);
        let a = w.snapshot(&[("aver.toml", &toml), ("keep", "a\n"), ("only-a", "")]);
        let b = w.snapshot(&[("aver.toml", &toml), ("keep", "a\n"), ("only-b", "")]);
        let exec = local(&w, caller(&[]));
        let first = output(&w, &run(&exec, &w, &a, "tree").await.run);
        assert!(
            first.contains("only-a") && !first.contains("left-behind"),
            "{first}"
        );
        let base = |snap: &SnapshotId| w.store.get::<Snapshot>(snap).unwrap().root;
        // Both in one base slot, so the second run swaps the first's tree.
        let runner = exec.slot_runner().unwrap();
        let place = runner.base_place(None, base(&a));
        let second = output(&w, &run(&exec.in_slot(place), &w, &b, "tree").await.run);
        assert!(second.contains("only-b"), "{second}");
        assert!(!second.contains("only-a"), "{second}");
        assert!(!second.contains("left-behind"), "{second}");
        assert!(
            second.trim_end().ends_with('a'),
            "keep was not rewritten: {second}"
        );
    }

    /// Defect: a test parameterized with `__file__` or a temporary path
    /// (`requests`) has another id in every slot, so the base's test reads
    /// as missing from the head's report (§4.5). The slot's path, as given
    /// and resolved, and the run's `TMPDIR` read as `<src>` and `<tmp>`.
    #[tokio::test]
    async fn slot_and_tmpdir_paths_in_test_ids_read_the_same_in_every_slot() {
        let w = World::new();
        let script = "tmp=$(cd \"$TMPDIR\" && pwd -P); printf '<testsuites><testsuite name=\"s\"><testcase name=\"at[%s/data.txt]\"/><testcase name=\"real[%s]\"/><testcase name=\"tmp[%s/x]\"/><testcase name=\"tmpreal[%s/x]\"/><testcase name=\"where\"><failure>at %s/t.py:3</failure></testcase></testsuite></testsuites>' \"$PWD\" \"$(pwd -P)\" \"$TMPDIR\" \"$tmp\" \"$PWD\" > \"$AVER_OUT/r.xml\"";
        let mut toml = shell_toml(&[("ids", script)]);
        toml.push_str("report = { format = \"junit\", path = \"/aver/out/r.xml\" }\n");
        let a = w.snapshot(&[("aver.toml", &toml), ("a", "")]);
        let b = w.snapshot(&[("aver.toml", &toml), ("b", "")]);
        let exec = local(&w, caller(&[]));
        let ids = |run: &ExecutedRun| -> Vec<String> {
            run.run.tests.iter().map(|t| t.id.to_string()).collect()
        };
        let ran = run(&exec, &w, &a, "ids").await;
        assert_eq!(
            ids(&ran),
            [
                "ids::s > at[<src>/data.txt]",
                "ids::s > real[<src>]",
                "ids::s > tmp[<tmp>/x]",
                "ids::s > tmpreal[<tmp>/x]",
                "ids::s > where",
            ],
            "{}",
            output(&w, &ran.run)
        );
        let message = ran.run.tests.iter().find_map(|t| t.message.clone());
        assert_eq!(
            message.as_ref().map(|m| m.as_str()),
            Some("at <src>/t.py:3"),
            "a path in an XML text node stays text"
        );
        assert_eq!(ids(&run(&exec, &w, &b, "ids").await), ids(&ran));
    }

    /// Defect: a run made in CI mode, or on another toolchain, answers for
    /// a request that would make it otherwise, so a receipt claims a
    /// provenance or a host its reused runs did not have.
    #[tokio::test]
    async fn provenance_and_host_profile_are_part_of_the_cache_key() {
        let w = World::new();
        let snap = w.snapshot(&[("aver.toml", &shell_toml(&[("ok", "true")]))]);
        let local_exec = local(&w, caller(&[]));
        assert!(!run(&local_exec, &w, &snap, "ok").await.cache_hit);
        assert!(run(&local_exec, &w, &snap, "ok").await.cache_hit);
        let ci = executor(&w, caller(&[]), ProvenanceMode::Ci);
        assert_eq!(ci.executor(), local_exec.executor());
        assert!(
            !run(&ci, &w, &snap, "ok").await.cache_hit,
            "a ci run came from a local run"
        );
        let tools = |version: &str| {
            let dir = w.dir.path().join(format!("tools-{version}"));
            executable(&dir.join("node"), &format!("#!/bin/sh\necho v{version}\n"));
            caller(&[(
                "PATH",
                &format!("{}:{}", dir.display(), std::env::var("PATH").unwrap()),
            )])
        };
        let one = local(&w, tools("24.1.0"));
        let two = local(&w, tools("24.2.0"));
        assert_ne!(one.executor(), two.executor());
        assert_ne!(one.provenance().host_profile, two.provenance().host_profile);
        assert!(!run(&one, &w, &snap, "ok").await.cache_hit);
        assert!(run(&one, &w, &snap, "ok").await.cache_hit);
        let other = run(&two, &w, &snap, "ok").await;
        assert!(
            !other.cache_hit,
            "a run on another toolchain came from the cache"
        );
        assert_eq!(other.run.executor, two.executor());
    }

    /// Defect: `aver check` and the verify after it key runs differently,
    /// so the verify runs everything again.
    #[tokio::test]
    async fn check_then_verify_starts_zero_runs() {
        let w = World::new();
        let toml = shell_toml(&[("calc", "test \"$(cat answer)\" = 42")]);
        let base = w.snapshot(&[("aver.toml", &toml), ("answer", "41")]);
        let head = w.snapshot(&[("aver.toml", &toml), ("answer", "42")]);
        let task = w.create_task(w.task_body(&w.admin, base, &[]));
        let exec = local(&w, caller(&[]));
        let verifier = Verifier::new(
            w.store.clone(),
            w.workspaces.clone(),
            exec.clone(),
            Arc::new(aver_core::SigningKey::from_bytes(&[7; 32])),
        );
        let request = || Request {
            task,
            head: Head::Snapshot {
                head,
                base: Some(base),
            },
            attempt: None,
            fresh: false,
            all: false,
            progress: None,
            ci: None,
            adhoc: None,
            reason: None,
        };
        let dry = verifier.dry_run(w.admin.id(), request()).await.unwrap();
        assert!(
            dry.runs.iter().any(|r| !r.cache_hit),
            "the dry run ran nothing"
        );
        assert!(
            w.store.get::<Receipt>(&dry.id).is_err(),
            "a dry run stored its receipt"
        );
        let started = exec.sandbox_starts();
        let real = verifier.verify(w.admin.id(), request()).await.unwrap();
        assert_eq!(exec.sandbox_starts(), started, "the verify started runs");
        assert!(real.runs.iter().all(|r| r.cache_hit));
        assert_eq!(real.receipt.body.verdict, dry.receipt.body.verdict);
        assert_eq!(real.receipt.body.schema, ReceiptSchema::V3);
        let provenance = real.receipt.body.provenance.clone().unwrap();
        assert_eq!(provenance.mode, ProvenanceMode::Local);
        assert_eq!(provenance.uid, Uid::Same);
        assert_eq!(provenance.network, Network::On);
        assert_eq!(
            real.receipt
                .body
                .levels
                .as_ref()
                .unwrap()
                .get(&common::p::<CheckId>("calc")),
            Some(&Level::Exit)
        );
        let server = aver_core::SigningKey::from_bytes(&[7; 32]).verifying_key();
        assert!(real.receipt.verify(&server).is_ok());
        assert!(dry.receipt.verify(&server).is_err());
    }

    /// Defect: a kept verifier key shares a user with the code under test.
    #[test]
    fn ci_with_a_kept_home_needs_a_separate_user() {
        let dir = tempfile::tempdir().unwrap();
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_aver"))
            .args(["ci", "--repo", ".", "--base", "a", "--head", "b", "--home"])
            .arg(dir.path().join("home"))
            .arg("--out-dir")
            .arg(dir.path().join("out"))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("--run-as"), "{err}");
    }

    #[test]
    fn long_lines_are_cut() {
        let long = "x".repeat(5000);
        let out = aver::place::proc::cap_lines(format!("short\n{long}\nend\n").as_bytes());
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("short\n"));
        assert!(text.contains("[aver] line cut at 4096 bytes"));
        assert!(text.ends_with("end\n"));
        assert!(!text.contains(&"x".repeat(4097)));
    }

    /// Defect: processes on one home probed the host's tools at once, each
    /// with answers of its own, so one whose slow probe timed out keyed its
    /// runs under another executor and missed the runs the others cached.
    #[test]
    fn processes_probing_the_host_at_once_agree_on_its_profile() {
        let dir = tempfile::tempdir().unwrap();
        let calls = dir.path().join("calls");
        // Answers with how many times it has been asked: a second probe
        // gives another version.
        executable(
            &dir.path().join("bin/node"),
            &format!(
                "#!/bin/sh\necho x >> {calls}\nn=$(/usr/bin/wc -l < {calls})\n/bin/sleep 0.3\necho $n\n",
                calls = calls.display()
            ),
        );
        let vars = BTreeMap::from([(
            "PATH".to_owned(),
            dir.path().join("bin").display().to_string(),
        )]);
        let versions = dir.path().join("sides/versions.json");
        let profiles: Vec<HostProfile> = std::thread::scope(|s| {
            let probes: Vec<_> = (0..2)
                .map(|_| s.spawn(|| aver::place::host::profile(&vars, &[], &versions)))
                .collect();
            probes.into_iter().map(|p| p.join().unwrap()).collect()
        });
        assert_eq!(profiles[0], profiles[1]);
        assert_eq!(fs::read_to_string(&calls).unwrap().lines().count(), 1);
    }
}

// ---- seed_workspace ----

use aver::disk::BaseFiles;
use aver::place::sides::{seed_workspace, Deps};
use aver::workspace::{FileEntry, FileKind};

/// The tracked files `(path, content)` of a base tree.
fn base_files(files: &[(&str, &str)]) -> BaseFiles {
    BaseFiles::new(files.iter().map(|(p, t)| {
        (
            (*p).to_owned(),
            FileEntry {
                kind: FileKind::File,
                blob: BlobId::of_bytes(t.as_bytes()),
            },
        )
    }))
}

fn deps(keep: &[&str], seed: &[&str], inputs: &[&str]) -> Deps {
    let own = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect();
    Deps {
        keep: own(keep),
        seed: own(seed),
        inputs: own(inputs),
    }
}

/// Defect: a `node_modules` installed from another lockfile than the
/// base's reaches the workspace, whose tests then run against packages the
/// base never named; and a mismatch in one package's lockfile withholds
/// another package's matching dependencies.
#[test]
fn a_kept_directory_is_cloned_only_when_the_lockfile_beside_it_matches_the_base() {
    let user = tempfile::tempdir().unwrap();
    write(user.path(), "package-lock.json", "v1");
    write(user.path(), "node_modules/pkg/index.js", "pkg");
    write(user.path(), "site/package-lock.json", "s1");
    write(user.path(), "site/node_modules/tsc/bin.js", "tsc");
    let layout = deps(
        &["node_modules", "site/node_modules"],
        &[],
        &["package-lock.json", "site/package-lock.json"],
    );
    let fresh = |files: &[(&str, &str)]| {
        let dir = tempfile::tempdir().unwrap();
        for (p, t) in files {
            write(dir.path(), p, t);
        }
        dir
    };

    let same = [
        ("package-lock.json", "v1"),
        ("site/package-lock.json", "s1"),
    ];
    let dir = fresh(&same);
    let cloned = seed_workspace(
        CloneStrategy::Copy,
        user.path(),
        dir.path(),
        &layout,
        &base_files(&same),
    )
    .unwrap();
    assert_eq!(cloned, ["node_modules", "site/node_modules"]);
    assert_eq!(read(dir.path(), "node_modules/pkg/index.js"), "pkg");
    assert_eq!(read(dir.path(), "site/node_modules/tsc/bin.js"), "tsc");

    let root_differs = [
        ("package-lock.json", "v2"),
        ("site/package-lock.json", "s1"),
    ];
    let dir = fresh(&root_differs);
    let cloned = seed_workspace(
        CloneStrategy::Copy,
        user.path(),
        dir.path(),
        &layout,
        &base_files(&root_differs),
    )
    .unwrap();
    assert_eq!(cloned, ["site/node_modules"]);
    assert!(!dir.path().join("node_modules").exists());
}

/// Defect (the round 12 hazard): a seeded `target` holds a unit the user
/// built from their own content, and the workspace's checkout of a file
/// that differs is older than that unit, so the build takes the unit as
/// fresh. A file equal to the user's must keep the user's mtime, or every
/// unit rebuilds and the seed is worthless.
#[test]
fn a_seeded_file_that_differs_from_the_checkout_is_newer_than_every_build_output() {
    let user = tempfile::tempdir().unwrap();
    write(user.path(), "Cargo.lock", "lock");
    write(user.path(), "src/a.rs", "edited by the user");
    write(user.path(), "src/b.rs", "same");
    write(
        user.path(),
        "target/debug/deps/liba.rlib",
        "built from the edit",
    );
    write(user.path(), "target/debug/.fingerprint/a/dep-lib-a", "dep");
    for f in ["Cargo.lock", "src/a.rs", "src/b.rs"] {
        age(user.path(), f, 3 * 3600);
    }
    for f in [
        "target/debug/deps/liba.rlib",
        "target/debug/.fingerprint/a/dep-lib-a",
    ] {
        age(user.path(), f, 3600);
    }
    let files = [
        ("Cargo.lock", "lock"),
        ("src/a.rs", "base"),
        ("src/b.rs", "same"),
    ];
    let dir = tempfile::tempdir().unwrap();
    for (p, t) in files {
        write(dir.path(), p, t);
        age(dir.path(), p, 2 * 3600);
    }

    let cloned = seed_workspace(
        CloneStrategy::Copy,
        user.path(),
        dir.path(),
        &deps(&[], &["target"], &["Cargo.lock"]),
        &base_files(&files),
    )
    .unwrap();
    assert_eq!(cloned, ["target"]);
    let mtime = |d: &Path, p: &str| fs::metadata(d.join(p)).unwrap().modified().unwrap();
    let built = [
        "target/debug/deps/liba.rlib",
        "target/debug/.fingerprint/a/dep-lib-a",
    ]
    .map(|p| mtime(dir.path(), p))
    .into_iter()
    .max()
    .unwrap();
    assert!(mtime(dir.path(), "src/a.rs") > built);
    assert_eq!(
        mtime(dir.path(), "src/b.rs"),
        mtime(user.path(), "src/b.rs")
    );
}

// ---------------------------------------------------------------------
// Commands in process, side by side (wp2-plan.md §5.1, tests 21 and 24),
// natively on `fixtures/sh-calc`: no Node, no Docker.

#[allow(dead_code)]
#[path = "custody/world.rs"]
mod sh_world;

mod in_process {
    use std::fs;
    use std::os::unix::fs::FileTypeExt;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    use super::sh_world::{git, solve, text, World};

    const AVER: &str = env!("CARGO_BIN_EXE_aver");
    const WAITING: &str = "waiting for another aver process";

    /// A world whose check appends a line to `count` (outside every slot)
    /// each time it runs on the base: no `neg` in `calc.sh`, no
    /// `test/neg.tests`.
    fn counting_world() -> (World, PathBuf) {
        let w = World::new();
        let count = w.faults.join("base-runs");
        let script = w.repo.join("test/run.sh");
        let body = fs::read_to_string(&script).unwrap().replacen(
            "out=${1:-unit.xml}\n",
            &format!(
                "out=${{1:-unit.xml}}\nif ! grep -q 'neg()' calc.sh && [ ! -e test/neg.tests ]; then echo run >> {}; fi\n",
                count.display()
            ),
            1,
        );
        fs::write(&script, body).unwrap();
        git(&w.repo, &["commit", "--quiet", "-am", "count base runs"]);
        (w, count)
    }

    /// Defect: verifies on one base serialized behind a home-wide lock, or
    /// each ran the base check itself because neither saw the other's run.
    #[test]
    fn two_verifies_on_one_base_run_side_by_side_and_share_the_base_run() {
        let (w, count) = counting_world();
        let dirs = [w.workspace(), w.workspace()];
        for dir in &dirs {
            solve(dir);
        }
        let verifies: Vec<_> = dirs
            .iter()
            .map(|dir| {
                w.command(Path::new(AVER), dir, &["verify"], None)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap()
            })
            .collect();
        for verify in verifies {
            let out = verify.wait_with_output().unwrap();
            assert!(
                String::from_utf8_lossy(&out.stdout).starts_with("DONE: PASS"),
                "{}",
                text(&out)
            );
            assert!(
                !String::from_utf8_lossy(&out.stderr).contains(WAITING),
                "{}",
                text(&out)
            );
        }
        let runs = fs::read_to_string(&count).unwrap_or_default();
        assert_eq!(runs.lines().count(), 1, "the base check ran: {runs:?}");
    }

    /// A world whose check, on a tree with `neg` (the head's sides),
    /// appends a line to `starts` and, until `awake` exists, its pid to
    /// `pids`, says "sleeping" and sleeps. Returns those three paths.
    fn sleeping_world() -> (World, [PathBuf; 3]) {
        let w = World::new();
        let [starts, pids, awake] = ["head-runs", "pids", "awake"].map(|f| w.faults.join(f));
        let script = w.repo.join("test/run.sh");
        let body = fs::read_to_string(&script).unwrap().replacen(
            "out=${1:-unit.xml}\n",
            &format!(
                "out=${{1:-unit.xml}}\nif grep -q 'neg()' calc.sh; then\n  echo run >> {}\n  if [ ! -e {} ]; then echo $$ >> {}; echo sleeping; sleep 300; fi\nfi\n",
                starts.display(),
                awake.display(),
                pids.display()
            ),
            1,
        );
        fs::write(&script, body).unwrap();
        git(&w.repo, &["commit", "--quiet", "-am", "sleep on the head"]);
        (w, [starts, pids, awake])
    }

    fn lines(path: &Path) -> Vec<String> {
        fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn interrupt(child: &std::process::Child) {
        // SAFETY: signals the child this test spawned.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    }

    /// `child`'s exit status once it exits, within `limit` of now.
    fn exits_within(child: &mut std::process::Child, limit: Duration) -> std::process::ExitStatus {
        let sent = Instant::now();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            if sent.elapsed() > limit {
                let _ = child.kill();
                panic!("still running {limit:?} after SIGINT");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Defect: Ctrl-C killed `aver` and left the check, in its own process
    /// group, running to its deadline; or stored the cut-short run (as
    /// `infra` or a failure) for a later verify to read.
    #[test]
    fn ctrl_c_ends_a_verify_and_its_check_and_stores_no_run() {
        let (w, [starts, pids, awake]) = sleeping_world();
        let dir = w.solved_workspace();
        let mut verify = w
            .command(Path::new(AVER), &dir, &["verify"], None)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        while lines(&pids).is_empty() {
            assert!(verify.try_wait().unwrap().is_none(), "verify ended first");
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "no check slept"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        interrupt(&verify);
        let status = exits_within(&mut verify, Duration::from_secs(3));
        let out = verify.wait_with_output().unwrap();
        assert_eq!(status.code(), Some(130), "{}", text(&out));

        let sleepers = lines(&pids);
        for pid in &sleepers {
            let pid: i32 = pid.parse().unwrap();
            // SAFETY: signal 0 only checks; the check's shell led its group.
            let alive = unsafe { libc::kill(-pid, 0) } == 0;
            assert!(!alive, "the check's group {pid} still runs");
        }

        let store = aver::store::Store::open(w.home.join("aver.db")).unwrap();
        let db = rusqlite::Connection::open(w.home.join("aver.db")).unwrap();
        let ids: Vec<String> = db
            .prepare("SELECT id FROM objects WHERE kind = 'run'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for id in &ids {
            let run: aver_core::CheckRun = store.get(&id.parse().unwrap()).unwrap();
            let output = store.get_raw(&run.output).unwrap();
            assert!(
                !String::from_utf8_lossy(&output).contains("sleeping"),
                "an interrupted run was stored: {run:?}"
            );
        }

        let before = lines(&starts).len();
        fs::write(&awake, "").unwrap();
        let report = w.ok(&dir, &["verify"]);
        assert!(report.starts_with("DONE: PASS"), "{report}");
        assert!(
            lines(&starts).len() > before,
            "the next verify reran the head's check"
        );
        assert_eq!(lines(&pids), sleepers);
    }

    /// Defect: with nothing running, Ctrl-C only asked to stop, and a
    /// command blocked on its input (or anything slow) never ended.
    #[test]
    fn ctrl_c_ends_a_command_with_nothing_running_at_once() {
        let w = World::new();
        // The first start of a new binary can take a while (macOS checks
        // it); the handler is installed only once `main` runs.
        w.ok(&w.repo, &["--version"]);
        let mut hook = w
            .command(Path::new(AVER), &w.repo, &["hook", "worktree-create"], None)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_secs(1));
        assert!(hook.try_wait().unwrap().is_none(), "it waits for its input");
        interrupt(&hook);
        let status = exits_within(&mut hook, Duration::from_millis(500));
        assert_eq!(status.code(), Some(130));
    }

    fn walk(dir: &Path, out: &mut Vec<(PathBuf, fs::FileType)>) {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                walk(&entry.path(), out);
            }
            out.push((entry.path(), kind));
        }
    }

    /// Defect: a command still starting or leaving a daemon behind, or the
    /// removed `aver serve` doing anything but saying so.
    #[test]
    fn no_command_starts_a_daemon_and_serve_is_gone() {
        let w = World::new();
        let dir = w.passing_workspace();
        w.accept(&dir, &[]);

        let ps = std::process::Command::new("ps")
            .args(["-axo", "command="])
            .output()
            .unwrap();
        let serving: Vec<String> = String::from_utf8_lossy(&ps.stdout)
            .lines()
            .filter(|l| l.starts_with(&format!("{AVER} serve")))
            .map(str::to_owned)
            .collect();
        assert!(serving.is_empty(), "{serving:?}");
        let mut files = Vec::new();
        walk(&w.home, &mut files);
        let left: Vec<&PathBuf> = files
            .iter()
            .filter(|(p, kind)| kind.is_socket() || p.file_name().unwrap() == "daemon.json")
            .map(|(p, _)| p)
            .collect();
        assert!(left.is_empty(), "{left:?}");

        for args in [&["serve"][..], &["daemon", "start"], &["daemon", "status"]] {
            let out = w.aver(&w.repo, args, None);
            assert_eq!(out.status.code(), Some(2), "{args:?}: {}", text(&out));
            assert_eq!(
                String::from_utf8_lossy(&out.stderr),
                "aver: removed in 0.4: commands run in process\n",
                "{args:?}"
            );
        }
        let out = w
            .command(Path::new(AVER), &w.repo, &["workspace", "ls"], None)
            .env("AVER_URL", "http://127.0.0.1:7070")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{}", text(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("removed in 0.4"),
            "{}",
            text(&out)
        );
    }
}

// ---- aver ci in place (REDESIGN.md §3.1) ----

mod in_place {
    use super::*;

    const AVER: &str = env!("CARGO_BIN_EXE_aver");
    /// Each run fails unless the kept `node_modules` is there and no
    /// earlier side's `dist/out` is; it prints where it runs and the job
    /// variables it sees, and on a tree whose `calc.sh` says HANG it
    /// touches `$HANG_MARK` and sleeps.
    const HARNESS: &str = r#"[ -f node_modules/marker ] || { echo NO-KEPT; exit 8; }
[ -e dist/out ] && { echo LEAKED; exit 9; }
mkdir -p dist && echo built > dist/out
echo "cwd=$(pwd -P)"
env | grep -E '^(GITHUB_|ACTIONS_|RUNNER_CONTROL)' | sed 's/^/env:/'
if grep -q HANG calc.sh && [ -n "$HANG_MARK" ]; then touch "$HANG_MARK"; sleep 60; fi
[ -n "$ORDER_LOG" ] && echo "code=$(grep -c 'sub()' calc.sh) tests=$(grep -c 'sub works' test/add.tests)" >> "$ORDER_LOG"
"#;
    const SUB: &str = "sub() { echo $(($1 - $2)); }\n";
    const SUB_TEST: &str = "sub works :: [ \"$(sub 3 1)\" = 2 ]\n";

    struct Repo {
        dir: tempfile::TempDir,
        home: tempfile::TempDir,
        base: String,
        head: String,
    }

    impl Repo {
        fn path(&self) -> PathBuf {
            self.dir.path().canonicalize().unwrap()
        }
    }

    /// The sh-calc fixture with [`HARNESS`] and an npm lockfile (so
    /// `node_modules` is kept), `base` applied to the base commit and
    /// `head` to the head, checked out at the head with untracked files of
    /// its own: a kept `node_modules/marker`, `notes.txt` and a stale
    /// `dist/out`.
    fn repo(base: impl Fn(&Path), head: impl Fn(&Path)) -> Repo {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        copy(&common::fixture("sh-calc"), root);
        let run = root.join("test/run.sh");
        let text = fs::read_to_string(&run).unwrap();
        let (first, rest) = text.split_once("cases=\"\"").unwrap();
        fs::write(&run, format!("{first}{HARNESS}cases=\"\"{rest}")).unwrap();
        fs::write(
            root.join("package-lock.json"),
            "{\"lockfileVersion\":3,\"packages\":{}}\n",
        )
        .unwrap();
        git(root, &["init", "-q"]);
        base(root);
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", "base"]);
        let base_sha = git(root, &["rev-parse", "HEAD"]).trim().to_owned();
        head(root);
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", "head"]);
        let head_sha = git(root, &["rev-parse", "HEAD"]).trim().to_owned();
        for (path, text) in [
            ("node_modules/marker", "kept\n"),
            ("notes.txt", "mine\n"),
            ("dist/out", "stale\n"),
        ] {
            fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
            fs::write(root.join(path), text).unwrap();
        }
        Repo {
            dir,
            home: tempfile::tempdir().unwrap(),
            base: base_sha,
            head: head_sha,
        }
    }

    fn copy(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for e in fs::read_dir(from).unwrap().flatten() {
            let target = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &target);
            } else {
                fs::copy(e.path(), &target).unwrap();
            }
        }
    }

    fn add_sub(root: &Path) {
        append(root, "calc.sh", SUB);
        append(root, "test/add.tests", SUB_TEST);
    }

    fn append(root: &Path, path: &str, text: &str) {
        let mut all = fs::read_to_string(root.join(path)).unwrap();
        all.push_str(text);
        fs::write(root.join(path), all).unwrap();
    }

    /// Every file of the checkout outside `.git`, with its bytes and mode.
    fn snapshot(root: &Path) -> BTreeMap<String, (Vec<u8>, u32)> {
        use std::os::unix::fs::PermissionsExt as _;
        fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, (Vec<u8>, u32)>) {
            for e in fs::read_dir(dir).unwrap().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let path = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                if path == ".git" {
                    continue;
                }
                let meta = fs::symlink_metadata(e.path()).unwrap();
                if meta.is_dir() {
                    walk(&e.path(), &path, out);
                } else {
                    out.insert(
                        path,
                        (fs::read(e.path()).unwrap(), meta.permissions().mode()),
                    );
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, "", &mut out);
        out
    }

    fn ci(repo: &Repo, out: &Path, extra: &[&str]) -> Command {
        let mut cmd = Command::new(AVER);
        cmd.env_remove("AVER_SCRIPTED")
            .env("AVER_HOME", repo.home.path())
            .arg("ci")
            .arg("--repo")
            .arg(repo.path())
            .args(["--base", &repo.base, "--head", &repo.head, "--out-dir"])
            .arg(out)
            .args(extra);
        cmd
    }

    fn logs(out: &Path) -> String {
        let mut all = String::new();
        for e in fs::read_dir(out.join("runs")).unwrap().flatten() {
            all.push_str(&fs::read_to_string(e.path()).unwrap());
        }
        all
    }

    fn verdict(out: &Path) -> String {
        let v: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("receipt.json")).unwrap()).unwrap();
        v["receipt"]["verdict"].as_str().unwrap().to_owned()
    }

    fn assert_untouched(repo: &Repo, before: &BTreeMap<String, (Vec<u8>, u32)>) {
        assert_eq!(&snapshot(&repo.path()), before, "the checkout as it was");
        assert!(!repo.path().join(".git/aver-swap.json").exists());
        assert!(!repo.path().join(".git/aver-stash").exists());
        assert_eq!(
            git(&repo.path(), &["status", "--porcelain"]),
            "?? dist/\n?? node_modules/\n?? notes.txt\n"
        );
    }

    /// Defect: in place, a check runs somewhere else, reinstalls what the
    /// job installed, sees another side's build output, or leaves the
    /// checkout at another tree or without the job's untracked files.
    #[test]
    fn ci_runs_in_the_checkout_and_leaves_it_as_it_found_it() {
        let marks = tempfile::tempdir().unwrap();
        let mark = marks.path().join("hanging");
        for (head, verdict_wanted, code) in [
            (add_sub as fn(&Path), "PASS", 0),
            (
                |root: &Path| {
                    let calc = fs::read_to_string(root.join("calc.sh")).unwrap();
                    fs::write(root.join("calc.sh"), calc.replace("$1 + $2", "$1 - $2")).unwrap();
                },
                "FAIL_OWN",
                1,
            ),
            // The head's runs time out, which a green base blames on the head.
            (
                |root: &Path| append(root, "calc.sh", "# HANG\n"),
                "FAIL_OWN",
                1,
            ),
        ] {
            let repo = repo(
                |root| {
                    let toml = fs::read_to_string(root.join("aver.toml")).unwrap();
                    fs::write(
                        root.join("aver.toml"),
                        toml.replace("timeout_s = 60", "timeout_s = 3"),
                    )
                    .unwrap();
                },
                head,
            );
            let before = snapshot(&repo.path());
            let out = tempfile::tempdir().unwrap();
            let run = ci(&repo, out.path(), &[])
                .env("HANG_MARK", &mark)
                .output()
                .unwrap();
            let err = String::from_utf8_lossy(&run.stderr);
            assert_eq!(run.status.code(), Some(code), "{err}");
            assert_eq!(verdict(out.path()), verdict_wanted, "{err}");
            assert!(!err.contains("slots"), "{err}");
            let logs = logs(out.path());
            assert!(
                !logs.contains("NO-KEPT") && !logs.contains("LEAKED"),
                "{logs}"
            );
            let cwd = format!("cwd={}", repo.path().display());
            assert_eq!(
                logs.matches("cwd=").count(),
                logs.matches(&cwd).count(),
                "{logs}"
            );
            assert!(logs.contains(&cwd), "{logs}");
            assert_untouched(&repo, &before);
        }
    }

    /// Defect: in place, the sides run in the order they asked (base,
    /// judged head, head tests, transplant), so the checkout swaps from
    /// head code back to base code and a build tool rebuilds twice.
    #[test]
    fn in_place_the_sides_run_grouped_by_code() {
        let repo = repo(|_| {}, add_sub);
        let out = tempfile::tempdir().unwrap();
        let marks = tempfile::tempdir().unwrap();
        let order = marks.path().join("order");
        let run = ci(&repo, out.path(), &[])
            .env("ORDER_LOG", &order)
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&run.stderr);
        assert_eq!(run.status.code(), Some(0), "{err}");
        let cwd = format!("cwd={}", repo.path().display());
        assert!(logs(out.path()).contains(&cwd), "ran in place");
        assert_eq!(
            fs::read_to_string(&order).unwrap(),
            "code=0 tests=0\ncode=0 tests=1\ncode=1 tests=0\ncode=1 tests=1\n",
            "base, transplant, judged head, head with its tests"
        );
    }

    /// Defect: a checkout that is not the head, or `--slots`, is swapped
    /// anyway, so a local edit is lost.
    #[test]
    fn slots_and_a_checkout_with_edits_run_elsewhere() {
        let repo = repo(|_| {}, add_sub);
        let out = tempfile::tempdir().unwrap();
        let run = ci(&repo, out.path(), &["--slots"]).output().unwrap();
        assert_eq!(run.status.code(), Some(0), "{run:?}");
        let cwd = format!("cwd={}", repo.path().display());
        assert!(!logs(out.path()).contains(&cwd));

        append(&repo.path(), "calc.sh", "# local edit\n");
        let before = snapshot(&repo.path());
        let run = ci(&repo, out.path(), &[]).output().unwrap();
        let err = String::from_utf8_lossy(&run.stderr);
        assert!(
            err.contains("running the checks in slots cloned from the checkout: the checkout has changes to tracked files"),
            "{err}"
        );
        assert!(!logs(out.path()).contains(&cwd));
        assert_eq!(snapshot(&repo.path()), before);
    }

    /// Waits for `mark`, checks the run hangs in the checkout at the base,
    /// then sends `signal` to `child`.
    fn signal_at(child: &std::process::Child, repo: &Repo, mark: &Path, signal: i32) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !mark.exists() {
            assert!(Instant::now() < deadline, "the run never reached the mark");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            read(&repo.path(), "calc.sh").contains("HANG"),
            "at the base"
        );
        // SAFETY: the child is ours and not yet reaped.
        unsafe { libc::kill(child.id() as i32, signal) };
    }

    /// The base hangs (its `calc.sh` says HANG) so the checkout is at the
    /// base when the signal comes.
    fn hanging(mark: &Path) -> (Repo, tempfile::TempDir, std::process::Child) {
        let repo = repo(
            |root| append(root, "calc.sh", "# HANG\n"),
            |root| {
                let calc = fs::read_to_string(root.join("calc.sh")).unwrap();
                fs::write(root.join("calc.sh"), calc.replace("# HANG\n", "")).unwrap();
                add_sub(root);
            },
        );
        let out = tempfile::tempdir().unwrap();
        let child = ci(&repo, out.path(), &[])
            .env("HANG_MARK", mark)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        (repo, out, child)
    }

    /// Defect: Ctrl-C or the runner's SIGTERM mid-run leaves the job's
    /// checkout at the base for the workflow's later steps.
    #[test]
    fn a_signal_mid_run_restores_the_checkout() {
        let marks = tempfile::tempdir().unwrap();
        let mark = marks.path().join("hanging");
        let (repo, _out, mut child) = hanging(&mark);
        let before_run = git(&repo.path(), &["rev-parse", "HEAD"]);
        signal_at(&child, &repo, &mark, libc::SIGTERM);
        let status = child.wait().unwrap();
        assert_eq!(status.code(), Some(130));
        assert_eq!(git(&repo.path(), &["rev-parse", "HEAD"]), before_run);
        assert_eq!(read(&repo.path(), "notes.txt"), "mine\n");
        assert_eq!(read(&repo.path(), "dist/out"), "stale\n");
        assert!(!read(&repo.path(), "calc.sh").contains("HANG"));
        assert_untouched_files(&repo);
    }

    fn assert_untouched_files(repo: &Repo) {
        assert!(!repo.path().join(".git/aver-swap.json").exists());
        assert_eq!(
            git(&repo.path(), &["status", "--porcelain"]),
            "?? dist/\n?? node_modules/\n?? notes.txt\n"
        );
    }

    /// Defect: an `aver ci` killed outright (the job cancelled) leaves the
    /// checkout at another tree, and nothing puts it back.
    #[test]
    fn aver_ci_restore_puts_back_what_a_killed_run_left() {
        let marks = tempfile::tempdir().unwrap();
        let mark = marks.path().join("hanging");
        let (repo, _out, mut child) = hanging(&mark);
        signal_at(&child, &repo, &mark, libc::SIGKILL);
        child.wait().unwrap();
        // The sleeping check outlives its killed parent; it holds nothing
        // the restore needs.
        assert!(
            read(&repo.path(), "calc.sh").contains("HANG"),
            "at the base"
        );
        assert!(repo.path().join(".git/aver-swap.json").exists());
        assert!(!repo.path().join("notes.txt").exists(), "moved aside");

        let restore = || {
            Command::new(AVER)
                .env_remove("AVER_SCRIPTED")
                .env("AVER_HOME", repo.home.path())
                .args(["ci", "restore", "--repo"])
                .arg(repo.path())
                .output()
                .unwrap()
        };
        let first = restore();
        assert_eq!(first.status.code(), Some(0), "{first:?}");
        assert!(!read(&repo.path(), "calc.sh").contains("HANG"));
        assert_eq!(read(&repo.path(), "notes.txt"), "mine\n");
        assert_eq!(read(&repo.path(), "dist/out"), "stale\n");
        assert_eq!(read(&repo.path(), "node_modules/marker"), "kept\n");
        assert_untouched_files(&repo);
        let again = restore();
        assert_eq!(again.status.code(), Some(0));
        assert!(
            String::from_utf8_lossy(&again.stderr).contains("nothing to restore"),
            "{again:?}"
        );
    }

    /// Defect: the next `aver ci` on a checkout a killed run left at the
    /// base adopts the base as if it were the job's commit.
    #[test]
    fn the_next_aver_ci_restores_what_a_killed_run_left_first() {
        let marks = tempfile::tempdir().unwrap();
        let mark = marks.path().join("hanging");
        let (repo, out, mut child) = hanging(&mark);
        signal_at(&child, &repo, &mark, libc::SIGKILL);
        child.wait().unwrap();
        let run = ci(&repo, out.path(), &[]).output().unwrap();
        let err = String::from_utf8_lossy(&run.stderr);
        assert_eq!(run.status.code(), Some(0), "{err}");
        assert!(err.contains("restored"), "{err}");
        assert!(!read(&repo.path(), "calc.sh").contains("HANG"));
        assert_eq!(read(&repo.path(), "notes.txt"), "mine\n");
        assert_untouched_files(&repo);
    }

    /// Defect: `--regression-only` (a merge queue) holds a reviewed change
    /// with the default's exit 3, or takes criteria it then ignores.
    #[test]
    fn regression_only_passes_a_change_for_review() {
        let repo = repo(
            |_| {},
            |root| {
                fs::create_dir_all(root.join(".github/workflows")).unwrap();
                fs::write(root.join(".github/workflows/ci.yml"), "on: push\n").unwrap();
            },
        );
        let out = tempfile::tempdir().unwrap();
        let default = ci(&repo, out.path(), &[]).output().unwrap();
        assert_eq!(default.status.code(), Some(3), "{default:?}");
        assert_eq!(verdict(out.path()), "REVIEW");
        let queue = ci(&repo, out.path(), &["--regression-only"])
            .output()
            .unwrap();
        assert_eq!(queue.status.code(), Some(0), "{queue:?}");
        let both = ci(
            &repo,
            out.path(),
            &["--regression-only", "--criteria", "c.md"],
        )
        .output()
        .unwrap();
        assert_eq!(both.status.code(), Some(2), "{both:?}");
    }

    /// Defect: in place, a base run after head code reads the build cache
    /// (`target`, a Cargo layer's seed) the head wrote, so the head could
    /// make its own failure the base's. The head is flaky (its first run
    /// fails), which reruns the base after it.
    #[test]
    fn a_base_run_after_head_code_starts_without_the_heads_build_cache() {
        let probe = "[ -f target/marker ] && echo POISONED\n\
            if grep -q HEADCODE calc.sh; then\n\
              mkdir -p target && touch target/marker\n\
              if [ -f \"$FLAKY\" ]; then FIRST=0; else touch \"$FLAKY\"; FIRST=1; fi\n\
              export FIRST\n\
            fi\n";
        let repo = repo(
            |root| {
                fs::write(root.join("Cargo.lock"), "version = 3\n").unwrap();
                append(root, "aver.toml", "\n[[deps]]\nlockfile = \"Cargo.lock\"\n");
                let run = fs::read_to_string(root.join("test/run.sh")).unwrap();
                let run = run.replacen("cases=\"\"", &format!("{probe}cases=\"\""), 1);
                fs::write(root.join("test/run.sh"), run).unwrap();
            },
            |root| {
                append(
                    root,
                    "calc.sh",
                    "# HEADCODE\nif [ \"$FIRST\" = 1 ]; then add() { echo 0; }; fi\n",
                );
            },
        );
        let marks = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let run = ci(&repo, out.path(), &["--fresh"])
            .env("FLAKY", marks.path().join("flaky"))
            .output()
            .unwrap();
        assert!(run.status.code().is_some_and(|c| c < 2), "{run:?}");
        let mut base_runs = Vec::new();
        for log in fs::read_dir(out.path().join("runs")).unwrap().flatten() {
            let name = log.file_name().to_string_lossy().into_owned();
            if name.starts_with("unit.base.") {
                base_runs.push((name, fs::read_to_string(log.path()).unwrap()));
            }
        }
        assert!(
            base_runs.len() > 1,
            "the base reran after the head: {base_runs:?}"
        );
        for (name, text) in base_runs {
            assert!(!text.contains("POISONED"), "{name}: {text}");
        }
    }

    /// Defect (rule 10): a check in the job's own checkout writes the job's
    /// outputs or reads its tokens.
    #[test]
    fn checks_in_place_never_see_the_jobs_step_files_or_tokens() {
        let repo = repo(|_| {}, add_sub);
        let out = tempfile::tempdir().unwrap();
        let mut cmd = ci(&repo, out.path(), &[]);
        for name in [
            "GITHUB_OUTPUT",
            "GITHUB_ENV",
            "GITHUB_PATH",
            "GITHUB_STEP_SUMMARY",
            "GITHUB_STATE",
            "GITHUB_TOKEN",
            "ACTIONS_RUNTIME_TOKEN",
            "ACTIONS_RUNTIME_URL",
            "ACTIONS_RESULTS_URL",
            "ACTIONS_CACHE_URL",
            "ACTIONS_ID_TOKEN_REQUEST_URL",
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
        ] {
            cmd.env(name, "secret");
        }
        let run = cmd.env("RUNNER_CONTROL", "seen").output().unwrap();
        assert_eq!(run.status.code(), Some(0), "{run:?}");
        let logs = logs(out.path());
        assert!(logs.contains("env:RUNNER_CONTROL=seen"), "{logs}");
        assert!(logs.contains(&format!("cwd={}", repo.path().display())));
        let leaked: Vec<&str> = logs
            .lines()
            .filter(|l| l.starts_with("env:GITHUB_") || l.starts_with("env:ACTIONS_"))
            .collect();
        assert!(leaked.is_empty(), "{leaked:?}");
    }
}
