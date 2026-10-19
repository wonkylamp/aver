//! Running a plan's checks: each side's runs, reruns, the base and fresh
//! confirmations, and the Cargo probe rebuild of restored test files.

use super::*;

impl Verifier {
    /// Every check's runs on each side under the rerun policy, the checks at
    /// once, with each side's wall time.
    pub(super) async fn run_checks(
        &self,
        plan: &Arc<Plan>,
        fresh: bool,
        progress: Option<Arc<crate::progress::Log>>,
        pools: Option<Places>,
    ) -> Result<(Vec<CheckRuns>, Vec<SideTime>), VerifyError> {
        let mut runs = tokio::task::JoinSet::new();
        // A task's runs wait in its own queue, so another task's verify
        // gets slots in turn (§2.4).
        let executor = self.executor.for_requester(plan.task_id);
        let in_place = executor
            .slot_runner()
            .is_some_and(crate::place::runner::SlotRunner::in_place);
        for planned in plan.checks.iter().cloned() {
            // On this machine, the base, the transplant and the head sides
            // each use their own slot (§1.8.5); the transplant runs at lower
            // priority so the head finishes first.
            let (exec, head_exec, transplant_exec) = match &pools {
                Some(p) => {
                    let side = |place: &Option<Place>| match place {
                        Some(place) => executor.in_slot(place.clone()),
                        None => executor.clone(),
                    };
                    (
                        executor.in_slot(p.base.clone()),
                        side(&p.head),
                        side(&p.transplant),
                    )
                }
                None => (executor.clone(), executor.clone(), executor.clone()),
            };
            // In place every run swaps one checkout, and a swap rebuilds
            // what it rewrites: the base's runs go first, then the
            // transplant's (base code), then the head's two trees (head
            // code), across every check.
            let (exec, transplant_exec, head_exec, head_tests_exec) = if in_place {
                (
                    exec.ranked(0),
                    transplant_exec.ranked(1),
                    head_exec.ranked(2),
                    head_exec.ranked(3),
                )
            } else {
                let head_tests_exec = head_exec.clone();
                (exec, transplant_exec, head_exec, head_tests_exec)
            };
            let plan = plan.clone();
            let progress = progress.clone();
            let store = self.store.clone();
            runs.spawn(async move {
                let def = &planned.def;
                // The repository's own per-test checks judge the base's
                // tests in a run without the head's added test files; a
                // task check, or one judged by its exit code, names or runs
                // them as a whole.
                let head_tree = if def.report.is_some() && planned.source == CheckSource::Env {
                    plan.judged_base_tests
                } else {
                    plan.judged_head
                };
                let separate = plan.head_tests != head_tree;
                // The transplant starts with the other sides when the check
                // could prove something (it proves criteria itself, or it
                // reports per test and the task has criteria) or could show
                // the change ([`shows_change`]); whether it proves is only
                // known from the results, so a transplant that turns out
                // not to matter is dropped.
                let needed = transplant_needed(&plan, &planned);
                let speculate = !fresh
                    && needed
                    && plan.transplant.is_some()
                    && (!planned.proves.is_empty()
                        || (def.report.is_some() && !plan.task.body.criteria.is_empty())
                        || shows_change(&plan, &planned));
                // The transplant runs once, and twice more only when that
                // run fails: a proving test fails there by design, but most
                // transplants pass, and three sandboxes of a slow suite at
                // once cost more than the wait. A command-level check whose
                // base's first run, already cached, failed is expected to
                // fail on the judged head too, so it starts all three (the
                // head-tests side takes one run, `Reruns::OWN_EXIT`). A check with
                // per-test results instead runs its head sides once and
                // reruns them only when the base does not explain the
                // failure (`explained_by_base`).
                let base_failed = !fresh
                    && def.report.is_none()
                    && match RunIndex::try_from(1u8) {
                        Ok(first) => exec
                            .cached(&plan.base, &plan.env, def, first)?
                            .is_some_and(|r| ending(&r.run, def) == Ending::Fail),
                        Err(_) => false,
                    };
                let (base_tx, base_rx) = tokio::sync::watch::channel(None);
                let (head_tx, _) = tokio::sync::watch::channel(None);
                let narrowed = parking_lot::Mutex::new(Vec::new());
                let gate = def.report.is_some().then_some(base_rx);
                let timed = |side: Side, env: Arc<Environment>, tree: SnapshotId| {
                    let exec = match side {
                        Side::Base => exec.clone(),
                        Side::Transplant => transplant_exec.clone(),
                        Side::Head => head_exec.clone(),
                        Side::HeadTests | Side::Ablation => head_tests_exec.clone(),
                    };
                    let def = def.clone();
                    let likely_fail = match side {
                        Side::Base | Side::Transplant | Side::Ablation => false,
                        Side::Head | Side::HeadTests => base_failed,
                    };
                    let gate = match side {
                        Side::Head | Side::HeadTests => gate.clone(),
                        Side::Base | Side::Transplant | Side::Ablation => None,
                    };
                    let log = progress.clone().map(|log| (log, side));
                    let store = store.clone();
                    // The transplant and the head-tests run differ from the
                    // base and the judged head only in test files.
                    let from = match side {
                        Side::Transplant => Some((plan.base, base_tx.subscribe())),
                        Side::HeadTests => Some((head_tree, head_tx.subscribe())),
                        Side::Base | Side::Head | Side::Ablation => None,
                    };
                    let (narrowed, source) = (&narrowed, planned.source);
                    async move {
                        let t = Instant::now();
                        let side_runs = |def: CheckDef, narrow: Option<Arc<narrow::Narrow>>| {
                            let (exec, store, env, gate, log) =
                                (&exec, &store, &env, gate.clone(), log.clone());
                            async move {
                                run_side(
                                    exec,
                                    store,
                                    env,
                                    &tree,
                                    &def,
                                    Reruns::of(side, &def),
                                    fresh,
                                    likely_fail,
                                    gate,
                                    log,
                                    narrow.as_deref(),
                                )
                                .await
                            }
                        };
                        let narrowing = match from {
                            Some((from, rx)) => {
                                narrow::Plan::of(&store, &def, source, &from, &tree, rx)?
                            }
                            None => narrow::Plan::No,
                        };
                        let runs = match narrowing {
                            narrow::Plan::Narrow(n) => {
                                let runs = side_runs(n.def.clone(), Some(n.clone())).await?;
                                narrowed.lock().push(n.record(&def.id, side));
                                if runs.is_empty() {
                                    side_runs(def.clone(), None).await?
                                } else {
                                    runs
                                }
                            }
                            narrow::Plan::Full(why) => {
                                narrowed.lock().push(narrow::full(&def.id, side, why));
                                side_runs(def.clone(), None).await?
                            }
                            narrow::Plan::No => side_runs(def.clone(), None).await?,
                        };
                        Ok::<_, ExecError>((runs, SideTime::of(side, t.elapsed())))
                    }
                };
                let skip = |side: Side| async move {
                    Ok::<_, ExecError>((Vec::new(), SideTime::of(side, Duration::ZERO)))
                };
                // A side whose snapshot is the base's (nothing changed, or
                // nothing a side runs) takes the base side's runs instead of
                // asking for its own: the same cache keys would join them
                // anyway, except under `fresh`, which joins nothing.
                let shared = |side: Side| {
                    let mut rx = base_tx.subscribe();
                    async move {
                        let runs = rx
                            .wait_for(Option::is_some)
                            .await
                            .map_err(|_| ExecError::Internal("the base side ended".into()))?
                            .as_deref()
                            .cloned()
                            .unwrap_or_default();
                        Ok::<_, ExecError>((runs, SideTime::of(side, Duration::ZERO)))
                    }
                };
                let base_tree = plan.base;
                let (shared, timed) = (&shared, &timed);
                let side_on = move |side: Side, env: Arc<Environment>, tree: SnapshotId| async move {
                    if tree == base_tree {
                        shared(side).await
                    } else {
                        timed(side, env, tree).await
                    }
                };
                let transplant_tree = plan.transplant.filter(|_| speculate);
                let base_side = async {
                    let side = timed(Side::Base, plan.env.clone(), plan.base).await?;
                    // Stored even with no receiver yet: a side that
                    // shares the base's runs may subscribe later.
                    base_tx.send_replace(Some(Arc::new(side.0.clone())));
                    Ok::<_, ExecError>(side)
                };
                let head_side = async {
                    let side = side_on(Side::Head, plan.head_env.clone(), head_tree).await?;
                    head_tx.send_replace(Some(Arc::new(side.0.clone())));
                    Ok::<_, ExecError>(side)
                };
                let head_tests_side = async {
                    if separate {
                        side_on(Side::HeadTests, plan.head_env.clone(), plan.head_tests).await
                    } else {
                        skip(Side::HeadTests).await
                    }
                };
                let transplant_side = async {
                    match transplant_tree {
                        Some(tree) => side_on(Side::Transplant, plan.env.clone(), tree).await,
                        None => skip(Side::Transplant).await,
                    }
                };
                // In place the runs take turns anyway; one side after
                // another keeps a run of head code from taking the slot
                // while the base's code is still wanted.
                let ((base, tb), (head, mut th), (head_tests, tt), (early, tx)) = if in_place {
                    let base = base_side.await?;
                    let early = transplant_side.await?;
                    let head = head_side.await?;
                    (base, head, head_tests_side.await?, early)
                } else {
                    tokio::try_join!(base_side, head_side, head_tests_side, transplant_side)?
                };
                let head_tests = if separate { head_tests } else { head.clone() };
                // A base read only from a record another run made is
                // trusted for what the head did not fail: a head failure
                // the record calls new, or explains, gets one fresh base
                // run first.
                let (base, base_fresh, confirmations) = if !fresh && rests_on_record(&exec, def, &base, [&head, &head_tests]) {
                    confirm_record(&exec, &store, &plan, def, base, [&head, &head_tests]).await?
                } else {
                    (base, fresh, Vec::new())
                };
                let (head, rebuild) = if separate
                    && lost_report(&base, &head)
                    && reported(&head_tests).is_some()
                {
                    let t = Instant::now();
                    let log = progress.clone().map(|log| (log, Side::Head));
                    let rebuilt =
                        rebuild(&store, &head_exec, &plan, def, head_tree, &base, head, fresh, log)
                            .await?;
                    th.ms += ms(t.elapsed());
                    rebuilt
                } else {
                    (head, None)
                };
                // A head flaky where the base passed is NEW (§1.9.3) once the
                // base has passed three times; a base that fails a rerun is
                // flaky itself.
                let base = if needs_confirmation(def, &base, &head) {
                    confirm_base(&exec, &plan.env, &plan.base, def, base_fresh, base, &head).await?
                } else {
                    base
                };
                let mut times = vec![tb, th, tt, tx];
                let transplant = match plan.transplant {
                    Some(_)
                        if !can_prove(&plan, &planned, [&base, &head, &head_tests])
                            && !shows_change(&plan, &planned) =>
                    {
                        times[3] = SideTime::of(Side::Transplant, Duration::ZERO);
                        Vec::new()
                    }
                    Some(_) if transplant_tree.is_some() => early,
                    // No test file the transplant adds is one this check
                    // runs, so it reports there what it reports on the base.
                    Some(_) if !needed => {
                        times[3] = SideTime::of(Side::Transplant, Duration::ZERO);
                        base.clone()
                    }
                    Some(tree) if tree == plan.base => {
                        times[3] = SideTime::of(Side::Transplant, Duration::ZERO);
                        base.clone()
                    }
                    Some(tree) => {
                        let (runs, t) = timed(Side::Transplant, plan.env.clone(), tree).await?;
                        times[3] = t;
                        runs
                    }
                    None => Vec::new(),
                };
                let (mut head, mut head_tests, mut transplant) = (head, head_tests, transplant);
                pair_dynamic_ids(
                    &exec,
                    &plan,
                    def,
                    &base,
                    [&mut head, &mut head_tests, &mut transplant],
                )
                .await?;
                Ok::<_, ExecError>((
                    CheckRuns {
                        planned,
                        base,
                        head,
                        head_tests,
                        separate,
                        transplant,
                        rebuild,
                        narrowed: narrowed.into_inner(),
                        confirmations,
                    },
                    times,
                ))
            });
        }
        let mut results = Vec::with_capacity(plan.checks.len());
        let mut sides: Vec<SideTime> = Vec::new();
        while let Some(joined) = runs.join_next().await {
            let (result, times) = joined.map_err(|e| VerifyError::Internal(e.to_string()))??;
            for t in times {
                match sides.iter_mut().find(|s| s.side == t.side) {
                    Some(s) => s.ms = s.ms.max(t.ms),
                    None => sides.push(t),
                }
            }
            results.push(result);
        }
        Ok((results, sides))
    }
}

