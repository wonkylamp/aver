//! Hunk necessity (ARCHITECTURE.md §1.9.4): for a verdict that would be
//! `PASS`, each source hunk of the change is reverted alone and only the
//! proving tests run on what is left, selected by name as a focused rerun
//! selects them ([`focus::select`]). A hunk whose reversion leaves every
//! proving test passing is untested, and holds the verdict at `UNPROVEN`.
//! A reversion that does not build or load (no proving test ran) is tried
//! again together with the hunks of its file that share the identifiers it
//! introduces. Everything uncertain counts as needed, so the phase can only
//! lower `PASS`.

use super::hunks::{self, Hunk};
use super::*;
use crate::diagnostics::{Ablated, HunkTried, Necessity, NotChecked, MAX_HUNKS};

/// One changed source file and its candidate hunks.
struct FileHunks {
    path: Path,
    base: Option<String>,
    head: String,
    hunks: Vec<Hunk>,
}

/// The proving tests of one check, and the command that runs only them.
struct Proving {
    def: CheckDef,
    ids: BTreeSet<String>,
}

impl Verifier {
    /// Hunk necessity over `judgment`, which would be `PASS`: what it found,
    /// with its runs. `None` when the change has no source hunk to revert.
    pub(super) async fn necessity(
        &self,
        plan: &Arc<Plan>,
        judgment: &Judgment,
        fresh: bool,
        progress: Option<Arc<crate::progress::Log>>,
        pools: Option<&Places>,
    ) -> Result<(Option<Necessity>, Vec<TimedRun>), VerifyError> {
        let this = self.clone();
        let p = plan.clone();
        let files = blocking(move || this.source_hunks(&p)).await?;
        let total: usize = files.iter().map(|f| f.hunks.len()).sum();
        if total == 0 {
            return Ok((None, Vec::new()));
        }
        let not_checked = |why| {
            Ok((
                Some(Necessity::NotChecked {
                    why,
                    hunks: u32::try_from(total).unwrap_or(u32::MAX),
                }),
                Vec::new(),
            ))
        };
        if total > MAX_HUNKS {
            return not_checked(NotChecked::TooManyHunks);
        }
        let proving = match proving_tests(plan, judgment) {
            Ok(p) if p.is_empty() => return not_checked(NotChecked::NoProvingTests),
            Ok(p) => p,
            Err(check) => return not_checked(NotChecked::NoSelection { check }),
        };
        let executor = self.executor.for_requester(plan.task_id);
        let exec = match pools.and_then(|p| p.head.clone()) {
            Some(place) => executor.in_slot(place),
            None => executor,
        };
        let head_tests = self.store.get::<Snapshot>(&plan.head_tests)?;
        let mut runs = Vec::new();
        let mut tried = Vec::new();
        for file in &files {
            let mut outcomes: Vec<Ablated> = Vec::new();
            for hunk in &file.hunks {
                let (outcome, ran) = self
                    .try_ablation(
                        plan,
                        &exec,
                        &head_tests,
                        file,
                        &[hunk],
                        &proving,
                        fresh,
                        &progress,
                    )
                    .await?;
                runs.extend(ran);
                outcomes.push(outcome);
            }
            // An uncompilable reversion is tried again with the hunks its
            // new identifiers tie it to; a component that then survives
            // flags each of its hunks no reversion on its own needed.
            let mut coupled: Vec<Option<Ablated>> = vec![None; file.hunks.len()];
            let mut flagged = vec![false; file.hunks.len()];
            let mut components: Vec<(BTreeSet<usize>, Ablated)> = Vec::new();
            for i in 0..file.hunks.len() {
                if outcomes[i] != Ablated::Uncompilable {
                    continue;
                }
                let members = hunks::component(&file.hunks, i);
                if members.len() < 2 {
                    continue;
                }
                let result = match components.iter().find(|(m, _)| *m == members) {
                    Some((_, result)) => *result,
                    None => {
                        let set: Vec<&Hunk> = members.iter().map(|m| &file.hunks[*m]).collect();
                        let (result, ran) = self
                            .try_ablation(
                                plan,
                                &exec,
                                &head_tests,
                                file,
                                &set,
                                &proving,
                                fresh,
                                &progress,
                            )
                            .await?;
                        runs.extend(ran);
                        components.push((members.clone(), result));
                        result
                    }
                };
                coupled[i] = Some(result);
                if result == Ablated::Survived {
                    for m in &members {
                        if outcomes[*m] != Ablated::Killed {
                            flagged[*m] = true;
                        }
                    }
                }
            }
            for (i, hunk) in file.hunks.iter().enumerate() {
                let (start, end) = hunk.lines();
                let untested = outcomes[i] == Ablated::Survived || flagged[i];
                tried.push(HunkTried {
                    path: file.path.to_string(),
                    start,
                    end,
                    outcome: outcomes[i],
                    coupled: coupled[i],
                    reason: untested.then_some(UnprovenReason::UntestedHunk),
                });
            }
        }
        Ok((Some(Necessity::Checked { hunks: tried }), runs))
    }

