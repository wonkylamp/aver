//! Agent templates (§2.10): how `aver run` starts and resumes each kind of
//! agent CLI in its workspace directory, and what it reads from its output.
//!
//! - `claude`: `claude -p` with the brief appended to the system prompt,
//!   stream-json output, a session id chosen up front (so resuming needs no
//!   parsing), `--permission-mode acceptEdits` plus the workspace's own
//!   allowlist (`.claude/settings.local.json`), or
//!   `--dangerously-skip-permissions` with `--yolo`. Its Stop hook runs the
//!   verify gate inside the session.
//! - `codex`: `codex exec --json` in its `workspace-write` sandbox; resumed
//!   by session id (`codex exec resume <id>`). The sandbox may keep its
//!   `aver verify` from writing the home, so the supervisor verifies after
//!   it exits.
//! - custom: a shell command template with `{dir}`, `{brief}`,
//!   `{brief_file}`, `{prompt}`, and for the resume template `{feedback}`
//!   and `{session_id}`, each substituted shell-quoted.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::state::Usage;

#[derive(Debug, Clone, PartialEq)]
pub enum Template {
    Claude {
        /// Bypass permission checks (`--dangerously-skip-permissions`).
        yolo: bool,
        model: Option<String>,
        max_budget_usd: Option<f64>,
        /// The CLI to run (default `claude`).
        bin: String,
    },
    Codex {
        model: Option<String>,
        bin: String,
    },
    Custom {
        spawn: String,
        resume: Option<String>,
    },
}

/// What a template needs to build a command line.
pub struct SpawnCtx<'a> {
    pub dir: &'a Path,
    /// The brief's text and its file (`.aver/BRIEF.md`).
    pub brief: &'a str,
    pub brief_file: &'a Path,
    /// The task as the user wrote it.
    pub prompt: &'a str,
    pub session_id: Option<&'a str>,
    /// Extra settings file for Claude when the repository tracks its local
    /// settings file.
    pub claude_settings: Option<&'a Path>,
}

/// What one line of agent output told the supervisor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Event {
    pub action: Option<String>,
    pub session_id: Option<String>,
    pub usage: Option<Usage>,
}

impl Template {
    pub fn claude(yolo: bool) -> Self {
        Self::Claude {
            yolo,
            model: None,
            max_budget_usd: None,
            bin: "claude".into(),
        }
    }

    pub fn codex() -> Self {
        Self::Codex {
            model: None,
            bin: "codex".into(),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Claude { .. } => "claude",
            Self::Codex { .. } => "codex",
            Self::Custom { .. } => "custom",
        }
    }

    /// Whether the agent runs Aver's verify gate itself before it stops
    /// (Claude's Stop hook), rather than the supervisor after it exits.
    pub fn gates_itself(&self) -> bool {
        matches!(self, Self::Claude { .. })
    }

    pub fn can_resume(&self) -> bool {
        match self {
            Self::Claude { .. } | Self::Codex { .. } => true,
            Self::Custom { resume, .. } => resume.is_some(),
        }
    }

    /// A session id to give the agent up front, if the template takes one.
    pub fn new_session_id(&self) -> Option<String> {
        match self {
            Self::Claude { .. } => Some(uuid_v4()),
            _ => None,
        }
    }

    /// The command line that starts the agent.
    pub fn spawn_argv(&self, ctx: &SpawnCtx<'_>) -> Vec<String> {
        match self {
            Self::Claude {
                yolo,
                model,
                max_budget_usd,
                bin,
            } => {
                let mut argv = vec![
                    bin.clone(),
                    "-p".into(),
                    ctx.prompt.into(),
                    "--append-system-prompt".into(),
                    ctx.brief.into(),
                ];
                if let Some(id) = ctx.session_id {
                    argv.extend(["--session-id".into(), id.into()]);
                }
                argv.extend(claude_common(*yolo, model, *max_budget_usd, ctx));
                argv
            }
            Self::Codex { model, bin } => {
                let mut argv = vec![
                    bin.clone(),
                    "exec".into(),
                    "--json".into(),
                    "--skip-git-repo-check".into(),
                    "--sandbox".into(),
                    "workspace-write".into(),
                    "--cd".into(),
                    ctx.dir.display().to_string(),
                ];
                if let Some(m) = model {
                    argv.extend(["--model".into(), m.clone()]);
                }
                argv.push(format!("{}\n\n{}", ctx.brief, ctx.prompt));
                argv
            }
            Self::Custom { spawn, .. } => shell(&fill(spawn, ctx, None)),
        }
    }

