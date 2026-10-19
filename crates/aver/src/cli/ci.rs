use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{bail, Context};
use aver::ci::{self, summary, Ci, FailOn, PullRequest, Spec};
use aver::diagnostics::Timings;
use aver::env::DockerResolver;
use aver::keys::{self, Home};
use aver::services::Services;
use aver_core::Subject;
use serde_json::json;
use tempfile::TempDir;

mod approve;

const RECEIPT_FILE: &str = "receipt.json";
const DIAGNOSTICS_FILE: &str = "diagnostics.json";
const SUMMARY_FILE: &str = "summary.md";
const PUBLIC_KEY_FILE: &str = "public-key.txt";
/// The directory of run outputs, one `<check>.<side>.<n>.log` per run.
const RUNS_DIR: &str = "runs";

#[derive(clap::Args)]
#[command(
    group = clap::ArgGroup::new("criteria-source").args(["criteria", "pr_body", "github_event"]),
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
pub struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// The Git repository holding both commits.
    #[arg(long, default_value = ".")]
    repo: String,
    /// The revision the change is judged against.
    #[arg(long, required = true)]
    base: Option<String>,
    /// The revision to verify.
    #[arg(long, required = true)]
    head: Option<String>,
    /// Run only this check of the base's aver.toml, as one job of a matrix: the receipt covers it alone.
    #[arg(long, value_name = "ID")]
    check: Option<aver_core::CheckId>,
    /// A file with the criteria block: `- <id>: <text>` and `protected: <glob>` lines, with or without the ```aver fence.
    #[arg(long)]
    criteria: Option<PathBuf>,
    /// A pull request description (Markdown) that may contain an ```aver block.
    #[arg(long)]
    pr_body: Option<PathBuf>,
    /// A GitHub event payload; its pull_request.body may contain an ```aver block.
    #[arg(long)]
    github_event: Option<PathBuf>,
    /// Where receipt.json, diagnostics.json, summary.md and public-key.txt are written.
    #[arg(long, default_value = "aver-out")]
    out_dir: PathBuf,
    /// Which verdicts exit 1 (unexplained: FAIL_OWN and a BLOCKED_NOT_OWN the base does not explain; REVIEW and UNPROVEN exit 3).
    #[arg(long, value_enum, default_value_t = FailOn::Unexplained)]
    fail_on: FailOn,
    /// The receipt viewer page to link from summary.md.
    #[arg(long)]
    viewer_url: Option<String>,
    /// Keep state in this AVER_HOME instead of a throwaway one.
    #[arg(long)]
    home: Option<PathBuf>,
    /// Run every check as this user through `sudo -n -u`, so the code under test cannot read the throwaway verifier key (the receipt records uid "separate"). Required with --home.
    #[arg(long, value_name = "USER")]
    run_as: Option<String>,
    /// A label for the receipt, recorded in its signed ci.subject: 1-256 bytes, no control characters.
    #[arg(long)]
    subject: Option<Subject>,
    /// Exit 2 when the base has no aver.toml, instead of verifying with the default environment (one `npm test` check).
    #[arg(long)]
    require_config: bool,
    /// Refuse an import that holds a possible secret (exit 2) instead of listing it in diagnostics.json and summary.md.
    #[arg(long)]
    strict_secrets: bool,
    /// Refuse to run unless the criteria (normalized, as in the receipt's ci.criteria_text; empty when none) have this SHA-256.
    #[arg(long, value_name = "HEX")]
    criteria_sha256: Option<String>,
    /// Ignore cached runs: every check runs again (with --home, a home's cache would otherwise answer).
    #[arg(long)]
    fresh: bool,
    /// Run every check, also those the change does not affect (by default a check runs only when a changed file is in its scope, a lockfile it installs from or harness on the way to its directory).
    #[arg(long)]
    all: bool,
    /// Restore the base's runs from a record that `--record` wrote for the base commit, so they do not run again; a record made on another runner, for another base tree or with other dependency inputs is ignored with one line on stderr.
    #[arg(long, value_name = "DIR")]
    base_record: Option<PathBuf>,
    /// After the verdict, run the head's checks as a base would (cache hits where the verification already did) and record them in DIR, for later pull requests based on this commit.
    #[arg(long, value_name = "DIR")]
    record: Option<PathBuf>,
    /// The workflow file whose Git blob in the head keys records, instead of the one GITHUB_WORKFLOW_REF names.
    #[arg(long, value_name = "PATH")]
    workflow: Option<String>,
    /// Keep no base record in $AVER_HOME/records. By default, outside CI (none of CI, GITHUB_ACTIONS, GITLAB_CI, BUILDKITE, CIRCLECI, JENKINS_URL, TF_BUILD, TEAMCITY_VERSION set) and without --base-record or --record, the base's runs are read from the record an earlier local run wrote for this base tree, runner and dependency inputs (not with --fresh), and written there after.
    #[arg(long)]
    no_records: bool,
    /// Run the checks in slots cloned from the checkout. By default, when --repo is a checkout at --head with no changes to tracked files, they run in it, swapping its tracked files between the trees, and it is restored after.
    #[arg(long)]
    slots: bool,
    /// For a merge queue: no criteria, and only FAIL_OWN and a BLOCKED_NOT_OWN the base does not explain fail (exit 1); REVIEW and UNPROVEN exit 0 (--fail-on regressions).
    #[arg(long, conflicts_with_all = ["criteria", "pr_body", "fail_on"])]
    regression_only: bool,
}

