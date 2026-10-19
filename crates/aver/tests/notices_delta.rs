//! Notices as deltas (§2.11): each workspace keeps the notice keys it has
//! shown in `.aver/notices-seen.json`, so verify and `aver check` print
//! only what is new, and mark what the other side is still editing. Two
//! workspaces on two tasks in one repository, in process, with the clock
//! passed in. No Docker.

mod common;
mod scripted;
mod world;

use std::fs;
use std::path::PathBuf;

use aver::agents::Capability;
use aver::coord::{Coord, Notices};
use aver::keys::AgentKey;
use aver::seen;
use aver::workspace::MaterializeOptions;
use aver_core::*;
use world::*;

const A_JS: &str = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n";
const B_JS: &str = "m1\nm2\nm3\nm4\nm5\nm6\nm7\nm8\nm9\nm10\n";

struct Fx {
    w: World,
    repo: PathBuf,
    base: SnapshotId,
    coord: Coord,
}

struct Att {
    key: AgentKey,
    ws: WorkspaceId,
    dir: PathBuf,
}

impl Fx {
    fn new() -> Self {
        let w = World::new();
        let repo = w.dir.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        let base = w.snapshot(&[("src/a.js", A_JS), ("src/b.js", B_JS)]);
        let coord = Coord::new(w.store.clone(), w.workspaces.clone());
        Self {
            w,
            repo,
            base,
            coord,
        }
    }

    fn attempt(&self, title: &str, n: u32) -> Att {
        let mut body = self.w.task_body(&self.w.admin, self.base, &[]);
        body.title = title.parse().unwrap();
        let task = self.w.create_task(body);
        let key = self.w.agent(
            &format!("agent-{n}"),
            &[Capability::TaskClaim, Capability::WorkspaceWrite],
        );
        let claim = self
            .w
            .tasks
            .claim(&key.id(), &task, Some(self.base))
            .unwrap();
        let m = self
            .w
            .workspaces
            .materialize(
                &claim.workspace,
                &MaterializeOptions {
                    repo: Some(self.repo.clone()),
                    deps: false,
                    ..Default::default()
                },
            )
            .unwrap();
        Att {
            key,
            ws: claim.workspace,
            dir: m.dir,
        }
    }

    fn edit(&self, a: &Att, path: &str, text: &str) {
        fs::write(a.dir.join(path), text).unwrap();
        self.w.workspaces.sync(&a.key.id(), &a.ws).unwrap();
    }

    fn notices(&self, a: &Att) -> Notices {
        self.coord.notices(&a.key.id(), &a.ws, true).unwrap()
    }
}

/// What a report (verify, `aver check`) in `dir` says of `n` at `now`,
/// recording what it shows.
fn told(dir: &std::path::Path, n: &Notices, now: Timestamp) -> String {
    aver::coord::agent_text(&seen::for_report(dir, Some(n), now).unwrap(), false)
}

/// A minute and a bit after now: whatever the other side did has settled.
fn later() -> Timestamp {
    Timestamp::from_unix_millis(Timestamp::now().unix_millis() + seen::SETTLE_MS + 1_000).unwrap()
}

#[test]
fn a_report_prints_each_overlap_once_and_only_what_changed() {
    let fx = Fx::new();
    let (theirs, mine) = (
        fx.attempt("Migrate sessions", 1),
        fx.attempt("Rate limit", 2),
    );
    fx.edit(&theirs, "src/a.js", &A_JS.replace("l3\n", "x3\n"));
    fx.edit(&theirs, "src/b.js", &B_JS.replace("m3\n", "x3\n"));
    fx.edit(&mine, "src/a.js", &A_JS.replace("l3\n", "y3\n"));
    fx.edit(&mine, "src/b.js", &B_JS.replace("m3\n", "y3\n"));

    // The full notice, once.
    let first = told(&mine.dir, &fx.notices(&mine), later());
    assert!(first.contains("OVERLAP src/a.js"), "{first}");
    assert!(first.contains("OVERLAP src/b.js"), "{first}");
    assert!(first.len() <= aver::coord::MAX_NOTICE_BYTES, "{first}");
    assert!(mine.dir.join(seen::PATH).is_file());
    let second = told(&mine.dir, &fx.notices(&mine), later());
    assert!(
        !second.contains("OVERLAP") && second.contains("unchanged"),
        "{second}"
    );

    // Editing the same lines again changes nothing they need to hear.
    fx.edit(&mine, "src/a.js", &A_JS.replace("l3\n", "z3\n"));
    assert!(!told(&mine.dir, &fx.notices(&mine), later()).contains("OVERLAP"));

    // The other side changes b.js again: only b.js prints, and a.js is
    // counted as unchanged.
    fx.edit(&theirs, "src/b.js", &B_JS.replace("m3\n", "w3\n"));
    let again = told(&mine.dir, &fx.notices(&mine), later());
    assert!(again.contains("OVERLAP src/b.js"), "{again}");
    assert!(!again.contains("src/a.js"), "{again}");
    assert!(
        again.contains("1 overlap unchanged (`aver notices`)."),
        "{again}"
    );

    // `aver notices` prints everything, whatever was seen.
    let all = aver::coord::agent_text(&fx.notices(&mine), true);
    assert!(
        all.contains("src/a.js") && all.contains("src/b.js"),
        "{all}"
    );
}

