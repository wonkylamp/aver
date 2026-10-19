//! `aver check [<criterion>…] [--all]` inside a workspace directory: what
//! `aver verify` will say (ARCHITECTURE.md §2.3.1, §3.4).
//!
//! By default it is a dry run of the verify engine itself: the same plan,
//! the same native runs in the same pools under the same run cache keys,
//! and a receipt signed by a throwaway key that is neither stored nor
//! counted against the verify budget. So its prediction is the verdict a
//! verify of the same tree signs, and that verify, run next, starts no run.
//! Named criteria narrow the report, not the runs.
//!
//! With `--combined` it also dry-runs the verify of a trial merge of the
//! workspace with other tasks' current work, against the same task and
//! base, and says what passes here and fails there.

use std::process::ExitCode;

use anyhow::{bail, Context as _};
use aver::keys::Home;
use aver::run::api::Verification;
use aver::run::report::{
    agent_text, exit_code, fence, note_once, one_line, ReportInput, FENCE_NOTE,
};
use aver_core::{BaseStatus, CriterionId, HeadStatus, ReceiptBody, Verdict};

use super::workspace::Context;
use aver::adhoc::{self, Outcome};
use aver::verify::Side;

#[derive(clap::Args)]
pub struct Args {
    /// Only these criteria's tagged tests: a fast answer for them, not a
    /// prediction of the whole verify (default: every test, every criterion).
    criteria: Vec<CriterionId>,
    /// List every failure, flaky and pre-existing test in full.
    #[arg(long, conflicts_with = "criteria")]
    all: bool,
    /// Print the preview as JSON.
    #[arg(long)]
    json: bool,
    /// Also dry-run the verify of a trial merge of your work with other
    /// tasks' current work in this repository.
    #[arg(long)]
    combined: bool,
    /// Run every check, also those your change does not affect (as
    /// `aver verify --all` does).
    #[arg(long)]
    all_checks: bool,
    /// Print no progress lines on stderr.
    #[arg(long, short)]
    quiet: bool,
    /// Print a progress line for every run that ends, not only for what is
    /// still running.
    #[arg(long, short, conflicts_with = "quiet")]
    verbose: bool,

    /// Run the test files near your change (changed test files, files named
    /// after a changed file, files that name it) with the runner that takes
    /// them, on your copy and on the original code.
    #[arg(long, conflicts_with_all = ["criteria", "combined", "command"])]
    near: bool,
    /// Your own test command, after `--`: Aver runs it on your copy, the
    /// original code and the original plus your test files, and says which
    /// failures are yours (`aver check -- cargo test div`).
    #[arg(last = true, conflicts_with_all = ["criteria", "combined"])]
    command: Vec<String>,
    /// Write `aver.toml` even for a command Aver judges by its exit code
    /// alone (no per-test report). A person's decision: such a check never
    /// shows which tests a change breaks.
    #[arg(long, requires = "command")]
    allow_exit_only: bool,
}

pub fn run(_: &Home, args: Args) -> anyhow::Result<ExitCode> {
    let ctx = Context::require("aver check previews the workspace you are in")?;
    let task = ctx
        .file
        .task
        .context("this workspace belongs to no task, so there is nothing to check against")?;
    super::workspace::refuse_unresolved(&ctx)?;
    let key = ctx.key()?;
    if args.near || !args.command.is_empty() {
        return adhoc(&ctx, &key, task, args);
    }
    dry_run(&ctx, &key, task, args)
}