#[derive(clap::Args)]
struct KeysArgs {
    /// The Git repository holding both commits.
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    /// The revision the change is judged against (default: from --github-event).
    #[arg(long)]
    base: Option<String>,
    /// The revision to verify (default: from --github-event).
    #[arg(long)]
    head: Option<String>,
    /// A GitHub event payload (pull_request, push or merge_group) naming the base and head.
    #[arg(long)]
    github_event: Option<PathBuf>,
    /// The workflow file whose Git blob in the head keys records, instead of the one GITHUB_WORKFLOW_REF names.
    #[arg(long, value_name = "PATH")]
    workflow: Option<String>,
    #[arg(long)]
    json: bool,
}

/// `aver ci keys`: Git plumbing and hashing, and the host's toolchain
/// probe for the executor id; nothing is imported.
fn keys(args: KeysArgs) -> anyhow::Result<ExitCode> {
    let event: Option<serde_json::Value> = match &args.github_event {
        Some(path) => Some(
            serde_json::from_slice(
                &std::fs::read(path).with_context(|| format!("reading {}", path.display()))?,
            )
            .with_context(|| format!("parsing the GitHub event {}", path.display()))?,
        ),
        None => None,
    };
    let from_event = |pointers: &[&str]| -> Option<String> {
        let event = event.as_ref()?;
        pointers
            .iter()
            .find_map(|p| event.pointer(p).and_then(|v| v.as_str()).map(str::to_owned))
    };
    let base = args
        .base
        .clone()
        .or_else(|| from_event(&["/pull_request/base/sha", "/merge_group/base_sha", "/before"]))
        .context("no base: pass --base, or --github-event with one")?;
    let head = args
        .head
        .clone()
        .or_else(|| from_event(&["/pull_request/head/sha", "/merge_group/head_sha", "/after"]))
        .context("no head: pass --head, or --github-event with one")?;
    let probe = TempDir::with_prefix("aver-keys-").context("creating a probe directory")?;
    let vars: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    let executor =
        aver::place::host::profile(&vars, &[], &probe.path().join("versions.json")).executor();
    let workflow = ci::record::workflow_blob(&args.repo, &head, args.workflow.as_deref())?;
    let runner = ci::record::RunnerKey::here(&executor, workflow);
    let keys = ci::record::keys(&args.repo, &base, &head, runner)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&keys)?);
    } else {
        println!("base={}\nhead={}", keys.base, keys.head);
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(clap::Subcommand)]
enum Command {
    /// Bind a pull request review's approval to the head commit it was given on (a pull_request_review event); runs no check. Exit 0 when one binds, 3 when none does.
    Approve(approve::Args),
    /// Print the actions/cache keys of this job's records: `base=` (restore it before `aver ci --base-record`) and `head=` (save `--record`'s directory under it).
    Keys(KeysArgs),
    /// Put back a checkout an interrupted `aver ci` left swapped: its head commit's tracked files and the untracked files it moved aside. A no-op when nothing is left to restore.
    Restore(RestoreArgs),
}

