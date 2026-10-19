//! `aver run "<task>" --agents N`: N agents on one task, each in its own
//! workspace directory, watched live, ranked by receipt at the end; with
//! `--tasks`, `--issue` or `--issues`, a batch of tasks, one run each, and
//! the batch summary at the end (ARCHITECTURE.md §2.10).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context as _};
use aver::keys::Home;
use aver::place::proc::interrupt;
use aver::run::agent::{self, Template};
use aver::run::batch::schedule::{Sequence, SequenceMode};
use aver::run::batch::{self, input::BatchTask, BatchBudget, BatchSpec, FeedbackSpec, RetryOn};
use aver::run::compare;
use aver::run::state::Budget;
use aver::run::supervisor::{RunSpec, Supervisor};
use aver::run::view;
use aver::services::key_in;

#[derive(clap::Args)]
#[command(group(clap::ArgGroup::new("batch").multiple(true).args(["tasks", "issue", "issues"])))]
pub struct Args {
    /// What the agents should do, in plain words.
    #[arg(required_unless_present = "batch", conflicts_with = "batch")]
    task: Option<String>,
    /// Many tasks at once, one per line of FILE; indented `- id: text` and
    /// `protected: <glob>` lines under a task are its criteria.
    #[arg(long, value_name = "FILE", conflicts_with_all = ["issue", "issues"])]
    tasks: Option<PathBuf>,
    /// A GitHub issue as a task (read with gh; criteria from an aver block
    /// in its body); repeat for more.
    #[arg(long, value_name = "N")]
    issue: Vec<u64>,
    /// Every open GitHub issue with this label, each as a task.
    #[arg(long, value_name = "LABEL")]
    issues: Vec<String>,
    /// The options of a batch, boxed so `Args` stays small.
    #[command(flatten)]
    many: Box<BatchArgs>,
    /// How many agents (per task with many tasks). Default: 3 for one task,
    /// 1 per task with many.
    #[arg(short = 'n', long)]
    agents: Option<u32>,
    /// A criterion `id: text`; repeat for more. A test proves it when its
    /// name contains [crit:<id>] and it fails without the change.
    #[arg(
        short = 'c',
        long = "criterion",
        value_name = "ID: TEXT",
        conflicts_with = "batch"
    )]
    criterion: Vec<String>,
    /// Criteria from a file in the `aver ci` block format.
    #[arg(long, value_name = "FILE", conflicts_with_all = ["criterion", "batch"])]
    criteria: Option<PathBuf>,
    /// The agent CLI: claude or codex; repeat or separate with commas to mix
    /// them (agents take them in turn). Default: the first installed.
    #[arg(long, value_delimiter = ',')]
    agent: Vec<String>,
    /// A custom agent: a shell command with {dir}, {brief}, {brief_file}
    /// and {prompt}, each substituted quoted.
    #[arg(long, value_name = "TEMPLATE", conflicts_with = "agent")]
    agent_cmd: Option<String>,
    /// How to resume a custom agent with the verify report: a shell command
    /// with {feedback}, {dir} and {session_id}.
    #[arg(long, value_name = "TEMPLATE", requires = "agent_cmd")]
    resume_cmd: Option<String>,
    /// The Git repository.
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    /// The commit agents start from.
    #[arg(long, default_value = "HEAD")]
    base: String,
    /// The task's title (default: the task's first line).
    #[arg(long, conflicts_with = "batch")]
    title: Option<String>,
    /// Wall time per agent, e.g. 30m, 90s, 1h.
    #[arg(long, default_value = "30m", value_parser = parse_duration)]
    timeout: u64,
    /// Stop an agent that prints nothing for this long.
    #[arg(long, default_value = "10m", value_parser = parse_duration)]
    idle: u64,
    /// Verifies per agent (`aver check` does not count).
    #[arg(long, default_value_t = 10)]
    max_verifies: u32,
    /// Claude only: stop an agent after spending this many dollars.
    #[arg(long, value_name = "USD")]
    max_budget_usd: Option<f64>,
    /// The model to ask the agent CLI for.
    #[arg(long)]
    model: Option<String>,
    /// Let Claude run any command without asking (--dangerously-skip-permissions).
    #[arg(long)]
    yolo: bool,
    /// One line per event instead of the live table.
    #[arg(long)]
    no_tui: bool,
    /// Skip the Git view in each directory.
    #[arg(long)]
    no_git: bool,
    /// Do not clone the dependencies the repository has installed.
    #[arg(long)]
    no_deps: bool,
    /// Ports per agent ($PORT and the next ones).
    #[arg(long, default_value_t = 10)]
    ports: u16,
    /// Reserve these paths (globs) for the run: agents on other tasks in this
    /// repository are kept out while it works. Repeat for more.
    #[arg(long, value_name = "GLOB", conflicts_with = "batch")]
    lease: Vec<aver_core::PathGlob>,
    /// Run in the background, surviving the terminal: prints the run id and
    /// returns; `aver status` follows it and its log is in the run's
    /// directory.
    #[arg(long)]
    detach: bool,
    /// The run's (or batch's) id (set by --detach for the background
    /// process).
    #[arg(long, hide = true)]
    run_id: Option<String>,
    /// Print the run and its comparison as JSON at the end.
    #[arg(long)]
    json: bool,
    /// When the run ends, resume each agent and ask it Aver's fixed
    /// questions about Aver (read-only); answers go to the run's directory
    /// and the ratings to aver status.
    #[arg(long)]
    feedback: bool,
    /// When two or more attempts pass (or need review, or are unproven), run
    /// each of the top three's tests on the others' code before ranking
    /// (auto), or not (off).
    #[arg(long, value_enum, default_value = "auto")]
    cross: aver::run::cross::CrossMode,
}

