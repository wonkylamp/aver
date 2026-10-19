//! Workspace benchmark (ARCHITECTURE.md §3, §7).
//!
//! Opens 1,000 workspaces on one snapshot, measures the storage each costs,
//! then edits one file in each of 100 workspaces and snapshots them. Without `--home` it
//! builds a synthetic base of 2,000 files in 210 directories in a temporary
//! `$AVER_HOME`. Run through `bench/workspaces.sh` (release mode).

use std::collections::BTreeMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use aver::env::{StaticResolver, TreeEnv};
use aver::keys::{self, Home};
use aver::workspace::{EditOp, EditRequest, Workspaces};
use aver_core::*;
use clap::Parser;
use rusqlite::Connection;
use serde_json::{json, Value};

const OPENS: usize = 1_000;
const EDITS: usize = 100;
const PACKAGES: usize = 10;
const MODULES: usize = 20;
const FILES: usize = 10;

#[derive(Parser)]
struct Args {
    /// An existing `$AVER_HOME` to benchmark against (initialized if needed).
    #[arg(long)]
    home: Option<PathBuf>,
    /// An existing snapshot in that home to open workspaces on.
    #[arg(long, requires = "home")]
    snapshot: Option<SnapshotId>,
    /// Where to write the JSON results.
    #[arg(long, default_value = concat!(env!("CARGO_MANIFEST_DIR"), "/../../bench/results/workspaces.json"))]
    out: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if cfg!(debug_assertions) {
        eprintln!("warning: debug build; run bench/workspaces.sh for release numbers");
    }
    let temp = tempfile::tempdir()?;
    let home = Home::new(
        args.home
            .clone()
            .unwrap_or_else(|| temp.path().join("home")),
    );
    let init = keys::init(&home)?;
    let store = Arc::new(home.open_store()?);
    let server_key = Arc::new(init.server.signing_key.clone());
    let workspaces = Workspaces::new(
        store.clone(),
        server_key,
        Arc::new(TreeEnv(
            Arc::new(StaticResolver::default()),
            aver::env::Detection::Off,
        )),
    );
    let owner = init.admin.id();

    let (base, synthetic) = match args.snapshot {
        Some(snap) => {
            if !store.has(&snap)? {
                bail!("snapshot {snap} is not in {}", home.root().display());
            }
            (snap, false)
        }
        None => (synthetic_base(&workspaces, &store, &owner)?, true),
    };
    let base_files = count_files(&workspaces, &base)?;
    eprintln!("base {base}: {base_files} files");

    let db = home.db_path();
    let before = storage(&db)?;
    let mut opened = Vec::with_capacity(OPENS);
    let mut in_process = Vec::with_capacity(OPENS);
    for _ in 0..OPENS {
        let t = Instant::now();
        let ws = workspaces.open(&owner, &base)?;
        in_process.push(t.elapsed());
        opened.push(ws.id);
    }
    let after = storage(&db)?;
    let workspace_bytes = after.saturating_sub(before);

    let files: Vec<Path> = sample_paths(&workspaces, &base, EDITS)?;
    // Unique per run, so reruns against one home still create new objects.
    let run: u64 = rand::random();
    let objects = Connection::open(&db)?;
    let count = || -> anyhow::Result<i64> {
        Ok(objects.query_row("SELECT COUNT(*) FROM objects", [], |r| r.get(0))?)
    };
    let (mut edit_times, mut snapshot_times) = (Vec::new(), Vec::new());
    let (mut edit_objects, mut snapshot_objects) = (Vec::new(), Vec::new());
    for (i, (ws, path)) in opened.iter().zip(&files).enumerate() {
        let request = EditRequest {
            if_version: 0,
            ops: vec![EditOp::Write {
                path: path.clone(),
                content: Some(format!(
                    "// run {run:016x}, workspace {i}\nexport const v{i} = {i};\n"
                )),
                content_b64: None,
            }],
        };
        let n0 = count()?;
        let t = Instant::now();
        workspaces.edit(&owner, ws, request)?;
        edit_times.push(t.elapsed());
        let n1 = count()?;
        let t = Instant::now();
        let snap = workspaces.snapshot(&owner, ws)?;
        snapshot_times.push(t.elapsed());
        let n2 = count()?;
        if snap == base {
            bail!("snapshot of an edited workspace equals the base");
        }
        edit_objects.push((n1 - n0) as f64);
        snapshot_objects.push((n2 - n1) as f64);
    }

    let results = json!({
        "benchmark": "workspaces",
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "base": {
            "snapshot": base,
            "synthetic": synthetic,
            "files": base_files,
        },
        "open_in_process": stats(&in_process),
        "storage": {
            "workspaces": OPENS,
            "bytes_delta": workspace_bytes,
            "bytes_per_workspace": workspace_bytes as f64 / OPENS as f64,
            "note": "database + WAL size after a TRUNCATE checkpoint",
        },
        "edit_one_file": {
            "workspaces": EDITS,
            "time": stats(&edit_times),
            "new_objects_mean": mean(&edit_objects),
        },
        "snapshot_after_edit": {
            "workspaces": EDITS,
            "time": stats(&snapshot_times),
            "new_objects_mean": mean(&snapshot_objects),
            "new_objects_max": snapshot_objects.iter().cloned().fold(0.0, f64::max),
        },
    });
    if let Some(dir) = args.out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&args.out, serde_json::to_string_pretty(&results)? + "\n")
        .with_context(|| format!("writing {}", args.out.display()))?;
    print_summary(&results);
    let out = std::fs::canonicalize(&args.out).unwrap_or(args.out);
    println!("wrote {}", out.display());
    Ok(())
}

