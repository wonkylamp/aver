//! The skills and instructions Aver gives agent tools name only commands
//! and flags that exist: every `aver …` they mention, in backticks or in a
//! skill's `allowed-tools`, is checked against `aver --help`.

use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;

fn help(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_aver"))
        .args(args)
        .arg("--help")
        .env_remove("AVER_HOME")
        .output()
        .unwrap();
    assert!(out.status.success(), "aver {} --help", args.join(" "));
    String::from_utf8(out.stdout).unwrap()
}

/// The subcommands a help text lists under `Commands:` and, in `aver
/// --help`, `Advanced:`.
fn subcommands(help: &str) -> Vec<String> {
    ["Commands:", "Advanced:"]
        .iter()
        .flat_map(|heading| {
            help.lines()
                .skip_while(move |l| l.trim() != *heading)
                .skip(1)
                .take_while(|l| !l.trim().is_empty())
                .filter_map(|l| l.split_whitespace().next())
        })
        .filter(|c| *c != "help")
        .map(str::to_owned)
        .collect()
}

/// Splits a command line into words, keeping a double-quoted string as one.
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                word.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// The plugin's skill and commands, which `aver init` also writes.
fn skill_files() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../integrations/claude-code");
    let mut files: Vec<PathBuf> = aver::integrate::plugin::SHARED
        .iter()
        .map(|f| root.join(f.plugin))
        .collect();
    files.sort();
    assert!(files.len() >= 5, "{files:?}");
    files
}

/// Every `aver …` command line a text mentions.
fn mentions(text: &str) -> Vec<String> {
    let inline = Regex::new(r"`(aver [^`]+)`").unwrap();
    let allowed = Regex::new(r"Bash\((aver [^:)]+)(:\*)?\)").unwrap();
    inline
        .captures_iter(text)
        .chain(allowed.captures_iter(text))
        .map(|c| c[1].to_owned())
        .collect()
}

/// Why `line` names something `aver` does not have, if it does.
fn problem(line: &str, top: &[String]) -> Option<String> {
    let w = words(line);
    let cmd = w.get(1)?;
    if !top.contains(cmd) {
        return Some(format!("`{line}`: aver has no command {cmd}"));
    }
    let mut path = vec![cmd.as_str()];
    let mut text = help(&path);
    let subs = subcommands(&text);
    if !subs.is_empty() {
        match w.get(2) {
            Some(sub) if subs.contains(sub) => {
                path.push(sub);
                text = help(&path);
            }
            other => {
                return Some(format!(
                    "`{line}`: aver {cmd} needs one of {subs:?}, not {other:?}"
                ))
            }
        }
    }
    for flag in w.iter().filter(|t| t.starts_with('-')) {
        let flag = flag.split('=').next().unwrap();
        let listed = Regex::new(&format!(r"(^|[\s,]){}([\s,=<]|$)", regex::escape(flag))).unwrap();
        if !text.lines().any(|l| listed.is_match(l)) {
            return Some(format!("`{line}`: aver {} has no {flag}", path.join(" ")));
        }
    }
    None
}

#[test]
fn skills_name_only_commands_and_flags_that_exist() {
    let top = subcommands(&help(&[]));
    assert!(top.contains(&"verify".to_owned()) && !top.contains(&"mcp".to_owned()));
    let mut checked = 0;
    let mut problems = Vec::new();
    for file in skill_files() {
        let text = std::fs::read_to_string(&file).unwrap();
        for line in mentions(&text) {
            checked += 1;
            if let Some(p) = problem(&line, &top) {
                problems.push(format!("{}: {p}", file.display()));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert!(checked >= 30, "only {checked} mentions found");
}

/// The plugin's hooks run `aver hook <name> --plugin` through a wrapper
/// that does nothing without `aver` and never fails, outside a workspace
/// or with an unknown hook (an older `aver`), so no session is blocked.
#[test]
fn plugin_hooks_never_block_outside_a_workspace() {
    let plugin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../integrations/claude-code");
    let hooks: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(plugin.join("hooks/hooks.json")).unwrap())
            .unwrap();
    let mut names = Vec::new();
    for (event, groups) in hooks["hooks"].as_object().unwrap() {
        for group in groups.as_array().unwrap() {
            for hook in group["hooks"].as_array().unwrap() {
                let command = hook["command"].as_str().unwrap();
                let name = command
                    .strip_prefix("sh \"${CLAUDE_PLUGIN_ROOT}/hooks/aver-hook.sh\" ")
                    .unwrap_or_else(|| panic!("{event}: {command}"));
                names.push(format!("{event} {name}"));
            }
        }
    }
    names.sort();
    assert_eq!(
        names,
        [
            "SessionStart session-start",
            "Stop stop",
            "SubagentStop stop",
        ]
    );

    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("a.js");
    let input = serde_json::json!({
        "cwd": outside.path(),
        "tool_name": "Edit",
        "tool_input": { "file_path": file, "command": "cargo test" },
    })
    .to_string();
    for (bin, name) in [
        (env!("CARGO_BIN_EXE_aver"), "stop"),
        (env!("CARGO_BIN_EXE_aver"), "pre-edit"),
        (env!("CARGO_BIN_EXE_aver"), "pre-agent"),
        (env!("CARGO_BIN_EXE_aver"), "pre-bash"),
        (env!("CARGO_BIN_EXE_aver"), "post-edit"),
        (env!("CARGO_BIN_EXE_aver"), "post-bash"),
        (env!("CARGO_BIN_EXE_aver"), "session-start"),
        (env!("CARGO_BIN_EXE_aver"), "no-such-hook"),
        ("/nonexistent/aver", "stop"),
    ] {
        let mut child = Command::new("sh")
            .arg(plugin.join("hooks/aver-hook.sh"))
            .arg(name)
            .current_dir(outside.path())
            .env("AVER_BIN", bin)
            .env("AVER_HOME", outside.path().join("home"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write as _;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{bin} {name}");
        assert!(out.stdout.is_empty(), "{bin} {name}: {:?}", out.stdout);
    }
}

/// The plugin's hooks find `aver` through scripts/aver.sh, which finds one
/// installed in ~/.local/bin under the bare PATH a GUI app on macOS gets,
/// and fails when there is none, so the hooks do nothing.
#[test]
fn plugin_finds_aver_under_a_gui_path() {
    let plugin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../integrations/claude-code");
    assert!(
        !plugin.join(".mcp.json").exists(),
        "the plugin starts no MCP server"
    );
    let script = plugin.join("scripts/aver.sh");
    let run = |home: &Path| {
        Command::new("sh")
            .arg(&script)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", home)
            .output()
            .unwrap()
    };

    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join(".local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("aver"), "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(
        bin.join("aver"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let out = run(home.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert_eq!(stdout, format!("{}\n", bin.join("aver").display()));

    let installed_elsewhere = ["/opt/homebrew/bin/aver", "/usr/local/bin/aver"]
        .iter()
        .any(|p| Path::new(p).exists());
    if !installed_elsewhere {
        let empty = tempfile::tempdir().unwrap();
        let out = run(empty.path());
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn the_check_catches_what_does_not_exist() {
    let top = subcommands(&help(&[]));
    assert!(problem("aver frobnicate", &top).is_some());
    assert!(problem("aver check --everything", &top).is_some());
    assert!(problem("aver workspace make", &top).is_some());
    assert!(problem("aver check <id>", &top).is_none());
    assert!(problem("aver lease <glob> --reason \"x y\" --shared", &top).is_none());
    assert!(problem("aver workspace new -c \"neg: negate\"", &top).is_none());
}