    /// The command line that resumes the agent with `feedback`, if the
    /// template can.
    pub fn resume_argv(&self, ctx: &SpawnCtx<'_>, feedback: &str) -> Option<Vec<String>> {
        match self {
            Self::Claude {
                yolo,
                model,
                max_budget_usd,
                bin,
            } => {
                let mut argv = vec![
                    bin.clone(),
                    "-p".into(),
                    feedback.into(),
                    "--resume".into(),
                    ctx.session_id?.into(),
                ];
                argv.extend(claude_common(*yolo, model, *max_budget_usd, ctx));
                Some(argv)
            }
            Self::Codex { bin, .. } => Some(vec![
                bin.clone(),
                "exec".into(),
                "--json".into(),
                "--skip-git-repo-check".into(),
                "resume".into(),
                ctx.session_id?.into(),
                feedback.into(),
            ]),
            Self::Custom { resume, .. } => {
                Some(shell(&fill(resume.as_deref()?, ctx, Some(feedback))))
            }
        }
    }

    /// The command line that resumes the agent's session to ask it
    /// `questions` and print its answer as plain text, without letting it
    /// change files: Claude in plan mode with its edit tools disallowed,
    /// Codex in its read-only sandbox, a custom agent through its resume
    /// template with `{feedback}` as the questions. `None` when the
    /// template cannot resume or the session is unknown.
    pub fn feedback_argv(&self, ctx: &SpawnCtx<'_>, questions: &str) -> Option<Vec<String>> {
        match self {
            Self::Claude { model, bin, .. } => {
                let mut argv = vec![
                    bin.clone(),
                    "-p".into(),
                    questions.into(),
                    "--resume".into(),
                    ctx.session_id?.into(),
                    "--output-format".into(),
                    "text".into(),
                    "--permission-mode".into(),
                    "plan".into(),
                    "--disallowedTools".into(),
                    "Edit,Write,MultiEdit,NotebookEdit".into(),
                ];
                if let Some(settings) = ctx.claude_settings {
                    argv.extend(["--settings".into(), settings.display().to_string()]);
                }
                if let Some(m) = model {
                    argv.extend(["--model".into(), m.clone()]);
                }
                Some(argv)
            }
            Self::Codex { bin, .. } => Some(vec![
                bin.clone(),
                "exec".into(),
                "--skip-git-repo-check".into(),
                "--sandbox".into(),
                "read-only".into(),
                "resume".into(),
                ctx.session_id?.into(),
                questions.into(),
            ]),
            Self::Custom { resume, .. } => {
                Some(shell(&fill(resume.as_deref()?, ctx, Some(questions))))
            }
        }
    }

    /// Reads one line of the agent's output.
    pub fn parse_line(&self, line: &str) -> Event {
        let line = line.trim();
        if line.is_empty() {
            return Event::default();
        }
        match self {
            Self::Claude { .. } => parse_claude(line),
            Self::Codex { .. } => parse_codex(line),
            Self::Custom { .. } => Event {
                action: Some(clean(line)),
                ..Event::default()
            },
        }
    }
}

fn claude_common(
    yolo: bool,
    model: &Option<String>,
    max_budget_usd: Option<f64>,
    ctx: &SpawnCtx<'_>,
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
    ];
    if yolo {
        argv.push("--dangerously-skip-permissions".into());
    } else {
        argv.extend(["--permission-mode".into(), "acceptEdits".into()]);
    }
    if let Some(settings) = ctx.claude_settings {
        argv.extend(["--settings".into(), settings.display().to_string()]);
    }
    if let Some(m) = model {
        argv.extend(["--model".into(), m.clone()]);
    }
    if let Some(usd) = max_budget_usd {
        argv.extend(["--max-budget-usd".into(), format!("{usd}")]);
    }
    argv
}