pub(super) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, VerifyError> + Send + 'static,
) -> Result<T, VerifyError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| VerifyError::Internal(e.to_string()))?
}

/// How far one side's reruns go (§1.9.3).
#[derive(Debug, Clone, Copy)]
pub(super) struct Reruns {
    /// The results the side may take.
    max: usize,
    /// Whether a failure that repeats as a whole ends the side before
    /// `max` ([`rules::failure_settled`]).
    settle: bool,
}

impl Reruns {
    pub(super) const BASE: Self = Self {
        max: rules::MAX_RUNS,
        settle: true,
    };
    const HEAD: Self = Self {
        max: rules::MAX_RUNS,
        settle: false,
    };
    /// The transplant's proving tests fail there by design: a rerun would
    /// only repeat the expected failure.
    const TRANSPLANT: Self = Self {
        max: 1,
        settle: false,
    };

    /// A hunk-necessity run: once, its proving tests only (an infra
    /// ending is retried once, as on every side).
    pub(super) const ABLATION: Self = Self {
        max: 1,
        settle: false,
    };

    /// The head-tests side of a check judged by its exit code alone stops
    /// at its first result: that side only says whether the head's own
    /// tests pass (`finish::own_of`), and one failing run already makes it
    /// `fail` whatever later runs show.
    const OWN_EXIT: Self = Self {
        max: 1,
        settle: false,
    };

