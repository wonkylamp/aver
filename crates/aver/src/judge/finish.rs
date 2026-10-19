//! Judging the runs: each check's entries and blockers, built-artifact,
//! protected, lease and judging-file entries, the verdict, the receipt and
//! its diagnostics.

use super::*;

/// A verification's judged receipt body before signing, and what its
/// diagnostics are built from.
pub(super) struct Judgment {
    pub(super) body: ReceiptBody,
    evidence: BTreeMap<EntryId, Evidence>,
    changes: Changes,
    pub(super) shown: diagnostics::Evidence,
    weak: Vec<diagnostics::WeakProof>,
    cap: Option<rules::Cap>,
    pub(super) results: Vec<CheckRuns>,
}

impl Verifier {
    /// The receipt body the runs give, with what its diagnostics need,
    /// before hunk necessity and signing.
    pub(super) fn judge_runs(
        &self,
        caller: &AgentId,
        plan: &Plan,
        results: &[CheckRuns],
        source: Option<&std::path::Path>,
    ) -> Result<Judgment, VerifyError> {
        let dense: Vec<CheckRuns> = results.iter().map(densify).collect();
        let results = dense.as_slice();
        let attempt = plan.attempt;
        let criteria = &plan.task.body.criteria;
        let mut entries = plan.protected.clone();
        let judging = judging_entries(&plan.judging);
        let harness_changed: Vec<EntryId> = judging.iter().map(|e| e.id.clone()).collect();
        let mut evidence = BTreeMap::new();
        for entry in &judging {
            evidence.insert(
                entry.id.clone(),
                Evidence {
                    note: Some(diagnostics::CHANGED_HARNESS_NOTE.to_owned()),
                    ..Evidence::default()
                },
            );
        }
        entries.extend(judging);
        for entry in &plan.leased {
            evidence.insert(
                entry.id.clone(),
                Evidence {
                    note: Some(diagnostics::LEASED_NOTE.to_owned()),
                    ..Evidence::default()
                },
            );
        }
        entries.extend(plan.leased.iter().cloned());
        for entry in &plan.sensitive {
            evidence.insert(
                entry.id.clone(),
                Evidence {
                    note: Some(diagnostics::SENSITIVE_NOTE.to_owned()),
                    ..Evidence::default()
                },
            );
        }
        entries.extend(plan.sensitive.iter().cloned());
        let context = EntryContext::new(&self.store, plan)?;
        for check in results {
            let budget = plan.env.limits.output_bytes as usize;
            let mut found = check_entries(&self.store, check, criteria, &context)?;
            found.extend(setup_entries(&self.store, check, budget)?);
            for (entry, from) in found {
                evidence.insert(entry.id.clone(), from);
                entries.push(entry);
            }
        }
        for (entry, from) in built_entries(&self.store, results)? {
            evidence.insert(entry.id.clone(), from);
            entries.push(entry);
        }
        if let (Some(dir), Some(first)) = (source, results.first()) {
            for (entry, from) in self.stale_entries(plan, dir, first)? {
                evidence.insert(entry.id.clone(), from);
                entries.push(entry);
            }
        }
        let checks = Set::new(entries).map_err(|e| VerifyError::Internal(e.to_string()))?;
        let changes = test_changes(&plan.tests, results, harness_changed);
        let uncovered = (!plan.uncovered.is_empty()).then(|| Listed::of(plan.uncovered.clone()));
        let (unproven, verdict) = rules::verdict_with_coverage(
            checks.as_slice(),
            criteria,
            ProofRule::Transplant,
            uncovered.is_some(),
        );
        let (ci, reproduce) = match &plan.ci {
            Some(ci) => (Some(ci.info.clone()), ci.reproduce.clone()),
            None => {
                let command = reproduce(&plan.task_id, &plan.head, &plan.base);
                (
                    None,
                    if plan.all {
                        format!("{command} --all")
                    } else {
                        command
                    },
                )
            }
        };
        let levels = plan
            .checks
            .iter()
            .map(|p| (p.def.id.clone(), Level::of(p.def.report.as_ref())))
            .collect();
        let mut body = ReceiptBody {
            schema: ReceiptSchema::V3,
            task_id: plan.task_id,
            attempt_id: attempt,
            agent_id: *caller,
            base_snapshot: plan.base,
            head_snapshot: plan.head,
            environment_hash: plan.env_id,
            env_changed: plan.env_changed,
            criteria: criteria.clone(),
            checks,
            unproven,
            verdict,
            reproduce,
            verifier: VERIFIER.to_owned(),
            created_at: Timestamp::now(),
            executor: Some(self.executor.executor().to_owned()),
            config: Some(plan.config),
            test_changes: Some(changes.receipt()),
            ci,
            uncovered,
            skipped: skipped_checks(&plan.skipped),
            provenance: Some(self.executor.provenance()),
            levels: Some(levels),
        };
        let names = self.changed_names(plan)?;
        let (shown, weak) =
            change_evidence(&self.store, results, &body, &context, &evidence, &names)?;
        weaken(&mut body, &weak);
        let runtime_tests = results.is_empty() || ran_base_tests(&self.store, results, &context)?;
        let (verdict, cap) = rules::cap(body.verdict, runtime_tests, plan.proposed);
        body.verdict = verdict;
        Ok(Judgment {
            body,
            evidence,
            changes,
            shown,
            weak,
            cap,
            results: dense,
        })
    }

    /// Signs `judgment`'s receipt, after hunk necessity has had its say,
    /// and stores it with its diagnostics.
    pub(super) fn seal(
        &self,
        plan: &Plan,
        judgment: Judgment,
        necessity: Option<diagnostics::Necessity>,
        ablation: Vec<TimedRun>,
    ) -> Result<Verified, VerifyError> {
        let Judgment {
            mut body,
            evidence,
            changes,
            shown,
            weak,
            cap,
            results,
        } = judgment;
        let results = results.as_slice();
        let attempt = plan.attempt;
        let outcome = match &necessity {
            Some(n @ diagnostics::Necessity::Checked { .. }) => {
                let survived: Vec<rules::HunkAt> = n
                    .untested()
                    .filter_map(|h| {
                        Some(rules::HunkAt {
                            path: h.path.parse().ok()?,
                            lines: aver_core::pushback::LineRange {
                                start: h.start,
                                end: h.end,
                            },
                        })
                    })
                    .collect();
                if survived.is_empty() {
                    rules::AblationOutcome::AllKilled
                } else {
                    rules::AblationOutcome::SurvivedHunk(survived)
                }
            }
            _ => rules::AblationOutcome::NotRun,
        };
        body.verdict = rules::ablated(body.verdict, &outcome);
        // A dry run's receipt is signed by a key nobody holds.
        let throwaway;
        let key = if self.dry {
            throwaway = SigningKey::from_bytes(&rand::random::<[u8; 32]>());
            &throwaway
        } else {
            &*self.server_key
        };
        let receipt = Receipt::sign(body, key);
        let id = aver_core::Object::id(&receipt);
        let mut diagnostics = self.diagnostics(plan, &receipt.body, id, &evidence)?;
        diagnostics.test_changes = Some(changes.diagnostics());
        diagnostics.evidence = Some(shown).filter(|e| !e.is_empty());
        for u in &mut diagnostics.unproven {
            if weak.iter().any(|w| w.criterion == u.criterion) {
                u.reason = UnprovenReason::WeakProof;
            }
        }
        diagnostics.weak_proofs = weak;
        diagnostics.cap = cap;
        diagnostics.reason = plan.reason.clone();
        diagnostics.proof_sources = self.proof_sources(plan, &receipt.body)?;
        diagnostics.uncovered = (!plan.uncovered.is_empty()).then(|| {
            diagnostics::Listing::of(plan.uncovered.iter().map(|p| p.to_string()).collect())
        });
        diagnostics.unbuilt = plan.unbuilt.clone();
        diagnostics.review = crate::ci::review_diffs(
            &self.store,
            &self.store.get::<Snapshot>(&plan.base)?,
            &self.store.get::<Snapshot>(&plan.head)?,
            &receipt.body,
            &diagnostics,
        )?
        .into_iter()
        .map(|d| diagnostics::ReviewDiff {
            path: d.path,
            diff: d.diff,
        })
        .collect();
        diagnostics.narrowed = results.iter().flat_map(|c| c.narrowed.clone()).collect();
        diagnostics.necessity = necessity;
        let stored = if self.dry {
            id
        } else {
            fault("verify.before_receipt")?;
            let stored = self.store_receipt(&receipt, &diagnostics, attempt)?;
            fault("verify.after_receipt")?;
            stored
        };
        debug_assert_eq!(stored, id);
        Ok(Verified {
            id,
            receipt,
            diagnostics,
            runs: timed_runs(results).into_iter().chain(ablation).collect(),
            timing: Timing::default(),
        })
    }

    /// One `ENV` entry per stale copy of a changed file in the kept
    /// dependency directories of `dir` (REDESIGN.md §4.1), under the first
    /// check: every head run read the copy, not the change.
    fn stale_entries(
        &self,
        plan: &Plan,
        dir: &std::path::Path,
        check: &CheckRuns,
    ) -> Result<Vec<(ReceiptCheck, Evidence)>, VerifyError> {
        let changed: Vec<Changed> = self
            .workspaces
            .changed_files(&plan.base, &plan.head)?
            .into_iter()
            .map(|(path, base, head)| Changed {
                path,
                base: base.map(|e| e.blob),
                head: head.map(|e| e.blob),
            })
            .collect();
        let deps = crate::place::sides::deps_of(&plan.head_env);
        let stale = crate::place::sides::stale_copies(&self.store, &deps.source(dir), &changed)
            .map_err(|e| VerifyError::Internal(e.to_string()))?;
        Ok(stale
            .iter()
            .filter_map(|s| {
                let blocker = Blocker::StaleCopy {
                    keep: s.keep.parse().ok()?,
                    tracked: s.tracked.parse().ok()?,
                };
                blocker_entry(check, &blocker, None)
            })
            .collect())
    }

    /// Stores a signed receipt and its diagnostics, and appends it to the
    /// attempt's receipt list, in one transaction that re-checks the attempt
    /// is still open.
    fn store_receipt(
        &self,
        receipt: &Receipt,
        diagnostics: &Diagnostics,
        attempt: Option<AttemptId>,
    ) -> Result<ReceiptId, VerifyError> {
        self.store.tx(|tx| {
            let stored = tx.put(receipt)?;
            diagnostics::store(tx, diagnostics)?;
            if let Some(attempt_id) = &attempt {
                let current = tasks::load_attempt(&tx.sql, attempt_id)?.ok_or_else(|| {
                    VerifyError::NotFound(format!("attempt {attempt_id} not found"))
                })?;
                tasks::check_attempt_open(&current)?;
                tasks::push_receipt(tx, attempt_id, &stored)?;
            }
            Ok::<_, VerifyError>(stored)
        })
    }