#[derive(clap::Args)]
struct RestoreArgs {
    /// The checkout `aver ci` ran in.
    #[arg(long, default_value = ".")]
    repo: PathBuf,
}

/// Runs CI mode. Any error also leaves a summary.md saying what went wrong,
/// so a pull request comment can show it.
pub fn run(home: &Home, mut args: Args) -> anyhow::Result<ExitCode> {
    let records = home_records(home, &args);
    match args.command.take() {
        Some(Command::Approve(args)) => return approve::run(home, args),
        Some(Command::Keys(args)) => return keys(args),
        Some(Command::Restore(restore)) => return restore_checkout(&restore.repo),
        None => {}
    }
    let started = std::time::Instant::now();
    let out_dir = args.out_dir.clone();
    for stale in [
        RECEIPT_FILE,
        DIAGNOSTICS_FILE,
        SUMMARY_FILE,
        PUBLIC_KEY_FILE,
    ] {
        match std::fs::remove_file(out_dir.join(stale)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(e)
                    .with_context(|| format!("removing {}", out_dir.join(stale).display()))
            }
            _ => {}
        }
    }
    match std::fs::remove_dir_all(out_dir.join(RUNS_DIR)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(e).with_context(|| format!("removing {}", out_dir.join(RUNS_DIR).display()))
        }
        _ => {}
    }
    execute(args, records, started).inspect_err(|e| {
        let _ = write(
            &out_dir,
            SUMMARY_FILE,
            &summary::render_error(&super::describe(e)),
        );
    })
}

/// The home's records directory when this run keeps base records there:
/// outside an ephemeral CI job, for a local repository, unless records are
/// named or turned off.
fn home_records(home: &Home, args: &Args) -> Option<PathBuf> {
    if args.no_records || args.base_record.is_some() || args.record.is_some() {
        return None;
    }
    if !Path::new(&args.repo).is_dir() || ci::record::ci_environment().is_some() {
        return None;
    }
    Some(ci::record::home_dir(home.root()))
}

