//! `aver setup`: the first command after installing. Finds the agent tools
//! on this machine, shows everything the matching `aver integrate` targets
//! would write in this repository, asks once and writes it. `--remove`
//! undoes every integration in the repository from its record.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use aver::atomic::{self, Mode};
use aver::fault;
use aver::integrate::remove;
use aver::integrate::tools::Tool;
use aver::keys::Home;

use super::integrate::{self, BaseArgs, Kind, Prepared};
use super::locate;

/// Where macOS apps live; tests point it at an empty directory.
const APPS_ENV: &str = "AVER_APPS_DIR";

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    base: BaseArgs,
    /// Set up this tool instead of the ones found (repeatable): claude,
    /// cursor, codex, gemini, vscode (or copilot: the same files) or
    /// agents-md.
    #[arg(long = "tool", value_name = "TOOL", value_parser = parse_kind)]
    tools: Vec<KindArg>,
    /// Undo what `aver setup` and `aver integrate` wrote in this repository.
    #[arg(long, conflicts_with = "tools")]
    remove: bool,
    /// Print each file in full before asking, not only its path.
    #[arg(long, short)]
    verbose: bool,
}

#[derive(Clone, Copy)]
struct KindArg(Kind);

fn parse_kind(name: &str) -> Result<KindArg, String> {
    let name = if name == "copilot" { "vscode" } else { name };
    Kind::parse(name).map(KindArg).ok_or_else(|| {
        let names: Vec<&str> = Kind::ALL.iter().map(|k| k.name()).collect();
        format!("unknown tool {name:?}; one of {}", names.join(", "))
    })
}

/// An agent tool found on this machine, and how.
pub(super) struct Found {
    pub kind: Kind,
    pub label: &'static str,
    pub how: String,
}

/// The agent tools installed here: their command on `PATH` or where
/// installers put it, or their macOS app.
fn detect() -> Vec<Found> {
    detect_in(&locate::widened_path())
}

/// The agent tools whose command is in `path`, or whose macOS app is
/// installed.
pub(super) fn detect_in(path: &std::ffi::OsStr) -> Vec<Found> {
    let apps = std::env::var_os(APPS_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| "/Applications".into());
    let tools: [(Kind, &str, &str, &[&str]); 5] = [
        (Kind::Claude, "Claude Code", "claude", &["Claude.app"]),
        (
            Kind::Tool(Tool::Cursor),
            "Cursor",
            "cursor",
            &["Cursor.app"],
        ),
        (Kind::Tool(Tool::Codex), "Codex CLI", "codex", &[]),
        (Kind::Tool(Tool::Gemini), "Gemini CLI", "gemini", &[]),
        (
            Kind::Tool(Tool::Vscode),
            "VS Code",
            "code",
            &["Visual Studio Code.app"],
        ),
    ];
    let mut out = Vec::new();
    for (kind, label, command, app_names) in tools {
        let how = locate::which_in(command, path)
            .map(|p| p.display().to_string())
            .or_else(|| {
                app_names
                    .iter()
                    .map(|a| apps.join(a))
                    .find(|a| a.is_dir())
                    .map(|a| a.display().to_string())
            });
        if let Some(how) = how {
            out.push(Found { kind, label, how });
        }
    }
    out
}

/// What in the repository shows it uses `kind`, if anything. Claude Code
/// or Codex on this machine is enough on its own (`aver init` sets them up
/// the same way, [`agent_kinds`]); the others need a sign in the
/// repository: for Copilot its own instructions file, since a `.vscode/`
/// directory shows only the editor.
fn evidence(root: &Path, kind: Kind) -> Option<&'static str> {
    let has = |p: &'static str| root.join(p).exists().then_some(p);
    match kind {
        Kind::Claude => has(".claude/").or(Some("installed")),
        Kind::Tool(Tool::Cursor) => has(".cursor/"),
        Kind::Tool(Tool::Codex) => has(".codex/").or(Some("installed")),
        Kind::Tool(Tool::Gemini) => has(".gemini/").or_else(|| has("GEMINI.md")),
        Kind::Tool(Tool::Vscode) => has(".github/copilot-instructions.md"),
        Kind::Tool(_) => None,
    }
}

