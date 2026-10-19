//! Undoing `aver integrate` and `aver setup` (`aver setup --remove`).
//!
//! Every write the integrations make is noted in a [`Record`] kept in the
//! repository's Git directory, with the file's contents before Aver first
//! touched it. A file that still holds exactly what Aver last wrote is put
//! back byte for byte (or deleted, when Aver created it). A file changed
//! since, or one written before records existed, is cleaned structurally:
//! Aver's server entry, hooks and marked blocks come out and everything
//! else stays.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::text;
use super::tools::{self, Tool};
use super::{plugin, FileWrite, LOCAL_SETTINGS, MCP, SERVER, SETTINGS};

/// The record's place in the repository's Git directory (`git rev-parse
/// --git-dir`).
pub const RECORD: &str = "aver/integrate.json";

/// What Aver wrote to one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The file before Aver first wrote it; `None` when Aver created it.
    pub before: Option<String>,
    /// What Aver last wrote.
    pub after: String,
    /// Whether `before` is still the file as it was without Aver: false
    /// once someone else changed it between two of Aver's writes.
    pub exact: bool,
}

/// Everything the integrations wrote in one repository.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// By path relative to the repository root.
    #[serde(default)]
    pub files: BTreeMap<String, Entry>,
    /// Directories Aver created (relative), removed again when empty.
    #[serde(default)]
    pub dirs: Vec<String>,
    /// `.git/info/exclude`, and the lines Aver added to it.
    #[serde(default)]
    pub exclude: Option<Entry>,
    #[serde(default)]
    pub exclude_lines: Vec<String>,
}

impl Record {
    pub fn parse(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("{RECORD} is not a valid record ({e})"))
    }

    pub fn to_json(&self) -> String {
        let mut out = serde_json::to_string_pretty(self).expect("records serialize");
        out.push('\n');
        out
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.dirs.is_empty() && self.exclude.is_none()
    }

    fn note(entry: &mut Option<Entry>, before: Option<&str>, after: &str) {
        match entry {
            Some(e) => {
                e.exact &= before == Some(e.after.as_str());
                e.after = after.to_owned();
            }
            None => {
                *entry = Some(Entry {
                    before: before.map(str::to_owned),
                    after: after.to_owned(),
                    exact: true,
                })
            }
        }
    }

    /// Notes a write about to happen.
    pub fn note_write(&mut self, w: &FileWrite) {
        let mut entry = self.files.remove(&w.path);
        Self::note(&mut entry, w.before.as_deref(), &w.after);
        self.files.insert(w.path.clone(), entry.expect("noted"));
    }

    /// Notes a write of `.git/info/exclude` adding `lines`.
    pub fn note_exclude(&mut self, before: Option<&str>, after: &str, lines: &[String]) {
        Self::note(&mut self.exclude, before, after);
        for l in lines {
            if !self.exclude_lines.contains(l) {
                self.exclude_lines.push(l.clone());
            }
        }
    }

    pub fn note_dir(&mut self, rel: &str) {
        if !self.dirs.iter().any(|d| d == rel) {
            self.dirs.push(rel.to_owned());
        }
    }
}

/// One file `--remove` changes: its contents now, and after (`None`
/// deletes it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub path: String,
    pub before: String,
    pub after: Option<String>,
}

/// Every file an integration may write or have written before 0.4,
/// relative to the repository root.
pub fn known_paths() -> Vec<&'static str> {
    let mut out = vec![SETTINGS, LOCAL_SETTINGS, MCP];
    out.extend(plugin::SHARED.iter().map(|f| f.project));
    for tool in Tool::ALL {
        out.extend(tool.legacy_mcp_config());
        out.push(tool.instructions_file());
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Whether `path` is an MCP configuration Aver before 0.4 registered its
/// server in.
pub fn known_legacy_mcp(path: &str) -> bool {
    path == MCP
        || Tool::ALL
            .iter()
            .any(|t| t.legacy_mcp_config() == Some(path))
}

/// Whether `command` is one of the hooks [`super::hooks`] installs.
fn is_aver_hook(command: &str) -> bool {
    super::parse_hook(command).is_some()
}

fn parse(path: &str, text: &str) -> Result<Map<String, Value>, String> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(m)) => Ok(m),
        _ => Err(format!(
            "{path} is not a JSON object; remove Aver's entries from it by hand"
        )),
    }
}

