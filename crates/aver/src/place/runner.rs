//! Runs one check in a slot (ARCHITECTURE.md §2.4.6): the slot is entered
//! (cloned when needed, swapped to the run's tree), the check's commands
//! run there through [`proc::run`] with the caller's environment
//! ([`proc::inherit`]), and the slot's lock is held until the run ends, so
//! two runs never share a slot at once.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use aver_core::{
    CheckDef, DepsState, Environment, GitSha, HostProfile, ProvenanceMode, SnapshotId, TreeId, Uid,
};

use super::proc::{self, append, disk_guard, private_tmp, remove_all, remove_runs, ProcFailure};
use super::sides::{deps_of, Deps, PlaceError, RunSlot, Sides, Slot, SlotName};
use super::workdir::Workdir;
use crate::exec::output::{Exit, SandboxLimits, SandboxOutput};
use crate::store::Store;
use crate::workspace::Glob;

/// Where a check's report and output files live in an environment's
/// `CheckDef`; argv mentioning this prefix is rewritten to the run's own
/// `AVER_OUT`.
const SANDBOX_OUT: &str = "/aver/out/";

/// Which slot a run uses, and what it is cloned from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Place {
    /// The repository directory whose slots these are; also the default
    /// source.
    pub repo: PathBuf,
    pub slot: SlotName,
    /// The directory the slot is cloned from (its dependencies installed):
    /// the agent's workspace, the user's checkout, the CI checkout. `None`
    /// clones an empty directory.
    pub source: Option<PathBuf>,
    /// A built slot of the same repository to clone instead of `source`
    /// when it is idle and was cloned with the same dependencies
    /// ([`super::sides::Sides::enter_seeded`]): the transplant's base.
    pub seed: Option<SlotName>,
    /// Run at lower priority (the transplant, so the head finishes first).
    pub nice: bool,
}

/// How a slot runner is set up.
#[derive(Debug, Clone)]
pub struct RunnerConfig {
    /// `$AVER_HOME`.
    pub home: PathBuf,
    /// The caller's environment, which every run inherits.
    pub vars: BTreeMap<String, String>,
    /// `local` for the verifier's machine, `ci` for `aver ci`.
    pub mode: ProvenanceMode,
    /// Run every command as this user through `sudo -n -u` (uid
    /// separation in CI, §3.2); `None` runs as the caller.
    pub run_as: Option<String>,
    /// Where the slots live; by default `$AVER_HOME/sides`.
    pub root: Option<PathBuf>,
    /// What a run with no place of its own is cloned from: the CI
    /// checkout, say.
    pub source: Option<PathBuf>,
    /// Run in this checkout instead of slots once [`SlotRunner::adopt`]
    /// takes it over (`aver ci` in place, ARCHITECTURE.md §2.9).
    pub in_place: Option<PathBuf>,
}

impl RunnerConfig {
    /// For this process: its environment.
    pub fn for_process(home: impl Into<PathBuf>, mode: ProvenanceMode) -> Self {
        Self {
            home: home.into(),
            vars: std::env::vars().collect(),
            mode,
            run_as: None,
            root: None,
            source: None,
            in_place: None,
        }
    }
}

/// One run, as the executor asks for it.
pub struct SlotRequest<'a> {
    pub root: TreeId,
    pub env: &'a Environment,
    pub check: &'a CheckDef,
    pub limits: SandboxLimits,
    pub place: &'a Place,
}

pub struct SlotRunner {
    config: RunnerConfig,
    sides: Sides,
    store: Arc<Store>,
    run_as: Option<proc::RunAs>,
    profile: OnceLock<HostProfile>,
    executor: OnceLock<String>,
    /// Set once by [`SlotRunner::adopt`]: the adopted checkout's layout,
    /// or why it was not adopted (runs then use slots).
    adopted: OnceLock<Result<Adoption, String>>,
}

