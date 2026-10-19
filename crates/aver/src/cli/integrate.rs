//! `aver integrate <tool>`: sets up an agent tool in one repository to work
//! with Aver (ARCHITECTURE.md §2.12). Shows exactly what it will write,
//! asks, then writes project-local files only, noting each write in the
//! record `aver setup --remove` undoes it from. `aver setup` runs several
//! targets through the same [`prepare`] and [`apply`].

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context as _};
use aver::atomic::{self, Mode};
use aver::fault;
use aver::integrate::remove::{self, Record};
use aver::integrate::tools::{self, Tool};
use aver::integrate::{self, FileWrite};
use aver::keys::{self, Home};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    target: Target,
}

#[derive(clap::Subcommand)]
enum Target {
    /// Claude Code: the aver skill and the /aver commands under .claude/,
    /// and Aver's hooks (the session brief, the verify gate, claude
    /// --worktree opening an Aver workspace) in .claude/settings.json.
    Claude(BaseArgs),
    /// Cursor: Aver's instructions as the rule .cursor/rules/aver.mdc.
    Cursor(BaseArgs),
    /// Codex CLI: Aver's instructions in AGENTS.md.
    Codex(BaseArgs),
    /// Gemini CLI: Aver's instructions in GEMINI.md.
    Gemini(BaseArgs),
    /// VS Code (GitHub Copilot): Aver's instructions in
    /// .github/copilot-instructions.md.
    Vscode(BaseArgs),
    /// Any tool that reads AGENTS.md: Aver's instructions there.
    AgentsMd(BaseArgs),
}

#[derive(clap::Args)]
pub(super) struct BaseArgs {
    /// The repository (default: the one holding the current directory).
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
    /// Write without asking.
    #[arg(long)]
    pub yes: bool,
}

/// One integration target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Claude,
    Tool(Tool),
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Claude,
        Kind::Tool(Tool::Cursor),
        Kind::Tool(Tool::Codex),
        Kind::Tool(Tool::Gemini),
        Kind::Tool(Tool::Vscode),
        Kind::Tool(Tool::AgentsMd),
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Claude => "claude",
            Kind::Tool(t) => t.name(),
        }
    }

    pub fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == name)
    }
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    let (kind, base) = match args.target {
        Target::Claude(base) => (Kind::Claude, base),
        Target::Cursor(base) => (Kind::Tool(Tool::Cursor), base),
        Target::Codex(base) => (Kind::Tool(Tool::Codex), base),
        Target::Gemini(base) => (Kind::Tool(Tool::Gemini), base),
        Target::Vscode(base) => (Kind::Tool(Tool::Vscode), base),
        Target::AgentsMd(base) => (Kind::Tool(Tool::AgentsMd), base),
    };
    let root = repo_root(&base.repo)?;
    let prepared = prepare(kind, &root)?;
    let name = kind.name();
    if !prepared.changes() {
        let what = if kind == Kind::Claude {
            "Claude Code"
        } else {
            name
        };
        aver::integrate::installed::record(home, &root, name)?;
        println!("{what} in {} is already set up for Aver.", root.display());
        return Ok(());
    }
    println!(
        "aver integrate {name} will write these files in {}:\n",
        root.display()
    );
    show(&prepared);
    if !confirm(base.yes, "Write them?")? {
        return Ok(());
    }
    apply(home, &root, std::slice::from_ref(&prepared))?;
    println!("{}", done(kind, &root));
    Ok(())
}

fn git_output(repo: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    git_output(repo, args).map(|s| s.trim().to_owned())
}

/// `rel` as `HEAD` holds it, byte for byte.
pub(super) fn committed(root: &Path, rel: &str) -> Option<String> {
    git_output(root, &["show", &format!("HEAD:{rel}")])
}

