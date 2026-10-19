//! `aver status [run|batch]`: a run's agents and their attempts ranked by
//! receipt, or a batch's summary, while it runs or after; `--all` lists
//! every batch and every run with its verdict. `--json` adds the task
//! graph: the batch's tasks (or, for a run, the tasks working in its
//! repository) and an `overlap` edge for each pair that changed the same
//! files, from the repository view (live) and the batch's collisions.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use aver::coord::{Merge, Overlap, RepoView};
use aver::keys::Home;
use aver::run::batch::summary::TaskSummary;
use aver::run::batch::{self, BatchState};
use aver::run::compare;
use aver::run::state::RunState;
use aver::services::key_in;
use aver_core::{SigningKey, TaskId};
use serde::Serialize;

#[derive(clap::Args)]
pub struct Args {
    /// The run or batch (default: the newest in this repository).
    run: Option<String>,
    /// List every batch and every run, with its verdict.
    #[arg(long)]
    all: bool,
    #[arg(long)]
    json: bool,
}

fn run_state(run: &RunState) -> String {
    if run.discarded {
        "discarded".to_owned()
    } else if let Some(d) = &run.decided {
        format!("accepted #{}", d.index)
    } else if run.supervisor_alive() {
        "running".to_owned()
    } else if run.finished_at.is_none() {
        "orphaned".to_owned()
    } else {
        "finished".to_owned()
    }
}

fn batch_state(b: &BatchState) -> &'static str {
    if b.alive() {
        "running"
    } else if b.finished_at.is_none() {
        "orphaned"
    } else {
        "finished"
    }
}

/// The run `id` names: a run id, or a task number (`3`, `#3`) of the
/// current batch in `repo`.
fn resolve(home: &Home, id: &str, repo: Option<&Path>) -> anyhow::Result<RunState> {
    if let Ok(run) = RunState::find(home, Some(id), None) {
        return Ok(run);
    }
    let Ok(n) = id.trim_start_matches('#').parse::<u32>() else {
        anyhow::bail!("no run or batch {id}; aver status --all lists them");
    };
    let Some(batch) = BatchState::current(home, repo) else {
        anyhow::bail!("no task #{n}: there is no batch; aver status --all lists runs");
    };
    if batch.entry(n).is_none() {
        let numbers: Vec<u32> = batch.tasks.iter().map(|e| e.number).collect();
        let range = match (numbers.iter().min(), numbers.iter().max()) {
            (Some(lo), Some(hi)) if lo == hi => format!("task is {lo}"),
            (Some(lo), Some(hi)) => format!("tasks are {lo}–{hi}"),
            _ => "it has no tasks".to_owned(),
        };
        anyhow::bail!("no task #{n} in batch {}; {range}", batch.id);
    }
    batch
        .run(home, n)
        .with_context(|| format!("task #{n} of batch {} has not started", batch.id))
}