/// An adopted checkout every run swaps (§3.1), whatever its place.
struct Adoption {
    dir: PathBuf,
    deps: Deps,
    /// The checkout's dependency state at the adopt, at the head: the
    /// cache key's, whichever tree the checkout holds when a key is made.
    state: DepsState,
    /// The base's tree: a run of it is a base run.
    base: TreeId,
    /// Whether a tree other than the base's ran since the last base run.
    foreign: AtomicBool,
}

impl SlotRunner {
    pub fn new(store: Arc<Store>, config: RunnerConfig) -> Self {
        let sides = match &config.root {
            Some(root) => Sides::at(root, store.clone()),
            None => Sides::new(&config.home, store.clone()),
        };
        let run_as = config.run_as.clone().map(|user| proc::RunAs { user });
        Self {
            config,
            sides,
            store,
            run_as,
            profile: OnceLock::new(),
            executor: OnceLock::new(),
            adopted: OnceLock::new(),
        }
    }

    /// Takes over the configured checkout (`in_place`) at `head` for every
    /// later run, keeping `deps` ([`Workdir::adopt`]). A no-op without one.
    /// When the checkout cannot be adopted (its tracked files are not the
    /// head's, say), runs use slots cloned from it, and the error says why.
    pub fn adopt(&self, head: (GitSha, TreeId), base: TreeId, deps: Deps) -> Result<(), String> {
        let Some(dir) = &self.config.in_place else {
            return Ok(());
        };
        let adopted = (|| {
            Workdir::adopt(
                dir,
                &self.store,
                head,
                deps.keep_all().map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let state = deps.state_of(dir).map_err(|e| e.to_string())?;
            Ok(Adoption {
                dir: dir.clone(),
                deps,
                state,
                base,
                foreign: AtomicBool::new(false),
            })
        })();
        let error = adopted.as_ref().err().cloned();
        let _ = self.adopted.set(adopted);
        error.map_or(Ok(()), Err)
    }

    /// Whether runs were set up to happen in a checkout.
    pub fn in_place(&self) -> bool {
        self.config.in_place.is_some()
    }

    /// The checkout runs swap, once adopted.
    pub fn adopted_dir(&self) -> Option<&FsPath> {
        self.adoption().map(|a| a.dir.as_path())
    }

    fn adoption(&self) -> Option<&Adoption> {
        self.adopted.get().and_then(|a| a.as_ref().ok())
    }

    pub fn sides(&self) -> &Sides {
        &self.sides
    }

    pub fn mode(&self) -> ProvenanceMode {
        self.config.mode
    }

    pub fn uid(&self) -> Uid {
        match self.run_as {
            Some(_) => Uid::Separate,
            None => Uid::Same,
        }
    }

    /// Whether runs must not overlap: one run's cleanup under uid
    /// separation kills every process of the run user.
    pub fn serial(&self) -> bool {
        self.run_as.is_some() || self.config.in_place.is_some()
    }

    /// The host's toolchains on the caller's `PATH`, probed on first use.
    pub fn profile(&self) -> &HostProfile {
        self.profile.get_or_init(|| {
            super::host::profile(
                &self.config.vars,
                &[],
                &self.sides.root().join("versions.json"),
            )
        })
    }

    /// `native-v0.<16 hex of the host profile>`.
    pub fn executor(&self) -> &str {
        self.executor.get_or_init(|| self.profile().executor())
    }

    /// The executor component of a run's cache key at `place`: the host
    /// profile, the provenance mode, and the dependency inputs of the
    /// directory the slot is cloned from, so a run with other installed
    /// dependencies is never served another's result.
    pub fn key_executor(&self, place: Option<&Place>, env: &Environment) -> String {
        let source = place
            .and_then(|p| p.source.clone())
            .or_else(|| self.config.source.clone());
        let deps = deps_of(env);
        let state = match (self.adoption(), &source) {
            (Some(adopted), _) => Some(adopted.state.clone()),
            (None, Some(dir)) => {
                let src = deps.source(dir);
                self.sides.deps_state_of(&src)
            }
            (None, None) => None,
        };
        let mut key = format!("{}+{}", self.executor(), self.config.mode.as_str());
        if let Some(state) = state {
            key.push('.');
            key.push_str(&state.as_str()[..16]);
        }
        if self.run_as.is_some() {
            key.push_str("+separate");
        }
        key
    }

    /// The place of a run outside any verification: a slot of its own
    /// snapshot, so head code never writes a base slot.
    pub fn place_of(&self, snapshot: &SnapshotId) -> Place {
        Place {
            repo: self.default_repo(),
            slot: SlotName::Snapshot(*snapshot),
            source: self.config.source.clone(),
            seed: None,
            nice: false,
        }
    }

    /// The base slot of tree `tree` in repository `repo` (when known, also
    /// what it is cloned from), else in the default one.
    pub fn base_place(&self, repo: Option<&FsPath>, tree: TreeId) -> Place {
        Place {
            repo: repo.map_or_else(|| self.default_repo(), FsPath::to_path_buf),
            slot: SlotName::Base(tree),
            source: repo
                .map(FsPath::to_path_buf)
                .or_else(|| self.config.source.clone()),
            seed: None,
            nice: false,
        }
    }

    /// The repository directory naming the slots of runs with no
    /// repository of their own: the configured source, else
    /// `<sides>/default`.
    pub fn default_repo(&self) -> PathBuf {
        self.config
            .source
            .clone()
            .unwrap_or_else(|| self.sides.root().join("default"))
    }

    /// Runs one check to its end, blocking. Holds one of the host's run
    /// slots and the slot's lock for the whole run.
    pub fn run(&self, req: SlotRequest<'_>) -> Result<SandboxOutput, ProcFailure> {
        let entered = self.enter(&req)?;
        self.run_in(req, &entered)
    }

    /// [`Self::run`], with the slot's lock held around the cache: `cached`
    /// is asked first, so a result another process stored while this one
    /// waited for the lock is returned instead of run again, and `keep`
    /// sees the run's result before the lock is released, so the next
    /// process to take it finds what `keep` stored. A slot that could not
    /// be entered goes to `keep` as the failure, with no lock held.
    pub fn run_cached<T>(
        &self,
        req: SlotRequest<'_>,
        cached: impl FnOnce() -> Option<T>,
        keep: impl FnOnce(Result<SandboxOutput, ProcFailure>) -> T,
    ) -> T {
        let entered = match self.enter(&req) {
            Ok(entered) => entered,
            Err(e) => return keep(Err(e)),
        };
        if let Some(hit) = cached() {
            return hit;
        }
        keep(self.run_in(req, &entered))
    }

    fn enter(&self, req: &SlotRequest<'_>) -> Result<Entered, ProcFailure> {
        if let Some(adopted) = self.adoption() {
            return self.enter_adopted(adopted, req.root);
        }
        let place = req.place;
        let empty = self.sides.root().join("empty");
        fs::create_dir_all(&empty)?;
        let source_dir = place
            .source
            .clone()
            .or_else(|| self.config.source.clone())
            .unwrap_or(empty);
        let deps = deps_of(req.env);
        let source = deps.source(&source_dir);
        disk_guard(self.sides.root())?;
        let run_slot = self.sides.run_slot()?;
        let slot = self
            .sides
            .enter_seeded(
                &place.repo,
                &place.slot,
                &source,
                place.seed.as_ref(),
                req.root,
            )
            .map_err(|e| match e {
                PlaceError::Io(e) => ProcFailure::from(e),
                e => ProcFailure::new(format!("preparing the run's directory: {e}")),
            })?;
        Ok(Entered {
            slot,
            _run_slot: run_slot,
        })
    }

    /// The adopted checkout swapped to `tree`, under its lock: every place
    /// runs here, one run at a time ([`SlotRunner::serial`]).
    fn enter_adopted(&self, adopted: &Adoption, tree: TreeId) -> Result<Entered, ProcFailure> {
        if proc::interrupt::requested() {
            return Err(ProcFailure::Interrupted);
        }
        disk_guard(self.sides.root())?;
        let run_slot = self.sides.run_slot()?;
        let base = tree == adopted.base;
        // The base's runs must not read what head code built: a head that
        // wrote into `target` could make its own failure the base's too.
        let scrub = base && adopted.foreign.load(Ordering::SeqCst);
        let slot = self
            .sides
            .enter_adopted(&adopted.dir, &adopted.deps, &adopted.state, tree, scrub)
            .map_err(|e| match e {
                PlaceError::Io(e) => ProcFailure::from(e),
                e => ProcFailure::new(format!("swapping the checkout: {e}")),
            })?;
        adopted.foreign.store(!base, Ordering::SeqCst);
        Ok(Entered {
            slot,
            _run_slot: run_slot,
        })
    }

    fn run_in(
        &self,
        req: SlotRequest<'_>,
        entered: &Entered,
    ) -> Result<SandboxOutput, ProcFailure> {
        let (place, slot) = (req.place, &entered.slot);
        // What the run's result is read from (its output files and log)
        // goes in a directory new to this run: nothing an earlier run's
        // process still writes can land there.
        fs::create_dir_all(&slot.runs)?;
        remove_runs(&slot.runs)?;
        let fresh = slot.runs.join(format!("run-{}", ulid::Ulid::generate()));
        let out = fresh.join("out");
        fs::create_dir_all(&out)?;
        let tmp = private_tmp(&slot.runs)?;
        let tag = ulid::Ulid::generate().to_string();
        let mut vars = proc::inherit(self.config.vars.clone(), &out, &tmp, &slot.dir, &tag);
        for (name, value) in &req.env.vars {
            let name = name.as_str();
            if !proc::stripped(name) && name != "PATH" {
                vars.insert(name.to_owned(), value.clone());
            }
        }
        if self.run_as.is_some() {
            // The run user cannot read the caller's home.
            let home = fresh.join("home");
            fs::create_dir_all(&home)?;
            vars.insert("HOME".into(), home.to_string_lossy().into_owned());
        }
        let cwd = match &req.check.cwd {
            Some(c) => slot.dir.join(c.as_str()),
            None => slot.dir.clone(),
        };
        let path = vars.get("PATH").map(std::ffi::OsStr::new);
        let command = |argv: &[String]| python_fallback(&rehome(argv, &out), path);
        let mut commands: Vec<Vec<String>> = req.check.setup.iter().map(|s| command(s)).collect();
        commands.push(command(&req.check.argv));
        let log = fresh.join("log");
        fs::write(&log, b"")?;
        if let Some(note) = &slot.leftover {
            append(&log, note.as_bytes())?;
        }
        if slot.deps_differ {
            append(
                &log,
                b"[aver] the dependency directories were installed from other lockfiles than this tree's\n",
            )?;
        }
        if !cwd.is_dir() {
            append(
                &log,
                format!(
                    "[aver] the check's directory {} does not exist\n",
                    req.check.cwd.as_ref().map_or("", |c| c.as_str())
                )
                .as_bytes(),
            )?;
            return Ok(proc::output(
                Exit::Exited(127),
                &log,
                &out,
                Duration::ZERO,
                req.limits.output_bytes,
            ));
        }
        let report = req
            .check
            .report
            .as_ref()
            .map(|d| d.path.as_str().trim_start_matches(SANDBOX_OUT));
        let result = proc::run(&proc::Spawn {
            commands: &commands,
            cwd: &cwd,
            vars: &vars,
            out: &out,
            log: &log,
            timeout: req.limits.timeout,
            output_bytes: req.limits.output_bytes,
            nice: place.nice,
            record: (&slot.record, &tag),
            report,
            own: &[&slot.dir, &fresh, &tmp],
            withheld: &slot.withheld,
            run_as: self.run_as.as_ref(),
        });
        let roots = [
            (slot.dir.as_path(), crate::exec::junit::SRC_ROOT),
            (tmp.as_path(), crate::exec::junit::TMP_ROOT),
        ];
        let result = result.map(|mut out| {
            if let Some(declared) = &req.check.report {
                for report in out.files.values_mut() {
                    *report = crate::exec::junit::unroot(report, declared.format, &roots);
                }
            }
            // Under the slot's lock still, so the files are this run's.
            out.built = slot
                .work
                .derived(built_globs())
                .ok()
                .map(|found| found.into_iter().collect());
            out
        });
        // What a test left in its TMPDIR goes with the run.
        let _ = remove_all(&tmp);
        result
    }
}

/// A slot entered for one run, with the host run slot it holds.
struct Entered {
    slot: Slot,
    _run_slot: RunSlot,
}

/// The built test files every run fingerprints
/// ([`aver_core::contract::DEFAULT_BUILT_TESTS`]).
fn built_globs() -> &'static [Glob] {
    static GLOBS: OnceLock<Vec<Glob>> = OnceLock::new();
    GLOBS.get_or_init(|| {
        aver_core::contract::DEFAULT_BUILT_TESTS
            .iter()
            .map(|g| Glob::new(g).expect("the default built test globs are valid"))
            .collect()
    })
}