    /// The diagnostics of a receipt about to be stored (§1.9.4).
    pub(super) fn diagnostics(
        &self,
        plan: &Plan,
        body: &ReceiptBody,
        receipt: ReceiptId,
        evidence: &BTreeMap<EntryId, Evidence>,
    ) -> Result<Diagnostics, VerifyError> {
        let mut failures = Vec::new();
        let rule = body.schema.proof_rule();
        for entry in body.checks.iter() {
            if entry.cause.is_none() && entry.head == HeadStatus::Pass {
                continue;
            }
            let from = evidence.get(&entry.id).cloned().unwrap_or_default();
            let (location, output) = match from.run {
                Some(run_id) => self.run_evidence(&run_id, from.test.as_ref())?,
                None => (None, None),
            };
            let note = rules::inconclusive(entry, rule)
                .then(|| {
                    if entry.cause == Some(aver_core::Cause::Flaky) {
                        diagnostics::FLAKY_BASE_NOTE
                    } else {
                        diagnostics::COMMAND_LEVEL_NOTE
                    }
                    .to_owned()
                })
                .or(from.note.clone());
            failures.push(Failure {
                id: entry.id.clone(),
                cause: entry.cause,
                message: entry.detail.as_ref().map(|d| d.as_str().to_owned()),
                location,
                output,
                note,
                own: from.own,
                file: from.file.clone(),
            });
        }
        let mut reasons: Vec<(CriterionId, UnprovenReason)> = body
            .unproven
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    rules::unproven_reason(body.checks.as_slice(), id, rule),
                )
            })
            .collect();
        let unclaimed: Vec<&CriterionId> = reasons
            .iter()
            .filter(|(_, r)| *r == UnprovenReason::NotClaimed)
            .map(|(id, _)| id)
            .collect();
        // An exit-only check reports no test names, so a tag in a file it
        // runs can never be found in a report: nothing went missing, and
        // when no check that ran names its tests (a type check names
        // errors, not tests), that is the reason.
        let per_test = plan.checks.iter().any(|p| p.def.report.is_some());
        let names_tests = plan.checks.iter().any(|p| {
            p.def
                .report
                .as_ref()
                .is_some_and(|r| !r.format.lists_failures_only())
        });
        if !unclaimed.is_empty() && !names_tests {
            for (_, reason) in &mut reasons {
                if *reason == UnprovenReason::NotClaimed {
                    *reason = UnprovenReason::NoTestNames;
                }
            }
        } else if !unclaimed.is_empty() && per_test {
            let tagged = self.tags_in_head_tests(plan, &unclaimed)?;
            for (id, reason) in &mut reasons {
                if *reason == UnprovenReason::NotClaimed && tagged.contains(id) {
                    *reason = UnprovenReason::ClaimedTestNotFound;
                }
            }
        }
        let test_names: Vec<&str> = body
            .checks
            .iter()
            .filter_map(|e| e.id.as_str().split_once("::"))
            .filter(|(check, test)| {
                *check != JUDGING_CHECK
                    && *check != PROTECTED_CHECK
                    && *check != SENSITIVE_CHECK
                    && *check != BUILT_CHECK
                    && *test != EXIT_TEST_PATH
                    && !test.starts_with(UNBUILT_TEST_PATH)
                    && !test.starts_with(NOT_RERUN_TEST_PATH)
                    && !test.starts_with(DELETED_SOURCE_TEST_PATH)
            })
            .map(|(_, test)| test)
            .collect();
        let unproven = reasons
            .into_iter()
            .map(|(id, reason)| {
                let text = body
                    .criteria
                    .iter()
                    .find(|c| c.id == id)
                    .map_or("", |c| c.text.as_str());
                let closest = match reason {
                    UnprovenReason::NotClaimed | UnprovenReason::ClaimedTestNotFound => {
                        diagnostics::closest(text, test_names.iter().copied())
                    }
                    _ => Vec::new(),
                };
                Unproven {
                    entries: body
                        .checks
                        .iter()
                        .filter(|e| e.proves.get(id.as_str()).is_some())
                        .map(|e| e.id.clone())
                        .collect(),
                    criterion: id,
                    reason,
                    closest,
                }
            })
            .collect();
        Ok(Diagnostics {
            schema: diagnostics::SCHEMA.to_owned(),
            receipt,
            failures,
            unproven,
            test_changes: None,
            timings: None,
            import: None,
            evidence: None,
            weak_proofs: Vec::new(),
            cap: None,
            reason: None,
            proof_sources: Vec::new(),
            uncovered: None,
            unbuilt: Vec::new(),
            review: Vec::new(),
            narrowed: Vec::new(),
            necessity: None,
        })
    }

    /// A failing entry's location (for a test) and output excerpt, from the
    /// run its detail came from.
    fn run_evidence(
        &self,
        run_id: &RunId,
        test: Option<&TestCheckId>,
    ) -> Result<(Option<diagnostics::Location>, Option<String>), VerifyError> {
        let run: CheckRun = self.store.get(run_id)?;
        let output = String::from_utf8_lossy(&self.store.get_raw(&run.output)?).into_owned();
        let output = (!output.is_empty()).then(|| diagnostics::excerpt(&output));
        let Some(test) = test else {
            return Ok((None, output));
        };
        let message = run
            .tests
            .get(test.as_str())
            .and_then(|t| t.message.as_ref())
            .map(|m| m.as_str().to_owned());
        let attributes = match run.report {
            Some(report) => {
                let bytes = self.store.get_raw(&report)?;
                let format = self
                    .store
                    .get::<Environment>(&run.env)?
                    .checks
                    .get(run.check.as_str())
                    .and_then(|c| c.report.as_ref())
                    .map_or(aver_core::ReportFormat::Junit, |r| r.format);
                junit::parse_as(&run.check, &bytes, format)
                    .ok()
                    .and_then(|(_, mut locations)| locations.remove(test.as_str()))
            }
            None => None,
        }
        .unwrap_or_default();
        let location = diagnostics::location(
            attributes.file.as_deref(),
            attributes.line,
            message.as_deref(),
        );
        Ok((location, output))
    }

    /// The source of the first passing proof of each claimed criterion, from
    /// the head's test files (at most [`MAX_PROOF_SOURCES`] criteria).
    fn proof_sources(
        &self,
        plan: &Plan,
        body: &ReceiptBody,
    ) -> Result<Vec<diagnostics::ProofSource>, VerifyError> {
        let rule = body.schema.proof_rule();
        let mut wanted: Vec<(CriterionId, EntryId, String)> = Vec::new();
        for c in body.criteria.iter().take(MAX_PROOF_SOURCES) {
            let Some(e) = body
                .checks
                .iter()
                .find(|e| e.proves.get(c.id.as_str()).is_some() && rules::proves(e, rule))
            else {
                continue;
            };
            let Some((_, test)) = e.id.as_str().split_once("::") else {
                continue;
            };
            let name = test.rsplit(junit::SEPARATOR).next().unwrap_or(test);
            wanted.push((c.id.clone(), e.id.clone(), name.to_owned()));
        }
        let mut found = Vec::new();
        if wanted.is_empty() {
            return Ok(found);
        }
        for (path, file) in self.workspaces.snapshot_files(&plan.head)? {
            if !plan.env.is_test_file(&path)
                || self
                    .store
                    .sizes(&[file.blob])?
                    .first()
                    .copied()
                    .unwrap_or(0)
                    > MAX_TAG_SEARCH_BYTES as u64
            {
                continue;
            }
            let bytes = self.store.get_raw(&file.blob)?;
            let text = String::from_utf8_lossy(&bytes);
            wanted.retain(|(criterion, entry, name)| {
                let Some((line, source)) = diagnostics::test_source(&text, name) else {
                    return true;
                };
                found.push(diagnostics::ProofSource {
                    criterion: criterion.clone(),
                    entry: entry.clone(),
                    path: path.to_string(),
                    line,
                    source,
                });
                false
            });
            if wanted.is_empty() {
                break;
            }
        }
        let order = |id: &CriterionId| body.criteria.iter().position(|c| c.id == *id);
        found.sort_by_key(|p| order(&p.criterion));
        Ok(found)
    }

    /// The names the change defines or edits in its source files (not its
    /// test files), for telling a proof that fails on the original code
    /// only because one of them is missing there (REDESIGN.md §4.6).
    pub(super) fn changed_names(&self, plan: &Plan) -> Result<BTreeSet<String>, VerifyError> {
        let text = |entry: Option<FileEntry>| -> Result<Option<String>, VerifyError> {
            let Some(entry) = entry else {
                return Ok(None);
            };
            if self
                .store
                .sizes(&[entry.blob])?
                .first()
                .copied()
                .unwrap_or(0)
                > MAX_TAG_SEARCH_BYTES as u64
            {
                return Ok(None);
            }
            let bytes = self.store.get_raw(&entry.blob)?;
            Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
        };
        let mut names = BTreeSet::new();
        let changed = self.workspaces.changed_files(&plan.base, &plan.head)?;
        for (path, base, head) in changed.into_iter().take(MAX_NAME_FILES) {
            if plan.env.is_test_file(&path) {
                continue;
            }
            let (old, new) = (text(base)?, text(head)?);
            names.extend(crate::idents::changed(
                &path,
                old.as_deref(),
                new.as_deref(),
            ));
        }
        Ok(names)
    }

    /// Which of `criteria` have their `[crit:<id>]` tag in a test file of the
    /// head (by the base environment's test patterns).
    fn tags_in_head_tests(
        &self,
        plan: &Plan,
        criteria: &[&CriterionId],
    ) -> Result<std::collections::BTreeSet<CriterionId>, VerifyError> {
        let mut found = std::collections::BTreeSet::new();
        let all = aver_core::tag::Tags::new(plan.task.body.criteria.iter().map(|c| &c.id));
        let sizes_ok = |blob| -> Result<bool, VerifyError> {
            Ok(self.store.sizes(&[blob])?.first().copied().unwrap_or(0)
                <= MAX_TAG_SEARCH_BYTES as u64)
        };
        for (path, file) in self.workspaces.snapshot_files(&plan.head)? {
            if !plan.env.is_test_file(&path) || !sizes_ok(file.blob)? {
                continue;
            }
            let bytes = self.store.get_raw(&file.blob)?;
            let text = String::from_utf8_lossy(&bytes);
            for id in criteria {
                if all.carries(&text, id) {
                    found.insert((*id).clone());
                }
            }
            if found.len() == criteria.len() {
                break;
            }
        }
        Ok(found)
    }
}

/// One entry per changed file under a protected path, failing as `NEW`.
pub(super) fn protected_entries(
    task: &Task,
    changed: &[crate::workspace::ChangedFile],
) -> Vec<ReceiptCheck> {
    let found = changed.iter().filter_map(|(path, base, head)| {
        let glob = task.body.protects(path)?;
        let change = match (base, head) {
            (None, _) => "added",
            (_, None) => "deleted",
            _ => "modified",
        };
        Some((
            path.as_str(),
            format!("{change} protected path {path} (matches {glob})"),
        ))
    });
    path_entries(PROTECTED_CHECK, CheckSource::Task, Cause::New, found)
}

/// One entry per changed file under another task's live exclusive lease,
/// failing as `CHANGED` (§2.11): nothing broke, but a person decides
/// whether the edit may land beside the work that reserved the path.
pub(super) fn lease_entries(
    held: &[crate::coord::HeldLease],
    changed: &[crate::workspace::ChangedFile],
) -> Vec<ReceiptCheck> {
    let found = changed.iter().filter_map(|(path, base, head)| {
        let held = held.iter().find(|h| h.lease.covers(path.as_str()))?;
        let change = match (base, head) {
            (None, _) => "added",
            (_, None) => "deleted",
            _ => "modified",
        };
        let globs: Vec<&str> = held.lease.paths.iter().map(|g| g.as_str()).collect();
        let task = match (&held.title, held.lease.task) {
            (Some(title), _) => format!("task {title:?}"),
            (None, Some(task)) => format!("task {task}"),
            (None, None) => format!("workspace {}", held.lease.workspace),
        };
        Some((
            path.as_str(),
            format!(
                "{change} {path} under {task}'s exclusive lease of {} (lease {}, reason: {:?})",
                globs.join(", "),
                held.lease.id,
                held.lease.reason
            ),
        ))
    });
    path_entries(LEASE_CHECK, CheckSource::Task, Cause::Changed, found)
}

/// One entry per changed file under the base contract's sensitive paths,
/// failing as `CHANGED` (§1.13): nothing broke, but a person reviews the
/// edit before it lands.
pub(super) fn sensitive_entries(
    judge: &aver_core::JudgeGlobs,
    changed: &[crate::workspace::ChangedFile],
) -> Vec<ReceiptCheck> {
    let found = changed.iter().filter_map(|(path, base, head)| {
        let glob = judge.sensitive_match(path.as_str())?;
        let change = match (base, head) {
            (None, _) => "added",
            (_, None) => "deleted",
            _ => "modified",
        };
        Some((
            path.as_str(),
            format!("{change} sensitive path {path} (matches {glob})"),
        ))
    });
    path_entries(SENSITIVE_CHECK, CheckSource::Env, Cause::Changed, found)
}

pub(super) fn coord_error(e: crate::coord::CoordError) -> VerifyError {
    match e {
        crate::coord::CoordError::Workspace(e) => VerifyError::Workspace(e),
        crate::coord::CoordError::Store(e) => VerifyError::Store(e),
        other => VerifyError::Internal(other.to_string()),
    }
}

/// One entry per head change to frozen harness, failing as `CHANGED`: the
/// base's copy still judged every run, and the edit is for a human to
/// review (§1.9.3).
fn judging_entries(changes: &[JudgingChange]) -> Vec<ReceiptCheck> {
    let targets: Vec<(String, String)> = changes.iter().map(|j| (j.target(), j.detail())).collect();
    path_entries(
        JUDGING_CHECK,
        CheckSource::Env,
        Cause::Changed,
        targets.iter().map(|(t, d)| (t.as_str(), d.clone())),
    )
}

/// Failing entries `<check>::<path>` with cause `cause`, one per path. A
/// path longer than [`MAX_ENTRY_PATH_BYTES`] is cut and marked in the id;
/// entries whose cut ids collide merge, and the detail names every path.
fn path_entries<'a>(
    check: &str,
    source: CheckSource,
    cause: Cause,
    found: impl Iterator<Item = (&'a str, String)>,
) -> Vec<ReceiptCheck> {
    let check: CheckId = check.parse().expect("a valid check id");
    let mut by_id: std::collections::BTreeMap<EntryId, Vec<String>> = Default::default();
    for (path, detail) in found {
        by_id
            .entry(path_entry_id(&check, path))
            .or_default()
            .push(detail);
    }
    by_id
        .into_iter()
        .map(|(id, details)| ReceiptCheck {
            id,
            base: BaseStatus::Pass,
            head: HeadStatus::Fail,
            cause: Some(cause),
            proves: Set::empty(),
            source,
            runs: SideRuns {
                base: Vec::new(),
                head: Vec::new(),
                transplant: Vec::new(),
                head_tests: Vec::new(),
            },
            detail: Some(Text::truncated(&details.join("\n"))),
            transplant: None,
            head_tests: None,
        })
        .collect()
}

/// `<check>::<path>`, with a path longer than [`MAX_ENTRY_PATH_BYTES`] cut
/// and marked.
fn path_entry_id(check: &CheckId, path: &str) -> EntryId {
    let named = if path.len() > MAX_ENTRY_PATH_BYTES {
        let mut end = MAX_ENTRY_PATH_BYTES;
        while !path.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &path[..end])
    } else {
        path.to_owned()
    };
    TestCheckId::new(check, &named)
        .expect("a cut path fits a test check id")
        .into()
}