/// The detected tools this repository or user evidently uses; each other
/// one is asked about on a terminal, or skipped with a note.
fn scoped(root: &Path, found: Vec<Found>, ask: bool) -> anyhow::Result<Vec<Kind>> {
    let mut kinds = Vec::new();
    for f in found {
        let wanted = evidence(root, f.kind).is_some()
            || (ask
                && integrate::confirm(
                    false,
                    &format!(
                        "Set up {} too? Nothing in this repository shows it uses it",
                        f.label
                    ),
                )?);
        if wanted {
            kinds.push(f.kind);
        } else {
            println!(
                "Skipping {}: nothing in this repository shows it uses it; `aver setup --tool {}` sets it up.",
                f.label,
                f.kind.name()
            );
        }
    }
    Ok(kinds)
}

/// The agent tools `aver init` sets up in the repository at `root`: Claude
/// Code where it is installed or `.claude/` exists, and Aver's block in
/// `AGENTS.md` where Codex is installed, `.codex/` exists or the
/// repository has an `AGENTS.md`.
pub(super) fn agent_kinds(root: &Path) -> Vec<Kind> {
    let found = detect();
    let installed = |k: Kind| found.iter().any(|f| f.kind == k);
    let mut out = Vec::new();
    if installed(Kind::Claude) || root.join(".claude").is_dir() {
        out.push(Kind::Claude);
    }
    if installed(Kind::Tool(Tool::Codex)) || root.join(".codex").is_dir() {
        out.push(Kind::Tool(Tool::Codex));
    } else if root.join("AGENTS.md").is_file() {
        out.push(Kind::Tool(Tool::AgentsMd));
    }
    out
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    let root = integrate::repo_root(&args.base.repo)?;
    if args.remove {
        return remove_all(&root, args.base.yes)
            .and_then(|()| Ok(aver::integrate::installed::forget(home, &root)?));
    }
    let kinds: Vec<Kind> = if args.tools.is_empty() {
        let found = detect();
        if found.is_empty() {
            println!("No agent tool found (claude, cursor, codex, gemini, code). Aver's instructions go to AGENTS.md, which most tools read; pass --tool to pick tools.\n");
            vec![Kind::Tool(Tool::AgentsMd)]
        } else {
            let names: Vec<&str> = found.iter().map(|f| f.label).collect();
            println!("Found {}.", names.join(", "));
            if args.verbose {
                for f in &found {
                    println!("  {:<12} {}", f.label, f.how);
                }
            }
            let ask = !args.base.yes && std::io::IsTerminal::is_terminal(&std::io::stdin());
            scoped(&root, found, ask)?
        }
    } else {
        let mut kinds: Vec<Kind> = args.tools.iter().map(|k| k.0).collect();
        kinds.dedup();
        kinds
    };

    let mut prepared: Vec<Prepared> = Vec::new();
    let mut skipped = Vec::new();
    for kind in kinds {
        match integrate::prepare(kind, &root) {
            Ok(p) => prepared.push(p),
            Err(e) => skipped.push(format!("{}: {}", kind.name(), super::describe(&e))),
        }
    }
    for s in &skipped {
        println!("Skipping {s}");
    }
    let (todo, ready): (Vec<Prepared>, Vec<Prepared>) =
        prepared.into_iter().partition(Prepared::changes);
    for p in &ready {
        println!("{} is already set up in {}.", p.kind.name(), root.display());
    }
    if todo.is_empty() {
        if skipped.is_empty() {
            println!("Nothing to do. {}", next_step());
            return Ok(());
        }
        anyhow::bail!("nothing could be set up");
    }
    println!("aver setup will write these files in {}:", root.display());
    for p in &todo {
        if args.verbose {
            println!("\n=== {} (aver integrate {})", p.kind.name(), p.kind.name());
            integrate::show(p);
        } else {
            println!("=== {} (aver integrate {})", p.kind.name(), p.kind.name());
            integrate::show_short(p);
        }
    }
    println!(
        "Nothing outside this repository and {} is touched. `aver setup --remove` undoes all of it{}.",
        home.root().display(),
        if args.verbose { "" } else { "; --verbose shows each file in full" }
    );
    if !integrate::confirm(args.base.yes, "Write them?")? {
        return Ok(());
    }
    integrate::apply(home, &root, &todo)?;
    for p in &todo {
        println!("{}", integrate::done(p.kind, &root));
    }
    println!("{}", next_step());
    Ok(())
}