pub fn run(home: &Home, args: Args) -> anyhow::Result<()> {
    if let Some(r) = aver::recover::recent(home) {
        eprintln!("aver: {}", r.summary());
    }
    let admin = key_in(home, None)?;
    let rt = tokio::runtime::Runtime::new()?;
    if args.all {
        return all(home, &rt, &admin.signing_key, args.json);
    }
    let repo = super::notices::this_repo();
    let batch = match &args.run {
        Some(id) if batch::exists(home, id) => Some(BatchState::load(home, id)?),
        Some(_) => None,
        None => BatchState::current(home, repo.as_deref()),
    };
    if let Some(b) = batch {
        let api = aver::run::api::Api::open(home)?;
        let summary = rt.block_on(batch::summarize(&api, home, &admin.signing_key, &b));
        if args.json {
            let runs: Vec<(u32, RunState)> = b
                .tasks
                .iter()
                .filter_map(|e| b.run(home, e.number).map(|r| (e.number, r)))
                .collect();
            let view = super::notices::repo_view(home, &b.repo).ok();
            let graph = batch_graph(&summary, &runs, view.as_ref());
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({ "batch": b, "summary": summary, "graph": graph })
                )?
            );
            return Ok(());
        }
        println!(
            "batch {} · {} · base {} · {} {} per task · {} at a time",
            b.id,
            batch_state(&b),
            &b.base_commit[..12.min(b.base_commit.len())],
            b.agents,
            if b.agents == 1 { "agent" } else { "agents" },
            b.parallel
        );
        print!("{}", summary.render(super::run::summary_width()));
        for e in &b.tasks {
            if let (Some(old), Some(new)) = b.run_of(home, e) {
                let why = e
                    .error
                    .as_deref()
                    .map(|w| format!(": {}", w.lines().next().unwrap_or("")))
                    .unwrap_or_default();
                println!(
                    "#{}: the rerun did not start (run {new}{why}); this is its last run, {}. Run it again: aver rerun {}",
                    e.number, old.id, e.number
                );
            }
        }
        return Ok(());
    }
    let run = match args.run.as_deref() {
        Some(id) => resolve(home, id, repo.as_deref())?,
        None => RunState::find(home, None, repo.as_deref())?,
    };
    let api = aver::run::api::Api::open(home)?;
    let compared = rt.block_on(compare::gather(&api, home, &admin.signing_key, &run));
    if args.json {
        let ranking = compare::ranked(&compared)
            .into_iter()
            .map(|(r, c)| {
                let mut row = serde_json::to_value(r)?;
                row["verifies"] = c.verifies.into();
                row["changed"] = c.changed.into();
                Ok(row)
            })
            .collect::<Result<Vec<_>, serde_json::Error>>()?;
        let graph = super::notices::repo_view(home, &run.repo)
            .ok()
            .map(|view| repo_graph(&view, &RunState::all(home)));
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({ "run": run, "ranking": ranking, "graph": graph })
            )?
        );
        return Ok(());
    }
    let state = if run.discarded {
        "discarded".to_owned()
    } else if let Some(d) = &run.decided {
        format!(
            "accepted #{} as {}",
            d.index,
            d.branch.as_deref().unwrap_or("-")
        )
    } else if run.supervisor_alive() {
        format!("running (pid {})", run.supervisor.pid)
    } else if run.finished_at.is_none() {
        "orphaned: its supervisor is gone (aver discard stops its agents)".to_owned()
    } else {
        "finished".to_owned()
    };
    let in_batch = run
        .batch
        .as_ref()
        .map(|b| format!(" · task #{} of batch {}", b.number, b.id))
        .unwrap_or_default();
    println!(
        "run {} · {} · base {}{in_batch} · {}",
        run.id,
        state,
        &run.base_commit[..12.min(run.base_commit.len())],
        run.title
    );
    for a in &run.agents {
        println!(
            "  #{} {:<7} {:<10} {}",
            a.index,
            a.template,
            a.state.as_str(),
            a.dir.display()
        );
    }
    if let Some(ratings) = aver::run::feedback::ratings(&run) {
        println!(
            "  feedback (1-10, \"I absolutely need this\"): {ratings}; answers in {}",
            aver::run::state::run_dir(home, &run.id).display()
        );
    }
    println!();
    print!("{}", compare::table(&compared));
    if run.decided.is_none() && !run.discarded && run.batch.is_none() {
        print!("{}", compare::hints(&compared));
    }
    Ok(())
}

/// The repository's directory name: the `--all` column. `--json` carries
/// the whole path.
fn repo_name(repo: &Path) -> std::borrow::Cow<'_, str> {
    repo.file_name()
        .map_or_else(|| repo.to_string_lossy(), |n| n.to_string_lossy())
}

