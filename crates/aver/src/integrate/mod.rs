//! `aver integrate claude` (ARCHITECTURE.md §2.12): what Aver writes into a
//! project so Claude Code works with Aver, the same files `aver init` and
//! `aver setup` write. All of it is meant to be committed, so it holds no
//! path of this machine, and the user's own `~/.claude` is never written:
//!
//! - `.claude/settings.json`: Aver's hooks ([`HOOKS`]) merged into what the
//!   file holds, each calling `aver` from `PATH` and doing nothing where it
//!   is missing ([`settings_command`]).
//! - `.claude/skills/aver/SKILL.md` and `.claude/commands/aver*.md`: the
//!   Claude Code plugin's skill and commands, byte for byte
//!   ([`plugin::SHARED`]).
//!
//! The hooks:
//! - `WorktreeCreate` and `WorktreeRemove`: `claude --worktree`, subagents
//!   with worktree isolation and background sessions get an Aver workspace
//!   directory instead of a `git worktree` (an attempt at a regression-only
//!   task, so `aver verify` works in it).
//! - `SessionStart`: the short brief for a session in a workspace (done
//!   means `aver verify` passes), and in the repository's own checkout the
//!   instruction to make one with `aver workspace new` before editing.
//! - `Stop` and `SubagentStop`: the verify gate, which blocks a stop when
//!   the workspace changed files no `PASS` or `REVIEW` receipt covers.
//!
//! Setting up again takes out every Aver hook the plan no longer holds: an
//! older rev's, those Aver before 0.4 installed ([`LEGACY_HOOK_NAMES`]),
//! and those Aver 0.4 wrote into `.claude/settings.local.json` with this
//! machine's paths ([`LOCAL_SETTINGS`]). Aver before 0.4 also registered an
//! MCP server (`aver mcp`) in `.mcp.json`; setting up again takes that
//! entry out ([`legacy_mcp_write`]).
//!
//! [`tools`] does the same for the other agent tools, with the shared
//! instructions of [`text`].

pub mod installed;
pub mod plugin;
pub mod remove;
pub mod text;
pub mod tools;

use std::path::Path;

use serde_json::{json, Map, Value};

/// Claude Code's project settings, committed with the repository.
pub const SETTINGS: &str = ".claude/settings.json";
/// Claude Code's project-local settings, which it keeps out of Git. Aver
/// 0.4 wrote its hooks here; setting up again moves them to [`SETTINGS`].
pub const LOCAL_SETTINGS: &str = ".claude/settings.local.json";
/// The project's MCP servers. Aver no longer registers one; the worktree
/// hook carries the user's file into workspaces.
pub const MCP: &str = ".mcp.json";
/// The name Aver's MCP server had in the MCP configurations it wrote
/// before 0.4.
pub const SERVER: &str = "aver";
/// The revision of what `aver integrate` writes; bumped whenever the hooks
/// or settings it writes change, so a stale integration can be detected.
pub const INTEGRATION_REV: u32 = 4;

/// One file the integration writes: its contents before (if any) and after.
/// An empty `after` deletes the file: one that held only Aver's old
/// entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite {
    /// Relative to the repository root.
    pub path: String,
    pub before: Option<String>,
    pub after: String,
}

impl FileWrite {
    pub fn changes(&self) -> bool {
        self.before.as_deref() != Some(self.after.as_str())
    }

    pub fn deletes(&self) -> bool {
        self.after.is_empty() && self.before.is_some()
    }
}

/// The write that takes the `aver` server Aver registered before 0.4 out of
/// the MCP configuration `rel` in `root`, if it holds one. A file Aver
/// cannot parse (JSON with comments, which VS Code allows) is not one Aver
/// wrote, so it is left alone.
pub fn legacy_mcp_write(root: &Path, rel: &str) -> Option<FileWrite> {
    let before = std::fs::read_to_string(root.join(rel)).ok()?;
    let after = remove::strip(rel, &before).ok()?.unwrap_or_default();
    (after != before).then(|| FileWrite {
        path: rel.to_owned(),
        before: Some(before),
        after,
    })
}

/// One hook Aver installs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookSpec {
    /// Claude Code's event.
    pub event: &'static str,
    /// The name `aver hook <name>` takes.
    pub name: &'static str,
    /// Seconds Claude Code waits for it.
    pub timeout: u32,
    /// A session hook, which the plugin installs too. The worktree hooks
    /// take over `claude --worktree`, so only a repository set up with Aver
    /// has them.
    pub session: bool,
}

