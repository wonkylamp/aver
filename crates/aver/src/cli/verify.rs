use std::process::ExitCode;
use std::time::Instant;

use anyhow::{bail, Context as _};
use aver::agents::AgentName;
use aver::keys::Home;
use aver::progress::Printer;
use aver::run::report::{agent_text, exit_code, note_once, with_handoff, ReportInput};
use aver::run::state::{AgentMarker, RunFile};
use aver::services::key_in;
use aver::verify::{Head, Request};
use aver::workspace::WorkspaceFile;
use aver_core::{AttemptId, Receipt, SnapshotId, TaskId, Timestamp, WorkspaceId};
use serde_json::json;

use super::workspace::Context;

#[derive(clap::Args)]
pub struct Args {
    /// The task to verify against. Inside a workspace directory, the
    /// workspace's own task.
    #[arg(long)]
    task: Option<TaskId>,
    /// Verify this workspace's current state against its base.
    #[arg(long, conflicts_with = "head")]
    workspace: Option<WorkspaceId>,
    /// Verify this snapshot.
    #[arg(long)]
    head: Option<SnapshotId>,
    /// With --head: the snapshot to compare against (default: the task's base).
    #[arg(long, requires = "head")]
    base: Option<SnapshotId>,
    /// Attach the receipt to this attempt.
    #[arg(long)]
    attempt: Option<AttemptId>,
    /// Run every check afresh instead of reading the run cache.
    #[arg(long)]
    fresh: bool,

    /// Run every check, also those the change does not affect (by default a
    /// check runs only when a changed file is in its scope, its lockfiles or
    /// its harness).
    #[arg(long, alias = "all-checks")]
    all: bool,
    /// Print no progress lines on stderr.
    #[arg(long, short)]
    quiet: bool,
    /// Print a progress line for every run that ends, not only for what is
    /// still running.
    #[arg(long, short, conflicts_with = "quiet")]
    verbose: bool,
    /// The key the receipt is issued to (default: admin, or the workspace's).
    #[arg(long)]
    key: Option<AgentName>,
    /// Inside a workspace: print the receipt, diagnostics and sync as JSON
    /// instead of the report.
    #[arg(long)]
    json: bool,
    /// Inside a workspace: print the latest verify's report again, marked
    /// stale when the directory changed since. Runs nothing, uses no verify,
    /// and exits as that verify did.
    #[arg(long, conflicts_with_all = ["fresh", "all", "head", "workspace", "json"])]
    last: bool,
    /// Inside a workspace: why your change rewrites tests or harness, shown
    /// to the reviewer beside the changes. Not checked; never changes the
    /// verdict.
    #[arg(long, conflicts_with_all = ["last", "head"])]
    reason: Option<String>,
}

/// `.aver/last-verify.json`: what the latest verify in a workspace
/// directory reported beyond its stored receipt (the sync and the timing),
/// for `aver verify --last`.
const LAST_PATH: &str = ".aver/last-verify.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct Last {
    receipt_id: aver_core::ReceiptId,
    #[serde(default)]
    sync: Option<aver::disk::SyncReport>,
    #[serde(default)]
    timing: Option<aver::verify::Timing>,
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<ExitCode> {
    if args.last {
        return last(args);
    }
    if args.workspace.is_none() && args.head.is_none() {
        return workspace_mode(args);
    }
    let Some(task) = args.task else {
        bail!("--task is required with --workspace or --head");
    };
    let key = key_in(home, args.key.as_ref())?;
    let head = match (args.workspace, args.head) {
        (Some(ws), _) => Head::Workspace(ws),
        (None, Some(head)) => Head::Snapshot {
            head,
            base: args.base,
        },
        (None, None) => unreachable!("workspace mode handles this"),
    };
    let request = Request {
        task,
        head,
        attempt: args.attempt,
        fresh: args.fresh,
        all: args.all,
        progress: None,
        ci: None,
        adhoc: None,
        reason: None,
    };
    let (rt, api) = super::open(home)?;
    let receipt: Receipt = rt.block_on(async {
        anyhow::Ok(
            api.services()
                .verifier
                .verify(key.id(), request)
                .await?
                .receipt,
        )
    })?;
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(ExitCode::SUCCESS)
}

