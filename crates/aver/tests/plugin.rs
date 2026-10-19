//! The Claude Code plugin (`integrations/claude-code`) and what `aver init`
//! writes into a repository are one set of files (ARCHITECTURE.md §2.12).
//! `make plugin-check` runs these. After an intended change to the hooks,
//! `AVER_UPDATE_GOLDEN=1 cargo test -p aver --test plugin` rewrites the
//! plugin's `hooks/hooks.json`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use aver::integrate::{self, plugin};
use serde_json::Value;

fn plugin_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(plugin::ROOT)
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The frontmatter of a Markdown file, as `key: value` pairs.
fn frontmatter(path: &Path, text: &str) -> Vec<(String, String)> {
    let rest = text
        .strip_prefix("---\n")
        .unwrap_or_else(|| panic!("{} has no frontmatter", path.display()));
    let (front, body) = rest
        .split_once("\n---\n")
        .unwrap_or_else(|| panic!("{}'s frontmatter does not end", path.display()));
    assert!(!body.trim().is_empty(), "{} has no body", path.display());
    front
        .lines()
        .map(|l| {
            let (k, v) = l
                .split_once(": ")
                .unwrap_or_else(|| panic!("{}: {l:?} is not `key: value`", path.display()));
            (k.to_owned(), v.to_owned())
        })
        .collect()
}

#[test]
fn the_manifests_are_valid() {
    let manifest: Value = serde_json::from_str(&read(&plugin_root().join(plugin::MANIFEST_PATH)))
        .expect("plugin.json is JSON");
    for key in ["name", "version", "description"] {
        assert!(manifest[key].is_string(), "plugin.json has no {key}");
    }
    assert_eq!(manifest["name"], "aver");
    let market_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.claude-plugin/marketplace.json");
    let market: Value =
        serde_json::from_str(&read(&market_path)).expect("marketplace.json is JSON");
    let entry = &market["plugins"][0];
    assert_eq!(entry["source"], "./integrations/claude-code");
    assert_eq!(entry["name"], manifest["name"]);
    assert_eq!(
        entry["version"], manifest["version"],
        "the marketplace names the plugin's version"
    );
}

#[test]
fn every_command_and_skill_is_one_aver_init_writes() {
    let root = plugin_root();
    let mut on_disk = Vec::new();
    for dir in ["commands", "skills"] {
        let mut stack = vec![root.join(dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    on_disk.push(p.strip_prefix(&root).unwrap().display().to_string());
                }
            }
        }
    }
    on_disk.sort();
    let mut shared: Vec<String> = plugin::SHARED.iter().map(|f| f.plugin.to_owned()).collect();
    shared.sort();
    assert_eq!(on_disk, shared, "a plugin file aver init does not write");
    for f in plugin::SHARED {
        let path = root.join(f.plugin);
        let text = read(&path);
        assert_eq!(text, f.text, "{} is not what this build embeds", f.plugin);
        let front = frontmatter(&path, &text);
        assert!(
            front
                .iter()
                .any(|(k, v)| k == "description" && !v.is_empty()),
            "{} has no description",
            f.plugin
        );
        if f.plugin.starts_with("skills/") {
            assert!(
                front.iter().any(|(k, v)| k == "name" && v == "aver"),
                "{} names no skill",
                f.plugin
            );
        }
    }
}

#[test]
fn the_plugin_hooks_are_rendered_from_avers_table() {
    let path = plugin_root().join(plugin::HOOKS_PATH);
    let want = plugin::hooks_json();
    if std::env::var_os("AVER_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &want).unwrap();
        return;
    }
    let have = read(&path);
    assert!(
        have == want,
        "{} differs from plugin::hooks_json(); rerun with AVER_UPDATE_GOLDEN=1\n--- have\n{have}\n--- want\n{want}",
        path.display()
    );
    let v: Value = serde_json::from_str(&have).unwrap();
    for h in integrate::HOOKS.iter().filter(|h| h.session) {
        assert!(v["hooks"][h.event].is_array(), "{}", h.event);
    }
}

/// A directory with `git` on `PATH` and nothing else Aver detects.
fn bare_path(root: &Path) -> PathBuf {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for tool in ["git", "sh"] {
        let found = Command::new("sh")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .unwrap();
        let found = String::from_utf8(found.stdout).unwrap();
        std::os::unix::fs::symlink(found.trim(), bin.join(tool)).unwrap();
    }
    bin
}

