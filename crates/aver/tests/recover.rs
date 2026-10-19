//! Custody K2 without fault injection: a verify or the whole process
//! killed while a check runs, on `fixtures/sh-calc` (shell checks: no Node, no Docker). The cuts at
//! named crash points are in `tests/custody/k2.rs`.

#[allow(dead_code)]
#[path = "custody/procs.rs"]
mod procs;
#[allow(dead_code)]
#[path = "custody/world.rs"]
mod world;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use procs::{ended_within, group_alive, pgid_of, pid_files, plant_leftover};
use world::{text, World};

const AVER: &str = env!("CARGO_BIN_EXE_aver");

/// Adds a test to the workspace at `dir` that runs while `marker` exists
/// (at most a minute with `sleep`, else until the marker goes).
fn slow_test(dir: &Path, marker: &Path, until_removed: bool) {
    fs::write(marker, "").unwrap();
    let body = if until_removed {
        format!("while [ -e {} ]; do sleep 0.1; done", marker.display())
    } else {
        format!("if [ -e {} ]; then sleep 60; fi", marker.display())
    };
    fs::write(dir.join("test/slow.tests"), format!("slow :: {body}\n")).unwrap();
}

/// The process group of a check run that is still going a second after it
/// was recorded: the slow one.
fn slow_group(w: &World, process: &mut Child) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        assert!(Instant::now() < deadline, "no slow run started");
        if let Some(status) = process.try_wait().unwrap() {
            panic!("the verify ended first: {status}");
        }
        let live: Vec<i32> = pid_files(w)
            .iter()
            .filter_map(|f| pgid_of(f))
            .filter(|g| group_alive(*g))
            .collect();
        if !live.is_empty() {
            std::thread::sleep(Duration::from_secs(1));
            if let Some(g) = live.into_iter().find(|g| group_alive(*g)) {
                return g;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn passes(out: &std::process::Output) {
    assert!(
        String::from_utf8_lossy(&out.stdout).starts_with("DONE: PASS"),
        "{}",
        text(out)
    );
}

/// A run of a killed verify keeps running in its process group (and would
/// write its report when it ends); the next verify kills it before its own
/// runs start, so nothing it writes reaches a signed run.
#[test]
fn a_leftover_run_of_a_killed_verify_is_killed_before_the_next_crit_k2_stale_reporter() {
    let w = World::new();
    let dir = w.solved_workspace();
    let marker = w.faults.join("slow");
    slow_test(&dir, &marker, false);
    let mut verify = w
        .command(Path::new(AVER), &dir, &["verify"], None)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stale = slow_group(&w, &mut verify);
    // SIGKILL: the run's group lives on.
    verify.kill().unwrap();
    verify.wait().unwrap();
    assert!(group_alive(stale), "the run outlives the verify");

    fs::remove_file(&marker).unwrap();
    let started = Instant::now();
    passes(&w.aver(&dir, &["verify"], None));
    assert!(!group_alive(stale), "the leftover run was killed");
    assert!(started.elapsed() < Duration::from_secs(50));
}

/// A process that held the home stopped uncleanly: the next command that
/// opens the home drops the stat caches and kills check runs it left;
/// status, ls and doctor answer and say so; verify passes.
#[test]
fn the_next_open_after_an_unclean_stop_recovers_crit_k2_unclean_recover() {
    let w = World::new();
    let dir = w.solved_workspace();
    let stats: Vec<PathBuf> = fs::read_dir(w.home.join("mat"))
        .unwrap()
        .flatten()
        .map(|e| e.path().join("stat"))
        .collect();
    assert!(stats.iter().any(|p| p.exists()), "workspace new saved one");
    // Planted before the dead owner's record: the warm-up `workspace new`
    // started may open the home first and do the recovery itself.
    let (mut leftover, pgid) = plant_leftover(&w, "B-unit");
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    let dead = gone.id();
    gone.wait().unwrap();
    let running = w.home.join("running");
    fs::create_dir_all(&running).unwrap();
    fs::write(
        running.join(format!("{dead}-1")),
        format!("{{\"pid\": {dead}, \"start\": 1}}"),
    )
    .unwrap();

    let out = w.aver(&dir, &["diff"], None);
    assert!(out.status.success(), "{}", text(&out));
    assert!(ended_within(&mut leftover, 10), "the leftover was killed");
    assert!(!group_alive(pgid));
    assert!(
        !running.join(format!("{dead}-1")).exists(),
        "the dead owner's record is gone"
    );

    let mut said = String::new();
    for args in [
        &["status"][..],
        &["workspace", "ls"],
        &["doctor", "--verbose"],
    ] {
        let out = w.aver(&w.repo, args, None);
        assert_ne!(out.status.code(), Some(101), "{}", text(&out));
        assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
        said.push_str(&text(&out));
    }
    assert!(said.contains("stopped uncleanly"), "{said}");
    assert!(said.contains("1 leftover check run(s) killed"), "{said}");
    passes(&w.aver(&dir, &["verify"], None));
}