/// `root` as the file's new text: `text` itself when nothing of Aver's was
/// in it (`untouched`), `None` when nothing else is left.
fn finish(text: &str, untouched: &Map<String, Value>, root: Map<String, Value>) -> Option<String> {
    if &root == untouched {
        return Some(text.to_owned());
    }
    (!root.is_empty()).then(|| super::pretty(&Value::Object(root)))
}

/// Claude Code settings (`path`) without Aver's hooks and the approval of
/// its old MCP server.
fn strip_settings(path: &str, text: &str) -> Result<Option<String>, String> {
    let mut root = parse(path, text)?;
    let untouched = root.clone();
    if let Some(Value::Object(hooks)) = root.get_mut("hooks") {
        for groups in hooks.values_mut() {
            if let Value::Array(list) = groups {
                // A group may hold the user's commands beside Aver's: only
                // Aver's go, and a group goes once nothing is left in it.
                list.retain_mut(|group| {
                    let Some(Value::Array(commands)) = group.get_mut("hooks") else {
                        return true;
                    };
                    let had = commands.len();
                    commands.retain(|h| !h["command"].as_str().is_some_and(is_aver_hook));
                    commands.len() == had || !commands.is_empty()
                });
            }
        }
        hooks.retain(|_, groups| !matches!(groups, Value::Array(l) if l.is_empty()));
    }
    if matches!(root.get("hooks"), Some(Value::Object(h)) if h.is_empty()) {
        root.remove("hooks");
    }
    if let Some(Value::Array(enabled)) = root.get_mut("enabledMcpjsonServers") {
        enabled.retain(|v| v != SERVER);
        if enabled.is_empty() {
            root.remove("enabledMcpjsonServers");
        }
    }
    Ok(finish(text, &untouched, root))
}

/// A JSON MCP configuration without the `aver` server Aver before 0.4
/// registered under `key`.
fn strip_server(path: &str, text: &str, key: &str) -> Result<Option<String>, String> {
    let mut root = parse(path, text)?;
    let untouched = root.clone();
    if let Some(Value::Object(servers)) = root.get_mut(key) {
        servers.remove(SERVER);
        if servers.is_empty() {
            root.remove(key);
        }
    }
    Ok(finish(text, &untouched, root))
}

/// Structural removal: `text` (the file at `path`) without Aver's parts,
/// `None` when nothing else is left.
pub fn strip(path: &str, text: &str) -> Result<Option<String>, String> {
    let unblocked = |begin: &str, end: &str| -> Result<Option<String>, String> {
        let out = text::remove(path, text, begin, end)?;
        Ok((!out.trim().is_empty()).then_some(out))
    };
    if path == SETTINGS || path == LOCAL_SETTINGS {
        return strip_settings(path, text);
    }
    // The plugin's skill and commands are Aver's whole files: one still as
    // this Aver wrote it goes, one someone changed stays.
    if let Some(f) = plugin::SHARED.iter().find(|f| f.project == path) {
        return Ok((text != f.text).then(|| text.to_owned()));
    }
    if path == MCP {
        return strip_server(path, text, "mcpServers");
    }
    for tool in Tool::ALL {
        if tool.legacy_mcp_config() == Some(path) {
            return match tool {
                Tool::Codex => unblocked(text::TOML_BEGIN, text::TOML_END),
                Tool::Vscode => strip_server(path, text, "servers"),
                _ => strip_server(path, text, "mcpServers"),
            };
        }
    }
    if path == Tool::Cursor.instructions_file()
        && text == tools::merge_instructions(Tool::Cursor, None)?
    {
        return Ok(None);
    }
    unblocked(text::BEGIN, text::END)
}

/// `stripped`, the JSON file at `path` without Aver's parts, in the layout
/// of its committed version (`committed`) when that holds the same value:
/// structural removal writes JSON in its own layout, and without a record
/// the committed bytes are the only copy of the user's.
fn committed_layout(path: &str, stripped: String, committed: Option<String>) -> String {
    let same = |a: &str, b: &str| {
        matches!(
            (serde_json::from_str::<Value>(a), serde_json::from_str::<Value>(b)),
            (Ok(a), Ok(b)) if a == b
        )
    };
    match committed {
        Some(c) if path.ends_with(".json") && same(&c, &stripped) => c,
        _ => stripped,
    }
}

