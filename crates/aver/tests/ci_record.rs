//! Cross-PR base records (ARCHITECTURE.md §2.9.2): `aver ci --record` in
//! one throwaway home, `aver ci --base-record` in another, `aver ci keys`,
//! and the fresh confirmation of a `NEW` that rests on a recorded base. The
//! `aver` binary on `fixtures/sh-calc`, whose checks are shell only.

mod common;
mod git_support;

use std::path::{Path as FsPath, PathBuf};
use std::process::{Command, Output};

use common::fixture;
use git_support::git_cmd;
use tempfile::TempDir;

const PROBE: &str = "AVER_TEST_HOST_PROBE";
/// Read by the `calc.sh` of [`flaky_base_then_broken`]: set, `add` answers
/// 0, as a base failing for a reason of the moment would.
const FLAKE: &str = "AVER_TEST_BASE_FLAKE";

/// A repository of `fixtures/sh-calc` plus `go.sum` (a dependency input),
/// whose tests also probe the host for [`PROBE`]: one commit per edit
/// list, each applied on the one before. Returns the directory and the
/// commits in order.
struct Repo {
    dir: TempDir,
    commits: Vec<String>,
    /// The `AVER_HOME` every `aver ci` here runs under: where the home's
    /// base records go.
    home: TempDir,
}

impl Repo {
    fn new(edits: &[&[(&str, &str)]]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        copy_dir(&fixture("sh-calc"), dir.path());
        let toml = dir.path().join("aver.toml");
        let mut text = std::fs::read_to_string(&toml).unwrap();
        text.push_str("\n[[deps]]\nlockfile = \"go.sum\"\n");
        std::fs::write(&toml, text).unwrap();
        std::fs::write(dir.path().join("go.sum"), "one v1\n").unwrap();
        let tests = dir.path().join("test/add.tests");
        let mut text = std::fs::read_to_string(&tests).unwrap();
        text.push_str(&format!("host probe :: [ -z \"${PROBE}\" ]\n"));
        std::fs::write(&tests, text).unwrap();
        Self::commit(dir, edits)
    }

    /// The files already in `dir` as the first commit, then one commit per
    /// edit list.
    fn commit(dir: TempDir, edits: &[&[(&str, &str)]]) -> Self {
        let git = |args: &[&str]| -> String {
            let out = git_cmd(dir.path()).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "calc"]);
        let mut commits = vec![git(&["rev-parse", "HEAD"])];
        for (n, edit) in edits.iter().enumerate() {
            git(&["checkout", "-q", &commits[commits.len() - 1]]);
            for (path, contents) in *edit {
                let at = dir.path().join(path);
                std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                std::fs::write(at, contents).unwrap();
            }
            git(&["add", "-A"]);
            git(&["commit", "-q", "-m", &format!("edit {n}")]);
            commits.push(git(&["rev-parse", "HEAD"]));
        }
        Self {
            dir,
            commits,
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> &FsPath {
        self.dir.path()
    }

    /// The home's base records.
    fn records(&self) -> PathBuf {
        self.home.path().join("records")
    }
}

fn copy_dir(from: &FsPath, to: &FsPath) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// The `aver` binary with none of the runner variables a key reads, and
/// none that mark a CI job, so each test sets the ones it means.
fn aver() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aver"));
    for var in [
        "AVER_SCRIPTED",
        "ImageOS",
        "ImageVersion",
        "GITHUB_WORKFLOW_REF",
        PROBE,
        FLAKE,
    ]
    .into_iter()
    .chain(aver::ci::record::CI_VARS)
    {
        cmd.env_remove(var);
    }
    cmd
}

fn ci(repo: &Repo, base: usize, head: usize, out: &FsPath, extra: &[&str]) -> Command {
    let mut cmd = aver();
    cmd.env("AVER_HOME", repo.home.path())
        .arg("ci")
        .arg("--repo")
        .arg(repo.path())
        .args(["--base", &repo.commits[base], "--head", &repo.commits[head]])
        .arg("--out-dir")
        .arg(out)
        .args(extra);
    cmd
}