/// Every batch with its counts, then every run with its best attempt's
/// verdict and each attempt's.
fn all(
    home: &Home,
    rt: &tokio::runtime::Runtime,
    admin: &SigningKey,
    json: bool,
) -> anyhow::Result<()> {
    let batches = BatchState::all(home);
    let runs = RunState::all(home);
    if batches.is_empty() && runs.is_empty() {
        if json {
            println!("{}", serde_json::json!({ "batches": [], "runs": [] }));
        } else {
            println!("No runs yet; start one with aver run.");
        }
        return Ok(());
    }
    let api = aver::run::api::Api::open(home)?;
    let mut batch_rows = Vec::new();
    for b in &batches {
        let summary = rt.block_on(batch::summarize(&api, home, admin, b));
        batch_rows.push((b, summary));
    }
    let mut run_rows = Vec::new();
    for r in &runs {
        let compared = if r.supervisor_alive() {
            None
        } else {
            Some(rt.block_on(compare::gather(&api, home, admin, r)))
        };
        let number = r.batch.as_ref().map_or(0, |b| b.number);
        let line = TaskSummary::of(
            number,
            &r.title,
            r.batch.as_ref().and_then(|b| b.issue),
            &r.id,
            Some(r),
            compared.as_deref(),
            false,
        );
        let attempts: Vec<(u32, Option<aver_core::Verdict>)> = compared
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|c| (c.agent.index, c.verdict))
            .collect();
        run_rows.push((r, line, attempts));
    }
    if json {
        let batches: Vec<_> = batch_rows
            .iter()
            .map(|(b, s)| {
                serde_json::json!({
                    "id": b.id,
                    "repo": b.repo,
                    "state": batch_state(b),
                    "started_at": b.supervisor.started_at,
                    "counts": s.counts(),
                    "summary": s,
                })
            })
            .collect();
        let runs: Vec<_> = run_rows
            .iter()
            .map(|(r, line, attempts)| {
                serde_json::json!({
                    "id": r.id,
                    "repo": r.repo,
                    "title": r.title,
                    "state": run_state(r),
                    "started_at": r.supervisor.started_at,
                    "batch": r.batch,
                    "best": line,
                    "attempts": attempts
                        .iter()
                        .map(|(i, v)| serde_json::json!({ "index": i, "verdict": v }))
                        .collect::<Vec<_>>(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "batches": batches, "runs": runs }))?
        );
        return Ok(());
    }
    for (b, s) in &batch_rows {
        println!(
            "batch {}  {}  {:<9} {:<12} {}",
            b.id,
            b.supervisor.started_at,
            batch_state(b),
            repo_name(&b.repo),
            s.counts()
        );
    }
    if !batch_rows.is_empty() {
        println!();
    }
    for (r, line, attempts) in &run_rows {
        let ratings = aver::run::feedback::ratings(r)
            .map(|x| format!("  feedback {x}"))
            .unwrap_or_default();
        let place = r
            .batch
            .as_ref()
            .map(|b| format!("  (batch {} #{})", b.id, b.number))
            .unwrap_or_default();
        let each = if attempts.len() > 1 {
            let v: Vec<String> = attempts
                .iter()
                .map(|(i, v)| format!("#{i} {}", v.map_or("-", |v| v.as_str())))
                .collect();
            format!("  [{}]", v.join(", "))
        } else {
            String::new()
        };
        let verdict = line.outcome.label();
        println!(
            "run {}  {}  {:<12} {:<12} {} {}  {:<15} {}{place}{each}{ratings}",
            r.id,
            r.supervisor.started_at,
            run_state(r),
            repo_name(&r.repo),
            r.agents.len(),
            if r.agents.len() == 1 {
                "agent "
            } else {
                "agents"
            },
            verdict,
            r.title
        );
    }
    Ok(())
}

/// The task graph `aver status --json` prints.
#[derive(Debug, Serialize, PartialEq)]
struct Graph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

/// A task: in a batch, by number; otherwise by task id.
#[derive(Debug, Serialize, PartialEq)]
struct Node {
    #[serde(skip_serializing_if = "Option::is_none")]
    number: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task: Option<TaskId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    /// The task's outcome category (`ready`, `running`, …); absent outside
    /// a batch.
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<String>,
    /// Where the task's run started: `"root"` (the batch's base), or
    /// `{"onto": …}` for a run stacked on other tasks' accepted work.
    base: serde_json::Value,
}

/// Two tasks related: `overlap` when both changed `paths` (the batch's
/// scheduler adds `after` and `held`).
#[derive(Debug, Serialize, PartialEq)]
struct Edge {
    kind: &'static str,
    a: NodeRef,
    b: NodeRef,
    paths: Vec<String>,
    /// The worst overlap of the paths.
    overlap: Overlap,
    /// The worst trial merge of the paths, where lines could be compared.
    #[serde(skip_serializing_if = "Option::is_none")]
    merge: Option<Merge>,
}

/// A batch task's number, or a task id outside a batch.
#[derive(Debug, Serialize, PartialEq, Eq, PartialOrd, Ord, Clone)]
#[serde(untagged)]
enum NodeRef {
    Number(u32),
    Task(String),
}

/// A node's `base`: `"root"`, or what its run is stacked on.
fn base_of(run: Option<&RunState>) -> serde_json::Value {
    match run.and_then(|r| r.stacked_on.as_ref()) {
        Some(stack) => serde_json::json!({ "onto": stack.onto, "commit": stack.commit }),
        None => serde_json::json!("root"),
    }
}

fn merge_rank(m: Option<Merge>) -> u8 {
    match m {
        None => 0,
        Some(Merge::Clean) => 1,
        Some(Merge::SamePlace) => 2,
        Some(Merge::Conflict) => 3,
    }
}

/// Paths two nodes both changed, each with its overlap and trial merge.
type Shared = BTreeMap<(NodeRef, NodeRef), BTreeMap<String, (Overlap, Option<Merge>)>>;

fn add_path(shared: &mut Shared, a: NodeRef, b: NodeRef, path: &str, o: Overlap, m: Option<Merge>) {
    let pair = if a <= b { (a, b) } else { (b, a) };
    let e = shared
        .entry(pair)
        .or_default()
        .entry(path.to_owned())
        .or_insert((o, m));
    e.0 = e.0.max(o);
    if merge_rank(m) > merge_rank(e.1) {
        e.1 = m;
    }
}

