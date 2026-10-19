//! `aver init`'s suggestion: the checks a repository declares that Aver's
//! automatic detection (the default check and [`crate::presets`]) does not
//! run, drafted as an `aver.toml`, and a dry run of that draft on `HEAD`.
//!
//! What the repository declares is read from its CI workflows
//! (`.github/workflows/*.yml` `run:` steps that look like test, lint or type
//! check commands), its Makefile's `test`, `check` and `lint` targets, its
//! `package.json` scripts (nested packages too), its justfile, `tox.ini`,
//! `noxfile.py` and pyproject's pytest table. CI configuration is input
//! only: a step that needs a secret, a service container, a GitHub
//! expression or the network is dropped with the reason, since Aver runs
//! checks with none of those.
//!
//! The draft keeps every check detection already makes (they would stop
//! running once `aver.toml` lists checks), adds the declared commands it
//! does not cover, each with its runner's report where [`adhoc::check_for`]
//! knows one, and is parsed by [`env::environment_with_config`] before it is
//! shown, so what `--yes` writes is what the parser accepts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path as FsPath;
use std::sync::Arc;
use std::time::Duration;

use aver_core::pushback::MAX_CANDIDATES;
use aver_core::{CheckDef, Environment, Node, Path, ReportFormat, TestStatus, TreeId};
use serde::Serialize;

use crate::adhoc;
use crate::env::{self, Detection, EnvError, ObjectSource, StaticResolver};
use crate::exec::output::{Exit, SandboxLimits};
use crate::place::proc::ProcFailure;
use crate::place::runner::{RunnerConfig, SlotRequest, SlotRunner};
use crate::store::StoreError;

/// Workflow commands that need the network, and what for.
const NETWORK: &[(&str, &str)] = &[
    ("curl", "curl"),
    ("wget", "wget"),
    ("ssh", "ssh"),
    ("scp", "scp"),
    ("gh", "the GitHub CLI"),
    ("docker", "Docker"),
    ("docker-compose", "Docker"),
    ("aws", "the AWS CLI"),
    ("gcloud", "the gcloud CLI"),
];

/// Most failing tests a red-base flag names.
const MAX_NAMED: usize = 5;

