//! A hook called by settings of another integration rev (ARCHITECTURE.md
//! §2.12) tells the agent once per session to have the integration
//! updated. The hook itself runs unchanged: its output is captured so the
//! notice joins whatever it prints (Claude Code reads one JSON object per
//! hook).

use std::io::{Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

use aver::integrate::installed::stale_hook_text;
use aver::keys::Home;
use serde_json::{json, Value};

/// A notice to deliver during this hook call.
pub(super) struct Due {
    text: String,
    event: String,
    /// Created once the notice reached the session.
    told: PathBuf,
}

/// The notice this call owes, if settings called it with a stale rev and
/// this session has not heard it yet. `raw` is the hook's input.
pub(super) fn due(home: &Home, rev: Option<u32>, plugin: bool, raw: &str) -> Option<Due> {
    // The plugin ships with the binary; `aver run` writes its agents'
    // settings itself, so neither can be stale.
    if plugin
        || std::env::var_os("AVER_RUN").is_some()
        || std::env::var_os(aver::run::feedback::FEEDBACK_ENV).is_some()
    {
        return None;
    }
    let text = stale_hook_text(rev)?;
    let input: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let session: String = input["session_id"]
        .as_str()
        .unwrap_or("unknown")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    let told = home.root().join("tmp/stale-told").join(session);
    if told.exists() {
        return None;
    }
    Some(Due {
        text,
        event: input["hook_event_name"].as_str().unwrap_or("").to_owned(),
        told,
    })
}

/// Runs `hook` with stdout captured, then prints its output with the
/// notice joined in, and remembers that the session heard it.
pub(super) fn deliver(
    home: &Home,
    due: Due,
    hook: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let Ok((captured, result)) = capture_stdout(home, hook) else {
        return Ok(());
    };
    match joined(&due.event, &captured, &due.text) {
        Some(out) => {
            println!("{out}");
            if let Some(dir) = due.told.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&due.told, "");
        }
        None => print!("{captured}"),
    }
    result
}

fn capture_stdout<T>(home: &Home, f: impl FnOnce() -> T) -> std::io::Result<(String, T)> {
    let dir = home.root().join("tmp");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("hook-output-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)?;
    let _ = std::fs::remove_file(&path);
    std::io::stdout().flush()?;
    // SAFETY: fd 1 and the file are open; the saved copy restores fd 1.
    let saved = unsafe { libc::dup(1) };
    if saved < 0 || unsafe { libc::dup2(file.as_raw_fd(), 1) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let value = f();
    let _ = std::io::stdout().flush();
    // SAFETY: `saved` is the original fd 1.
    unsafe {
        libc::dup2(saved, 1);
        libc::close(saved);
    }
    let mut out = String::new();
    file.rewind()?;
    file.read_to_string(&mut out)?;
    Ok((out, value))
}

/// The hook's output `captured` with `text` added where `event` lets the
/// agent read it; `None` when it cannot go anywhere this time.
fn joined(event: &str, captured: &str, text: &str) -> Option<String> {
    let context_event = matches!(
        event,
        "PreToolUse" | "PostToolUse" | "SessionStart" | "SubagentStart" | "UserPromptSubmit"
    );
    if captured.trim().is_empty() {
        return context_event.then(|| {
            json!({ "hookSpecificOutput": { "hookEventName": event, "additionalContext": text } })
                .to_string()
        });
    }
    let mut v: Value = serde_json::from_str(captured.trim()).ok()?;
    if !v.is_object() {
        return None;
    }
    let append = |slot: &mut Value| match slot.as_str() {
        Some(s) => {
            *slot = Value::String(format!("{s}\n\n{text}"));
            true
        }
        None => false,
    };
    let specific = &mut v["hookSpecificOutput"];
    if specific.is_object() {
        if append(&mut specific["additionalContext"])
            || append(&mut specific["permissionDecisionReason"])
        {
            return Some(v.to_string());
        }
        if context_event {
            specific["additionalContext"] = Value::String(text.to_owned());
            return Some(v.to_string());
        }
    } else if v.get("hookSpecificOutput").is_some() {
        v.as_object_mut()?.remove("hookSpecificOutput");
    }
    append(&mut v["reason"]).then(|| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::joined;
    use serde_json::Value;

    #[test]
    fn the_notice_joins_what_the_hook_prints() {
        let alone = joined("PostToolUse", "", "update").unwrap();
        let v: Value = serde_json::from_str(&alone).unwrap();
        assert_eq!(v["hookSpecificOutput"]["additionalContext"], "update");
        assert_eq!(joined("Stop", "", "update"), None);

        let context = r#"{"hookSpecificOutput":{"hookEventName":"PostToolUse","additionalContext":"notices"}}"#;
        let v: Value =
            serde_json::from_str(&joined("PostToolUse", context, "update").unwrap()).unwrap();
        assert_eq!(
            v["hookSpecificOutput"]["additionalContext"],
            "notices\n\nupdate"
        );

        let block = r#"{"decision":"block","reason":"verify first"}"#;
        let v: Value = serde_json::from_str(&joined("Stop", block, "update").unwrap()).unwrap();
        assert_eq!(v["reason"], "verify first\n\nupdate");

        assert_eq!(joined("WorktreeCreate", "/path\n", "update"), None);
    }
}