fn execute(
    args: Args,
    home_records: Option<PathBuf>,
    started: std::time::Instant,
) -> anyhow::Result<ExitCode> {
    let (base_rev, head_rev) = (
        args.base.clone().expect("clap requires --base"),
        args.head.clone().expect("clap requires --head"),
    );
    let fail_on = if args.regression_only {
        FailOn::Regressions
    } else {
        args.fail_on
    };
    let (spec, pr) = read_spec(&args)?;
    if let Some(expected) = &args.criteria_sha256 {
        let actual = spec.sha256();
        if !expected.eq_ignore_ascii_case(&actual) {
            bail!(
                "--criteria-sha256: the criteria hash to {actual}, not {expected}; write the receipt's ci.criteria_text to the file passed with --criteria"
            );
        }
    }
    let throwaway;
    let home = match &args.home {
        Some(dir) => Home::new(dir),
        None => {
            throwaway = TempDir::with_prefix("aver-ci-").context("creating a throwaway home")?;
            Home::new(throwaway.path())
        }
    };
    let lock = home
        .lock(|| {
            eprintln!(
                "aver: waiting for another aver process using {}",
                home.root().display()
            )
        })
        .context("locking the Aver home")?;
    if let Some(user) = &args.run_as {
        if !aver::place::proc::sudo_works(user) {
            bail!("--run-as {user}: `sudo -n -u {user}` does not work here; create the user and allow passwordless sudo, or leave --run-as out");
        }
    } else if args.home.is_some() {
        bail!("--home keeps a verifier key that checks, running as this user, could read: pass --run-as <user> (a user that cannot read the home)");
    }
    keys::init_locked(&home, &lock).context("initializing keys")?;
    let services = Services::open(&home)?;
    let admin = services.key(None)?;
    let public_key = keys::AgentKey::read(&home.key_path(&keys::SERVER_KEY.parse()?))?
        .to_file()
        .public_key;
    let mut request = ci::Request::new(
        args.repo.clone(),
        base_rev.clone(),
        head_rev.clone(),
        spec,
        pr.as_ref(),
    );
    request.only = args.check.clone();
    request.subject = args.subject.clone();
    request.require_config = args.require_config;
    request.strict_secrets = args.strict_secrets;
    request.fresh = args.fresh;
    request.all = args.all;
    request.base_record = args.base_record.clone();
    request.record = args.record.clone();
    request.home_records = home_records;
    request.workflow = args.workflow.clone();

    toolchain_warnings(&args.repo, &base_rev);
    let in_place = in_place(&args, &head_rev, home.root())?;

    // Under another user, the slots live outside the home, which that user
    // cannot read.
    let native_root = match &args.run_as {
        Some(_) => Some(TempDir::with_prefix("aver-sides-").context("creating the slot root")?),
        None => None,
    };
    if let Some(root) = &native_root {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755))?;
    }
    let restore = RestoreOnDrop(in_place.clone());
    if in_place.is_some() {
        // A signal then stops the runs and lets the checkout be restored.
        aver::place::proc::interrupt::graceful();
    }
    let verified = tokio::runtime::Runtime::new()?.block_on(async {
        let mut config = aver::place::runner::RunnerConfig::for_process(
            home.root(),
            aver_core::ProvenanceMode::Ci,
        );
        config.run_as = args.run_as.clone();
        // Every side is cloned from the CI checkout, where the job's setup
        // installed the dependencies.
        let checkout = Path::new(&args.repo);
        if checkout.is_dir() {
            config.source = Some(checkout.canonicalize()?);
        }
        if let Some(root) = &native_root {
            config.root = Some(root.path().to_owned());
        }
        config.in_place = in_place.clone();
        let executor = services.executor_for(config);
        let id = executor.executor().to_owned();
        let ci = Ci::new(
            services.store.clone(),
            services.server_key.clone(),
            &admin,
            Arc::new(DockerResolver),
            executor,
        );
        anyhow::Ok((ci.run(request).await?, id))
    });
    drop(restore);
    let (mut outcome, executor) = verified?;
    if let ci::CiPlace::Slots { why: Some(why) } = &outcome.place {
        eprintln!("aver: ran the checks in slots cloned from the checkout: {why}");
    }

    let resolution = &outcome.resolution;
    if resolution.moved() {
        eprintln!(
            "aver: --base {} ({}) is not an ancestor of --head; verifying against their merge base {}",
            base_rev, resolution.requested, resolution.effective
        );
    }
    let verdict = outcome.receipt.body.verdict;
    let reproduce = local_reproduce(&args, &outcome);
    let markdown = summary::render(&summary::Context {
        outcome: &outcome,
        public_key: &public_key,
        executor: &executor,
        throwaway_key: args.home.is_none(),
        fail_on,
        viewer_url: args.viewer_url.as_deref(),
        reproduce: &reproduce,
    });
    let receipt = json!({ "receipt_id": outcome.receipt_id, "receipt": outcome.receipt });
    outcome.diagnostics.timings = Some(Timings::of(started.elapsed(), &outcome.runs));
    outcome.diagnostics.import = Some(outcome.import.clone());
    write(
        &args.out_dir,
        RECEIPT_FILE,
        &(serde_json::to_string_pretty(&receipt)? + "\n"),
    )?;
    write(
        &args.out_dir,
        DIAGNOSTICS_FILE,
        &(serde_json::to_string_pretty(&outcome.diagnostics)? + "\n"),
    )?;
    write(&args.out_dir, PUBLIC_KEY_FILE, &format!("{public_key}\n"))?;
    write(&args.out_dir, SUMMARY_FILE, &markdown)?;
    let runs = args.out_dir.join(RUNS_DIR);
    std::fs::create_dir_all(&runs).with_context(|| format!("creating {}", runs.display()))?;
    for log in &outcome.logs {
        let path = runs.join(&log.name);
        std::fs::write(&path, &log.output)
            .with_context(|| format!("writing {}", path.display()))?;
    }

    if let (Some(dir), Some(n)) = (&args.record, outcome.recorded) {
        eprintln!("aver: recorded {n} runs of the head in {}", dir.display());
    }
    if let Some(record) = outcome
        .base_record
        .as_ref()
        .filter(|r| r.source == ci::RecordSource::Home && r.served > 0)
    {
        eprintln!(
            "aver: reused {} base runs from the home's record; {} base runs started",
            record.served, record.started
        );
    }
    if let Some((dir, n)) = &outcome.home_recorded {
        eprintln!("aver: recorded {n} base runs in {}", dir.display());
    }
    let code = fail_on.exit_code(&outcome.receipt.body);
    println!(
        "{} {} (exit {code}); wrote {}",
        verdict.as_str(),
        outcome.receipt_id,
        args.out_dir.display()
    );
    Ok(ExitCode::from(code))
}