/// Every hook Aver installs: all of them in a repository's settings, the
/// session hooks in the plugin. `Stop` waits as long as a verify may take,
/// since in an `aver run` workspace it runs one.
pub const HOOKS: [HookSpec; 5] = [
    HookSpec {
        event: "SessionStart",
        name: "session-start",
        timeout: 30,
        session: true,
    },
    HookSpec {
        event: "Stop",
        name: "stop",
        timeout: 1800,
        session: true,
    },
    HookSpec {
        event: "SubagentStop",
        name: "stop",
        timeout: 120,
        session: true,
    },
    HookSpec {
        event: "WorktreeCreate",
        name: "worktree-create",
        timeout: 600,
        session: false,
    },
    HookSpec {
        event: "WorktreeRemove",
        name: "worktree-remove",
        timeout: 120,
        session: false,
    },
];

/// The hooks [`hooks`] installs, by the name `aver hook <name>` takes.
pub const HOOK_NAMES: [&str; 4] = [
    "session-start",
    "stop",
    "worktree-create",
    "worktree-remove",
];

/// The hooks Aver installed before 0.4. `aver hook` exits 0 for them, and
/// setting up again or `aver setup --remove` takes them out.
pub const LEGACY_HOOK_NAMES: [&str; 5] = [
    "pre-edit",
    "pre-agent",
    "pre-bash",
    "post-edit",
    "post-bash",
];

/// An Aver hook command as settings hold it: `aver hook <name>[ --rev N]`,
/// alone (`[AVER_HOME=… ]<…/aver> hook <name>`, as Aver before 0.4.1 wrote
/// it) or inside the shell [`settings_command`] wraps it in. `rev` is
/// `None` for a command written before integrations carried a rev.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookCommand<'a> {
    pub name: &'a str,
    pub rev: Option<u32>,
}

/// Parses `command` as one of Aver's hook commands.
pub fn parse_hook(command: &str) -> Option<HookCommand<'static>> {
    let command = command.trim_end();
    command.match_indices(" hook ").find_map(|(at, sep)| {
        let bin = command[..at].rsplit([' ', '/', ';', '&', '|']).next()?;
        if bin.trim_matches(['\'', '"']) != "aver" {
            return None;
        }
        let rest = &command[at + sep.len()..];
        let len = rest
            .find(|c: char| !(c.is_ascii_lowercase() || c == '-'))
            .unwrap_or(rest.len());
        let name = *HOOK_NAMES
            .iter()
            .chain(&LEGACY_HOOK_NAMES)
            .find(|n| **n == &rest[..len])?;
        let mut tail = &rest[len..];
        let mut rev = None;
        if let Some(r) = tail.strip_prefix(" --rev ") {
            let digits = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
            rev = Some(r[..digits].parse::<u32>().ok()?);
            tail = &r[digits..];
        }
        (tail.is_empty() || tail.starts_with([' ', ';'])).then_some(HookCommand { name, rev })
    })
}

/// What Claude Code does for `claude --worktree` without a WorktreeCreate
/// hook, for a machine without `aver`: a Git worktree at
/// `.claude/worktrees/<name>` on the branch `worktree-<name>`, its path the
/// last line of stdout.
const WORKTREE_CREATE_WITHOUT_AVER: &str = r#"n=$(sed -n 's/.*"name":"\([^"]*\)".*/\1/p' | tr -cd 'A-Za-z0-9._-'); d="$(git rev-parse --show-toplevel)/.claude/worktrees/$n"; git worktree add -q -b "worktree-$n" "$d" HEAD >&2 && printf '%s\n' "$d""#;
/// And its removal.
const WORKTREE_REMOVE_WITHOUT_AVER: &str =
    r#"p=$(sed -n 's/.*"worktree_path":"\([^"]*\)".*/\1/p'); git worktree remove "$p" >&2"#;

/// The command hook `name` runs from a repository's settings: `aver` from
/// `PATH`, so the committed file holds no path of this machine, and nothing
/// where `aver` is missing, so a teammate without it sees no hook error.
/// `|| true` cannot hide a Stop block: `aver hook stop` blocks with JSON on
/// stdout and exit code 0. The worktree hooks must print a directory, so
/// without `aver` they make a plain Git worktree, as Claude Code would.
pub fn settings_command(name: &str) -> String {
    let call = format!("aver hook {name} --rev {INTEGRATION_REV}");
    let found = "command -v aver >/dev/null 2>&1";
    match name {
        "worktree-create" => {
            format!("if {found}; then {call}; else {WORKTREE_CREATE_WITHOUT_AVER}; fi")
        }
        "worktree-remove" => {
            format!("if {found}; then {call}; else {WORKTREE_REMOVE_WITHOUT_AVER}; fi")
        }
        _ => format!("{found} && {call} || true"),
    }
}

