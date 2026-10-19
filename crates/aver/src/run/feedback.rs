//! Built-in agent feedback (ARCHITECTURE.md §2.10): agents are Aver's
//! main users, so after a run Aver resumes each agent's session and asks it
//! the same fixed questions about Aver, read-only. The answers go to
//! `runs/<run>/feedback-<n>.md` and the rating to `run.json`, where `aver
//! status` shows it. The questions are one constant, so ratings compare
//! across versions of Aver.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, UNIX_EPOCH};

use aver_core::Timestamp;

use super::agent::{SpawnCtx, Template};
use super::brief::CLAUDE_SETTINGS_FALLBACK;
use super::state::{run_dir, AgentFeedback, AgentRun, RunState};
use crate::keys::Home;

/// The environment variable set while an agent answers: Aver's hooks then
/// let it stop without verifying and refuse every edit.
pub const FEEDBACK_ENV: &str = "AVER_FEEDBACK";

/// The questions, unchanged across rounds so ratings stay comparable.
pub const QUESTIONS: &str = "Aver is asking every agent of this run the same questions about Aver itself (the tool that gave you the brief, `aver check`, `aver verify` and its reports). Answer from your experience in this session. Do not modify any files and do not run commands that change anything: just answer.

1. What helped and what was friction? Quote the moments (a line of a report, a command, an error).
2. On hard tasks (flaky or already-failing test suites, unclear done criteria, other agents editing the same code), would Aver be better or worse for you than a plain checkout of the repository? Why?
3. Rate Aver from 1 to 10 against \"I absolutely need this\". Put the number on a line of its own as RATING: <n>/10.
4. What one change would raise your rating the most?";

/// The rating in an answer: the number in `RATING: <n>/10`, else the first
/// `<n>/10`, when it is 1 to 10.
pub fn rating(answer: &str) -> Option<u8> {
    let upper = answer.to_ascii_uppercase();
    let after_label = upper
        .match_indices("RATING")
        .find_map(|(at, _)| number_over_ten(&upper[at + "RATING".len()..]));
    after_label.or_else(|| {
        upper
            .match_indices("/10")
            .find_map(|(at, _)| leading_number(&upper[..at]))
    })
}

/// The `<n>/10` right after a label (skipping `:`, `*` and spaces).
fn number_over_ten(text: &str) -> Option<u8> {
    let rest = text.trim_start_matches([':', '*', ' ', '\t']);
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let after = rest[digits.len()..].trim_start();
    let n: u8 = digits.parse().ok()?;
    let over_ten = after
        .strip_prefix('/')
        .is_some_and(|r| r.trim_start().starts_with("10"));
    ((1..=10).contains(&n) && over_ten).then_some(n)
}

/// The number that ends `text` (before a `/10`), allowing spaces.
fn leading_number(text: &str) -> Option<u8> {
    let text = text.trim_end();
    let digits: String = text
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let n: u8 = digits.parse().ok()?;
    (1..=10).contains(&n).then_some(n)
}

/// The files of a workspace directory with their sizes and modification
/// times, sorted: equal before and after means the agent changed nothing.
/// Aver's own state, Git's and `node_modules` are left out.
fn fingerprint(dir: &Path) -> Vec<(PathBuf, u64, u128)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name();
            if matches!(name.to_str(), Some(".aver" | ".git" | "node_modules")) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(e.path()) else {
                continue;
            };
            if meta.is_dir() {
                stack.push(e.path());
            } else {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                out.push((e.path(), meta.len(), mtime));
            }
        }
    }
    out.sort();
    out
}

/// One agent's answer, or why it could not be asked.
#[derive(Debug)]
pub struct Asked {
    pub index: u32,
    pub outcome: Result<AgentFeedback, String>,
}

/// The template that resumes `agent`: the run's own for Claude and Codex
/// (by name), `resume_cmd` for a custom agent.
fn template_of(agent: &AgentRun, resume_cmd: Option<&str>) -> Option<Template> {
    match agent.template.as_str() {
        "claude" => Some(Template::claude(false)),
        "codex" => Some(Template::codex()),
        _ => resume_cmd.map(|r| Template::Custom {
            spawn: String::new(),
            resume: Some(r.to_owned()),
        }),
    }
}