/// The options only a batch (`--tasks`, `--issue`, `--issues`) takes.
#[derive(clap::Args)]
struct BatchArgs {
    /// With many tasks: at most K agents at a time, the other tasks queued
    /// (default: half the CPU cores, 2 to 8).
    #[arg(long, value_name = "K", requires = "batch")]
    parallel: Option<u32>,
    /// With many tasks: hold a queued task behind running ones it is
    /// predicted to change the same lines as (auto), or never (off).
    #[arg(long, value_enum, default_value = "auto", requires = "batch")]
    sequence: SequenceMode,
    /// The predicted-conflict score, 0-100, at or above which `--sequence
    /// auto` holds a task.
    #[arg(long, value_name = "SCORE", default_value_t = 50, requires = "batch",
          value_parser = clap::value_parser!(u8).range(0..=100))]
    sequence_threshold: u8,
    /// With many tasks: run a task again on a fresh run, up to N times, when
    /// it ends in one of --retry-on; the new run's brief says what the last
    /// one ran into.
    #[arg(long, value_name = "N", default_value_t = 0, requires = "batch")]
    retry: u32,
    /// The outcomes --retry retries: failed, no-result, timeout, budget,
    /// fail-own, unproven.
    #[arg(
        long,
        value_name = "OUTCOMES",
        value_delimiter = ',',
        default_value = "failed,no-result,timeout",
        requires = "retry"
    )]
    retry_on: Vec<RetryOn>,
    /// With many tasks: wall time for the whole batch (2h, 90m); when it
    /// runs out the running agents stop and their directories stay.
    #[arg(long, value_name = "DURATION", value_parser = parse_duration, requires = "batch")]
    batch_timeout: Option<u64>,
    /// With many tasks: verifies across the batch; once they are spent no
    /// task starts, and running ones finish.
    #[arg(long, value_name = "N", requires = "batch")]
    batch_verifies: Option<u32>,
    /// With many tasks: dollars across the batch, as agents report them
    /// (custom agents in .aver/usage.json); when spent, the running agents
    /// stop.
    #[arg(long, value_name = "USD", requires = "batch")]
    batch_usd: Option<f64>,
    /// With many tasks: do not accept a passing prerequisite (`after: #n`)
    /// for the tasks that build on it; they wait, and the summary says how
    /// to start them once you accept it.
    #[arg(long, requires = "batch")]
    no_auto_accept: bool,
}