/// The default `aver check`: a dry run of the verify engine (§3.4).
fn dry_run(
    ctx: &Context,
    key: &aver::keys::AgentKey,
    task: aver_core::TaskId,
    args: Args,
) -> anyhow::Result<ExitCode> {
    let started = std::time::Instant::now();
    let (rt, api) = super::open(&ctx.home)?;
    let v = dry_verify(&rt, &api, ctx, key, task, &args, args.all_checks, None)?;
    api.services()
        .workspaces()
        .checkpoint(&ctx.file.workspace, &v.receipt.body);
    let combined = if args.combined {
        Some(combined(&rt, &api, ctx, key, task, &v)?)
    } else {
        None
    };
    let ids: Vec<&str> = v
        .receipt
        .body
        .criteria
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    if let Some(unknown) = args.criteria.iter().find(|c| !ids.contains(&c.as_str())) {
        bail!(
            "the task has no criterion {unknown}; its criteria are: {}",
            if ids.is_empty() {
                "none".to_owned()
            } else {
                ids.join(", ")
            }
        );
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "dry_run": true,
                "verdict": v.receipt.body.verdict,
                "receipt": v.receipt,
                "diagnostics": v.diagnostics,
                "sync": v.sync,
                "timing": v.timing,
                "combined": combined.as_ref().map(|c| c.as_ref().map(Combined::json)),
            }))?
        );
    }
    // Shares the seen-set with verify and the hook (§2.11).
    let notices = if args.json {
        None
    } else {
        aver::seen::for_report(&ctx.root, v.notices.as_ref(), aver_core::Timestamp::now())
    };
    let input = ReportInput {
        receipt_id: &v.receipt_id,
        receipt: &v.receipt,
        diagnostics: v.diagnostics.as_ref(),
        sync: v.sync.as_ref(),
        took: Some(started.elapsed()),
        budget: None,
        notices: notices.as_ref(),
        timing: v.timing.as_ref(),
    };
    if !args.json {
        let mut text = dry_text(&input, &args.criteria, &v);
        if let Some(c) = &combined {
            let max = if args.all { usize::MAX } else { MAX_LISTED };
            let section = match c {
                Some(c) => c.text(max),
                None => "\nCombined: no other task works in this repository on your base now, so there is nothing to merge with.\n".to_owned(),
            };
            // The last line stays the verify's prediction: the trial merge
            // says nothing about this workspace's verify.
            let at = text.trim_end().rfind('\n').map_or(0, |i| i + 1);
            text.insert_str(at, &format!("{}\n", section.trim_end()));
        }
        print!("{}", note_once(&ctx.root, text));
    }
    Ok(ExitCode::from(exit_code(&input)))
}

/// Most tests and paths a `--combined` section lists without `--all`.
const MAX_LISTED: usize = 10;

/// What `aver check --combined` found: the dry verify of the trial merge of
/// this workspace with other tasks' current work, set against this
/// workspace's own dry verify.
struct Combined {
    trial: aver::coord::Trial,
    /// Tests on the merge that carry a tag of the task's criteria or that
    /// this change added (the base never ran them), and how many pass.
    yours: usize,
    yours_pass: usize,
    /// Entries that pass in this workspace's dry verify and fail in the
    /// merge's, with the merge's failure excerpt.
    breaks: Vec<(String, Option<String>)>,
}

/// `--combined`: `None` when no other task works in the repository on this
/// workspace's base.
fn combined(
    rt: &tokio::runtime::Runtime,
    api: &aver::run::api::Api,
    ctx: &Context,
    key: &aver::keys::AgentKey,
    task: aver_core::TaskId,
    mine: &Verification,
) -> anyhow::Result<Option<Combined>> {
    let body = &mine.receipt.body;
    let Some(trial) =
        rt.block_on(api.trial(&key.signing_key, &ctx.file.workspace, &body.head_snapshot))?
    else {
        return Ok(None);
    };
    let merged = rt.block_on(api.verify_head_dry(
        &key.signing_key,
        &task,
        &trial.tree,
        &body.base_snapshot,
    ))?;
    let (yours, yours_pass, breaks) = compare(body, &merged.receipt.body);
    Ok(Some(Combined {
        trial,
        yours,
        yours_pass,
        breaks,
    }))
}

type Compared = (usize, usize, Vec<(String, Option<String>)>);