    /// The candidate hunks of every changed source file, in path order; a
    /// file that is not UTF-8 text has none.
    fn source_hunks(&self, plan: &Plan) -> Result<Vec<FileHunks>, VerifyError> {
        let text = |blob: &BlobId| -> Result<Option<String>, VerifyError> {
            Ok(String::from_utf8(self.store.get_raw(blob)?).ok())
        };
        let mut out = Vec::new();
        for source in &plan.sources {
            let Some(head) = text(&source.head)? else {
                continue;
            };
            let base = match &source.base {
                Some(blob) => match text(blob)? {
                    Some(b) => Some(b),
                    None => continue,
                },
                None => None,
            };
            let hunks = hunks::hunks(base.as_deref(), &head);
            if !hunks.is_empty() {
                out.push(FileHunks {
                    path: source.path.clone(),
                    base,
                    head,
                    hunks,
                });
            }
        }
        Ok(out)
    }

    /// The head-tests tree with `set` reverted in `file`, its proving tests
    /// run once per check: needed if any check's run says so, else
    /// uncompilable if any could not run them, else survived.
    #[allow(clippy::too_many_arguments)]
    async fn try_ablation(
        &self,
        plan: &Plan,
        exec: &Executor,
        head_tests: &Snapshot,
        file: &FileHunks,
        set: &[&Hunk],
        proving: &[Proving],
        fresh: bool,
        progress: &Option<Arc<crate::progress::Log>>,
    ) -> Result<(Ablated, Vec<TimedRun>), VerifyError> {
        let text = hunks::ablate(file.base.as_deref(), &file.head, set);
        let entry = if file.base.is_none() && text.is_empty() {
            None
        } else {
            Some(FileEntry {
                kind: FileKind::File,
                blob: self.store.put_blob(text.as_bytes())?,
            })
        };
        let tree = self.workspaces.derive_snapshot(
            &head_tests.root,
            &[(file.path.clone(), entry)],
            head_tests.env.ok_or_else(|| {
                VerifyError::Internal("the head-tests tree has no environment".into())
            })?,
        )?;
        let mut outcomes = Vec::new();
        let mut timed = Vec::new();
        for p in proving {
            let log = progress.clone().map(|log| (log, Side::Ablation));
            let runs = run_side(
                exec,
                &self.store,
                &plan.head_env,
                &tree,
                &p.def,
                Reruns::ABLATION,
                fresh,
                false,
                None,
                log,
                None,
            )
            .await?;
            outcomes.push(match runs.last() {
                Some(r) => classify(&r.run, &p.def, &p.ids),
                None => Ablated::Killed,
            });
            for r in runs {
                timed.push(TimedRun {
                    side: Side::Ablation,
                    check: p.def.id.clone(),
                    index: r.run.index.get(),
                    run: r.id,
                    cache_hit: r.cache_hit,
                    from_cache: r.cache_hit,
                    started_at: r.run.started_at,
                    install: if r.cache_hit {
                        Duration::ZERO
                    } else {
                        r.timing.install
                    },
                    sandbox: if r.cache_hit {
                        Duration::ZERO
                    } else {
                        r.timing.check
                    },
                    confirmation: false,
                });
            }
        }
        let outcome = if outcomes.contains(&Ablated::Killed) {
            Ablated::Killed
        } else if outcomes.contains(&Ablated::Uncompilable) {
            Ablated::Uncompilable
        } else {
            Ablated::Survived
        };
        Ok((outcome, timed))
    }
}