/// Says up front, on stderr, when the base's root `go.mod` or `Cargo.toml`
/// asks for a newer Go or Rust than this runner has: native runs use the
/// local toolchain, which then runs no test, and the verdict will be
/// `BLOCKED_NOT_OWN` for a reason the runner, not the change, can fix. Read
/// from the base commit's objects (`git cat-file`), never a working tree;
/// a remote repository is not checked here.
fn toolchain_warnings(repo: &str, base: &str) {
    let dir = Path::new(repo);
    if !dir.exists() {
        return;
    }
    let show = |file: &str| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["cat-file", "-p", &format!("{base}:{file}")])
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let (go_mod, cargo) = (show("go.mod"), show("Cargo.toml"));
    if go_mod.is_none() && cargo.is_none() {
        return;
    }
    let cwd = match show("rust-toolchain.toml").or_else(|| show("rust-toolchain")) {
        Some(_) => dir.to_owned(),
        None => std::env::temp_dir(),
    };
    let (go, rustc) = aver::exec::toolchain::local_versions(&cwd);
    for m in aver::exec::toolchain::mismatches(
        go_mod.as_deref(),
        cargo.as_deref(),
        go.as_deref(),
        rustc.as_deref(),
    ) {
        eprintln!("aver: warning: {m} (fix: {})", m.fix);
    }
}

/// The spec and pull request from whichever source was given; none is
/// regression-only.
fn read_spec(args: &Args) -> anyhow::Result<(Spec, Option<PullRequest>)> {
    let read =
        |path: &Path| std::fs::read(path).with_context(|| format!("reading {}", path.display()));
    let markdown = |text: &str, path: &Path| -> anyhow::Result<Spec> {
        let spec = ci::spec::from_markdown(text)
            .with_context(|| format!("parsing the aver block in {}", path.display()))?;
        Ok(spec.unwrap_or_else(Spec::regression_only))
    };
    if let Some(path) = &args.criteria {
        let text = String::from_utf8(read(path)?).context("the criteria file is not UTF-8")?;
        let spec =
            ci::spec::from_file(&text).with_context(|| format!("parsing {}", path.display()))?;
        return Ok((spec, None));
    }
    if let Some(path) = &args.pr_body {
        let text = String::from_utf8(read(path)?).context("the pull request body is not UTF-8")?;
        return Ok((markdown(&text, path)?, None));
    }
    if let Some(path) = &args.github_event {
        let pr = ci::pull_request(&read(path)?)
            .with_context(|| format!("parsing the GitHub event {}", path.display()))?;
        if args.regression_only {
            return Ok((Spec::regression_only(), pr));
        }
        let spec = match pr.as_ref().and_then(|p| p.body.as_deref()) {
            Some(body) => markdown(body, path)?,
            None => Spec::regression_only(),
        };
        return Ok((spec, pr));
    }
    Ok((Spec::regression_only(), None))
}

