//! Import benchmark (ARCHITECTURE.md §7): imports one commit of a local
//! repository into a fresh store, re-imports it, exports it to a new bare
//! repository and checks the tree SHA survived. Run through `bench/import.sh`.
//!
//! Usage: bench_import <repo> <rev> <results.json>

use std::path::Path;
use std::process::Command;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use aver::env::DockerResolver;
use aver::git::{self, ImportOptions};
use aver::store::Store;
use aver_core::{AgentId, SigningKey};

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("running git {args:?}"))?;
    if !out.status.success() {
        bail!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8(out.stdout)?.trim_end().to_owned())
}

/// Lines and files of tracked TypeScript at `rev`.
fn typescript_lines(repo: &Path, rev: &str) -> Result<(u64, u64)> {
    let out = git(
        repo,
        &[
            "grep", "--count", "-I", "-e", "", rev, "--", "*.ts", "*.tsx", "*.mts", "*.cts",
        ],
    )?;
    let mut lines = 0;
    let mut files = 0;
    for row in out.lines() {
        let count = row.rsplit(':').next().context("git grep row")?;
        lines += count.parse::<u64>()?;
        files += 1;
    }
    Ok((lines, files))
}

fn dir_size(path: &Path) -> Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("aver.db") {
            total += entry.metadata()?.len();
        }
    }
    Ok(total)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [repo, rev, out] = args.as_slice() else {
        bail!("usage: bench_import <repo> <rev> <results.json>");
    };
    let repo_path = Path::new(repo);
    let commit = git(
        repo_path,
        &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
    )?;
    let original_tree = git(repo_path, &["rev-parse", &format!("{commit}^{{tree}}")])?;
    let (ts_lines, ts_files) = typescript_lines(repo_path, &commit)?;

    let home = tempfile::tempdir()?;
    let key = SigningKey::from_bytes(&[42; 32]);
    let agent = AgentId::of_public_key(&key.verifying_key());
    let resolver = DockerResolver;
    let store = Store::open(home.path().join("aver.db"))?;
    let options = ImportOptions::new(repo, &commit, agent, &key, &resolver);

    let first = git::import(&store, &options)?;
    let started = Instant::now();
    let second = git::import(&store, &options)?;
    let reimport = started.elapsed();
    assert!(second.already_imported && second.new_objects == 0);

    let target = home.path().join("export.git");
    let started = Instant::now();
    let exported = git::export(&store, &first.snapshot, &target, "main")?;
    let export = started.elapsed();
    let exported_tree = git(
        &target,
        &["rev-parse", &format!("{}^{{tree}}", exported.commit)],
    )?;
    let tree_matches = exported_tree == original_tree;

    // Closing the store checkpoints the WAL into the main file.
    drop(store);
    let db_bytes = dir_size(home.path())?;

    let results = serde_json::json!({
        "repo": repo,
        "commit": commit,
        "files": first.files,
        "typescript_files": ts_files,
        "typescript_lines": ts_lines,
        "total_bytes": first.bytes,
        "objects_written": first.new_objects,
        "import_ms": first.duration.as_secs_f64() * 1000.0,
        "reimport_ms": reimport.as_secs_f64() * 1000.0,
        "export_ms": export.as_secs_f64() * 1000.0,
        "export_commit": exported.commit.as_str(),
        "tree_sha": original_tree,
        "tree_sha_matches": tree_matches,
        "db_bytes": db_bytes,
        "snapshot": first.snapshot.to_string(),
        "cpus": std::thread::available_parallelism().map_or(1, |n| n.get()),
    });
    if let Some(dir) = Path::new(out).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(out, serde_json::to_string_pretty(&results)? + "\n")?;

    let mib = |b: u64| b as f64 / (1024.0 * 1024.0);
    println!("Import benchmark: {repo} @ {commit}");
    println!(
        "  files              {} ({} TypeScript files, {} TypeScript lines)",
        first.files, ts_files, ts_lines
    );
    println!("  total bytes        {:.1} MiB", mib(first.bytes));
    println!("  objects written    {}", first.new_objects);
    println!(
        "  import             {:.0} ms",
        first.duration.as_secs_f64() * 1000.0
    );
    println!(
        "  re-import          {:.0} ms",
        reimport.as_secs_f64() * 1000.0
    );
    println!(
        "  export round trip  {:.0} ms, tree SHA {}",
        export.as_secs_f64() * 1000.0,
        if tree_matches { "matches" } else { "DIFFERS" }
    );
    println!("  database size      {:.1} MiB", mib(db_bytes));
    println!("  results            {out}");
    if !tree_matches {
        bail!("exported tree {exported_tree} differs from the original {original_tree}");
    }
    Ok(())
}