    pub(super) fn of(side: Side, check: &CheckDef) -> Self {
        match side {
            Side::Base => Self::BASE,
            Side::HeadTests if check.report.is_none() => Self::OWN_EXIT,
            Side::Head | Side::HeadTests => Self::HEAD,
            Side::Transplant => Self::TRANSPLANT,
            Side::Ablation => Self::ABLATION,
        }
    }
}

/// How each failing run of a side failed, in order ([`rules::Failure`]): a
/// run with its per-test report failed tests; one without stopped at a
/// compiler error when its output shows one, else failed as a whole.
pub(super) fn failures(
    store: &Store,
    runs: &[&CheckRun],
    check: &CheckDef,
) -> Result<Vec<rules::Failure>, StoreError> {
    let mut found = Vec::new();
    for run in runs.iter().filter(|r| ending(r, check) == Ending::Fail) {
        if check.report.is_some() && run.report.is_some() {
            found.push(rules::Failure::Tests);
            continue;
        }
        let output = store.get_raw(&run.output)?;
        let text = String::from_utf8_lossy(&output);
        found.push(if toolchain::build_errors(&text, 1).is_empty() {
            rules::Failure::Command {
                exit_code: run.result.exit_code().unwrap_or_default(),
                output: run.output,
            }
        } else {
            rules::Failure::Build
        });
    }
    Ok(found)
}

/// Whether a side's runs need no more under `reruns`.
pub(super) fn side_done(
    store: &Store,
    runs: &[&CheckRun],
    check: &CheckDef,
    reruns: Reruns,
) -> Result<bool, StoreError> {
    let endings: Vec<Ending> = runs.iter().map(|r| ending(r, check)).collect();
    Ok(!rules::needs_rerun(&endings, reruns.max)
        || (reruns.settle && rules::failure_settled(&failures(store, runs, check)?)))
}

