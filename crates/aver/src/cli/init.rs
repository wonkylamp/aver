use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use aver::ci::workflow;
use aver::keys::{self, Home};
use aver::suggest;
use serde_json::{json, Value};

#[derive(clap::Args)]
pub struct Args {
    /// Write the drafted aver.toml (atomically; an existing one is kept)
    /// and set up the agent tools without asking.
    #[arg(long)]
    yes: bool,
    /// With --yes, replace an existing aver.toml (with --ci: an existing
    /// workflow; aver.toml is kept).
    #[arg(long)]
    force: bool,
    /// Print the keys, the suggestion and its dry run as JSON.
    #[arg(long)]
    json: bool,
    /// Also draft .github/workflows/aver.yml, the GitHub Actions workflow
    /// that verifies pull requests, records base runs on pushes to the
    /// default branch, guards a merge queue and binds approvals.
    #[arg(long)]
    ci: bool,
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    let init = keys::init(home)?;
    let verb = if init.changed {
        "initialized"
    } else {
        "already initialized"
    };
    let keys = json!({
        "home": home.root(),
        "server_key": init.server.id().to_string(),
        "admin_key": init.admin.id().to_string(),
    });
    if !args.json {
        println!("{verb} {}", home.root().display());
        println!("server key: {}", init.server.id());
        println!(
            "admin key:  {} ({})",
            init.admin.id(),
            home.key_path(&init.admin.name).display()
        );
    }
    let Some(repo) = repository_root() else {
        if args.ci {
            bail!("aver init --ci writes a workflow into a Git repository; run it inside one");
        }
        if args.json {
            println!("{}", serde_json::to_string_pretty(&keys)?);
        }
        return Ok(());
    };
    let ci = args.ci.then(|| Ci::new(&repo));
    if let Some(ci) = &ci {
        if args.yes && ci.exists && !args.force {
            bail!(
                "{} exists; pass --force to replace it (nothing was written)",
                ci.path.display()
            );
        }
    }
    // With --ci, --force is the workflow's: an aver.toml in place is kept.
    let force_toml = args.force && !args.ci;
    let config = repo.join(aver::env::CONFIG_PATH);
    let exists = config.exists();
    let mut out = if exists && !force_toml {
        // Its checks are what Aver runs; drafting another (and its dry run)
        // is for when it is to be replaced.
        if !args.json {
            println!(
                "\n{} exists: Aver runs the checks it lists. `aver init --force` drafts a replacement.",
                config.display()
            );
        }
        let mut out = keys;
        out["repository"] = json!(repo);
        out["aver_toml_exists"] = json!(true);
        out["written"] = json!(false);
        out
    } else {
        draft(&repo, &config, exists, &args, keys)?
    };
    if let Some(ci) = ci {
        out["workflow"] = ci.finish(&args)?;
    }
    out["agents"] = agents(home, &repo, &args)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    }
    Ok(())
}

/// Sets up the agent tools [`super::setup::agent_kinds`] finds, as `aver
/// setup` would: with `--yes`, or after asking on a terminal; otherwise it
/// says how. Returns the object `--json` prints under `agents`.
fn agents(home: &Home, repo: &Path, args: &Args) -> anyhow::Result<Value> {
    use super::integrate::{self, Prepared};
    let root = integrate::repo_root(repo)?;
    let mut todo: Vec<Prepared> = Vec::new();
    let mut ready = Vec::new();
    let mut skipped = Vec::new();
    for kind in super::setup::agent_kinds(&root) {
        match integrate::prepare(kind, &root) {
            Ok(p) if p.changes() => todo.push(p),
            Ok(_) => ready.push(kind.name()),
            Err(e) => skipped.push(json!({"tool": kind.name(), "reason": super::describe(&e)})),
        }
    }
    let tools: Vec<&str> = todo.iter().map(|p| p.kind.name()).collect();
    let out = |written: bool| json!({"tools": tools, "written": written, "already": ready, "skipped": skipped});
    if !args.json {
        for s in &skipped {
            println!("Skipping {}: {}", s["tool"], s["reason"]);
        }
        for name in &ready {
            println!(
                "
{name} is already set up for Aver in {}.",
                root.display()
            );
        }
    }
    if todo.is_empty() {
        return Ok(out(false));
    }
    let ask = !args.yes && !args.json && std::io::IsTerminal::is_terminal(&std::io::stdin());
    if !args.json {
        println!(
            "
For the agent tools, aver init {} these files in {}:",
            if args.yes || ask {
                "will write"
            } else {
                "would write"
            },
            root.display()
        );
        for p in &todo {
            println!("=== {} (aver integrate {})", p.kind.name(), p.kind.name());
            integrate::show_short(p);
        }
        println!("Commit them to share the setup; `aver setup --remove` undoes them.");
    }
    let write = args.yes || (ask && integrate::confirm(false, "Write them?")?);
    if write {
        integrate::apply(home, &root, &todo)?;
        if !args.json {
            for p in &todo {
                println!("{}", integrate::done(p.kind, &root));
            }
        }
    } else if !ask && !args.json {
        println!("Nothing written for them; `aver init --yes` writes them.");
    }
    Ok(out(write))
}

