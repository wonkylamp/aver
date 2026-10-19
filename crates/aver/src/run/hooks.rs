//! Claude Code hooks (§2.10, §2.11). The Stop hook runs the verify gate inside the
//! agent's session: when the agent tries to stop while it is not done and
//! verifies remain, the hook blocks the stop and hands it the report, so it
//! keeps its context. It blocks at most [`MAX_CONSECUTIVE_BLOCKS`] times in
//! a row (Claude marks a stop that follows a block with
//! `stop_hook_active`), and the verify budget bounds it overall; after
//! that the supervisor decides.
//!
//! In a workspace made outside `aver run` (a Claude Code worktree session
//! or subagent, §2.12) the hooks brief the agent at start
//! ([`start_context`]) and gate its stop without a sandbox
//! ([`worktree_stop`]).

use std::path::Path;

use aver_core::rank::GateDecision;
use aver_core::Verdict;

use crate::workspace::WorkspaceFile;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Consecutive blocks before the hook lets the agent stop anyway.
pub const MAX_CONSECUTIVE_BLOCKS: u32 = 2;
/// `.aver/stop-blocks`: how many times in a row the hook blocked.
pub const BLOCKS_PATH: &str = ".aver/stop-blocks";

/// What the hook does, and the consecutive block count to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopAction {
    Allow,
    Block,
}

/// Decides the Stop hook from the gate. `blocks` is how many times in a
/// row it blocked before; a stop without `stop_hook_active` starts a new
/// series. Returns the action and the new count.
pub fn stop(active: bool, blocks: u32, gate: GateDecision) -> (StopAction, u32) {
    let blocks = if active { blocks } else { 0 };
    match gate {
        GateDecision::Continue if blocks < MAX_CONSECUTIVE_BLOCKS => {
            (StopAction::Block, blocks + 1)
        }
        _ => (StopAction::Allow, 0),
    }
}

/// The hook's stdout for a block: `{"decision":"block","reason":…}`.
pub fn block_json(reason: &str) -> String {
    json!({ "decision": "block", "reason": reason }).to_string()
}

pub fn read_blocks(dir: &Path) -> u32 {
    std::fs::read_to_string(dir.join(BLOCKS_PATH))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

pub fn write_blocks(dir: &Path, n: u32) {
    let _ = WorkspaceFile::write_meta(dir, BLOCKS_PATH, n.to_string());
}

/// The part of every Claude Code hook input the workspace hooks read
/// outside `aver run` (SessionStart, Stop, SubagentStop).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SessionInput {
    #[serde(default)]
    pub hook_event_name: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<std::path::PathBuf>,
    /// Set when the hook fires inside a subagent.
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub stop_hook_active: bool,
}

impl SessionInput {
    /// Whether the event is a subagent's (SubagentStop) rather than the
    /// session's.
    pub fn subagent_event(&self) -> bool {
        self.hook_event_name
            .as_deref()
            .is_some_and(|e| e.starts_with("Subagent"))
    }

    /// Who the stop gate counts blocks for: the subagent, else the session.
    pub fn stopper(&self) -> String {
        self.agent_id
            .as_deref()
            .or(self.session_id.as_deref())
            .unwrap_or("session")
            .to_owned()
    }
}

/// The label of the workspace directory `aver hook worktree-create` makes
/// for Claude Code's worktree `name` (`wt-<name>`, made path safe). A
/// subagent's worktree is named `agent-<agent_id>`, so its hooks find the
/// directory from their `agent_id` even when their `cwd` is the parent's.
pub fn worktree_label(name: &str) -> String {
    let label: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(48)
        .collect();
    format!("wt-{}", label.trim_matches('-'))
}

/// The worktree name Claude Code gives the subagent `agent_id`.
pub fn subagent_worktree(agent_id: &str) -> String {
    format!("agent-{agent_id}")
}