fn run(cmd: &mut Command) -> Output {
    let out = cmd.output().unwrap();
    assert!(
        matches!(out.status.code(), Some(0 | 1 | 3)),
        "aver: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn json(path: PathBuf) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
}

fn verdict(out: &FsPath) -> String {
    json(out.join("receipt.json"))["receipt"]["verdict"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The base side's runs in diagnostics: (cache hit, run id) each.
fn base_runs(out: &FsPath) -> Vec<(bool, String)> {
    json(out.join("diagnostics.json"))["timings"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["side"] == "base")
        .map(|r| {
            (
                r["cache_hit"].as_bool().unwrap(),
                r["run"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// The causes of the receipt's entries, by id.
fn causes(out: &FsPath) -> Vec<(String, String)> {
    json(out.join("receipt.json"))["receipt"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| {
            Some((
                e["id"].as_str()?.to_owned(),
                e["cause"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

fn summary(out: &FsPath) -> String {
    std::fs::read_to_string(out.join("summary.md")).unwrap()
}

/// commits[1] is the default branch's push (recorded), commits[2] a pull
/// request on it that touches only a comment.
fn pushed_then_pr(pr: &[(&str, &str)]) -> (Repo, TempDir) {
    let calc = std::fs::read_to_string(fixture("sh-calc").join("calc.sh")).unwrap();
    let pushed = format!("{calc}# pushed\n");
    let repo = Repo::new(&[&[("calc.sh", &pushed)], pr]);
    let tmp = tempfile::tempdir().unwrap();
    run(&mut ci(
        &repo,
        0,
        1,
        &tmp.path().join("push"),
        &["--record", tmp.path().join("record").to_str().unwrap()],
    ));
    (repo, tmp)
}

fn comment_pr() -> String {
    let calc = std::fs::read_to_string(fixture("sh-calc").join("calc.sh")).unwrap();
    format!("{calc}# pushed\n# pr\n")
}

/// A push records its head; a pull request on it, in another throwaway
/// home, restores the record and starts no base run, and reaches the
/// verdict a run without the record reaches.
#[test]
fn a_restored_record_starts_no_base_run_and_gives_the_same_verdict() {
    let (repo, tmp) = pushed_then_pr(&[("calc.sh", &comment_pr())]);
    let record = tmp.path().join("record");
    assert!(record.join("record.json").is_file());

    let plain = tmp.path().join("plain");
    run(&mut ci(&repo, 1, 2, &plain, &[]));

    let restored = tmp.path().join("restored");
    run(&mut ci(
        &repo,
        1,
        2,
        &restored,
        &["--base-record", record.to_str().unwrap()],
    ));
    assert_eq!(verdict(&restored), verdict(&plain));
    assert_eq!(verdict(&restored), "PASS");
    assert!(
        summary(&restored).contains("1 base run was recorded by an earlier job")
            && summary(&restored).contains("0 base runs started on this runner"),
        "{}",
        summary(&restored)
    );
}

/// A record made on another runner (another `ImageVersion`), or for a
/// pull request whose head changed a dependency input, is ignored with one
/// line on stderr, and the base runs here.
#[test]
fn a_record_from_another_runner_or_dependency_inputs_is_ignored() {
    let (repo, tmp) = pushed_then_pr(&[("calc.sh", &comment_pr())]);
    let record = tmp.path().join("record");
    let restore = ["--base-record", record.to_str().unwrap()];

    let other = tmp.path().join("other-image");
    let out = run(ci(&repo, 1, 2, &other, &restore).env("ImageVersion", "20991231.1"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr.matches("ignored").count(), 1, "{stderr}");
    assert!(stderr.contains("on another runner"), "{stderr}");
    assert!(
        summary(&other).contains("was ignored"),
        "{}",
        summary(&other)
    );

    let calc = comment_pr();
    let deps_repo = Repo::new(&[&[("calc.sh", calc.as_str())], &[("go.sum", "one v2\n")]]);
    let tmp2 = tempfile::tempdir().unwrap();
    let record2 = tmp2.path().join("record");
    run(&mut ci(
        &deps_repo,
        0,
        1,
        &tmp2.path().join("push"),
        &["--record", record2.to_str().unwrap()],
    ));
    let changed = tmp2.path().join("changed-deps");
    let out = run(&mut ci(
        &deps_repo,
        1,
        2,
        &changed,
        &["--base-record", record2.to_str().unwrap()],
    ));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("with other dependency inputs"), "{stderr}");
    assert!(
        summary(&changed).contains("was ignored"),
        "{}",
        summary(&changed)
    );
}

/// A test that passed on the recorded base and fails on the head, because
/// of the runner and not the change, fails a fresh base run too: the base
/// side runs here, and the entry is PREEXISTING, not NEW.
#[test]
fn a_new_entry_that_the_fresh_base_also_fails_is_preexisting() {
    let (repo, tmp) = pushed_then_pr(&[("calc.sh", &comment_pr())]);
    let out = tmp.path().join("probe");
    run(ci(
        &repo,
        1,
        2,
        &out,
        &["--base-record", tmp.path().join("record").to_str().unwrap()],
    )
    .env(PROBE, "1"));
    let probe: Vec<_> = causes(&out)
        .into_iter()
        .filter(|(id, _)| id.contains("host probe"))
        .collect();
    assert_eq!(probe.len(), 1, "{:?}", causes(&out));
    assert_eq!(probe[0].1, "PREEXISTING", "{}", summary(&out));
    assert_eq!(verdict(&out), "PASS", "{}", summary(&out));
    assert!(
        base_runs(&out).iter().any(|(hit, _)| !hit),
        "a fresh base run judged it"
    );
}

/// A test the head really breaks passes the fresh base run, so the entry
/// stays NEW; the fresh run is what confirmed it.
#[test]
fn a_new_entry_that_the_fresh_base_passes_stays_new() {
    let broken = "add() { echo $(($1 - $2)); }\n# pushed\n";
    let (repo, tmp) = pushed_then_pr(&[("calc.sh", broken)]);
    let out = tmp.path().join("broken");
    run(&mut ci(
        &repo,
        1,
        2,
        &out,
        &["--base-record", tmp.path().join("record").to_str().unwrap()],
    ));
    let new: Vec<_> = causes(&out)
        .into_iter()
        .filter(|(_, cause)| cause == "NEW")
        .collect();
    assert!(!new.is_empty(), "{}", summary(&out));
    assert_eq!(verdict(&out), "FAIL_OWN");
    let base = base_runs(&out);
    let (recorded, fresh): (Vec<_>, Vec<_>) = base.iter().partition(|(hit, _)| *hit);
    assert!(!recorded.is_empty(), "the recorded runs stand: {base:?}");
    assert_eq!(fresh.len(), 1, "the confirmation is listed: {base:?}");
    let logs: Vec<String> = std::fs::read_dir(out.join("runs"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(logs.iter().any(|l| l.ends_with("-confirm.log")), "{logs:?}");
    assert!(
        summary(&out).contains("checked by 1 fresh base run first"),
        "{}",
        summary(&out)
    );
}

fn keys(repo: &Repo, base: usize, head: usize, extra: &[&str]) -> (String, String) {
    let out = aver()
        .args(["ci", "keys", "--repo"])
        .arg(repo.path())
        .args(["--base", &repo.commits[base], "--head", &repo.commits[head]])
        .args(extra)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    let get = |name: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("no {name}= in {text}"))
            .to_owned()
    };
    (get("base"), get("head"))
}

/// The same inputs give the same keys; a pull request that changes only
/// source restores the record its base's push saved; a changed workflow
/// file or dependency input changes the key.
#[test]
fn ci_keys_are_stable_and_name_the_runner_and_dependency_inputs() {
    let calc = comment_pr();
    let repo = Repo::new(&[
        &[(".github/workflows/ci.yml", "on: push\n")],
        &[("calc.sh", &calc)],
        &[("go.sum", "one v2\n")],
        &[(".github/workflows/ci.yml", "on: pull_request\n")],
    ]);
    let wf = ["--workflow", ".github/workflows/ci.yml"];
    let first = keys(&repo, 1, 2, &wf);
    assert_eq!(first, keys(&repo, 1, 2, &wf), "deterministic");
    assert!(first.0.starts_with("aver-base-") && first.1.starts_with("aver-base-"));
    assert_eq!(
        keys(&repo, 0, 1, &wf).1,
        first.0,
        "the push of commit 1 saves what a pull request on it restores"
    );
    assert_ne!(keys(&repo, 1, 3, &wf).0, first.0, "a changed lockfile");
    assert_ne!(
        keys(&repo, 1, 4, &wf).0,
        keys(&repo, 1, 3, &wf).0,
        "a changed workflow"
    );
    let image = aver()
        .args(["ci", "keys", "--repo"])
        .arg(repo.path())
        .args(["--base", &repo.commits[1], "--head", &repo.commits[2]])
        .args(wf)
        .env("ImageVersion", "20991231.1")
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&image.stdout).contains(&first.0),
        "another image"
    );
}

/// Beyond the inputs `aver.toml` names, the key follows the files that
/// identify what an install put in the checkout: a pnpm repository's
/// `package.json` or `pnpm-lock.yaml` changing changes it, a source edit
/// does not.
#[test]
fn ci_keys_follow_package_manifests_the_contract_does_not_name() {
    let plain = std::fs::read_to_string(fixture("sh-calc").join("aver.toml")).unwrap();
    let calc = comment_pr();
    let repo = Repo::new(&[
        &[
            ("aver.toml", plain.as_str()),
            ("go.sum", ""),
            ("package.json", "{\"name\": \"calc\"}\n"),
            ("pnpm-lock.yaml", "lockfileVersion: '9.0'\n"),
        ],
        &[("calc.sh", &calc)],
        &[(
            "package.json",
            "{\"name\": \"calc\", \"version\": \"2.0.0\"}\n",
        )],
        &[("pnpm-lock.yaml", "lockfileVersion: '9.1'\n")],
    ]);
    let wf = ["--workflow", ".github/workflows/ci.yml"];
    let source = keys(&repo, 1, 2, &wf).0;
    assert_eq!(keys(&repo, 1, 1, &wf).0, source, "a source edit");
    assert_ne!(keys(&repo, 1, 3, &wf).0, source, "a changed package.json");
    assert_ne!(keys(&repo, 1, 4, &wf).0, source, "a changed pnpm lockfile");
}

// The home's base records: a local `aver ci` (no CI variable, no
// `--record` or `--base-record`) reads its base from `$AVER_HOME/records`
// and writes it back.

/// The `aver-base-*` directories holding a record in the home.
fn home_records(repo: &Repo) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(repo.records()) else {
        return Vec::new();
    };
    entries
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("record.json").is_file())
        .collect()
}

fn comment_repo() -> Repo {
    let calc = std::fs::read_to_string(fixture("sh-calc").join("calc.sh")).unwrap();
    let one = format!("{calc}# one\n");
    let two = format!("{calc}# one\n# two\n");
    Repo::new(&[&[("calc.sh", &one)], &[("calc.sh", &two)]])
}

/// The second local run against a base starts no base run: every base run
/// is the record's, and it reaches the first run's verdict.
#[test]
fn a_second_local_run_reads_its_base_from_the_home_record() {
    let repo = comment_repo();
    let tmp = tempfile::tempdir().unwrap();
    let first = tmp.path().join("first");
    let out = run(&mut ci(&repo, 0, 1, &first, &[]));
    assert_eq!(home_records(&repo).len(), 1);
    assert!(
        base_runs(&first).iter().all(|(hit, _)| !hit),
        "the warm's runs started here: {:?}",
        base_runs(&first)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("recorded 1 base runs in"), "{stderr}");
    assert!(!stderr.contains("reused"), "{stderr}");

    let second = tmp.path().join("second");
    let out = run(&mut ci(&repo, 0, 1, &second, &[]));
    let base = base_runs(&second);
    assert!(
        !base.is_empty() && base.iter().all(|(hit, _)| *hit),
        "{base:?}"
    );
    let ids = |runs: Vec<(bool, String)>| runs.into_iter().map(|(_, id)| id).collect::<Vec<_>>();
    assert_eq!(
        ids(base),
        ids(base_runs(&first)),
        "the record's runs, by id"
    );
    assert_eq!(verdict(&second), verdict(&first));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("reused 1 base runs from the home's record; 0 base runs started"),
        "{stderr}"
    );
    assert!(
        summary(&second).contains("recorded by an earlier `aver ci` on this machine"),
        "{}",
        summary(&second)
    );
}

/// `--fresh` reads no record but writes one; `--no-records` and a CI
/// environment neither read nor write one. A run that reads no record
/// starts its own base runs, whose ids differ from the record's.
#[test]
fn fresh_no_records_and_ci_variables_keep_the_home_record_out() {
    let repo = comment_repo();
    let tmp = tempfile::tempdir().unwrap();
    let ids = |out: &FsPath| -> Vec<String> {
        let base: Vec<String> = base_runs(out).into_iter().map(|(_, id)| id).collect();
        assert!(!base.is_empty());
        base
    };

    run(&mut ci(
        &repo,
        0,
        1,
        &tmp.path().join("off"),
        &["--no-records"],
    ));
    run(ci(&repo, 0, 1, &tmp.path().join("ci"), &[]).env("CI", "true"));
    run(ci(&repo, 0, 1, &tmp.path().join("gha"), &[]).env("GITHUB_ACTIONS", "true"));
    assert!(home_records(&repo).is_empty(), "{:?}", home_records(&repo));

    let fresh = tmp.path().join("fresh");
    run(&mut ci(&repo, 0, 1, &fresh, &["--fresh"]));
    assert_eq!(home_records(&repo).len(), 1, "--fresh writes the record");
    let fresh2 = tmp.path().join("fresh2");
    let out = run(&mut ci(&repo, 0, 1, &fresh2, &["--fresh"]));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("reused"));
    let recorded = ids(&fresh2);
    assert_ne!(recorded, ids(&fresh), "--fresh read no record");

    let off = tmp.path().join("off2");
    let out = run(&mut ci(&repo, 0, 1, &off, &["--no-records"]));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("reused"));
    assert_ne!(ids(&off), recorded, "--no-records read no record");
    let ci_env = tmp.path().join("ci2");
    let out = run(ci(&repo, 0, 1, &ci_env, &[]).env("BUILDKITE", "true"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("reused"));
    assert_ne!(ids(&ci_env), recorded, "a CI job read no record");

    let read = tmp.path().join("read");
    run(&mut ci(&repo, 0, 1, &read, &[]));
    assert_eq!(
        ids(&read),
        recorded,
        "the second --fresh run wrote the record"
    );
}

/// A record in the home under this base's key that was made for another
/// base tree is ignored with one line on stderr, and the base runs here.
#[test]
fn a_home_record_made_under_another_key_is_ignored() {
    let repo = comment_repo();
    let tmp = tempfile::tempdir().unwrap();
    run(&mut ci(&repo, 0, 1, &tmp.path().join("a"), &[]));
    let of_zero = home_records(&repo);
    run(&mut ci(&repo, 1, 2, &tmp.path().join("b"), &[]));
    let of_one: Vec<_> = home_records(&repo)
        .into_iter()
        .filter(|p| !of_zero.contains(p))
        .collect();
    assert_eq!((of_zero.len(), of_one.len()), (1, 1));
    std::fs::remove_dir_all(&of_one[0]).unwrap();
    copy_dir(&of_zero[0], &of_one[0]);

    let out_dir = tmp.path().join("stale");
    let out = run(&mut ci(&repo, 1, 2, &out_dir, &[]));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr.matches("ignored").count(), 1, "{stderr}");
    assert!(stderr.contains("for another base tree"), "{stderr}");
    assert!(!stderr.contains("reused"), "{stderr}");
    assert!(
        summary(&out_dir).contains("was ignored"),
        "{}",
        summary(&out_dir)
    );
}

/// commits[1] is a base whose `add` answers 0 while [`FLAKE`] is set,
/// commits[2] a head that really breaks `add`.
fn flaky_base_then_broken() -> Repo {
    let base =
        format!("add() {{ if [ -n \"${FLAKE}\" ]; then echo 0; else echo $(($1 + $2)); fi; }}\n");
    Repo::new(&[
        &[("calc.sh", &base)],
        &[("calc.sh", "add() { echo $(($1 - $2)); }\n")],
    ])
}

/// A record whose base failed a test for a reason of the moment would call
/// the head's real break of it PREEXISTING. A head failure the record
/// alone explains gets a fresh base run first; that passes, so the base
/// runs here and the entry is NEW.
#[test]
fn a_head_failure_only_a_recorded_base_failure_explains_is_checked_fresh() {
    let repo = flaky_base_then_broken();
    let tmp = tempfile::tempdir().unwrap();
    let flaky = tmp.path().join("flaky");
    run(ci(&repo, 1, 2, &flaky, &[]).env(FLAKE, "1"));
    assert!(
        causes(&flaky)
            .iter()
            .any(|(id, cause)| id.contains("adds two numbers") && cause == "PREEXISTING"),
        "{:?}",
        causes(&flaky)
    );

    let out = tmp.path().join("broken");
    run(&mut ci(&repo, 1, 2, &out, &[]));
    let adds: Vec<_> = causes(&out)
        .into_iter()
        .filter(|(id, _)| id.contains("adds two numbers"))
        .collect();
    assert_eq!(adds.len(), 1, "{:?}", causes(&out));
    assert_eq!(adds[0].1, "NEW", "{}", summary(&out));
    assert_eq!(verdict(&out), "FAIL_OWN");
    assert!(
        summary(&out).contains("did not hold here: a fresh base run disagreed with it"),
        "{}",
        summary(&out)
    );
}

/// `aver gc` removes a home record unwritten for over 14 days and keeps
/// the one a run just wrote; beyond the newest kept, older ones go too.
#[test]
fn gc_prunes_old_home_records() {
    let repo = comment_repo();
    let tmp = tempfile::tempdir().unwrap();
    run(&mut ci(&repo, 0, 1, &tmp.path().join("a"), &[]));
    let live = home_records(&repo);
    assert_eq!(live.len(), 1);
    let old = repo.records().join("aver-base-old");
    copy_dir(&live[0], &old);
    let day = std::time::Duration::from_secs(24 * 3600);
    std::fs::File::options()
        .write(true)
        .open(old.join("record.json"))
        .unwrap()
        .set_modified(std::time::SystemTime::now() - 15 * day)
        .unwrap();

    let out = aver()
        .env("AVER_HOME", repo.home.path())
        .arg("gc")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("removed 1 base records"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(home_records(&repo), live);

    run(&mut ci(&repo, 1, 2, &tmp.path().join("b"), &[]));
    assert_eq!(home_records(&repo).len(), 2);
    let removed = aver::ci::record::prune(&repo.records(), 14 * day, 1).unwrap();
    assert_eq!(removed, 1);
    assert_eq!(home_records(&repo).len(), 1);
}

const PY_CONTRACT: &str = "[[check]]\nid = \"py\"\nrun = \"python3 -m pytest -p no:cacheprovider -o junit_family=xunit1 --junitxml=$AVER_OUT/py.xml\"\nreport = \"junit\"\nmin_tests = 1\n";
const PY_CALC: &str = "def add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b\n";

/// A pytest project whose tests also probe the host for [`PROBE`]; a
/// push (commits[1]) recorded, and a pull request (commits[2]) with `pr`.
fn py_pushed_then_pr(pr: &[(&str, &str)]) -> (Repo, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let probe = format!(
        "import os\n\n\ndef test_host_probe():\n    assert not os.environ.get(\"{PROBE}\")\n"
    );
    for (path, text) in [
        ("aver.toml", PY_CONTRACT),
        ("calc.py", PY_CALC),
        (
            "tests/test_add.py",
            "from calc import add\n\n\ndef test_add():\n    assert add(2, 3) == 5\n",
        ),
        (
            "tests/test_sub.py",
            "from calc import sub\n\n\ndef test_sub():\n    assert sub(5, 3) == 2\n",
        ),
        ("tests/test_probe.py", probe.as_str()),
    ] {
        let at = dir.path().join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
    let pushed = format!("{PY_CALC}# pushed\n");
    let repo = Repo::commit(dir, &[&[("calc.py", &pushed)], pr]);
    let tmp = tempfile::tempdir().unwrap();
    run(&mut ci(
        &repo,
        0,
        1,
        &tmp.path().join("push"),
        &["--record", tmp.path().join("record").to_str().unwrap()],
    ));
    (repo, tmp)
}

/// The output of the fresh confirmation's runs.
fn confirm_logs(out: &FsPath) -> String {
    let mut logs: Vec<PathBuf> = std::fs::read_dir(out.join("runs"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with("-confirm.log"))
        .collect();
    logs.sort();
    logs.iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect()
}

/// The fresh confirmation of a head failure resting on a record runs only
/// the tests the head failed: a real break passes there and stays NEW,
/// and a failure of the moment fails there too, so the base side reruns
/// in full and the entry is PREEXISTING, as an unfocused confirmation
/// would judge each.
#[test]
#[ignore = "runs pytest natively: needs Python 3 and pytest on PATH"]
fn a_record_confirmation_runs_only_the_tests_the_head_failed() {
    let broken = PY_CALC.replace("return a - b", "return a + b");
    let (repo, tmp) = py_pushed_then_pr(&[("calc.py", &format!("{broken}# pushed\n"))]);
    let record = tmp.path().join("record");
    let out = tmp.path().join("broken");
    run(&mut ci(
        &repo,
        1,
        2,
        &out,
        &["--base-record", record.to_str().unwrap()],
    ));
    assert_eq!(verdict(&out), "FAIL_OWN", "{}", summary(&out));
    let new: Vec<_> = causes(&out)
        .into_iter()
        .filter(|(_, c)| c == "NEW")
        .collect();
    assert!(new.len() == 1 && new[0].0.ends_with("test_sub"), "{new:?}");
    let logs = confirm_logs(&out);
    assert!(
        logs.contains("1 passed") && !logs.contains("3 passed"),
        "{logs}"
    );

    let (repo, tmp) = py_pushed_then_pr(&[("calc.py", &format!("{PY_CALC}# pushed\n# pr\n"))]);
    let record = tmp.path().join("record");
    let out = tmp.path().join("probe");
    run(ci(
        &repo,
        1,
        2,
        &out,
        &["--base-record", record.to_str().unwrap()],
    )
    .env(PROBE, "1"));
    assert_eq!(verdict(&out), "PASS", "{}", summary(&out));
    let probe: Vec<_> = causes(&out)
        .into_iter()
        .filter(|(id, _)| id.ends_with("test_host_probe"))
        .collect();
    assert_eq!(probe.len(), 1, "{:?}", causes(&out));
    assert_eq!(probe[0].1, "PREEXISTING", "{}", summary(&out));
    let logs = confirm_logs(&out);
    assert!(
        logs.contains("1 failed") && !logs.contains("2 passed"),
        "{logs}"
    );
}
