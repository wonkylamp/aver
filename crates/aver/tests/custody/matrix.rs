//! The crash matrix (custody plan §1.3): one row per operation, crash
//! point and action, with what must hold after the fault and after the
//! operation runs again. An invariant a row lists as broken names the work
//! package that fixes it; the row must then be changed with the fix, since
//! the audit compares the broken set exactly.

use std::fs;
use std::path::PathBuf;

use super::audit::{audit, Allowed, Inv, RepoState};
use super::world::{git, text, World, CRITERION, DOUBLE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    /// `aver accept` in a workspace directory verified PASS.
    Accept,
    /// `aver setup --tool codex --tool gemini --yes` (AGENTS.md and GEMINI.md).
    Integrate,
    /// `aver setup --remove --yes` after that integration.
    Remove,
    /// `aver accept 1` of a finished run.
    AcceptRun,
    /// `aver discard` of a finished run.
    Discard,
    /// `aver run --tasks` of one scripted task.
    Batch,
    /// `aver accept 1 --pr` of a finished run, with a stub `gh`.
    AcceptPr,
    /// `aver combine 1 2` of a batch of two passing tasks.
    Combine,
    /// `aver rerun 1` of a batch's finished task.
    Rerun,
    /// `aver rerun 2 --onto 1`: task 1 is accepted first.
    RerunOnto,
    /// `aver verify` in a solved workspace directory never verified.
    Verify,
    /// `aver workspace new --count 2`.
    WorkspaceNew,
    /// `aver workspace rm` of an unchanged directory.
    Rm,
    /// `aver gc --accepted` over two accepted directories kept with
    /// `--keep` and the orphan of a killed `workspace new`.
    Gc,
    /// `aver restore calc.sh test/neg.tests` over unsynced edits.
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exit {
    Ok,
    /// The fault's own end: 137 for `abort`, 2 for `enospc`.
    Fault,
    /// Exit 2, saying what to do.
    Explains,
    /// Exit 1: the verify ran and the work is not done.
    NotDone,
}

struct Row {
    op: Op,
    fault: &'static str,
    after_fault: (Exit, &'static [(Inv, &'static str)]),
    after_rerun: (Exit, &'static [(Inv, &'static str)]),
}

const fn row(
    op: Op,
    fault: &'static str,
    after_fault: (Exit, &'static [(Inv, &'static str)]),
    after_rerun: (Exit, &'static [(Inv, &'static str)]),
) -> Row {
    Row {
        op,
        fault,
        after_fault,
        after_rerun,
    }
}

const CLEAN: &[(Inv, &str)] = &[];
/// The export's fast-import ref, until the next export sweeps it.
const TEMP_REF: &[(Inv, &str)] = &[(
    Inv::I1,
    "refs/aver/export/<pid>-<start>-<ulid> stays until the next export",
)];

const fn ok_again(op: Op, fault: &'static str) -> Row {
    row(op, fault, (Exit::Fault, CLEAN), (Exit::Ok, CLEAN))
}