/// How the verify gate outside `aver run` treats a stop, from `[agents]
/// stop` in the base's `aver.toml`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StopMode {
    /// Block once per version of the files, at most [`MAX_GATE_BLOCKS`]
    /// times, never right after a block.
    #[default]
    Once,
    /// `stop = "strict"`: block every stop until a `PASS` or `REVIEW`
    /// receipt covers the files; `aver giveup` is the only other way out.
    /// Claude Code ends the turn anyway after its own cap of consecutive
    /// blocks (8 in Claude Code 2.1).
    Strict,
}

impl StopMode {
    /// The mode `aver.toml`'s text sets; [`StopMode::Once`] for text that
    /// does not parse or sets no other.
    pub fn from_config(text: &str) -> StopMode {
        let strict = text
            .parse::<toml::Table>()
            .ok()
            .and_then(|t| t.get("agents")?.get("stop")?.as_str().map(str::to_owned));
        match strict.as_deref() {
            Some("strict") => StopMode::Strict,
            _ => StopMode::Once,
        }
    }
}

/// `.aver/stop-gate.json`: the verify gate's blocks, per session or subagent.
pub const GATE_PATH: &str = ".aver/stop-gate.json";
/// Blocks of one stopper in one workspace before the gate always allows.
pub const MAX_GATE_BLOCKS: u32 = 3;

/// What a session or subagent in a workspace directory is told when it
/// starts: the short form of `.aver/BRIEF.md`. `branch` is the branch its
/// commits land on, when the directory has a Git view.
pub fn start_context(dir: &Path, branch: Option<&str>) -> String {
    let mut out = format!(
        "Aver: you are working in an Aver workspace directory, {}. Aver checks your work there before it counts.\n\
         - DONE MEANS `aver verify` prints DONE: PASS. If you changed an existing test on purpose it prints NEEDS HUMAN REVIEW: REVIEW: stop and say why; only a person approves it (never run `aver accept --approve-review` yourself). Work that changes no file needs no verify.\n\
         - CHECK AS YOU GO with `aver check`: it runs the tests your change affects natively, on your copy and on the original code, reruns failing tests to spot flaky ones, and ends with a prediction (\"Verify should PASS\" or what stops it). It is free; prefer it to running the test runner by hand.\n\
         - VERIFY TO FINISH with `aver verify` once `aver check` predicts a pass: it runs the checks on both sides in clean copies and signs a receipt; exit code 0 means done, and its last line repeats the verdict (`aver verify --last` reprints it). Verify before you report back; stopping with unverified changes gets one reminder.\n\
         - If you broke things and cannot see how, `aver restore --last-green` puts the directory back to your last check or verify without a NEW failure.\n\
         - {}\n",
        dir.display(),
        super::brief::TOOLS_LINE
    );
    match branch {
        Some(branch) => out.push_str(&format!(
            "- COMMIT only when whoever started you asks for commits: they land on the branch {branch}. Report the commit SHA when you do.\n"
        )),
        None => {
            out.push_str("- Aver records your files itself; `aver diff` shows what it recorded.\n")
        }
    }
    out.push_str("- Blocked by something you cannot fix? `aver giveup \"<reason>\"`, then stop.\nThe full brief is .aver/BRIEF.md in that directory.");
    out
}

/// What a session started in the checkout of a repository set up with
/// Aver is told: an instruction to work in a workspace directory, which
/// nothing enforces.
pub const ROOT_CONTEXT: &str = "Aver: this repository is set up with Aver, and agents work in Aver workspace directories, not in this checkout. Before your first edit, run `aver workspace new` here and make every change in the directory it prints: `cd` there or use absolute paths under it. There `aver check` previews and `aver verify` decides when the work is done. Reading files and answering questions need no workspace; if the user asks you to edit this checkout itself, do that instead.";

/// SessionStart's stdout adding `text` to the context.
pub fn start_json(event: &str, text: &str) -> String {
    json!({
        "hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": text,
        }
    })
    .to_string()
}

/// One stopper's blocks in a workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateRecord {
    pub blocks: u32,
    /// The head snapshot of the last block, as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
}

