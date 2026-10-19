//! `aver rerun <n> [--onto <m|run|branch>]`: task `<n>` of a batch (or a
//! run) again, on a fresh run with the prompt, criteria and protected paths
//! of its Task object, on the base its last run had or on top of other
//! work: task `<m>`'s accepted winner (accepted first when it passed), a
//! run's, or a branch Aver made (ARCHITECTURE.md §2.11). The new run
//! replaces the task's run in the batch; the old one moves to its history.
//! Exits 0 when an attempt passed, 1 when none did.

use std::process::ExitCode;

use anyhow::{bail, Context as _};
use aver::keys::Home;
use aver::place::proc::interrupt;
use aver::run::agent::{self, Template};
use aver::run::batch::BatchState;
use aver::run::compare;
use aver::run::rerun::{self, Onto, RerunOptions};
use aver::run::supervisor::Supervisor;
use aver::run::view;
use aver::services::key_in;

#[derive(clap::Args)]
pub struct Args {
    /// The task's number in the batch, or a run id.
    task: String,
    /// What to start on: a task's number (its accepted winner, accepted
    /// first when it passed), a run, or a branch aver accept or aver combine
    /// wrote. Default: the base the task's last run had.
    #[arg(long, value_name = "M|RUN|BRANCH")]
    onto: Option<String>,
    /// Accept the task --onto names even when it is REVIEW (it changes what
    /// an existing test expects): passing this is your approval.
    #[arg(long, requires = "onto")]
    approve_review: bool,
    /// The batch (default: the newest in this repository).
    #[arg(long)]
    batch: Option<String>,
    /// How many agents (default: as many as the last run had).
    #[arg(short = 'n', long)]
    agents: Option<u32>,
    /// The agent CLI: claude or codex; repeat or separate with commas to mix
    /// them (default: the last run's).
    #[arg(long, value_delimiter = ',')]
    agent: Vec<String>,
    /// A custom agent: a shell command with {dir}, {brief}, {brief_file}
    /// and {prompt}, each substituted quoted.
    #[arg(long, value_name = "TEMPLATE", conflicts_with = "agent")]
    agent_cmd: Option<String>,
    /// How to resume a custom agent with the verify report.
    #[arg(long, value_name = "TEMPLATE", requires = "agent_cmd")]
    resume_cmd: Option<String>,
    /// Let Claude run any command without asking (--dangerously-skip-permissions).
    #[arg(long)]
    yolo: bool,
    /// Run in the background: prints the run id and returns; `aver status`
    /// follows it.
    #[arg(long)]
    detach: bool,
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
    /// The new run's id (set by --detach for the background process).
    #[arg(long, hide = true)]
    run_id: Option<String>,
}