fn edges(shared: Shared) -> Vec<Edge> {
    let mut out: Vec<Edge> = shared
        .into_iter()
        .filter_map(|((a, b), paths)| {
            let overlap = paths.values().map(|(o, _)| *o).max()?;
            let merge = paths
                .values()
                .map(|(_, m)| *m)
                .max_by_key(|m| merge_rank(*m))
                .flatten();
            Some(Edge {
                kind: "overlap",
                a,
                b,
                paths: paths.into_keys().collect(),
                overlap,
                merge,
            })
        })
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.overlap));
    out
}

/// A batch's graph: a node per task, an `overlap` edge per pair of tasks
/// that changed the same files, live from the repository view and from
/// the summary's collisions (which outlast the directories).
fn batch_graph(
    summary: &batch::summary::Summary,
    runs: &[(u32, RunState)],
    view: Option<&RepoView>,
) -> Graph {
    let nodes = summary
        .tasks
        .iter()
        .map(|t| Node {
            number: Some(t.number),
            task: runs
                .iter()
                .find(|(n, _)| *n == t.number)
                .map(|(_, r)| r.task),
            run: Some(t.run.clone()),
            title: Some(t.title.clone()),
            outcome: Some(t.outcome.category().to_owned()),
            base: base_of(runs.iter().find(|(n, _)| *n == t.number).map(|(_, r)| r)),
        })
        .collect();
    let numbers: BTreeMap<TaskId, u32> = runs.iter().map(|(n, r)| (r.task, *n)).collect();
    let mut shared = Shared::new();
    for c in &summary.collisions {
        let [a, b] = c.tasks;
        add_path(
            &mut shared,
            NodeRef::Number(a),
            NodeRef::Number(b),
            &c.path,
            c.overlap,
            None,
        );
    }
    for e in view.map(|v| v.edges.as_slice()).unwrap_or_default() {
        let number = |t: Option<TaskId>| t.and_then(|t| numbers.get(&t).copied());
        let (Some(a), Some(b)) = (number(e.a), number(e.b)) else {
            continue;
        };
        for p in &e.paths {
            add_path(
                &mut shared,
                NodeRef::Number(a),
                NodeRef::Number(b),
                &p.path,
                p.overlap,
                p.merge,
            );
        }
    }
    Graph {
        nodes,
        edges: edges(shared),
    }
}

/// The graph of a repository outside a batch: a node per task working
/// there (with its run, when `aver run` started it), an `overlap` edge per
/// pair that changed the same files.
fn repo_graph(view: &RepoView, runs: &[RunState]) -> Graph {
    let name = |t: Option<TaskId>| t.map_or_else(|| "-".to_owned(), |t| t.to_string());
    let nodes = view
        .tasks
        .iter()
        .map(|t| Node {
            number: None,
            task: t.task,
            run: runs
                .iter()
                .find(|r| Some(r.task) == t.task)
                .map(|r| r.id.clone()),
            title: t.title.clone(),
            outcome: None,
            base: base_of(runs.iter().find(|r| Some(r.task) == t.task)),
        })
        .collect();
    let mut shared = Shared::new();
    for e in &view.edges {
        for p in &e.paths {
            add_path(
                &mut shared,
                NodeRef::Task(name(e.a)),
                NodeRef::Task(name(e.b)),
                &p.path,
                p.overlap,
                p.merge,
            );
        }
    }
    Graph {
        nodes,
        edges: edges(shared),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_edges_keep_the_worst_overlap_and_merge_per_pair() {
        let mut shared = Shared::new();
        let (one, two) = (NodeRef::Number(1), NodeRef::Number(2));
        let three = NodeRef::Number(3);
        add_path(
            &mut shared,
            two.clone(),
            one.clone(),
            "a.js",
            Overlap::Disjoint,
            None,
        );
        add_path(
            &mut shared,
            one.clone(),
            two.clone(),
            "a.js",
            Overlap::Lines,
            Some(Merge::Conflict),
        );
        add_path(
            &mut shared,
            one.clone(),
            two.clone(),
            "b.js",
            Overlap::Disjoint,
            Some(Merge::Clean),
        );
        add_path(&mut shared, one.clone(), three, "c.js", Overlap::File, None);
        let edges = edges(shared);
        assert_eq!(edges.len(), 2);
        assert_eq!((&edges[0].a, &edges[0].b), (&one, &two));
        assert_eq!(edges[0].paths, vec!["a.js", "b.js"]);
        assert_eq!(edges[0].overlap, Overlap::Lines);
        assert_eq!(edges[0].merge, Some(Merge::Conflict));
        assert_eq!(edges[1].overlap, Overlap::File);
        assert_eq!(edges[1].merge, None);
        let json = serde_json::to_value(&edges[0]).unwrap();
        assert_eq!(json["kind"], "overlap");
        assert_eq!(json["a"], 1);
        assert_eq!(json["merge"], "conflict");
    }
}
