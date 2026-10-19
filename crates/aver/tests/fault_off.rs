//! A build without the `faults` feature ignores `AVER_FAULT`: the crash
//! points of `aver::fault` compile to nothing (custody plan §1.2).

#![cfg(not(feature = "faults"))]

use std::process::Command;

#[test]
fn without_the_feature_aver_fault_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let out = Command::new("git")
        .args(["init", "--quiet", "--initial-branch=main"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(out.status.success());
    for point in aver::fault::POINTS {
        assert!(aver::fault::point(point).is_ok());
    }
    let out = Command::new(env!("CARGO_BIN_EXE_aver"))
        .args(["integrate", "codex", "--yes"])
        .current_dir(&repo)
        .env("AVER_HOME", dir.path().join("home"))
        .env(
            "AVER_FAULT",
            "integrate.recorded=abort,integrate.file=enospc",
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(repo.join("AGENTS.md").exists());
}