/// Sets the merge's entries against this workspace's: how many of its own
/// tests ran and pass on the merge, and what passes here and fails there.
fn compare(mine: &ReceiptBody, merged: &ReceiptBody) -> Compared {
    let tags = aver_core::tag::Tags::new(mine.criteria.iter().map(|c| &c.id));
    let (mut yours, mut pass, mut breaks) = (0, 0, Vec::new());
    for e in merged.checks.iter() {
        let id = e.id.as_str();
        let test = id.split_once("::").map(|(_, t)| t);
        if test.is_some_and(|t| t.starts_with('(')) {
            continue;
        }
        let here = mine.checks.iter().find(|m| m.id == e.id);
        let new_here = here.is_some_and(|m| m.base == BaseStatus::NotApplicable);
        if test.is_some() && (new_here || tags.carried(id).next().is_some()) {
            yours += 1;
            if e.head == HeadStatus::Pass {
                pass += 1;
            }
        }
        if here.is_some_and(|m| m.head == HeadStatus::Pass) && e.head != HeadStatus::Pass {
            breaks.push((
                id.to_owned(),
                e.detail.as_ref().map(|d| d.as_str().to_owned()),
            ));
        }
    }
    (yours, pass, breaks)
}

impl Combined {
    fn text(&self, max: usize) -> String {
        let t = &self.trial;
        let mut out = String::from(
            "\nCombined with other tasks' current work (as of their last sync), on a trial merge:\n",
        );
        let mut lines = vec![format!("with task {}", t.with.join(", task "))];
        if !t.merged.is_empty() {
            lines.push(format!("merged line by line: {}", listed(&t.merged, max)));
        }
        if !t.conflicts.is_empty() {
            lines.push(format!(
                "conflicts, so the trial keeps your version: {}",
                listed(&t.conflicts, max)
            ));
        }
        out.push_str(&fence(&lines.join("\n")));
        out.push_str(&format!(
            "Your tests (tagged, or new in your change): {} of {} pass on the merge.\n",
            self.yours_pass, self.yours
        ));
        if self.breaks.is_empty() {
            out.push_str("Nothing that passes on your copy fails on the merge.\n");
            return out;
        }
        out.push_str(&format!(
            "Fail on the merge but pass on your copy ({}; your change and theirs disagree there: adapt your code to theirs, or reserve that code with `aver lease` and leave the rest to them):\n",
            self.breaks.len()
        ));
        let mut text = String::new();
        for (entry, detail) in self.breaks.iter().take(max) {
            text.push_str(&format!("{}\n", one_line(entry)));
            for line in detail.iter().flat_map(|d| d.lines().take(3)) {
                text.push_str(&format!("  {}\n", one_line(line)));
            }
        }
        if self.breaks.len() > max {
            text.push_str(&format!("… {} more\n", self.breaks.len() - max));
        }
        out.push_str(&fence(&text));
        out
    }

    fn json(&self) -> serde_json::Value {
        let t = &self.trial;
        serde_json::json!({
            "with": t.with,
            "merged": t.merged,
            "conflicts": t.conflicts,
            "yours_ran": self.yours,
            "yours_pass": self.yours_pass,
            "breaks": self.breaks.iter().map(|(e, d)| serde_json::json!({ "test": e, "detail": d })).collect::<Vec<_>>(),
        })
    }
}

/// Up to `max` of `names`, one line each, then how many more.
fn listed(names: &[String], max: usize) -> String {
    let shown: Vec<String> = names.iter().take(max).map(|n| one_line(n)).collect();
    match names.len().saturating_sub(max) {
        0 => shown.join(", "),
        more => format!("{} and {more} more", shown.join(", ")),
    }
}

/// The verify report of a dry run, worded as the prediction it is: what a
/// verify of this tree will sign, and that it will reuse these runs.
pub fn dry_text(input: &ReportInput<'_>, focus: &[CriterionId], v: &Verification) -> String {
    let text = agent_text(input);
    let verdict = v.receipt.body.verdict;
    let done = matches!(verdict, Verdict::Pass | Verdict::Review);
    let mut out = String::new();
    let lines: Vec<&str> = text.lines().collect();
    let last = lines.len().saturating_sub(1);
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("Hand-off:") || line.starts_with("Verifies:") {
            continue;
        }
        let line = if i == 0 || (i == last && lines.len() > 2) {
            predicted(line)
        } else if done && line.starts_with("Next: nothing") {
            "Next: run `aver verify` for the receipt; it reuses these runs and starts none."
                .to_owned()
        } else {
            (*line).to_owned()
        };
        out.push_str(&line);
        out.push('\n');
        if i == 0 {
            let mut note = "Dry run of `aver verify`: the same runs, natively; nothing signed or stored, no verify used.".to_owned();
            if !focus.is_empty() {
                let names: Vec<&str> = focus.iter().map(|c| c.as_str()).collect();
                note.push_str(&format!(
                    " Asked about {}: see Criteria below.",
                    names.join(", ")
                ));
            }
            out.push_str(&note);
            out.push('\n');
        }
    }
    out
}

