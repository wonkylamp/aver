//! Workspace lifecycle (custody K4): one test per acceptance criterion,
//! each driving the `aver` binary against a fresh world.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::world::{solve, text, World, CRITERION};

/// [`World::command`] for `aver args`, its output piped.
fn piped(w: &World, cwd: &Path, args: &[&str], fault: Option<&str>) -> Command {
    let mut cmd = w.command(Path::new(env!("CARGO_BIN_EXE_aver")), cwd, args, fault);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd
}

fn work_dirs(w: &World) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for repo in fs::read_dir(w.home.join("work"))
        .into_iter()
        .flatten()
        .flatten()
    {
        for dir in fs::read_dir(repo.path()).into_iter().flatten().flatten() {
            out.push(dir.path());
        }
    }
    out
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Files under `dir`, outside `.aver/`, whose name marks a temporary file.
fn temp_files(root: &Path, dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path == root.join(".aver") {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().unwrap().is_dir() {
            out.extend(temp_files(root, &path));
        } else if name.contains("restore") || aver::atomic::is_tmp(&name) {
            out.push(path);
        }
    }
    out
}

#[test]
fn crit_k4_gc_race_gc_never_takes_a_directory_being_made() {
    let w = World::new();
    let maker = piped(
        &w,
        &w.repo,
        &["workspace", "new", "-c", CRITERION],
        Some("materialize.cloned#1=pause"),
    )
    .spawn()
    .unwrap();
    w.paused("materialize.cloned");
    let dirs = work_dirs(&w);
    assert_eq!(dirs.len(), 1, "the clone is there, without a row: {dirs:?}");
    let mut gc = piped(&w, &w.repo, &["gc"], None).spawn().unwrap();
    // Without the work lock gc trashes the orphan-looking directory at once.
    std::thread::sleep(Duration::from_millis(1500));
    assert!(dirs[0].is_dir(), "gc took the directory being made");
    assert!(
        gc.try_wait().unwrap().is_none(),
        "gc waits for the materialize"
    );
    w.resume("materialize.cloned");
    let made = maker.wait_with_output().unwrap();
    assert!(made.status.success(), "{}", text(&made));
    let printed = PathBuf::from(String::from_utf8_lossy(&made.stdout).trim());
    assert_eq!(printed, dirs[0]);
    let swept = gc.wait_with_output().unwrap();
    assert!(swept.status.success(), "{}", text(&swept));
    assert!(printed.join(".aver/workspace.toml").is_file());
    let listed = w.ok(&w.repo, &["workspace", "ls"]);
    assert!(
        listed
            .lines()
            .any(|l| l.contains("live") && l.ends_with(&*printed.to_string_lossy())),
        "{listed}"
    );
}

#[test]
fn crit_k4_rm_guard_refuses_unsaved_changes_without_force() {
    let w = World::new();
    let dir = w.workspace();
    solve(&dir);
    let target = dir.display().to_string();
    let out = w.aver(&w.repo, &["workspace", "rm", &target], None);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    let err = stderr(&out);
    assert!(err.contains("calc.sh") && err.contains("+1 -0"), "{err}");
    assert!(
        err.contains("test/neg.tests") && err.contains("(new)"),
        "{err}"
    );
    assert!(err.contains("2 files changed, 2 insertions(+)"), "{err}");
    assert!(err.contains("--force"), "{err}");
    assert!(dir.join("test/neg.tests").is_file(), "nothing was removed");

    // Accepted, then edited: the edit is the change no commit holds.
    let report = w.ok(&dir, &["verify"]);
    assert!(report.starts_with("DONE: PASS"), "{report}");
    w.accept(&dir, &["--keep"]);
    fs::write(dir.join("notes.txt"), "after the accept\n").unwrap();
    let out = w.aver(&w.repo, &["workspace", "rm", &target], None);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(stderr(&out).contains("notes.txt"), "{}", text(&out));
    assert!(dir.is_dir());

    w.ok(&w.repo, &["workspace", "rm", "--force", &target]);
    assert!(!dir.exists());

    // An unchanged directory goes without --force.
    let clean = w.workspace();
    w.ok(&w.repo, &["workspace", "rm", &clean.display().to_string()]);
    assert!(!clean.exists());
}