/// Every hook the integration installs in a repository's settings, by
/// event.
pub fn hooks() -> Value {
    let mut out = Map::new();
    for h in HOOKS {
        out.insert(
            h.event.to_owned(),
            json!([{ "hooks": [{
                "type": "command",
                "command": settings_command(h.name),
                "timeout": h.timeout,
            }] }]),
        );
    }
    Value::Object(out)
}

fn parse_object(path: &str, text: Option<&str>) -> Result<Map<String, Value>, String> {
    match text.map(str::trim).filter(|t| !t.is_empty()) {
        None => Ok(Map::new()),
        Some(t) => match serde_json::from_str::<Value>(t) {
            Ok(Value::Object(map)) => Ok(map),
            Ok(_) => Err(format!(
                "{path} is not a JSON object; fix or remove it first"
            )),
            Err(e) => Err(format!(
                "{path} is not valid JSON ({e}); fix or remove it first"
            )),
        },
    }
}

fn pretty(v: &Value) -> String {
    let mut text = serde_json::to_string_pretty(v).expect("JSON values serialize");
    text.push('\n');
    text
}

fn commands_of(group: &Value) -> Vec<&str> {
    group["hooks"]
        .as_array()
        .map(|hs| hs.iter().filter_map(|h| h["command"].as_str()).collect())
        .unwrap_or_default()
}

/// Removes from `list` (one event's matcher groups) every Aver hook that
/// `add` (the event's planned groups, if any) does not hold: an older rev's
/// or binary's, a hook Aver no longer has, a hook on an event it no longer
/// uses. A group left without hooks goes too; others' hooks stay.
fn drop_unplanned_hooks(list: &mut Vec<Value>, add: Option<&Value>) {
    let adding: Vec<&str> = add
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(commands_of)
        .collect();
    let unplanned = |command: &str| !adding.contains(&command) && parse_hook(command).is_some();
    list.retain_mut(|group| {
        let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = hooks.len();
        hooks.retain(|h| !h["command"].as_str().is_some_and(unplanned));
        before == 0 || !hooks.is_empty()
    });
}

/// `existing` settings with `add` (hooks by event) merged in: a hook whose
/// command is already configured for the event is not added again, and
/// everything else in the file is kept but the approval of Aver's old
/// `.mcp.json` server (`enabledMcpjsonServers`).
pub fn merge_settings(existing: Option<&str>, add: &Value) -> Result<String, String> {
    let mut root = parse_object(SETTINGS, existing)?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(hooks) = hooks else {
        return Err(format!("{SETTINGS}: \"hooks\" is not an object"));
    };
    let add = add.as_object().expect("hooks by event");
    let mut emptied = Vec::new();
    for (event, list) in hooks.iter_mut() {
        if let Value::Array(list) = list {
            let had = list.len();
            drop_unplanned_hooks(list, add.get(event));
            if had > 0 && list.is_empty() && !add.contains_key(event) {
                emptied.push(event.clone());
            }
        }
    }
    for event in emptied {
        hooks.remove(&event);
    }
    for (event, groups) in add {
        let list = hooks
            .entry(event.clone())
            .or_insert_with(|| Value::Array(Vec::new()));
        let Value::Array(list) = list else {
            return Err(format!("{SETTINGS}: hooks.{event} is not an array"));
        };
        for group in groups.as_array().expect("matcher groups") {
            let present = list.iter().any(|g| {
                commands_of(g)
                    .iter()
                    .any(|c| commands_of(group).contains(c))
            });
            if !present {
                list.push(group.clone());
            }
        }
    }
    if let Some(Value::Array(enabled)) = root.get_mut("enabledMcpjsonServers") {
        enabled.retain(|v| v != SERVER);
        if enabled.is_empty() {
            root.remove("enabledMcpjsonServers");
        }
    }
    Ok(pretty(&Value::Object(root)))
}