/// The agents: as asked, else the last run's kinds, `names` (a custom
/// agent's command is not kept, so it must be given again).
fn templates(args: &Args, names: &[String]) -> anyhow::Result<Vec<Template>> {
    let n = args.agents.unwrap_or(names.len().max(1) as u32);
    if n == 0 || n > 32 {
        bail!("--agents must be between 1 and 32");
    }
    if let Some(cmd) = &args.agent_cmd {
        return Ok((0..n)
            .map(|_| Template::Custom {
                spawn: cmd.clone(),
                resume: args.resume_cmd.clone(),
            })
            .collect());
    }
    let names: Vec<String> = if args.agent.is_empty() {
        names.to_vec()
    } else {
        args.agent.clone()
    };
    if names.is_empty() {
        bail!("say which agent to run: --agent claude|codex, or --agent-cmd '<command>'");
    }
    let mut out = Vec::new();
    for i in 0..n as usize {
        let t = match names[i % names.len()].trim() {
            "claude" => Template::claude(args.yolo),
            "codex" => Template::codex(),
            "custom" => bail!(
                "the last run used a custom agent, whose command is not kept; pass it again with --agent-cmd '<command>'"
            ),
            other => bail!("unknown agent {other:?}: use claude or codex, or --agent-cmd"),
        };
        if let Template::Claude { bin, .. } | Template::Codex { bin, .. } = &t {
            if agent::find_on_path(bin).is_none() {
                bail!("{bin} is not installed (not on PATH)");
            }
        }
        out.push(t);
    }
    Ok(out)
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<ExitCode> {
    if args.approve_review {
        aver::run::accept::refuse_agent_approval(&format!(
            "aver rerun {} --onto {} --approve-review",
            args.task,
            args.onto.as_deref().unwrap_or("<m>")
        ))?;
    }
    let batch = match &args.batch {
        Some(id) => Some(BatchState::load(home, id).with_context(|| format!("batch {id}"))?),
        None if rerun::task_number(&args.task).is_some() => {
            BatchState::current(home, super::notices::this_repo().as_deref())
        }
        None => None,
    };
    if let Some(b) = batch.as_ref().filter(|b| b.alive()) {
        bail!(
            "batch {} is still running (pid {}); rerun its tasks once it ends, or stop it with ctrl-c",
            b.id,
            b.supervisor.pid
        );
    }
    if args.detach {
        return detach(home);
    }
    let exe = std::env::current_exe().context("locating the aver binary")?;
    let admin = key_in(home, None)?;
    let rt = tokio::runtime::Runtime::new()?;
    let api = aver::run::api::Api::open(home)?;
    // The task's last agents, before anything is accepted.
    let agents = match rerun::prior(home, batch.as_ref(), &args.task) {
        Ok(prior) => templates(&args, &prior.agents)?,
        Err(_) => Vec::new(),
    };
    let onto = args.onto.as_deref().map(|o| Onto::parse(home, o));
    let rerun = rt.block_on(rerun::spec_for(
        &api,
        home,
        &admin,
        batch.as_ref(),
        &args.task,
        onto.as_ref(),
        RerunOptions {
            agents,
            budget: None,
            deps: !args.no_deps,
            git_view: !args.no_git,
            aver_bin: exe,
            ports: args.ports,
            id: args.run_id.clone(),
            approve_review: args.approve_review,
        },
    ))?;
    let name = match (&rerun.batch, &rerun.old) {
        (Some(b), _) => format!("task #{}", b.number),
        (None, Some(old)) => format!("run {}'s task", old.id),
        (None, None) => "the task".to_owned(),
    };
    let id = rerun.spec.id.clone().expect("spec_for sets the id");
    if let Some(base) = &rerun.onto {
        if base.accepted {
            println!(
                "Accepted {} as {} to build on.",
                base.label,
                base.branch.as_deref().unwrap_or("-")
            );
        }
        if base.approved_review {
            println!(
                "It was REVIEW (it changes what an existing test expects): accepted because you passed --approve-review."
            );
        }
        eprintln!(
            "aver: {name} again as run {id}, on {} ({})",
            base.label,
            &base.commit[..12.min(base.commit.len())]
        );
    } else if let Some(old) = &rerun.old {
        eprintln!(
            "aver: {name} again as run {id}, on the base of run {}",
            old.id
        );
    } else {
        eprintln!("aver: {name}, which never started, as run {id} on the batch's base");
    }
    drop(rt);
    interrupt::graceful();
    let title = format!("aver rerun · {name} · {}", rerun.spec.rev);
    let mut view = view::for_stdout(args.no_tui, title);
    let recorded = std::cell::Cell::new(false);
    let services = api.services().clone();
    let record = Box::new(|_: &aver::run::state::RunState| {
        rerun::record(home, &rerun)?;
        recorded.set(true);
        aver::fault::point("rerun.recorded")?;
        // Once the new run speaks for the task, the old run's attempts no
        // longer do: other tasks' notices and trial merges drop them, and
        // their leases end. Before that, a cut rerun leaves them acceptable.
        if let Some(old) = &rerun.old {
            let ids: Vec<_> = old.agents.iter().map(|a| a.attempt).collect();
            services
                .tasks
                .abandon(&ids)
                .map_err(std::io::Error::other)?;
        }
        Ok(())
    });
    let outcome = match Supervisor::new(home, api, admin, view.as_mut(), interrupt::flag())
        .and_then(|s| s.after_first_save(record).run(rerun.spec.clone()))
    {
        Ok(o) => o,
        Err(e) => {
            // A run that could not be prepared still replaces the task's,
            // so the batch shows why.
            if !recorded.get() {
                rerun::record(home, &rerun)?;
            }
            rerun::record_error(home, &rerun, &e.to_string());
            return Err(e.into());
        }
    };
    drop(view);
    println!("\nRanked by receipt (run {}):", outcome.state.id);
    print!("{}", compare::table(&outcome.compared));
    print!("{}", compare::hints(&outcome.compared));
    if outcome.any_pass() {
        let me = rerun
            .batch
            .as_ref()
            .map_or(outcome.state.id.clone(), |b| b.number.to_string());
        let combine = match rerun.spec.stacked_on.as_ref().map(|s| &s.onto) {
            Some(aver::run::state::StackOnto::Run {
                number: Some(m), ..
            }) if rerun.batch.is_some() => format!("aver combine {m} {me}"),
            Some(_) => format!("aver combine {me}"),
            None => format!("aver accept {me}"),
        };
        println!("Next: {combine}");
    }
    Ok(if outcome.any_pass() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Starts this same command again in its own session, without `--detach`,
/// in line mode with its output in the new run's directory, and returns.
fn detach(home: &Home) -> anyhow::Result<ExitCode> {
    let id = aver::run::state::new_run_id();
    let log_path = aver::run::state::run_dir(home, &id).join("supervisor.log");
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
    let child = cmd
        .spawn()
        .context("starting the rerun in the background")?;
    println!("{id}");
    eprintln!(
        "aver: run {id} started in the background (pid {}); follow it with `aver status {id}`, log in {}",
        child.id(),
        log_path.display()
    );
    Ok(ExitCode::SUCCESS)
}
