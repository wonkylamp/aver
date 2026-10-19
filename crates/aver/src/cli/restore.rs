//! `aver restore <path>…` inside a workspace directory: puts each path back
//! to its base version (or removes it where the base has none). `aver
//! restore --last-green` puts the directory back to the newest `aver check`
//! or `aver verify` with no NEW failure, `--checkpoints` lists them, and
//! `aver restore --undo` puts back what the last restore replaced.

use aver::keys::Home;
use aver_core::Path;

use super::workspace::Context;

#[derive(clap::Args)]
pub struct Args {
    /// Workspace-relative paths; a directory restores every base file under it.
    #[arg(required_unless_present_any = ["undo", "last_green", "checkpoints"])]
    paths: Vec<String>,
    /// Put back what the last restore in this directory replaced, even
    /// unsynced edits; again to go one restore further back.
    #[arg(long, conflicts_with_all = ["paths", "last_green", "checkpoints"])]
    undo: bool,
    /// Put the directory back to the newest `aver check` or `aver verify`
    /// here that had no NEW failure, writing only the files that differ.
    #[arg(long, conflicts_with_all = ["paths", "checkpoints"])]
    last_green: bool,
    /// List the checks and verifies recorded here, newest last.
    #[arg(long, conflicts_with = "paths")]
    checkpoints: bool,
}

pub fn run(_: &Home, args: Args) -> anyhow::Result<()> {
    let ctx = Context::require("aver restore works on a workspace directory")?;
    if args.undo {
        let key = ctx.key()?;
        let services = aver::services::Services::open(&ctx.home)?;
        for path in services
            .workspaces()
            .restore_undo(&key.id(), &ctx.file.workspace)?
        {
            println!("put back {path}");
        }
        return Ok(());
    }
    if args.checkpoints {
        let services = aver::services::Services::open(&ctx.home)?;
        let all = services.workspaces().checkpoints(&ctx.file.workspace)?;
        if all.is_empty() {
            println!("No checkpoint yet: each `aver check` and `aver verify` here records one.");
        }
        for c in all {
            let green = if c.green() { "  green" } else { "" };
            println!("{}  {}  {}{green}", c.at, c.verdict.as_str(), c.delta());
        }
        return Ok(());
    }
    if args.last_green {
        let key = ctx.key()?;
        let services = aver::services::Services::open(&ctx.home)?;
        let done = services
            .workspaces()
            .restore_last_green(&key.id(), &ctx.file.workspace)?;
        let c = &done.checkpoint;
        let what = format!("{} at {}, {}", c.verdict.as_str(), c.at, c.delta());
        if done.written.is_empty() {
            println!("The directory already holds the last green checkpoint ({what}).");
        } else {
            for path in &done.written {
                println!("restored {path}");
            }
            println!("Back to the last green checkpoint ({what}).");
            println!("aver restore --undo puts back what this replaced");
        }
        return Ok(());
    }
    let root = std::fs::canonicalize(&ctx.root)?;
    let cwd = std::fs::canonicalize(std::env::current_dir()?)?;
    let rel_cwd = cwd.strip_prefix(&root).unwrap_or(std::path::Path::new(""));
    let paths = args
        .paths
        .iter()
        .map(|p| workspace_path(&root, rel_cwd, p))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let key = ctx.key()?;
    let restored = aver::services::Services::open(&ctx.home)?
        .workspaces()
        .restore(&key.id(), &ctx.file.workspace, &paths)?;
    for path in &restored {
        println!("restored {path}");
    }
    if !restored.is_empty() {
        println!("aver restore --undo puts back what this replaced");
    }
    Ok(())
}

/// `arg`, typed in the directory `rel_cwd` under the workspace `root`, as a
/// workspace path: resolved component by component, so `..` climbs from the
/// cwd and may not leave the workspace.
fn workspace_path(
    root: &std::path::Path,
    rel_cwd: &std::path::Path,
    arg: &str,
) -> anyhow::Result<Path> {
    use std::path::Component;
    let typed = std::path::Path::new(arg);
    let outside = || anyhow::anyhow!("{arg} is outside the workspace directory");
    let (start, rest) = if typed.is_absolute() {
        (
            std::path::Path::new(""),
            typed.strip_prefix(root).map_err(|_| outside())?,
        )
    } else {
        (rel_cwd, typed)
    };
    let mut parts: Vec<String> = Vec::new();
    for c in start.components().chain(rest.components()) {
        match c {
            Component::Normal(name) => parts.push(
                name.to_str()
                    .ok_or_else(|| anyhow::anyhow!("{arg}: not UTF-8"))?
                    .to_owned(),
            ),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop().ok_or_else(outside)?;
            }
            Component::RootDir | Component::Prefix(_) => return Err(outside()),
        }
    }
    if parts.is_empty() {
        anyhow::bail!("{arg} is the whole workspace directory; name the paths to restore");
    }
    parts
        .join("/")
        .parse::<Path>()
        .map_err(|e| anyhow::anyhow!("{arg}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path as FsPath;

    fn at(cwd: &str, arg: &str) -> Result<String, String> {
        workspace_path(FsPath::new("/ws"), FsPath::new(cwd), arg)
            .map(|p| p.as_str().to_owned())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn paths_resolve_against_the_cwd_by_component() {
        assert_eq!(at("", "src/a.ts").unwrap(), "src/a.ts");
        assert_eq!(at("src", "a.ts").unwrap(), "src/a.ts");
        assert_eq!(at("src", "./a.ts").unwrap(), "src/a.ts");
        assert_eq!(at("src/deep", "../a.ts").unwrap(), "src/a.ts");
        assert_eq!(at("src", "../config.json").unwrap(), "config.json");
        assert_eq!(at("src", "lib/").unwrap(), "src/lib");
        assert_eq!(at("src", "x/../.env").unwrap(), "src/.env");
        assert_eq!(at("", "/ws/src/a.ts").unwrap(), "src/a.ts");
    }

    #[test]
    fn paths_outside_the_workspace_are_refused() {
        assert!(at("", "../config.json").is_err());
        assert!(at("src", "../../etc/passwd").is_err());
        assert!(at("", "/etc/passwd").is_err());
        assert!(at("", ".").is_err());
    }
}
