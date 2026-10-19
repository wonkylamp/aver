//! Cache benchmark (PRD §6 DoD 3): imports a fixture into a fresh store, runs
//! every check of its environment twice on the same snapshot natively, and
//! records how long each pass took and how many runs it started. The second
//! pass must start none. Run through `bench/run.sh`.
//!
//! Usage: bench_cache <fixture dir> <results.json>

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, ensure, Context, Result};
use aver::env::DockerResolver;
use aver::exec::{ExecConfig, Executor};
use aver::git::{self, ImportOptions};
use aver::place::runner::{RunnerConfig, SlotRunner};
use aver::store::Store;
use aver_core::{AgentId, RunIndex, SigningKey};
use serde_json::json;

fn sh(dir: &Path, program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .current_dir(dir)
        .args(args)
        .output()
        .with_context(|| format!("running {program} {args:?}"))?;
    ensure!(
        out.status.success(),
        "{program} {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?.trim_end().to_owned())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [fixture, out] = args.as_slice() else {
        bail!("usage: bench_cache <fixture dir> <results.json>");
    };
    let source = std::fs::canonicalize(fixture).with_context(|| format!("fixture {fixture}"))?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    sh(
        temp.path(),
        "cp",
        &["-R", &source.to_string_lossy(), &repo.to_string_lossy()],
    )?;
    sh(&repo, "git", &["init", "--quiet"])?;
    sh(&repo, "git", &["add", "-A"])?;
    sh(
        &repo,
        "git",
        &[
            "-c",
            "user.name=bench",
            "-c",
            "user.email=bench@aver.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    )?;

    let store = Arc::new(Store::open(temp.path().join("aver.db"))?);
    let key = SigningKey::from_bytes(&[42; 32]);
    let agent = AgentId::of_public_key(&key.verifying_key());
    let repo_arg = repo.to_string_lossy().into_owned();
    let options = ImportOptions::new(&repo_arg, "HEAD", agent, &key, &DockerResolver);
    let snapshot = git::import(&store, &options)?.snapshot;

    let config = RunnerConfig::for_process(temp.path(), aver_core::ProvenanceMode::Local);
    let executor = Executor::slots(
        store.clone(),
        SlotRunner::new(store, config),
        ExecConfig::default(),
    );
    let mut passes = Vec::new();
    let mut run_ids = Vec::new();
    for pass in 1..=2 {
        let starts = executor.sandbox_starts();
        let started = Instant::now();
        let runs = executor.run_env_checks(&snapshot, RunIndex::FIRST).await?;
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let sandboxes = executor.sandbox_starts() - starts;
        let hits = runs.iter().filter(|r| r.cache_hit).count();
        println!(
            "  pass {pass}: {} checks in {ms:.0} ms, {sandboxes} sandboxes started, {hits} cache hits",
            runs.len()
        );
        run_ids.push(runs.iter().map(|r| r.id.to_string()).collect::<Vec<_>>());
        passes.push(json!({
            "pass": pass,
            "ms": (ms * 10.0).round() / 10.0,
            "checks": runs.len(),
            "sandboxes_started": sandboxes,
            "cache_hits": hits,
            "outcomes": runs.iter().map(|r| json!({
                "check": r.run.check,
                "outcome": r.run.result.outcome(),
                "tests": r.run.tests.len(),
            })).collect::<Vec<_>>(),
        }));
    }
    let second_from_cache = passes[1]["sandboxes_started"] == 0
        && passes[1]["cache_hits"] == passes[1]["checks"]
        && run_ids[0] == run_ids[1];

    let results = json!({
        "benchmark": "cache",
        "fixture": fixture,
        "snapshot": snapshot,
        "executor": executor.executor(),
        "passes": passes,
        "second_pass_served_from_cache": second_from_cache,
    });
    if let Some(dir) = Path::new(out).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(out, serde_json::to_string_pretty(&results)? + "\n")?;
    println!("  results {out}");
    ensure!(
        second_from_cache,
        "the second pass was not served entirely from cache"
    );
    Ok(())
}