/// A verify headline or last line, as a prediction: `DONE: PASS…` becomes
/// `Verify should PASS…`, `NEEDS HUMAN REVIEW: REVIEW…` `Verify should give
/// REVIEW…`, `NOT DONE: FAIL_OWN…` `Verify would stop at FAIL_OWN…`.
fn predicted(line: &str) -> String {
    if let Some(rest) = line.strip_prefix("NEEDS HUMAN REVIEW: ") {
        format!("Verify should give {rest}")
    } else if let Some(rest) = line.strip_prefix("DONE: ") {
        format!("Verify should {rest}")
    } else if let Some(rest) = line.strip_prefix("NOT DONE: ") {
        format!("Verify would stop at {rest}")
    } else {
        line.to_owned()
    }
}

/// `aver check -- <cmd>` and `aver check --near` (§2.7.1): the agent's own
/// test command as the one check of a verify dry run; commands joined by
/// `&&` each as a check of their own. In a repository whose `aver.toml`
/// names no check, the command becomes its contract (§1.13.2).
fn adhoc(
    ctx: &Context,
    key: &aver::keys::AgentKey,
    task: aver_core::TaskId,
    args: Args,
) -> anyhow::Result<ExitCode> {
    let (rt, api) = super::open(&ctx.home)?;
    if args.near {
        let diff = rt.block_on(api.diff(&key.signing_key, &ctx.file.workspace))?;
        let n = near(ctx, diff)?;
        let tried = attempt(&rt, &api, ctx, key, task, &args, &n.argv, n.cwd)?;
        let mut text = adhoc::text(&tried.outcome);
        let at = text.find('\n').map_or(text.len(), |i| i + 1);
        text.insert_str(at, &format!("Near: {}.\n", n.note));
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&tried.json(Some(&n.note)))?
            );
        } else {
            print!("{}", note_once(&ctx.root, text));
        }
        return Ok(ExitCode::from(u8::from(tried.outcome.broke())));
    }
    let cwd = here(&ctx.root)?;
    let bootstrap = !ctx.root.join(aver::env::CONFIG_PATH).exists();
    let mut tried = Vec::new();
    for argv in adhoc::segments(&args.command) {
        tried.push(attempt(
            &rt,
            &api,
            ctx,
            key,
            task,
            &args,
            &argv,
            cwd.clone(),
        )?);
    }
    if bootstrap {
        let refusal = tried.iter().find_map(|t| match t.made.check.report {
            None if args.allow_exit_only => None,
            None => Some(t.exit_only_refusal()),
            Some(_) => t.refusal(&api),
        });
        if let Some(refusal) = refusal {
            print!("{}", note_once(&ctx.root, refusal));
            return Ok(ExitCode::from(2));
        }
    }
    let broke = tried.iter().any(|t| t.outcome.broke());
    if args.json {
        let shown: Vec<serde_json::Value> = tried.iter().map(|t| t.json(None)).collect();
        match shown.as_slice() {
            [one] => println!("{}", serde_json::to_string_pretty(one)?),
            _ => println!("{}", serde_json::to_string_pretty(&shown)?),
        }
    }
    if bootstrap {
        let written = write_contract(ctx, &api, &tried)?;
        if !args.json {
            print!("{written}");
        }
    }
    if !args.json {
        for t in &tried {
            print!("{}", note_once(&ctx.root, adhoc::text(&t.outcome)));
        }
    }
    Ok(ExitCode::from(u8::from(broke)))
}

/// One command's dry run and what it showed.
struct Tried {
    argv: Vec<String>,
    made: adhoc::Adhoc,
    outcome: Outcome,
    v: Verification,
}