/// The proving tests by check, with each check's command narrowed to them:
/// the tests that proved a criterion, or with no criteria the change's own
/// tests that show it. `Err(check)` when a proof is not a named test or its
/// check cannot run tests by name.
fn proving_tests(plan: &Plan, judgment: &Judgment) -> Result<Vec<Proving>, CheckId> {
    let body = &judgment.body;
    let ids: Vec<EntryId> = if body.criteria.is_empty() {
        // Without criteria the change's evidence is its own tests that show
        // it and the base's tests it fixed: a fixed test reaches the fix as
        // surely as a new one, and leaving it out would call the code it
        // needs untested.
        let fixed = body.checks.iter().filter(|e| {
            e.base == BaseStatus::Fail
                && e.head == HeadStatus::Pass
                && e.cause.is_none()
                && e.id
                    .as_str()
                    .split_once("::")
                    .is_some_and(|(_, test)| !test.starts_with('('))
        });
        judgment
            .shown
            .shows_change
            .iter()
            .map(|s| s.id.clone())
            .chain(fixed.map(|e| e.id.clone()))
            .collect()
    } else {
        body.checks
            .iter()
            .filter(|e| !e.proves.is_empty() && rules::proves(e, ProofRule::Transplant))
            .map(|e| e.id.clone())
            .collect()
    };
    let mut by_check: BTreeMap<CheckId, BTreeSet<String>> = BTreeMap::new();
    for id in ids {
        let Some((check, test)) = id.as_str().split_once("::") else {
            let check: CheckId = id.as_str().parse().map_err(|_| unknown())?;
            return Err(check);
        };
        let check: CheckId = check.parse().map_err(|_| unknown())?;
        if test.starts_with('(') {
            return Err(check);
        }
        by_check.entry(check).or_default().insert(test.to_owned());
    }
    by_check
        .into_iter()
        .map(|(check, ids)| {
            let def = plan
                .checks
                .iter()
                .find(|p| p.def.id == check)
                .map(|p| &p.def)
                .ok_or_else(|| check.clone())?;
            if def
                .report
                .as_ref()
                .is_none_or(|r| r.format.lists_failures_only())
            {
                return Err(check);
            }
            let def = focus::select(def, &ids).ok_or_else(|| check.clone())?;
            Ok(Proving { def, ids })
        })
        .collect()
}

fn unknown() -> CheckId {
    "unknown".parse().expect("a valid check id")
}

/// What one run of the proving tests over a reverted hunk says. Only a
/// clean run that reports every proving test passing is `Survived`; no
/// proving test reported by a run that failed is `Uncompilable`; anything
/// else, a lost run included, keeps the hunk.
pub(super) fn classify(run: &CheckRun, def: &CheckDef, ids: &BTreeSet<String>) -> Ablated {
    let end = ending(run, def);
    if end.is_infra() {
        return Ablated::Killed;
    }
    let statuses: Vec<Option<TestStatus>> = ids
        .iter()
        .map(|id| {
            run.tests
                .iter()
                .find(|t| t.id.test_path() == id)
                .map(|t| t.status)
        })
        .collect();
    if statuses.iter().all(Option::is_none) {
        return if end == Ending::Fail {
            Ablated::Uncompilable
        } else {
            Ablated::Killed
        };
    }
    if end == Ending::Pass
        && run.report.is_some()
        && statuses.iter().all(|s| *s == Some(TestStatus::Pass))
    {
        Ablated::Survived
    } else {
        Ablated::Killed
    }
}
