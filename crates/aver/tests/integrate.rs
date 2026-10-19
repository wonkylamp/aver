//! `aver integrate claude` and the worktree hooks it installs, through the
//! `aver` binary with the scripted runner. No Docker, no Claude.

mod common;
mod git_support;
mod run_support;
mod scripted;
mod world;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use run_support::*;
use serde_json::Value;

/// Runs `aver` with a throwaway `$HOME`, so the user's own settings can be
/// shown untouched.
fn aver_in(d: &TestHome, cwd: &Path, args: &[&str], stdin: &str) -> Output {
    let home = d.root.join("user-home");
    std::fs::create_dir_all(&home).unwrap();
    let mut child = Command::new(aver_bin())
        .args(args)
        .current_dir(cwd)
        .env("AVER_HOME", d.home.root())
        .env("HOME", &home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("AVER_URL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn integrate_claude_asks_then_writes_committable_files() {
    let d = TestHome::start();
    let repo = d.calc();
    std::fs::create_dir_all(repo.join(".claude")).unwrap();
    let local = r#"{"permissions":{"allow":["Bash(make:*)"]}}"#;
    std::fs::write(repo.join(".claude/settings.local.json"), local).unwrap();
    let args = ["integrate", "claude"];

    let refused = aver_in(&d, &repo, &args, "");
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("pass --yes"), "{stderr}");
    let shown = String::from_utf8_lossy(&refused.stdout);
    assert!(
        shown.contains("--- .claude/settings.json (new file)"),
        "{shown}"
    );
    assert!(
        shown.contains("--- .claude/skills/aver/SKILL.md (new file)"),
        "{shown}"
    );
    assert!(
        shown.contains("--- .claude/commands/aver.md (new file)"),
        "{shown}"
    );
    assert!(!shown.contains("settings.local.json"), "{shown}");
    assert!(!shown.contains(".mcp.json"), "{shown}");
    assert!(shown.contains("hook worktree-create"), "{shown}");
    assert!(
        !repo.join(".claude/settings.json").exists(),
        "nothing written without consent"
    );

    let mut yes = args.to_vec();
    yes.push("--yes");
    let done = aver_in(&d, &repo, &yes, "");
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    let settings = json(&repo.join(".claude/settings.json"));
    let events: Vec<&String> = settings["hooks"].as_object().unwrap().keys().collect();
    assert_eq!(
        events,
        [
            "SessionStart",
            "Stop",
            "SubagentStop",
            "WorktreeCreate",
            "WorktreeRemove"
        ]
    );
    for event in events {
        assert_eq!(
            settings["hooks"][event].as_array().map(Vec::len),
            Some(1),
            "{event}"
        );
    }
    let text = std::fs::read_to_string(repo.join(".claude/settings.json")).unwrap();
    for machine in [
        d.home.root().display().to_string(),
        aver_bin().display().to_string(),
        "AVER_HOME".into(),
    ] {
        assert!(!text.contains(&machine), "{machine} in {text}");
    }
    for f in aver::integrate::plugin::SHARED {
        assert_eq!(
            std::fs::read_to_string(repo.join(f.project)).unwrap(),
            f.text,
            "{} is the plugin's {}",
            f.project,
            f.plugin
        );
    }
    assert_eq!(
        std::fs::read_to_string(repo.join(".claude/settings.local.json")).unwrap(),
        local,
        "the local settings are the user's"
    );
    assert!(
        !repo.join(".mcp.json").exists(),
        "no MCP server is registered"
    );
    let exclude = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap_or_default();
    assert!(!exclude.contains(".claude"), "{exclude}");
    let user_home = d.root.join("user-home");
    assert_eq!(
        std::fs::read_dir(&user_home).unwrap().count(),
        0,
        "nothing under the user's home"
    );

    let again = aver_in(&d, &repo, &yes, "");
    assert!(String::from_utf8_lossy(&again.stdout).contains("already set up"));
}

/// The committed settings are merged like any others, and Aver 0.4's hooks
/// with this machine's paths move out of the local settings; the user's
/// own hooks in either file stay.
#[test]
fn integrate_claude_moves_the_local_hooks_into_the_committed_settings() {
    let d = TestHome::start();
    let repo = d.calc();
    std::fs::create_dir_all(repo.join(".claude")).unwrap();
    let committed = r#"{"permissions":{"allow":["Bash(make:*)"]}}"#;
    std::fs::write(repo.join(".claude/settings.json"), committed).unwrap();
    git_support::git(&repo, &["add", ".claude/settings.json"], b"");
    git_support::git(&repo, &["commit", "-q", "-m", "settings"], b"");
    let local = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"AVER_HOME=/h /old/aver hook stop --rev 3"},{"type":"command","command":"notify.sh"}]}],"WorktreeCreate":[{"hooks":[{"type":"command","command":"AVER_HOME=/h /old/aver hook worktree-create --rev 3"}]}]}}"#;
    std::fs::write(repo.join(".claude/settings.local.json"), local).unwrap();

    let out = aver_in(&d, &repo, &["integrate", "claude", "--yes"], "");
    let shown = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        shown.contains("--- .claude/settings.local.json (updated; only Aver's old hooks go)"),
        "{shown}"
    );
    assert_eq!(
        json(&repo.join(".claude/settings.local.json")),
        serde_json::json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":"notify.sh"}]}]}})
    );
    let settings = json(&repo.join(".claude/settings.json"));
    assert_eq!(settings["permissions"]["allow"][0], "Bash(make:*)");
    assert!(settings["hooks"]["WorktreeCreate"].is_array(), "{settings}");

    // Removing Aver takes its hooks out of the committed file and never
    // puts the old ones back into the local one.
    let removed = aver_in(&d, &repo, &["setup", "--remove", "--yes"], "");
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(repo.join(".claude/settings.json")).unwrap(),
        committed
    );
    let left = std::fs::read_to_string(repo.join(".claude/settings.local.json")).unwrap();
    assert!(!left.contains("aver hook"), "{left}");
    assert!(left.contains("notify.sh"), "{left}");
    assert!(!repo.join(".claude/skills").exists());
    assert!(!repo.join(".claude/commands").exists());
}

