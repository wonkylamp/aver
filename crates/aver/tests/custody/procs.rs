//! Process helpers for the custody tests: leftover check runs, their
//! records beside the slots, and whether a group still runs.

use std::fs;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use super::world::World;

/// Whether a process of group `pgid` still runs. A killed one may stay a
/// zombie for a while when the init process reaps lazily, as in some
/// containers, so on Linux zombies do not count.
pub fn group_alive(pgid: i32) -> bool {
    if !cfg!(target_os = "linux") {
        // SAFETY: signal 0 only checks.
        return unsafe { libc::kill(-pgid, 0) == 0 };
    }
    fs::read_dir("/proc").unwrap().flatten().any(|e| {
        let Ok(stat) = fs::read_to_string(e.path().join("stat")) else {
            return false;
        };
        let Some(rest) = stat.rfind(')').map(|i| &stat[i + 1..]) else {
            return false;
        };
        let fields: Vec<&str> = rest.split_whitespace().collect();
        fields.first().is_some_and(|s| *s != "Z") && fields.get(2) == Some(&&*pgid.to_string())
    })
}

/// The `<slot>.pid` records under the world's slots
/// (`<home>/sides/<repo16>/`).
pub fn pid_files(w: &World) -> Vec<PathBuf> {
    fs::read_dir(w.home.join("sides"))
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|repo| fs::read_dir(repo.path()).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "pid"))
        .collect()
}

pub fn pgid_of(pid_file: &Path) -> Option<i32> {
    let v: serde_json::Value = serde_json::from_slice(&fs::read(pid_file).ok()?).ok()?;
    v["pgid"].as_i64().map(|g| g as i32)
}

/// A process group of its own sleeping for a minute, recorded as a slot's
/// leftover run of a dead process at `<home>/sides/repo/<slot>.pid`.
pub fn plant_leftover(w: &World, slot: &str) -> (std::process::Child, i32) {
    let child = Command::new("sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .unwrap();
    let pgid = child.id() as i32;
    let repo = w.home.join("sides/repo");
    fs::create_dir_all(&repo).unwrap();
    let record = serde_json::json!({
        "pgid": pgid,
        "start": aver::run::state::process_start(child.id()),
        "tag": "",
    });
    fs::write(repo.join(format!("{slot}.pid")), record.to_string()).unwrap();
    fs::write(repo.join(format!("{slot}.lock")), "").unwrap();
    (child, pgid)
}

/// Waits for `child` (reaping it) for at most `secs`; whether it ended.
pub fn ended_within(child: &mut std::process::Child, secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}