/// Runs one side of one check until the rerun policy, at most
/// `reruns.max` results, is satisfied. Run indexes count results, so an
/// infra retry reuses the index it retries. Each tree installs its
/// dependency layer from its own lockfile, which `env` carries.
///
/// A side whose first result fails needs up to `reruns.max` results, so
/// the remaining reruns are asked for at once and run concurrently; their
/// results are then taken in index order under the same policy, so the side
/// keeps exactly the runs a one-at-a-time loop would (a rerun the policy
/// does not need stays in the cache, unused). A failure the next run could
/// settle (`reruns.settle`, a failure as a whole) asks for one rerun at a
/// time. With `likely_fail` (the side is expected to fail, so it will need
/// every run) all three start at once.
///
/// A head side given the base side's runs through `base` (they arrive when
/// the base side is done) is not rerun when its first run fails only where
/// the base failed every time ([`explained_by_base`]).
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_side(
    exec: &Executor,
    store: &Store,
    env: &Environment,
    snapshot: &SnapshotId,
    check: &CheckDef,
    reruns: Reruns,
    fresh: bool,
    likely_fail: bool,
    mut base: Option<BaseRuns>,
    progress: Option<(Arc<crate::progress::Log>, Side)>,
    narrow: Option<&narrow::Narrow>,
) -> Result<Vec<ExecutedRun>, ExecError> {
    let deps = snapshot;
    let progress = progress.as_ref();
    let fetch = |n: usize, check: &CheckDef| {
        let check = check.clone();
        async move {
            let check = &check;
            let index = u8::try_from(n)
                .ok()
                .and_then(|n| RunIndex::try_from(n).ok())
                .expect("the rerun policy stops at three results");
            let started = Instant::now();
            // The log hears a run wait for its slot and start; a cache hit
            // only ends.
            let watched;
            let exec = match progress {
                Some((log, side)) => {
                    let (log, side, id, n) =
                        (log.clone(), *side, check.id.to_string(), index.get());
                    watched = exec.watched(Arc::new(move |event| {
                        let kind = match event {
                            crate::exec::SlotEvent::Queued { ahead } => {
                                crate::progress::Kind::Queued { position: ahead }
                            }
                            crate::exec::SlotEvent::Started => crate::progress::Kind::Started,
                        };
                        log.push(side, &id, n, kind);
                    }));
                    &watched
                }
                None => exec,
            };
            let run = if fresh {
                exec.run_fresh_from(snapshot, deps, env, check, index).await
            } else {
                exec.run_from(snapshot, deps, env, check, index).await
            };
            if let (Some((log, side)), Ok(run)) = (progress, &run) {
                let ending = match ending(&run.run, check) {
                    Ending::Pass => "pass",
                    Ending::Fail => "fail",
                    _ => "infra",
                };
                log.push(
                    *side,
                    check.id.as_str(),
                    index.get(),
                    crate::progress::Kind::Finished {
                        ending: ending.to_owned(),
                        cached: run.cache_hit,
                        ms: ms(started.elapsed()),
                    },
                );
            }
            run
        }
    };
    let done = |runs: &[ExecutedRun]| {
        let runs: Vec<&CheckRun> = runs.iter().map(|r| &r.run).collect();
        side_done(store, &runs, check, reruns)
    };
    let mut runs: Vec<ExecutedRun> = Vec::new();
    let mut focus = None;
    loop {
        if done(&runs)? {
            return Ok(runs);
        }
        if let ([first], None, None) = (runs.as_slice(), narrow, &focus) {
            focus = focus::Focus::failing(check, &first.run);
        }
        // A focused rerun that cannot be read as the full command's runs
        // the full command instead.
        let fetch = |n: usize| {
            let focus = focus.as_ref();
            async move {
                if let Some(f) = focus {
                    if let Some(run) = f.fill(check, fetch(n, &f.def).await?) {
                        return Ok(run);
                    }
                }
                let run = fetch(n, check).await?;
                settled(check, run, |def| fetch(n, &def)).await
            }
        };
        if let ([first], Some(rx)) = (runs.as_slice(), base.as_mut()) {
            if ending(&first.run, check) == Ending::Fail {
                let explained = rx.wait_for(Option::is_some).await.is_ok_and(|b| {
                    b.as_deref()
                        .is_some_and(|b| explained_by_base(b, &first.run))
                });
                if explained {
                    return Ok(runs);
                }
                base = None;
            }
        }
        let endings: Vec<Ending> = runs.iter().map(|r| ending(&r.run, check)).collect();
        let results = endings.iter().filter(|e| !e.is_infra()).count();
        let next = results + 1;
        let last_failed = endings.last().is_some_and(|e| !e.is_infra());
        let settles_next = match runs.last() {
            Some(r) if reruns.settle && last_failed => matches!(
                failures(store, &[&r.run], check)?.as_slice(),
                [rules::Failure::Command { .. }]
            ),
            _ => false,
        };
        let first_cached = || match RunIndex::try_from(1u8) {
            Ok(first) => exec
                .cached(snapshot, env, check, first)
                .map(|r| r.is_some()),
            Err(_) => Ok(true),
        };
        let batch = if runs.is_empty()
            && likely_fail
            && reruns.max > 1
            && narrow.is_none()
            && !focus::can(check)
            && !fresh
            && !first_cached()?
        {
            let (a, b, c) = tokio::try_join!(fetch(1), fetch(2), fetch(3))?;
            vec![a, b, c]
        } else if last_failed && next < reruns.max && !settles_next {
            let (a, b) = tokio::try_join!(fetch(next), fetch(next + 1))?;
            vec![a, b]
        } else {
            vec![fetch(next).await?]
        };
        for run in batch {
            // A narrowed run is read as the full command's; one that
            // cannot be leaves the side to run in full (`Vec::new()`).
            let run = match narrow {
                Some(n) => match n.fill(store, check, run).await? {
                    Some(run) => run,
                    None => return Ok(Vec::new()),
                },
                None => run,
            };
            let expected = runs
                .iter()
                .filter(|r| !r.run.result.ending().is_infra())
                .count()
                + 1;
            if done(&runs)? || usize::from(run.run.index.get()) != expected {
                break;
            }
            runs.push(run);
        }
    }
}

