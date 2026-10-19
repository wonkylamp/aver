//! The `aver` command line. Each subcommand is a module exposing `Args`
//! (clap) and `run(&Home, Args)`, registered with one line in `commands!`.
//! `run` returns `()` for exit code 0 or an `ExitCode`; an error prints and
//! exits 2, so a command can keep 1 for "ran and found a failure".

mod accept;
mod agent;
mod brief;
mod check;
mod ci;
mod combine;
mod daemonctl;
mod demo;
mod diff;
mod discard;
mod doctor;
mod export;
mod feedback;
mod gc;
mod giveup;
mod hook;
mod import;
mod init;
mod integrate;
mod internal;
mod key;
mod lease;
mod locate;
mod next;
mod notices;
mod receipt;
mod release;
mod rerun;
mod restore;
mod run;
mod serve;
mod setup;
mod status;
mod task;
mod verify;
mod workspace;

use std::process::ExitCode;

use anyhow::Context as _;
use aver::keys::Home;
use aver::place::proc::interrupt;
use aver::run::api::{Api, ApiError, NOT_CHECKED};
use clap::{CommandFactory as _, FromArgMatches as _, Parser, Subcommand};

/// The home's services, opened in this process (§2.7.3), and a runtime to
/// call them from.
pub(crate) fn open(home: &Home) -> anyhow::Result<(tokio::runtime::Runtime, Api)> {
    let api = Api::open(home).context("opening the Aver home")?;
    Ok((tokio::runtime::Runtime::new()?, api))
}

