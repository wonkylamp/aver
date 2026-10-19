//! `aver integrate cursor|codex|gemini|vscode|agents-md`: Aver's
//! instructions in the project-local file each tool reads. Each tool's
//! format, from its own documentation (checked 2026-10):
//!
//! - Cursor: project rules are `.cursor/rules/*.mdc` with `description`,
//!   `globs` and `alwaysApply` frontmatter, and a rule with a description
//!   and `alwaysApply: false` is pulled in when the agent finds it relevant
//!   (<https://cursor.com/docs/context/rules>).
//! - Codex CLI: instructions in `AGENTS.md`
//!   (<https://learn.chatgpt.com/docs/agent-configuration/agents-md>).
//! - Gemini CLI: context in `GEMINI.md`
//!   (<https://geminicli.com/docs/cli/gemini-md/>).
//! - VS Code (Copilot): instructions in `.github/copilot-instructions.md`
//!   (<https://code.visualstudio.com/docs/copilot/customization/custom-instructions>).
//! - `AGENTS.md`: plain Markdown with no required structure
//!   (<https://agents.md/>), read by Codex, Cursor, VS Code, Gemini CLI
//!   (when configured) and others.
//!
//! Markdown gets one marked block (§2.12) that running it again replaces.
//! Aver before 0.4 also wrote an MCP server entry into each tool's
//! configuration ([`Tool::legacy_mcp_config`]); setting up again takes it
//! out.

use std::path::Path;

use super::text;
use super::FileWrite;

/// An agent tool `aver integrate` sets up, besides Claude Code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Cursor,
    Codex,
    Gemini,
    Vscode,
    AgentsMd,
}

impl Tool {
    pub const ALL: [Tool; 5] = [
        Tool::Cursor,
        Tool::Codex,
        Tool::Gemini,
        Tool::Vscode,
        Tool::AgentsMd,
    ];

    /// The name `aver integrate` takes.
    pub fn name(self) -> &'static str {
        match self {
            Tool::Cursor => "cursor",
            Tool::Codex => "codex",
            Tool::Gemini => "gemini",
            Tool::Vscode => "vscode",
            Tool::AgentsMd => "agents-md",
        }
    }

    /// The MCP configuration Aver before 0.4 registered its server in.
    pub fn legacy_mcp_config(self) -> Option<&'static str> {
        match self {
            Tool::Cursor => Some(".cursor/mcp.json"),
            Tool::Codex => Some(".codex/config.toml"),
            Tool::Gemini => Some(".gemini/settings.json"),
            Tool::Vscode => Some(".vscode/mcp.json"),
            Tool::AgentsMd => None,
        }
    }

    /// The file that carries the instructions.
    pub fn instructions_file(self) -> &'static str {
        match self {
            Tool::Cursor => ".cursor/rules/aver.mdc",
            Tool::Codex | Tool::AgentsMd => "AGENTS.md",
            Tool::Gemini => "GEMINI.md",
            Tool::Vscode => ".github/copilot-instructions.md",
        }
    }
}

/// The Cursor rule file for a repository that has none.
fn cursor_rule() -> String {
    format!(
        "---\ndescription: {}\nalwaysApply: false\n---\n\n{}",
        serde_json::to_string(text::description()).expect("strings serialize"),
        text::block()
    )
}

/// The instructions file of `tool` with Aver's block in it.
pub fn merge_instructions(tool: Tool, existing: Option<&str>) -> Result<String, String> {
    let path = tool.instructions_file();
    match (tool, existing.filter(|t| !t.trim().is_empty())) {
        (Tool::Cursor, None) => Ok(cursor_rule()),
        (_, existing) => text::upsert(path, existing, text::BEGIN, text::END, &text::block()),
    }
}

/// The writes `aver integrate <tool>` makes in the repository at `root`:
/// the instructions, and with `legacy_mcp` Aver's old server out of the
/// tool's MCP configuration.
pub fn plan(tool: Tool, root: &Path, legacy_mcp: bool) -> Result<Vec<FileWrite>, String> {
    let mut out = Vec::new();
    if let Some(config) = tool.legacy_mcp_config().filter(|_| legacy_mcp) {
        out.extend(super::legacy_mcp_write(root, config));
    }
    let path = tool.instructions_file();
    let before = std::fs::read_to_string(root.join(path)).ok();
    out.push(FileWrite {
        path: path.into(),
        after: merge_instructions(tool, before.as_deref())?,
        before,
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_up_again_takes_the_old_codex_server_block_out() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codex")).unwrap();
        let old = format!(
            "model = \"o4\"\n\n{} written by `aver integrate`\n[mcp_servers.aver]\ncommand = \"aver\"\nargs = [\"mcp\"]\n{}\n",
            text::TOML_BEGIN,
            text::TOML_END
        );
        std::fs::write(dir.path().join(".codex/config.toml"), old).unwrap();
        let writes = plan(Tool::Codex, dir.path(), true).unwrap();
        assert_eq!(writes[0].path, ".codex/config.toml");
        assert_eq!(writes[0].after.trim(), "model = \"o4\"");
        assert_eq!(writes[1].path, "AGENTS.md");
    }

    #[test]
    fn every_tool_names_its_files() {
        for tool in Tool::ALL {
            assert!(!tool.instructions_file().starts_with('/'));
        }
    }
}