/// What `--remove` changes in the repository at `root`, given its record.
/// `committed` reads a path's committed version, if it has one.
pub fn plan(
    root: &Path,
    record: Option<&Record>,
    committed: impl Fn(&str) -> Option<String>,
) -> Result<Vec<Removal>, String> {
    let mut paths: Vec<String> = known_paths().into_iter().map(str::to_owned).collect();
    if let Some(r) = record {
        paths.extend(r.files.keys().cloned());
    }
    paths.sort();
    paths.dedup();
    let mut out = Vec::new();
    for path in paths {
        let Ok(now) = std::fs::read_to_string(root.join(&path)) else {
            continue;
        };
        let entry = record.and_then(|r| r.files.get(&path));
        // A file's text before the record began may still hold what an
        // older Aver wrote there (its MCP server, or its hooks in the local
        // settings this Aver moved them out of); that is not restored.
        let before = |e: &Entry| {
            let text = e.before.as_deref()?;
            if !known_legacy_mcp(&path) && path != LOCAL_SETTINGS {
                return Some(text.to_owned());
            }
            strip(&path, text).unwrap_or_else(|_| Some(text.to_owned()))
        };
        let after = match entry {
            Some(e) if e.exact && e.after == now => before(e),
            _ => match strip(&path, &now) {
                Ok(Some(a)) if a != now => Some(committed_layout(&path, a, committed(&path))),
                Ok(after) => after,
                // Neither what Aver wrote nor a file it can parse: a write
                // torn by a crash or a full disk (before atomic writes),
                // so it goes back to what it was before Aver.
                Err(e) => match entry {
                    Some(entry) => before(entry),
                    None => return Err(e),
                },
            },
        };
        if after.as_deref() != Some(now.as_str()) {
            out.push(Removal {
                path,
                before: now,
                after,
            });
        }
    }
    Ok(out)
}