fn git(repo: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

/// `aver init --yes` in a repository with `.claude/` writes the plugin's
/// skill and commands byte for byte, and the settings hooks; with an
/// `AGENTS.md` it adds Aver's block there too.
#[test]
fn aver_init_writes_the_plugins_files_byte_for_byte() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".claude")).unwrap();
    std::fs::write(repo.join("AGENTS.md"), "# Agents\n").unwrap();
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let apps = tmp.path().join("apps");
    std::fs::create_dir_all(&apps).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_aver"))
        .args(["init", "--yes"])
        .current_dir(&repo)
        .env("AVER_HOME", tmp.path().join("aver-home"))
        .env("HOME", &home)
        .env("PATH", bare_path(tmp.path()))
        .env("AVER_APPS_DIR", &apps)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for f in plugin::SHARED {
        assert_eq!(
            std::fs::read(repo.join(f.project)).unwrap(),
            std::fs::read(plugin_root().join(f.plugin)).unwrap(),
            "{} differs from the plugin's {}",
            f.project,
            f.plugin
        );
    }
    let settings: Value = serde_json::from_str(&read(&repo.join(integrate::SETTINGS))).unwrap();
    assert_eq!(settings["hooks"], integrate::hooks());
    let agents = read(&repo.join("AGENTS.md"));
    assert!(agents.starts_with("# Agents\n"), "{agents}");
    assert!(agents.contains(integrate::text::BEGIN), "{agents}");
    assert_eq!(
        std::fs::read_dir(&home).unwrap().count(),
        0,
        "nothing under the user's home"
    );

    // Without --yes and without a terminal it shows and writes nothing.
    let fresh = tmp.path().join("fresh");
    std::fs::create_dir_all(fresh.join(".claude")).unwrap();
    std::fs::write(fresh.join("README.md"), "hello\n").unwrap();
    git(&fresh, &["init", "-q", "-b", "main"]);
    git(&fresh, &["add", "."]);
    git(&fresh, &["commit", "-q", "-m", "init"]);
    let out = Command::new(env!("CARGO_BIN_EXE_aver"))
        .arg("init")
        .current_dir(&fresh)
        .env("AVER_HOME", tmp.path().join("aver-home"))
        .env("HOME", &home)
        .env("PATH", bare_path(&tmp.path().join("p2")))
        .env("AVER_APPS_DIR", &apps)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains(".claude/skills/aver/SKILL.md"), "{stdout}");
    assert!(stdout.contains("`aver init --yes` writes them"), "{stdout}");
    assert!(!fresh.join(".claude/skills").exists());
}

/// The plugin's hooks with no `aver` installed: a starting session is told
/// to run `/aver`, and every other hook does nothing, with exit code 0.
#[test]
fn the_plugin_hooks_without_aver_never_fail() {
    let tmp = tempfile::tempdir().unwrap();
    let script = plugin_root().join("hooks/aver-hook.sh");
    let run = |name: &str| {
        let mut child = Command::new("/bin/sh")
            .arg(&script)
            .arg(name)
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", tmp.path())
            .env("AVER_BIN", tmp.path().join("no-aver"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        drop(child.stdin.take());
        child.wait_with_output().unwrap()
    };
    let start = run("session-start");
    assert_eq!(start.status.code(), Some(0));
    assert!(
        start.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&start.stderr)
    );
    let v: Value = serde_json::from_slice(&start.stdout).unwrap();
    assert_eq!(
        v["hookSpecificOutput"]["additionalContext"],
        "Aver plugin is installed, but the `aver` CLI is missing. Run `/aver` to install the binary and initialize this repository."
    );
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
    for name in ["stop", "worktree-create"] {
        let out = run(name);
        assert_eq!(out.status.code(), Some(0), "{name}");
        assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{name}");
    }
}

/// Each settings hook with no `aver` on `PATH`: the session hooks print
/// nothing and exit 0, so a teammate without Aver sees no error.
#[test]
fn the_settings_hooks_without_aver_never_fail() {
    let tmp = tempfile::tempdir().unwrap();
    for h in integrate::HOOKS.iter().filter(|h| h.session) {
        let out = Command::new("/bin/sh")
            .args(["-c", &integrate::settings_command(h.name)])
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", tmp.path())
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", h.event);
        assert!(
            out.stdout.is_empty() && out.stderr.is_empty(),
            "{}",
            h.event
        );
    }
}