#[allow(clippy::too_many_arguments)]
fn attempt(
    rt: &tokio::runtime::Runtime,
    api: &aver::run::api::Api,
    ctx: &Context,
    key: &aver::keys::AgentKey,
    task: aver_core::TaskId,
    args: &Args,
    argv: &[String],
    cwd: Option<aver_core::Path>,
) -> anyhow::Result<Tried> {
    let dir = cwd
        .as_ref()
        .map_or(ctx.root.clone(), |c| ctx.root.join(c.as_str()));
    let manifest = std::fs::read(dir.join("package.json")).ok();
    let lockfile = std::fs::read(dir.join("package-lock.json")).ok();
    let made = adhoc::check_for(argv, cwd, manifest.as_deref(), lockfile.as_deref())?;
    let v = dry_verify(
        rt,
        api,
        ctx,
        key,
        task,
        args,
        false,
        Some(made.check.clone()),
    )?;
    let transplant = adhoc::TransplantRuns::of(&side_runs(api, &v, Side::Transplant));
    let command = argv
        .iter()
        .map(|w| aver::ci::shell_quote(w))
        .collect::<Vec<_>>()
        .join(" ");
    let outcome = Outcome::of(
        command,
        made.runner,
        &v.receipt.body,
        v.diagnostics.as_ref(),
        &transplant,
    );
    Ok(Tried {
        argv: argv.to_vec(),
        made,
        outcome,
        v,
    })
}

/// Lines of the original code's output a refusal shows.
const REFUSAL_LINES: usize = 15;

impl Tried {
    fn is_typecheck(&self) -> bool {
        self.made
            .check
            .report
            .as_ref()
            .is_some_and(|r| r.format == aver_core::ReportFormat::Tsc)
    }

    /// How many tests the original code ran, and how many of them failed,
    /// in its first run.
    fn base(&self, api: &aver::run::api::Api) -> (usize, usize, Option<aver_core::CheckRun>) {
        let run = side_runs(api, &self.v, Side::Base).into_iter().next();
        let ran = run.as_ref().map_or(0, |r| r.tests.len());
        let failed = run.as_ref().map_or(0, |r| {
            r.tests
                .iter()
                .filter(|t| {
                    t.status != aver_core::TestStatus::Pass
                        && t.status != aver_core::TestStatus::Skip
                })
                .count()
        });
        (ran, failed, run)
    }