#[test]
fn a_notice_is_marked_while_the_other_side_keeps_editing() {
    let fx = Fx::new();
    let (theirs, mine) = (
        fx.attempt("Migrate sessions", 1),
        fx.attempt("Rate limit", 2),
    );
    fx.edit(&mine, "src/a.js", &A_JS.replace("l3\n", "y3\n"));
    fx.edit(&theirs, "src/a.js", &A_JS.replace("l3\n", "x3\n"));
    // They edited just now: a report shows it, marked.
    let now = Timestamp::now();
    let report = seen::delta(&mine.dir, &fx.notices(&mine), now);
    let text = aver::coord::agent_text(&report, false);
    assert!(text.contains("OVERLAP src/a.js"), "{text}");
    assert!(text.contains("still changing"), "{text}");
    // Once they stop, it is not.
    let settled =
        aver::coord::agent_text(&seen::delta(&mine.dir, &fx.notices(&mine), later()), false);
    assert!(settled.contains("OVERLAP src/a.js"), "{settled}");
    assert!(!settled.contains("still changing"), "{settled}");
}

#[test]
fn verify_and_check_share_the_seen_set_and_say_unchanged() {
    let fx = Fx::new();
    let (theirs, mine) = (
        fx.attempt("Migrate sessions", 1),
        fx.attempt("Rate limit", 2),
    );
    fx.edit(&theirs, "src/a.js", &A_JS.replace("l3\n", "x3\n"));
    fx.edit(&mine, "src/a.js", &A_JS.replace("l3\n", "y3\n"));
    // `aver check` showed it.
    assert!(told(&mine.dir, &fx.notices(&mine), later()).contains("OVERLAP src/a.js"));
    // The verify report after it lists it as unchanged, in one line.
    let shown = seen::for_report(&mine.dir, Some(&fx.notices(&mine)), later()).unwrap();
    let text = aver::coord::agent_text(&shown, false);
    assert_eq!(text, "Other tasks: 1 overlap unchanged (`aver notices`).\n");
    // A new overlap one report showed is not repeated by the next.
    fx.edit(&theirs, "src/b.js", &B_JS.replace("m3\n", "x3\n"));
    fx.edit(&mine, "src/b.js", &B_JS.replace("m3\n", "y3\n"));
    let shown = seen::for_report(&mine.dir, Some(&fx.notices(&mine)), later()).unwrap();
    assert!(aver::coord::agent_text(&shown, false).contains("OVERLAP src/b.js"));
    assert!(!told(&mine.dir, &fx.notices(&mine), later()).contains("OVERLAP"));
}

/// After a merge of the main branch, dozens of files overlap: the output
/// stays within 2 KB and points to `aver notices` for the rest, and what
/// it left out is told next time.
#[test]
fn a_burst_of_overlaps_stays_within_two_kilobytes() {
    let w = World::new();
    let repo = w.dir.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    let names: Vec<String> = (0..40).map(|i| format!("src/f{i:02}.js")).collect();
    let files: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), A_JS)).collect();
    let base = w.snapshot(&files);
    let fx = Fx {
        coord: Coord::new(w.store.clone(), w.workspaces.clone()),
        w,
        repo,
        base,
    };
    let (theirs, mine) = (
        fx.attempt("Migrate sessions", 1),
        fx.attempt("Rate limit", 2),
    );
    for n in &names {
        fs::write(theirs.dir.join(n), A_JS.replace("l3\n", "x3\n")).unwrap();
        fs::write(mine.dir.join(n), A_JS.replace("l3\n", "y3\n")).unwrap();
    }
    fx.w.workspaces.sync(&theirs.key.id(), &theirs.ws).unwrap();
    let all = fx.notices(&mine);
    assert_eq!(all.notices.len(), 40);
    let full = aver::coord::agent_text(&all, false).len();
    let first = told(&mine.dir, &all, later());
    assert!(
        first.len() <= aver::coord::MAX_NOTICE_BYTES,
        "{}",
        first.len()
    );
    assert!(
        first.contains("more (`aver notices` lists them all)"),
        "{first}"
    );
    let second = told(&mine.dir, &all, later());
    assert!(!second.is_empty(), "the rest is told next time");
    assert!(second.contains("unchanged (`aver notices`)"), "{second}");
    eprintln!(
        "notice burst: {} bytes uncapped, {} and {} bytes in the first two reports",
        aver::coord::render(&all, true).text.len(),
        first.len(),
        second.len()
    );
    assert!(full <= aver::coord::MAX_NOTICE_BYTES);
}