/// `run` with each test it failed on pytest-xdist workers taken from a
/// serial rerun of those tests ([`focus::Focus::serial`]); `run` itself
/// when it ran serially, or the rerun cannot be read as its tests.
pub(super) async fn settled<F, Fut>(
    check: &CheckDef,
    run: ExecutedRun,
    fetch: F,
) -> Result<ExecutedRun, ExecError>
where
    F: FnOnce(CheckDef) -> Fut,
    Fut: std::future::Future<Output = Result<ExecutedRun, ExecError>>,
{
    let Some(serial) = focus::Focus::serial(check, &run.run) else {
        return Ok(run);
    };
    Ok(serial
        .fill(check, fetch(serial.def.clone()).await?)
        .unwrap_or(run))
}

/// What the head sides fail that every base run passed: failing test ids
/// when the runs report per test, else the whole command (`None` in the
/// set).
fn broken_by_head(
    check: &CheckDef,
    base: &[ExecutedRun],
    heads: [&Vec<ExecutedRun>; 2],
) -> BTreeSet<Option<TestCheckId>> {
    let mut broken = BTreeSet::new();
    if let Some(reported_base) = reported(base) {
        for run in heads.iter().flat_map(|runs| runs.iter()) {
            for t in run
                .run
                .tests
                .iter()
                .filter(|t| t.status == TestStatus::Fail)
            {
                let passed_on_base = reported_base.iter().all(|b| {
                    b.tests
                        .get(t.id.as_str())
                        .is_some_and(|bt| bt.status == TestStatus::Pass)
                });
                if passed_on_base {
                    broken.insert(Some(t.id.clone()));
                }
            }
        }
        return broken;
    }
    let base_passed = base.iter().all(|r| ending(&r.run, check) == Ending::Pass);
    let head_failed = heads
        .iter()
        .flat_map(|runs| runs.iter())
        .any(|r| ending(&r.run, check) == Ending::Fail);
    if base_passed && head_failed {
        broken.insert(None);
    }
    broken
}

/// Whether every base run came from a restored CI record and the head
/// fails something those runs passed (a `NEW` that rests on the record
/// alone) or also failed (a head failure the record alone explains).
fn rests_on_record(
    exec: &Executor,
    check: &CheckDef,
    base: &[ExecutedRun],
    heads: [&Vec<ExecutedRun>; 2],
) -> bool {
    !base.is_empty()
        && base.iter().all(|r| r.cache_hit && exec.is_recorded(&r.id))
        && (!broken_by_head(check, base, heads).is_empty()
            || !failed_on_both(check, base, heads).is_empty())
}

/// What the head sides fail that some base run failed too, in the shape
/// of [`broken_by_head`].
fn failed_on_both(
    check: &CheckDef,
    base: &[ExecutedRun],
    heads: [&Vec<ExecutedRun>; 2],
) -> BTreeSet<Option<TestCheckId>> {
    let mut explained = BTreeSet::new();
    if let Some(reported_base) = reported(base) {
        for run in heads.iter().flat_map(|runs| runs.iter()) {
            for t in run
                .run
                .tests
                .iter()
                .filter(|t| t.status == TestStatus::Fail)
            {
                let failed_on_base = reported_base.iter().any(|b| {
                    b.tests
                        .get(t.id.as_str())
                        .is_some_and(|bt| bt.status == TestStatus::Fail)
                });
                if failed_on_base {
                    explained.insert(Some(t.id.clone()));
                }
            }
        }
        return explained;
    }
    let base_failed = base.iter().any(|r| ending(&r.run, check) == Ending::Fail);
    let head_failed = heads
        .iter()
        .flat_map(|runs| runs.iter())
        .any(|r| ending(&r.run, check) == Ending::Fail);
    if base_failed && head_failed {
        explained.insert(None);
    }
    explained
}

