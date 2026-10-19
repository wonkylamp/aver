//! Search and tree benchmark (ARCHITECTURE.md §2.3): imports one commit of a
//! local repository, opens a workspace with one edited file, then times
//! `Workspaces::search` and `Workspaces::walk` over its whole current state.
//! Run through `bench/search.sh`.
//!
//! Usage: bench_search <repo> <rev> <results.json>

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use aver::env::{DockerResolver, TreeEnv};
use aver::git::{self, ImportOptions};
use aver::keys::{self, Home};
use aver::workspace::{EditRequest, SearchRequest, WalkRequest, Workspaces};
use serde_json::{json, Value};

const RUNS: usize = 20;

fn timed<T>(mut f: impl FnMut() -> Result<T>) -> Result<(T, Vec<Duration>)> {
    let first = f()?;
    let mut times = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let started = Instant::now();
        f()?;
        times.push(started.elapsed());
    }
    times.sort();
    Ok((first, times))
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 10_000.0).round() / 10.0
}

fn stats(times: &[Duration]) -> Value {
    json!({
        "median_ms": ms(times[times.len() / 2]),
        "p95_ms": ms(times[(times.len() * 95).div_ceil(100) - 1]),
        "max_ms": ms(times[times.len() - 1]),
    })
}

fn search(pattern: &str, regex: bool, case_sensitive: bool, glob: Option<&str>) -> SearchRequest {
    serde_json::from_value(json!({
        "pattern": pattern,
        "regex": regex,
        "case_sensitive": case_sensitive,
        "glob": glob,
        "max_results": 100,
    }))
    .expect("a valid search request")
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [repo, rev, out] = args.as_slice() else {
        bail!("usage: bench_search <repo> <rev> <results.json>");
    };
    let dir = tempfile::tempdir()?;
    let home = Home::new(dir.path().join("home"));
    let init = keys::init(&home)?;
    let key = init.server.signing_key.clone();
    let agent = init.admin.id();
    let resolver = Arc::new(DockerResolver);
    let store = Arc::new(home.open_store()?);
    let imported = git::import(
        &store,
        &ImportOptions::new(repo, rev, agent, &key, &*resolver),
    )?;
    let workspaces = Workspaces::new(
        store.clone(),
        Arc::new(key),
        Arc::new(TreeEnv(resolver, aver::env::Detection::Off)),
    );
    let ws = workspaces.open(&agent, &imported.snapshot)?;
    let edit: EditRequest = serde_json::from_value(json!({
        "if_version": 0,
        "ops": [{ "op": "write", "path": "src/averBench.ts", "content": "export function averBench() {}\n" }],
    }))?;
    workspaces.edit(&agent, &ws.id, edit)?;

    let queries = [
        (
            "literal, many matches (stops at 100)",
            search("export", false, true, None),
        ),
        ("literal, one match", search("averBench", false, true, None)),
        (
            "literal, no match (reads every file)",
            search("zq_no_such_text_qz", false, true, None),
        ),
        ("regex", search(r"export function \w+\(", true, true, None)),
        (
            "case-insensitive, no match",
            search("ZQ_NO_SUCH_TEXT_QZ", false, false, None),
        ),
        (
            "glob *.ts, no match",
            search("zq_no_such_text_qz", false, true, Some("*.ts")),
        ),
    ];
    let mut rows = Vec::new();
    println!("Search benchmark: {repo} @ {rev}, {} files", imported.files);
    for (name, req) in &queries {
        let (result, times) = timed(|| Ok(workspaces.search(&agent, &ws.id, req)?))?;
        println!(
            "  {name:<40} {:>4} matches, {:>4} files read: median {:>6.1} ms, p95 {:>6.1} ms",
            result.matches.len(),
            result.files_searched,
            ms(times[times.len() / 2]),
            ms(times[(times.len() * 95).div_ceil(100) - 1]),
        );
        rows.push(json!({
            "query": name,
            "pattern": req.pattern,
            "matches": result.matches.len(),
            "truncated": result.truncated,
            "files_searched": result.files_searched,
            "time": stats(&times),
        }));
    }
    let walks = [
        (
            "tree, every file",
            WalkRequest {
                path: None,
                glob: None,
                max_entries: Some(20_000),
            },
        ),
        (
            "tree, pkgs/core/**/*.ts",
            WalkRequest {
                path: None,
                glob: Some("pkgs/core/**/*.ts".into()),
                max_entries: Some(20_000),
            },
        ),
    ];
    let mut tree_rows = Vec::new();
    for (name, req) in &walks {
        let (result, times) = timed(|| Ok(workspaces.walk(&agent, &ws.id, req)?))?;
        println!(
            "  {name:<40} {:>5} entries: median {:>6.1} ms, p95 {:>6.1} ms",
            result.entries.len(),
            ms(times[times.len() / 2]),
            ms(times[(times.len() * 95).div_ceil(100) - 1]),
        );
        tree_rows
            .push(json!({ "query": name, "entries": result.entries.len(), "time": stats(&times) }));
    }

    let results = json!({
        "repo": repo,
        "rev": rev,
        "files": imported.files,
        "bytes": imported.bytes,
        "runs": RUNS,
        "cpus": std::thread::available_parallelism().map_or(1, |n| n.get()),
        "search": rows,
        "tree": tree_rows,
    });
    if let Some(dir) = Path::new(out).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(out, serde_json::to_string_pretty(&results)? + "\n")?;
    println!("  results {out}");
    Ok(())
}