#[test]
fn crit_k4_gc_accepted_keeps_unexported_and_changed_work() {
    let w = World::new();
    // Committed, then cut before the export: no branch holds the commit.
    // Its own change, since the same snapshot exported for another
    // workspace would be on a branch.
    let unexported = w.workspace();
    solve(&unexported);
    fs::write(unexported.join("README.unexported"), "only here\n").unwrap();
    let report = w.ok(&unexported, &["verify"]);
    assert!(report.starts_with("DONE: PASS"), "{report}");
    let out = w.aver(&unexported, &["accept"], Some("accept.committed=abort"));
    assert_eq!(out.status.code(), Some(137), "{}", text(&out));
    // Accepted and kept, then changed.
    let changed = w.passing_workspace();
    w.accept(&changed, &["--keep"]);
    fs::write(changed.join("notes.txt"), "after the accept\n").unwrap();
    // Accepted and kept, unchanged: the one gc removes.
    let done = w.passing_workspace();
    w.accept(&done, &["--keep"]);

    let report = w.ok(&w.repo, &["gc", "--accepted"]);
    assert!(unexported.is_dir(), "{report}");
    assert!(changed.join("notes.txt").is_file(), "{report}");
    assert!(!done.exists(), "{report}");
    assert!(report.contains("never exported"), "{report}");
    assert!(
        report.contains("changed since the commit (notes.txt)"),
        "{report}"
    );

    // The accept-time release keeps a directory edited between the commit
    // and the release.
    let late = w.passing_workspace();
    let child = Command::new(env!("CARGO_BIN_EXE_aver"))
        .arg("accept")
        .current_dir(&late)
        .env("AVER_HOME", &w.home)
        .env("AVER_FAULT_DIR", &w.faults)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("AVER_FAULT", "accept.decided=pause")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    w.paused("accept.decided");
    fs::write(late.join("calc.sh"), "echo edited late\n").unwrap();
    w.resume("accept.decided");
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), stderr(&out));
    assert!(said.contains("calc.sh changed after the commit"), "{said}");
    assert_eq!(
        fs::read_to_string(late.join("calc.sh")).unwrap(),
        "echo edited late\n"
    );
}

#[test]
fn crit_k4_restore_safe_a_killed_restore_leaves_nothing_and_undo_puts_back() {
    let w = World::new();
    let (file, edited) = w.edited_workspace();
    let dir = file.parent().unwrap().to_owned();
    solve(&dir);
    let solved = fs::read(&file).unwrap();
    assert_ne!(solved, edited);
    let base = fs::read(w.repo.join("calc.sh")).unwrap();

    // Killed with the first file staged, then after the first file.
    for fault in ["restore.tmp=abort", "restore.partial#1=abort"] {
        let out = w.aver(&dir, &["restore", "calc.sh", "test/neg.tests"], Some(fault));
        assert_eq!(out.status.code(), Some(137), "{fault}: {}", text(&out));
        assert!(
            temp_files(&dir, &dir).is_empty(),
            "{fault}: {:?}",
            temp_files(&dir, &dir)
        );
        let synced = w.ok(&dir, &["workspace", "sync", "--json"]);
        let report: serde_json::Value = serde_json::from_str(&synced).unwrap();
        let changed: Vec<&str> = report["changed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap())
            .collect();
        assert!(
            changed
                .iter()
                .all(|p| ["calc.sh", "test/neg.tests"].contains(p)),
            "{fault}: the sync recorded {changed:?}"
        );
    }
    // The cut restore wrote calc.sh; undo puts the solved version back.
    assert_eq!(fs::read(&file).unwrap(), base);
    w.ok(&dir, &["restore", "--undo"]);
    assert_eq!(fs::read(&file).unwrap(), solved);
    assert!(dir.join("test/neg.tests").is_file());

    // A whole restore and its undo.
    let said = w.ok(&dir, &["restore", "calc.sh", "test/neg.tests"]);
    assert!(said.contains("restore --undo"), "{said}");
    assert_eq!(fs::read(&file).unwrap(), base);
    assert!(!dir.join("test/neg.tests").exists());
    let said = w.ok(&dir, &["restore", "--undo"]);
    assert!(said.contains("put back calc.sh"), "{said}");
    assert_eq!(fs::read(&file).unwrap(), solved);
    assert!(dir.join("test/neg.tests").is_file());
    // The first, killed restore is next; nothing before it.
    w.ok(&dir, &["restore", "--undo"]);
    assert_eq!(fs::read(&file).unwrap(), solved);
    let out = w.aver(&dir, &["restore", "--undo"], None);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(stderr(&out).contains("nothing to undo"), "{}", text(&out));
}

#[test]
fn crit_k4_meta_atomic_a_killed_write_leaves_a_parseable_workspace_file() {
    let w = World::new();
    // Cut writing the first directory's workspace file.
    let out = w.aver(
        &w.repo,
        &["workspace", "new", "-c", CRITERION],
        Some("meta.write#1=abort"),
    );
    assert_eq!(out.status.code(), Some(137), "{}", text(&out));
    for dir in work_dirs(&w) {
        match fs::read_to_string(dir.join(".aver/workspace.toml")) {
            Ok(t) => assert!(t.parse::<toml::Table>().is_ok(), "{t:?}"),
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
        }
    }
    // A live directory's files survive a cut rewrite of one of them, and
    // the cut write's temporary file goes with the next write.
    let dir = w.workspace();
    let toml_before = fs::read(dir.join(".aver/workspace.toml")).unwrap();
    let out = w.aver(
        &dir,
        &["giveup", "the fixture is broken"],
        Some("meta.write=abort"),
    );
    assert_eq!(out.status.code(), Some(137), "{}", text(&out));
    assert_eq!(
        fs::read(dir.join(".aver/workspace.toml")).unwrap(),
        toml_before
    );
    assert!(!aver::atomic::leftovers(&dir.join(".aver")).is_empty());
    w.ok(&dir, &["giveup", "the fixture is broken"]);
    assert!(aver::atomic::leftovers(&dir.join(".aver")).is_empty());
    let out = w.aver(&dir, &["workspace", "sync"], None);
    assert!(out.status.success(), "{}", text(&out));
}