/// Quotes `s` for a POSIX shell.
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:,+@%".contains(&b))
    {
        return s.to_owned();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn fill(template: &str, ctx: &SpawnCtx<'_>, feedback: Option<&str>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let value = match &after[..end] {
            "dir" => Some(sh_quote(&ctx.dir.display().to_string())),
            "brief" => Some(sh_quote(ctx.brief)),
            "brief_file" => Some(sh_quote(&ctx.brief_file.display().to_string())),
            "prompt" => Some(sh_quote(ctx.prompt)),
            "feedback" => feedback.map(sh_quote),
            "session_id" => ctx.session_id.map(sh_quote),
            _ => None,
        };
        match value {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[start..start + end + 2]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn shell(command: &str) -> Vec<String> {
    vec!["/bin/sh".into(), "-c".into(), command.into()]
}

/// The agent CLIs on `PATH`, in preference order.
pub fn installed() -> Vec<&'static str> {
    ["claude", "codex"]
        .into_iter()
        .filter(|bin| find_on_path(bin).is_some())
        .collect()
}

pub fn find_on_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|p| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// A line without control characters, at most 120 characters.
fn clean(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip an ANSI escape sequence.
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if c.is_control() {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    let out = out.trim();
    match out.char_indices().nth(120) {
        Some((i, _)) => format!("{}…", &out[..i]),
        None => out.to_owned(),
    }
}

fn short_path(path: &str) -> String {
    path.rsplit('/')
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("/")
}

fn parse_claude(line: &str) -> Event {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Event::default();
    };
    let mut event = Event {
        session_id: v["session_id"].as_str().map(str::to_owned),
        ..Event::default()
    };
    match v["type"].as_str() {
        Some("assistant") => {
            if let Some(items) = v["message"]["content"].as_array() {
                for item in items {
                    if item["type"] == "tool_use" {
                        let name = item["name"].as_str().unwrap_or("tool");
                        let input = &item["input"];
                        let detail = input["command"]
                            .as_str()
                            .map(str::to_owned)
                            .or_else(|| input["file_path"].as_str().map(short_path))
                            .or_else(|| input["pattern"].as_str().map(str::to_owned))
                            .unwrap_or_default();
                        event.action = Some(clean(&format!("{name} {detail}")));
                    } else if item["type"] == "text" && event.action.is_none() {
                        if let Some(text) = item["text"].as_str() {
                            event.action = Some(clean(text));
                        }
                    }
                }
            }
        }
        Some("result") => {
            event.usage = Some(Usage {
                cost_usd: v["total_cost_usd"].as_f64(),
                input_tokens: v["usage"]["input_tokens"].as_u64(),
                output_tokens: v["usage"]["output_tokens"].as_u64(),
            });
            event.action = Some(match v["subtype"].as_str() {
                Some("success") => "finished its turn".into(),
                Some(other) => clean(&format!("ended: {other}")),
                None => "ended".into(),
            });
        }
        _ => {}
    }
    event
}

fn parse_codex(line: &str) -> Event {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Event::default();
    };
    let mut event = Event {
        session_id: v["thread_id"]
            .as_str()
            .or(v["session_id"].as_str())
            .or(v["msg"]["session_id"].as_str())
            .map(str::to_owned),
        ..Event::default()
    };
    let item = &v["item"];
    match (v["type"].as_str(), item["type"].as_str()) {
        (Some("item.started" | "item.completed"), Some("command_execution")) => {
            event.action = item["command"].as_str().map(|c| clean(&format!("$ {c}")));
        }
        (Some("item.completed"), Some("file_change")) => {
            let paths: Vec<String> = item["changes"]
                .as_array()
                .map(|c| {
                    c.iter()
                        .filter_map(|ch| ch["path"].as_str().map(short_path))
                        .collect()
                })
                .unwrap_or_default();
            event.action = Some(clean(&format!("edit {}", paths.join(", "))));
        }
        (Some("item.completed"), Some("agent_message")) => {
            event.action = item["text"].as_str().map(clean);
        }
        (Some("turn.completed"), _) => {
            event.usage = Some(Usage {
                cost_usd: None,
                input_tokens: v["usage"]["input_tokens"].as_u64(),
                output_tokens: v["usage"]["output_tokens"].as_u64(),
            });
        }
        _ => {}
    }
    event
}