/// The directory's state as the gate sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkState {
    /// No file differs from the base.
    Unchanged,
    Changed {
        /// The head snapshot, as text.
        head: String,
        files: u32,
        /// The newest verdict of a receipt for exactly `head`, if any.
        verified: Option<Verdict>,
        /// The newest receipt's verdict when it was for another version.
        stale: Option<Verdict>,
    },
}

/// The verify gate for a session or subagent in a workspace made outside
/// `aver run` (a Claude Code worktree, `aver workspace new`). It blocks a
/// stop with changed files and no `PASS` or `REVIEW` receipt for them, at
/// most once per version of the files and [`MAX_GATE_BLOCKS`] times in
/// all, and never when the stop follows a block (`active`) or after `aver
/// giveup`; [`StopMode::Strict`] blocks every such stop, until a receipt
/// covers the files or `aver giveup`. `look` reads the directory; it is
/// called only when the answer depends on it. Returns the action, the
/// record to store and the reason.
pub fn worktree_stop<E>(
    mode: StopMode,
    active: bool,
    gave_up: bool,
    record: &GateRecord,
    look: impl FnOnce() -> Result<WorkState, E>,
) -> Result<(StopAction, GateRecord, Option<String>), E> {
    let allow = |r: &GateRecord| (StopAction::Allow, r.clone(), None);
    let strict = mode == StopMode::Strict;
    if gave_up || (!strict && (active || record.blocks >= MAX_GATE_BLOCKS)) {
        return Ok(allow(record));
    }
    let WorkState::Changed {
        head,
        files,
        verified,
        stale,
    } = look()?
    else {
        return Ok(allow(record));
    };
    if matches!(verified, Some(Verdict::Pass | Verdict::Review))
        || (!strict && record.last.as_deref() == Some(head.as_str()))
    {
        return Ok(allow(record));
    }
    let files = match files {
        1 => "1 file".to_owned(),
        n => format!("{n} files"),
    };
    let why = match (verified, stale) {
        (Some(v), _) => format!(
            "The last `aver verify` of these files said {}, so they are not done.",
            v.as_str()
        ),
        (None, Some(v)) => format!(
            "The last `aver verify` ({}) was of an earlier version: you changed files since.",
            v.as_str()
        ),
        (None, None) => "No `aver verify` has run on them.".to_owned(),
    };
    let again = if strict {
        "This repository's aver.toml makes the gate strict: every stop is blocked until a verify passes or you give up."
    } else {
        "This reminder comes once; stopping again is allowed."
    };
    let reason = format!(
        "NOT DONE: you changed {files} in this workspace, and no passing `aver verify` covers them yet. {why}\nDone means `aver verify` prints DONE: PASS (or NEEDS HUMAN REVIEW: REVIEW, which a person approves). Run `aver check` (seconds, predicts the verdict) and fix what it reports, then `aver verify` (both sides, a signed receipt). If you cannot finish, run `aver giveup \"<reason>\"`. {again}\nNOT DONE: verify before you stop"
    );
    Ok((
        StopAction::Block,
        GateRecord {
            blocks: record.blocks + 1,
            last: Some(head),
        },
        Some(reason),
    ))
}