/// The receipt's `reproduce` command as a shell snippet for whoever reads
/// the summary: the criteria file written first, and `<repo>` (a local
/// repository) replaced by the path this run used.
fn local_reproduce(args: &Args, outcome: &ci::Outcome) -> String {
    let receipt = &outcome.receipt.body;
    let command = receipt.reproduce.replacen(
        "--repo <repo>",
        &format!("--repo {}", ci::shell_quote(&args.repo)),
        1,
    );
    match receipt.ci.as_ref().and_then(|c| c.criteria_text.as_deref()) {
        Some(text) => format!(
            "cat > {} <<'AVER'\n{text}\nAVER\n{command}",
            ci::CRITERIA_FILE
        ),
        None => command,
    }
}

fn write(dir: &Path, name: &str, contents: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(name);
    std::fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))
}

/// The checkout to run the checks in (ARCHITECTURE.md §2.9), or `None` for
/// slots: `--repo` must be the top of a local checkout at `--head` with no
/// changes to tracked files, holding no Aver home. Says on stderr why not,
/// unless `--slots` asked for slots. A checkout an interrupted run left
/// swapped is restored first.
fn in_place(args: &Args, head: &str, home: &Path) -> anyhow::Result<Option<PathBuf>> {
    let dir = Path::new(&args.repo);
    if aver::place::workdir::ci_journal(dir).is_file() {
        restore_checkout(dir)?;
    }
    if args.slots {
        return Ok(None);
    }
    let why = match checkout_at(dir, head, home) {
        Ok(dir) => return Ok(Some(dir)),
        Err(why) => why,
    };
    eprintln!("aver: running the checks in slots cloned from the checkout: {why}");
    Ok(None)
}

fn checkout_at(dir: &Path, head: &str, home: &Path) -> Result<PathBuf, String> {
    let dir = dir
        .canonicalize()
        .map_err(|_| format!("{} is not a local directory", dir.display()))?;
    if !dir.join(".git").is_dir() {
        return Err(format!(
            "{} is not the top of a Git checkout with its own .git directory",
            dir.display()
        ));
    }
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(args)
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    let commit = |rev: &str| {
        git(&[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{rev}^{{commit}}"),
        ])
    };
    let at = commit("HEAD");
    if at.is_none() || at != commit(head) {
        return Err(format!(
            "the checkout is at {}, not at --head {head}",
            at.as_deref().unwrap_or("no commit")
        ));
    }
    match git(&["status", "--porcelain", "--untracked-files=no"]) {
        Some(s) if s.is_empty() => {}
        _ => return Err("the checkout has changes to tracked files".into()),
    }
    if home.canonicalize().is_ok_and(|h| h.starts_with(&dir)) {
        return Err("the Aver home is inside the checkout".into());
    }
    Ok(dir)
}

/// Restores the adopted checkout when dropped: on every way out of a
/// verification, a panic's unwinding included.
struct RestoreOnDrop(Option<PathBuf>);

impl Drop for RestoreOnDrop {
    fn drop(&mut self) {
        let Some(dir) = &self.0 else { return };
        if !aver::place::workdir::ci_journal(dir).is_file() {
            return;
        }
        if let Err(e) = aver::place::workdir::Workdir::restore(dir) {
            eprintln!(
                "aver: restoring {}: {e}; run `aver ci restore --repo {}`",
                dir.display(),
                ci::shell_quote(&dir.to_string_lossy())
            );
        }
    }
}

/// `aver ci restore`: idempotent, and a no-op with nothing to restore.
fn restore_checkout(dir: &Path) -> anyhow::Result<ExitCode> {
    use aver::place::workdir::{Workdir, WorkdirError};
    match Workdir::restore(dir) {
        Ok(r) => eprintln!(
            "aver: restored {} to its head: {} files written, {} removed, {} untracked put back",
            dir.display(),
            r.written,
            r.removed,
            r.returned
        ),
        Err(WorkdirError::NotAdopted(_)) => {
            eprintln!("aver: {} has nothing to restore", dir.display())
        }
        Err(e) => return Err(e).with_context(|| format!("restoring {}", dir.display())),
    }
    Ok(ExitCode::SUCCESS)
}
