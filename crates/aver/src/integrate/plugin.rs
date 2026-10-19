//! The Claude Code plugin (`integrations/claude-code`) as the one source of
//! what Claude Code gets (ARCHITECTURE.md §2.12). Its skill and commands
//! are compiled in here and written byte for byte into a repository by
//! `aver integrate claude` (and so `aver init` and `aver setup`); its
//! `hooks/hooks.json` is rendered from [`super::HOOKS`], the table the
//! repository's settings hooks come from too. `make plugin-check` checks
//! the files on disk against these.

/// A plugin file a repository gets too: its path in the plugin, its path
/// in the repository, and its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shared {
    pub plugin: &'static str,
    pub project: &'static str,
    pub text: &'static str,
}

/// The plugin's root, from this crate.
pub const ROOT: &str = "../../integrations/claude-code";

/// The skill and the commands, in the plugin and in a repository.
pub const SHARED: [Shared; 6] = [
    Shared {
        plugin: "skills/aver/SKILL.md",
        project: ".claude/skills/aver/SKILL.md",
        text: include_str!("../../../../integrations/claude-code/skills/aver/SKILL.md"),
    },
    Shared {
        plugin: "commands/aver.md",
        project: ".claude/commands/aver.md",
        text: include_str!("../../../../integrations/claude-code/commands/aver.md"),
    },
    Shared {
        plugin: "commands/aver-check.md",
        project: ".claude/commands/aver-check.md",
        text: include_str!("../../../../integrations/claude-code/commands/aver-check.md"),
    },
    Shared {
        plugin: "commands/aver-verify.md",
        project: ".claude/commands/aver-verify.md",
        text: include_str!("../../../../integrations/claude-code/commands/aver-verify.md"),
    },
    Shared {
        plugin: "commands/aver-status.md",
        project: ".claude/commands/aver-status.md",
        text: include_str!("../../../../integrations/claude-code/commands/aver-status.md"),
    },
    Shared {
        plugin: "commands/aver-run.md",
        project: ".claude/commands/aver-run.md",
        text: include_str!("../../../../integrations/claude-code/commands/aver-run.md"),
    },
];

/// The plugin's manifest.
pub const MANIFEST: &str =
    include_str!("../../../../integrations/claude-code/.claude-plugin/plugin.json");
pub const MANIFEST_PATH: &str = ".claude-plugin/plugin.json";
pub const HOOKS_PATH: &str = "hooks/hooks.json";

/// What each plugin hook runs: its script, which calls `aver hook <name>
/// --plugin` when `aver` exists and otherwise tells a starting session to
/// run `/aver`.
pub fn command(name: &str) -> String {
    format!("sh \"${{CLAUDE_PLUGIN_ROOT}}/hooks/aver-hook.sh\" {name}")
}

/// `hooks/hooks.json` as it must be: the session hooks of [`super::HOOKS`].
/// The worktree hooks stay out: they take over `claude --worktree` and so
/// belong to the repositories set up with Aver, not to every repository
/// of a user who installed the plugin.
pub fn hooks_json() -> String {
    let mut hooks = serde_json::Map::new();
    for h in super::HOOKS.iter().filter(|h| h.session) {
        hooks.insert(
            h.event.to_owned(),
            serde_json::json!([{ "hooks": [{
                "type": "command",
                "command": command(h.name),
                "timeout": h.timeout,
            }] }]),
        );
    }
    super::pretty(&serde_json::json!({
        "description": "Aver's session hooks: the short brief when a session starts in an Aver workspace (or a pointer to one in a repository set up with Aver), and the verify gate when an agent stops with unverified changes. Without the aver CLI, a starting session is told to run /aver.",
        "hooks": hooks,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shared_file_has_frontmatter_with_a_description() {
        for f in SHARED {
            let rest = f.text.strip_prefix("---\n").expect(f.plugin);
            let (front, body) = rest.split_once("\n---\n").expect(f.plugin);
            assert!(
                front.lines().any(|l| l.starts_with("description: ")),
                "{}",
                f.plugin
            );
            assert!(!body.trim().is_empty(), "{}", f.plugin);
        }
        let manifest: serde_json::Value = serde_json::from_str(MANIFEST).unwrap();
        assert_eq!(manifest["name"], "aver");
    }
}