/// A random UUID (version 4) for Claude's `--session-id`.
fn uuid_v4() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex = data_encoding::HEXLOWER.encode(&b);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(dir: &'a Path, brief_file: &'a Path) -> SpawnCtx<'a> {
        SpawnCtx {
            dir,
            brief: "BRIEF it's",
            brief_file,
            prompt: "do the thing",
            session_id: Some("s-1"),
            claude_settings: None,
        }
    }

    #[test]
    fn custom_templates_substitute_quoted_values() {
        let dir = Path::new("/w/a b");
        let file = Path::new("/w/a b/.aver/BRIEF.md");
        let t = Template::Custom {
            spawn: "agent --dir {dir} --brief {brief} {unknown} {prompt}".into(),
            resume: Some("agent --resume {session_id} {feedback}".into()),
        };
        assert_eq!(
            t.spawn_argv(&ctx(dir, file))[2],
            "agent --dir '/w/a b' --brief 'BRIEF it'\\''s' {unknown} 'do the thing'"
        );
        assert_eq!(
            t.resume_argv(&ctx(dir, file), "fix x").unwrap()[2],
            "agent --resume s-1 'fix x'"
        );
        let no_resume = Template::Custom {
            spawn: "x".into(),
            resume: None,
        };
        assert!(no_resume.resume_argv(&ctx(dir, file), "f").is_none());
    }

    #[test]
    fn claude_uses_accept_edits_unless_yolo() {
        let dir = Path::new("/w");
        let file = Path::new("/w/.aver/BRIEF.md");
        let argv = Template::claude(false).spawn_argv(&ctx(dir, file));
        assert!(argv
            .windows(2)
            .any(|w| w == ["--permission-mode", "acceptEdits"]));
        assert!(argv.windows(2).any(|w| w == ["--session-id", "s-1"]));
        assert!(!argv.contains(&"--dangerously-skip-permissions".to_owned()));
        let yolo = Template::claude(true).spawn_argv(&ctx(dir, file));
        assert!(yolo.contains(&"--dangerously-skip-permissions".to_owned()));
        let resume = Template::claude(false)
            .resume_argv(&ctx(dir, file), "report")
            .unwrap();
        assert!(resume.windows(2).any(|w| w == ["--resume", "s-1"]));
    }

    #[test]
    fn output_lines_become_actions_sessions_and_usage() {
        let claude = Template::claude(false);
        let e = claude.parse_line(r#"{"type":"assistant","session_id":"abc","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"/w/src/rate/limit.ts"}}]}}"#);
        assert_eq!(e.action.as_deref(), Some("Edit rate/limit.ts"));
        assert_eq!(e.session_id.as_deref(), Some("abc"));
        let r = claude.parse_line(r#"{"type":"result","subtype":"success","total_cost_usd":1.84,"usage":{"input_tokens":10,"output_tokens":5}}"#);
        assert_eq!(r.usage.unwrap().short(), "$1.84");
        let codex = Template::codex();
        let c = codex.parse_line(
            r#"{"type":"item.started","item":{"type":"command_execution","command":"npm test"}}"#,
        );
        assert_eq!(c.action.as_deref(), Some("$ npm test"));
        let t = codex.parse_line(r#"{"type":"thread.started","thread_id":"t-9"}"#);
        assert_eq!(t.session_id.as_deref(), Some("t-9"));
        let custom = Template::Custom {
            spawn: String::new(),
            resume: None,
        };
        assert_eq!(
            custom
                .parse_line("\u{1b}[32mediting\u{1b}[0m src/x.js\n")
                .action
                .as_deref(),
            Some("editing src/x.js")
        );
        assert_eq!(uuid_v4().len(), 36);
    }
}