/// `30m`, `90s`, `1h`; a bare number is minutes. Returns milliseconds.
pub(super) fn parse_duration(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = num
        .parse()
        .map_err(|_| format!("{s:?} is not a duration like 30m"))?;
    let secs = match unit {
        "" | "m" | "min" => n * 60,
        "s" => n,
        "h" => n * 3600,
        _ => return Err(format!("{s:?} is not a duration like 30m")),
    };
    Ok(secs * 1000)
}

fn templates(args: &Args, n: u32) -> anyhow::Result<Vec<Template>> {
    if let Some(cmd) = &args.agent_cmd {
        return Ok((0..n)
            .map(|_| Template::Custom {
                spawn: cmd.clone(),
                resume: args.resume_cmd.clone(),
            })
            .collect());
    }
    let names: Vec<String> = if args.agent.is_empty() {
        match agent::installed().first() {
            Some(name) => vec![(*name).to_owned()],
            None => bail!(
                "no agent CLI found on PATH: install Claude Code (claude) or Codex (codex), or pass --agent-cmd '<command using {{dir}} and {{brief}}>'"
            ),
        }
    } else {
        args.agent.clone()
    };
    let mut out = Vec::new();
    for i in 0..n as usize {
        let name = names[i % names.len()].trim();
        let mut t = match name {
            "claude" => Template::claude(args.yolo),
            "codex" => Template::codex(),
            other => bail!("unknown agent {other:?}: use claude or codex, or --agent-cmd"),
        };
        let bin = match &t {
            Template::Claude { bin, .. } | Template::Codex { bin, .. } => bin.clone(),
            Template::Custom { .. } => unreachable!(),
        };
        if agent::find_on_path(&bin).is_none() {
            bail!("{bin} is not installed (not on PATH)");
        }
        match &mut t {
            Template::Claude {
                model,
                max_budget_usd,
                ..
            } => {
                *model = args.model.clone();
                *max_budget_usd = args.max_budget_usd;
            }
            Template::Codex { model, .. } => *model = args.model.clone(),
            Template::Custom { .. } => {}
        }
        out.push(t);
    }
    Ok(out)
}

fn budget(args: &Args) -> Budget {
    Budget {
        timeout_ms: args.timeout,
        idle_ms: args.idle,
        max_verifies: args.max_verifies,
        max_usd: args.max_budget_usd,
    }
}

fn is_batch(args: &Args) -> bool {
    args.tasks.is_some() || !args.issue.is_empty() || !args.issues.is_empty()
}