/// `.git/info/exclude` (`now`) without the lines Aver added: as it was
/// before (`None`: it did not exist), when Aver's write is the last one.
pub fn exclude_after(record: &Record, now: &str) -> Option<String> {
    if let Some(e) = &record.exclude {
        if e.exact && e.after == now {
            return e.before.clone();
        }
    }
    let mut out = String::new();
    for line in now.split_inclusive('\n') {
        if !record.exclude_lines.iter().any(|l| l == line.trim_end()) {
            out.push_str(line);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aver_hooks_are_recognized_and_others_are_not() {
        assert!(is_aver_hook("/opt/aver/bin/aver hook pre-edit"));
        assert!(is_aver_hook("'/my tools/aver' hook post-edit"));
        assert!(is_aver_hook(
            "AVER_HOME=/h/.aver /opt/aver/aver hook worktree-create"
        ));
        assert!(!is_aver_hook("lint.sh"));
        assert!(!is_aver_hook("/opt/other/bin/never hook pre-edit"));
        assert!(is_aver_hook("AVER_HOME=/h /opt/aver/aver hook stop"));
        assert!(is_aver_hook("/opt/aver/aver hook session-start"));
        assert!(!is_aver_hook("/opt/aver/aver hook stopper"));
    }

    #[test]
    fn a_rewrite_after_someone_else_edited_the_file_is_not_restored_exactly() {
        let mut r = Record::default();
        let w = |before: Option<&str>, after: &str| FileWrite {
            path: "AGENTS.md".into(),
            before: before.map(str::to_owned),
            after: after.into(),
        };
        r.note_write(&w(Some("mine\n"), "mine\nAVER\n"));
        r.note_write(&w(Some("mine\nAVER\n"), "mine\nAVER2\n"));
        assert!(r.files["AGENTS.md"].exact);
        assert_eq!(r.files["AGENTS.md"].before.as_deref(), Some("mine\n"));
        r.note_write(&w(Some("edited\nAVER2\n"), "edited\nAVER3\n"));
        assert!(!r.files["AGENTS.md"].exact);
    }

    #[test]
    fn a_torn_file_goes_back_to_before() {
        let dir = tempfile::tempdir().unwrap();
        let mine = "{\"permissions\":{\"allow\":[]}}\n";
        let ours = "{\"permissions\":{\"allow\":[]},\"enabledMcpjsonServers\":[\"aver\"]}\n";
        let mut r = Record::default();
        r.note_write(&FileWrite {
            path: SETTINGS.into(),
            before: Some(mine.into()),
            after: ours.into(),
        });
        r.note_write(&FileWrite {
            path: MCP.into(),
            before: None,
            after: "{\"mcpServers\":{\"aver\":{}}}\n".into(),
        });
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(dir.path().join(SETTINGS), &ours[..20]).unwrap();
        std::fs::write(dir.path().join(MCP), "").unwrap();
        let plan = plan(dir.path(), Some(&r), |_| None).unwrap();
        let by_path = |p: &str| plan.iter().find(|r| r.path == p).unwrap().after.clone();
        assert_eq!(by_path(SETTINGS).as_deref(), Some(mine));
        assert_eq!(by_path(MCP), None, "Aver created it, so it goes");

        // Without a record, a file Aver cannot parse is the user's to fix.
        assert!(super::plan(dir.path(), None, |_| None).is_err());
    }

    #[test]
    fn without_a_record_json_goes_back_to_its_committed_layout() {
        let dir = tempfile::tempdir().unwrap();
        let committed = "{ \"permissions\": { \"allow\": [] } }\n";
        let ours = "{\"enabledMcpjsonServers\":[\"aver\"],\"permissions\":{\"allow\":[]}}";
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(dir.path().join(SETTINGS), ours).unwrap();
        let head = |p: &str| (p == SETTINGS).then(|| committed.to_owned());
        let plan = plan(dir.path(), None, head).unwrap();
        assert_eq!(plan[0].after.as_deref(), Some(committed));

        // A file with nothing of Aver's keeps its own layout.
        std::fs::write(
            dir.path().join(SETTINGS),
            "{\"permissions\":{\"allow\":[]}}",
        )
        .unwrap();
        assert_eq!(super::plan(dir.path(), None, head).unwrap(), vec![]);
        // A committed version with other values is not used.
        let other = |_: &str| Some("{\"permissions\":{}}\n".to_owned());
        std::fs::write(dir.path().join(SETTINGS), ours).unwrap();
        let plan = super::plan(dir.path(), None, other).unwrap();
        assert!(plan[0].after.as_deref().unwrap().contains("\"allow\""));
    }

    #[test]
    fn a_users_hook_in_the_same_group_as_avers_stays() {
        let settings = r#"{"hooks":{"PostToolUse":[{"matcher":"Edit|Write","hooks":[{"type":"command","command":"/b/aver hook post-edit"},{"type":"command","command":"prettier --write"}]},{"matcher":"Bash","hooks":[{"type":"command","command":"/b/aver hook post-bash"}]}]}}"#;
        let out: Value =
            serde_json::from_str(&strip(SETTINGS, settings).unwrap().unwrap()).unwrap();
        assert_eq!(
            out,
            serde_json::json!({"hooks":{"PostToolUse":[{"matcher":"Edit|Write","hooks":[{"type":"command","command":"prettier --write"}]}]}})
        );
    }

    #[test]
    fn stripping_keeps_everything_that_is_not_avers() {
        let settings = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"lint.sh"}]},{"matcher":"Edit","hooks":[{"type":"command","command":"/b/aver hook pre-edit"}]}],"WorktreeCreate":[{"hooks":[{"type":"command","command":"AVER_HOME=/h /b/aver hook worktree-create"}]}]},"enabledMcpjsonServers":["aver"],"permissions":{}}"#;
        let out: Value =
            serde_json::from_str(&strip(SETTINGS, settings).unwrap().unwrap()).unwrap();
        assert_eq!(
            out,
            serde_json::json!({"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"lint.sh"}]}]},"permissions":{}})
        );
        let only_ours = r#"{"hooks":{"PostToolUse":[{"hooks":[{"command":"/b/aver hook post-edit"}]}]},"enabledMcpjsonServers":["aver"]}"#;
        assert_eq!(strip(SETTINGS, only_ours).unwrap(), None);
        let vscode = r#"{"servers":{"aver":{},"github":{"type":"http"}}}"#;
        let out = strip(".vscode/mcp.json", vscode).unwrap().unwrap();
        assert!(!out.contains("aver") && out.contains("github"), "{out}");
        assert_eq!(strip(MCP, r#"{"mcpServers":{"aver":{}}}"#).unwrap(), None);
        let md = format!("# Mine\n\n{}", text::block());
        assert_eq!(
            strip("AGENTS.md", &md).unwrap().as_deref(),
            Some("# Mine\n")
        );
        assert_eq!(strip("GEMINI.md", &text::block()).unwrap(), None);
        assert!(strip(MCP, "[]").is_err());
        for (path, theirs) in [
            (SETTINGS, r#"{"permissions":{"allow":[]}}"#),
            (MCP, r#"{"mcpServers":{"db":{}}}"#),
        ] {
            assert_eq!(strip(path, theirs).unwrap().as_deref(), Some(theirs));
        }
    }
}