/// Asks every agent of `run` (or those in `only`) the [`QUESTIONS`], at
/// once, each with `timeout`, writes their answers and records them in
/// `run.json`.
pub fn ask(
    home: &Home,
    run: &mut RunState,
    resume_cmd: Option<&str>,
    timeout: Duration,
    only: &[u32],
) -> std::io::Result<Vec<Asked>> {
    let dir = run_dir(home, &run.id);
    fs::create_dir_all(&dir)?;
    let shared: &RunState = run;
    let run_id = shared.id.as_str();
    let asked: Vec<Asked> = std::thread::scope(|s| {
        let handles: Vec<_> = shared
            .agents
            .iter()
            .filter(|a| only.is_empty() || only.contains(&a.index))
            .map(|agent| {
                let dir = &dir;
                s.spawn(move || Asked {
                    index: agent.index,
                    outcome: ask_one(home, run_id, agent, resume_cmd, timeout, dir),
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("asking an agent does not panic"))
            .collect()
    });
    *run = RunState::update(home, &run.id, |run| {
        for a in &asked {
            if let (Ok(feedback), Some(agent)) = (
                &a.outcome,
                run.agents.iter_mut().find(|r| r.index == a.index),
            ) {
                agent.feedback = Some(feedback.clone());
            }
        }
    })?;
    Ok(asked)
}

fn ask_one(
    home: &Home,
    run_id: &str,
    agent: &AgentRun,
    resume_cmd: Option<&str>,
    timeout: Duration,
    run_dir: &Path,
) -> Result<AgentFeedback, String> {
    if !agent.dir.is_dir() {
        return Err(format!(
            "its directory {} is gone (accepted or discarded runs release them)",
            agent.dir.display()
        ));
    }
    let template = template_of(agent, resume_cmd).ok_or(
        "a custom agent is resumed with --resume-cmd '<command with {session_id} and {feedback}>'",
    )?;
    let settings = agent.dir.join(CLAUDE_SETTINGS_FALLBACK);
    let brief_file = agent.dir.join(".aver/BRIEF.md");
    let ctx = SpawnCtx {
        dir: &agent.dir,
        brief: "",
        brief_file: &brief_file,
        prompt: "",
        session_id: agent.session_id.as_deref(),
        claude_settings: settings.is_file().then_some(settings.as_path()),
    };
    let argv = template
        .feedback_argv(&ctx, QUESTIONS)
        .ok_or("its session id is unknown, so it cannot be resumed")?;
    let before = fingerprint(&agent.dir);
    let log = run_dir.join(format!("feedback-{}.log", agent.index));
    let stderr = fs::File::create(&log).map_err(|e| e.to_string())?;
    let mut path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_owned))
        .map(|bin| bin.into_os_string())
        .unwrap_or_default();
    if let Some(rest) = std::env::var_os("PATH") {
        path.push(":");
        path.push(rest);
    }
    let out_path = run_dir.join(format!("feedback-{}.out", agent.index));
    let stdout = fs::File::create(&out_path).map_err(|e| e.to_string())?;
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&agent.dir)
        .env("AVER_HOME", home.root())
        .env("PATH", path)
        .env(FEEDBACK_ENV, "1")
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", argv[0]))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("no answer within {} s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let answer = fs::read_to_string(&out_path).unwrap_or_default();
    let _ = fs::remove_file(&out_path);
    if !status.success() || answer.trim().is_empty() {
        return Err(format!(
            "it exited {} without an answer; see {}",
            status.code().unwrap_or(-1),
            log.display()
        ));
    }
    let changed_files = fingerprint(&agent.dir) != before;
    let rating = rating(&answer);
    let file = run_dir.join(format!("feedback-{}.md", agent.index));
    let text = format!(
        "# Feedback from agent {} ({}) of run {run_id}\n\nRating: {}\n{}\n## Questions\n\n{QUESTIONS}\n\n## Answer\n\n{}\n",
        agent.index,
        agent.template,
        rating.map_or("none given".to_owned(), |r| format!("{r}/10")),
        if changed_files {
            "\nWarning: files in the workspace changed while the agent answered.\n"
        } else {
            ""
        },
        answer.trim_end()
    );
    fs::write(&file, text).map_err(|e| e.to_string())?;
    Ok(AgentFeedback {
        rating,
        file,
        at: Timestamp::now(),
        changed_files,
    })
}

/// `#1 7/10, #2 8/10`: the ratings of a run's agents that answered.
pub fn ratings(run: &RunState) -> Option<String> {
    let parts: Vec<String> = run
        .agents
        .iter()
        .filter_map(|a| {
            let f = a.feedback.as_ref()?;
            Some(format!(
                "#{} {}",
                a.index,
                f.rating
                    .map_or("no rating".to_owned(), |r| format!("{r}/10"))
            ))
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratings_are_read_from_the_labelled_line_first() {
        assert_eq!(rating("Helpful.\nRATING: 7/10\nOther: 3/10"), Some(7));
        assert_eq!(rating("**Rating:** 8 / 10"), Some(8));
        assert_eq!(rating("I'd give it 9/10 overall"), Some(9));
        assert_eq!(rating("RATING: 11/10"), None);
        assert_eq!(rating("no number here"), None);
    }
}