/// One fresh base run for a head failure resting on a recorded base. When
/// it fails something the head broke, or does not fail something the
/// record failed too, the record does not hold on this runner: the base
/// side runs again here, fresh, under its rerun policy, and those runs
/// judge the entry. Otherwise the recorded runs stand. Returns the base
/// side's runs, whether they are fresh, and the fresh runs it made that
/// those runs leave out.
async fn confirm_record(
    exec: &Executor,
    store: &Store,
    plan: &Plan,
    check: &CheckDef,
    recorded: Vec<ExecutedRun>,
    heads: [&Vec<ExecutedRun>; 2],
) -> Result<(Vec<ExecutedRun>, bool, Vec<ExecutedRun>), ExecError> {
    let broken = broken_by_head(check, &recorded, heads);
    let explained = failed_on_both(check, &recorded, heads);
    let at = |index: RunIndex| {
        move |def: CheckDef| async move {
            exec.run_fresh_from(&plan.base, &plan.base, &plan.env, &def, index)
                .await
        }
    };
    let fresh = |def: CheckDef| async move {
        let run = at(RunIndex::FIRST)(def.clone()).await?;
        settled(&def, run, at(RunIndex::FIRST)).await
    };
    // Only the tests it confirms run where the runner selects them by name;
    // every other test keeps the record's result for this one question.
    let ids: Option<BTreeSet<String>> = broken
        .iter()
        .chain(&explained)
        .map(|id| id.as_ref().map(|id| id.test_path().to_owned()))
        .collect();
    let focus = match (ids, recorded.first()) {
        (Some(ids), Some(first)) => focus::Focus::on(check, &first.run, ids),
        _ => None,
    };
    let focused = match &focus {
        Some(f) => f.fill(check, fresh(f.def.clone()).await?),
        None => None,
    };
    let narrowed = focused.is_some();
    let first = match focused {
        Some(run) => run,
        None => fresh(check.clone()).await?,
    };
    exec.note_confirmation();
    let fails = |b: &Option<TestCheckId>| match b {
        Some(id) => first
            .run
            .tests
            .get(id.as_str())
            .is_some_and(|t| t.status == TestStatus::Fail),
        None => ending(&first.run, check) == Ending::Fail,
    };
    let contradicted = broken.iter().any(fails) || explained.iter().any(|e| !fails(e));
    if !contradicted {
        return Ok((recorded, false, vec![first]));
    }
    // The record does not hold here, so neither do the results a focused
    // run took from it: the base side starts again with the full command.
    let (first, left) = if narrowed {
        (fresh(check.clone()).await?, vec![first])
    } else {
        (first, Vec::new())
    };
    let mut runs = vec![first];
    loop {
        let done = {
            let refs: Vec<&CheckRun> = runs.iter().map(|r| &r.run).collect();
            side_done(store, &refs, check, Reruns::BASE)?
        };
        if done {
            return Ok((runs, true, left));
        }
        let results = runs
            .iter()
            .filter(|r| !ending(&r.run, check).is_infra())
            .count();
        let Some(index) = u8::try_from(results + 1)
            .ok()
            .and_then(|n| RunIndex::try_from(n).ok())
        else {
            return Ok((runs, true, left));
        };
        let run = at(index)(check.clone()).await?;
        runs.push(settled(check, run, at(index)).await?);
    }
}

/// Whether some entry of a check is flaky on the head over a base that
/// passed every one of fewer than three runs
/// ([`rules::base_needs_confirmation`]): per test when both sides report,
/// else for the whole command.
fn needs_confirmation(check: &CheckDef, base: &[ExecutedRun], head: &[ExecutedRun]) -> bool {
    if let (Some(b), Some(h)) = (reported(base), reported(head)) {
        return !unconfirmed(&b, &h).is_empty();
    }
    let endings = |runs: &[ExecutedRun]| -> Vec<Ending> {
        runs.iter().map(|r| ending(&r.run, check)).collect()
    };
    rules::base_needs_confirmation(&endings(base), &endings(head))
}

/// The test paths whose base outcomes do not yet settle their head
/// outcomes ([`rules::base_needs_confirmation`]).
fn unconfirmed(base: &[&CheckRun], head: &[&CheckRun]) -> BTreeSet<String> {
    let ids: BTreeSet<&TestCheckId> = head
        .iter()
        .flat_map(|r| r.tests.iter())
        .map(|t| &t.id)
        .collect();
    ids.into_iter()
        .filter(|id| {
            rules::base_needs_confirmation(&test_outcomes(base, id), &test_outcomes(head, id))
        })
        .map(|id| id.test_path().to_owned())
        .collect()
}

/// The base side's runs rerun to [`rules::MAX_RUNS`] results (an infra run
/// retried once), so a test flaky on the head is judged against a base
/// that passed three times or showed its own flakiness.
async fn confirm_base(
    exec: &Executor,
    env: &Environment,
    snapshot: &SnapshotId,
    check: &CheckDef,
    fresh: bool,
    mut runs: Vec<ExecutedRun>,
    head: &[ExecutedRun],
) -> Result<Vec<ExecutedRun>, ExecError> {
    // Only the tests that need it run again where the runner selects them.
    let focus = match (reported(&runs), reported(head), runs.first()) {
        (Some(b), Some(h), Some(first)) => focus::Focus::on(check, &first.run, unconfirmed(&b, &h)),
        _ => None,
    };
    loop {
        let endings: Vec<Ending> = runs.iter().map(|r| ending(&r.run, check)).collect();
        let results = endings.iter().filter(|e| !e.is_infra()).count();
        let ended_infra = matches!(endings.as_slice(), [.., a, b] if a.is_infra() && b.is_infra());
        if results >= rules::MAX_RUNS || ended_infra {
            return Ok(runs);
        }
        let index = u8::try_from(results + 1)
            .ok()
            .and_then(|n| RunIndex::try_from(n).ok())
            .expect("at most three results");
        let fetch = |def: &CheckDef| {
            let def = def.clone();
            async move {
                if fresh {
                    exec.run_fresh_from(snapshot, snapshot, env, &def, index)
                        .await
                } else {
                    exec.run_from(snapshot, snapshot, env, &def, index).await
                }
            }
        };
        let focused = match &focus {
            Some(f) => f.fill(check, fetch(&f.def).await?),
            None => None,
        };
        let run = match focused {
            Some(run) => run,
            None => {
                settled(check, fetch(check).await?, |def| async move {
                    fetch(&def).await
                })
                .await?
            }
        };
        runs.push(run);
    }
}

