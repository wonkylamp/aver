//! The scripted runner (`aver::exec::scripted`) is a test seam: only the
//! crate's dev-dependency on itself turns its `scripted` feature on, so the
//! `aver` that `cargo build`, `cargo install` and a release build make has
//! no scripted runner and `AVER_SCRIPTED` means nothing to it.

use std::process::Command;

const FEATURE: &str = "aver feature \"scripted\"";

fn tree(edges: &str) -> String {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args([
            "tree",
            "--offline",
            "-e",
            edges,
            "-p",
            "aver",
            "--prefix",
            "none",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn only_the_tests_turn_the_scripted_runner_on() {
    let built = tree("normal,build,features");
    assert!(!built.contains(FEATURE), "{built}");
    let tested = tree("features");
    assert!(tested.contains(FEATURE), "{tested}");
}