/// Drafts `aver.toml`, prints it unless `--json`, and writes it with
/// `--yes`; returns the object `--json` prints.
fn draft(
    repo: &Path,
    config: &Path,
    exists: bool,
    args: &Args,
    keys: Value,
) -> anyhow::Result<Value> {
    if !args.json {
        eprintln!("aver: reading what {} declares at HEAD", repo.display());
    }
    let (suggestion, commit) = suggest::for_repository(repo, true)
        .with_context(|| format!("suggesting a configuration for {}", repo.display()))?;
    let written = match (&suggestion.toml, args.yes) {
        (Some(toml), true) => {
            aver::atomic::write(config, toml.as_bytes(), aver::atomic::Mode::Keep)
                .with_context(|| format!("writing {}", config.display()))?;
            true
        }
        _ => false,
    };
    if !args.json {
        print!(
            "{}",
            suggest::text(&suggestion, &commit, config, exists, written)
        );
    }
    let mut out = serde_json::to_value(&suggestion)?;
    out["keys"] = keys;
    out["repository"] = json!(repo);
    out["commit"] = json!(commit);
    out["aver_toml_exists"] = json!(exists);
    out["written"] = json!(written);
    Ok(out)
}

/// The workflow `--ci` drafts.
struct Ci {
    path: PathBuf,
    exists: bool,
    plan: workflow::Plan,
}

impl Ci {
    fn new(repo: &Path) -> Self {
        let path = repo.join(workflow::PATH);
        Self {
            exists: path.exists(),
            plan: workflow::plan(repo, &default_branch(repo)),
            path,
        }
    }

    /// Prints the workflow unless `--json` and writes it with `--yes`;
    /// returns its JSON object.
    fn finish(self, args: &Args) -> anyhow::Result<Value> {
        let content = workflow::render(&self.plan);
        let write = args.yes && (!self.exists || args.force);
        if write {
            let dir = self.path.parent().expect("the workflow path has a parent");
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            aver::atomic::write(&self.path, content.as_bytes(), aver::atomic::Mode::Keep)
                .with_context(|| format!("writing {}", self.path.display()))?;
        }
        if !args.json {
            println!("\n{}:\n\n{content}", workflow::PATH);
            if write {
                println!("Wrote {}.", workflow::PATH);
            } else if self.exists {
                println!(
                    "{} exists; nothing written. `aver init --ci --yes --force` replaces it.",
                    workflow::PATH
                );
            } else {
                println!(
                    "Nothing written; `aver init --ci --yes` writes {}.",
                    workflow::PATH
                );
            }
            print!("\n{}", workflow::explain(&self.plan));
        }
        let setup: Vec<&str> = self.plan.setup.iter().map(|s| s.name.as_str()).collect();
        Ok(json!({
            "path": workflow::PATH,
            "exists": self.exists,
            "written": write,
            "branch": self.plan.branch,
            "check": workflow::CHECK_NAME,
            "action": workflow::action_ref(),
            "setup": setup,
            "content": content,
        }))
    }
}

/// The branch whose pushes record base runs: `origin`'s default branch,
/// else the checked-out one, else `main`. A name YAML would need quoted is
/// not used.
fn default_branch(repo: &Path) -> String {
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        let name = String::from_utf8(out.stdout).ok()?;
        let name = name.trim();
        (out.status.success() && !name.is_empty()).then(|| name.to_owned())
    };
    git(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .and_then(|r| r.strip_prefix("origin/").map(str::to_owned))
        .or_else(|| git(&["symbolic-ref", "--short", "HEAD"]))
        .filter(|b| {
            b.bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-' | b'/'))
        })
        .unwrap_or_else(|| "main".to_owned())
}

/// The top of the Git working tree around the current directory, if any.
fn repository_root() -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let dir = String::from_utf8(out.stdout).ok()?;
    (out.status.success() && !dir.trim().is_empty()).then(|| PathBuf::from(dir.trim()))
}