pub(super) fn repo_root(repo: &Path) -> anyhow::Result<PathBuf> {
    let root = git(repo, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .with_context(|| format!("{} is not inside a Git repository", repo.display()))?;
    Ok(std::fs::canonicalize(&root)?)
}

fn tracked(root: &Path, rel: &str) -> bool {
    git(root, &["ls-files", "--error-unmatch", rel]).is_some()
}

fn ensure_home(home: &Home) -> anyhow::Result<()> {
    if !home.key_path(&keys::ADMIN_KEY.parse()?).exists() {
        keys::init(home)?;
    }
    Ok(())
}

/// Asks `question`; `Ok(false)` when the user says no.
pub(super) fn confirm(yes: bool, question: &str) -> anyhow::Result<bool> {
    if yes {
        return Ok(true);
    }
    let terminal = std::io::stdin().is_terminal();
    print!("\n{question} [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    // Piped stdin answers too (`echo n | aver setup`); only no answer at
    // all, an empty pipe, is an error.
    if std::io::stdin().lock().read_line(&mut answer)? == 0 && !terminal {
        println!();
        bail!("pass --yes to write these files without asking; nothing was written");
    }
    if !terminal {
        println!("{}", answer.trim());
    }
    let yes = matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes");
    if !yes {
        println!("Nothing written.");
    }
    Ok(yes)
}

/// The repository's Git directory (per worktree).
fn git_dir(root: &Path) -> anyhow::Result<PathBuf> {
    git(root, &["rev-parse", "--absolute-git-dir"])
        .map(PathBuf::from)
        .context("locating the repository's .git directory")
}

/// The repository's `.git/info/exclude`.
pub(super) fn exclude_file(root: &Path) -> anyhow::Result<PathBuf> {
    let gitdir = git(root, &["rev-parse", "--git-common-dir"])
        .map(|d| root.join(d))
        .context("locating the repository's .git directory")?;
    Ok(gitdir.join("info/exclude"))
}

/// The record of what the integrations wrote in the repository at `root`.
pub(super) fn record_path(root: &Path) -> anyhow::Result<PathBuf> {
    Ok(git_dir(root)?.join(remove::RECORD))
}

pub(super) fn read_record(root: &Path) -> anyhow::Result<Option<Record>> {
    let path = record_path(root)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => match Record::parse(&text) {
            Ok(record) => Ok(Some(record)),
            // A record torn by an older Aver: what it held is lost, so
            // removal falls back to taking Aver's parts out of each file.
            Err(e) => {
                eprintln!("aver: {e}; ignoring it");
                Ok(None)
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// One target, ready to show and apply.
pub(super) struct Prepared {
    pub kind: Kind,
    pub writes: Vec<FileWrite>,
    pub notes: Vec<String>,
}

impl Prepared {
    pub fn changes(&self) -> bool {
        self.writes.iter().any(FileWrite::changes)
    }
}

/// What `kind` would write in the repository at `root`. Reads only.
pub(super) fn prepare(kind: Kind, root: &Path) -> anyhow::Result<Prepared> {
    let mut notes = Vec::new();
    match kind {
        Kind::Claude => {
            let legacy = !tracked(root, integrate::MCP);
            let writes = integrate::plan(root, legacy).map_err(anyhow::Error::msg)?;
            legacy_note(&writes, &mut notes);
            Ok(Prepared {
                kind,
                writes,
                notes,
            })
        }
        Kind::Tool(tool) => {
            let legacy = tool
                .legacy_mcp_config()
                .is_some_and(|config| !tracked(root, config));
            let writes = tools::plan(tool, root, legacy).map_err(anyhow::Error::msg)?;
            legacy_note(&writes, &mut notes);
            notes.push(tool_note(tool).to_owned());
            Ok(Prepared {
                kind,
                writes,
                notes,
            })
        }
    }
}

/// Says why a write besides the instructions or hooks is there: it takes
/// out the MCP server Aver registered before 0.4.
fn legacy_note(writes: &[FileWrite], notes: &mut Vec<String>) {
    let legacy: Vec<&str> = writes
        .iter()
        .filter(|w| remove::known_legacy_mcp(&w.path))
        .map(|w| w.path.as_str())
        .collect();
    if !legacy.is_empty() {
        notes.push(format!(
            "Aver no longer runs an MCP server; this takes the aver server an older Aver registered out of {}.",
            legacy.join(" and ")
        ));
    }
}

/// Prints what `p` writes, after a heading the caller printed.
pub(super) fn show(p: &Prepared) {
    print!("{}", integrate::describe(&p.writes));
    println!();
    match p.kind {
        Kind::Claude => println!(
            "These files hold no paths of this machine: commit them to share the setup. The hooks do nothing where aver is not installed. Nothing outside this repository is touched; your ~/.claude settings stay as they are."
        ),
        Kind::Tool(tool) => println!(
            "{} holds no paths of this machine: commit it to share the instructions. Nothing outside this repository is touched.",
            tool.instructions_file()
        ),
    }
    for note in &p.notes {
        println!("Note: {note}");
    }
}

/// What `p` writes, one line per file, then its notes: `aver setup`'s
/// default listing. [`show`] prints the files' contents.
pub(super) fn show_short(p: &Prepared) {
    for w in p.writes.iter().filter(|w| w.changes()) {
        println!("--- {} ({})", w.path, integrate::what(w));
    }
    for note in &p.notes {
        println!("Note: {note}");
    }
}

/// The first directory of `rel`'s parents under `root` that does not exist.
fn first_missing_dir(root: &Path, rel: &str) -> Option<String> {
    let mut missing = None;
    let mut dir = Path::new(rel).parent();
    while let Some(d) = dir.filter(|d| !d.as_os_str().is_empty()) {
        if root.join(d).exists() {
            break;
        }
        missing = Some(d.display().to_string());
        dir = d.parent();
    }
    missing
}

/// Writes every change of `prepared`, noting each in the repository's
/// record first.
pub(super) fn apply(home: &Home, root: &Path, prepared: &[Prepared]) -> anyhow::Result<()> {
    ensure_home(home)?;
    let record_file = record_path(root)?;
    let mut record = read_record(root)?.unwrap_or_default();
    let save = |record: &Record| -> anyhow::Result<()> {
        std::fs::create_dir_all(record_file.parent().expect("the record has a directory"))?;
        atomic::write(&record_file, record.to_json().as_bytes(), Mode::Keep)
            .with_context(|| format!("writing {}", record_file.display()))?;
        Ok(fault::point("integrate.recorded")?)
    };
    for w in prepared.iter().flat_map(|p| &p.writes) {
        let path = root.join(&w.path);
        // Read again: two targets may write the same file (AGENTS.md).
        let before = std::fs::read_to_string(&path).ok();
        if before.as_deref() == Some(w.after.as_str()) {
            continue;
        }
        if let Some(dir) = first_missing_dir(root, &w.path) {
            record.note_dir(&dir);
        }
        record.note_write(&FileWrite {
            path: w.path.clone(),
            before,
            after: w.after.clone(),
        });
        save(&record)?;
        atomic::sweep(&path);
        if w.deletes() {
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            atomic::write(&path, w.after.as_bytes(), Mode::Keep)
                .with_context(|| format!("writing {}", path.display()))?;
        }
        fault::point("integrate.file")?;
    }
    for p in prepared {
        aver::integrate::installed::record(home, root, p.kind.name())?;
    }
    Ok(())
}

/// What `kind` gives once it is applied in `root`.
pub(super) fn done(kind: Kind, root: &Path) -> String {
    match kind {
        Kind::Claude => format!(
            "Done. In {}, Claude Code sessions get Aver's brief and verify gate, `claude --worktree <name>` opens an Aver workspace, and /aver, /aver-check, /aver-verify and /aver-status are there. Commit .claude/ to share it.",
            root.display()
        ),
        Kind::Tool(tool) => format!(
            "Done. Agents in {} now get Aver's workflow in {}.",
            tool.name(),
            root.display()
        ),
    }
}

/// What to know about `tool` once it is set up.
fn tool_note(tool: Tool) -> &'static str {
    match tool {
        Tool::Cursor => "Cursor pulls the rule in when it is relevant (or when you @-mention it).",
        Tool::Codex => "Codex reads AGENTS.md in every project.",
        Tool::Gemini => "Gemini CLI reads GEMINI.md as context.",
        Tool::Vscode => "Copilot reads .github/copilot-instructions.md while github.copilot.chat.codeGeneration.useInstructionFiles is on.",
        Tool::AgentsMd => "Codex, Cursor, VS Code and other tools read AGENTS.md.",
    }
}
