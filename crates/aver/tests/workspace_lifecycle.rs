//! Workspace lifecycle (custody K4) through the `aver` binary, without the
//! fault harness: what a plain `cargo test` can show of each criterion.
//! `tests/custody/lifecycle.rs` cuts the same operations at their crash
//! points.

#[allow(dead_code)]
#[path = "custody/world.rs"]
mod world;

use std::fs;
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::MetadataExt as _;
use std::time::Duration;

use world::{solve, text, World};

#[test]
fn gc_waits_for_a_materialize_holding_the_work_lock_crit_k4_gc_race() {
    let w = World::new();
    w.workspace();
    // A directory without a row, as a materialize between its clone and
    // its row leaves one, while that materialize holds the lock shared.
    let making = w.home.join("work/repo/ws-being-made");
    fs::create_dir_all(&making).unwrap();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(w.home.join("work.lock"))
        .unwrap();
    // SAFETY: the descriptor is open.
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_SH) }, 0);
    let mut gc = std::process::Command::new(env!("CARGO_BIN_EXE_aver"))
        .arg("gc")
        .current_dir(&w.repo)
        .env("AVER_HOME", &w.home)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(making.is_dir(), "gc took a directory being made");
    assert!(gc.try_wait().unwrap().is_none(), "gc waits for the lock");
    drop(lock);
    let out = gc.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert!(!making.exists(), "an orphan goes once nothing makes it");
}

#[test]
fn rm_refuses_unaccepted_changes_with_a_diff_stat_crit_k4_rm_guard() {
    let w = World::new();
    let dir = w.workspace();
    solve(&dir);
    let target = dir.display().to_string();
    let out = w.aver(&w.repo, &["workspace", "rm", &target], None);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("calc.sh") && err.contains("+1 -0"), "{err}");
    assert!(err.contains("2 files changed, 2 insertions(+)"), "{err}");
    assert!(err.contains("--force"), "{err}");
    assert!(dir.join("test/neg.tests").is_file());
    w.ok(&w.repo, &["workspace", "rm", "--force", &target]);
    assert!(!dir.exists());
}

#[test]
fn gc_accepted_keeps_a_directory_changed_since_its_commit_crit_k4_gc_accepted() {
    let w = World::new();
    let changed = w.passing_workspace();
    w.accept(&changed, &["--keep"]);
    fs::write(changed.join("notes.txt"), "after the accept\n").unwrap();
    let done = w.passing_workspace();
    w.accept(&done, &["--keep"]);
    let report = w.ok(&w.repo, &["gc", "--accepted"]);
    assert!(changed.join("notes.txt").is_file(), "{report}");
    assert!(!done.exists(), "{report}");
    assert!(
        report.contains("changed since the commit (notes.txt)"),
        "{report}"
    );
}

#[test]
fn restore_undo_puts_back_an_unsynced_edit_crit_k4_restore_safe() {
    let w = World::new();
    let (file, edited) = w.edited_workspace();
    let dir = file.parent().unwrap().to_owned();
    let base = fs::read(w.repo.join("calc.sh")).unwrap();
    w.ok(&dir, &["restore", "calc.sh"]);
    assert_eq!(fs::read(&file).unwrap(), base);
    // Staged under .aver/, never beside the file, so no sync records one.
    let stray: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains("restore"))
        .collect();
    assert!(stray.is_empty(), "{stray:?}");
    let out = w.aver(&dir, &["restore", "--undo"], None);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(fs::read(&file).unwrap(), edited);
}

#[test]
fn a_workspace_file_is_replaced_never_truncated_crit_k4_meta_atomic() {
    let w = World::new();
    let dir = w.workspace();
    let meta = dir.join(".aver/workspace.toml");
    let before = fs::read(&meta).unwrap();
    // A reader that opened the file before a rewrite keeps reading the
    // whole old file: the rewrite is a new file renamed over it, so a kill
    // mid-write cannot leave it empty.
    let reader = fs::File::open(&meta).unwrap();
    aver::workspace::WorkspaceFile::write_meta(&dir, ".aver/workspace.toml", &before[..10])
        .unwrap();
    let held = reader.metadata().unwrap();
    assert_eq!(
        held.len(),
        before.len() as u64,
        "the old file was truncated"
    );
    assert_ne!(held.ino(), fs::metadata(&meta).unwrap().ino());
    assert_eq!(fs::read(&meta).unwrap(), &before[..10]);
}