#[derive(Debug, thiserror::Error)]
pub enum SuggestError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("reading the repository's environment: {0}")]
    Env(#[from] EnvError),
    #[error("the drafted aver.toml does not parse ({0}); please report this")]
    Draft(EnvError),
    #[error("importing HEAD: {0}")]
    Import(#[from] crate::git::ImportError),
    #[error(transparent)]
    Keys(#[from] crate::keys::KeyError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("interrupted")]
    Interrupted,
}

/// A check Aver runs here.
#[derive(Debug, Clone, Serialize)]
pub struct Planned {
    pub id: String,
    pub command: String,
    /// The report format Aver reads; `None`: its exit code only.
    pub report: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// One check of the draft.
#[derive(Debug, Clone, Serialize)]
pub struct Suggested {
    pub id: String,
    /// The command as the repository declares it.
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Where it is declared: `preset` for a check detection already makes.
    pub source: String,
    /// The report format Aver reads; `None`: judged by its exit code only.
    pub report: Option<&'static str>,
    #[serde(skip)]
    pub check: CheckDef,
}

/// A declared command left out of the draft, and why.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Dropped {
    pub source: String,
    pub command: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FlagKind {
    /// The check fails on `HEAD`: every change would start from red.
    RedBase,
    /// A program the check runs is not installed here.
    MissingTool,
    /// No per-test report: only the exit code is compared.
    ExitOnly,
    /// A package declares dependencies no committed lockfile pins.
    MissingLockfile,
    /// The dry run could not run the check for another reason.
    CouldNotRun,
}

impl FlagKind {
    pub fn word(self) -> &'static str {
        match self {
            Self::RedBase => "red base",
            Self::MissingTool => "missing tool",
            Self::ExitOnly => "exit code only",
            Self::MissingLockfile => "missing lockfile",
            Self::CouldNotRun => "could not run",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Flag {
    pub kind: FlagKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    pub detail: String,
}

/// One check's dry run on `HEAD`.
#[derive(Debug, Clone, Serialize)]
pub struct DryRun {
    pub check: String,
    /// `pass`, `fail`, `missing-tool` or `could-not-run`.
    pub outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    /// Tests passed and failed, when the report was read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tests: Option<(usize, usize)>,
    pub seconds: u64,
}

/// What `aver init` found.
#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    /// The checks Aver detects here, which `aver ci` runs without
    /// `aver.toml` and the draft keeps.
    pub preset: Vec<Planned>,
    /// The draft's checks.
    pub checks: Vec<Suggested>,
    pub dropped: Vec<Dropped>,
    pub flags: Vec<Flag>,
    /// Empty until [`dry_run`].
    pub dry_run: Vec<DryRun>,
    /// The draft `aver.toml`.
    pub toml: Option<String>,
}

/// A command the repository declares.
struct Candidate {
    source: String,
    /// The command as declared.
    command: String,
    cwd: Option<String>,
    /// The check (under a placeholder id).
    check: CheckDef,
    report: Option<&'static str>,
    /// Dedupe key: what it runs, by [`key`].
    key: String,
    /// For a Makefile target: the keys of its recipe lines, which detection
    /// covers when it covers them all.
    recipe: Vec<String>,
    /// For a check name: `npm-test`, `make-test`.
    stem: String,
}

/// The suggestion for the tree rooted at `root`, without the dry run.
pub fn suggest(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
) -> Result<(Suggestion, Option<Environment>), SuggestError> {
    let resolver = StaticResolver::default();
    let preset_env = env::environment_with_config(src, root, b"", &resolver, Detection::Presets)?;
    let manifest = |dir: &str| read(src, root, &join(dir, "package.json"));
    let root_manifest = manifest("")?;
    let preset_checks: Vec<&CheckDef> = preset_env
        .checks
        .iter()
        .filter(|c| {
            // The default `npm test` stands in for every repository; it runs
            // something only where the root package has a test script.
            c.id.as_str() != env::DEFAULT_CHECK_ID
                || c.cwd.is_some()
                || root_manifest.as_deref().and_then(test_script).is_some()
        })
        .collect();
    let preset: Vec<Planned> = preset_checks
        .iter()
        .map(|c| Planned {
            id: c.id.to_string(),
            command: display(&c.argv),
            report: c.report.as_ref().map(|r| format_name(r.format)),
            cwd: c.cwd.as_ref().map(|p| p.as_str().to_owned()),
        })
        .collect();
    let covered_keys: BTreeMap<(Option<String>, String), String> = preset_checks
        .iter()
        .map(|c| {
            let k = if crate::presets::is_typecheck(c) {
                "typecheck".to_owned()
            } else {
                key(&display(&c.argv))
            };
            (
                (c.cwd.as_ref().map(|p| p.as_str().to_owned()), k),
                c.id.to_string(),
            )
        })
        .collect();

    let mut dropped = Vec::new();
    let mut flags = Vec::new();
    let mut candidates = Vec::new();
    workflows(src, root, &mut candidates, &mut dropped)?;
    makefile(src, root, &mut candidates)?;
    justfile(src, root, &mut candidates)?;
    python(src, root, &mut candidates, &mut flags)?;
    packages(src, root, &mut candidates, &mut flags)?;

    let mut kept: Vec<Candidate> = Vec::new();
    for c in candidates {
        let at = (c.cwd.clone(), c.key.clone());
        let reason = if let Some(id) = covered_keys.get(&at) {
            Some(format!("Aver already runs it (check {id})"))
        } else if !c.recipe.is_empty()
            && c.recipe
                .iter()
                .all(|k| covered_keys.contains_key(&(c.cwd.clone(), k.clone())))
        {
            Some("its recipe is what Aver already runs".to_owned())
        } else {
            kept.iter()
                .find(|k| (k.cwd.clone(), k.key.clone()) == at)
                .map(|k| format!("the same command as {} ({})", k.command, k.source))
        };
        match reason {
            Some(reason) => dropped.push(Dropped {
                source: c.source,
                command: c.command,
                reason,
            }),
            None => kept.push(c),
        }
    }

    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut checks: Vec<Suggested> = Vec::new();
    for c in &preset_checks {
        ids.insert(c.id.to_string());
        checks.push(Suggested {
            id: c.id.to_string(),
            command: display(&c.argv),
            cwd: c.cwd.as_ref().map(|p| p.as_str().to_owned()),
            source: "preset".to_owned(),
            report: c.report.as_ref().map(|r| format_name(r.format)),
            check: CheckDef {
                deps: None,
                ..(*c).clone()
            },
        });
    }
    for c in kept {
        let mut base = c.stem.clone();
        if let Some(dir) = &c.cwd {
            base = format!("{base}-{}", slug(dir));
        }
        base.truncate(56);
        let mut id = base.clone();
        let mut n = 2;
        while ids.contains(&id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        ids.insert(id.clone());
        let mut check = adhoc::fit(&rename(&c.check, &id), &preset_env);
        if check.deps.is_some() && check.deps == env_default_deps(&check, &preset_env) {
            check.deps = None;
        }
        checks.push(Suggested {
            id,
            command: c.command,
            cwd: c.cwd,
            source: c.source,
            report: c.report,
            check,
        });
    }
    for s in &checks {
        if s.report.is_none() {
            flags.push(Flag {
                kind: FlagKind::ExitOnly,
                check: Some(s.id.clone()),
                detail: "no per-test report Aver reads, so base and head are compared by its exit code and per-test causes (NEW, PREEXISTING, FIXED) are unavailable".into(),
            });
        }
    }
    let toml = render(&preset_env, &checks);
    let draft =
        env::environment_with_config(src, root, toml.as_bytes(), &resolver, Detection::Presets)
            .map_err(SuggestError::Draft)?;
    Ok((
        Suggestion {
            preset,
            checks,
            dropped,
            flags,
            dry_run: Vec::new(),
            toml: Some(toml),
        },
        Some(draft),
    ))
}

/// What `aver verify` and `aver check` say when nothing names a check:
/// how to name one, and up to [`MAX_CANDIDATES`] commands the repository
/// declares at its root (CI workflows, Makefile, justfile, Python runners,
/// `package.json`), test commands first; where it declares none, the
/// commands of the toolchains detection finds.
pub fn not_checked(src: &(impl ObjectSource + ?Sized), root: &TreeId) -> String {
    let mut out = String::from(
        "NOT CHECKED: Aver does not know this repository's test command yet.\n\
         Run once: aver check -- <the test command you use>\n",
    );
    let commands = candidate_commands(src, root);
    if !commands.is_empty() {
        out.push_str("Commands this repository declares:\n");
        for command in commands {
            out.push_str(&format!("  aver check -- {command}\n"));
        }
    }
    out
}

/// The commands [`not_checked`] offers, for [`aver_core::Pushback::NoContract`].
pub fn candidate_commands(src: &(impl ObjectSource + ?Sized), root: &TreeId) -> Vec<String> {
    let mut candidates = Vec::new();
    let (mut dropped, mut flags) = (Vec::new(), Vec::new());
    let gathered = workflows(src, root, &mut candidates, &mut dropped)
        .and_then(|()| makefile(src, root, &mut candidates))
        .and_then(|()| justfile(src, root, &mut candidates))
        .and_then(|()| python(src, root, &mut candidates, &mut flags))
        .and_then(|()| packages(src, root, &mut candidates, &mut flags));
    let mut commands: Vec<(bool, String)> = Vec::new();
    if gathered.is_ok() {
        for c in candidates.into_iter().filter(|c| c.cwd.is_none()) {
            let command = one_line(&c.command);
            if !commands.iter().any(|(_, seen)| key(seen) == key(&command)) {
                commands.push((!c.key.contains("test"), command));
            }
        }
    }
    let detected = || {
        env::environment_with_config(
            src,
            root,
            b"",
            &StaticResolver::default(),
            Detection::Presets,
        )
    };
    if let Some(env) = commands.is_empty().then(detected).and_then(Result::ok) {
        let npm_test = read(src, root, "package.json")
            .ok()
            .flatten()
            .is_some_and(|m| test_script(&m).is_some());
        for check in env.checks.iter().filter(|c| c.cwd.is_none()) {
            let command = match env::Family::of(env.check_image(check)) {
                Some(env::Family::Rust) => "cargo test",
                Some(env::Family::Go) => "go test ./...",
                Some(env::Family::Python) => "python -m pytest",
                _ if check.id.as_str() == env::DEFAULT_CHECK_ID && npm_test => "npm test",
                _ => continue,
            };
            if !commands.iter().any(|(_, c)| c == command) {
                commands.push((false, command.to_owned()));
            }
        }
    }
    commands.sort_by_key(|(other, _)| *other);
    commands
        .into_iter()
        .map(|(_, c)| c)
        .take(MAX_CANDIDATES)
        .collect()
}

fn env_default_deps(check: &CheckDef, env: &Environment) -> Option<aver_core::Set<Path>> {
    env::default_deps(
        check,
        &env.image,
        env.lockfile.as_ref(),
        env.layers.as_slice(),
    )
}

/// Imports `HEAD` of the repository at `repo` into a throwaway home,
/// suggests, and, unless `run` is false, dry-runs the draft there natively. Returns the suggestion and `HEAD`'s
/// commit.
pub fn for_repository(repo: &FsPath, run: bool) -> Result<(Suggestion, String), SuggestError> {
    let tmp = tempfile::TempDir::with_prefix("aver-init-")?;
    let home = crate::keys::Home::new(tmp.path());
    let keys = crate::keys::init(&home)?;
    let store = Arc::new(home.open_store()?);
    let source = repo.to_string_lossy().into_owned();
    let resolver = StaticResolver::default();
    let mut options = crate::git::ImportOptions::new(
        &source,
        "HEAD",
        keys.admin.id(),
        &keys.server.signing_key,
        &resolver,
    );
    options.secrets = crate::git::SecretPolicy::Warn;
    options.oversize = crate::git::OversizePolicy::Placeholder;
    options.detection = Detection::Presets;
    let imported = match crate::git::import(&store, &options) {
        Ok(imported) => imported,
        // An `aver.toml` the parser rejects is what `--force` replaces: the
        // tree is read as if it had none.
        Err(crate::git::ImportError::Env(_)) => {
            return Err(SuggestError::Env(EnvError::Invalid(
                "the committed aver.toml does not parse; fix or remove it first".into(),
            )))
        }
        Err(e) => return Err(e.into()),
    };
    let snapshot: aver_core::Snapshot = store.get(&imported.snapshot)?;
    let (mut suggestion, draft) = suggest(&*store, &snapshot.root)?;
    if let (true, Some(draft)) = (run, draft) {
        let mut config = RunnerConfig::for_process(home.root(), aver_core::ProvenanceMode::Local);
        config.source = Some(repo.to_path_buf());
        let runner = SlotRunner::new(store.clone(), config);
        dry_run(snapshot.root, &draft, &runner, &mut suggestion)?;
    }
    Ok((suggestion, imported.commit.to_string()))
}

/// Runs each of `draft`'s checks once on the tree rooted at `root` with
/// `runner` (in its base slot of that tree, cloned from its configured
/// source), recording what each did in `suggestion.dry_run` and flagging a
/// check that fails, one whose program is missing, and one that could not
/// run. An interrupted run stops the dry run.
pub fn dry_run(
    root: TreeId,
    draft: &Environment,
    runner: &SlotRunner,
    suggestion: &mut Suggestion,
) -> Result<(), SuggestError> {
    let place = runner.base_place(None, root);
    for check in draft.checks.iter() {
        let limits = SandboxLimits::of(
            &draft.limits,
            check.timeout_ms.unwrap_or(draft.limits.timeout_ms),
        );
        let ran = match runner.run(SlotRequest {
            root,
            env: draft,
            check,
            limits,
            place: &place,
        }) {
            Ok(out) => Ok(out),
            Err(ProcFailure::Failed { message, .. }) => Err(message),
            Err(ProcFailure::Interrupted) => return Err(SuggestError::Interrupted),
        };
        let id = check.id.to_string();
        let (run, flag) = judge(check, ran);
        suggestion.dry_run.push(DryRun { check: id, ..run });
        suggestion.flags.extend(flag);
    }
    // Most telling first: red bases, missing tools, then the rest.
    suggestion.flags.sort_by_key(|f| match f.kind {
        FlagKind::RedBase => 0,
        FlagKind::MissingTool => 1,
        FlagKind::CouldNotRun => 2,
        FlagKind::MissingLockfile => 3,
        FlagKind::ExitOnly => 4,
    });
    Ok(())
}

fn judge(
    check: &CheckDef,
    ran: Result<crate::exec::output::SandboxOutput, String>,
) -> (DryRun, Option<Flag>) {
    let id = check.id.to_string();
    let mut run = DryRun {
        check: id.clone(),
        outcome: "pass",
        exit: None,
        tests: None,
        seconds: 0,
    };
    let flag = |kind, detail: String| {
        Some(Flag {
            kind,
            check: Some(id.clone()),
            detail,
        })
    };
    let out = match ran {
        Ok(out) => out,
        Err(message) => {
            let missing = message.contains("is not installed on this machine")
                || message.contains("only as the version manager shim");
            run.outcome = if missing {
                "missing-tool"
            } else {
                "could-not-run"
            };
            let kind = if missing {
                FlagKind::MissingTool
            } else {
                FlagKind::CouldNotRun
            };
            return (run, flag(kind, message));
        }
    };
    run.seconds = out.duration.as_secs();
    let output = String::from_utf8_lossy(&out.output).into_owned();
    let code = match out.exit {
        Exit::Exited(code) => code,
        Exit::Oom(_) => {
            run.outcome = "could-not-run";
            return (
                run,
                flag(
                    FlagKind::CouldNotRun,
                    "it ran out of memory; raise [limits] memory_mb".into(),
                ),
            );
        }
        Exit::Timeout(_) => {
            run.outcome = "could-not-run";
            let limit = Duration::from_millis(check.timeout_ms.unwrap_or(0).into());
            return (
                run,
                flag(
                    FlagKind::CouldNotRun,
                    format!(
                        "it did not finish in time{}; raise its timeout_s",
                        if limit.is_zero() {
                            String::new()
                        } else {
                            format!(" ({} s)", limit.as_secs())
                        }
                    ),
                ),
            );
        }
    };
    run.exit = Some(code);
    if code == 127 {
        run.outcome = "missing-tool";
        let line = output
            .lines()
            .rev()
            .find(|l| l.contains("not found") || l.contains("does not exist"))
            .unwrap_or("exit 127: a command was not found")
            .trim()
            .chars()
            .take(200)
            .collect();
        return (run, flag(FlagKind::MissingTool, line));
    }
    let mut failing = Vec::new();
    if let Some(report) = &check.report {
        let rel = report.path.as_str().trim_start_matches("/aver/out/");
        if let Some(Ok((tests, _))) = out
            .files
            .get(rel)
            .map(|bytes| crate::exec::junit::parse_as(&check.id, bytes, report.format))
        {
            let passed = tests
                .iter()
                .filter(|t| t.status == TestStatus::Pass)
                .count();
            failing = tests
                .iter()
                .filter(|t| t.status == TestStatus::Fail)
                .map(|t| t.id.to_string())
                .collect();
            run.tests = Some((passed, failing.len()));
        }
    }
    if code == 0 && failing.is_empty() {
        return (run, None);
    }
    run.outcome = "fail";
    let what = if failing.is_empty() {
        let last = output
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("[aver]"))
            .map(|l| format!(": {}", l.chars().take(200).collect::<String>()))
            .unwrap_or_default();
        format!("exits {code} on HEAD{last}")
    } else {
        let more = failing.len().saturating_sub(MAX_NAMED);
        let mut named = failing
            .iter()
            .take(MAX_NAMED)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if more > 0 {
            named.push_str(&format!(" and {more} more"));
        }
        format!("{} test(s) fail on HEAD: {named}", failing.len())
    };
    (
        run,
        flag(
            FlagKind::RedBase,
            format!("{what}; verify judges each change against this failing base, fix it or leave the check out"),
        ),
    )
}

/// The `aver.toml` of `checks`, with the dependency layers and memory limit
/// detection gave `preset`.
fn render(preset: &Environment, checks: &[Suggested]) -> String {
    let mut out = String::from(
        "# Drafted by `aver init` from what this repository declares.\n\
         # Listing checks turns off Aver's own detection: the ones it found are kept below.\n",
    );
    if preset.limits.memory_mb != env::DEFAULT_LIMITS.memory_mb {
        out.push_str(&format!(
            "\n[limits]\nmemory_mb = {}\n",
            preset.limits.memory_mb
        ));
    }
    for layer in preset.layers.iter() {
        let name = layer.lockfile.path.segments().last().unwrap_or_default();
        out.push_str(&format!(
            "\n[[deps]]\nlockfile = {}\n",
            quote(layer.lockfile.path.as_str())
        ));
        if env::kind_of_lockfile(name) != Some(layer.kind) {
            out.push_str(&format!("kind = {}\n", quote(layer_kind(layer.kind))));
        }
        if layer.install.argv != env::default_install(layer.kind, name) {
            out.push_str(&format!("install = {}\n", array(&layer.install.argv)));
        }
    }
    let default_image = env::default_image();
    for s in checks {
        let c = &s.check;
        out.push('\n');
        if s.source == "preset" {
            out.push_str("# Found by Aver's detection.\n");
        } else {
            out.push_str(&format!("# From {}: {}\n", s.source, one_line(&s.command)));
        }
        if c.report.is_none() {
            out.push_str(
                "# Exit code only: no per-test report, so per-test causes are unavailable.\n",
            );
        }
        out.push_str(&format!("[[check]]\nid = {}\n", quote(c.id.as_str())));
        out.push_str(&format!("run = {}\n", array(&c.argv)));
        if let Some(cwd) = &c.cwd {
            out.push_str(&format!("cwd = {}\n", quote(cwd.as_str())));
        }
        if let Some(report) = &c.report {
            out.push_str(&format!(
                "report = {{ format = {}, path = {} }}\n",
                quote(format_name(report.format)),
                quote(report.path.as_str())
            ));
        }
        if !c.setup.is_empty() {
            let steps: Vec<String> = c.setup.iter().map(|s| array(s)).collect();
            out.push_str(&format!("setup = [{}]\n", steps.join(", ")));
        }
        let image = c.image.as_ref().unwrap_or(&preset.image);
        if *image != default_image {
            out.push_str(&format!(
                "image = {}\n",
                quote(&format!("{}@{}", image.reference, image.digest))
            ));
        }
        if let Some(deps) = &c.deps {
            let paths: Vec<String> = deps.iter().map(|p| p.as_str().to_owned()).collect();
            out.push_str(&format!("deps = {}\n", array(&paths)));
        }
        if !c.covers.is_empty() {
            let globs: Vec<String> = c.covers.iter().map(|g| g.as_str().to_owned()).collect();
            out.push_str(&format!("covers = {}\n", array(&globs)));
        }
        if let Some(ms) = c.timeout_ms {
            out.push_str(&format!("timeout_s = {}\n", ms.div_ceil(1000)));
        }
    }
    out
}

/// What `aver init` prints: what Aver runs, what was left out and why, the
/// draft, its dry run on `HEAD` (`commit`), the flags, and what was written
/// to `config`.
pub fn text(s: &Suggestion, commit: &str, config: &FsPath, exists: bool, written: bool) -> String {
    let mut out = String::from("\n");
    let short = &commit[..commit.len().min(12)];
    let list = |planned: &[Planned]| {
        planned
            .iter()
            .map(|p| {
                // What it runs, not the reporter options Aver adds.
                let mut what = key(&p.command);
                if let Some(cwd) = &p.cwd {
                    what.push_str(&format!(" in {cwd}"));
                }
                match p.report {
                    Some(format) => format!("{} ({what}, {format} report)", p.id),
                    None => format!("{} ({what}, exit code only)", p.id),
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    if s.preset.is_empty() {
        out.push_str("Aver's detection finds nothing to run here.\n");
    } else {
        out.push_str(&format!("Aver's detection finds: {}.\n", list(&s.preset)));
    }
    if !s.dropped.is_empty() {
        out.push_str("\nLeft out:\n");
        for d in &s.dropped {
            out.push_str(&format!(
                "  - {}: `{}`: {}\n",
                d.source,
                one_line(&d.command),
                d.reason
            ));
        }
    }
    if let Some(toml) = &s.toml {
        out.push_str("\nDraft aver.toml:\n\n");
        out.push_str(toml);
        if !s.dry_run.is_empty() {
            out.push_str(&format!("\nDry run on HEAD ({short}):\n"));
            for r in &s.dry_run {
                let exit = r.exit.map(|c| format!(", exit {c}")).unwrap_or_default();
                let tests = r
                    .tests
                    .map(|(p, f)| format!(", {p} passed, {f} failed"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "  {:<24} {}{exit}{tests} ({} s)\n",
                    r.check, r.outcome, r.seconds
                ));
            }
        }
    }
    if !s.flags.is_empty() {
        out.push_str("\nFlags:\n");
        for f in &s.flags {
            match &f.check {
                Some(id) => out.push_str(&format!("  ! {}: {id}: {}\n", f.kind.word(), f.detail)),
                None => out.push_str(&format!("  ! {}: {}\n", f.kind.word(), f.detail)),
            }
        }
    }
    if s.toml.is_some() {
        out.push('\n');
        if written {
            out.push_str(&format!("Wrote {}.\n", config.display()));
        } else if exists {
            out.push_str(&format!(
                "Nothing written: {} exists; `aver init --yes --force` replaces it.\n",
                config.display()
            ));
        } else {
            out.push_str("Nothing written; `aver init --yes` writes this as aver.toml.\n");
        }
    }
    out
}

fn layer_kind(kind: aver_core::LayerKind) -> &'static str {
    use aver_core::LayerKind::*;
    match kind {
        Npm => "npm",
        Uv => "uv",
        Pip => "pip",
        Cargo => "cargo",
        Go => "go",
    }
}

/// The `aver.toml` name of a report format.
pub fn format_name(format: ReportFormat) -> &'static str {
    match format {
        ReportFormat::Junit => "junit",
        ReportFormat::JestJson => "jest-json",
        ReportFormat::MochaXunit => "mocha-xunit",
        ReportFormat::Libtest => "libtest",
        ReportFormat::GoJson => "go-json",
        ReportFormat::PytestJunit => "pytest-junit",
        ReportFormat::Tsc => "tsc",
        ReportFormat::Tap => "tap",
        ReportFormat::CargoProbe => "cargo-probe",
    }
}

/// A TOML basic string: JSON's escapes are TOML's.
fn quote(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}

fn array(items: &[String]) -> String {
    let items: Vec<String> = items.iter().map(|s| quote(s)).collect();
    format!("[{}]", items.join(", "))
}

fn one_line(s: &str) -> String {
    let line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    line.chars().take(160).collect()
}

/// How a check's command reads: the script of `sh -c`, else the words.
pub fn display(argv: &[String]) -> String {
    match argv {
        [sh, c, script] if sh == "sh" && c == "-c" => script.clone(),
        _ => argv
            .iter()
            .map(|a| {
                if a.is_empty()
                    || a.contains(|c: char| c.is_whitespace() || "'\"$&|;<>*`".contains(c))
                {
                    crate::ci::shell_quote(a)
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// What a command runs, for telling two commands apart: the test runner or
/// tool it invokes, else its words.
fn key(command: &str) -> String {
    let words: Vec<String> = command
        .split(|c: char| c.is_whitespace() || c == ';' || c == '&' || c == '|')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let w = w.trim_matches(|c| c == '\'' || c == '"');
            w.rsplit('/').next().unwrap_or(w).to_owned()
        })
        .collect();
    let at = |i: usize| words.get(i).map(String::as_str);
    for i in 0..words.len() {
        match (at(i), at(i + 1), at(i + 2)) {
            (Some("cargo"), Some(sub @ ("test" | "clippy" | "check")), _) => {
                return format!("cargo {sub}")
            }
            (Some("go"), Some(sub @ ("test" | "vet")), _) => return format!("go {sub}"),
            (Some("pytest" | "py.test"), _, _) => return "pytest".into(),
            (Some("-m"), Some("pytest" | "unittest"), _) => return "pytest".into(),
            (Some("npm" | "yarn" | "pnpm"), Some("test" | "t"), _)
            | (Some("npm" | "yarn" | "pnpm"), Some("run"), Some("test")) => {
                return "npm test".into()
            }
            (Some("tsc"), _, _) => return "typecheck".into(),
            (Some("npm" | "yarn" | "pnpm"), Some("run"), Some(s)) if typecheck_script(s) => {
                return "typecheck".into()
            }
            _ => {}
        }
    }
    words.join(" ")
}

fn typecheck_script(name: &str) -> bool {
    matches!(
        name,
        "typecheck" | "type-check" | "check-types" | "types" | "tsc"
    )
}

/// Whether a workflow command looks like a test, lint or type check, and
/// the stem of its check id.
fn classify(command: &str) -> Option<String> {
    let words: Vec<String> = command
        .split_whitespace()
        .filter(|w| !w.contains('='))
        .map(|w| {
            let w = w.trim_matches(|c| c == '\'' || c == '"');
            w.rsplit('/').next().unwrap_or(w).to_ascii_lowercase()
        })
        .collect();
    let first = words.first()?.as_str();
    if matches!(
        first,
        "echo" | "cd" | "export" | "mkdir" | "cp" | "mv" | "rm" | "cat" | "ls" | "git" | "sudo"
    ) || words.iter().skip(1).take(2).any(|w| {
        matches!(
            w.as_str(),
            "install" | "ci" | "i" | "add" | "sync" | "build"
        )
    }) {
        return None;
    }
    let looks = |w: &str| {
        w.contains("test")
            || w.contains("lint")
            || matches!(
                w,
                "pytest"
                    | "tox"
                    | "nox"
                    | "jest"
                    | "vitest"
                    | "mocha"
                    | "ctest"
                    | "clippy"
                    | "eslint"
                    | "ruff"
                    | "flake8"
                    | "pylint"
                    | "shellcheck"
                    | "tsc"
                    | "mypy"
                    | "pyright"
                    | "vet"
                    | "typecheck"
                    | "type-check"
                    | "check-types"
                    | "check"
            )
    };
    if !words.iter().any(|w| looks(w)) {
        return None;
    }
    let stem: Vec<&str> = words
        .iter()
        .take(if words.len() > 1 && words[1] == "run" {
            3
        } else {
            2
        })
        .map(String::as_str)
        .filter(|w| !w.starts_with('-'))
        .collect();
    Some(slug(&stem.join("-")))
}

/// Lowercase letters, digits, `.`, `_` and `-`, as a check id holds them.
fn slug(s: &str) -> String {
    let mut out: String = s
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches(|c| c == '-' || c == '.').to_owned();
    if out.is_empty() || !out.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        format!("check-{out}")
    } else {
        out
    }
}

/// `check` under id `to`, its report file renamed with it.
fn rename(check: &CheckDef, to: &str) -> CheckDef {
    let old = format!("/aver/out/{}.", check.id);
    let new = format!("/aver/out/{to}.");
    let mut out = check.clone();
    out.id = to.parse().expect("a slug is a valid check id");
    for arg in &mut out.argv {
        *arg = arg.replace(&old, &new);
    }
    if let Some(report) = &mut out.report {
        report.path = report
            .path
            .as_str()
            .replace(&old, &new)
            .parse()
            .expect("a renamed report path stays valid");
    }
    out
}

/// A command as a check: its runner's report where Aver knows one.
fn candidate(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
    source: String,
    command: &str,
    cwd: Option<String>,
    stem: String,
) -> Result<Option<Candidate>, StoreError> {
    let shell = command.contains(|c: char| "&|;<>$`'\"*()\\".contains(c));
    let words: Vec<String> = command.split_whitespace().map(str::to_owned).collect();
    let dir = cwd.clone().unwrap_or_default();
    let cwd_path = match cwd.as_deref().map(str::parse::<Path>) {
        Some(Ok(p)) => Some(p),
        Some(Err(_)) => return Ok(None),
        None => None,
    };
    let (check, report) = if shell || words.is_empty() {
        let mut check = CheckDef::new(
            adhoc::ID.parse().expect("a valid id"),
            vec!["sh".into(), "-c".into(), command.to_owned()],
        );
        check.cwd = cwd_path;
        (check, None)
    } else {
        let manifest = read(src, root, &join(&dir, "package.json"))?;
        let lockfile = read(src, root, &join(&dir, env::DEFAULT_LOCKFILE))?;
        match adhoc::check_for(&words, cwd_path, manifest.as_deref(), lockfile.as_deref()) {
            Ok(a) => {
                let report = a.check.report.as_ref().map(|r| format_name(r.format));
                (a.check, report)
            }
            Err(_) => return Ok(None),
        }
    };
    Ok(Some(Candidate {
        source,
        command: command.to_owned(),
        key: key(command),
        cwd,
        check,
        report,
        recipe: Vec::new(),
        stem,
    }))
}

/// One `run:` step of a workflow job.
#[derive(Debug, Default, PartialEq, Eq)]
struct Step {
    job: String,
    name: Option<String>,
    run: String,
    /// Every line of the step, for the secrets and expressions it uses.
    text: String,
    cwd: Option<String>,
    services: Vec<String>,
    /// Variables the job or workflow sets from a secret.
    secret_vars: Vec<String>,
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn meaningful(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && !t.starts_with('#')
}

/// A YAML scalar as written on one line, without quotes or a comment.
fn scalar(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(inner) = raw
        .strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .or_else(|| raw.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')))
    {
        return inner.to_owned();
    }
    match raw.find(" #") {
        Some(at) => raw[..at].trim().to_owned(),
        None => raw.to_owned(),
    }
}

/// `key: value` of a mapping line (a leading `- ` removed).
fn entry(line: &str) -> Option<(String, String)> {
    let t = line.trim_start().trim_start_matches("- ").trim_start();
    let (k, v) = t.split_once(':')?;
    if k.is_empty() || k.contains(' ') && !k.starts_with('"') {
        return None;
    }
    Some((k.trim_matches('"').to_owned(), v.trim().to_owned()))
}

/// The block under the line at `at`: the following lines indented deeper.
fn block<'a>(lines: &[&'a str], at: usize) -> Vec<&'a str> {
    let base = indent(lines[at]);
    lines[at + 1..]
        .iter()
        .take_while(|l| !meaningful(l) || indent(l) > base)
        .copied()
        .collect()
}

/// A value at `lines[at]` (`key: value`), with a `|` or `>` block scalar
/// read from the lines below.
fn value(lines: &[&str], at: usize, inline: &str) -> String {
    let style = inline.split_whitespace().next().unwrap_or("");
    if style.starts_with('|') || style.starts_with('>') {
        let body = block(lines, at);
        let deepest = body
            .iter()
            .filter(|l| meaningful(l))
            .map(|l| indent(l))
            .min()
            .unwrap_or(0);
        let text: Vec<&str> = body
            .iter()
            .map(|l| {
                if l.len() >= deepest {
                    &l[deepest..]
                } else {
                    l.trim()
                }
            })
            .collect();
        if style.starts_with('>') {
            text.join(" ")
        } else {
            text.join("\n")
        }
    } else {
        scalar(inline)
    }
}

/// The variables a mapping block sets from `${{ secrets.* }}`.
fn secret_vars(body: &[&str]) -> Vec<String> {
    body.iter()
        .filter(|l| l.contains("secrets."))
        .filter_map(|l| entry(l).map(|(k, _)| k))
        .collect()
}

/// The `run:` steps of a GitHub Actions workflow. A line-based reading of
/// the YAML GitHub documents for jobs and steps, which is all `aver init`
/// needs; anything it cannot place is skipped.
fn steps(text: &str) -> Vec<Step> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut workflow_secrets = Vec::new();
    let mut jobs_at = None;
    for (i, line) in lines.iter().enumerate() {
        if indent(line) != 0 || !meaningful(line) {
            continue;
        }
        match entry(line).map(|(k, _)| k).as_deref() {
            Some("env") => workflow_secrets = secret_vars(&block(&lines, i)),
            Some("jobs") => jobs_at = Some(i),
            _ => {}
        }
    }
    let Some(jobs_at) = jobs_at else {
        return out;
    };
    let jobs = block(&lines, jobs_at);
    let job_indent = match jobs.iter().find(|l| meaningful(l)) {
        Some(l) => indent(l),
        None => return out,
    };
    let starts: Vec<usize> = (0..jobs.len())
        .filter(|&i| meaningful(jobs[i]) && indent(jobs[i]) == job_indent)
        .collect();
    for &j in &starts {
        let Some((job, _)) = entry(jobs[j]) else {
            continue;
        };
        let body = block(&jobs, j);
        let key_indent = match body.iter().find(|l| meaningful(l)) {
            Some(l) => indent(l),
            None => continue,
        };
        let mut services = Vec::new();
        let mut secrets = workflow_secrets.clone();
        let mut default_cwd = None;
        let mut steps_at = None;
        for (i, line) in body.iter().enumerate() {
            if !meaningful(line) || indent(line) != key_indent {
                continue;
            }
            match entry(line).map(|(k, _)| k).as_deref() {
                Some("services") => {
                    let inner = block(&body, i);
                    let depth = inner
                        .iter()
                        .find(|l| meaningful(l))
                        .map(|l| indent(l))
                        .unwrap_or(0);
                    services = inner
                        .iter()
                        .filter(|l| meaningful(l) && indent(l) == depth)
                        .filter_map(|l| entry(l).map(|(k, _)| k))
                        .collect();
                }
                Some("env") => secrets.extend(secret_vars(&block(&body, i))),
                Some("defaults") => {
                    default_cwd = block(&body, i).iter().find_map(|l| {
                        entry(l)
                            .filter(|(k, _)| k == "working-directory")
                            .map(|(_, v)| scalar(&v))
                    })
                }
                Some("steps") => steps_at = Some(i),
                _ => {}
            }
        }
        let Some(steps_at) = steps_at else {
            continue;
        };
        let list = block(&body, steps_at);
        let dash = match list.iter().find(|l| l.trim_start().starts_with("- ")) {
            Some(l) => indent(l),
            None => continue,
        };
        let items: Vec<usize> = (0..list.len())
            .filter(|&i| indent(list[i]) == dash && list[i].trim_start().starts_with("- "))
            .collect();
        for (n, &s) in items.iter().enumerate() {
            let end = items.get(n + 1).copied().unwrap_or(list.len());
            let lines_of: Vec<&str> = list[s..end].to_vec();
            let field_indent = dash + 2;
            let mut step = Step {
                job: job.clone(),
                text: lines_of.join("\n"),
                cwd: default_cwd.clone(),
                services: services.clone(),
                secret_vars: secrets.clone(),
                ..Step::default()
            };
            let mut has_run = false;
            for (i, line) in lines_of.iter().enumerate() {
                let first = i == 0;
                if !meaningful(line) || (!first && indent(line) != field_indent) {
                    continue;
                }
                let Some((k, v)) = entry(line) else {
                    continue;
                };
                match k.as_str() {
                    "run" => {
                        step.run = value(&lines_of, i, &v);
                        has_run = true;
                    }
                    "name" => step.name = Some(scalar(&v)),
                    "working-directory" => step.cwd = Some(scalar(&v)),
                    "env" => step.secret_vars.extend(secret_vars(&block(&lines_of, i))),
                    _ => {}
                }
            }
            if has_run {
                out.push(step);
            }
        }
    }
    out
}

/// The commands of a `run:` script, one per line, continuations joined.
fn commands(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in script.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        if let Some(head) = t.strip_suffix('\\') {
            current.push_str(head.trim_end());
            current.push(' ');
            continue;
        }
        current.push_str(t);
        let done = std::mem::take(&mut current);
        if !done.trim().is_empty() {
            out.push(done.trim().to_owned());
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_owned());
    }
    out
}

/// Why a workflow command cannot run under Aver, if it cannot.
fn unrunnable(step: &Step, command: &str) -> Option<String> {
    if let Some(at) = step
        .text
        .find("${{ secrets.")
        .or(step.text.find("${{secrets."))
    {
        let used: String = step.text[at..].split("}}").next().unwrap_or("").to_owned();
        return Some(format!("uses a secret ({used}}}}}), which Aver never has"));
    }
    if let Some(var) = step
        .secret_vars
        .iter()
        .find(|v| command.contains(&format!("${v}")) || command.contains(&format!("${{{v}}}")))
    {
        return Some(format!(
            "reads ${var}, which the workflow sets from a secret Aver never has"
        ));
    }
    if !step.services.is_empty() {
        return Some(format!(
            "its job starts service containers ({}), which Aver does not run",
            step.services.join(", ")
        ));
    }
    if let Some(at) = command.find("${{") {
        let expr: String = command[at..].split("}}").next().unwrap_or("").to_owned();
        let what = if expr.contains("matrix.") {
            "a matrix value"
        } else {
            "a GitHub expression"
        };
        return Some(format!(
            "uses {what} ({expr}}}}}), which Aver cannot evaluate"
        ));
    }
    let words: Vec<&str> = command
        .split(|c: char| c.is_whitespace() || c == ';' || c == '&' || c == '|' || c == '(')
        .collect();
    if let Some((_, what)) = NETWORK
        .iter()
        .find(|(program, _)| words.iter().any(|w| w.rsplit('/').next() == Some(program)))
    {
        return Some(format!(
            "needs {what}; Aver runs checks without the network"
        ));
    }
    None
}

fn workflows(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
    candidates: &mut Vec<Candidate>,
    dropped: &mut Vec<Dropped>,
) -> Result<(), StoreError> {
    for (name, _) in list(src, root, ".github/workflows")? {
        if !(name.ends_with(".yml") || name.ends_with(".yaml")) {
            continue;
        }
        let path = format!(".github/workflows/{name}");
        let Some(bytes) = read(src, root, &path)? else {
            continue;
        };
        for step in steps(&String::from_utf8_lossy(&bytes)) {
            let source = match &step.name {
                Some(n) => format!("{path} (job {}, step {n:?})", step.job),
                None => format!("{path} (job {})", step.job),
            };
            for command in commands(&step.run) {
                let Some(stem) = classify(&command) else {
                    continue;
                };
                if let Some(reason) = unrunnable(&step, &command) {
                    dropped.push(Dropped {
                        source: source.clone(),
                        command,
                        reason,
                    });
                    continue;
                }
                let (cwd, command) = in_dir(step.cwd.as_deref(), &command);
                if let Some(c) = candidate(src, root, source.clone(), &command, cwd, stem)? {
                    candidates.push(c);
                }
            }
        }
    }
    Ok(())
}

/// A leading `cd <dir> &&` as the command's directory, joined to the
/// step's own, normalized to a repository path (`None`: the root).
fn in_dir(step_cwd: Option<&str>, command: &str) -> (Option<String>, String) {
    let mut dir: Vec<String> = Vec::new();
    let mut push = |d: &str| {
        for part in d.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    dir.pop();
                }
                p => dir.push(p.to_owned()),
            }
        }
    };
    if let Some(d) = step_cwd {
        push(d.trim_start_matches("${{ github.workspace }}"));
    }
    let mut rest = command.to_owned();
    if let Some(after) = command.strip_prefix("cd ") {
        if let Some((d, tail)) = after.split_once("&&") {
            if !d.trim().contains(char::is_whitespace) {
                push(d.trim());
                rest = tail.trim().to_owned();
            }
        }
    }
    let cwd = (!dir.is_empty()).then(|| dir.join("/"));
    (cwd, rest)
}

fn makefile(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
    candidates: &mut Vec<Candidate>,
) -> Result<(), StoreError> {
    let mut text = None;
    for name in ["GNUmakefile", "makefile", "Makefile"] {
        if let Some(bytes) = read(src, root, name)? {
            text = Some((name, String::from_utf8_lossy(&bytes).into_owned()));
            break;
        }
    }
    let Some((name, text)) = text else {
        return Ok(());
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut seen = BTreeSet::new();
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with(['\t', ' ', '#', '.']) {
            continue;
        }
        let Some((targets, rest)) = line.split_once(':') else {
            continue;
        };
        if rest.starts_with('=') || targets.contains('=') || targets.contains('$') {
            continue;
        }
        for target in targets.split_whitespace() {
            if !matches!(target, "test" | "check" | "lint") || !seen.insert(target) {
                continue;
            }
            let recipe: Vec<String> = lines[i + 1..]
                .iter()
                .take_while(|l| l.starts_with('\t') || l.trim().is_empty())
                .map(|l| l.trim().trim_start_matches(['@', '-', '+']).to_owned())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect();
            let command = format!("make {target}");
            if let Some(mut c) = candidate(
                src,
                root,
                name.to_owned(),
                &command,
                None,
                format!("make-{target}"),
            )? {
                c.recipe = recipe.iter().map(|r| key(r)).collect();
                candidates.push(c);
            }
        }
    }
    Ok(())
}

fn justfile(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
    candidates: &mut Vec<Candidate>,
) -> Result<(), StoreError> {
    let mut found = None;
    for name in ["justfile", "Justfile", ".justfile"] {
        if let Some(bytes) = read(src, root, name)? {
            found = Some((name, String::from_utf8_lossy(&bytes).into_owned()));
            break;
        }
    }
    let Some((name, text)) = found else {
        return Ok(());
    };
    let mut seen = BTreeSet::new();
    for line in text.lines() {
        if line.starts_with([' ', '\t', '#']) {
            continue;
        }
        let Some((head, rest)) = line.split_once(':') else {
            continue;
        };
        if rest.starts_with('=') {
            continue;
        }
        let recipe = head.trim_start_matches('@').split_whitespace().next();
        if let Some(r @ ("test" | "check" | "lint")) = recipe {
            if seen.insert(r) {
                let command = format!("just {r}");
                if let Some(c) = candidate(
                    src,
                    root,
                    name.to_owned(),
                    &command,
                    None,
                    format!("just-{r}"),
                )? {
                    candidates.push(c);
                }
            }
        }
    }
    Ok(())
}

fn python(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
    candidates: &mut Vec<Candidate>,
    flags: &mut Vec<Flag>,
) -> Result<(), StoreError> {
    if read(src, root, "tox.ini")?.is_some() {
        candidates.extend(candidate(
            src,
            root,
            "tox.ini".into(),
            "tox",
            None,
            "tox".into(),
        )?);
    }
    if read(src, root, "noxfile.py")?.is_some() {
        candidates.extend(candidate(
            src,
            root,
            "noxfile.py".into(),
            "nox",
            None,
            "nox".into(),
        )?);
    }
    for (dir, files) in walk(src, root)? {
        if !files.contains("pyproject.toml") {
            continue;
        }
        let path = join(&dir, "pyproject.toml");
        let Some(bytes) = read(src, root, &path)? else {
            continue;
        };
        let Ok(doc) = String::from_utf8_lossy(&bytes).parse::<toml::Table>() else {
            continue;
        };
        let tool = doc.get("tool").and_then(|t| t.as_table());
        if tool.is_some_and(|t| t.contains_key("pytest")) {
            let cwd = (!dir.is_empty()).then(|| dir.clone());
            candidates.extend(candidate(
                src,
                root,
                path.clone(),
                "pytest",
                cwd,
                "pytest".into(),
            )?);
        }
        let declares = doc
            .get("project")
            .and_then(|p| p.get("dependencies"))
            .and_then(|d| d.as_array())
            .is_some_and(|d| !d.is_empty());
        let pinned = files.contains("uv.lock")
            || files
                .iter()
                .any(|f| f.contains("requirements") && f.ends_with(".txt"));
        if declares && !pinned {
            flags.push(Flag {
                kind: FlagKind::MissingLockfile,
                check: None,
                detail: format!(
                    "{path} declares dependencies, but no uv.lock or requirements file pins them: Aver installs nothing for it, so its tests run only where those packages already are. Commit a uv.lock (`uv lock`)."
                ),
            });
        }
    }
    Ok(())
}

fn packages(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
    candidates: &mut Vec<Candidate>,
    flags: &mut Vec<Flag>,
) -> Result<(), StoreError> {
    let dirs = walk(src, root)?;
    let locked: Vec<&String> = dirs
        .iter()
        .filter(|(_, f)| f.contains("package-lock.json") || f.contains("npm-shrinkwrap.json"))
        .map(|(d, _)| d)
        .collect();
    for (dir, files) in &dirs {
        if !files.contains("package.json") {
            continue;
        }
        let path = join(dir, "package.json");
        let Some(bytes) = read(src, root, &path)? else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let cwd = (!dir.is_empty()).then(|| dir.clone());
        if test_script(&bytes).is_some() {
            candidates.extend(candidate(
                src,
                root,
                path.clone(),
                "npm test",
                cwd.clone(),
                "npm-test".into(),
            )?);
        }
        if let Some(scripts) = manifest["scripts"].as_object() {
            for name in scripts.keys() {
                if name == "lint" || typecheck_script(name) {
                    let command = format!("npm run {name}");
                    candidates.extend(candidate(
                        src,
                        root,
                        path.clone(),
                        &command,
                        cwd.clone(),
                        slug(&format!("npm-{name}")),
                    )?);
                }
            }
        }
        let declares = ["dependencies", "devDependencies"]
            .iter()
            .any(|k| manifest[k].as_object().is_some_and(|d| !d.is_empty()));
        // A workspace member installs from the lockfile of a package above it.
        let pinned = locked.iter().any(|d| env::within(dir, d));
        if declares && !pinned {
            let other = ["yarn.lock", "pnpm-lock.yaml", "bun.lockb", "bun.lock"]
                .into_iter()
                .find(|f| files.contains(*f))
                .map(|f| {
                    format!(" ({f} is committed, but Aver installs only from package-lock.json)")
                })
                .unwrap_or_default();
            flags.push(Flag {
                kind: FlagKind::MissingLockfile,
                check: None,
                detail: format!(
                    "{path} lists dependencies, but no package-lock.json is committed{other}: Aver installs nothing for it and its runner's reporter cannot be added. Commit one (`npm install --package-lock-only`)."
                ),
            });
        }
    }
    Ok(())
}

/// A package's `test` script, unless it is npm's placeholder.
fn test_script(manifest: &[u8]) -> Option<String> {
    let manifest: serde_json::Value = serde_json::from_slice(manifest).ok()?;
    let script = manifest["scripts"]["test"].as_str()?;
    (!script.contains("no test specified") && !script.trim().is_empty()).then(|| script.to_owned())
}

/// The directories detection searches (the root, and at most
/// [`crate::presets::MAX_DEPTH`] down, skipping hidden and
/// [`crate::presets::SKIP`] ones), with their file names.
fn walk(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
) -> Result<Vec<(String, BTreeSet<String>)>, StoreError> {
    let mut out = Vec::new();
    let mut queue = vec![(String::new(), *root, 0usize)];
    while let Some((path, tree, depth)) = queue.pop() {
        let tree: aver_core::Tree = src.tree(&tree)?;
        let mut files = BTreeSet::new();
        for e in tree.entries.iter() {
            let name = e.name.as_str();
            match e.node {
                Node::Tree(id) => {
                    if depth < crate::presets::MAX_DEPTH
                        && !name.starts_with('.')
                        && !crate::presets::SKIP.contains(&name)
                    {
                        queue.push((join(&path, name), id, depth + 1));
                    }
                }
                _ => {
                    files.insert(name.to_owned());
                }
            }
        }
        out.push((path, files));
    }
    out.sort();
    Ok(out)
}

/// The files of directory `dir`.
fn list(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
    dir: &str,
) -> Result<Vec<(String, Node)>, StoreError> {
    let Ok(path) = dir.parse::<Path>() else {
        return Ok(Vec::new());
    };
    match env::lookup(src, root, &path)? {
        Some(Node::Tree(id)) => {
            let tree: aver_core::Tree = src.tree(&id)?;
            Ok(tree
                .entries
                .iter()
                .filter(|e| !matches!(e.node, Node::Tree(_)))
                .map(|e| (e.name.as_str().to_owned(), e.node))
                .collect())
        }
        _ => Ok(Vec::new()),
    }
}

fn read(
    src: &(impl ObjectSource + ?Sized),
    root: &TreeId,
    path: &str,
) -> Result<Option<Vec<u8>>, StoreError> {
    match path.parse::<Path>() {
        Ok(p) => env::read_file(src, root, &p),
        Err(_) => Ok(None),
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKFLOW: &str = r#"name: CI
on: [push]
env:
  DEPLOY_TOKEN: ${{ secrets.DEPLOY }}
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install
        run: npm ci
      - name: Unit tests
        run: |
          npm test
          ./scripts/lint.sh
        working-directory: web
  integration:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:16
    steps:
      - run: make test
  publish:
    steps:
      - name: Upload
        run: curl -T out.tgz https://example.com
      - name: Signed tests
        env:
          TOKEN: ${{ secrets.NPM }}
        run: npm test
      - run: npm test --node=${{ matrix.node }}
"#;

    #[test]
    fn steps_are_read_with_their_jobs_services_and_secrets_crit_init_secrets() {
        let steps = steps(WORKFLOW);
        let runs: Vec<(&str, &str)> = steps
            .iter()
            .map(|s| (s.job.as_str(), s.run.as_str()))
            .collect();
        assert_eq!(
            runs,
            [
                ("test", "npm ci"),
                ("test", "npm test\n./scripts/lint.sh"),
                ("integration", "make test"),
                ("publish", "curl -T out.tgz https://example.com"),
                ("publish", "npm test"),
                ("publish", "npm test --node=${{ matrix.node }}"),
            ]
        );
        assert_eq!(steps[1].cwd.as_deref(), Some("web"));
        assert_eq!(steps[2].services, ["postgres"]);
        assert_eq!(steps[1].secret_vars, ["DEPLOY_TOKEN"]);

        let why = |i: usize| unrunnable(&steps[i], &commands(&steps[i].run)[0]);
        assert_eq!(why(1), None);
        assert!(why(2).unwrap().contains("service containers (postgres)"));
        assert!(why(3).unwrap().contains("needs curl"));
        assert!(why(4).unwrap().contains("secret (${{ secrets.NPM }})"));
        assert!(why(5)
            .unwrap()
            .contains("a matrix value (${{ matrix.node }})"));
        let reads = Step {
            secret_vars: vec!["DEPLOY_TOKEN".into()],
            ..Step::default()
        };
        assert!(unrunnable(&reads, "./test.sh $DEPLOY_TOKEN")
            .unwrap()
            .contains("reads $DEPLOY_TOKEN"));
    }

    #[test]
    fn commands_are_classified_and_keyed_crit_init_ci_draft() {
        assert_eq!(classify("npm test").as_deref(), Some("npm-test"));
        assert_eq!(classify("npm run lint").as_deref(), Some("npm-run-lint"));
        assert_eq!(
            classify("cargo clippy --workspace -- -D warnings").as_deref(),
            Some("cargo-clippy")
        );
        assert_eq!(
            classify("./scripts/run-tests.sh").as_deref(),
            Some("run-tests.sh")
        );
        assert_eq!(classify("npm ci"), None);
        assert_eq!(classify("pip install -e ."), None);
        assert_eq!(classify("echo testing"), None);
        assert_eq!(classify("cargo build --release"), None);

        assert_eq!(key("npm run test"), "npm test");
        assert_eq!(
            key("cargo test --workspace --no-fail-fast --locked >/aver/out/rust.txt 2>&1"),
            "cargo test"
        );
        assert_eq!(key("python -m pytest -q"), "pytest");
        assert_eq!(key("make test"), "make test");

        assert_eq!(
            in_dir(Some("web"), "cd ../api && npm test"),
            (Some("api".to_owned()), "npm test".to_owned())
        );
        assert_eq!(
            commands("cargo test \\\n  --workspace\n# note\nnpm test"),
            ["cargo test --workspace", "npm test"]
        );
    }
}