    /// Why this command cannot become the repository's contract: it ran no
    /// test on the original code, or every one failed there. A type check
    /// reports errors, not tests, and is never refused.
    fn refusal(&self, api: &aver::run::api::Api) -> Option<String> {
        if self.is_typecheck() {
            return None;
        }
        let (ran, failed, run) = self.base(api);
        let why = match (ran, failed) {
            (0, _) => "ran no test Aver could read on the original code",
            (n, f) if f == n => "failed every test on the original code",
            _ => return None,
        };
        let output = run
            .and_then(|r| api.services().store.get_raw(&r.output).ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let lines: Vec<&str> = output.lines().collect();
        let tail = lines[lines.len().saturating_sub(REFUSAL_LINES)..].join("\n");
        Some(format!(
            "aver.toml not written: `{}` {why}.\n{FENCE_NOTE}\n{}\n",
            self.outcome.command,
            fence(&tail)
        ))
    }

    /// Why an exit-code-only command is not written without
    /// `--allow-exit-only`, and how it could report per test.
    fn exit_only_refusal(&self) -> String {
        format!(
            "aver.toml not written: `{}` gives no per-test report, so Aver could judge it only by its exit code. To get per-test results, {}; to write it anyway, a person runs `aver check --allow-exit-only -- <cmd>`.\n",
            self.outcome.command,
            adhoc::exit_only_hint(&self.argv)
        )
    }

    fn json(&self, near: Option<&str>) -> serde_json::Value {
        let outcome = &self.outcome;
        let new: Vec<serde_json::Value> = outcome
            .new
            .iter()
            .map(|f| serde_json::json!({ "test": f.test, "detail": f.detail }))
            .collect();
        serde_json::json!({
            "dry_run": true,
            "command": outcome.command,
            "near": near,
            "runner": outcome.runner,
            "summary": outcome.summary(),
            "shows_change": outcome.shows,
            "new": new,
            "already_failing": outcome.already,
            "pass_without_change": outcome.pass_without,
            "flaky": outcome.flaky,
            "changed": outcome.changed,
            "env": outcome.env,
            "passing": outcome.passing,
            "check": self.made.check,
            "receipt": self.v.receipt,
            "diagnostics": self.v.diagnostics,
        })
    }
}

/// Directories `aver check -- <cmd>` keeps across runs when present.
const KEEP: &[&str] = &["node_modules", ".venv", "venv", "target"];
/// Lockfiles whose change reinstalls what [`KEEP`] holds.
const INPUTS: &[&str] = &[
    "package-lock.json",
    "npm-shrinkwrap.json",
    "uv.lock",
    "requirements.txt",
    "Cargo.lock",
    "go.sum",
];

/// Writes the commands `tried` as the repository's `aver.toml` contract,
/// each test command with a floor of 90% of the tests the original code
/// ran, and says what it wrote.
fn write_contract(
    ctx: &Context,
    api: &aver::run::api::Api,
    tried: &[Tried],
) -> anyhow::Result<String> {
    let mut ids: Vec<String> = Vec::new();
    for t in tried {
        let stem = if t.is_typecheck() {
            "typecheck"
        } else {
            "test"
        };
        let mut id = stem.to_owned();
        let mut n = 2;
        while ids.contains(&id) {
            id = format!("{stem}-{n}");
            n += 1;
        }
        ids.push(id);
    }
    let written: Vec<adhoc::Written<'_>> = tried
        .iter()
        .zip(&ids)
        .map(|(t, id)| adhoc::Written {
            id,
            check: &t.made.check,
            min_tests: t.made.check.report.as_ref().map(|_| {
                if t.is_typecheck() {
                    std::num::NonZeroU32::MIN
                } else {
                    adhoc::floor(t.base(api).0)
                }
            }),
        })
        .collect();
    let present = |names: &[&'static str]| -> Vec<&'static str> {
        names
            .iter()
            .copied()
            .filter(|n| ctx.root.join(n).exists())
            .collect()
    };
    let toml = adhoc::contract_toml(&written, &present(KEEP), &present(INPUTS));
    let path = ctx.root.join(aver::env::CONFIG_PATH);
    aver::atomic::write(&path, toml.as_bytes(), aver::atomic::Mode::Keep)
        .with_context(|| format!("writing {}", path.display()))?;
    let floors: Vec<String> = written
        .iter()
        .map(|w| match w.min_tests {
            Some(n) if w.check.report.is_some() => format!("{} (min_tests {n})", w.id),
            _ => format!("{} (exit code)", w.id),
        })
        .collect();
    Ok(format!(
        "Wrote aver.toml: {}. Commit it with your change; `aver verify` now runs it.\n\n",
        floors.join(", ")
    ))
}

/// The directory the agent is in, relative to the workspace root (`None`
/// at the root).
fn here(root: &std::path::Path) -> anyhow::Result<Option<aver_core::Path>> {
    let cwd = std::env::current_dir()?;
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
    let rel = canon(&cwd)
        .strip_prefix(canon(root))
        .map(|r| r.to_string_lossy().into_owned())
        .unwrap_or_default();
    if rel.is_empty() {
        return Ok(None);
    }
    Ok(Some(rel.parse().with_context(|| {
        format!("{rel} is not a repository path")
    })?))
}

/// A verify dry run of the workspace against `task` (with `adhoc`, of that
/// one check), printing progress unless `--quiet` or `--json`.
#[allow(clippy::too_many_arguments)]
fn dry_verify(
    rt: &tokio::runtime::Runtime,
    api: &aver::run::api::Api,
    ctx: &Context,
    key: &aver::keys::AgentKey,
    task: aver_core::TaskId,
    args: &Args,
    all: bool,
    adhoc: Option<aver_core::CheckDef>,
) -> anyhow::Result<Verification> {
    let flags = aver::run::api::VerifyFlags {
        fresh: false,
        all,
        dry_run: true,
        adhoc,
        reason: None,
    };
    let printer = (!args.quiet && !args.json)
        .then(|| aver::progress::Printer::at("aver check", "natively").verbose(args.verbose));
    Ok(aver::progress::printed(printer, |log| {
        rt.block_on(api.verify_with(&key.signing_key, &task, &ctx.file.workspace, flags, log))
    })?)
}

/// The dry run's runs of the ad-hoc check on `side`, read back from the
/// run cache by the ids its diagnostics list.
fn side_runs(api: &aver::run::api::Api, v: &Verification, side: Side) -> Vec<aver_core::CheckRun> {
    v.diagnostics
        .as_ref()
        .and_then(|d| d.timings.as_ref())
        .map(|t| {
            t.runs
                .iter()
                .filter(|r| r.side == side && r.check.as_str() == adhoc::ID)
                .filter_map(|r| api.services().store.get(&r.run).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Directories never searched for test files.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "vendor",
    "dist",
    "build",
    "venv",
    "__pycache__",
];
/// Most directory entries `--near` looks at.
const MAX_FILES: usize = 50_000;

/// `--near`: the narrowed command for the test files near the change.
fn near(ctx: &Context, diff: Vec<aver::run::api::DiffFile>) -> anyhow::Result<adhoc::Narrowed> {
    let changed: Vec<String> = diff
        .into_iter()
        .filter(|f| !matches!(f.change, aver::workspace::DiffChange::Deleted))
        .map(|f| f.path)
        .collect();
    if changed.is_empty() {
        bail!("nothing changed yet, so nothing is near the change; run `aver check` or `aver check -- <your test command>`");
    }
    let globs: Vec<aver_core::PathGlob> = aver_core::env::DEFAULT_TEST_GLOBS
        .iter()
        .chain(aver_core::env::ECOSYSTEM_TEST_GLOBS)
        .map(|g| g.parse().expect("default globs are valid"))
        .collect();
    let mut tests = Vec::new();
    let mut stack = vec![String::new()];
    let mut seen = 0;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(ctx.root.join(&dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_FILES {
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let rel = if dir.is_empty() {
                name.clone()
            } else {
                format!("{dir}/{name}")
            };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(rel);
                }
            } else if kind.is_file() && runnable_test(&rel, &globs) {
                tests.push(rel);
            }
        }
    }
    tests.sort();
    let read_text = |p: &str| {
        let bytes = std::fs::read(ctx.root.join(p)).ok()?;
        (bytes.len() <= 1 << 20).then(|| String::from_utf8_lossy(&bytes).into_owned())
    };
    let files = adhoc::near(&changed, &tests, &read_text);
    if files.is_empty() {
        bail!("no test file is near your change (none changed, none named after or naming a changed file); run `aver check` or `aver check -- <your test command>`");
    }
    let read = |p: &str| std::fs::read(ctx.root.join(p)).ok();
    adhoc::narrowed(&files, &changed, &read).context(
        "the test files near your change belong to no runner Aver can narrow; run `aver check -- <your test command>`",
    )
}

/// Whether `path` is a test file a runner runs: a test file by the default
/// patterns, in a language Aver narrows, and not a fixture, snapshot, mock
/// or setup file.
fn runnable_test(path: &str, globs: &[aver_core::PathGlob]) -> bool {
    const SUPPORT: &[&str] = &[
        "fixtures",
        "__fixtures__",
        "__mocks__",
        "__snapshots__",
        "testdata",
        "common",
    ];
    const SOURCE: &[&str] = &[
        ".rs", ".go", ".py", ".js", ".mjs", ".cjs", ".ts", ".mts", ".cts", ".jsx", ".tsx",
    ];
    let name = path.rsplit('/').next().unwrap_or(path);
    globs.iter().any(|g| g.covers(path))
        && SOURCE.iter().any(|e| name.ends_with(e))
        && !path.split('/').any(|s| SUPPORT.contains(&s))
        && !matches!(name, "conftest.py" | "__init__.py" | "mod.rs")
        && !name.starts_with("setup")
        && !name.contains(".setup.")
        && !name.contains(".config.")
}