/// The head-side runs (`sides`) with each test id that differs from the
/// base's only in its parameter or subtest suffix ([`aver_core::params`])
/// renamed to the base's, when the base itself did not report that id on
/// every run: the suffix is made per run (a clock, a pointer), not by the
/// change, whether or not the head edited the file defining the test (it
/// usually adds its own test there). The base side runs once more for that
/// ([`confirm_fresh`]) unless its runs already disagree. Every other id is
/// left as it is, so a test the head did drop or rename is still missing
/// from its report.
#[allow(clippy::too_many_arguments)]
async fn pair_dynamic_ids(
    exec: &Executor,
    plan: &Plan,
    def: &CheckDef,
    base: &[ExecutedRun],
    sides: [&mut Vec<ExecutedRun>; 3],
) -> Result<(), ExecError> {
    let Some(reported_base) = reported(base) else {
        return Ok(());
    };
    let ids = |run: &CheckRun| -> BTreeSet<String> {
        run.tests.iter().map(|t| t.id.as_str().to_owned()).collect()
    };
    let base_ids: BTreeSet<String> = reported_base.iter().flat_map(|r| ids(r)).collect();
    let pairs_in = |run: &CheckRun| -> Vec<(String, String)> {
        let own = ids(run);
        let lost: Vec<&str> = base_ids.difference(&own).map(String::as_str).collect();
        let gained: Vec<&str> = own.difference(&base_ids).map(String::as_str).collect();
        aver_core::params::pair(&lost, &gained)
            .into_iter()
            .map(|(b, h)| (b.to_owned(), h.to_owned()))
            .collect()
    };
    let candidates: BTreeSet<String> = sides
        .iter()
        .flat_map(|runs| runs.iter())
        .filter(|r| r.run.report.is_some())
        .flat_map(|r| pairs_in(&r.run))
        .map(|(b, _)| b)
        .collect();
    if candidates.is_empty() {
        return Ok(());
    }
    let mut evidence: Vec<BTreeSet<String>> = reported_base.iter().map(|r| ids(r)).collect();
    let steady = |evidence: &[BTreeSet<String>]| {
        candidates
            .iter()
            .all(|b| evidence.iter().all(|run| run.contains(b)))
    };
    if steady(&evidence) {
        if let Some(extra) = confirm_fresh(exec, plan, def, reported_base.len()).await? {
            evidence.push(ids(&extra.run));
        }
    }
    let dynamic: BTreeSet<&String> = candidates
        .iter()
        .filter(|b| evidence.iter().any(|run| !run.contains(*b)))
        .collect();
    if dynamic.is_empty() {
        return Ok(());
    }
    for runs in sides {
        for r in runs.iter_mut().filter(|r| r.run.report.is_some()) {
            let renames: BTreeMap<String, String> = pairs_in(&r.run)
                .into_iter()
                .filter(|(b, _)| dynamic.contains(b))
                .map(|(b, h)| (h, b))
                .collect();
            if renames.is_empty() {
                continue;
            }
            let tests = r.run.tests.iter().map(|t| {
                let mut t = t.clone();
                if let Some(Ok(id)) = renames.get(t.id.as_str()).map(|b| b.parse()) {
                    t.id = id;
                }
                t
            });
            r.run.tests = Set::new(tests.collect()).expect("a rename pairs one id with one");
        }
    }
    Ok(())
}

/// One more run of the base side, bypassing the cache, to see whether its
/// test ids are the same from run to run; `None` when the base already has
/// every result a side may take.
async fn confirm_fresh(
    exec: &Executor,
    plan: &Plan,
    def: &CheckDef,
    results: usize,
) -> Result<Option<ExecutedRun>, ExecError> {
    let Some(index) = u8::try_from(results + 1)
        .ok()
        .and_then(|n| RunIndex::try_from(n).ok())
    else {
        return Ok(None);
    };
    let run = exec
        .run_fresh_from(&plan.base, &plan.base, &plan.env, def, index)
        .await?;
    Ok(Some(run))
}