fn rehome(argv: &[String], out: &FsPath) -> Vec<String> {
    let to = format!("{}/", out.display());
    argv.iter().map(|a| a.replace(SANDBOX_OUT, &to)).collect()
}

/// The executable `name` in the directories of `path`, if any.
fn on_path(name: &str, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    std::env::split_paths(path?)
        .map(|d| d.join(name))
        .find(|p| fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// `argv` with its program `python` (after `env` and its `NAME=value`
/// arguments, as generic mode writes it) replaced by `python3` when `path`
/// has no `python` but has `python3`: stock macOS and many Linux
/// distributions ship only `python3`. The slot runner applies it to every
/// command of a verify.
pub fn python_fallback(argv: &[String], path: Option<&std::ffi::OsStr>) -> Vec<String> {
    let mut out = argv.to_vec();
    let mut at = 0;
    if out.first().is_some_and(|p| p == "env") {
        at = 1;
        while out
            .get(at)
            .is_some_and(|a| a.contains('=') && !a.starts_with('-'))
        {
            at += 1;
        }
    }
    if out.get(at).is_some_and(|p| p == "python")
        && on_path("python", path).is_none()
        && on_path("python3", path).is_some()
    {
        out[at] = "python3".to_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generic mode's `python -m pytest` runs as `python3 -m pytest` where
    /// only `python3` is installed, also behind `env NAME=value`.
    #[test]
    fn python_falls_back_to_python3_where_only_python3_is_on_path() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let exe = |name: &str| {
            let p = dir.path().join(name);
            fs::write(&p, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        };
        let path = dir.path().as_os_str().to_owned();
        let argv = |s: &str| s.split(' ').map(str::to_owned).collect::<Vec<_>>();
        let pytest = argv("env PYTHONPATH=src python -m pytest");
        assert_eq!(
            python_fallback(&pytest, Some(&path)),
            pytest,
            "neither: unchanged"
        );
        exe("python3");
        assert_eq!(
            python_fallback(&pytest, Some(&path)),
            argv("env PYTHONPATH=src python3 -m pytest")
        );
        assert_eq!(
            python_fallback(&argv("python -m unittest"), Some(&path)),
            argv("python3 -m unittest")
        );
        assert_eq!(
            python_fallback(&argv("npm test"), Some(&path)),
            argv("npm test")
        );
        exe("python");
        assert_eq!(python_fallback(&pytest, Some(&path)), pytest, "python wins");
    }
}