/// `aver verify` inside a workspace directory: syncs the directory and
/// verifies it against the workspace's task. Prints the agent report;
/// exits 0 when done and 1 otherwise.
fn workspace_mode(args: Args) -> anyhow::Result<ExitCode> {
    let ctx = Context::require("pass --task with --workspace or --head")?;
    let task = args
        .task
        .or(ctx.file.task)
        .context("this workspace belongs to no task; pass --task")?;
    let key = match &args.key {
        Some(name) => key_in(&ctx.home, Some(name))?,
        None => ctx.key()?,
    };
    super::workspace::refuse_unresolved(&ctx)?;
    let started = Instant::now();
    let (rt, api) = super::open(&ctx.home)?;
    // The run's budget is a limit, not a report: verify N+1 of N runs
    // nothing, so an agent cannot buy a better verdict with extra verifies.
    if let (Some(run), Some(attempt)) = (RunFile::read(&ctx.root), ctx.file.attempt) {
        let used = rt
            .block_on(api.attempt(&key.signing_key, &attempt))?
            .receipts
            .len() as u32;
        if used >= run.max_verifies {
            bail!(
                "out of verifies: {used} of {} used; `aver check` is unlimited, `aver giveup \"<why>\"` ends the attempt",
                run.max_verifies
            );
        }
    }
    let _marker = AgentMarker::verifying(&ctx.root);
    let flags = aver::run::api::VerifyFlags {
        fresh: args.fresh,
        all: args.all,
        dry_run: false,
        adhoc: None,
        reason: args.reason.clone(),
    };
    let printer =
        (!args.quiet).then(|| Printer::at("aver verify", "natively").verbose(args.verbose));
    let outcome = aver::progress::printed(printer, |log| {
        rt.block_on(api.verify_with(&key.signing_key, &task, &ctx.file.workspace, flags, log))
    })?;
    api.services()
        .workspaces()
        .checkpoint(&ctx.file.workspace, &outcome.receipt.body);
    let budget = match (RunFile::read(&ctx.root), ctx.file.attempt) {
        (Some(run), Some(attempt)) => rt
            .block_on(api.attempt(&key.signing_key, &attempt))
            .ok()
            .map(|a| (a.receipts.len() as u32, run.max_verifies)),
        _ => None,
    };
    if args.json {
        let out = json!({
            "receipt_id": outcome.receipt_id,
            "receipt": outcome.receipt,
            "diagnostics": outcome.diagnostics,
            "sync": outcome.sync,
            "notices": outcome.notices,
            "timing": outcome.timing,
            "budget": budget.map(|(used, max)| json!({ "used": used, "max": max })),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    }
    if let Ok(json) = serde_json::to_vec(&Last {
        receipt_id: outcome.receipt_id,
        sync: outcome.sync.clone(),
        timing: outcome.timing.clone(),
    }) {
        let _ = WorkspaceFile::write_meta(&ctx.root, LAST_PATH, json);
    }
    // The JSON carries every notice; the report only what is new to this
    // workspace (§2.11).
    let notices = if args.json {
        outcome.notices.clone()
    } else {
        aver::seen::for_report(&ctx.root, outcome.notices.as_ref(), Timestamp::now())
    };
    let input = ReportInput {
        receipt_id: &outcome.receipt_id,
        receipt: &outcome.receipt,
        diagnostics: outcome.diagnostics.as_ref(),
        sync: outcome.sync.as_ref(),
        took: Some(started.elapsed()),
        budget,
        notices: notices.as_ref(),
        timing: outcome.timing.as_ref(),
    };
    if !args.json {
        let mut text = agent_text(&input);
        if RunFile::read(&ctx.root).is_none() {
            text = with_handoff(text, &input);
        }
        print!("{}", note_once(&ctx.root, text));
    }
    Ok(ExitCode::from(exit_code(&input)))
}

/// `aver verify --last`: the latest receipt's report again, from the store,
/// marked stale when the directory no longer holds the files it judged.
/// Syncs the directory to tell, and runs nothing else.
fn last(args: Args) -> anyhow::Result<ExitCode> {
    let ctx =
        Context::require("aver verify --last reprints the report of the workspace you are in")?;
    let attempt = ctx
        .file
        .attempt
        .context("this workspace belongs to no attempt, so it has no receipts")?;
    let key = match &args.key {
        Some(name) => key_in(&ctx.home, Some(name))?,
        None => ctx.key()?,
    };
    let (rt, api) = super::open(&ctx.home)?;
    let receipts = rt
        .block_on(api.attempt(&key.signing_key, &attempt))?
        .receipts;
    let Some(rid) = receipts.last() else {
        println!("No verify yet in this workspace: run `aver verify`.");
        return Ok(ExitCode::from(1));
    };
    let v = rt.block_on(api.receipt(rid))?;
    let used = receipts.len() as u32;
    let head = rt.block_on(api.snapshot(&key.signing_key, &ctx.file.workspace))?;
    let stale = head != v.receipt.body.head_snapshot;
    let saved: Option<Last> = std::fs::read(ctx.root.join(LAST_PATH))
        .ok()
        .and_then(|b| serde_json::from_slice::<Last>(&b).ok())
        .filter(|l| l.receipt_id == v.receipt_id);
    let (sync, timing) = saved.map_or((None, None), |l| (l.sync, l.timing));
    let budget = RunFile::read(&ctx.root).map(|run| (used, run.max_verifies));
    let input = ReportInput {
        receipt_id: &v.receipt_id,
        receipt: &v.receipt,
        diagnostics: v.diagnostics.as_ref(),
        sync: sync.as_ref(),
        took: None,
        budget,
        notices: None,
        timing: timing.as_ref(),
    };
    let mut text = agent_text(&input);
    if RunFile::read(&ctx.root).is_none() {
        text = with_handoff(text, &input);
    }
    if stale {
        text = aver::run::report::mark_stale(&text);
    }
    print!("{}", note_once(&ctx.root, text));
    Ok(ExitCode::from(exit_code(&input)))
}