/// A check's runs with every report complete: for a report that lists
/// failures only (`tsc`), an entry one run reports and another does not
/// passed in the other, so each run with a report gets a passing result for
/// every entry some run of the check reports. Then the per-test rules treat
/// a type error the head fixed as fixed, not as a test missing from the
/// head's report, and one it added as `NEW`. Other checks are unchanged.
pub(super) fn densify(check: &CheckRuns) -> CheckRuns {
    let format = check.planned.def.report.as_ref().map(|r| r.format);
    let failures_only = format.is_some_and(|f| f.lists_failures_only());
    // A go-json `(package)` entry exists only while its package fails, so a
    // report without it passed it: a package the head fixed is fixed, not
    // missing from the head's report. The transplant keeps its own reports,
    // whose missing tests are the proof.
    let packages = format == Some(aver_core::ReportFormat::GoJson);
    if !failures_only && !packages {
        return check.clone();
    }
    let suffix = format!(
        "{}{}",
        junit::SEPARATOR,
        crate::exec::gojson::PACKAGE_FAILED
    );
    let sides: &[&Vec<ExecutedRun>] = if packages {
        &[&check.base, &check.head, &check.head_tests]
    } else {
        &[
            &check.base,
            &check.head,
            &check.head_tests,
            &check.transplant,
        ]
    };
    let ids: BTreeSet<TestCheckId> = sides
        .iter()
        .flat_map(|runs| runs.iter())
        .filter(|r| r.run.report.is_some())
        .flat_map(|r| r.run.tests.iter().map(|t| t.id.clone()))
        .filter(|id| !packages || id.test_path().ends_with(&suffix))
        .collect();
    let fill = |runs: &[ExecutedRun]| -> Vec<ExecutedRun> {
        runs.iter()
            .map(|r| {
                if r.run.report.is_none() {
                    return r.clone();
                }
                let mut tests: Vec<aver_core::TestResult> = r.run.tests.iter().cloned().collect();
                for id in &ids {
                    if r.run.tests.get(id.as_str()).is_none() {
                        tests.push(aver_core::TestResult {
                            id: id.clone(),
                            status: TestStatus::Pass,
                            message: None,
                        });
                    }
                }
                let mut r = r.clone();
                r.run.tests = Set::new(tests).expect("ids are distinct");
                r
            })
            .collect()
    };
    CheckRuns {
        planned: check.planned.clone(),
        base: fill(&check.base),
        head: fill(&check.head),
        head_tests: fill(&check.head_tests),
        separate: check.separate,
        transplant: if packages {
            check.transplant.clone()
        } else {
            fill(&check.transplant)
        },
        rebuild: check.rebuild.clone(),
        narrowed: check.narrowed.clone(),
        confirmations: check.confirmations.clone(),
    }
}

/// The base side's runs of one check, published to its head sides once the
/// base side is done.
pub(super) type BaseRuns = tokio::sync::watch::Receiver<Option<Arc<Vec<ExecutedRun>>>>;

/// Whether a head side's failing first run fails only where the base failed
/// on every run: it carries a report with at least one failing test, every
/// base run carries a report, and each test failing on the head failed in
/// every base run. Those entries are `PREEXISTING` after one head run, and
/// reruns could only make them `FLAKY` (head runs disagreeing), which
/// changes no verdict, so the side keeps its one run (§1.9.3). The base's
/// own reruns are what showed those failures to be stable.
fn explained_by_base(base: &[ExecutedRun], head: &CheckRun) -> bool {
    let Some(base) = reported(base) else {
        return false;
    };
    let mut failing = head
        .tests
        .iter()
        .filter(|t| t.status == TestStatus::Fail)
        .peekable();
    head.report.is_some()
        && failing.peek().is_some()
        && failing.all(|failed| {
            base.iter().all(|run| {
                run.tests
                    .iter()
                    .any(|t| t.id == failed.id && t.status == TestStatus::Fail)
            })
        })
}

/// The judged head's runs, and what probing its build found, when `head`
/// lost the report (§1.9.3). The head's runs over the test targets that
/// build when only base test files it modified or deleted, restored onto
/// it, stopped the build; [`Rebuild::Lost`] when Aver lost the probe's
/// result. `head` unchanged and no rebuild for a check that is not a plain
/// `cargo test` ([`crate::exec::cargo`]), a build stopped by anything
/// else, or a rerun that still has no report.
#[allow(clippy::too_many_arguments)]
pub(super) async fn rebuild(
    store: &Store,
    exec: &Executor,
    plan: &Plan,
    def: &CheckDef,
    tree: SnapshotId,
    base: &[ExecutedRun],
    head: Vec<ExecutedRun>,
    fresh: bool,
    progress: Option<(Arc<crate::progress::Log>, Side)>,
) -> Result<(Vec<ExecutedRun>, Option<Rebuild>), ExecError> {
    use crate::exec::cargo::{CargoTest, Probe};
    let Some(cargo) = CargoTest::of(def) else {
        return Ok((head, None));
    };
    let probe_def = cargo.probe();
    let first = RunIndex::try_from(1u8).expect("1 is a run index");
    let probe = if fresh {
        exec.run_fresh_from(&tree, &tree, &plan.head_env, &probe_def, first)
            .await?
    } else {
        exec.run_from(&tree, &tree, &plan.head_env, &probe_def, first)
            .await?
    };
    let stream = match probe.run.report {
        Some(report) if !probe.run.result.ending().is_infra() => store.get_raw(&report)?,
        _ => return Ok((head, Some(Rebuild::Lost(probe)))),
    };
    let found = Probe::read(&String::from_utf8_lossy(&stream));
    if found.success.is_none() {
        return Ok((head, Some(Rebuild::Lost(probe))));
    }
    let restored: BTreeSet<String> = plan
        .tests
        .modified
        .iter()
        .chain(&plan.tests.deleted)
        .map(|p| p.as_str().to_owned())
        .collect();
    let cwd = def.cwd.as_ref().map(|p| p.as_str());
    let (Some(unbuilt), Some(over)) = (found.restored_only(cwd, &restored), cargo.over(&found))
    else {
        return Ok((head, None));
    };
    let (_tx, gate) = tokio::sync::watch::channel(Some(Arc::new(base.to_vec())));
    let runs = run_side(
        exec,
        store,
        &plan.head_env,
        &tree,
        &over,
        Reruns::HEAD,
        fresh,
        false,
        Some(gate),
        progress,
        None,
    )
    .await?;
    if reported(&runs).is_none() {
        return Ok((head, None));
    }
    let rebuilt = Rebuilt {
        probe,
        lost: head,
        unbuilt,
    };
    Ok((runs, Some(Rebuild::Rebuilt(rebuilt))))
}