macro_rules! commands {
    ($($(#[$doc:meta])* $variant:ident => $module:ident),* $(,)?) => {
        #[derive(Subcommand)]
        enum Command {
            $($(#[$doc])* $variant($module::Args),)*
        }

        impl Command {
            fn run(self, home: &Home) -> anyhow::Result<ExitCode> {
                match self {
                    $(Command::$variant(args) => $module::run(home, args).map(Finish::exit_code),)*
                }
            }
        }
    };
}

commands! {
    /// Run N agents on one task, each in its own workspace directory, and
    /// rank their attempts by receipt.
    Run => run,
    /// A run's agents and attempts, ranked by receipt.
    Status => status,
    /// Turn a verified workspace directory, or a run's chosen attempt, into
    /// a Git branch (and, with --pr, a pull request).
    Accept => accept,
    /// Stop a run's agents and delete its directories.
    Discard => discard,
    /// Ask a finished run's agents Aver's fixed questions about Aver.
    Feedback => feedback,
    /// Merge the work of runs or workspace directories on different tasks
    /// into one branch, verified against every task.
    Combine => combine,
    /// Run a batch task again on a fresh run (one that never started too),
    /// on its old base or on top of another task's accepted work (--onto).
    Rerun => rerun,
    /// Create the server key and the admin key and register them (idempotent);
    /// inside a Git repository, also say whether it needs an aver.toml and
    /// draft one from what it declares (written only with --yes).
    Init => init,
    /// Print the verifier public key (unpadded base64url), initializing the home if needed.
    Key => key,
    /// Removed in 0.4.
    #[command(hide = true)]
    Serve => serve,
    /// Manage agents.
    Agent => agent,
    /// Import a Git commit as a snapshot.
    Import => import,
    /// Export a snapshot and its history to a Git branch.
    Export => export,
    /// Verify a workspace or snapshot against a task and print the receipt;
    /// inside a workspace directory, sync it, verify it and print the agent report.
    Verify => verify,
    /// Inside a workspace directory: run the tests that matter natively on
    /// your copy and on the original code, and say whether each criterion
    /// would prove. A local preview: unlimited, not a verify, signs nothing.
    Check => check,
    /// Create, list, sync and remove on-disk workspace directories.
    Workspace => workspace,
    /// Inside a workspace directory: its changes against the base; or a
    /// run's attempts, one or two compared.
    Diff => diff,
    /// Inside a workspace directory: put paths back to their base version,
    /// or the directory back to its last green check (--last-green).
    Restore => restore,
    /// Inside a workspace directory: reserve paths against agents on other
    /// tasks in the repository, or list the live leases; with --repo [DIR],
    /// from anywhere: every live lease in the repository.
    Lease => lease,
    /// Inside a workspace directory: give back its leases; with --repo
    /// <lease id>…, from anywhere: free any task's lease as the admin.
    Release => release,
    /// Inside a workspace directory: what agents on other tasks change that
    /// overlaps your changes, and the leases they hold; with --repo [DIR],
    /// from anywhere: every task's changes, overlaps and leases.
    Notices => notices,
    /// Inside a workspace directory: stop trying, with a reason.
    Giveup => giveup,
    /// Inside a workspace directory of a run: print the agent's brief.
    Brief => brief,
    /// Remove released and orphaned workspace directories and idle caches.
    Gc => gc,
    /// Set up the agent tools found on this machine for Aver in this
    /// repository (the matching integrate targets), after asking once;
    /// --remove undoes it.
    Setup => setup,
    /// Check what Aver needs here (system, Git, Docker, disk, Node, agent
    /// tools, this repository) and say how to fix each problem.
    Doctor => doctor,
    /// Set up an agent tool (Claude Code, Cursor, Codex, Gemini CLI, VS Code
    /// or any AGENTS.md reader) for Aver, in project-local files, after
    /// showing what it writes.
    Integrate => integrate,
    /// Removed in 0.4.
    #[command(hide = true)]
    Daemon => daemonctl,
    /// What aver commands start in a process of their own.
    #[command(hide = true)]
    Internal => internal,
    /// Agent tool hooks (Claude Code's Stop hook).
    #[command(hide = true)]
    Hook => hook,
    /// Create and manage tasks.
    Task => task,
    /// Print a stored receipt.
    Receipt => receipt,
    /// Verify a head commit against a base commit in a throwaway home, as a pull request check.
    Ci => ci,
    /// Watch Aver catch an agent that games its tests, offline, in a
    /// throwaway repository and home (about 30 seconds).
    Demo => demo,
}

/// The commands `aver --help` lists first, in this order; the rest follow
/// under "Advanced:".
const EVERYDAY: &[&str] = &["check", "verify", "restore", "accept", "init", "demo"];

/// [`Cli`]'s command with its subcommands listed in two groups. Clap gives
/// subcommands one heading, so the list is the help template's own.
fn command() -> clap::Command {
    let cmd = Cli::command();
    let shown: Vec<&clap::Command> = cmd.get_subcommands().filter(|c| !c.is_hide_set()).collect();
    let width = shown.iter().map(|c| c.get_name().len()).max().unwrap_or(0);
    let line = |name: &str, about: String| format!("  {name:width$}  {about}");
    let about = |c: &clap::Command| c.get_about().map(|a| a.to_string()).unwrap_or_default();
    let everyday: Vec<String> = EVERYDAY
        .iter()
        .filter_map(|n| shown.iter().find(|c| c.get_name() == *n))
        .map(|c| line(c.get_name(), about(c)))
        .collect();
    let mut advanced: Vec<String> = shown
        .iter()
        .filter(|c| !EVERYDAY.contains(&c.get_name()))
        .map(|c| line(c.get_name(), about(c)))
        .collect();
    advanced.push(line(
        "help",
        "Print this message or the help of the given subcommand(s)".to_owned(),
    ));
    let template = format!(
        "{{about-with-newline}}\n{{usage-heading}} {{usage}}\n\nCommands:\n{}\n\nAdvanced:\n{}\n\nOptions:\n{{options}}{{after-help}}",
        everyday.join("\n"),
        advanced.join("\n")
    );
    cmd.help_template(template)
}

/// Exit code for a command that failed to run.
const EXIT_ERROR: u8 = 2;

/// What `aver serve`, `aver daemon` and a set `AVER_URL` get since 0.4.
const REMOVED: &str = "removed in 0.4: commands run in process";

fn removed() -> ExitCode {
    eprintln!("aver: {REMOVED}");
    ExitCode::from(EXIT_ERROR)
}

trait Finish {
    fn exit_code(self) -> ExitCode;
}

impl Finish for () {
    fn exit_code(self) -> ExitCode {
        ExitCode::SUCCESS
    }
}

impl Finish for ExitCode {
    fn exit_code(self) -> ExitCode {
        self
    }
}

/// Catches AI agents that game their tests, in copy-on-write workspaces.
///
/// Data lives in $AVER_HOME (default ~/.aver).
///
/// With no command: where things stand here and the command to run next.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

pub fn main() -> ExitCode {
    if std::env::var_os("AVER_URL").is_some() {
        return removed();
    }
    interrupt::install();
    end_quietly_on_a_closed_stdout();
    let cli = Cli::from_arg_matches(&command().get_matches()).unwrap_or_else(|e| e.exit());
    let home = Home::from_env();
    let Some(command) = cli.command else {
        return match next::run(&home) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("aver: {}", describe(&e));
                ExitCode::from(EXIT_ERROR)
            }
        };
    };
    // A newer binary over an older integration: say so once per build.
    // The demo touches nothing of the caller's, this warning's record too.
    if !matches!(
        command,
        Command::Hook(_) | Command::Integrate(_) | Command::Setup(_) | Command::Demo(_)
    ) {
        if let Some(text) = std::env::current_dir()
            .and_then(std::fs::canonicalize)
            .ok()
            .and_then(|cwd| aver::integrate::installed::warn_once(&home, &cwd))
        {
            eprintln!("{text}");
        }
    }
    match command.run(&home) {
        Ok(code) => code,
        // Whatever a stop request cut short ends as the signal's exit.
        Err(_) if interrupt::requested() => {
            eprintln!("aver: interrupted");
            ExitCode::from(interrupt::EXIT)
        }
        Err(e) => {
            // Not an error in Aver but the step the user takes next: said
            // as the verdict line would be, on standard output.
            match e.chain().find_map(|c| c.downcast_ref::<ApiError>()) {
                Some(ApiError::Refused { code, message }) if code == NOT_CHECKED => {
                    print!("{message}")
                }
                _ => eprintln!("aver: {}", describe(&e)),
            }
            ExitCode::from(EXIT_ERROR)
        }
    }
}

/// A reader that went away (`aver status | head`) ends the command as it
/// ends `cat`: by SIGPIPE, without the panic `println!` raises. SIGPIPE
/// itself stays ignored, so a Git child that dies while Aver writes to it
/// is an error Aver reports rather than a silent death mid-export.
fn end_quietly_on_a_closed_stdout() {
    let default = std::panic::take_hook();
    let broken = std::io::Error::from_raw_os_error(libc::EPIPE).to_string();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info.payload_as_str().unwrap_or("");
        if msg.starts_with("failed printing to std") && msg.ends_with(&broken) {
            // SAFETY: restores the default action and raises the signal on
            // this process; nothing runs after it.
            unsafe {
                libc::signal(libc::SIGPIPE, libc::SIG_DFL);
                libc::raise(libc::SIGPIPE);
            }
        }
        default(info)
    }));
}

/// An error and its causes on one line, like `{:#}`, but without a cause
/// whose message the text already holds: many errors here print their
/// source in their own message, which `{:#}` would repeat.
pub(crate) fn describe(e: &anyhow::Error) -> String {
    let mut text = e.to_string();
    for cause in e.chain().skip(1) {
        let cause = cause.to_string();
        if !text.contains(&cause) {
            text.push_str(": ");
            text.push_str(&cause);
        }
    }
    text
}
