//! Golden files for every `aver integrate` target: what it writes into an
//! empty repository and into one that already holds the tool's files, and
//! that running it again on its own output changes nothing.
//!
//! The goldens live in `tests/golden/integrate/<target>-{fresh,merge}.txt`,
//! with the shared instructions (the `aver` skill's body) shown as
//! `{{AVER_INSTRUCTIONS}}` and the plugin's files Claude Code gets as
//! `{{PLUGIN <path>}}`. After an intentional change, regenerate them
//! with `AVER_UPDATE_GOLDEN=1 cargo test -p aver --test integrate_golden`
//! and review the diff.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aver::integrate::tools::{self, Tool};
use aver::integrate::{self, remove, text, FileWrite};

#[derive(Clone, Copy)]
enum Target {
    Claude,
    Tool(Tool),
}

impl Target {
    fn name(self) -> &'static str {
        match self {
            Target::Claude => "claude",
            Target::Tool(t) => t.name(),
        }
    }

    fn plan(self, root: &Path) -> Vec<FileWrite> {
        match self {
            Target::Claude => integrate::plan(root, true),
            Target::Tool(t) => tools::plan(t, root, true),
        }
        .unwrap()
    }

    /// Files the repository holds before, for the merge case: the tool's
    /// own settings and someone's instructions, sometimes with a stale
    /// Aver block. The MCP configurations hold others' servers only, so
    /// setting up leaves them as they are.
    fn seed(self) -> Vec<(&'static str, &'static str)> {
        match self {
            Target::Claude => vec![(
                ".claude/settings.json",
                r#"{"permissions":{"allow":["Bash(make:*)"]},"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"lint.sh"}]}]}}"#,
            )],
            Target::Tool(Tool::Cursor) => vec![(
                ".cursor/rules/aver.mdc",
                "---\ndescription: Team notes on Aver\nalwaysApply: true\n---\n\nAsk before accepting a run.\n",
            )],
            Target::Tool(Tool::Codex) => vec![(
                "AGENTS.md",
                "# Agents\n\nRun `make test`.\n\n<!-- BEGIN aver: an older version -->\nOld instructions.\n<!-- END aver -->\n\n## Style\n\nTabs.\n",
            )],
            Target::Tool(Tool::Gemini) => vec![("GEMINI.md", "# Project\n\nUse pnpm.")],
            Target::Tool(Tool::Vscode) => vec![(
                ".github/copilot-instructions.md",
                "Prefer small pull requests.\n",
            )],
            Target::Tool(Tool::AgentsMd) => vec![("AGENTS.md", "# Agents\n\nRun `make test`.\n")],
        }
    }

    /// The tool's MCP configuration as Aver before 0.4 left it: its `aver`
    /// server beside someone else's, and the same without Aver's.
    fn legacy_mcp(self) -> Option<(&'static str, &'static str, &'static str)> {
        Some(match self {
            Target::Claude => (
                ".mcp.json",
                r#"{"mcpServers":{"aver":{"command":"/opt/aver/bin/aver","args":["mcp"]},"db":{"command":"db-mcp"}}}"#,
                r#"{"mcpServers":{"db":{"command":"db-mcp"}}}"#,
            ),
            Target::Tool(Tool::Cursor) => (
                ".cursor/mcp.json",
                r#"{"mcpServers":{"aver":{"type":"stdio","command":"aver"},"db":{"command":"db-mcp","args":["--port","5432"]}}}"#,
                r#"{"mcpServers":{"db":{"command":"db-mcp","args":["--port","5432"]}}}"#,
            ),
            Target::Tool(Tool::Codex) => (
                ".codex/config.toml",
                "# Team defaults\nmodel = \"o4\"\n\n[mcp_servers.db]\ncommand = \"db-mcp\"\n\n# BEGIN aver: written by `aver integrate`; running it again replaces this block\n[mcp_servers.aver]\ncommand = \"aver\"\nargs = [\"mcp\"]\n# END aver\n",
                "# Team defaults\nmodel = \"o4\"\n\n[mcp_servers.db]\ncommand = \"db-mcp\"\n",
            ),
            Target::Tool(Tool::Gemini) => (
                ".gemini/settings.json",
                r#"{"theme":"GitHub","mcpServers":{"aver":{"command":"aver"},"db":{"command":"db-mcp"}}}"#,
                r#"{"theme":"GitHub","mcpServers":{"db":{"command":"db-mcp"}}}"#,
            ),
            Target::Tool(Tool::Vscode) => (
                ".vscode/mcp.json",
                r#"{"servers":{"aver":{"type":"stdio","command":"aver"},"github":{"type":"http","url":"https://api.githubcopilot.com/mcp/"}}}"#,
                r#"{"servers":{"github":{"type":"http","url":"https://api.githubcopilot.com/mcp/"}}}"#,
            ),
            Target::Tool(Tool::AgentsMd) => return None,
        })
    }
}

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/integrate")
        .join(format!("{name}.txt"))
}