/// One `built::<path>` entry per built test file whose content differs
/// between a check's base runs and its judged head runs (REDESIGN.md §4.3).
/// Both sides compiled the same frozen test sources, so a difference is the
/// head's build rewriting what the tests are: judged by the table as
/// `BuiltArtifactModified`, `CHANGED` unless the check's head failed (`NEW`)
/// or a side lost its result (`ENV`). A file only one side built is left to
/// the tests it holds, and a side whose runner did not look compares
/// nothing. A file counts as changed only when no head run's content equals
/// any base run's, so a build that varies from run to run is not a change.
/// The first check to show a path names its entry.
fn built_entries(
    store: &Store,
    results: &[CheckRuns],
) -> Result<Vec<(ReceiptCheck, Evidence)>, StoreError> {
    let ids = |runs: &[ExecutedRun]| -> Vec<RunId> { runs.iter().map(|r| r.id).collect() };
    let side =
        |runs: &[ExecutedRun]| -> Result<Option<BTreeMap<String, BTreeSet<BlobId>>>, StoreError> {
            let mut files: Option<BTreeMap<String, BTreeSet<BlobId>>> = None;
            for r in runs.iter().filter(|r| !r.run.result.ending().is_infra()) {
                if let Some(built) = crate::exec::built::of(store, &r.id)? {
                    let files = files.get_or_insert_with(BTreeMap::new);
                    for (path, blob) in built {
                        files.entry(path).or_default().insert(blob);
                    }
                }
            }
            Ok(files)
        };
    let check_id: CheckId = BUILT_CHECK.parse().expect("a valid check id");
    let mut out: BTreeMap<EntryId, (ReceiptCheck, Evidence)> = BTreeMap::new();
    for check in results {
        let (Some(base), Some(head)) = (side(&check.base)?, side(&check.head)?) else {
            continue;
        };
        let def = &check.planned.def;
        let endings = |runs: &[ExecutedRun]| -> Vec<Ending> {
            runs.iter().map(|r| ending(&r.run, def)).collect()
        };
        let dims = judge::Entry {
            origin: Origin::BuiltArtifactModified,
            base: Base::of(&endings(&check.base)),
            judged: Judged::of(&endings(&check.head)),
            own: Own::NotRun,
            transplant: judge::Transplant::NotRun,
        };
        for (path, before) in &base {
            let Some(after) = head.get(path) else {
                continue;
            };
            let id = path_entry_id(&check_id, path);
            if !before.is_disjoint(after) || out.contains_key(&id) {
                continue;
            }
            let c = judge::judge(dims);
            let entry = ReceiptCheck {
                id: id.clone(),
                base: c.base,
                head: c.head,
                cause: c.cause,
                proves: Set::empty(),
                source: check.planned.source,
                runs: SideRuns {
                    base: ids(&check.base),
                    head: ids(&check.head),
                    transplant: Vec::new(),
                    head_tests: Vec::new(),
                },
                detail: Some(Text::truncated(&format!(
                    "{path}: check {} built this test file with other content on the judged head than on the base, from the same frozen test sources",
                    def.id
                ))),
                transplant: None,
                head_tests: None,
            };
            let evidence = Evidence {
                run: check.head.last().map(|r| r.id),
                note: Some(diagnostics::BUILT_NOTE.to_owned()),
                ..Evidence::default()
            };
            out.insert(id, (entry, evidence));
        }
    }
    Ok(out.into_values().collect())
}

/// The command that re-runs this verification without the cache.
pub fn reproduce(task: &TaskId, head: &SnapshotId, base: &SnapshotId) -> String {
    format!("aver verify --task {task} --head {head} --base {base} --fresh")
}

/// How a run ended for the cause rules: a check that declares a report and
/// exits 0 without a valid one has failed, since its result is unreadable.
pub(super) fn ending(run: &CheckRun, check: &CheckDef) -> Ending {
    match run.result.ending() {
        Ending::Pass if check.report.is_some() && run.report.is_none() => Ending::Fail,
        other => other,
    }
}

/// The non-infra runs of a side, if every one of them carries a report and
/// the side did not end in infra.
pub(super) fn reported(runs: &[ExecutedRun]) -> Option<Vec<&CheckRun>> {
    let ended_infra = runs
        .last()
        .is_some_and(|r| r.run.result.ending().is_infra());
    let results: Vec<&CheckRun> = runs
        .iter()
        .map(|r| &r.run)
        .filter(|r| !r.result.ending().is_infra())
        .collect();
    let complete =
        !ended_infra && !results.is_empty() && results.iter().all(|r| r.report.is_some());
    complete.then_some(results)
}

/// Whether the head side lost the per-test report every base run produced
/// (§1.9.3): every base run carries one, and the head's runs ended (not in
/// infra) with at least one result that carries none.
pub(super) fn lost_report(base: &[ExecutedRun], head: &[ExecutedRun]) -> bool {
    let ended_infra = head
        .last()
        .is_some_and(|r| r.run.result.ending().is_infra());
    reported(base).is_some()
        && !ended_infra
        && head
            .iter()
            .any(|r| !r.run.result.ending().is_infra() && r.run.report.is_none())
}