const ROWS: &[Row] = &[
    // Row 5: accept in a workspace directory. Run again, every one exits 0
    // (K3): the branch at the verified snapshot is adopted, and once the
    // directory is released `aver accept` in the repository reports it.
    ok_again(Op::Accept, "accept.journaled=abort"),
    row(
        Op::Accept,
        "accept.committed=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Accept,
        "accept.committed=enospc",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Accept,
        "accept.decided=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Accept,
        "export.streamed=abort",
        (Exit::Fault, TEMP_REF),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Accept,
        "export.mapped=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Accept,
        "export.ref=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    ok_again(Op::Accept, "accept.released=abort"),
    // A run's attempt, at every point.
    ok_again(Op::AcceptRun, "accept.journaled=abort"),
    ok_again(Op::AcceptRun, "accept.committed=abort"),
    ok_again(Op::AcceptRun, "accept.decided=abort"),
    row(
        Op::AcceptRun,
        "export.streamed=abort",
        (Exit::Fault, TEMP_REF),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::AcceptRun,
        "export.mapped=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::AcceptRun,
        "export.ref=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    ok_again(Op::AcceptRun, "accept.released=abort"),
    // The decision is saved last; run again, the accept says it is done.
    ok_again(Op::AcceptRun, "accept.saved=abort"),
    // accept --pr: the pull request `gh` opened is adopted, not opened
    // twice.
    ok_again(Op::AcceptPr, "pr.pushed=abort"),
    ok_again(Op::AcceptPr, "pr.created=abort"),
    // combine: run again, it resumes from its plan.
    ok_again(Op::Combine, "combine.merged=abort"),
    ok_again(Op::Combine, "combine.verified=abort"),
    ok_again(Op::Combine, "combine.accepted#1=abort"),
    ok_again(Op::Combine, "combine.accepted#2=abort"),
    ok_again(Op::Combine, "combine.committed=abort"),
    ok_again(Op::Combine, "combine.exported=abort"),
    // rerun: the batch names the new run only once its run.json exists.
    ok_again(Op::Rerun, "rerun.recorded=abort"),
    ok_again(Op::RerunOnto, "rerun.onto_accepted=abort"),
    // Row 12: integrate and its removal.
    row(
        Op::Integrate,
        "integrate.recorded#1=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Integrate,
        "integrate.recorded#2=enospc",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Integrate,
        "integrate.file#1=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Integrate,
        "integrate.file#2=enospc",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Remove,
        "remove.file#1=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Remove,
        "remove.file#2=enospc",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Remove,
        "remove.record=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    // Row 15: host state files.
    row(
        Op::Discard,
        "run.save=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Discard,
        "run.save=enospc",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Batch,
        "batch.save#1=enospc",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    // Row 7 (K4): workspace lifecycle. A directory cut before its row is
    // an orphan gc trashes; one cut after it is live and whole.
    row(
        Op::WorkspaceNew,
        "materialize.cloned#2=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::WorkspaceNew,
        "materialize.cloned#1=enospc",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::WorkspaceNew,
        "materialize.view#1=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::WorkspaceNew,
        "materialize.recorded#2=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    // .gitignore and workspace.toml in one write, then BRIEF.md, per
    // directory: #1 is the first directory's workspace file, #2 its brief
    // once its row is in.
    row(
        Op::WorkspaceNew,
        "meta.write#1=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::WorkspaceNew,
        "meta.write#2=enospc",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Rm,
        "rm.discarded=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Rm,
        "release.trashed=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Gc,
        "gc.released#1=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Gc,
        "gc.released#2=enospc",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Gc,
        "gc.trashed=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Restore,
        "restore.tmp=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Restore,
        "restore.tmp=enospc",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Restore,
        "restore.partial#1=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    // Row 14: verify, and the sync before it. A kill anywhere leaves a
    // workspace the rerun syncs and verifies PASS; the recovery after the
    // unclean stop drops the stat cache.
    row(
        Op::Verify,
        "sync.blobs=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Verify,
        "sync.committed=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Verify,
        "sync.committed=enospc",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Verify,
        "verify.planned=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Verify,
        "proc.spawned=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    // A full disk while a check runs: an ENV entry, not done, never the
    // change's failure (K2); the rerun passes.
    row(
        Op::Verify,
        "proc.spawned=enospc",
        (Exit::NotDone, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    // A kill while a side's slot is cloned or swapped: the rerun reclones a
    // slot with no source record and finishes a journaled swap.
    row(
        Op::Verify,
        "slot.cloned=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Verify,
        "swap.journaled=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Verify,
        "swap.partial=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Verify,
        "swap.done=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Verify,
        "verify.before_receipt=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
    row(
        Op::Verify,
        "verify.after_receipt=abort",
        (Exit::Fault, CLEAN),
        (Exit::Ok, CLEAN),
    ),
];

const AGENTS_FILES: &[&str] = &["AGENTS.md", "GEMINI.md"];

fn allowed(op: Op) -> Allowed {
    match op {
        Op::Accept | Op::AcceptRun | Op::Combine | Op::RerunOnto => Allowed {
            branches: vec!["aver/"],
            ..Allowed::default()
        },
        Op::AcceptPr => Allowed {
            branches: vec!["aver/"],
            pushes: true,
            ..Allowed::default()
        },
        Op::Integrate | Op::Remove => Allowed {
            paths: AGENTS_FILES.to_vec(),
            ..Allowed::default()
        },
        Op::Discard
        | Op::Batch
        | Op::Rerun
        | Op::Verify
        | Op::WorkspaceNew
        | Op::Rm
        | Op::Restore => Allowed::default(),
        Op::Gc => Allowed {
            branches: vec!["aver/"],
            ..Allowed::default()
        },
    }
}

const NEGATE: (&str, &str) = ("negate numbers", CRITERION);
const DOUBLE_TASK: (&str, &str) = ("double numbers", DOUBLE);

/// What the row's operation runs on, prepared without faults.
struct Setup {
    cwd: PathBuf,
    args: Vec<String>,
    /// A file edited after its directory's last sync, and what it holds:
    /// no fault and no rerun may lose it (I4).
    edited: Option<(PathBuf, Vec<u8>)>,
}

fn setup(w: &mut World, op: Op) -> Setup {
    let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect();
    match op {
        Op::Accept => Setup {
            cwd: w.passing_workspace(),
            args: s(&["accept"]),
            edited: None,
        },
        Op::Verify => Setup {
            cwd: w.solved_workspace(),
            args: s(&["verify"]),
            edited: None,
        },
        Op::Integrate => Setup {
            cwd: w.repo.clone(),
            args: s(&["setup", "--tool", "codex", "--tool", "gemini", "--yes"]),
            edited: None,
        },
        Op::Remove => {
            w.ok(
                &w.repo,
                &["setup", "--tool", "codex", "--tool", "gemini", "--yes"],
            );
            Setup {
                cwd: w.repo.clone(),
                args: s(&["setup", "--remove", "--yes"]),
                edited: None,
            }
        }
        Op::AcceptRun => {
            w.finished_run();
            Setup {
                cwd: w.repo.clone(),
                args: s(&["accept", "1"]),
                edited: None,
            }
        }
        Op::Discard => {
            w.finished_run();
            Setup {
                cwd: w.repo.clone(),
                args: s(&["discard"]),
                edited: None,
            }
        }
        Op::AcceptPr => {
            w.finished_run();
            w.stub_gh();
            Setup {
                cwd: w.repo.clone(),
                args: s(&["accept", "1", "--pr"]),
                edited: None,
            }
        }
        Op::Combine => {
            w.batch(&[NEGATE, DOUBLE_TASK]);
            Setup {
                cwd: w.repo.clone(),
                args: s(&["combine", "1", "2"]),
                edited: None,
            }
        }
        Op::Rerun | Op::RerunOnto => {
            let tasks: &[(&str, &str)] = if op == Op::Rerun {
                &[NEGATE]
            } else {
                &[NEGATE, DOUBLE_TASK]
            };
            w.batch(tasks);
            let mut args = s(&["rerun"]);
            args.extend(s(if op == Op::Rerun {
                &["1"]
            } else {
                &["2", "--onto", "1"]
            }));
            args.extend([
                "--agent-cmd".into(),
                w.agent_cmd(),
                "--no-tui".into(),
                "--no-deps".into(),
            ]);
            Setup {
                cwd: w.repo.clone(),
                args,
                edited: None,
            }
        }
        Op::Batch => {
            let tasks = w.faults.join("tasks.txt");
            fs::write(&tasks, format!("negate numbers\n  - {CRITERION}\n")).unwrap();
            let agent = w.faults.join("agent.sh");
            fs::write(
                &agent,
                "cd \"$1\"\nprintf 'neg() { echo $((0 - $1)); }\\n' >> calc.sh\nprintf '[crit:neg] negates :: [ \"$(neg 2)\" = -2 ]\\n' > test/neg.tests\n",
            )
            .unwrap();
            Setup {
                cwd: w.repo.clone(),
                args: vec![
                    "run".into(),
                    "--tasks".into(),
                    tasks.display().to_string(),
                    "--agent-cmd".into(),
                    format!("/bin/sh {} {{dir}}", agent.display()),
                    "--no-tui".into(),
                    "--no-deps".into(),
                ],
                edited: None,
            }
        }
        Op::WorkspaceNew => Setup {
            cwd: w.repo.clone(),
            args: s(&["workspace", "new", "-c", CRITERION, "--count", "2"]),
            edited: Some(w.edited_workspace()),
        },
        Op::Rm => {
            let edited = w.edited_workspace();
            let dir = w.workspace();
            Setup {
                cwd: w.repo.clone(),
                args: vec!["workspace".into(), "rm".into(), dir.display().to_string()],
                edited: Some(edited),
            }
        }
        Op::Gc => {
            for _ in 0..2 {
                let dir = w.passing_workspace();
                w.accept(&dir, &["--keep"]);
            }
            let out = w.aver(
                &w.repo,
                &["workspace", "new", "-c", CRITERION],
                Some("materialize.cloned=abort"),
            );
            assert_eq!(out.status.code(), Some(137), "{}", text(&out));
            Setup {
                cwd: w.repo.clone(),
                args: s(&["gc", "--accepted"]),
                edited: Some(w.edited_workspace()),
            }
        }
        Op::Restore => {
            let (file, _) = w.edited_workspace();
            let dir = file.parent().unwrap().to_owned();
            super::world::solve(&dir);
            let bytes = fs::read(&file).unwrap();
            Setup {
                cwd: dir,
                args: s(&["restore", "calc.sh", "test/neg.tests"]),
                edited: Some((file, bytes)),
            }
        }
    }
}

fn exit_of(code: Option<i32>, fault: &str) -> Option<Exit> {
    match code {
        Some(0) => Some(Exit::Ok),
        Some(1) => Some(Exit::NotDone),
        Some(137) if fault.ends_with("=abort") => Some(Exit::Fault),
        Some(2) if fault.ends_with("=enospc") => Some(Exit::Fault),
        Some(2) => Some(Exit::Explains),
        _ => None,
    }
}

fn check(
    w: &World,
    row: &Row,
    stage: &str,
    out: &std::process::Output,
    fault: &str,
    expect: (Exit, &[(Inv, &str)]),
    before: &RepoState,
) -> Vec<String> {
    let mut problems = Vec::new();
    let exit = exit_of(out.status.code(), fault);
    if exit != Some(expect.0) {
        problems.push(format!(
            "{:?} {} {stage}: expected {:?}, got {}",
            row.op,
            row.fault,
            expect.0,
            text(out)
        ));
    }
    let broken = audit(w, before, &allowed(row.op));
    let expected: Vec<Inv> = expect.1.iter().map(|(i, _)| *i).collect();
    let got: Vec<Inv> = broken.keys().copied().collect();
    if got != expected {
        problems.push(format!(
            "{:?} {} {stage}: invariants broken {got:?}, expected {expected:?}: {broken:#?}",
            row.op, row.fault
        ));
    }
    problems
}

fn run_row(row: &Row) -> Vec<String> {
    let mut w = World::new();
    let original = RepoState::of(&w.repo);
    let op = row.op;
    let s = setup(&mut w, op);
    let before = RepoState::of(&w.repo);
    let args: Vec<&str> = s.args.iter().map(String::as_str).collect();
    let out = w.aver(&s.cwd, &args, Some(row.fault));
    let mut problems = Vec::new();
    if op == Op::WorkspaceNew {
        // Each directory is named as it lands, and only a live one.
        let printed: Vec<&str> = std::str::from_utf8(&out.stdout).unwrap().lines().collect();
        let listed = w.ok(&w.repo, &["workspace", "ls"]);
        for dir in &printed {
            if !listed
                .lines()
                .any(|l| l.contains("live") && l.ends_with(dir))
            {
                problems.push(format!("{} printed {dir}, which is not live", row.fault));
            }
        }
        if row.fault.starts_with("materialize.") && row.fault.contains("#2") && printed.is_empty() {
            problems.push(format!("{} named no directory before the cut", row.fault));
        }
    }
    problems.extend(check(
        &w,
        row,
        "after the fault",
        &out,
        row.fault,
        row.after_fault,
        &before,
    ));
    let still_edited = |stage: &str, problems: &mut Vec<String>| {
        if let Some((file, bytes)) = &s.edited {
            if op != Op::Restore && fs::read(file).ok().as_ref() != Some(bytes) {
                problems.push(format!(
                    "{} {stage}: lost the edit to {}",
                    row.fault,
                    file.display()
                ));
            }
        }
    };
    still_edited("after the fault", &mut problems);
    let cwd = if s.cwd.exists() {
        s.cwd.clone()
    } else {
        w.repo.clone()
    };
    let out = w.aver(&cwd, &args, None);
    problems.extend(check(
        &w,
        row,
        "after the rerun",
        &out,
        "",
        row.after_rerun,
        &before,
    ));
    still_edited("after the rerun", &mut problems);
    if op == Op::Restore {
        // However the first restore was cut, undoing back through both puts
        // the unsynced edit back.
        let (file, bytes) = s.edited.as_ref().unwrap();
        let mut undone = 0;
        while fs::read(file).ok().as_ref() != Some(bytes) && undone < 2 {
            let out = w.aver(&s.cwd, &["restore", "--undo"], None);
            if !out.status.success() {
                problems.push(format!("{} restore --undo: {}", row.fault, text(&out)));
                break;
            }
            undone += 1;
        }
        if fs::read(file).ok().as_ref() != Some(bytes) {
            problems.push(format!(
                "{} restore --undo did not put the edit back",
                row.fault
            ));
        }
    }
    if op == Op::Remove && out.status.success() {
        // Undone exactly: every file as it was before the integration.
        let now = RepoState::of(&w.repo);
        for path in AGENTS_FILES {
            if now.files.get(*path) != original.files.get(*path) {
                problems.push(format!("{} {path} is not as before Aver", row.fault));
            }
        }
    }
    if op == Op::Discard && !only_run(&w)["discarded"].as_bool().unwrap_or(false) {
        problems.push(format!("{} the run is not discarded", row.fault));
    }
    if matches!(op, Op::AcceptRun | Op::AcceptPr) {
        let run = only_run(&w);
        if run["decided"]["branch"].as_str().is_none() {
            problems.push(format!("{} the run records no decision", row.fault));
        }
        let journal = fs::read_dir(w.home.join("runs"))
            .unwrap()
            .flatten()
            .any(|d| d.path().join("accepting.json").exists());
        if journal {
            problems.push(format!("{} the accept's journal stays", row.fault));
        }
    }
    if op == Op::AcceptPr {
        let log = fs::read_to_string(w.faults.join("bin/gh.log")).unwrap_or_default();
        let created = log.lines().filter(|l| l.starts_with("pr create")).count();
        if created != 1 {
            problems.push(format!("{} gh pr create ran {created} times", row.fault));
        }
        if only_run(&w)["decided"]["pr"].as_str() != Some("https://github.com/o/r/pull/7") {
            problems.push(format!("{} the run records no pull request", row.fault));
        }
    }
    if op == Op::Combine {
        problems.extend(combined_once(&w, row.fault));
    }
    if op == Op::Accept
        && w.home
            .join("accepts")
            .read_dir()
            .is_ok_and(|mut d| d.next().is_some())
    {
        problems.push(format!("{} the accept's journal stays", row.fault));
    }
    if !aver::atomic::leftovers(&w.home.join("runs")).is_empty() {
        problems.push(format!("{} temporary files stay in runs/", row.fault));
    }
    problems
}

/// After a combine that resumed: one combine branch, both tasks' branches,
/// the export recorded and no plan left.
fn combined_once(w: &World, fault: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let branches = git(&w.repo, &["branch", "--format=%(refname:short)"]);
    let combines = branches
        .lines()
        .filter(|b| b.starts_with("aver/combine-"))
        .count();
    let tasks = branches
        .lines()
        .filter(|b| b.starts_with("aver/") && !b.starts_with("aver/combine-"))
        .count();
    if (combines, tasks) != (1, 2) {
        problems.push(format!("{fault} branches: {branches}"));
    }
    let records: Vec<String> = fs::read_dir(w.home.join("combines"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    if records.iter().any(|r| r.starts_with("pending-")) || records.len() != 1 {
        problems.push(format!("{fault} combine records: {records:?}"));
    }
    problems
}

fn only_run(w: &World) -> serde_json::Value {
    let dir = fs::read_dir(w.home.join("runs"))
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .path();
    serde_json::from_slice(&fs::read(dir.join("run.json")).unwrap()).unwrap()
}

fn is_ok(problems: &[String]) -> Result<(), String> {
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n\n"))
    }
}

fn rows_of(op: Op) -> impl Iterator<Item = &'static Row> {
    ROWS.iter().filter(move |r| r.op == op)
}

fn run_rows(op: Op) {
    let failures: Vec<String> = std::thread::scope(|s| {
        let handles: Vec<_> = rows_of(op).map(|r| s.spawn(move || run_row(r))).collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a row does not panic"))
            .collect()
    });
    is_ok(&failures).unwrap();
}

#[test]
fn k3_accept_retry_in_a_workspace() {
    run_rows(Op::Accept);
}

#[test]
fn k3_accept_retry_of_a_run() {
    run_rows(Op::AcceptRun);
}

#[test]
fn k3_pr_adopt() {
    run_rows(Op::AcceptPr);
}

#[test]
fn k3_combine_resume() {
    run_rows(Op::Combine);
}

#[test]
fn k3_rerun_record_rows() {
    run_rows(Op::Rerun);
    run_rows(Op::RerunOnto);
}

/// A rerun cut once the batch names its run: `aver status` shows the
/// task's last run and says the rerun did not start, and `aver accept 1`
/// accepts that run.
#[test]
fn k3_rerun_record() {
    let mut w = World::new();
    let before = RepoState::of(&w.repo);
    let s = setup(&mut w, Op::Rerun);
    let args: Vec<&str> = s.args.iter().map(String::as_str).collect();
    let out = w.aver(&w.repo, &args, Some("rerun.recorded=abort"));
    assert_eq!(out.status.code(), Some(137), "{}", text(&out));
    let status = w.aver(&w.repo, &["status"], None);
    let said = String::from_utf8_lossy(&status.stdout);
    assert!(status.status.success(), "{}", text(&status));
    assert!(said.contains("the rerun did not start"), "{said}");
    w.ok(&w.repo, &["accept", "1"]);
    let broken = audit(&w, &before, &allowed(Op::RerunOnto));
    assert!(broken.is_empty(), "{broken:#?}");
}

/// An edit that lands in the directory once the accept committed it: the
/// branch holds the snapshot the receipt verified, and the directory, which
/// holds more, is kept.
#[test]
fn k3_edit_at_commit() {
    let w = World::new();
    let before = RepoState::of(&w.repo);
    let dir = w.passing_workspace();
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_aver"))
        .arg("accept")
        .current_dir(&dir)
        .env("AVER_HOME", &w.home)
        .env("AVER_FAULT_DIR", &w.faults)
        .env("AVER_FAULT", "accept.committed=pause")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("AVER_URL")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    w.paused("accept.committed");
    let calc = dir.join("calc.sh");
    let mut edited = fs::read_to_string(&calc).unwrap();
    edited.push_str("half() { echo $(($1 / 2)); }\n");
    fs::write(&calc, &edited).unwrap();
    w.resume("accept.committed");
    let out = child.wait_with_output().unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", text(&out));
    let branch = git(
        &w.repo,
        &["branch", "--format=%(refname:short)", "--list", "aver/*"],
    );
    let exported = git(&w.repo, &["show", &format!("{branch}:calc.sh")]);
    assert!(!exported.contains("half"), "{exported}");
    assert!(exported.contains("neg"), "{exported}");
    assert_eq!(fs::read_to_string(&calc).unwrap(), edited);
    assert!(said.contains("calc.sh changed after the commit"), "{said}");
    let broken = audit(&w, &before, &allowed(Op::Accept));
    assert!(broken.is_empty(), "{broken:#?}");
}

/// The fast-import ref of an export that died, and one an older Aver left,
/// are gone once another export runs; a live one stays.
#[test]
fn k3_temp_refs() {
    let w = World::new();
    let before = RepoState::of(&w.repo);
    let dir = w.passing_workspace();
    let out = w.aver(&dir, &["accept"], Some("export.streamed=abort"));
    assert_ne!(out.status.code(), Some(0), "{}", text(&out));
    let left = git(
        &w.repo,
        &["for-each-ref", "--format=%(refname)", "refs/aver/"],
    );
    assert!(left.starts_with("refs/aver/export/"), "{left:?}");
    let head = git(&w.repo, &["rev-parse", "HEAD"]);
    let old = "refs/aver/export/01ARZ3NDEKTSV4RRFFQ69G5FAV";
    git(&w.repo, &["update-ref", old, &head]);
    let live = format!(
        "refs/aver/export/{}-{}-01ARZ3NDEKTSV4RRFFQ69G5FAV",
        std::process::id(),
        aver::run::state::process_start(std::process::id()).unwrap_or(0)
    );
    git(&w.repo, &["update-ref", &live, &head]);
    w.ok(&dir, &["accept"]);
    let left = git(
        &w.repo,
        &["for-each-ref", "--format=%(refname)", "refs/aver/"],
    );
    assert_eq!(left, live, "only the live export's ref stays");
    git(&w.repo, &["update-ref", "-d", &live]);
    let broken = audit(&w, &before, &allowed(Op::Accept));
    assert!(broken.is_empty(), "{broken:#?}");
}

#[test]
fn integrate_and_remove() {
    run_rows(Op::Integrate);
    run_rows(Op::Remove);
}

#[test]
fn verify_and_sync() {
    run_rows(Op::Verify);
}

#[test]
fn workspace_lifecycle() {
    run_rows(Op::WorkspaceNew);
    run_rows(Op::Rm);
    run_rows(Op::Gc);
    run_rows(Op::Restore);
}

#[test]
fn host_state_files() {
    run_rows(Op::Discard);
    run_rows(Op::Batch);
}

/// Every crash point is cut by some row, or by `workdir.rs` for the
/// points of a checkout no command adopts yet.
#[test]
fn every_point_has_a_row() {
    let missing: Vec<&str> = aver::fault::POINTS
        .iter()
        .copied()
        .filter(|p| {
            !super::workdir::POINTS.contains(p)
                && !ROWS.iter().any(|r| {
                    let point = r.fault.split(['=', '#']).next().unwrap();
                    point == *p
                })
        })
        .collect();
    assert!(missing.is_empty(), "points no row cuts: {missing:?}");
}

/// A clean world passes the audit.
#[test]
fn a_clean_world_passes() {
    let w = World::new();
    let before = RepoState::of(&w.repo);
    let dir = w.passing_workspace();
    let broken = audit(&w, &before, &allowed(Op::Accept));
    assert!(broken.is_empty(), "{broken:#?}");
    w.ok(&dir, &["accept"]);
    let broken = audit(&w, &before, &allowed(Op::Accept));
    assert!(broken.is_empty(), "{broken:#?}");
}

/// A pause holds the operation at its point until the test lets it go.
#[test]
fn a_paused_operation_waits_for_the_test() {
    let w = World::new();
    let before = RepoState::of(&w.repo);
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_aver"))
        .args(["setup", "--tool", "codex", "--tool", "gemini", "--yes"])
        .current_dir(&w.repo)
        .env("AVER_HOME", &w.home)
        .env("AVER_FAULT_DIR", &w.faults)
        .env("AVER_FAULT", "integrate.file#1=pause")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid = w.paused("integrate.file");
    assert_eq!(pid, child.id());
    let now = RepoState::of(&w.repo);
    let written = AGENTS_FILES
        .iter()
        .filter(|p| now.files.get(**p) != before.files.get(**p))
        .count();
    assert_eq!(written, 1, "paused after the first file");
    w.resume("integrate.file");
    assert!(child.wait().unwrap().success());
}

/// A file torn by an older Aver goes back to what it was before Aver, and
/// a torn record no longer stops `--remove`.
#[test]
fn a_torn_file_or_record_goes_back_to_before() {
    let w = World::new();
    let original = RepoState::of(&w.repo);
    let as_before = |w: &World| {
        let now = RepoState::of(&w.repo);
        for path in AGENTS_FILES {
            assert_eq!(now.files.get(*path), original.files.get(*path), "{path}");
        }
    };
    w.ok(
        &w.repo,
        &["setup", "--tool", "codex", "--tool", "gemini", "--yes"],
    );
    let agents = w.repo.join("AGENTS.md");
    let whole = fs::read_to_string(&agents).unwrap();
    fs::write(&agents, &whole[..whole.len() / 2]).unwrap();
    w.ok(&w.repo, &["setup", "--remove", "--yes"]);
    as_before(&w);

    w.ok(
        &w.repo,
        &["setup", "--tool", "codex", "--tool", "gemini", "--yes"],
    );
    fs::write(w.repo.join(".git/aver/integrate.json"), "{\"files\": {").unwrap();
    let out = w.aver(&w.repo, &["setup", "--remove", "--yes"], None);
    assert!(out.status.success(), "{}", text(&out));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not a valid record"));
    as_before(&w);
}
