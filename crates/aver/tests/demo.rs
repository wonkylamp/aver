//! `aver demo` and bare `aver`, through the binary.

use std::process::Command;

fn aver(dir: &std::path::Path, home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_aver"))
        .args(args)
        .current_dir(dir)
        .env("AVER_HOME", home)
        .output()
        .unwrap()
}

#[test]
fn the_demo_shows_a_gamed_test_caught_a_restore_and_a_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let out = aver(tmp.path(), &home, &["demo"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("NOT DONE: FAIL_OWN"), "{text}");
    assert!(text.contains("Back to the last green checkpoint"), "{text}");
    assert!(text.contains("DONE: PASS"), "{text}");
    // Its own home, removed with its directory: the caller's is untouched.
    assert!(!home.exists());
    let dir = text
        .lines()
        .find_map(|l| l.strip_prefix("Aver demo, in "))
        .and_then(|l| l.split(" (").next())
        .unwrap();
    assert!(!std::path::Path::new(dir).exists(), "{dir} left behind");
}

#[test]
fn bare_aver_outside_a_repository_names_init_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let out = aver(tmp.path(), &home, &[]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    assert!(
        text.contains("`aver init`") && text.contains("`aver demo`"),
        "{text}"
    );
    assert!(!home.exists());
}

#[test]
fn help_lists_everyday_commands_first_and_the_rest_under_advanced() {
    let tmp = tempfile::tempdir().unwrap();
    let out = aver(tmp.path(), &tmp.path().join("home"), &["--help"]);
    let text = String::from_utf8_lossy(&out.stdout);
    let names = |heading: &str| -> Vec<String> {
        text.lines()
            .skip_while(|l| l.trim() != heading)
            .skip(1)
            .take_while(|l| !l.trim().is_empty())
            .filter_map(|l| l.split_whitespace().next().map(str::to_owned))
            .collect()
    };
    assert_eq!(
        names("Commands:"),
        ["check", "verify", "restore", "accept", "init", "demo"]
    );
    let advanced = names("Advanced:");
    for c in [
        "workspace",
        "run",
        "combine",
        "rerun",
        "ci",
        "doctor",
        "gc",
        "status",
        "lease",
    ] {
        assert!(advanced.iter().any(|a| a == c), "{c} missing:\n{text}");
    }
}