/// The receipt entries of one check, each with the run its detail came
/// from: one per test when both sides have reports (plus one per test only
/// the head's own versions of the test files report), and `<check>::(exit)`
/// when a head run's exit code or empty report says something its tests do
/// not; else one for the command. Each entry's cause is the table's
/// ([`judge::judge`]) over what the head did to the file that defines its
/// test and what each side showed. A proving entry of a check that ran on
/// the transplant records how it fared there and in the head-tests run.
fn check_entries(
    store: &Store,
    check: &CheckRuns,
    criteria: &Criteria,
    context: &EntryContext,
) -> Result<Vec<(ReceiptCheck, Evidence)>, StoreError> {
    let ids = |runs: &[ExecutedRun]| -> Vec<RunId> { runs.iter().map(|r| r.id).collect() };
    let def = &check.planned.def;
    let builds = Builds::new(store, check)?;
    // An entry's dimensions before its proof runs, where the file that
    // defines its test is known.
    let dims_of = |test: Option<&TestCheckId>, base: Base, judged: Judged| {
        let defines = context.defines(def, test);
        let e = judge::Entry {
            origin: context.origin(&defines),
            base,
            judged,
            own: own_of(&check.head_tests, test, def),
            transplant: judge::Transplant::NotRun,
        };
        (e, defines)
    };
    // What the diagnostics say about a cause that the head's own version
    // of the test decided (§1.9.3).
    let explain = |c: &Classification,
                   e: &judge::Entry,
                   defines: &Defines,
                   test: Option<&TestCheckId>,
                   evidence: &mut Evidence| {
        let own = head_tests_status(&check.head_tests, test, def);
        match c.cause {
            Some(Cause::Changed) => {
                evidence.own = Some(own);
                evidence.file = defines.file();
                evidence.note = Some(
                    match (e.judged, e.own) {
                        (Judged::Pass, _) => diagnostics::CHANGED_REMOVED_NOTE,
                        (_, Own::Skipped | Own::Absent) => diagnostics::CHANGED_DELETED_NOTE,
                        (Judged::Fail, _) if e.base == Base::Fail => {
                            diagnostics::CHANGED_FAILING_NOTE
                        }
                        _ => diagnostics::CHANGED_NOTE,
                    }
                    .to_owned(),
                );
            }
            Some(Cause::New) => {
                // Only a test file the head added or changed has a version
                // of the agent's own to report on.
                if defines.touched() {
                    evidence.own = Some(own);
                }
                if defines.changed() && own == Transplant::Absent {
                    evidence.file = defines.file();
                    evidence.note = Some(diagnostics::HIDDEN_REGRESSION_NOTE.to_owned());
                }
            }
            Some(Cause::Preexisting | Cause::Flaky | Cause::Env) | None => {}
        }
    };
    // Rule 7a, which the table does not hold: a test the base never ran,
    // failing in a test target the head declares and the base's frozen
    // manifest lacks (a merged newer commit that gives it
    // `required-features`), is not the change's failure as far as anyone
    // can tell: it ran without what its target declares. CHANGED, so the
    // verdict is at best REVIEW, with the harness entry beside it.
    let rule_7a = |c: Classification, base: Base, test: Option<&TestCheckId>| {
        if c.cause == Some(Cause::New) && base == Base::Absent && context.retargeted(test) {
            Classification {
                cause: Some(Cause::Changed),
                ..c
            }
        } else {
            c
        }
    };
    let retarget =
        |c: Classification, base: Base, test: Option<&TestCheckId>, evidence: &mut Evidence| {
            let moved = rule_7a(c, base, test);
            if moved != c {
                evidence.note = Some(diagnostics::RETARGETED_NOTE.to_owned());
            }
            moved
        };
    let entry = |id: EntryId,
                 dims: judge::Entry,
                 proves: Set<CriterionId>,
                 test: Option<&TestCheckId>,
                 detail: Option<String>,
                 head_runs: &[ExecutedRun],
                 evidence: &mut Evidence| {
        let proof = (!proves.is_empty() && !check.transplant.is_empty()).then(|| {
            let (t, build) = builds.proof(check, test);
            (t, head_tests_status(&check.head_tests, test, def), build)
        });
        let transplant = match &proof {
            Some((Transplant::Fail | Transplant::Absent, _, _)) => {
                judge::Transplant::FailOnAssertion
            }
            Some((Transplant::Pass, _, _)) => judge::Transplant::Pass,
            Some((Transplant::Infra, _, _)) => judge::Transplant::Infra,
            None => judge::Transplant::NotRun,
        };
        let without_proof = rule_7a(judge::judge(dims), dims.base, test);
        let c = rule_7a(
            judge::judge(judge::Entry { transplant, ..dims }),
            dims.base,
            test,
        );
        let mut detail = detail;
        match proof.as_ref().map(|p| &p.2) {
            Some(BuildProof::Transplant(errors)) => evidence.build = Some(errors.clone()),
            // A base that did not build ran no test, which the table reads
            // as ENV on its own; the head passing is what makes the
            // original's build the one reason nothing proves.
            Some(BuildProof::Base(first))
                if c.head == HeadStatus::Pass
                    && matches!(without_proof.cause, None | Some(Cause::Env)) =>
            {
                evidence.note = Some(diagnostics::original_build_note(def.id.as_str()));
                detail = detail.or_else(|| Some(first.clone()));
            }
            _ => {}
        }
        let proof = proof.map(|(t, h, _)| (t, h));
        ReceiptCheck {
            id,
            base: c.base,
            head: c.head,
            cause: c.cause,
            proves,
            source: check.planned.source,
            runs: SideRuns {
                base: ids(&check.base),
                head: ids(head_runs),
                transplant: match proof {
                    Some(_) => ids(&check.transplant),
                    None => Vec::new(),
                },
                head_tests: match proof {
                    Some(_) => ids(&check.head_tests),
                    None => Vec::new(),
                },
            },
            detail: detail
                .filter(|d| !d.is_empty())
                .map(|d| Text::truncated(tail(&d, MAX_DETAIL_BYTES))),
            transplant: proof.map(|p| p.0),
            head_tests: proof.map(|p| p.1),
        }
    };
    let run_id = |runs: &[ExecutedRun], run: &CheckRun| {
        runs.iter()
            .find(|r| std::ptr::eq(&r.run, run))
            .map(|r| r.id)
    };
    let failing = |c: &Classification| c.cause.is_some() || c.head != HeadStatus::Pass;
    // The first failure message of a test, preferring `first`, and the run
    // it came from.
    let message = |first: (&[&CheckRun], &[ExecutedRun]),
                   then: (&[&CheckRun], &[ExecutedRun]),
                   id: &TestCheckId| {
        let (runs, executed) = match first_message(first.0, id) {
            Some(_) => first,
            None => then,
        };
        let found = runs.iter().find(|r| {
            r.tests
                .get(id.as_str())
                .is_some_and(|t| t.message.is_some())
        });
        let detail = found
            .and_then(|r| r.tests.get(id.as_str()))
            .and_then(|t| t.message.as_ref())
            .map(|m| m.as_str().to_owned());
        (detail, found.and_then(|r| run_id(executed, r)))
    };
    // An entry that stands for tests the head took away or Aver did not
    // run: no test of its own, so no own version and no proof.
    let standing = |origin: Origin, base: Base, judged: Judged, own: Own| judge::Entry {
        origin,
        base,
        judged,
        own,
        transplant: judge::Transplant::NotRun,
    };

    if let (Some(base), Some(head)) = (reported(&check.base), reported(&check.head)) {
        let tests: BTreeSet<&TestCheckId> = base
            .iter()
            .chain(&head)
            .flat_map(|r| r.tests.iter())
            .filter(|t| t.status != TestStatus::Skip)
            .map(|t| &t.id)
            .collect();
        let mut out = Vec::with_capacity(tests.len());
        // The test targets the rerun over the targets that built reported.
        let rerun: BTreeSet<&str> = head
            .iter()
            .flat_map(|r| r.tests.iter())
            .filter_map(|t| t.id.test_path().split(junit::SEPARATOR).next())
            .collect();
        let mut not_rerun: BTreeMap<&str, usize> = BTreeMap::new();
        let mut deleted: BTreeMap<String, usize> = BTreeMap::new();
        for id in &tests {
            let id = *id;
            let (b, h) = (test_outcomes(&base, id), test_outcomes(&head, id));
            if let (true, Some(rebuilt), Some(target)) = (
                h.is_empty(),
                check.rebuilt(),
                id.test_path().split(junit::SEPARATOR).next(),
            ) {
                // A test of a target that did not build is judged by the
                // entry of the restored file that stopped it, below.
                if rebuilt.unbuilt.targets.contains(target) {
                    continue;
                }
                // Nor is one of a target that built and the rerun left out
                // missing: Aver did not run it.
                if !rerun.contains(target) {
                    *not_rerun.entry(target).or_default() += 1;
                    continue;
                }
            }
            // A base test inside a source file the head deleted went with
            // its code: the file's entry, below, stands for it.
            if h.is_empty() && !b.is_empty() {
                if let Some(path) = context.deleted_source(id) {
                    *deleted.entry(path).or_default() += 1;
                    continue;
                }
            }
            let mut evidence = Evidence {
                test: Some(id.clone()),
                ..Evidence::default()
            };
            // A test module the base could not import ran none of its
            // tests there: the base gave no evidence about them, so its
            // failure explains nothing on the head.
            let uncollected =
                collection_failed(&base, id) && context.uncollected_related(store, def, id)?;
            let base_side = match Base::of(&b) {
                Base::Fail | Base::Flaky if uncollected => Base::ZeroTests,
                side => side,
            };
            let (dims, defines) = dims_of(Some(id), base_side, Judged::of(&h));
            let c = judge::judge(dims);
            if c.cause == Some(Cause::New) && c.head == HeadStatus::Flaky {
                evidence.note = Some(diagnostics::FLAKY_HEAD_NOTE.to_owned());
            }
            if uncollected && c.cause == Some(Cause::Env) {
                evidence.note = Some(diagnostics::UNCOLLECTED_NOTE.to_owned());
            }
            explain(&c, &dims, &defines, Some(id), &mut evidence);
            let c = retarget(c, dims.base, Some(id), &mut evidence);
            let detail = if !failing(&c) {
                None
            } else if h.is_empty() {
                let skipped = head.iter().any(|r| r.tests.get(id.as_str()).is_some());
                Some(
                    if skipped {
                        SKIPPED_ON_HEAD
                    } else {
                        MISSING_FROM_HEAD
                    }
                    .to_owned(),
                )
            } else {
                let (detail, run) = message((&head, &check.head), (&base, &check.base), id);
                evidence.run = run;
                detail
            };
            if c.cause == Some(Cause::Preexisting)
                && detail.as_deref().is_some_and(diagnostics::needs_network)
            {
                evidence.note = Some(diagnostics::NETWORK_NOTE.to_owned());
            }
            let proves = proves(&check.planned.proves, id.test_path(), criteria);
            let entry = entry(
                id.clone().into(),
                dims,
                proves,
                Some(id),
                detail,
                &check.head,
                &mut evidence,
            );
            out.push((entry, evidence));
        }
        if let Some(own) = reported(&check.head_tests).filter(|_| check.separate) {
            let only_own: BTreeSet<&TestCheckId> = own
                .iter()
                .flat_map(|r| r.tests.iter())
                .filter(|t| t.status != TestStatus::Skip && !tests.contains(&t.id))
                .map(|t| &t.id)
                .collect();
            for id in only_own {
                let (dims, _) = dims_of(
                    Some(id),
                    Base::of(&test_outcomes(&base, id)),
                    Judged::NotApplicable,
                );
                let mut evidence = Evidence {
                    test: Some(id.clone()),
                    ..Evidence::default()
                };
                let c = retarget(judge::judge(dims), dims.base, Some(id), &mut evidence);
                let detail = if failing(&c) {
                    let (detail, run) =
                        message((&own, &check.head_tests), (&base, &check.base), id);
                    evidence.run = run;
                    detail
                } else {
                    None
                };
                let proves = proves(&check.planned.proves, id.test_path(), criteria);
                let entry = entry(
                    id.clone().into(),
                    dims,
                    proves,
                    Some(id),
                    detail,
                    &check.head_tests,
                    &mut evidence,
                );
                out.push((entry, evidence));
            }
        }
        let base_side = Base::of(
            &check
                .base
                .iter()
                .map(|r| ending(&r.run, def))
                .collect::<Vec<_>>(),
        );
        for (path, count) in &deleted {
            let Ok(id) = TestCheckId::new(&def.id, &format!("{DELETED_SOURCE_TEST_PATH} {path}"))
            else {
                continue;
            };
            let detail = format!(
                "{path}: the change deleted this source file, and {count} test{} the base ran inside it went with it, so {} not run here.",
                if *count == 1 { "" } else { "s" },
                if *count == 1 { "it was" } else { "they were" },
            );
            let mut evidence = Evidence {
                file: path.parse().ok(),
                note: Some(diagnostics::DELETED_SOURCE_NOTE.to_owned()),
                ..Evidence::default()
            };
            let entry = entry(
                id.into(),
                standing(
                    Origin::DeletedSourceWithInlineTests,
                    base_side,
                    Judged::NotApplicable,
                    Own::Absent,
                ),
                Set::empty(),
                None,
                Some(detail),
                &check.head,
                &mut evidence,
            );
            out.push((entry, evidence));
        }
        if let Some(rebuilt) = check.rebuilt() {
            for (path, file) in &rebuilt.unbuilt.files {
                let Ok(id) = TestCheckId::new(&def.id, &format!("{UNBUILT_TEST_PATH} {path}"))
                else {
                    continue;
                };
                let skipped = tests
                    .iter()
                    .filter(|t| {
                        t.test_path()
                            .split(junit::SEPARATOR)
                            .next()
                            .is_some_and(|target| file.targets.contains(target))
                    })
                    .count();
                let detail = format!(
                    "{path}: its base version does not compile against the new code, so {} test target{} did not build and {skipped} test{} the base ran did not run here. First error:\n{}",
                    file.targets.len(),
                    if file.targets.len() == 1 { "" } else { "s" },
                    if skipped == 1 { "" } else { "s" },
                    file.error,
                );
                let mut evidence = Evidence {
                    run: Some(rebuilt.probe.id),
                    file: path.parse().ok(),
                    note: Some(diagnostics::UNBUILT_TEST_NOTE.to_owned()),
                    ..Evidence::default()
                };
                // The probe found only restored files stopping the build, so
                // the head's own versions built: they ran as its head-tests.
                let (origin, own) = if context.deleted.contains(path) {
                    (Origin::DeletedTestFile, Own::Absent)
                } else {
                    (
                        Origin::ModifiedTestFile,
                        own_of(&check.head_tests, None, def),
                    )
                };
                let entry = entry(
                    id.into(),
                    standing(origin, base_side, Judged::CompileError, own),
                    Set::empty(),
                    None,
                    Some(detail),
                    std::slice::from_ref(&rebuilt.probe),
                    &mut evidence,
                );
                out.push((entry, evidence));
            }
            for (target, count) in &not_rerun {
                let Ok(id) = TestCheckId::new(&def.id, &format!("{NOT_RERUN_TEST_PATH} {target}"))
                else {
                    continue;
                };
                let detail = format!(
                    "{target}: this test target built on the judged head, but Aver did not run it again, so {count} test{} the base ran {} not judged. Aver lost the target, not the change.",
                    if *count == 1 { "" } else { "s" },
                    if *count == 1 { "is" } else { "are" },
                );
                let mut evidence = Evidence {
                    run: Some(rebuilt.probe.id),
                    note: Some(diagnostics::NOT_RERUN_NOTE.to_owned()),
                    ..Evidence::default()
                };
                let entry = entry(
                    id.into(),
                    standing(
                        Origin::UntouchedSource,
                        base_side,
                        Judged::Infra,
                        Own::NotRun,
                    ),
                    Set::empty(),
                    None,
                    Some(detail),
                    std::slice::from_ref(&rebuilt.probe),
                    &mut evidence,
                );
                out.push((entry, evidence));
            }
        }
        let base_tests = base.iter().map(|r| r.tests.len()).max().unwrap_or(0);
        // Tests of targets that did not build are judged by their file's
        // entry above, so a rerun that has none of them left reports none.
        let expected = match check.rebuilt() {
            Some(rebuilt) => base
                .iter()
                .map(|r| {
                    r.tests
                        .iter()
                        .filter(|t| {
                            t.id.test_path()
                                .split(junit::SEPARATOR)
                                .next()
                                .is_none_or(|target| !rebuilt.unbuilt.targets.contains(target))
                        })
                        .count()
                })
                .max()
                .unwrap_or(0),
            None => base_tests,
        };
        let head_exit = exit_endings(&head, expected);
        if head_exit.contains(&Ending::Fail) {
            // A base whose reports hold no test explains no head failure.
            let exit_base = match base_tests {
                0 => Base::ZeroTests,
                _ => Base::of(&exit_endings(&base, 0)),
            };
            let dims = standing(
                Origin::UntouchedSource,
                exit_base,
                Judged::of(&head_exit),
                Own::NotRun,
            );
            let id = TestCheckId::new(&def.id, EXIT_TEST_PATH).expect("the exit entry id is valid");
            let (detail, bad) = exit_detail(store, &check.head, &head, expected)?;
            let mut evidence = Evidence {
                run: Some(bad),
                ..Evidence::default()
            };
            if judge::judge(dims).cause == Some(Cause::Env) {
                evidence.note = Some(diagnostics::NO_TESTS_NOTE.to_owned());
            }
            let entry = entry(
                id.into(),
                dims,
                Set::empty(),
                None,
                Some(detail),
                &check.head,
                &mut evidence,
            );
            out.push((entry, evidence));
        }
        // The exit entry above already blames a base that ran no test.
        let base_judged = base_tests == 0 && head_exit.contains(&Ending::Fail);
        out.extend(blocker_entries(store, check, context, true, base_judged)?);
        return Ok(out);
    }

    let endings = |runs: &[ExecutedRun]| -> Vec<Ending> {
        runs.iter().map(|r| ending(&r.run, def)).collect()
    };
    let mut evidence = Evidence::default();
    let lost_probe = match &check.rebuild {
        Some(Rebuild::Lost(probe)) => Some(probe),
        _ => None,
    };
    let lost = lost_report(&check.base, &check.head);
    let (ran_none, refusal) = base_ran_no_tests(store, check)?;
    let judged = match (lost_probe, lost) {
        (Some(_), _) => Judged::Infra,
        (None, true) => Judged::Missing,
        (None, false) => Judged::of(&endings(&check.head)),
    };
    let base_side = match ran_none {
        true => Base::ZeroTests,
        false => Base::of(&endings(&check.base)),
    };
    let (dims, defines) = dims_of(None, base_side, judged);
    let c = judge::judge(dims);
    if let Some(probe) = lost_probe {
        evidence.note = Some(diagnostics::LOST_PROBE_NOTE.to_owned());
        evidence.run = Some(probe.id);
    } else if lost {
        evidence.note = Some(diagnostics::LOST_REPORT_NOTE.to_owned());
    }
    explain(&c, &dims, &defines, None, &mut evidence);
    if c.cause == Some(Cause::Env) && ran_none {
        evidence.note = Some(match &refusal {
            Some(t) => diagnostics::toolchain_note(t.fix),
            None => diagnostics::NO_TESTS_NOTE.to_owned(),
        });
    }
    let detail = if let Some(t) = refusal.filter(|_| c.cause == Some(Cause::Env) && ran_none) {
        evidence.run = check
            .base
            .iter()
            .rev()
            .find(|r| !r.run.result.ending().is_infra())
            .map(|r| r.id);
        Some(t.to_string())
    } else if let Some(probe) = lost_probe {
        let output = String::from_utf8_lossy(&store.get_raw(&probe.run.output)?).into_owned();
        Some(format!(
            "Aver lost the result of its build probe ({}), so it cannot tell whether restored test files or the change stopped the head's build. Its output:\n{output}",
            if probe.run.result.ending().is_infra() {
                "the run ended in infra"
            } else if probe.run.report.is_none() {
                "it wrote no stream"
            } else {
                "its stream stops before cargo's build-finished message"
            }
        ))
    } else if failing(&c) {
        let last_bad = |runs: &[ExecutedRun]| {
            runs.iter()
                .rev()
                .find(|r| ending(&r.run, def) != Ending::Pass)
                .map(|r| (r.id, r.run.output))
        };
        match last_bad(&check.head).or_else(|| last_bad(&check.base)) {
            Some((run, blob)) => {
                evidence.run = Some(run);
                Some(String::from_utf8_lossy(&store.get_raw(&blob)?).into_owned())
            }
            None => None,
        }
    } else {
        None
    };
    // Over a base that does not build, a check reports no test there, so
    // its tests tagged with this task's criteria are judged by this entry,
    // which proves nothing (§1.9.3): the original code cannot show that
    // the change is needed.
    let mut proves_here = check.planned.proves.clone();
    if builds.base.is_some() && reported(&check.base).is_none() {
        let tagged: Vec<CriterionId> = [&check.head, &check.head_tests]
            .iter()
            .flat_map(|runs| runs.iter())
            .flat_map(|r| r.run.tests.iter())
            .flat_map(|t| {
                proves(&Set::empty(), t.id.test_path(), criteria)
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect();
        proves_here = Set::new(
            proves_here
                .iter()
                .cloned()
                .chain(tagged)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        )
        .expect("criterion ids are their keys");
    }
    let entry = entry(
        def.id.clone().into(),
        dims,
        proves_here,
        None,
        detail,
        &check.head,
        &mut evidence,
    );
    let mut out = vec![(entry, evidence)];
    // Without a report on both sides there is nothing to count: the
    // command's entry judges a side that wrote none.
    let base_judged = ran_none && c.cause == Some(Cause::Env);
    out.extend(blocker_entries(store, check, context, false, base_judged)?);
    Ok(out)
}

/// Whether every base run of a check ended before its first test result
/// (§1.9.3), and the toolchain refusal the base's output names, if any:
/// the base failed every run that completed, and either its output names a
/// toolchain that refused to start the tests ([`toolchain::classify`]) or
/// the check declares a report and no base run wrote one with a test in
/// it. Such a base explains no failure of the change.
fn base_ran_no_tests(
    store: &Store,
    check: &CheckRuns,
) -> Result<(bool, Option<toolchain::Toolchain>), StoreError> {
    let def = &check.planned.def;
    let runs: Vec<&CheckRun> = check
        .base
        .iter()
        .map(|r| &r.run)
        .filter(|r| !r.result.ending().is_infra())
        .collect();
    if runs.is_empty() || runs.iter().any(|r| ending(r, def) == Ending::Pass) {
        return Ok((false, None));
    }
    let mut refusal = None;
    // A missing command in the output of a run that reported tests is a
    // test's own output, not a runner that never started.
    let reported_tests = runs.iter().any(|r| !r.tests.is_empty());
    for r in runs.iter().rev() {
        refusal = toolchain::classify(&String::from_utf8_lossy(&store.get_raw(&r.output)?))
            .filter(|t| !(reported_tests && t.fix == toolchain::MISSING_COMMAND_FIX));
        if refusal.is_some() {
            break;
        }
    }
    let unreported = def.report.is_some()
        && runs
            .iter()
            .all(|r| r.report.is_none() || r.tests.is_empty());
    Ok((refusal.is_some() || unreported, refusal))
}

/// One entry per finding of a check's failing setup commands
/// ([`crate::exec::findings`]), `<check>::(setup) <file>: <message>`, when
/// every base and head run's output can be read whole: a finding a run does
/// not print passed there, so a finding the change adds is `NEW` even when
/// the base's type check or `go vet` already fails (and its `(exit)` entry
/// is `PREEXISTING`).
fn setup_entries(
    store: &Store,
    check: &CheckRuns,
    budget: usize,
) -> Result<Vec<(ReceiptCheck, Evidence)>, StoreError> {
    let def = &check.planned.def;
    if def.setup.is_empty() {
        return Ok(Vec::new());
    }
    let side = |runs: &[ExecutedRun]| -> Result<Option<Vec<RunFindings>>, StoreError> {
        let mut out = Vec::new();
        for r in runs.iter().filter(|r| !r.run.result.ending().is_infra()) {
            let output = String::from_utf8_lossy(&store.get_raw(&r.run.output)?).into_owned();
            match crate::exec::findings::setup_findings(&output, budget) {
                Some(found) => out.push((r.id, found)),
                None => return Ok(None),
            }
        }
        Ok((!out.is_empty()).then_some(out))
    };
    let (Some(base), Some(head)) = (side(&check.base)?, side(&check.head)?) else {
        return Ok(Vec::new());
    };
    let keys: BTreeSet<&String> = base
        .iter()
        .chain(&head)
        .flat_map(|(_, f)| f.iter())
        .collect();
    let outcomes = |runs: &[RunFindings], key: &String| -> Vec<Ending> {
        runs.iter()
            .map(|(_, f)| {
                if f.contains(key) {
                    Ending::Fail
                } else {
                    Ending::Pass
                }
            })
            .collect()
    };
    let mut out = Vec::new();
    for key in keys {
        let Ok(id) = TestCheckId::new(&def.id, &format!("{SETUP_TEST_PATH} {key}")) else {
            continue;
        };
        // A test the runner itself reported under that name keeps it.
        let reported = check
            .base
            .iter()
            .chain(&check.head)
            .chain(&check.head_tests)
            .any(|r| r.run.tests.get(id.as_str()).is_some());
        if reported {
            continue;
        }
        let c = judge::judge(judge::Entry {
            origin: Origin::UntouchedSource,
            base: Base::of(&outcomes(&base, key)),
            judged: Judged::of(&outcomes(&head, key)),
            own: Own::NotRun,
            transplant: judge::Transplant::NotRun,
        });
        let failed_in = head
            .iter()
            .chain(&base)
            .find(|(_, f)| f.contains(key))
            .map(|(run, _)| *run);
        out.push((
            ReceiptCheck {
                id: id.into(),
                base: c.base,
                head: c.head,
                cause: c.cause,
                proves: Set::empty(),
                source: check.planned.source,
                runs: SideRuns {
                    base: base.iter().map(|(r, _)| *r).collect(),
                    head: head.iter().map(|(r, _)| *r).collect(),
                    transplant: Vec::new(),
                    head_tests: Vec::new(),
                },
                detail: (c.cause.is_some() || c.head != HeadStatus::Pass)
                    .then(|| Text::truncated(tail(key, MAX_DETAIL_BYTES))),
                transplant: None,
                head_tests: None,
            },
            Evidence {
                run: failed_in,
                ..Evidence::default()
            },
        ));
    }
    Ok(out)
}

/// The test path prefix of a blocker's entry (REDESIGN.md §4.1): what
/// keeps a check's evidence from counting.
pub const EVIDENCE_TEST_PATH: &str = "(evidence)";

/// One `ENV` entry per blocker of a check ([`evidence::decide_check`]), so
/// zero evidence is never `PASS`: a side that ran no test or whose runner
/// stopped on its own error, a base below the check's `min_tests` or with
/// no passing test, a Go package cut off by a panic, a failed setup
/// command, an exit-only check no person approved. Counts are read only
/// when both sides reported (`counted`) and the format lists every test,
/// so a type check's errors are never held to a floor. The base's
/// blockers are left out when the check's own entry already blames a base
/// that ran no test (`base_judged`), and a side that lost its result is
/// already `ENV`.
fn blocker_entries(
    store: &Store,
    check: &CheckRuns,
    context: &EntryContext,
    counted: bool,
    base_judged: bool,
) -> Result<Vec<(ReceiptCheck, Evidence)>, StoreError> {
    let def = &check.planned.def;
    let lost = |runs: &[ExecutedRun]| runs.last().is_none_or(|r| r.run.result.ending().is_infra());
    if lost(&check.base) || lost(&check.head) {
        return Ok(Vec::new());
    }
    let base = side_evidence(store, def, &check.base, context)?;
    // Over restored test files that did not build, the head's evidence is
    // its own versions of them, which the head-tests run built and ran.
    let head_runs = match check.rebuilt() {
        Some(_) if check.separate => &check.head_tests,
        _ => &check.head,
    };
    let head = side_evidence(store, def, head_runs, context)?;
    let counting = match def.report.as_ref().map(|r| r.format) {
        None => evidence::Counting::Exit {
            approved: def.approved,
        },
        Some(format) if counted && !format.lists_failures_only() => evidence::Counting::Tests {
            min_tests: def.min_tests.map_or(1, |n| n.get()),
        },
        Some(_) => evidence::Counting::Uncounted,
    };
    let seen = match (base.count.ran(), head.count.ran()) {
        (0, 0) => "the base and your version both ran 0 tests".to_owned(),
        (b, h) => format!(
            "the base ran {b} test{} ({} passed, {} failed) and your version ran {h}",
            if b == 1 { "" } else { "s" },
            base.count.passed,
            base.count.failed,
        ),
    };
    let mut out = Vec::new();
    for b in evidence::decide_check(&def.id, counting, &base, &head)
        .iter()
        .filter(|b| !(base_judged && blocked_side(b) == Some(evidence::Side::Base)))
    {
        let counts = matches!(
            b,
            Blocker::NoTests { .. } | Blocker::AllRed { .. } | Blocker::BelowFloor { .. }
        );
        let Some(mut entry) = blocker_entry(check, b, counts.then_some(seen.as_str())) else {
            continue;
        };
        if let (Blocker::SetupFailed { .. }, Some(run)) = (b, entry.1.run) {
            missing_setup_command(store, check, run, &mut entry.0)?;
        }
        out.push(entry);
    }
    Ok(out)
}

/// Adds the line of a setup command a shell could not find to a
/// setup-failed entry's detail: what to install, not only that setup
/// failed.
fn missing_setup_command(
    store: &Store,
    check: &CheckRuns,
    run: RunId,
    entry: &mut ReceiptCheck,
) -> Result<(), StoreError> {
    let Some(r) = check.base.iter().chain(&check.head).find(|r| r.id == run) else {
        return Ok(());
    };
    let output = store.get_raw(&r.run.output)?;
    if let Some(t) = toolchain::missing_command(&String::from_utf8_lossy(&output)) {
        let detail = entry.detail.as_ref().map_or("", |d| d.as_str());
        let detail = format!("{detail}\n{t}");
        entry.detail = Some(Text::truncated(tail(&detail, MAX_DETAIL_BYTES)));
    }
    Ok(())
}

/// Whether a check that counts tests ran at least one on the base: a type
/// check lists errors, and an exit code names no test, so neither ran the
/// code (§1.9.3). With no check run at all (every check skipped), nothing
/// was asked of the runs, and this does not apply.
fn ran_base_tests(
    store: &Store,
    results: &[CheckRuns],
    context: &EntryContext,
) -> Result<bool, StoreError> {
    for check in results {
        let def = &check.planned.def;
        // An approved exit-code check runs the repository's tests even if
        // Aver cannot count them; it is a limit the report names, not none.
        let ran = check
            .base
            .last()
            .is_some_and(|r| !r.run.result.ending().is_infra());
        if def.report.is_none() && def.approved && ran {
            return Ok(true);
        }
        let counts = def
            .report
            .as_ref()
            .is_some_and(|r| !r.format.lists_failures_only());
        if counts && side_evidence(store, def, &check.base, context)?.count.ran() > 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

/// What a side's completed runs showed: each test counted once at its best
/// result (a test that passed in any run passed), and the faults of its
/// last run.
fn side_evidence(
    store: &Store,
    def: &CheckDef,
    runs: &[ExecutedRun],
    context: &EntryContext,
) -> Result<SideEvidence, StoreError> {
    let done: Vec<&CheckRun> = runs
        .iter()
        .map(|r| &r.run)
        .filter(|r| !r.result.ending().is_infra())
        .collect();
    let rank = |s: TestStatus| match s {
        TestStatus::Pass => 2,
        TestStatus::Fail => 1,
        TestStatus::Skip => 0,
    };
    // A go-json package failure is an entry, not a test.
    let package_entry = format!(
        "{}{}",
        junit::SEPARATOR,
        crate::exec::gojson::PACKAGE_FAILED
    );
    let mut best: BTreeMap<&str, TestStatus> = BTreeMap::new();
    for t in done
        .iter()
        .flat_map(|r| r.tests.iter())
        .filter(|t| !t.id.test_path().ends_with(&package_entry))
    {
        let s = best.entry(t.id.as_str()).or_insert(t.status);
        if rank(t.status) > rank(*s) {
            *s = t.status;
        }
    }
    let mut count = evidence::SideCount::default();
    for s in best.values() {
        match s {
            TestStatus::Pass => count.passed += 1,
            TestStatus::Fail => count.failed += 1,
            TestStatus::Skip => count.skipped += 1,
        }
    }
    let mut faults = Vec::new();
    if let Some(last) = done.last() {
        let setup_failed = !def.setup.is_empty()
            && crate::exec::findings::setup_failed(&String::from_utf8_lossy(
                &store.get_raw(&last.output)?,
            ));
        if setup_failed {
            faults.push(SideFault::PrepareFailed);
        } else if let Some(exit_code) = last
            .result
            .exit_code()
            .filter(|c| runs_pytest(def) && evidence::pytest_runner_error(*c))
        {
            faults.push(SideFault::RunnerError { exit_code });
        }
        let go = def.report.as_ref().map(|r| r.format) == Some(aver_core::ReportFormat::GoJson);
        if let (true, Some(report)) = (go, last.report) {
            let reported: BTreeSet<&str> = last.tests.iter().map(|t| t.id.test_path()).collect();
            for package in crate::exec::gojson::failed_packages(&store.get_raw(&report)?) {
                let prefix = format!("{package}{}", junit::SEPARATOR);
                // A package that reported no test never started its test
                // binary (a build failure): nothing cut it off.
                let started = reported.iter().any(|p| {
                    p.strip_prefix(&prefix)
                        .is_some_and(|t| t != crate::exec::gojson::PACKAGE_FAILED)
                });
                let declared = context.go_tests(store, &package, def)?;
                let missing =
                    evidence::truncated(started, declared.iter().map(String::as_str), &|n| {
                        reported.contains(format!("{prefix}{n}").as_str())
                    });
                if let Some(missing) = missing {
                    faults.push(SideFault::Truncated {
                        target: Text::truncated(&package),
                        missing,
                    });
                }
            }
        }
    }
    Ok(SideEvidence { count, faults })
}

/// Whether a check runs pytest, whose exits 2 to 4 are its own errors.
fn runs_pytest(def: &CheckDef) -> bool {
    def.report.as_ref().map(|r| r.format) == Some(aver_core::ReportFormat::PytestJunit)
        || def.argv.iter().any(|a| {
            let name = a.rsplit('/').next().unwrap_or(a);
            name == "pytest" || name == "py.test"
        })
}

/// The `ENV` entry `<check>::(evidence) <reason>` of a blocker, whose
/// detail says what was `seen` and the blocker's sentence. The blocked side
/// is read as giving no evidence (the base) or losing its result (the
/// judged head; both for a place blocker), so the judge table makes it
/// `ENV` and the verdict `BLOCKED_NOT_OWN`.
fn blocker_entry(
    check: &CheckRuns,
    blocker: &Blocker,
    seen: Option<&str>,
) -> Option<(ReceiptCheck, Evidence)> {
    use evidence::Side;
    let def = &check.planned.def;
    let side = blocked_side(blocker);
    let on = match side {
        Some(Side::Base) => "the base",
        _ => "your version",
    };
    let reason = match blocker {
        Blocker::NoTests { .. } => format!("no tests on {on}"),
        Blocker::BelowFloor { .. } => format!("below the floor on {on}"),
        Blocker::AllRed { .. } => "no test passed on the base".to_owned(),
        Blocker::UnapprovedExit { .. } => "exit code only".to_owned(),
        Blocker::RunnerError { .. } => format!("runner error on {on}"),
        Blocker::Truncated { target, .. } => format!("{target} cut off on {on}"),
        Blocker::SetupFailed { .. } => format!("setup failed on {on}"),
        Blocker::StaleCopy { keep, .. } => format!("stale copy {keep}"),
        Blocker::DirtyCheckout { .. } => "dirty checkout".to_owned(),
    };
    let id = TestCheckId::new(&def.id, &format!("{EVIDENCE_TEST_PATH} {reason}")).ok()?;
    let endings = |runs: &[ExecutedRun]| -> Vec<Ending> {
        runs.iter().map(|r| ending(&r.run, def)).collect()
    };
    let no_evidence = if seen.is_some() {
        Base::ZeroTests
    } else {
        Base::Infra
    };
    let (base, judged) = match side {
        Some(Side::Base) => (no_evidence, Judged::of(&endings(&check.head))),
        Some(Side::JudgedHead | Side::HeadTests | Side::Transplant) => {
            (Base::of(&endings(&check.base)), Judged::Infra)
        }
        None => (Base::Infra, Judged::Infra),
    };
    let c = judge::judge(judge::Entry {
        origin: Origin::UntouchedSource,
        base,
        judged,
        own: Own::NotRun,
        transplant: judge::Transplant::NotRun,
    });
    debug_assert_eq!(c.cause, Some(Cause::Env), "{blocker:?}");
    let sentence = capitalized(&blocker.sentence().to_line());
    let detail = match seen {
        Some(seen) => format!("{}. {sentence}", capitalized(seen)),
        None => sentence,
    };
    let run = |runs: &[ExecutedRun]| {
        runs.iter()
            .rev()
            .find(|r| !r.run.result.ending().is_infra())
            .map(|r| r.id)
    };
    let blamed = match side {
        Some(Side::Base) => &check.base,
        _ => &check.head,
    };
    let ids = |runs: &[ExecutedRun]| -> Vec<RunId> { runs.iter().map(|r| r.id).collect() };
    Some((
        ReceiptCheck {
            id: id.into(),
            base: c.base,
            head: c.head,
            cause: c.cause,
            proves: Set::empty(),
            source: check.planned.source,
            runs: SideRuns {
                base: ids(&check.base),
                head: ids(&check.head),
                transplant: Vec::new(),
                head_tests: Vec::new(),
            },
            detail: Some(Text::truncated(tail(&detail, MAX_DETAIL_BYTES))),
            transplant: None,
            head_tests: None,
        },
        Evidence {
            run: run(blamed),
            ..Evidence::default()
        },
    ))
}

/// The side a blocker is about; `None` for a place blocker.
fn blocked_side(blocker: &Blocker) -> Option<evidence::Side> {
    match blocker {
        Blocker::NoTests { side, .. }
        | Blocker::RunnerError { side, .. }
        | Blocker::Truncated { side, .. }
        | Blocker::SetupFailed { side, .. } => Some(*side),
        Blocker::BelowFloor { .. } | Blocker::AllRed { .. } | Blocker::UnapprovedExit { .. } => {
            Some(evidence::Side::Base)
        }
        Blocker::StaleCopy { .. } | Blocker::DirtyCheckout { .. } => None,
    }
}

fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A run and the findings of its failing setup commands.
pub(super) type RunFindings = (RunId, Vec<String>);

/// The test path prefix of a setup finding's entry.
pub const SETUP_TEST_PATH: &str = "(setup)";

/// The test path prefix of the entry of a restored test file that does not
/// compile on the judged head.
pub const UNBUILT_TEST_PATH: &str = "(did not compile)";

/// The test path prefix of the entry of a test target that built on the
/// judged head but that the rerun over the built targets did not report
/// (Aver's loss: `ENV`).
pub const NOT_RERUN_TEST_PATH: &str = "(not run again)";

/// The test path prefix of the entry of a Rust source file the head
/// deleted with the tests inside it.
pub const DELETED_SOURCE_TEST_PATH: &str = "(deleted with its code)";

/// What judging an entry needs to know about the head (§1.9.3): which test
/// files it added, changed and deleted (an entry's [`Origin`]), and which
/// dependencies with install scripts its lockfile adds.
pub(super) struct EntryContext {
    env: Arc<Environment>,
    /// Existing test files the head modified or deleted.
    changed: BTreeSet<String>,
    /// Existing test files the head deleted.
    deleted: BTreeSet<String>,
    /// Test files the head added or modified.
    touched: BTreeSet<String>,
    retargeted: Vec<RetargetedTest>,
    /// Rust source files the head deleted with the tests inside them.
    deleted_sources: BTreeSet<String>,
    /// The base tree, whose Go test files declare the tests a package that
    /// ended early should have reported.
    base_root: aver_core::TreeId,
    head_root: aver_core::TreeId,
    /// The base's files and the paths the head changed, read only when a
    /// test module failed to collect.
    trees: std::sync::OnceLock<(BTreeMap<String, FileEntry>, BTreeSet<String>)>,
}

/// The files, relative to a package, that may define the libtest entry at
/// `path` of `target`, deepest first. libtest names its target relative to
/// the package (`tests/api.rs`, `src/lib.rs`); a test in a module
/// (`verify::tests::adds`) may be in each of the module's files, and a
/// doc-test is named by its source file (`src/map.rs - map::Map`).
fn rust_candidates(target: &str, path: &str) -> Option<Vec<String>> {
    let name = path.split(junit::SEPARATOR).nth(1);
    if target == "doc-tests" {
        return Some(vec![name?.split_once(" - ")?.0.to_owned()]);
    }
    if !target.ends_with(".rs") {
        return None;
    }
    let modules: Vec<&str> = name
        .map(|name| name.split("::").collect::<Vec<_>>())
        .unwrap_or_default();
    // A target is a crate root, whose `mod` declarations resolve in its
    // own directory whatever its name (`tests/tests.rs` declares
    // `tests/misc.rs`).
    let mut relative = Vec::new();
    let mut dir = target.rsplit_once('/').map_or("", |(d, _)| d).to_owned();
    for module in modules.iter().take(modules.len().saturating_sub(1)) {
        let at = |file: String| match dir.as_str() {
            "" => file,
            d => format!("{d}/{file}"),
        };
        relative.push(at(format!("{module}.rs")));
        relative.push(at(format!("{module}/mod.rs")));
        dir = at((*module).to_owned());
    }
    relative.reverse();
    relative.push(target.to_owned());
    Some(relative)
}

/// The file of `set` that the package-relative path `rel` names: in a
/// Cargo workspace it sits below the repository root, so files match by
/// suffix, and when several packages have one, the one whose directory is
/// named like the binary wins, else any.
fn pick(set: &BTreeSet<String>, binary: &str, rel: &str) -> Option<String> {
    let crate_dir = binary.replace('_', "-");
    let hits: Vec<&String> = set
        .iter()
        .filter(|p| *p == rel || p.ends_with(&format!("/{rel}")))
        .collect();
    hits.iter()
        .find(|p| {
            p.strip_suffix(rel)
                .map(|d| d.trim_end_matches('/'))
                .is_some_and(|d| d.rsplit('/').next() == Some(crate_dir.as_str()))
        })
        .or(hits.first())
        .map(|p| (*p).clone())
}

/// The one file of `changed` in a crate directory that a libtest target
/// path names: the target is relative to its crate, so in a Cargo
/// workspace `tests/ci.rs` is `crates/aver/tests/ci.rs`. Two such files
/// leave it unknown.
pub(super) fn crate_target<'a>(changed: &'a BTreeSet<String>, target: &str) -> Option<&'a String> {
    let suffix = format!("/{target}");
    let mut nested = changed.iter().filter(|c| c.ends_with(&suffix));
    match (nested.next(), nested.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

/// Where the test of an entry is defined, as far as its path says.
pub(super) enum Defines {
    /// An existing test file the head modified or deleted.
    Changed(Path),
    /// A test file the head added or left alone.
    Other { touched: bool },
    /// The path names no test file (a runner that reports suite names
    /// only), or the entry is a whole command.
    Unknown {
        any_changed: bool,
        any_touched: bool,
    },
}

impl Defines {
    /// Whether the head modified or deleted the file that defines it. An
    /// entry whose file is unknown counts any modified or deleted test file:
    /// the judged head and the head-tests tree differ only in those.
    pub(super) fn changed(&self) -> bool {
        match self {
            Self::Changed(_) => true,
            Self::Other { .. } => false,
            Self::Unknown { any_changed, .. } => *any_changed,
        }
    }

    /// Whether the head added or modified the file that defines it.
    pub(super) fn touched(&self) -> bool {
        match self {
            Self::Changed(_) => true,
            Self::Other { touched } => *touched,
            Self::Unknown { any_touched, .. } => *any_touched,
        }
    }

    pub(super) fn file(&self) -> Option<Path> {
        match self {
            Self::Changed(p) => Some(p.clone()),
            _ => None,
        }
    }
}

impl EntryContext {
    pub(super) fn new(store: &Store, plan: &Plan) -> Result<Self, StoreError> {
        let names = |paths: &[Path]| -> Vec<String> {
            paths.iter().map(|p| p.as_str().to_owned()).collect()
        };
        let changed: BTreeSet<String> = names(&plan.tests.modified)
            .into_iter()
            .chain(names(&plan.tests.deleted))
            .collect();
        let touched: BTreeSet<String> = names(&plan.tests.added)
            .into_iter()
            .chain(names(&plan.tests.modified))
            .collect();
        Ok(Self {
            env: plan.env.clone(),
            changed,
            deleted: names(&plan.tests.deleted).into_iter().collect(),
            touched,
            retargeted: plan.retargeted.clone(),
            deleted_sources: names(&plan.deleted_sources).into_iter().collect(),
            base_root: store.get::<Snapshot>(&plan.base)?.root,
            head_root: store.get::<Snapshot>(&plan.head)?.root,
            trees: std::sync::OnceLock::new(),
        })
    }

    /// Whether the change can explain why the test module `id` failed to
    /// collect: the head changed the module, or [`python_imports_changed`].
    /// A module the base cannot read counts as related.
    fn uncollected_related(
        &self,
        store: &Store,
        def: &CheckDef,
        id: &TestCheckId,
    ) -> Result<bool, StoreError> {
        let file = id
            .test_path()
            .split(junit::SEPARATOR)
            .next()
            .unwrap_or_default();
        let file = match &def.cwd {
            Some(cwd) => format!("{cwd}/{file}"),
            None => file.to_owned(),
        };
        if self.changed.contains(&file) || self.touched.contains(&file) {
            return Ok(true);
        }
        if self.trees.get().is_none() {
            let base: BTreeMap<String, FileEntry> =
                crate::place::workdir::tree_files(store, &self.base_root)?
                    .into_iter()
                    .collect();
            let head = crate::place::workdir::tree_files(store, &self.head_root)?;
            let mut changed: BTreeSet<String> = head
                .iter()
                .filter(|(p, e)| base.get(p) != Some(e))
                .map(|(p, _)| p.clone())
                .collect();
            let head: BTreeSet<&String> = head.iter().map(|(p, _)| p).collect();
            changed.extend(base.keys().filter(|p| !head.contains(p)).cloned());
            let _ = self.trees.set((base, changed));
        }
        let (base, changed) = self.trees.get().expect("set above");
        let Some(entry) = base.get(&file) else {
            return Ok(true);
        };
        let text = String::from_utf8_lossy(&store.get_raw(&entry.blob)?).into_owned();
        Ok(python_imports_changed(
            &file,
            def.cwd.as_ref().map(|c| c.as_str()),
            &text,
            base,
            changed,
        ))
    }

    /// The top-level tests the base's `_test.go` files declare in the Go
    /// package with import path `package`: the files directly in its
    /// directory that `go test` builds here with the check's tags, found
    /// through the `go.mod` whose module path is the longest prefix of it.
    fn go_tests(
        &self,
        store: &Store,
        package: &str,
        def: &CheckDef,
    ) -> Result<Vec<String>, StoreError> {
        let tags = crate::exec::gojson::tags_of(&def.argv);
        let files = crate::place::workdir::tree_files(store, &self.base_root)?;
        let mut best: Option<(usize, String)> = None;
        for (path, entry) in &files {
            let dir = match path.strip_suffix("go.mod") {
                Some("") => "",
                Some(d) if d.ends_with('/') => d.trim_end_matches('/'),
                _ => continue,
            };
            let text = String::from_utf8_lossy(&store.get_raw(&entry.blob)?).into_owned();
            let Some(module) = text
                .lines()
                .find_map(|l| l.trim().strip_prefix("module "))
                .map(|m| m.trim().trim_matches('"'))
            else {
                continue;
            };
            let rest = match package.strip_prefix(module) {
                Some("") => "",
                Some(r) if r.starts_with('/') => &r[1..],
                _ => continue,
            };
            if best.as_ref().is_none_or(|(len, _)| module.len() > *len) {
                let at: Vec<&str> = [dir, rest].into_iter().filter(|p| !p.is_empty()).collect();
                best = Some((module.len(), at.join("/")));
            }
        }
        let Some((_, dir)) = best else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for (path, entry) in &files {
            let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
            if parent == dir && name.ends_with("_test.go") {
                let text = String::from_utf8_lossy(&store.get_raw(&entry.blob)?).into_owned();
                if crate::exec::gojson::builds(name, &text, crate::exec::gojson::host(), &tags) {
                    out.extend(crate::exec::gojson::declared_tests(&text));
                }
            }
        }
        Ok(out)
    }

    /// Where the test `test` of check `def` is defined: the first segment of
    /// its path (the file, for Jest, Vitest and tap), relative to the
    /// repository or to the check's directory.
    pub(super) fn defines(&self, def: &CheckDef, test: Option<&TestCheckId>) -> Defines {
        let unknown = Defines::Unknown {
            any_changed: !self.changed.is_empty(),
            any_touched: !self.touched.is_empty(),
        };
        let Some(test) = test else {
            return unknown;
        };
        let first = test
            .test_path()
            .split(junit::SEPARATOR)
            .next()
            .unwrap_or_default();
        let mut candidates = vec![first.to_owned()];
        // libtest names the binary and its target: `calc src/lib.rs`.
        if let Some((binary, target)) = first.split_once(' ') {
            candidates.push(target.to_owned());
            if let Some(found) = self.rust_file(binary, target, test.test_path()) {
                return found;
            }
            candidates.extend(crate_target(&self.changed, target).cloned());
        }
        if let Some(cwd) = &def.cwd {
            for c in candidates.clone() {
                candidates.push(format!("{cwd}/{c}"));
            }
        }
        let mut named = false;
        for path in &candidates {
            if self.changed.contains(path) {
                if let Ok(p) = path.parse() {
                    return Defines::Changed(p);
                }
            }
            named |= self.env.is_test_file(path);
        }
        if named {
            Defines::Other {
                touched: candidates.iter().any(|p| self.touched.contains(p)),
            }
        } else {
            unknown
        }
    }

    /// What the head did to the file that defines an entry's test. A file
    /// the path does not name counts as modified when the head modified or
    /// deleted any test file: the judged head and the head-tests tree
    /// differ only in those.
    pub(super) fn origin(&self, defines: &Defines) -> Origin {
        match defines {
            Defines::Changed(p) if self.deleted.contains(p.as_str()) => Origin::DeletedTestFile,
            Defines::Changed(_) => Origin::ModifiedTestFile,
            Defines::Other { touched: true } => Origin::AddedTestFile,
            Defines::Other { touched: false } => Origin::UntouchedTestFile,
            Defines::Unknown {
                any_changed: true, ..
            } => Origin::ModifiedSource,
            Defines::Unknown {
                any_changed: false, ..
            } => Origin::UntouchedSource,
        }
    }

    /// Where a libtest entry is defined, among the test files the head
    /// changed or added ([`rust_candidates`], [`pick`]).
    fn rust_file(&self, binary: &str, target: &str, path: &str) -> Option<Defines> {
        for rel in &rust_candidates(target, path)? {
            if let Some(path) = pick(&self.changed, binary, rel).and_then(|p| p.parse().ok()) {
                return Some(Defines::Changed(path));
            }
            if pick(&self.touched, binary, rel).is_some() {
                return Some(Defines::Other { touched: true });
            }
        }
        None
    }

    /// The Rust source file the head deleted that held the libtest entry
    /// `test`, matched as [`Self::rust_file`] matches.
    fn deleted_source(&self, test: &TestCheckId) -> Option<String> {
        if self.deleted_sources.is_empty() {
            return None;
        }
        let (binary, target) = test
            .test_path()
            .split(junit::SEPARATOR)
            .next()?
            .split_once(' ')?;
        rust_candidates(target, test.test_path())?
            .iter()
            .find_map(|rel| pick(&self.deleted_sources, binary, rel))
    }

    /// Whether a libtest entry belongs to a test target the head declares
    /// in a `[[test]]` table the base's frozen `Cargo.toml` lacks, in a
    /// package where the head added or changed test files.
    pub(super) fn retargeted(&self, test: Option<&TestCheckId>) -> bool {
        let Some(first) = test.and_then(|t| t.test_path().split(junit::SEPARATOR).next()) else {
            return false;
        };
        let Some((binary, target)) = first.split_once(' ') else {
            return false;
        };
        self.retargeted.iter().any(|r| {
            r.is_label(binary, target)
                && self
                    .touched
                    .iter()
                    .any(|p| r.dir.is_empty() || p.starts_with(&format!("{}/", r.dir)))
        })
    }
}

/// Most criteria whose proving test's source diagnostics carry.
const MAX_PROOF_SOURCES: usize = 10;

/// Weak proofs alone never prove (§1.9.4): each criterion whose every
/// proof is weak joins `unproven`, and a verdict that needed it proven
/// (`PASS`, `REVIEW`) becomes `UNPROVEN`. `FAIL_OWN` and `BLOCKED_NOT_OWN`
/// outrank it and stand.
fn weaken(body: &mut ReceiptBody, weak: &[diagnostics::WeakProof]) {
    if weak.is_empty() {
        return;
    }
    let unproven: Vec<CriterionId> = body
        .criteria
        .iter()
        .map(|c| c.id.clone())
        .filter(|id| body.unproven.contains(id) || weak.iter().any(|w| w.criterion == *id))
        .collect();
    body.unproven = unproven;
    use aver_core::Verdict;
    if matches!(body.verdict, Verdict::Pass | Verdict::Review) {
        body.verdict = Verdict::Unproven;
    }
}

/// What the change's tests show (§1.9.4): its own tests that fail on the
/// transplant and pass in the head-tests run, the tests it fixed and those
/// still failing, and the proven criteria whose proofs only fail to load on
/// the original code.
fn change_evidence(
    store: &Store,
    results: &[CheckRuns],
    body: &ReceiptBody,
    context: &EntryContext,
    entries: &BTreeMap<EntryId, Evidence>,
    names: &BTreeSet<String>,
) -> Result<(diagnostics::Evidence, Vec<diagnostics::WeakProof>), StoreError> {
    let mut shown: BTreeMap<EntryId, diagnostics::Shown> = BTreeMap::new();
    for check in results {
        let def = &check.planned.def;
        let Some(own) = reported(&check.head_tests) else {
            continue;
        };
        let transplant = reported(&check.transplant);
        // Cargo stops at the first test target that does not compile, so a
        // test of what the change adds leaves the whole transplant without
        // a report. Over a base that built, the tests the base never ran
        // could not exist on the original code: they show the change, as
        // tests that do not load there.
        let unbuilt = transplant.is_none()
            && reported(&check.base).is_some()
            && !Builds::new(store, check)?.transplant.is_empty();
        if transplant.is_none() && !unbuilt {
            continue;
        }
        let base = reported(&check.base).unwrap_or_default();
        let ids: BTreeSet<&TestCheckId> = own
            .iter()
            .flat_map(|r| r.tests.iter())
            .filter(|t| t.status != TestStatus::Skip)
            .map(|t| &t.id)
            .collect();
        for id in ids {
            if head_tests_status(&check.head_tests, Some(id), def) != Transplant::Pass
                || !context.defines(def, Some(id)).touched()
            {
                continue;
            }
            // A test that fails on the base too is one the change fixed,
            // listed as such; it says nothing about what the change adds.
            let before = test_outcomes(&base, id);
            if !before.is_empty() && before.iter().all(|e| *e == Ending::Fail) {
                continue;
            }
            if unbuilt && !before.is_empty() {
                continue;
            }
            let original = if unbuilt {
                Transplant::Absent
            } else {
                transplant_status(&check.transplant, Some(id))
            };
            if !matches!(original, Transplant::Fail | Transplant::Absent) {
                continue;
            }
            // A test whose rewritten expectation awaits review shows nothing
            // the change does: listing it as evidence would vouch for it.
            let changed = body
                .checks
                .get(id.as_str())
                .is_some_and(|e| e.cause == Some(Cause::Changed));
            if changed {
                continue;
            }
            let message = (original == Transplant::Fail)
                .then(|| transplant.as_deref().and_then(|t| first_message(t, id)))
                .flatten();
            let missing_module = message.as_deref().is_some_and(diagnostics::missing_module);
            let name = message
                .as_deref()
                .and_then(|m| crate::idents::missing(m, names))
                .filter(|_| !missing_module);
            let missing_name = !missing_module
                && (name.is_some() || message.as_deref().is_some_and(diagnostics::missing_name));
            shown.insert(
                id.clone().into(),
                diagnostics::Shown {
                    id: id.clone().into(),
                    original,
                    missing_module,
                    missing_name,
                    name: name.map(str::to_owned),
                },
            );
        }
    }
    let rule = body.schema.proof_rule();
    let mut weak = Vec::new();
    for criterion in body.criteria.iter() {
        if body.unproven.contains(&criterion.id) {
            continue;
        }
        let proofs: Vec<&ReceiptCheck> = body
            .checks
            .iter()
            .filter(|e| e.proves.get(criterion.id.as_str()).is_some() && rules::proves(e, rule))
            .collect();
        let built = |e: &ReceiptCheck| entries.get(&e.id).and_then(|v| v.build.as_ref());
        let builds_only = !proofs.is_empty() && proofs.iter().all(|e| built(e).is_some());
        let only_loads = !proofs.is_empty()
            && proofs.iter().all(|e| {
                built(e).is_some()
                    || e.transplant == Some(Transplant::Absent)
                    || shown.get(&e.id).is_some_and(diagnostics::Shown::weak)
            });
        if builds_only {
            // Every proof's test failed to build on the original code. When
            // each error is a name that does not resolve there (the head's
            // tests passed, so the head defines it), that is a test of a
            // new API; any other build error says less.
            let new_api = proofs.iter().all(|e| {
                built(e).is_some_and(|errors| {
                    !errors.is_empty() && errors.iter().all(|x| toolchain::unresolved_name(x))
                })
            });
            weak.push(diagnostics::WeakProof {
                criterion: criterion.id.clone(),
                entries: proofs.iter().map(|e| e.id.clone()).collect(),
                reason: diagnostics::WEAK_BUILD_ERROR.to_owned(),
                new_api,
                name: None,
            });
        } else if only_loads {
            let names = proofs
                .iter()
                .all(|e| built(e).is_some() || shown.get(&e.id).is_some_and(|s| s.missing_name));
            weak.push(diagnostics::WeakProof {
                criterion: criterion.id.clone(),
                entries: proofs.iter().map(|e| e.id.clone()).collect(),
                reason: if names {
                    diagnostics::WEAK_MISSING_NAME
                } else {
                    diagnostics::WEAK_MISSING_MODULE
                }
                .to_owned(),
                new_api: false,
                name: proofs
                    .iter()
                    .find_map(|e| shown.get(&e.id).and_then(|s| s.name.clone())),
            });
        }
    }
    let ids = |keep: &dyn Fn(&ReceiptCheck) -> bool| {
        diagnostics::Listing::of(
            body.checks
                .iter()
                .filter(|e| keep(e))
                .map(|e| e.id.as_str().to_owned())
                .collect(),
        )
    };
    let total = shown.len();
    let mut shows_change: Vec<diagnostics::Shown> = shown.into_values().collect();
    shows_change.truncate(diagnostics::MAX_LISTING);
    let evidence = diagnostics::Evidence {
        shows_change_omitted: (total - shows_change.len()) as u64,
        shows_change,
        // A setup finding the head no longer prints is no test, and over a
        // base that did not build it is the base's build error.
        fixed: ids(&|e| {
            e.base == BaseStatus::Fail
                && e.head == HeadStatus::Pass
                && e.cause.is_none()
                && !e
                    .id
                    .as_str()
                    .split_once("::")
                    .is_some_and(|(_, path)| path.starts_with(SETUP_TEST_PATH))
        }),
        still_failing: ids(&|e| e.cause == Some(Cause::Preexisting)),
    };
    Ok((evidence, weak))
}

/// How an entry fared on the transplant runs of its check (§1.9.3): `infra`
/// when they ended in infra; for a test whose transplant runs all carry
/// reports, `pass` if it passed in any, `fail` if it failed in one, else
/// `absent`; otherwise `pass` if any transplant run passed, else `fail`.
fn transplant_status(runs: &[ExecutedRun], test: Option<&TestCheckId>) -> Transplant {
    if runs
        .last()
        .is_some_and(|r| r.run.result.ending().is_infra())
    {
        return Transplant::Infra;
    }
    let results: Vec<&CheckRun> = runs
        .iter()
        .map(|r| &r.run)
        .filter(|r| !r.result.ending().is_infra())
        .collect();
    if let Some(id) = test.filter(|_| results.iter().all(|r| r.report.is_some())) {
        let outcomes = test_outcomes(&results, id);
        return if outcomes.contains(&Ending::Pass) {
            Transplant::Pass
        } else if outcomes.contains(&Ending::Fail) {
            Transplant::Fail
        } else {
            Transplant::Absent
        };
    }
    if results.iter().any(|r| r.result.ending() == Ending::Pass) {
        Transplant::Pass
    } else {
        Transplant::Fail
    }
}

/// Most build errors kept per side.
const MAX_BUILD_ERRORS: usize = 8;

/// What a check's runs say about builds, for the transplant proof rule
/// (§1.9.3): a transplant run that failed to build counts as failing only
/// over a base that built and reported.
#[derive(Default)]
pub(super) struct Builds {
    /// The first build error in the output of a base run that reported no
    /// test, or whose go-json report has a package that did not build.
    base: Option<String>,
    /// The build errors in the output of such a transplant run.
    transplant: Vec<String>,
}

/// What a proving entry's transplant result rests on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BuildProof {
    /// Its tests ran there (or the transplant passed).
    None,
    /// The transplant failed to build over a base that built: its errors.
    Transplant(Vec<String>),
    /// The original code does not build: its first error. The transplant
    /// then proves nothing and counts as `infra`.
    Base(String),
}

impl Builds {
    pub(super) fn new(store: &Store, check: &CheckRuns) -> Result<Self, StoreError> {
        let def = &check.planned.def;
        if check.transplant.is_empty() || def.report.is_none() {
            return Ok(Self::default());
        }
        let errors = |runs: &[ExecutedRun]| -> Result<Vec<String>, StoreError> {
            let unbuilt = runs.iter().filter(|r| {
                !r.run.result.ending().is_infra()
                    && (r.run.report.is_none() || !package_failures(&r.run).is_empty())
            });
            for r in unbuilt {
                let output = String::from_utf8_lossy(&store.get_raw(&r.run.output)?).into_owned();
                let mut found = toolchain::build_errors(&output, MAX_BUILD_ERRORS);
                if found.is_empty() {
                    found = package_failures(&r.run)
                        .iter()
                        .flat_map(|m| toolchain::build_errors(m, MAX_BUILD_ERRORS))
                        .take(MAX_BUILD_ERRORS)
                        .collect();
                }
                if !found.is_empty() {
                    return Ok(found);
                }
            }
            Ok(Vec::new())
        };
        Ok(Self {
            base: errors(&check.base)?.into_iter().next(),
            transplant: errors(&check.transplant)?,
        })
    }

    /// How a proving entry fared on the transplant ([`transplant_status`]),
    /// and whether that rests on a build: for a test missing from a go-json
    /// report whose package did not build there, or for a transplant run
    /// without a report whose output holds build errors. Over a base that
    /// did not build (the base run reported nothing, or the test's package
    /// did not build there either) the transplant is `infra`.
    pub(super) fn proof(
        &self,
        check: &CheckRuns,
        test: Option<&TestCheckId>,
    ) -> (Transplant, BuildProof) {
        let status = transplant_status(&check.transplant, test);
        if !matches!(status, Transplant::Fail | Transplant::Absent) {
            return (status, BuildProof::None);
        }
        let all_reported = reported(&check.transplant).is_some();
        let base = reported(&check.base);
        let package = test
            .filter(|_| all_reported && status == Transplant::Absent)
            .and_then(|t| package_unbuilt(&check.transplant, t));
        let base_unbuilt = match (&package, test) {
            (Some(_), Some(t)) => base.is_none() || package_unbuilt(&check.base, t).is_some(),
            (None, _) if !all_reported && !self.transplant.is_empty() => base.is_none(),
            _ => return (status, BuildProof::None),
        };
        let errors = match package {
            Some(messages) => {
                let found: Vec<String> = messages
                    .iter()
                    .flat_map(|m| toolchain::build_errors(m, MAX_BUILD_ERRORS))
                    .take(MAX_BUILD_ERRORS)
                    .collect();
                if found.is_empty() {
                    self.transplant.clone()
                } else {
                    found
                }
            }
            None => self.transplant.clone(),
        };
        if base_unbuilt {
            let first = self
                .base
                .clone()
                .or_else(|| errors.first().cloned())
                .unwrap_or_else(|| "no report".to_owned());
            return (Transplant::Infra, BuildProof::Base(first));
        }
        (status, BuildProof::Transplant(errors))
    }
}

/// The messages of a go-json run's failing `<package> > (package)` entries:
/// packages that failed without a failing test (one that did not build).
fn package_failures(run: &CheckRun) -> Vec<String> {
    let suffix = format!(
        "{}{}",
        junit::SEPARATOR,
        crate::exec::gojson::PACKAGE_FAILED
    );
    run.tests
        .iter()
        .filter(|t| t.status == TestStatus::Fail && t.id.test_path().ends_with(&suffix))
        .map(|t| t.message.as_ref().map_or("", |m| m.as_str()).to_owned())
        .collect()
}

/// The messages of the failing `(package)` entry of `test`'s package in
/// `runs` when one of them says the package did not build.
fn package_unbuilt(runs: &[ExecutedRun], test: &TestCheckId) -> Option<Vec<String>> {
    let package = test.test_path().split(junit::SEPARATOR).next()?;
    let id = TestCheckId::new(
        &test.check(),
        &format!(
            "{package}{}{}",
            junit::SEPARATOR,
            crate::exec::gojson::PACKAGE_FAILED
        ),
    )
    .ok()?;
    let messages: Vec<String> = runs
        .iter()
        .filter_map(|r| r.run.tests.get(id.as_str()))
        .filter(|t| t.status == TestStatus::Fail)
        .map(|t| t.message.as_ref().map_or("", |m| m.as_str()).to_owned())
        .filter(|m| m.contains("[build failed]") || !toolchain::build_errors(m, 1).is_empty())
        .collect();
    (!messages.is_empty()).then_some(messages)
}

/// How an entry fared in the head-tests runs of its check (§1.9.3), strictly
/// the other way round from the transplant: `infra` when they ended in
/// infra; for a test, `fail` if a run has no report or the test failed in
/// one, `pass` if it passed, else `absent`; for the command, `pass` only if
/// every run passed.
fn head_tests_status(
    runs: &[ExecutedRun],
    test: Option<&TestCheckId>,
    def: &CheckDef,
) -> Transplant {
    if runs
        .last()
        .is_some_and(|r| r.run.result.ending().is_infra())
    {
        return Transplant::Infra;
    }
    let results: Vec<&CheckRun> = runs
        .iter()
        .map(|r| &r.run)
        .filter(|r| !r.result.ending().is_infra())
        .collect();
    if results.is_empty() {
        return Transplant::Fail;
    }
    match test {
        Some(_) if results.iter().any(|r| r.report.is_none()) => Transplant::Fail,
        Some(id) => {
            let outcomes = test_outcomes(&results, id);
            if outcomes.contains(&Ending::Fail) {
                Transplant::Fail
            } else if outcomes.contains(&Ending::Pass) {
                Transplant::Pass
            } else {
                Transplant::Absent
            }
        }
        None if results.iter().all(|r| ending(r, def) == Ending::Pass) => Transplant::Pass,
        None => Transplant::Fail,
    }
}

/// How the head's own version of an entry's test fared in its check's
/// head-tests runs, for the table: a side whose every run hit a resource
/// cap is the head's, a side otherwise ended in infra is lost, a run
/// without a declared report did not build, and a test no run reports
/// passing or failing was skipped if a run lists it, else deleted.
fn own_of(runs: &[ExecutedRun], test: Option<&TestCheckId>, def: &CheckDef) -> Own {
    let endings: Vec<Ending> = runs.iter().map(|r| r.run.result.ending()).collect();
    if endings.last().is_some_and(|e| e.is_infra()) {
        return if endings.iter().all(|e| *e == Ending::Cap) {
            Own::Exhausted
        } else {
            Own::Infra
        };
    }
    let results: Vec<&CheckRun> = runs
        .iter()
        .map(|r| &r.run)
        .filter(|r| !r.result.ending().is_infra())
        .collect();
    if results.is_empty() {
        return Own::NotRun;
    }
    if def.report.is_some() && results.iter().any(|r| r.report.is_none()) {
        return Own::CompileError;
    }
    let Some(id) = test else {
        return if results.iter().all(|r| ending(r, def) == Ending::Pass) {
            Own::Pass
        } else {
            Own::Fail
        };
    };
    let outcomes = test_outcomes(&results, id);
    match (
        outcomes.contains(&Ending::Pass),
        outcomes.contains(&Ending::Fail),
    ) {
        (true, true) => Own::Flaky,
        (true, false) => Own::Pass,
        (false, true) => Own::Fail,
        (false, false) if results.iter().any(|r| r.tests.get(id.as_str()).is_some()) => {
            Own::Skipped
        }
        (false, false) => Own::Absent,
    }
}

/// What the head did to the tests, in full: the test files it added,
/// changed and deleted, the tests with a result at base that its own
/// versions of the tests no longer report or only skip, and the judging
/// entries.
pub(super) struct Changes {
    files: TestFiles,
    removed: BTreeSet<TestCheckId>,
    skipped: BTreeSet<TestCheckId>,
    harness: Vec<EntryId>,
}

pub(super) fn test_changes(
    files: &TestFiles,
    results: &[CheckRuns],
    harness: Vec<EntryId>,
) -> Changes {
    let mut removed = BTreeSet::new();
    let mut skipped = BTreeSet::new();
    for check in results {
        let (Some(base), Some(own)) = (reported(&check.base), reported(&check.head_tests)) else {
            continue;
        };
        for id in base.iter().flat_map(|r| r.tests.iter()).map(|t| &t.id) {
            if test_outcomes(&base, id).is_empty() {
                continue;
            }
            if !own.iter().any(|r| r.tests.get(id.as_str()).is_some()) {
                removed.insert(id.clone());
            } else if test_outcomes(&own, id).is_empty() {
                skipped.insert(id.clone());
            }
        }
    }
    Changes {
        files: files.clone(),
        removed,
        skipped,
        harness,
    }
}

impl Changes {
    pub(super) fn receipt(&self) -> TestChanges {
        TestChanges {
            files_added: Listed::of(self.files.added.clone()),
            files_modified: Listed::of(self.files.modified.clone()),
            files_deleted: Listed::of(self.files.deleted.clone()),
            tests_removed: Listed::of(self.removed.iter().cloned().collect()),
            tests_skipped: Listed::of(self.skipped.iter().cloned().collect()),
            harness_changed: Listed::of(self.harness.clone()),
        }
    }

    pub(super) fn diagnostics(&self) -> TestChangeLists {
        let list = |items: Vec<String>| diagnostics::Listing::of(items);
        let paths = |p: &[Path]| list(p.iter().map(ToString::to_string).collect());
        TestChangeLists {
            files_added: paths(&self.files.added),
            files_modified: paths(&self.files.modified),
            files_deleted: paths(&self.files.deleted),
            tests_removed: list(self.removed.iter().map(ToString::to_string).collect()),
            tests_skipped: list(self.skipped.iter().map(ToString::to_string).collect()),
            harness_changed: list(self.harness.iter().map(ToString::to_string).collect()),
        }
    }
}

/// Every run of a verification once, with the side it first ran for; a run
/// listed again (the same tree on two sides) counts no sandbox time.
fn timed_runs(results: &[CheckRuns]) -> Vec<TimedRun> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for check in results {
        let rebuilt: Vec<ExecutedRun> = match &check.rebuild {
            Some(Rebuild::Rebuilt(r)) => r.lost.iter().chain([&r.probe]).cloned().collect(),
            Some(Rebuild::Lost(probe)) => vec![probe.clone()],
            None => Vec::new(),
        };
        let sides = [
            (Side::Base, &check.base),
            (Side::Base, &check.confirmations),
            (Side::Head, &check.head),
            (Side::Head, &rebuilt),
            (Side::HeadTests, &check.head_tests),
            (Side::Transplant, &check.transplant),
        ];
        for (n, (side, runs)) in sides.into_iter().enumerate() {
            if side == Side::HeadTests && !check.separate {
                continue;
            }
            for r in runs {
                let first = seen.insert(r.id) && !r.cache_hit;
                out.push(TimedRun {
                    side,
                    check: check.planned.def.id.clone(),
                    index: r.run.index.get(),
                    run: r.id,
                    cache_hit: !first,
                    from_cache: r.cache_hit,
                    started_at: r.run.started_at,
                    install: if first {
                        r.timing.install
                    } else {
                        Duration::ZERO
                    },
                    sandbox: if first {
                        r.timing.check
                    } else {
                        Duration::ZERO
                    },
                    confirmation: n == 1,
                });
            }
        }
    }
    out
}

/// Per reported run, whether the run as a whole says more than its tests: it
/// exited non-zero although no test failed, or it reported no tests although
/// the base reported `base_tests`. Either fails the run's exit entry, so a
/// head cannot pass by writing a report that hides how the command ended.
fn exit_endings(runs: &[&CheckRun], base_tests: usize) -> Vec<Ending> {
    runs.iter()
        .map(|r| {
            let failing_test = r.tests.iter().any(|t| t.status == TestStatus::Fail);
            let unexplained = r.result.ending() == Ending::Fail && !failing_test;
            let emptied = r.tests.is_empty() && base_tests > 0;
            if unexplained || emptied {
                Ending::Fail
            } else {
                Ending::Pass
            }
        })
        .collect()
}

/// Why the head's exit entry failed, with the end of the last such run's
/// output, and that run.
fn exit_detail(
    store: &Store,
    executed: &[ExecutedRun],
    head: &[&CheckRun],
    base_tests: usize,
) -> Result<(String, RunId), StoreError> {
    let bad = head
        .iter()
        .zip(exit_endings(head, base_tests))
        .rev()
        .find(|(_, e)| *e == Ending::Fail)
        .map(|(r, _)| *r)
        .expect("called with a failing exit entry");
    let reason = if bad.tests.is_empty() && base_tests > 0 {
        format!("the report has no tests; the base reported {base_tests}")
    } else {
        format!(
            "exited {} with no failing test in its report",
            bad.result.exit_code().unwrap_or_default()
        )
    };
    let run = executed
        .iter()
        .find(|r| std::ptr::eq(&r.run, bad))
        .expect("the run is one of the head's");
    let output = store.get_raw(&run.run.output)?;
    let output = String::from_utf8_lossy(&output);
    let room = MAX_DETAIL_BYTES.saturating_sub(reason.len() + 1);
    Ok((format!("{reason}\n{}", tail(&output, room)), run.id))
}

/// A test's pass and fail results across a side's runs; skips do not count.
pub(super) fn test_outcomes(runs: &[&CheckRun], id: &TestCheckId) -> Vec<Ending> {
    runs.iter()
        .filter_map(|r| r.tests.get(id.as_str()))
        .filter_map(|t| match t.status {
            TestStatus::Pass => Some(Ending::Pass),
            TestStatus::Fail => Some(Ending::Fail),
            TestStatus::Skip => None,
        })
        .collect()
}

/// Whether a change to `changed` can explain why the test module at
/// `file` (source `text`) failed to collect: a `conftest.py` above it
/// changed, or it imports the repository's own Python code (a top-level
/// name of a `.py` file in `tree` or `changed`, below the check's `cwd` and
/// a `src/` directory; a relative import counts) and the change touched
/// any file of that code. Imports are followed no further than the top
/// level, since the package it imports may import the changed one.
fn python_imports_changed(
    file: &str,
    cwd: Option<&str>,
    text: &str,
    tree: &BTreeMap<String, FileEntry>,
    changed: &BTreeSet<String>,
) -> bool {
    let dir = file.rsplit_once('/').map_or("", |(d, _)| d);
    let conftest = changed.iter().any(|path| {
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
        name == "conftest.py"
            && (parent.is_empty() || dir == parent || dir.starts_with(&format!("{parent}/")))
    });
    if conftest {
        return true;
    }
    let top = |path: &str| -> Option<String> {
        let path = match cwd {
            Some(cwd) => path.strip_prefix(cwd)?.strip_prefix('/')?,
            None => path,
        };
        let path = path.strip_prefix("src/").unwrap_or(path);
        let first = path.split('/').next().unwrap_or_default();
        Some(first.strip_suffix(".py").unwrap_or(first).to_owned())
    };
    let own: BTreeSet<String> = tree
        .keys()
        .chain(changed)
        .filter(|p| p.ends_with(".py"))
        .filter_map(|p| top(p))
        .collect();
    let mut imports_own = false;
    for line in text.lines() {
        let line = line.trim_start();
        let modules: Vec<&str> = if let Some(rest) = line.strip_prefix("from ") {
            rest.split_whitespace().next().into_iter().collect()
        } else if let Some(rest) = line.strip_prefix("import ") {
            rest.split(',')
                .filter_map(|m| m.split_whitespace().next())
                .collect()
        } else {
            continue;
        };
        imports_own |= modules.iter().any(|m| {
            let name = m.split('.').next().unwrap_or_default();
            name.is_empty() || own.contains(name)
        });
    }
    imports_own
        && changed
            .iter()
            .any(|p| top(p).is_some_and(|t| own.contains(&t)))
}

/// Whether any of `runs` reports `id` as a test module that failed to
/// collect ([`junit::collection_error`]).
fn collection_failed(runs: &[&CheckRun], id: &TestCheckId) -> bool {
    runs.iter()
        .filter_map(|r| r.tests.get(id.as_str()))
        .any(junit::collection_error)
}

fn first_message(runs: &[&CheckRun], id: &TestCheckId) -> Option<String> {
    runs.iter()
        .filter_map(|r| r.tests.get(id.as_str()))
        .find_map(|t| t.message.as_ref())
        .map(|m| m.as_str().to_owned())
}

/// The check's own `proves` plus every criterion of this task whose tag the
/// test's name carries.
pub(super) fn proves(
    check: &Set<CriterionId>,
    test_path: &str,
    criteria: &Criteria,
) -> Set<CriterionId> {
    let tags = aver_core::tag::Tags::new(criteria.iter().map(|c| &c.id));
    let tagged = tags.carried(test_path).cloned().collect::<Vec<_>>();
    Set::new(check.iter().cloned().chain(tagged).collect()).expect("criterion ids are their keys")
}

/// The last `max` bytes of `text`, starting on a character boundary.
fn tail(text: &str, max: usize) -> &str {
    let mut start = text.len().saturating_sub(max);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}