/// The writes `aver integrate claude` makes in the repository at `root`:
/// its hooks in the settings, the plugin's skill and commands, Aver 0.4's
/// hooks out of the local settings, and with `legacy_mcp` Aver's old
/// server out of `.mcp.json`.
pub fn plan(root: &Path, legacy_mcp: bool) -> Result<Vec<FileWrite>, String> {
    let read = |rel: &str| std::fs::read_to_string(root.join(rel)).ok();
    let before = read(SETTINGS);
    let mut out = vec![FileWrite {
        path: SETTINGS.into(),
        after: merge_settings(before.as_deref(), &hooks())?,
        before,
    }];
    if let Some(local) = read(LOCAL_SETTINGS) {
        let after = remove::strip(LOCAL_SETTINGS, &local)?.unwrap_or_default();
        if after != local {
            out.push(FileWrite {
                path: LOCAL_SETTINGS.into(),
                before: Some(local),
                after,
            });
        }
    }
    for f in plugin::SHARED {
        out.push(FileWrite {
            path: f.project.into(),
            before: read(f.project),
            after: f.text.into(),
        });
    }
    if legacy_mcp {
        out.extend(legacy_mcp_write(root, MCP));
    }
    Ok(out)
}

/// The plan as the user sees it before confirming: each file and exactly
/// what it will hold.
pub fn describe(writes: &[FileWrite]) -> String {
    let mut out = String::new();
    for w in writes {
        out.push_str(&format!("--- {} ({})\n{}", w.path, what(w), w.after));
    }
    out
}