/// The gate records of a workspace, by stopper.
pub fn read_gate(dir: &Path) -> std::collections::BTreeMap<String, GateRecord> {
    std::fs::read_to_string(dir.join(GATE_PATH))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn write_gate(dir: &Path, records: &std::collections::BTreeMap<String, GateRecord>) {
    if let Ok(text) = serde_json::to_string(records) {
        let _ = WorkspaceFile::write_meta(dir, GATE_PATH, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed(verified: Option<Verdict>, stale: Option<Verdict>) -> WorkState {
        WorkState::Changed {
            head: "snap_b".into(),
            files: 2,
            verified,
            stale,
        }
    }

    fn gate(
        active: bool,
        gave_up: bool,
        record: &GateRecord,
        state: Option<WorkState>,
    ) -> (StopAction, GateRecord, Option<String>) {
        worktree_stop(StopMode::Once, active, gave_up, record, || {
            state.ok_or("the directory was read when the answer did not need it")
        })
        .unwrap()
    }

    #[test]
    fn the_worktree_gate_blocks_once_for_unverified_changes() {
        let fresh = GateRecord::default();
        // Unchanged: never blocks.
        let (a, r, _) = gate(false, false, &fresh, Some(WorkState::Unchanged));
        assert_eq!((a, r), (StopAction::Allow, fresh.clone()));
        // Changed, never verified: one block naming check and verify.
        let (a, r, why) = gate(false, false, &fresh, Some(changed(None, None)));
        assert_eq!(a, StopAction::Block);
        assert_eq!(
            r,
            GateRecord {
                blocks: 1,
                last: Some("snap_b".into())
            }
        );
        let why = why.unwrap();
        assert!(why.contains("you changed 2 files"), "{why}");
        assert!(why.contains("No `aver verify` has run"), "{why}");
        assert!(
            why.contains("`aver check`") && why.contains("`aver verify`"),
            "{why}"
        );
        assert!(why.contains("aver giveup"), "{why}");
        // Stopping again right after the block, or later on the same files:
        // allowed, without reading the directory when the block was just now.
        assert_eq!(gate(true, false, &r, None).0, StopAction::Allow);
        assert_eq!(
            gate(false, false, &r, Some(changed(None, None))).0,
            StopAction::Allow
        );
        // A stale receipt (an earlier version) blocks a new version, and says so.
        let other = GateRecord {
            blocks: 1,
            last: Some("snap_a".into()),
        };
        let (a, r2, why) = gate(
            false,
            false,
            &other,
            Some(changed(None, Some(Verdict::Pass))),
        );
        assert_eq!(a, StopAction::Block);
        assert_eq!(r2.blocks, 2);
        assert!(why.unwrap().contains("earlier version"));
        // A failing receipt for exactly these files blocks, naming its verdict.
        let (a, _, why) = gate(
            false,
            false,
            &fresh,
            Some(changed(Some(Verdict::FailOwn), Some(Verdict::FailOwn))),
        );
        assert_eq!(a, StopAction::Block);
        assert!(why.unwrap().contains("said FAIL_OWN"));
    }

    #[test]
    fn the_worktree_gate_allows_a_fresh_pass_a_giveup_and_after_the_limit() {
        let fresh = GateRecord::default();
        for v in [Verdict::Pass, Verdict::Review] {
            let (a, r, why) = gate(false, false, &fresh, Some(changed(Some(v), Some(v))));
            assert_eq!((a, r, why), (StopAction::Allow, fresh.clone(), None));
        }
        assert_eq!(gate(false, true, &fresh, None).0, StopAction::Allow);
        let spent = GateRecord {
            blocks: MAX_GATE_BLOCKS,
            last: Some("snap_a".into()),
        };
        assert_eq!(gate(false, false, &spent, None).0, StopAction::Allow);
        let mut record = fresh;
        for n in 0..MAX_GATE_BLOCKS + 2 {
            let state = WorkState::Changed {
                head: format!("snap_{n}"),
                files: 1,
                verified: None,
                stale: None,
            };
            let (a, r, _) = gate(false, false, &record, Some(state));
            let expect = if n < MAX_GATE_BLOCKS {
                StopAction::Block
            } else {
                StopAction::Allow
            };
            assert_eq!(a, expect, "stop {n}");
            record = r;
        }
        assert_eq!(record.blocks, MAX_GATE_BLOCKS);
    }

    #[test]
    fn subagent_worktrees_map_to_their_labels() {
        assert_eq!(
            worktree_label(&subagent_worktree("ab31cda7418696159")),
            "wt-agent-ab31cda7418696159"
        );
        assert_eq!(worktree_label("bold oak/a3f2"), "wt-bold-oak-a3f2");
        let input: SessionInput = serde_json::from_str(
            r#"{"hook_event_name":"SubagentStop","session_id":"s1","agent_id":"a1","stop_hook_active":true,"cwd":"/r"}"#,
        )
        .unwrap();
        assert!(input.subagent_event() && input.stop_hook_active);
        assert_eq!(input.stopper(), "a1");
        let main: SessionInput =
            serde_json::from_str(r#"{"hook_event_name":"Stop","session_id":"s1"}"#).unwrap();
        assert!(!main.subagent_event());
        assert_eq!(main.stopper(), "s1");
    }

    #[test]
    fn start_context_says_done_check_verify_and_commit() {
        let text = start_context(Path::new("/w/wt-x"), Some("aver/wt-x"));
        for want in [
            "/w/wt-x",
            "DONE MEANS `aver verify` prints DONE: PASS",
            "`aver check`",
            "VERIFY TO FINISH",
            "aver/wt-x",
            "aver giveup",
            ".aver/BRIEF.md",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        assert!(!start_context(Path::new("/w"), None).contains("git commit"));
        assert_eq!(
            start_json("SessionStart", "x"),
            r#"{"hookSpecificOutput":{"additionalContext":"x","hookEventName":"SessionStart"}}"#
        );
    }

    #[test]
    fn blocks_twice_in_a_row_then_allows() {
        let c = GateDecision::Continue;
        assert_eq!(stop(false, 0, c), (StopAction::Block, 1));
        assert_eq!(stop(true, 1, c), (StopAction::Block, 2));
        assert_eq!(stop(true, 2, c), (StopAction::Allow, 0));
        // A fresh stop (not right after a block) starts over.
        assert_eq!(stop(false, 2, c), (StopAction::Block, 1));
        for g in [
            GateDecision::Done,
            GateDecision::GiveUp,
            GateDecision::BudgetExhausted,
        ] {
            assert_eq!(stop(false, 0, g), (StopAction::Allow, 0));
        }
    }

    #[test]
    fn the_strict_gate_blocks_until_a_pass_or_a_giveup() {
        assert_eq!(StopMode::from_config(""), StopMode::Once);
        assert_eq!(
            StopMode::from_config("[agents]\nstop = \"once\"\n"),
            StopMode::Once
        );
        assert_eq!(StopMode::from_config("{oops"), StopMode::Once);
        let strict = StopMode::from_config("[agents]\nstop = \"strict\"\n");
        assert_eq!(strict, StopMode::Strict);
        let mut record = GateRecord::default();
        for n in 0..MAX_GATE_BLOCKS + 3 {
            // The same files, right after a block, past the once gate's limit.
            let (a, r, why) = worktree_stop(strict, n > 0, false, &record, || {
                Ok::<_, ()>(changed(None, None))
            })
            .unwrap();
            assert_eq!(a, StopAction::Block, "stop {n}");
            assert!(why.unwrap().contains("strict"));
            record = r;
        }
        for v in [Verdict::Pass, Verdict::Review] {
            let (a, ..) = worktree_stop(strict, true, false, &record, || {
                Ok::<_, ()>(changed(Some(v), None))
            })
            .unwrap();
            assert_eq!(a, StopAction::Allow);
        }
        let (a, ..) = worktree_stop(strict, true, false, &record, || {
            Ok::<_, ()>(changed(Some(Verdict::FailOwn), None))
        })
        .unwrap();
        assert_eq!(a, StopAction::Block, "a failing verify does not end it");
        let (a, ..) = worktree_stop(strict, true, true, &record, || Err(())).unwrap();
        assert_eq!(a, StopAction::Allow, "aver giveup does");
        let (a, ..) = worktree_stop(strict, true, false, &record, || {
            Ok::<_, ()>(WorkState::Unchanged)
        })
        .unwrap();
        assert_eq!(a, StopAction::Allow, "and so does an unchanged directory");
    }

    #[test]
    fn block_json_golden() {
        assert_eq!(
            block_json("UNPROVEN: not done yet.\nNext: prove neg."),
            r#"{"decision":"block","reason":"UNPROVEN: not done yet.\nNext: prove neg."}"#
        );
    }
}
