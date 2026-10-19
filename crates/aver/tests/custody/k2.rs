//! Custody K2: the runner after a kill or a full disk (custody plan, K2), cut at K2's crash points. Each test ends
//! with the criterion it proves; `tests/recover.rs` has the ones that need
//! no fault.

use std::fs;
use std::path::PathBuf;

use super::procs::{ended_within, group_alive, plant_leftover};
use super::world::{text, World};

/// A full disk while a check runs is the host's problem: the entry is
/// `ENV`, the verdict never `FAIL_OWN`, and the rerun passes.
#[test]
fn enospc_while_a_check_runs_gives_env_never_fail_own_crit_k2_disk_full() {
    let w = World::new();
    let dir = w.solved_workspace();
    let out = w.aver(&dir, &["verify", "--json"], Some("proc.spawned=enospc"));
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let body = &json["receipt"];
    assert_ne!(body["verdict"], "FAIL_OWN", "{body:#}");
    let causes: Vec<&str> = body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["cause"].as_str())
        .collect();
    assert!(causes.contains(&"ENV"), "{causes:?}");
    assert!(!causes.contains(&"NEW"), "{causes:?}");
    assert!(stdout.contains("the disk is full"), "{stdout}");

    let out = w.aver(&dir, &["verify"], None);
    assert!(
        String::from_utf8_lossy(&out.stdout).starts_with("DONE: PASS"),
        "{}",
        text(&out)
    );
}

/// After an unclean stop the next open drops the stat caches and kills
/// leftover check runs; status, ls and doctor answer, and verify passes.
#[test]
fn a_kill_after_the_sync_commit_recovers_on_the_next_open_crit_k2_unclean_recover() {
    let w = World::new();
    let dir = w.solved_workspace();
    let out = w.aver(&dir, &["verify"], Some("sync.committed=abort"));
    assert_eq!(out.status.code(), Some(137), "{}", text(&out));
    let stats: Vec<PathBuf> = fs::read_dir(w.home.join("mat"))
        .unwrap()
        .flatten()
        .map(|e| e.path().join("stat"))
        .filter(|p| p.exists())
        .collect();
    assert!(!stats.is_empty(), "workspace new saved a stat cache");
    let (mut leftover, pgid) = plant_leftover(&w, "B-unit");

    // The next command that opens the home recovers.
    let out = w.aver(&dir, &["diff"], None);
    assert!(out.status.success(), "{}", text(&out));
    assert!(ended_within(&mut leftover, 10), "the leftover was killed");
    assert!(!group_alive(pgid));
    let recovery: serde_json::Value =
        serde_json::from_slice(&fs::read(w.home.join("recovery.json")).unwrap()).unwrap();
    assert!(recovery["stat_caches"].as_u64().unwrap() >= 1, "{recovery}");
    assert_eq!(recovery["killed"], serde_json::json!(["repo/B-unit"]));
    // A record left is the live warm-up `workspace new` started.
    for entry in fs::read_dir(w.home.join("running")).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let pid: u32 = name.split('-').next().unwrap().parse().unwrap();
        assert!(
            aver::run::state::process_alive(pid),
            "a dead owner's record is left: {name}"
        );
    }

    for args in [
        &["status"][..],
        &["workspace", "ls"],
        &["doctor", "--verbose"],
    ] {
        let out = w.aver(&w.repo, args, None);
        assert_ne!(out.status.code(), Some(101), "{}", text(&out));
        assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
    }
    let doctor = w.aver(&w.repo, &["doctor", "--verbose"], None);
    assert!(
        String::from_utf8_lossy(&doctor.stdout).contains("stopped uncleanly"),
        "{}",
        text(&doctor)
    );
    let out = w.aver(&dir, &["verify"], None);
    assert!(
        String::from_utf8_lossy(&out.stdout).starts_with("DONE: PASS"),
        "{}",
        text(&out)
    );
}