/// What `w` does to its file, in a few words.
pub fn what(w: &FileWrite) -> &'static str {
    match &w.before {
        _ if !w.changes() => "unchanged",
        _ if w.deletes() => "deleted: it held only Aver's old entries",
        None => "new file",
        Some(_) if plugin::SHARED.iter().any(|f| f.project == w.path) => {
            "replaced with this version of Aver's"
        }
        Some(_) if w.path == LOCAL_SETTINGS => "updated; only Aver's old hooks go",
        Some(_) if w.path.ends_with(".md") || w.path.ends_with(".mdc") => {
            "updated; only Aver's block changes"
        }
        Some(_) => "updated; existing settings kept",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_keep_what_is_there_and_never_duplicate_hooks() {
        let existing = r#"{"permissions":{"allow":["Bash(make:*)"]},"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"lint.sh"}]}]},"enabledMcpjsonServers":["aver","db"]}"#;
        let once = merge_settings(Some(existing), &hooks()).unwrap();
        let v: Value = serde_json::from_str(&once).unwrap();
        assert_eq!(v["permissions"]["allow"][0], "Bash(make:*)");
        assert_eq!(
            v["hooks"]["PreToolUse"],
            json!([{"matcher":"Bash","hooks":[{"type":"command","command":"lint.sh"}]}]),
            "the user's own hook stays and Aver adds none on tool calls"
        );
        let events: Vec<&String> = v["hooks"].as_object().unwrap().keys().collect();
        assert_eq!(
            events,
            [
                "PreToolUse",
                "SessionStart",
                "Stop",
                "SubagentStop",
                "WorktreeCreate",
                "WorktreeRemove"
            ]
        );
        assert_eq!(
            v["hooks"]["Stop"][0]["hooks"][0]["command"],
            "command -v aver >/dev/null 2>&1 && aver hook stop --rev 4 || true"
        );
        assert!(!once.contains("AVER_HOME"), "no path of this machine");
        assert_eq!(
            v["enabledMcpjsonServers"],
            json!(["db"]),
            "the approval of Aver's old MCP server goes"
        );
        let twice = merge_settings(Some(&once), &hooks()).unwrap();
        assert_eq!(once, twice, "running it again changes nothing");
        assert!(merge_settings(Some("[1]"), &hooks()).is_err());
        assert!(merge_settings(Some("{oops"), &hooks()).is_err());
    }

    #[test]
    fn hook_commands_parse_alone_and_wrapped() {
        let parsed = parse_hook("AVER_HOME=/h '/my tools/aver' hook stop --rev 3").unwrap();
        assert_eq!((parsed.name, parsed.rev), ("stop", Some(3)));
        let old = parse_hook("/opt/aver/aver hook post-edit").unwrap();
        assert_eq!(
            (old.name, old.rev),
            ("post-edit", None),
            "a hook before 0.4"
        );
        for h in HOOKS {
            let parsed = parse_hook(&settings_command(h.name)).unwrap();
            assert_eq!((parsed.name, parsed.rev), (h.name, Some(INTEGRATION_REV)));
        }
        assert!(parse_hook("/opt/aver/aver hook stopper").is_none());
        assert!(parse_hook("/opt/never hook stop --rev 1").is_none());
        assert!(parse_hook("/opt/aver/aver hook stop --rev x").is_none());
        assert!(parse_hook("sh \"$P/hooks/aver-hook.sh\" stop").is_none());
    }

    #[test]
    fn integrating_again_replaces_an_older_revs_hooks_and_keeps_others() {
        let old = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"AVER_HOME=/h /old/aver hook stop"},{"type":"command","command":"notify.sh"}]}],"PostToolUse":[{"matcher":"Edit","hooks":[{"type":"command","command":"/old/aver hook post-edit"},{"type":"command","command":"prettier --write"}]},{"matcher":"Bash","hooks":[{"type":"command","command":"AVER_HOME=/h /old/aver hook post-bash --rev 2"}]}],"PreToolUse":[{"matcher":"Agent|Task","hooks":[{"type":"command","command":"/old/aver hook pre-agent --rev 2"}]}],"SubagentStart":[{"hooks":[{"type":"command","command":"AVER_HOME=/h /old/aver hook session-start --rev 2"}]}]}}"#;
        let new = merge_settings(Some(old), &hooks()).unwrap();
        let v: Value = serde_json::from_str(&new).unwrap();
        let stop: Vec<&str> = v["hooks"]["Stop"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(commands_of)
            .collect();
        assert_eq!(
            stop,
            [
                "notify.sh",
                "command -v aver >/dev/null 2>&1 && aver hook stop --rev 4 || true"
            ]
        );
        assert_eq!(
            v["hooks"]["PostToolUse"],
            json!([{"matcher":"Edit","hooks":[{"type":"command","command":"prettier --write"}]}]),
            "the hooks before 0.4 go, a user's hook beside them stays"
        );
        assert!(v["hooks"].get("PreToolUse").is_none(), "{new}");
        assert!(v["hooks"].get("SubagentStart").is_none(), "{new}");
    }

    #[test]
    fn setting_up_moves_avers_local_hooks_and_leaves_others() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        let local = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"AVER_HOME=/h /b/aver hook stop --rev 3"},{"type":"command","command":"notify.sh"}]}],"SessionStart":[{"hooks":[{"type":"command","command":"AVER_HOME=/h /b/aver hook session-start --rev 3"}]}]},"permissions":{"allow":["Bash(ls)"]}}"#;
        std::fs::write(dir.path().join(LOCAL_SETTINGS), local).unwrap();
        let writes = plan(dir.path(), false).unwrap();
        let local = writes.iter().find(|w| w.path == LOCAL_SETTINGS).unwrap();
        let v: Value = serde_json::from_str(&local.after).unwrap();
        assert_eq!(
            v,
            json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":"notify.sh"}]}]},"permissions":{"allow":["Bash(ls)"]}})
        );
        for f in plugin::SHARED {
            let w = writes.iter().find(|w| w.path == f.project).unwrap();
            assert_eq!(w.after, f.text, "{} is the plugin's file", f.project);
        }

        let only_ours = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"/b/aver hook stop --rev 3"}]}]}}"#;
        std::fs::write(dir.path().join(LOCAL_SETTINGS), only_ours).unwrap();
        let writes = plan(dir.path(), false).unwrap();
        let local = writes.iter().find(|w| w.path == LOCAL_SETTINGS).unwrap();
        assert!(local.deletes(), "a file of Aver's hooks alone goes");
    }

    #[test]
    fn setting_up_again_takes_avers_old_mcp_server_out_and_keeps_others() {
        let dir = tempfile::tempdir().unwrap();
        let theirs = r#"{"mcpServers":{"aver":{"command":"aver","args":["mcp"]},"db":{"command":"db-mcp"}}}"#;
        std::fs::write(dir.path().join(MCP), theirs).unwrap();
        let w = legacy_mcp_write(dir.path(), MCP).unwrap();
        let v: Value = serde_json::from_str(&w.after).unwrap();
        assert_eq!(v, json!({"mcpServers":{"db":{"command":"db-mcp"}}}));

        std::fs::write(dir.path().join(MCP), r#"{"mcpServers":{"aver":{}}}"#).unwrap();
        let w = legacy_mcp_write(dir.path(), MCP).unwrap();
        assert!(w.deletes(), "a file that held only Aver's server goes");

        std::fs::write(dir.path().join(MCP), r#"{"mcpServers":{"db":{}}}"#).unwrap();
        assert_eq!(legacy_mcp_write(dir.path(), MCP), None);
        assert_eq!(legacy_mcp_write(dir.path(), ".cursor/mcp.json"), None);
        std::fs::write(
            dir.path().join(MCP),
            "// mine\n{\"mcpServers\":{\"aver\":{}}}",
        )
        .unwrap();
        assert_eq!(
            legacy_mcp_write(dir.path(), MCP),
            None,
            "JSON with comments is not Aver's"
        );
    }
}