/// Builds a base of PACKAGES x MODULES directories with FILES files each.
fn synthetic_base(
    ws: &Workspaces,
    store: &aver::store::Store,
    owner: &AgentId,
) -> anyhow::Result<SnapshotId> {
    let env = store.put(&default_env())?;
    let root = store.put(&Tree::new(Set::empty()))?;
    let empty = store.put(&Snapshot::new(root, env))?;
    let mut ops = Vec::new();
    for p in 0..PACKAGES {
        for m in 0..MODULES {
            for f in 0..FILES {
                let body: String = (0..30)
                    .map(|line| {
                        format!(
                            "export const f{p}_{m}_{f}_{line} = (x: number) => x * {line} + {f};\n"
                        )
                    })
                    .collect();
                ops.push(EditOp::Write {
                    path: format!("packages/pkg{p:02}/mod{m:02}/file{f:02}.ts").parse()?,
                    content: Some(body),
                    content_b64: None,
                });
            }
        }
    }
    let builder = ws.open(owner, &empty)?;
    ws.edit(owner, &builder.id, EditRequest { if_version: 0, ops })?;
    Ok(ws.snapshot(owner, &builder.id)?)
}

fn default_env() -> Environment {
    Environment {
        schema: EnvSchema::V0,
        tests: Set::empty(),
        platform: Platform::LinuxArm64,
        image: Image {
            reference: "node:22-bookworm-slim".into(),
            digest: "sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392"
                .parse()
                .expect("valid digest"),
        },
        lockfile: None,
        install: Install {
            argv: vec!["npm".into(), "ci".into(), "--ignore-scripts".into()],
            allow_scripts: false,
            rebuild: false,
        },
        checks: Set::empty(),
        limits: Limits {
            cpu_millis: 2000,
            memory_mb: 2048,
            pids: 512,
            timeout_ms: 300_000,
            output_bytes: 65_536,
        },
        vars: BTreeMap::new(),
        layers: aver_core::Set::empty(),
    }
}

/// Every file path in `snapshot`, walking the tree through the snapshot API.
fn all_files(ws: &Workspaces, snapshot: &SnapshotId) -> anyhow::Result<Vec<Path>> {
    let mut out = Vec::new();
    let mut dirs: Vec<Option<Path>> = vec![None];
    while let Some(dir) = dirs.pop() {
        for entry in ws.list_snapshot(snapshot, dir.as_ref())? {
            let path: Path = match &dir {
                Some(dir) => format!("{dir}/{}", entry.name).parse()?,
                None => entry.name.as_str().parse()?,
            };
            match entry.node {
                Node::Tree(_) => dirs.push(Some(path)),
                _ => out.push(path),
            }
        }
    }
    out.sort();
    Ok(out)
}

fn count_files(ws: &Workspaces, snapshot: &SnapshotId) -> anyhow::Result<usize> {
    Ok(all_files(ws, snapshot)?.len())
}

/// `n` file paths spread evenly over the snapshot.
fn sample_paths(ws: &Workspaces, snapshot: &SnapshotId, n: usize) -> anyhow::Result<Vec<Path>> {
    let files = all_files(ws, snapshot)?;
    if files.is_empty() {
        bail!("snapshot {snapshot} has no files");
    }
    Ok((0..n).map(|i| files[i * files.len() / n].clone()).collect())
}

/// Database plus WAL size after folding the WAL into the database.
fn storage(db: &FsPath) -> anyhow::Result<u64> {
    let conn = Connection::open(db)?;
    let (busy, _, _): (i64, i64, i64) =
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    if busy != 0 {
        bail!("checkpoint was blocked");
    }
    let size = |p: PathBuf| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let wal = PathBuf::from(format!("{}-wal", db.display()));
    Ok(size(db.to_path_buf()) + size(wal))
}

fn micros(d: &Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn stats(times: &[Duration]) -> Value {
    let mut us: Vec<f64> = times.iter().map(micros).collect();
    us.sort_by(f64::total_cmp);
    let pct = |p: f64| us[((p / 100.0) * (us.len() - 1) as f64).round() as usize];
    let round = |v: f64| (v * 10.0).round() / 10.0;
    json!({
        "n": us.len(),
        "mean_us": round(mean(&us)),
        "p50_us": round(pct(50.0)),
        "p95_us": round(pct(95.0)),
        "p99_us": round(pct(99.0)),
        "max_us": round(*us.last().unwrap()),
    })
}

fn print_summary(r: &Value) {
    let f = |v: &Value| v.as_f64().unwrap_or_default();
    let line = |label: &str, s: &Value| {
        println!(
            "{label:<22} mean {:>8.1} us  p50 {:>8.1} us  p95 {:>8.1} us  p99 {:>8.1} us  max {:>8.1} us",
            f(&s["mean_us"]),
            f(&s["p50_us"]),
            f(&s["p95_us"]),
            f(&s["p99_us"]),
            f(&s["max_us"]),
        );
    };
    println!(
        "base: {} files ({})",
        r["base"]["files"],
        if r["base"]["synthetic"] == true {
            "synthetic"
        } else {
            "existing"
        }
    );
    line("open (in-process)", &r["open_in_process"]);
    println!(
        "storage: {} bytes for {} workspaces = {:.1} bytes each",
        r["storage"]["bytes_delta"],
        r["storage"]["workspaces"],
        r["storage"]["bytes_per_workspace"]
            .as_f64()
            .unwrap_or_default(),
    );
    line("edit one file", &r["edit_one_file"]["time"]);
    line("snapshot after edit", &r["snapshot_after_edit"]["time"]);
    println!(
        "new objects: {} per edit, {} per snapshot (max {})",
        r["edit_one_file"]["new_objects_mean"],
        r["snapshot_after_edit"]["new_objects_mean"],
        r["snapshot_after_edit"]["new_objects_max"],
    );
}