/// The batch's tasks, from the tasks file or GitHub.
fn batch_tasks(args: &Args, repo: &std::path::Path) -> anyhow::Result<Vec<BatchTask>> {
    if let Some(file) = &args.tasks {
        let text =
            std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
        return Ok(batch::input::parse_tasks(&text)?);
    }
    let gh = agent::find_on_path("gh").ok_or_else(|| {
        anyhow::anyhow!(
            "--issue and --issues read issues with the GitHub CLI: install gh and run `gh auth login`"
        )
    })?;
    let issues = batch::input::issues(&gh, repo, &args.issue, &args.issues)?;
    let tasks = issues
        .iter()
        .map(batch::input::issue_task)
        .collect::<Result<Vec<_>, _>>()?;
    if tasks.is_empty() {
        bail!("no issues to work on");
    }
    Ok(tasks)
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<ExitCode> {
    let batch = is_batch(&args);
    let agents = args.agents.unwrap_or(if batch { 1 } else { 3 });
    if !batch && args.task.as_deref().is_none_or(|t| t.trim().is_empty()) {
        bail!("say what the agents should do: aver run \"<task>\", or many tasks with --tasks <file> or --issue <n>");
    }
    if agents == 0 || agents > 32 {
        bail!("--agents must be between 1 and 32");
    }
    if args.many.parallel == Some(0) {
        bail!("--parallel must be at least 1");
    }
    let templates = templates(&args, agents)?;
    aver::keys::init(home)?;
    if args.yolo
        && templates
            .iter()
            .any(|t| matches!(t, Template::Claude { .. }))
    {
        eprintln!(
            "aver: warning: --yolo lets each agent run any command on this machine as you, without asking. Workspace directories are copies, but commands are not sandboxed."
        );
    }
    let repo = std::fs::canonicalize(&args.repo)
        .with_context(|| format!("repository {}", args.repo.display()))?;
    if batch {
        // Read the tasks before detaching, so a bad file, issue or task
        // graph fails here, before anything is imported.
        let tasks = batch_tasks(&args, &repo)?;
        batch::graph::TaskGraph::new(&tasks)?;
        if args
            .many
            .batch_usd
            .is_some_and(|usd| usd.is_nan() || usd <= 0.0)
        {
            bail!("--batch-usd must be more than 0");
        }
        if args.detach {
            return detach(home, true);
        }
        return run_batch(home, &args, repo, tasks, templates);
    }
    let mut protected = Vec::new();
    let criteria: Vec<(String, String)> = match &args.criteria {
        Some(file) => {
            let text = std::fs::read_to_string(file)
                .with_context(|| format!("reading {}", file.display()))?;
            let spec = aver::ci::spec::from_file(&text)?;
            protected = spec.protected_paths.iter().cloned().collect();
            spec.criteria
                .iter()
                .map(|c| (c.id.to_string(), c.text.clone()))
                .collect()
        }
        None => args
            .criterion
            .iter()
            .map(|c| {
                let c = super::workspace::parse_criterion(c)?;
                anyhow::Ok((c.id.to_string(), c.text))
            })
            .collect::<anyhow::Result<_>>()?,
    };
    if args.detach {
        return detach(home, false);
    }
    let task = args.task.clone().unwrap_or_default();
    let exe = std::env::current_exe().context("locating the aver binary")?;
    let admin = key_in(home, None)?;
    let api = aver::run::api::Api::open(home)?;
    let spec = RunSpec {
        repo,
        rev: args.base.clone(),
        prompt: task,
        title: args.title.clone(),
        criteria,
        protected,
        agents: templates,
        budget: budget(&args),
        deps: !args.no_deps,
        git_view: !args.no_git,
        aver_bin: exe,
        ports: args.ports,
        leases: args.lease.clone(),
        id: args.run_id.clone(),
        batch: None,
        siblings: Vec::new(),
        stacked_on: None,
        context: None,
    };
    interrupt::graceful();
    let title = format!(
        "aver run · {} {} · {}",
        agents,
        if agents == 1 { "agent" } else { "agents" },
        view::short_rev(&args.base)
    );
    let mut view: Box<dyn view::View> = if args.json {
        Box::new(view::Lines::new(std::io::stderr()))
    } else {
        view::for_stdout(args.no_tui, title)
    };
    let outcome = Supervisor::new(home, api, admin, view.as_mut(), interrupt::flag())?
        .cross(args.cross)
        .run(spec)?;
    drop(view);
    let run = &outcome.state;
    if args.json {
        let ranking: Vec<_> = compare::ranked(&outcome.compared)
            .into_iter()
            .map(|(r, _)| r)
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "run": run, "ranking": ranking }))?
        );
    } else {
        println!("\nRanked by receipt (run {}):", run.id);
        print!("{}", compare::table(&outcome.compared));
        print!("{}", compare::hints(&outcome.compared));
    }
    if args.feedback {
        let mut run = outcome.state.clone();
        super::feedback::collect(
            home,
            &mut run,
            args.resume_cmd.as_deref(),
            std::time::Duration::from_secs(600),
            &[],
        )?;
    }
    Ok(if outcome.any_pass() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Width for the summary: the terminal's, or wide enough for logs.
pub(super) fn summary_width() -> usize {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        view::columns().max(60) - 1
    } else {
        160
    }
}