/// The writes as one text, the shared instructions abbreviated.
fn render(writes: &[FileWrite]) -> String {
    let description = serde_json::to_string(text::description()).unwrap();
    let mut out = String::new();
    for w in writes {
        out.push_str(&format!("=== {}\n", w.path));
        if let Some(f) = integrate::plugin::SHARED
            .iter()
            .find(|f| f.project == w.path && f.text == w.after)
        {
            out.push_str(&format!("{{{{PLUGIN {}}}}}\n", f.plugin));
            continue;
        }
        out.push_str(
            &w.after
                .replace(text::instructions(), "{{AVER_INSTRUCTIONS}}")
                .replace(&description, "{{AVER_DESCRIPTION}}"),
        );
    }
    out
}

fn check(name: &str, actual: &str) {
    let path = golden_path(name);
    if std::env::var_os("AVER_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "{} is missing; run with AVER_UPDATE_GOLDEN=1",
            path.display()
        )
    });
    assert!(
        expected == actual,
        "{name} differs from {}; if the change is intended, rerun with AVER_UPDATE_GOLDEN=1 and review the diff.\n--- expected\n{expected}\n--- actual\n{actual}",
        path.display()
    );
}

fn apply(root: &Path, writes: &[FileWrite]) {
    for w in writes {
        let path = root.join(&w.path);
        if w.deletes() {
            std::fs::remove_file(path).unwrap();
            continue;
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, &w.after).unwrap();
    }
}

fn golden(target: Target) {
    for (case, seed) in [("fresh", Vec::new()), ("merge", target.seed())] {
        let dir = tempfile::tempdir().unwrap();
        for (rel, content) in &seed {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        let writes = target.plan(dir.path());
        assert!(writes.iter().all(FileWrite::changes), "{case}");
        for w in &writes {
            let seeded = seed.iter().find(|(rel, _)| *rel == w.path).map(|s| s.1);
            assert_eq!(w.before.as_deref(), seeded, "{} {case}", w.path);
        }
        check(&format!("{}-{case}", target.name()), &render(&writes));
        apply(dir.path(), &writes);
        let again = target.plan(dir.path());
        for w in &again {
            assert!(
                !w.changes(),
                "running {} again changes {} ({case})",
                target.name(),
                w.path
            );
        }
        removal_round_trip(target, case, &seed);
    }
}

/// Every file under `root`, by relative path.
fn files(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else {
                let rel = p.strip_prefix(root).unwrap().display().to_string();
                out.insert(rel, std::fs::read_to_string(&p).unwrap());
            }
        }
    }
    out
}

fn undo(root: &Path, removals: &[remove::Removal]) {
    for r in removals {
        let path = root.join(&r.path);
        match &r.after {
            Some(text) => std::fs::write(path, text).unwrap(),
            None => std::fs::remove_file(path).unwrap(),
        }
    }
}

/// Removal from the record gives back the seed byte for byte; removal
/// without one leaves no trace of Aver and keeps every other entry.
fn removal_round_trip(target: Target, case: &str, seed: &[(&str, &str)]) {
    let seeded: BTreeMap<String, String> = seed
        .iter()
        .map(|(p, c)| ((*p).to_owned(), (*c).to_owned()))
        .collect();
    for with_record in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        for (rel, content) in seed {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        let writes = target.plan(dir.path());
        let mut record = remove::Record::default();
        for w in &writes {
            record.note_write(w);
        }
        apply(dir.path(), &writes);
        let removals = remove::plan(dir.path(), with_record.then_some(&record), |_| None).unwrap();
        undo(dir.path(), &removals);
        let left = files(dir.path());
        let name = target.name();
        if with_record {
            assert_eq!(left, seeded, "{name} {case}: removal restores the seed");
        } else {
            for (path, text) in &left {
                assert!(
                    !text.contains(text::BEGIN) && !text.contains(text::TOML_BEGIN),
                    "{name} {case}: {path} keeps a block\n{text}"
                );
                assert!(
                    !text.contains("\"aver\"") && !text.contains(" hook "),
                    "{name} {case}: {path} keeps Aver's entries\n{text}"
                );
            }
            let kept: Vec<&String> = left.keys().collect();
            let expected: Vec<&String> = seeded.keys().collect();
            assert_eq!(kept, expected, "{name} {case}: only Aver's own files go");
            assert!(
                remove::plan(dir.path(), None, |_| None).unwrap().is_empty(),
                "{name} {case}: nothing is left to remove"
            );
        }
    }
}

#[test]
fn claude() {
    golden(Target::Claude);
}

#[test]
fn cursor() {
    golden(Target::Tool(Tool::Cursor));
}

#[test]
fn codex() {
    golden(Target::Tool(Tool::Codex));
}

#[test]
fn gemini() {
    golden(Target::Tool(Tool::Gemini));
}