/// What to run after setting up: the checks, then the first real command.
fn next_step() -> &'static str {
    "Next: `aver doctor` checks the tools Aver needs; then `aver workspace new -c \"<id>: <what done means>\"` makes a workspace directory for an agent."
}

/// Removes empty directories from `dir` up to (not including) `root`, as
/// long as each is one Aver created (`created`).
fn prune(root: &Path, mut dir: Option<&Path>, created: &[String]) {
    while let Some(d) = dir.filter(|d| d.starts_with(root) && *d != root) {
        let rel = d
            .strip_prefix(root)
            .expect("under root")
            .display()
            .to_string();
        let ours = created
            .iter()
            .any(|c| rel == *c || rel.starts_with(&format!("{c}/")));
        if !ours || std::fs::remove_dir(d).is_err() {
            return;
        }
        dir = d.parent();
    }
}

fn remove_all(root: &Path, yes: bool) -> anyhow::Result<()> {
    let record = integrate::read_record(root)?;
    let removals = remove::plan(root, record.as_ref(), |rel| integrate::committed(root, rel))
        .map_err(anyhow::Error::msg)?;
    let exclude_path = integrate::exclude_file(root)?;
    let exclude_now = std::fs::read_to_string(&exclude_path).ok();
    let exclude_after = match (&record, &exclude_now) {
        (Some(r), Some(now)) => remove::exclude_after(r, now),
        _ => exclude_now.clone(),
    };
    let exclude_changes = exclude_after != exclude_now;
    let record_path = integrate::record_path(root)?;
    // A record that did not parse is still there to delete.
    let record_file = record_path.exists();
    if removals.is_empty() && !exclude_changes && !record_file {
        println!("Aver has nothing set up in {}.", root.display());
        return Ok(());
    }
    println!(
        "aver setup --remove will change these files in {}:\n",
        root.display()
    );
    for r in &removals {
        let what = match &r.after {
            None => "deleted",
            Some(_) => "Aver's entries removed",
        };
        println!("--- {} ({what})", r.path);
        if let Some(after) = &r.after {
            print!("{after}");
        }
    }
    if exclude_changes {
        println!("\nIt will also take Aver's lines out of .git/info/exclude.");
    }
    println!("Agent keys stay in the Aver home; the repository's committed state is not touched.");
    if !integrate::confirm(yes, "Change them?")? {
        return Ok(());
    }
    // Without a record, the directories that hold only Aver's files (the
    // skill's and commands' under .claude/, Cursor's rules) go once empty.
    let created = record.as_ref().map_or_else(
        || {
            remove::known_paths()
                .iter()
                .filter(|p| p.matches('/').count() >= 2)
                .map(|p| p.splitn(3, '/').take(2).collect::<Vec<_>>().join("/"))
                .collect()
        },
        |r| r.dirs.clone(),
    );
    for r in &removals {
        let path = root.join(&r.path);
        atomic::sweep(&path);
        match &r.after {
            Some(text) => atomic::write(&path, text.as_bytes(), Mode::Keep),
            None => std::fs::remove_file(&path),
        }
        .with_context(|| format!("changing {}", path.display()))?;
        fault::point("remove.file")?;
        prune(root, path.parent(), &created);
    }
    if exclude_changes {
        match &exclude_after {
            Some(text) => atomic::write(&exclude_path, text.as_bytes(), Mode::Keep)?,
            None => std::fs::remove_file(&exclude_path)?,
        }
    }
    // The record goes last: until then a second --remove still knows each
    // file's `before`.
    fault::point("remove.record")?;
    if record_file {
        std::fs::remove_file(&record_path)?;
        if let Some(dir) = record_path.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }
    println!("Done. Aver is no longer set up in {}.", root.display());
    Ok(())
}