/// `aver run --tasks`, `--issue`, `--issues`: one run per task, at most
/// `--parallel` agents at a time, then the batch summary. Exits 0 when
/// every task is ready.
fn run_batch(
    home: &Home,
    args: &Args,
    repo: PathBuf,
    tasks: Vec<BatchTask>,
    templates: Vec<Template>,
) -> anyhow::Result<ExitCode> {
    let exe = std::env::current_exe().context("locating the aver binary")?;
    let admin = key_in(home, None)?;
    let api = aver::run::api::Api::open(home)?;
    let parallel = args.many.parallel.unwrap_or_else(batch::default_parallel);
    let n = tasks.len();
    let spec = BatchSpec {
        repo,
        rev: args.base.clone(),
        tasks,
        agents: templates,
        budget: budget(args),
        deps: !args.no_deps,
        git_view: !args.no_git,
        aver_bin: exe,
        ports: args.ports,
        parallel,
        id: args.run_id.clone(),
        feedback: args.feedback.then(|| FeedbackSpec {
            resume_cmd: args.resume_cmd.clone(),
            timeout: std::time::Duration::from_secs(600),
        }),
        limits: BatchBudget {
            wall_ms: args.many.batch_timeout,
            verifies: args.many.batch_verifies,
            usd: args.many.batch_usd,
            retries: args.many.retry,
            retry_on: args.many.retry_on.clone(),
        },
        auto_accept: !args.many.no_auto_accept,
        sequence: Sequence {
            mode: args.many.sequence,
            threshold: args.many.sequence_threshold,
        },
    };
    interrupt::graceful();
    let title = format!(
        "aver run · {n} tasks · {} at a time · {}",
        match parallel.min(n as u32 * args.agents.unwrap_or(1)) {
            1 => "1 agent".to_owned(),
            k => format!("{k} agents"),
        },
        view::short_rev(&args.base)
    );
    let mut view: Box<dyn batch::BatchView> = if args.json {
        Box::new(batch::LinesBatch::new(std::io::stderr()))
    } else {
        batch::view_for_stdout(args.no_tui, title)
    };
    let outcome = batch::run(home, &api, &admin, view.as_mut(), interrupt::flag(), spec)?;
    drop(view);
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "batch": outcome.state,
                "summary": outcome.summary,
            }))?
        );
    } else {
        println!("\nBatch {}:", outcome.state.id);
        print!("{}", outcome.summary.render(summary_width()));
    }
    for (number, run, asked) in &outcome.feedback {
        for a in asked {
            match &a.outcome {
                Ok(f) => println!(
                    "  feedback #{number}.{} rating {}{}  {}",
                    a.index,
                    f.rating
                        .map_or("none given".to_owned(), |r| format!("{r}/10")),
                    if f.changed_files {
                        " (files changed while it answered)"
                    } else {
                        ""
                    },
                    f.file.display()
                ),
                Err(why) => println!(
                    "  feedback #{number}.{} (run {run}) not asked: {why}",
                    a.index
                ),
            }
        }
    }
    Ok(if outcome.all_ready() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Starts this same command again in its own session, without `--detach`,
/// in line mode with its output in the run's (or batch's) directory, and
/// returns.
fn detach(home: &Home, batch: bool) -> anyhow::Result<ExitCode> {
    let id = aver::run::state::new_run_id();
    let log_path = if batch {
        batch::log_path(home, &id)
    } else {
        aver::run::state::run_dir(home, &id).join("supervisor.log")
    };
    std::fs::create_dir_all(log_path.parent().expect("the log has a directory"))?;
    let log = std::fs::File::create(&log_path)?;
    let args: Vec<std::ffi::OsString> = std::env::args_os()
        .skip(1)
        .filter(|a| a != "--detach")
        .collect();
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.args(args)
        .args(["--no-tui", "--run-id", &id])
        .env("AVER_HOME", home.root())
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // SAFETY: setsid is async-signal-safe; the child leaves this terminal's
    // session, so closing the terminal does not hang it up.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(&mut cmd, || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn().context("starting the run in the background")?;
    println!("{id}");
    eprintln!(
        "aver: {} {id} started in the background (pid {}); follow it with `aver status {id}`, log in {}",
        if batch { "batch" } else { "run" },
        child.id(),
        log_path.display()
    );
    Ok(ExitCode::SUCCESS)
}