#[test]
fn vscode() {
    golden(Target::Tool(Tool::Vscode));
}

#[test]
fn agents_md() {
    golden(Target::Tool(Tool::AgentsMd));
}

/// Two tools that share `AGENTS.md` (Codex and the generic target) keep
/// one block between them, and new instructions replace an old block.
#[test]
fn tools_share_one_block() {
    let dir = tempfile::tempdir().unwrap();
    for tool in [Tool::Codex, Tool::AgentsMd, Tool::Codex] {
        let writes = tools::plan(tool, dir.path(), true).unwrap();
        apply(dir.path(), &writes);
    }
    let agents = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert_eq!(agents.matches(text::BEGIN).count(), 1, "{agents}");
    assert_eq!(agents, text::block());
}

/// Setting up over what Aver before 0.4 wrote takes its MCP server out of
/// every tool's configuration and keeps everyone else's servers, and
/// removal afterwards never puts Aver's server back.
#[test]
fn setting_up_again_removes_the_old_mcp_server() {
    let targets = [Target::Claude]
        .into_iter()
        .chain(Tool::ALL.map(Target::Tool));
    for target in targets {
        let Some((rel, old, cleaned)) = target.legacy_mcp() else {
            continue;
        };
        let name = target.name();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, old).unwrap();
        let writes = target.plan(dir.path());
        let w = writes.iter().find(|w| w.path == rel).unwrap();
        assert_eq!(
            serde_text(rel, &w.after),
            serde_text(rel, cleaned),
            "{name}"
        );
        let mut record = remove::Record::default();
        for w in &writes {
            record.note_write(w);
        }
        apply(dir.path(), &writes);
        undo(
            dir.path(),
            &remove::plan(dir.path(), Some(&record), |_| None).unwrap(),
        );
        let left = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            serde_text(rel, &left),
            serde_text(rel, cleaned),
            "{name}: removal keeps others' servers and not Aver's"
        );

        let key = if rel == ".vscode/mcp.json" {
            "servers"
        } else {
            "mcpServers"
        };
        std::fs::write(&path, format!(r#"{{"{key}":{{"aver":{{}}}}}}"#)).unwrap();
        if rel.ends_with(".json") {
            let writes = target.plan(dir.path());
            let w = writes.iter().find(|w| w.path == rel).unwrap();
            assert!(w.deletes(), "{name}: a file of Aver's server alone goes");
        }
    }
}

/// `text` as a value to compare: JSON parsed, TOML as is.
fn serde_text(rel: &str, text: &str) -> serde_json::Value {
    if rel.ends_with(".json") {
        serde_json::from_str(text).unwrap()
    } else {
        serde_json::Value::String(text.trim_end().to_owned())
    }
}

/// An MCP configuration Aver cannot parse (VS Code allows comments) is not
/// one Aver wrote, so setting up leaves it alone.
#[test]
fn an_mcp_configuration_aver_cannot_parse_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".vscode")).unwrap();
    std::fs::write(
        dir.path().join(".vscode/mcp.json"),
        "// comments\n{\"servers\":{\"aver\":{}}}",
    )
    .unwrap();
    let writes = tools::plan(Tool::Vscode, dir.path(), true).unwrap();
    let paths: Vec<&str> = writes.iter().map(|w| w.path.as_str()).collect();
    assert_eq!(paths, [".github/copilot-instructions.md"]);
}

/// The hooks `aver integrate claude` writes, without their rev: a change
/// here must come with an `INTEGRATION_REV` bump, so a stale integration
/// is detected. `AVER_UPDATE_GOLDEN=1` records a bump but never a change
/// at the same rev.
#[test]
fn the_claude_hooks_change_only_with_a_rev_bump() {
    let rev = integrate::INTEGRATION_REV;
    let current = serde_json::to_string_pretty(&integrate::hooks())
        .unwrap()
        .replace(&format!(" --rev {rev}"), "");
    let path = golden_path("claude-hooks-rev");
    let stored = std::fs::read_to_string(&path).unwrap_or_default();
    let (stored_rev, stored_hooks) = stored
        .split_once('\n')
        .and_then(|(head, rest)| Some((head.strip_prefix("rev ")?.parse::<u32>().ok()?, rest)))
        .unwrap_or((0, ""));
    assert!(
        stored_hooks == current || stored_rev != rev,
        "the hooks aver integrate claude writes changed but INTEGRATION_REV is still {rev}; bump it (crates/aver/src/integrate/mod.rs), then rerun with AVER_UPDATE_GOLDEN=1"
    );
    if stored_hooks == current && stored_rev == rev {
        return;
    }
    assert!(
        std::env::var_os("AVER_UPDATE_GOLDEN").is_some(),
        "{} records rev {stored_rev}; rerun with AVER_UPDATE_GOLDEN=1 to record rev {rev}",
        path.display()
    );
    std::fs::write(&path, format!("rev {rev}\n{current}")).unwrap();
}