/// Over an integration of Aver before 0.4, `aver integrate claude` takes
/// the `aver mcp` server out of `.mcp.json` and its approval out of the
/// settings, and keeps the user's other servers.
#[test]
fn integrate_claude_takes_out_the_old_mcp_server() {
    let d = TestHome::start();
    let repo = d.calc();
    std::fs::create_dir_all(repo.join(".claude")).unwrap();
    std::fs::write(
        repo.join(".claude/settings.local.json"),
        r#"{"enabledMcpjsonServers":["aver","db"]}"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(".mcp.json"),
        r#"{"mcpServers":{"aver":{"command":"/old/aver","args":["mcp","--key","claude-calc"]},"db":{"command":"db-mcp"}}}"#,
    )
    .unwrap();
    let done = aver_in(&d, &repo, &["integrate", "claude", "--yes"], "");
    let shown = String::from_utf8_lossy(&done.stdout);
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    assert!(shown.contains("no longer runs an MCP server"), "{shown}");
    assert_eq!(
        json(&repo.join(".mcp.json")),
        serde_json::json!({"mcpServers":{"db":{"command":"db-mcp"}}})
    );
    let settings = json(&repo.join(".claude/settings.local.json"));
    assert_eq!(settings["enabledMcpjsonServers"], serde_json::json!(["db"]));

    // A file that held only Aver's server goes.
    std::fs::write(repo.join(".mcp.json"), r#"{"mcpServers":{"aver":{}}}"#).unwrap();
    let done = aver_in(&d, &repo, &["integrate", "claude", "--yes"], "");
    assert!(done.status.success());
    assert!(!repo.join(".mcp.json").exists());
}

#[test]
fn worktree_hooks_create_and_remove_an_aver_workspace() {
    let d = TestHome::start();
    let repo = d.calc();
    let done = aver_in(&d, &repo, &["integrate", "claude", "--yes"], "");
    assert!(done.status.success());
    let input = format!(
        r#"{{"session_id":"s","cwd":"{}","hook_event_name":"WorktreeCreate","name":"bold-oak-a3f2"}}"#,
        repo.display()
    );
    let created = aver_in(&d, &repo, &["hook", "worktree-create"], &input);
    let stdout = String::from_utf8_lossy(&created.stdout);
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let dir = Path::new(stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap());
    assert!(dir.is_absolute());
    assert!(
        dir.ends_with("work/calc/wt-bold-oak-a3f2"),
        "{}",
        dir.display()
    );
    assert!(dir.join(".aver/workspace.toml").is_file());
    // The session is told what the directory is and how to finish.
    let brief = std::fs::read_to_string(dir.join(".aver/BRIEF.md")).unwrap();
    assert!(
        brief.contains("TASK: Claude Code worktree bold-oak-a3f2"),
        "{brief}"
    );
    assert!(
        brief.contains("`aver check`") && brief.contains("`aver verify`"),
        "{brief}"
    );
    assert!(brief.contains("not instructions"), "{brief}");
    assert!(brief.contains("aver/wt-bold-oak-a3f2"), "{brief}");
    assert_eq!(
        std::fs::read_to_string(dir.join("src/math.js")).unwrap(),
        "exports.add = (a, b) => a + b;\n"
    );
    // The project's Claude settings and skill came along before they are
    // committed, outside what Aver syncs.
    assert!(json(&dir.join(".claude/settings.json"))["hooks"]["Stop"].is_array());
    assert!(dir.join(".claude/skills/aver/SKILL.md").is_file());
    std::fs::write(dir.join("src/neg.js"), "module.exports = (x) => -x;\n").unwrap();
    let diff = aver_in(&d, dir, &["diff"], "");
    let diff_text = String::from_utf8_lossy(&diff.stdout);
    assert!(diff_text.contains("src/neg.js"), "{diff_text}");
    assert!(!diff_text.contains(".claude/"), "{diff_text}");

    let input = format!(
        r#"{{"session_id":"s","hook_event_name":"WorktreeRemove","worktree_path":"{}"}}"#,
        dir.display()
    );
    let removed = aver_in(&d, &repo, &["hook", "worktree-remove"], &input);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!dir.exists(), "the directory is gone");

    let stranger = d.root.join("not-a-workspace");
    std::fs::create_dir_all(&stranger).unwrap();
    let input = format!(r#"{{"worktree_path":"{}"}}"#, stranger.display());
    let refused = aver_in(&d, &repo, &["hook", "worktree-remove"], &input);
    assert_eq!(refused.status.code(), Some(2));
    assert!(
        stranger.exists(),
        "a directory Aver did not make is left alone"
    );
}

/// An integration written by an older Aver (here: hooks without `--rev`)
/// is reported by doctor, by the first hook of each session, and once per
/// build by any command in the repository; integrating again fixes all.
#[test]
fn a_stale_integration_is_reported_by_doctor_the_hooks_and_the_cli() {
    let d = TestHome::start();
    let repo = d.calc();
    let done = aver_in(&d, &repo, &["integrate", "claude", "--yes"], "");
    assert!(done.status.success());
    let doctor = |want: &str| {
        let out = aver_in(&d, &repo, &["doctor", "--verbose"], "");
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let line = text
            .lines()
            .find(|l| l.contains("integration"))
            .unwrap_or_else(|| panic!("no integration line: {text}"))
            .to_owned();
        assert!(line.contains(want), "{line}");
    };
    doctor("Claude Code hooks are current");

    // The settings an older Aver wrote: the same hooks without a rev.
    let path = repo.join(".claude/settings.json");
    let rev = format!(" --rev {}", aver::integrate::INTEGRATION_REV);
    let current = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, current.replace(&rev, "")).unwrap();
    doctor("differ from what this aver writes (installed rev none");

    // Each start is its own event: a different `source`, so the second is
    // not taken for a copy of the first.
    let start = |session: &str, source: &str| {
        let input = serde_json::json!({
            "session_id": session, "cwd": repo, "hook_event_name": "SessionStart", "source": source,
        });
        let out = aver_in(&d, &repo, &["hook", "session-start"], &input.to_string());
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    let first: Value = serde_json::from_str(&start("s1", "startup")).unwrap();
    assert!(
        context(&first).contains("run `aver integrate claude`"),
        "{first}"
    );
    // The root's pointer to `aver workspace new` comes at every start; the
    // warning once per session.
    let again = start("s1", "resume");
    assert!(again.contains("aver workspace new"), "{again}");
    assert!(!again.contains("aver integrate claude"), "once per session");
    assert!(
        start("s2", "startup").contains("aver integrate claude"),
        "a new session hears it too"
    );

    // integrations.json from an older build: one warning on stderr.
    let record = d.home.root().join("integrations.json");
    let mut installed: Value =
        serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    installed["entries"][0]["rev"] = 0.into();
    std::fs::write(&record, installed.to_string()).unwrap();
    let warned = aver_in(&d, &repo, &["key"], "");
    let stderr = String::from_utf8_lossy(&warned.stderr);
    assert!(stderr.contains("is from an older Aver"), "{stderr}");
    let again = aver_in(&d, &repo, &["key"], "");
    assert!(!String::from_utf8_lossy(&again.stderr).contains("older Aver"));

    // Integrating again replaces the old hooks instead of adding to them.
    let fixed = aver_in(&d, &repo, &["integrate", "claude", "--yes"], "");
    assert!(fixed.status.success());
    let settings = json(&path);
    assert_eq!(
        settings["hooks"]["Stop"].as_array().unwrap().len(),
        1,
        "{settings}"
    );
    doctor("Claude Code hooks are current");
}

/// Runs `aver hook <name>` with a hook input built from `fields`, and
/// returns its stdout parsed as JSON (`Null` when it printed nothing).
fn hook(d: &TestHome, cwd: &Path, name: &str, fields: Value) -> Value {
    let rev = aver::integrate::INTEGRATION_REV.to_string();
    let out = aver_in(d, cwd, &["hook", name, "--rev", &rev], &fields.to_string());
    assert!(
        out.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    match stdout.trim() {
        "" => Value::Null,
        text => serde_json::from_str(text).unwrap_or_else(|e| panic!("{name}: {e}: {text}")),
    }
}

fn context(out: &Value) -> &str {
    out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_else(|| panic!("no context: {out}"))
}

/// The hooks `aver integrate claude` installs for sessions and subagents in
/// worktree workspaces: the short brief at a session's start and the verify
/// gate on stop (a subagent's workspace found by its `agent_id` while its
/// `cwd` is the parent's), each fed Claude Code's hook input as it sends
/// it.
#[test]
fn worktree_hooks_brief_the_session_and_gate_its_stop() {
    let d = TestHome::start();
    let repo = d.calc();
    let done = aver_in(&d, &repo, &["integrate", "claude", "--yes"], "");
    assert!(done.status.success());
    let settings = json(&repo.join(".claude/settings.json"));
    for (event, name) in [
        ("SessionStart", "session-start"),
        ("Stop", "stop"),
        ("SubagentStop", "stop"),
    ] {
        let command = settings["hooks"][event][0]["hooks"][0]["command"]
            .as_str()
            .unwrap_or_else(|| panic!("{event}"));
        assert_eq!(command, aver::integrate::settings_command(name), "{event}");
    }
    assert!(settings["hooks"].get("PostToolUse").is_none(), "{settings}");

    let agent = "a5e1f00dcafe";
    let input = serde_json::json!({
        "session_id": "s", "cwd": repo, "hook_event_name": "WorktreeCreate",
        "name": format!("agent-{agent}"),
    });
    let created = aver_in(&d, &repo, &["hook", "worktree-create"], &input.to_string());
    assert!(created.status.success());
    let stdout = String::from_utf8_lossy(&created.stdout);
    let dir = Path::new(stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap()).to_owned();

    let sub = |event: &str, extra: Value| {
        let mut v = serde_json::json!({
            "session_id": "parent", "cwd": repo, "hook_event_name": event,
            "agent_id": agent, "agent_type": "general-purpose",
        });
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    };
    // (a) The brief: a `claude --worktree` session's start, its cwd the
    // worktree.
    let out = hook(
        &d,
        &dir,
        "session-start",
        serde_json::json!({"session_id": "w", "cwd": dir, "hook_event_name": "SessionStart", "source": "startup"}),
    );
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "SessionStart");
    let brief = context(&out);
    assert!(brief.contains(&dir.display().to_string()), "{brief}");
    assert!(
        brief.contains("DONE MEANS `aver verify` prints DONE: PASS"),
        "{brief}"
    );
    assert!(brief.contains("`aver check`"), "{brief}");
    assert!(brief.contains(&format!("aver/wt-agent-{agent}")), "{brief}");
    // A session in the repository's own checkout is told to make a
    // workspace before editing; its stop is not gated.
    let outside =
        serde_json::json!({"session_id": "p", "cwd": repo, "hook_event_name": "SessionStart"});
    let root = hook(&d, &repo, "session-start", outside.clone());
    assert_eq!(context(&root), aver::run::hooks::ROOT_CONTEXT);
    assert!(context(&root).contains("`aver workspace new`"));
    assert_eq!(hook(&d, &repo, "stop", outside), Value::Null);
    // A subagent's start there, and a repository not set up with Aver, hear
    // nothing.
    let subagent = serde_json::json!({"session_id": "p", "cwd": repo, "hook_event_name": "SessionStart", "agent_id": "a1"});
    assert_eq!(hook(&d, &repo, "session-start", subagent), Value::Null);
    let plain = d.root.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    git_support::git(&plain, &["init", "-q"], b"");
    let elsewhere =
        serde_json::json!({"session_id": "p", "cwd": plain, "hook_event_name": "SessionStart"});
    assert_eq!(hook(&d, &plain, "session-start", elsewhere), Value::Null);

    // (b) The gate. Unchanged: the subagent stops.
    let stop = |extra: Value| hook(&d, &repo, "stop", sub("SubagentStop", extra));
    assert_eq!(
        stop(serde_json::json!({"last_assistant_message": "nothing to do"})),
        Value::Null
    );
    // Changed and never verified: blocked once, fast.
    std::fs::write(dir.join("src/neg.js"), "module.exports = (x) => -x;\n").unwrap();
    std::fs::write(
        dir.join("test/neg.test.json"),
        r#"{"negates": "pass-if:src/neg.js"}"#,
    )
    .unwrap();
    let started = std::time::Instant::now();
    let out = stop(serde_json::json!({}));
    let took = started.elapsed();
    assert_eq!(out["decision"], "block", "{out}");
    let reason = out["reason"].as_str().unwrap();
    assert!(reason.contains("you changed 2 files"), "{reason}");
    assert!(reason.contains("No `aver verify` has run"), "{reason}");
    assert!(
        took < std::time::Duration::from_secs(3),
        "the gate took {took:?}"
    );
    // A subagent without a worktree, in a directory with changes it did
    // not make, is not gated.
    let shared = serde_json::json!({"session_id": "w", "cwd": dir, "hook_event_name": "SubagentStop", "agent_id": "a0ther"});
    assert_eq!(hook(&d, &dir, "stop", shared), Value::Null);
    // The stop that follows the block, and a later one on the same files.
    assert_eq!(
        stop(serde_json::json!({"stop_hook_active": true})),
        Value::Null
    );
    assert_eq!(
        stop(serde_json::json!({"last_assistant_message": "later"})),
        Value::Null
    );
    // The parent session's own gate is counted apart.
    let main = |session: &str| {
        hook(
            &d,
            &dir,
            "stop",
            serde_json::json!({"session_id": session, "cwd": dir, "hook_event_name": "Stop", "stop_hook_active": false}),
        )
    };
    assert_eq!(main("w")["decision"], "block");

    // A PASS receipt for exactly these files lets every stopper go.
    let verified = aver_in(&d, &dir, &["verify"], "");
    let text = String::from_utf8_lossy(&verified.stdout);
    assert_eq!(verified.status.code(), Some(0), "{text}");
    assert_eq!(main("fresh"), Value::Null, "a fresh PASS");
    // A change after it: the receipt is stale.
    std::fs::write(dir.join("README.md"), "calc, with negate\n").unwrap();
    let out = main("stale");
    assert_eq!(out["decision"], "block", "{out}");
    assert!(
        out["reason"].as_str().unwrap().contains("earlier version"),
        "{out}"
    );
    // The block limit: one stopper, a new version of the files each time.
    let mut blocked = 0;
    for n in 0..5 {
        std::fs::write(dir.join("notes.md"), format!("draft {n}\n")).unwrap();
        // Each stop follows new work, so Claude Code's input differs.
        let out = hook(
            &d,
            &dir,
            "stop",
            serde_json::json!({"session_id": "limit", "cwd": dir, "hook_event_name": "Stop", "last_assistant_message": format!("draft {n}")}),
        );
        if out["decision"] == "block" {
            blocked += 1;
        }
    }
    assert_eq!(blocked, 3, "blocks stop after the limit");
    // After `aver giveup`, nobody is blocked.
    let gave = aver_in(&d, &dir, &["giveup", "needs a dependency"], "");
    assert!(gave.status.success());
    assert_eq!(main("after-giveup"), Value::Null);
}

/// Settings an Aver before 0.4 wrote still call `aver hook pre-edit` and
/// the other hooks it had; each exits 0 and prints nothing, so no tool call
/// fails before `aver setup` rewrites them.
#[test]
fn the_hooks_before_0_4_exit_quietly() {
    let d = TestHome::start();
    let repo = d.calc();
    let input = serde_json::json!({
        "session_id": "s", "cwd": repo, "hook_event_name": "PreToolUse", "tool_name": "Edit",
        "tool_input": {"file_path": repo.join("src/math.js")},
    });
    for name in aver::integrate::LEGACY_HOOK_NAMES {
        for args in [
            vec!["hook", name],
            vec!["hook", name, "--rev", "2"],
            vec!["hook", name, "--plugin"],
        ] {
            let out = aver_in(&d, &repo, &args, &input.to_string());
            assert_eq!(out.status.code(), Some(0), "{args:?}");
            assert!(out.stdout.is_empty(), "{args:?}");
            assert!(
                out.stderr.is_empty(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}

#[test]
fn integrate_codex_writes_agents_md_then_changes_nothing() {
    let d = TestHome::start();
    let repo = d.calc();
    let mine = "# Agents\n\nRun `npm test`.\n";
    std::fs::write(repo.join("AGENTS.md"), mine).unwrap();
    let args = ["integrate", "codex"];
    let refused = aver_in(&d, &repo, &args, "");
    assert_eq!(refused.status.code(), Some(2));
    let shown = String::from_utf8_lossy(&refused.stdout);
    assert!(!shown.contains(".codex/config.toml"), "{shown}");
    assert!(
        shown.contains("--- AGENTS.md (updated; only Aver's block changes)"),
        "{shown}"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("AGENTS.md")).unwrap(),
        mine,
        "nothing written without consent"
    );

    let mut yes = args.to_vec();
    yes.push("--yes");
    let done = aver_in(&d, &repo, &yes, "");
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    assert!(!repo.join(".codex").exists());
    let agents = std::fs::read_to_string(repo.join("AGENTS.md")).unwrap();
    assert!(
        agents.starts_with("# Agents\n\nRun `npm test`.\n"),
        "{agents}"
    );
    assert!(agents.contains("`aver check`"), "{agents}");
    let exclude = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap_or_default();
    assert!(
        !exclude.contains("AGENTS.md"),
        "the instructions are the user's to commit"
    );

    let again = aver_in(&d, &repo, &yes, "");
    assert!(
        String::from_utf8_lossy(&again.stdout).contains("already set up"),
        "{}",
        String::from_utf8_lossy(&again.stdout)
    );
    let generic = aver_in(&d, &repo, &["integrate", "agents-md", "--yes"], "");
    assert!(String::from_utf8_lossy(&generic.stdout).contains("already set up"));
}

/// The repository's files, and `.git/info/exclude`, by path: what setup
/// and remove may change.
fn snapshot(repo: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut dirs = vec![repo.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            let rel = p.strip_prefix(repo).unwrap().display().to_string();
            if rel == ".git" {
                continue;
            }
            if p.is_dir() {
                out.insert(format!("{rel}/"), Vec::new());
                dirs.push(p);
            } else {
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    let exclude = repo.join(".git/info/exclude");
    out.insert(
        ".git/info/exclude".into(),
        std::fs::read(&exclude).unwrap_or_default(),
    );
    out
}

/// `aver setup` for every tool and then `aver setup --remove` leave the
/// repository byte for byte as it was, files of the user's own included.
#[test]
fn setup_then_remove_leaves_the_repository_byte_identical() {
    let d = TestHome::start();
    let repo = d.calc();
    for (rel, text) in [
        ("AGENTS.md", "# Agents\n\nRun `npm test`."),
        (
            ".vscode/mcp.json",
            r#"{"servers":{"github":{"type":"http","url":"https://api.githubcopilot.com/mcp/"}}}"#,
        ),
        (
            ".claude/settings.local.json",
            r#"{"permissions":{"allow":["Bash(make:*)"]}}"#,
        ),
    ] {
        let path = repo.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let before = snapshot(&repo);
    let mut args = vec!["setup"];
    for tool in ["claude", "cursor", "codex", "gemini", "vscode", "agents-md"] {
        args.extend(["--tool", tool]);
    }

    let asked = aver_in(&d, &repo, &args, "");
    assert_eq!(asked.status.code(), Some(2));
    let shown = String::from_utf8_lossy(&asked.stdout);
    for heading in [
        "=== claude",
        "=== cursor",
        "=== codex",
        "=== gemini",
        "=== vscode",
    ] {
        assert!(shown.contains(heading), "{shown}");
    }
    assert!(shown.contains("aver setup --remove"), "{shown}");
    assert_eq!(snapshot(&repo), before, "nothing written without consent");

    args.push("--yes");
    let done = aver_in(&d, &repo, &args, "");
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    for rel in [
        ".cursor/rules/aver.mdc",
        "GEMINI.md",
        ".github/copilot-instructions.md",
    ] {
        assert!(repo.join(rel).is_file(), "{rel}");
    }
    for rel in [
        ".mcp.json",
        ".cursor/mcp.json",
        ".codex/config.toml",
        ".gemini/settings.json",
    ] {
        assert!(!repo.join(rel).exists(), "{rel}");
    }
    assert!(repo.join(".git/aver/integrate.json").is_file());
    let again = aver_in(&d, &repo, &args, "");
    let again_out = String::from_utf8_lossy(&again.stdout);
    assert!(again.status.success(), "{again_out}");
    assert!(again_out.contains("Nothing to do"), "{again_out}");

    let refused = aver_in(&d, &repo, &["setup", "--remove"], "");
    assert_eq!(refused.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&refused.stdout).contains("(deleted)"));
    let removed = aver_in(&d, &repo, &["setup", "--remove", "--yes"], "");
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert_eq!(snapshot(&repo), before, "the repository is as it was");
    assert!(!repo.join(".git/aver").exists(), "the record is gone");
    let nothing = aver_in(&d, &repo, &["setup", "--remove", "--yes"], "");
    assert!(String::from_utf8_lossy(&nothing.stdout).contains("nothing set up"));
}

/// Files set up by an older aver, with no record, lose Aver's parts (the
/// MCP server it registered before 0.4 too) and keep everything else.
#[test]
fn remove_without_a_record_takes_out_only_avers_parts() {
    let d = TestHome::start();
    let repo = d.calc();
    let done = aver_in(&d, &repo, &["integrate", "codex", "--yes"], "");
    assert!(done.status.success());
    std::fs::remove_dir_all(repo.join(".git/aver")).unwrap();
    let config = repo.join(".codex/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        "# BEGIN aver: written by `aver integrate`; running it again replaces this block\n[mcp_servers.aver]\ncommand = \"aver\"\n# END aver\n\nmodel = \"o4\"\n",
    )
    .unwrap();
    let removed = aver_in(&d, &repo, &["setup", "--remove", "--yes"], "");
    assert!(removed.status.success());
    assert!(!repo.join("AGENTS.md").exists(), "Aver's own file goes");
    let left = std::fs::read_to_string(&config).unwrap();
    assert_eq!(left.trim(), "model = \"o4\"", "{left}");
}

/// A torn record is lost, but removal still leaves the repository as it
/// was: a committed settings file in the team's own layout, no directory
/// Aver made, and no torn record behind.
#[test]
fn remove_over_a_torn_record_leaves_the_repository_as_it_was() {
    let d = TestHome::start();
    let repo = d.calc();
    let settings = "{\n  \"permissions\": { \"allow\": [\"Bash(make:*)\"] },\n  \"hooks\": {\n    \"PreToolUse\": [ { \"matcher\": \"Bash\", \"hooks\": [ { \"type\": \"command\", \"command\": \"lint.sh\" } ] } ]\n  }\n}\n";
    std::fs::create_dir_all(repo.join(".claude")).unwrap();
    std::fs::write(repo.join(".claude/settings.json"), settings).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["add", ".claude/settings.json"]);
    git(&["commit", "-qm", "team settings"]);
    let before = snapshot(&repo);
    for tool in ["claude", "agents-md"] {
        assert!(aver_in(&d, &repo, &["integrate", tool, "--yes"], "")
            .status
            .success());
    }
    let record = repo.join(".git/aver/integrate.json");
    let whole = std::fs::read_to_string(&record).unwrap();
    std::fs::write(&record, &whole[..whole.len() / 2]).unwrap();

    let removed = aver_in(&d, &repo, &["setup", "--remove", "--yes"], "");
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert_eq!(snapshot(&repo), before, "the repository is as it was");
    assert!(!record.exists(), "the torn record is gone");
}

/// Without --tool, setup offers the tools it finds on PATH.
#[test]
fn setup_offers_the_tools_it_finds() {
    let d = TestHome::start();
    let repo = d.calc();
    let bin = d.root.join("stub-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let codex = bin.join("codex");
    std::fs::write(&codex, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&codex, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let git = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    std::os::unix::fs::symlink(git.trim(), bin.join("git")).unwrap();
    let apps = d.root.join("no-apps");
    std::fs::create_dir_all(&apps).unwrap();
    // Codex is set up only in a repository that shows it uses it.
    std::fs::create_dir_all(repo.join(".codex")).unwrap();
    let out = Command::new(aver_bin())
        .arg("setup")
        .current_dir(&repo)
        .env("AVER_HOME", d.home.root())
        .env("HOME", d.root.join("user-home"))
        .env("PATH", &bin)
        .env("AVER_APPS_DIR", &apps)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let shown = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "{shown}");
    assert!(shown.contains("Codex CLI"), "{shown}");
    assert!(shown.contains("--- AGENTS.md (new file)"), "{shown}");
}

/// Starts `aver` like [`aver_in`] without waiting for it.
fn spawn_in(d: &TestHome, cwd: &Path, args: &[&str], stdin: &str) -> std::process::Child {
    let home = d.root.join("user-home");
    std::fs::create_dir_all(&home).unwrap();
    let mut child = Command::new(aver_bin())
        .args(args)
        .current_dir(cwd)
        .env("AVER_HOME", d.home.root())
        .env("HOME", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child
}

fn last_line(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .to_owned()
}

/// The workspace directory a `worktree-create` hook made in `repo`.
fn worktree(d: &TestHome, repo: &Path, name: &str) -> std::path::PathBuf {
    let input = serde_json::json!({
        "session_id": "s", "cwd": repo, "hook_event_name": "WorktreeCreate", "name": name,
    });
    let created = aver_in(d, repo, &["hook", "worktree-create"], &input.to_string());
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    last_line(&created).into()
}

/// The plugin's hooks and a repository's settings both run `aver hook` for
/// one event, in parallel and with the same input: one acts, the other
/// exits 0 having done nothing, and a second `worktree-create` prints the
/// first one's directory, since Claude Code compares the paths.
#[test]
fn a_hook_run_twice_for_one_event_acts_once() {
    let d = TestHome::start();
    let repo = d.calc();
    let rev = aver::integrate::INTEGRATION_REV.to_string();

    // WorktreeCreate, from the settings and from the plugin at once.
    let input = serde_json::json!({
        "session_id": "s", "cwd": repo, "hook_event_name": "WorktreeCreate", "name": "twice",
    })
    .to_string();
    let a = spawn_in(
        &d,
        &repo,
        &["hook", "worktree-create", "--rev", &rev],
        &input,
    );
    let b = spawn_in(&d, &repo, &["hook", "worktree-create", "--plugin"], &input);
    let (a, b) = (a.wait_with_output().unwrap(), b.wait_with_output().unwrap());
    for out in [&a, &b] {
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let dir = last_line(&a);
    assert_eq!(dir, last_line(&b), "both print the one directory");
    assert!(dir.ends_with("work/calc/wt-twice"), "{dir}");
    let made: Vec<_> = std::fs::read_dir(d.home.root().join("work/calc"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(made.len(), 1, "one workspace: {made:?}");
    // A copy that comes after the first finished gets the same answer.
    let late = aver_in(&d, &repo, &["hook", "worktree-create"], &input);
    assert_eq!(last_line(&late), dir);
    let dir = Path::new(&dir).to_owned();

    // SessionStart: one brief.
    let start = serde_json::json!({"session_id": "w", "cwd": dir, "hook_event_name": "SessionStart", "source": "startup"}).to_string();
    let a = spawn_in(&d, &dir, &["hook", "session-start", "--rev", &rev], &start);
    let b = spawn_in(&d, &dir, &["hook", "session-start", "--plugin"], &start);
    let outs = [a.wait_with_output().unwrap(), b.wait_with_output().unwrap()];
    let briefs = outs.iter().filter(|o| !o.stdout.is_empty()).count();
    assert_eq!(briefs, 1, "one brief");

    // Stop with unverified changes: one block, one silent run.
    std::fs::write(dir.join("src/neg.js"), "module.exports = (x) => -x;\n").unwrap();
    let stop = serde_json::json!({"session_id": "w", "cwd": dir, "hook_event_name": "Stop", "stop_hook_active": false, "last_assistant_message": "done"}).to_string();
    let a = spawn_in(&d, &dir, &["hook", "stop", "--rev", &rev], &stop);
    let b = spawn_in(&d, &dir, &["hook", "stop", "--plugin"], &stop);
    let outs = [a.wait_with_output().unwrap(), b.wait_with_output().unwrap()];
    let blocks: Vec<&Output> = outs.iter().filter(|o| !o.stdout.is_empty()).collect();
    assert_eq!(blocks.len(), 1, "one block");
    let block: Value = serde_json::from_slice(&blocks[0].stdout).unwrap();
    assert_eq!(block["decision"], "block");
    for o in &outs {
        assert!(o.status.success());
    }

    // A claim expires: the same input later is a new event and runs.
    let claims = d.home.root().join("tmp/hook-claims");
    for e in std::fs::read_dir(&claims).unwrap().flatten() {
        let mut r: Value =
            serde_json::from_str(&std::fs::read_to_string(e.path()).unwrap()).unwrap();
        if r["done_ms"].is_u64() {
            r["done_ms"] = 1.into();
        }
        std::fs::write(e.path(), r.to_string()).unwrap();
    }
    std::fs::write(dir.join("src/neg.js"), "module.exports = (x) => 0 - x;\n").unwrap();
    let again = aver_in(&d, &dir, &["hook", "stop"], &stop);
    let again: Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(again["decision"], "block", "an expired claim runs again");
    // A claim whose process died unfinished is taken at once.
    for e in std::fs::read_dir(&claims).unwrap().flatten() {
        std::fs::write(e.path(), r#"{"pid":999999,"start":1}"#).unwrap();
    }
    std::fs::write(dir.join("src/neg.js"), "module.exports = (x) => -1 * x;\n").unwrap();
    let again = aver_in(&d, &dir, &["hook", "stop"], &stop);
    let again: Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(again["decision"], "block", "a dead claim is taken");

    // A failed run gives its event up: a WorktreeRemove refused for an
    // unfetched commit and retried at once is refused again, not skipped.
    let kept = worktree(&d, &repo, "kept");
    std::fs::write(kept.join("src/neg.js"), "module.exports = (x) => -x;\n").unwrap();
    git_support::git(&kept, &["add", "src/neg.js"], b"");
    git_support::git(&kept, &["commit", "-q", "-m", "negate"], b"");
    let remove = serde_json::json!({"session_id": "s", "cwd": repo, "hook_event_name": "WorktreeRemove", "worktree_path": kept}).to_string();
    for attempt in ["first", "retry"] {
        let out = aver_in(&d, &repo, &["hook", "worktree-remove"], &remove);
        assert_eq!(out.status.code(), Some(2), "{attempt}");
        assert!(kept.is_dir(), "{attempt}: the directory is kept");
    }
}

/// `[agents] stop = "strict"` in the base's `aver.toml` keeps blocking a
/// stop, also right after a block and on the same files, until `aver
/// giveup` (or a passing verify); the agent's own copy of `aver.toml`
/// cannot turn it off.
#[test]
fn the_strict_stop_gate_blocks_until_a_giveup() {
    let d = TestHome::start();
    let repo = d.calc();
    let toml = std::fs::read_to_string(repo.join("aver.toml")).unwrap();
    std::fs::write(
        repo.join("aver.toml"),
        format!("{toml}\n[agents]\nstop = \"strict\"\n"),
    )
    .unwrap();
    git_support::git(&repo, &["commit", "-q", "-am", "strict"], b"");
    let dir = worktree(&d, &repo, "strict");
    std::fs::write(dir.join("src/neg.js"), "module.exports = (x) => -x;\n").unwrap();
    std::fs::write(dir.join("aver.toml"), &toml).unwrap();
    let stop = |n: u32| {
        hook(
            &d,
            &dir,
            "stop",
            serde_json::json!({"session_id": "w", "cwd": dir, "hook_event_name": "Stop", "stop_hook_active": n > 0, "last_assistant_message": format!("try {n}")}),
        )
    };
    for n in 0..5 {
        let out = stop(n);
        assert_eq!(out["decision"], "block", "stop {n}: {out}");
        assert!(out["reason"].as_str().unwrap().contains("strict"), "{out}");
    }
    let gave = aver_in(&d, &dir, &["giveup", "cannot finish"], "");
    assert!(gave.status.success());
    assert_eq!(stop(9), Value::Null, "aver giveup ends it");
}
