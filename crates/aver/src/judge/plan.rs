//! What a verification runs: the plan (checks, judging split, tests the
//! head changed) and the checks a change affects.

use super::*;

/// Resolves what a verification judges (§2.5.1), from the store and the
/// workspaces alone: the verifier's first step.
#[derive(Clone)]
pub(super) struct Planner {
    store: Arc<Store>,
    workspaces: Arc<Workspaces>,
}

impl Planner {
    pub(super) fn new(store: Arc<Store>, workspaces: Arc<Workspaces>) -> Self {
        Self { store, workspaces }
    }

    pub(super) fn plan(&self, caller: &AgentId, req: &Request) -> Result<Plan, VerifyError> {
        let task: Task = self.store.get(&req.task).map_err(|e| match e {
            StoreError::NotFound(_) => {
                VerifyError::NotFound(format!("task {} not found", req.task))
            }
            other => other.into(),
        })?;
        let attempt = req
            .attempt
            .map(|id| -> Result<_, VerifyError> {
                let attempt = self
                    .store
                    .read(|conn| tasks::load_attempt(conn, &id))?
                    .ok_or_else(|| VerifyError::NotFound(format!("attempt {id} not found")))?;
                tasks::check_attempt_owner(&attempt, caller)?;
                tasks::check_attempt_open(&attempt)?;
                if attempt.task != req.task {
                    return Err(VerifyError::Invalid(format!(
                        "attempt {id} is for task {}, not {}",
                        attempt.task, req.task
                    )));
                }
                Ok(attempt)
            })
            .transpose()?;
        let mut attempt = attempt;
        let (head, base) = match &req.head {
            Head::Workspace(ws) => {
                if let Some(attempt) = attempt.as_ref().filter(|a| a.workspace != *ws) {
                    return Err(VerifyError::Invalid(format!(
                        "attempt {} edits workspace {}, not {ws}",
                        attempt.id, attempt.workspace
                    )));
                }
                let state = self.workspaces.get(caller, ws)?;
                self.own_task(&state, &req.task)?;
                if attempt.is_none() {
                    attempt = self.workspace_attempt(caller, &state, &req.task)?;
                }
                (self.workspaces.snapshot(caller, ws)?, state.base)
            }
            Head::Snapshot { head, base } => {
                let default = attempt.as_ref().map_or(task.body.base, |a| a.base);
                (*head, base.unwrap_or(default))
            }
        };
        // Accepting trusts an attempt's receipts, so every one of them must
        // judge the head against the attempt's own base (§2.6).
        if let Some(attempt) = attempt.as_ref().filter(|a| a.base != base) {
            return Err(VerifyError::Invalid(format!(
                "attempt {} is on base {}, not {base}; verify against its base or without the attempt",
                attempt.id, attempt.base
            )));
        }
        let head_snap = self.snapshot(&head)?;
        let base_snap = self.snapshot(&base)?;
        let governing = crate::env::governing(&self.store, &base_snap, &head_snap)?;
        let env_id = governing.id;
        let env: Arc<Environment> = Arc::new(governing.env);
        if env.checks.get(JUDGING_CHECK).is_some() {
            return Err(VerifyError::Invalid(format!(
                "the base environment defines a check {JUDGING_CHECK:?}, which judging-file entries reserve"
            )));
        }
        if env.checks.get(BUILT_CHECK).is_some() {
            return Err(VerifyError::Invalid(format!(
                "the base environment defines a check {BUILT_CHECK:?}, which built test file entries reserve"
            )));
        }
        let mut checks: Vec<Planned> = env
            .checks
            .iter()
            .map(|def| Planned {
                def: def.clone(),
                source: CheckSource::Env,
                proves: Set::empty(),
            })
            .collect();
        for def in task.body.checks.iter() {
            if env.checks.get(def.id.as_str()).is_some() {
                return Err(VerifyError::Invalid(format!(
                    "task check {} collides with a check of the base environment",
                    def.id
                )));
            }
            if def.id.as_str() == JUDGING_CHECK {
                return Err(VerifyError::Invalid(format!(
                    "task check {JUDGING_CHECK:?} collides with judging-file entries"
                )));
            }
            if def.id.as_str() == BUILT_CHECK {
                return Err(VerifyError::Invalid(format!(
                    "task check {BUILT_CHECK:?} collides with built test file entries"
                )));
            }
            // A task check runs in the base environment and mounts the
            // layers a check of its image and `cwd` mounts by default.
            let mut check = task_check(def);
            check.deps = crate::env::default_deps(
                &check,
                &env.image,
                env.lockfile.as_ref(),
                env.layers.as_slice(),
            );
            checks.push(Planned {
                def: check,
                source: CheckSource::Task,
                proves: def.proves.clone(),
            });
        }
        if checks.is_empty() && req.adhoc.is_none() {
            return Err(VerifyError::NotChecked(crate::suggest::not_checked(
                &*self.store,
                &head_snap.root,
            )));
        }
        let mut changed = self.workspaces.changed_files(&base, &head)?;
        let proposed = matches!(
            governing.approval,
            Some(aver_core::Approval::Proposed { .. })
        );
        if proposed {
            changed.retain(|(path, _, _)| path.as_str() != crate::env::CONFIG_PATH);
        }
        let defs = checks.iter().map(|c| &c.def).collect::<Vec<_>>();
        let base_judge = crate::env::base_judge(&self.store, &base_snap.root)?;
        // A sensitive path is accounted for by its own CHANGED entry: a
        // person reviews the edit, which is what an uncovered file asks for.
        let mut uncovered =
            crate::coverage::uncovered(&env, &defs, &changed, governing.approval.is_some());
        let mut unbuilt = crate::cargo_features::uncompiled(
            &env,
            &defs,
            &changed,
            || crate::place::workdir::tree_files(&self.store, &head_snap.root),
            || crate::place::workdir::tree_files(&self.store, &base_snap.root),
            |blob| self.store.get_raw(blob),
        )?;
        unbuilt.retain(|u| base_judge.sensitive_match(&u.path).is_none());
        uncovered.extend(unbuilt.iter().filter_map(|u| u.path.parse().ok()));
        uncovered.sort();
        uncovered.dedup();
        uncovered.retain(|path| base_judge.sensitive_match(path.as_str()).is_none());
        let protected = if task.body.protected_paths.is_empty() {
            Vec::new()
        } else {
            if env.checks.get(PROTECTED_CHECK).is_some() {
                return Err(VerifyError::Invalid(format!(
                    "the base environment defines a check {PROTECTED_CHECK:?}, which the task's protected paths reserve"
                )));
            }
            protected_entries(&task, &changed)
        };
        let leased = match &req.head {
            Head::Workspace(ws) => {
                let coord = crate::coord::Coord::new(self.store.clone(), self.workspaces.clone());
                let held = coord.binding_exclusive(caller, ws).map_err(coord_error)?;
                let entries = lease_entries(&held, &changed);
                let reserved = env.checks.get(LEASE_CHECK).is_some()
                    || checks.iter().any(|c| c.def.id.as_str() == LEASE_CHECK);
                if reserved && !entries.is_empty() {
                    return Err(VerifyError::Invalid(format!(
                        "a check is named {LEASE_CHECK:?}, which entries for edits under other tasks' leases reserve"
                    )));
                }
                entries
            }
            Head::Snapshot { .. } => Vec::new(),
        };
        let sensitive = sensitive_entries(&base_judge, &changed);
        let reserved = env.checks.get(SENSITIVE_CHECK).is_some()
            || checks.iter().any(|c| c.def.id.as_str() == SENSITIVE_CHECK);
        if reserved && !sensitive.is_empty() {
            return Err(VerifyError::Invalid(format!(
                "a check is named {SENSITIVE_CHECK:?}, which entries for changes to sensitive paths reserve"
            )));
        }
        let dirs = judging::harness_dirs(checks.iter().map(|c| c.def.cwd.as_ref()));
        let split = judging::split(&env, &dirs, &changed, |blob| self.store.get_raw(blob))?;
        // Affected checks (§1.9.3): an environment check that nothing the
        // head changed could affect reports on the head what it reports on
        // the base, so it runs on neither side. Task checks always run, and
        // so does everything when nothing changed: that report is about the
        // original code itself.
        if let Some(only) = req.ci.as_ref().and_then(|ci| ci.only.as_ref()) {
            if !checks.iter().any(|c| c.def.id == *only) {
                let known: Vec<&str> = checks.iter().map(|c| c.def.id.as_str()).collect();
                return Err(VerifyError::Invalid(format!(
                    "--check {only}: the base has no such check; its checks are {}",
                    known.join(", ")
                )));
            }
            checks.retain(|c| c.def.id == *only);
        }
        let skipped: Vec<CheckId> = if req.all || changed.is_empty() {
            Vec::new()
        } else {
            checks
                .iter()
                .filter(|c| c.source == CheckSource::Env)
                .filter(|c| {
                    !changed.iter().any(|(path, at_base, _)| {
                        crate::coverage::affects(&env, &dirs, &c.def, path, at_base.is_some())
                    })
                })
                .map(|c| c.def.id.clone())
                .collect()
        };
        checks.retain(|c| !skipped.contains(&c.def.id));
        // `aver check -- <cmd>` (§2.7.1): the agent's command is the only
        // check, in the base environment's place for its toolchain, and it
        // runs whatever the change touched.
        if let Some(def) = &req.adhoc {
            checks = vec![Planned {
                def: crate::adhoc::fit(def, &env),
                source: CheckSource::Task,
                proves: Set::empty(),
            }];
        }
        // Hunk necessity (§1.9.4) reverts hunks of the source files the
        // change touched that a check covers and that judge nothing: not
        // tests, not the judging set, not sensitive paths.
        let declared = governing.approval.is_some();
        let sources = changed
            .iter()
            .filter(|(raw, at_base, at_head)| {
                let file = |f: &Option<FileEntry>| f.is_some_and(|f| f.kind == FileKind::File);
                file(at_head)
                    && (at_base.is_none() || file(at_base))
                    && matches!(
                        crate::coverage::kind(raw),
                        crate::coverage::Kind::Source(_) | crate::coverage::Kind::OtherSource
                    )
                    && judging::why(&env, &dirs, raw, at_base.is_some()).is_none()
                    && !env.is_test_file(raw)
                    && base_judge.sensitive_match(raw).is_none()
                    && checks
                        .iter()
                        .any(|c| crate::coverage::covers(&env, &c.def, raw, declared))
            })
            .filter_map(|(raw, at_base, at_head)| {
                Some(Source {
                    path: raw.parse().ok()?,
                    base: at_base.map(|f| f.blob),
                    head: at_head.as_ref()?.blob,
                })
            })
            .collect();
        let harness = self.contents(&split.harness)?;
        let mut judged = harness.clone();
        judged.extend(self.contents(&split.base_tests)?);
        let config_path: Path = crate::env::CONFIG_PATH.parse().expect("a valid path");
        let config = match crate::env::lookup(&*self.store, &base_snap.root, &config_path)? {
            Some(_) => ConfigSource::AverToml,
            None => ConfigSource::Default,
        };
        let head_env = Arc::new(self.head_environment(&env, &head_snap)?);
        let head_env_id = self.store.put(&*head_env)?;
        let judged_head = self
            .workspaces
            .derive_snapshot(&head_snap.root, &judged, head_env_id)?;
        let judged_base_tests = if split.added_tests.is_empty() {
            judged_head
        } else {
            let mut without = judged.clone();
            without.extend(self.contents(&split.added_tests)?);
            self.workspaces
                .derive_snapshot(&head_snap.root, &without, head_env_id)?
        };
        let head_tests = self
            .workspaces
            .derive_snapshot(&head_snap.root, &harness, head_env_id)?;
        let transplant_files: Vec<String> = split
            .transplant
            .iter()
            .map(|(p, _)| p.as_str().to_owned())
            .collect();
        // The transplant runs in the base environment, so its snapshot
        // carries the base's: its id, and so its runs' cache keys, depend
        // only on the base and the head's test files. With the head's
        // environment, a head that changed its lockfile would rerun an
        // unchanged transplant on every verify.
        let transplant = if split.judging.is_empty() {
            let changes = self.contents(&split.transplant)?;
            Some(
                self.workspaces
                    .derive_snapshot(&base_snap.root, &changes, env_id)?,
            )
        } else {
            None
        };
        Ok(Plan {
            task_id: req.task,
            attempt: attempt.map(|a| a.id),
            task,
            base,
            head,
            judged_head,
            judged_base_tests,
            head_tests,
            env,
            env_id,
            head_env,
            env_changed: head_snap.env != base_snap.env,
            checks,
            protected,
            leased,
            sensitive,
            judging: split.judging,
            tests: split.tests,
            retargeted: split.retargeted,
            deleted_sources: split.deleted_sources,
            config,
            proposed,
            reason: req
                .reason
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .map(|r| {
                    let mut end = r.len().min(crate::diagnostics::MAX_REASON_BYTES);
                    while !r.is_char_boundary(end) {
                        end -= 1;
                    }
                    r[..end].to_owned()
                }),
            ci: req.ci.clone(),
            transplant,
            transplant_files,
            uncovered,
            unbuilt,
            skipped,
            sources,
            all: req.all,
        })
    }

    /// File entries for `contents`, storing new contents as regular files.
    fn contents(
        &self,
        contents: &[(Path, Content)],
    ) -> Result<Vec<(Path, Option<FileEntry>)>, VerifyError> {
        contents
            .iter()
            .map(|(path, content)| {
                let entry = match content {
                    Content::Entry(entry) => *entry,
                    Content::Bytes(bytes) => Some(FileEntry {
                        kind: FileKind::File,
                        blob: self.store.put_blob(bytes)?,
                    }),
                };
                Ok((path.clone(), entry))
            })
            .collect()
    }

    /// The base environment with the head's lockfiles (at the base's
    /// lockfile path, else `package-lock.json`, and at each layer's), which
    /// head-side runs install from. A layer whose lockfile the head deleted
    /// is dropped, and so are checks' references to it.
    fn head_environment(
        &self,
        env: &Environment,
        head: &Snapshot,
    ) -> Result<Environment, VerifyError> {
        let blob_at = |path: &Path| -> Result<Option<BlobId>, VerifyError> {
            Ok(match crate::env::lookup(&*self.store, &head.root, path)? {
                Some(Node::File(blob) | Node::Exec(blob)) => Some(blob),
                _ => None,
            })
        };
        let path = judging::lockfile_path(env);
        let lockfile = blob_at(&path)?.map(|blob| Lockfile { path, blob });
        let mut layers: Vec<Layer> = Vec::new();
        for layer in env.layers.iter() {
            if let Some(blob) = blob_at(&layer.lockfile.path)? {
                layers.push(Layer {
                    lockfile: Lockfile {
                        path: layer.lockfile.path.clone(),
                        blob,
                    },
                    ..layer.clone()
                });
            }
        }
        let layers = Set::new(layers).expect("the base's layers are distinct");
        let known = |p: &Path| {
            lockfile.as_ref().is_some_and(|l| l.path == *p) || layers.get(p.as_str()).is_some()
        };
        let default: Vec<&str> = lockfile.iter().map(|l| l.path.as_str()).collect();
        let checks: Vec<CheckDef> = env
            .checks
            .iter()
            .map(|c| {
                let mut c = c.clone();
                if let Some(deps) = &c.deps {
                    let kept: Vec<Path> = deps.iter().filter(|p| known(p)).cloned().collect();
                    let names: Vec<&str> = kept.iter().map(Path::as_str).collect();
                    c.deps = (names != default)
                        .then(|| Set::new(kept).expect("a subset of a set is a set"));
                }
                c
            })
            .collect();
        let checks = Set::new(checks).expect("the base's checks are distinct");
        Ok(Environment {
            schema: EnvSchema::required(&env.tests, &env.install, &checks, &layers),
            lockfile,
            layers,
            checks,
            ..env.clone()
        })
    }

    /// The attempt a workspace belongs to, when it is the caller's, open and
    /// for `task`. Verifying a task workspace without naming its attempt
    /// still records the receipt there, so `decide` finds it.
    /// Refuses to judge a workspace that belongs to an attempt against any
    /// task but that attempt's: a directory's `.aver/workspace.toml` is the
    /// agent's to edit, so its `task` line is never what decides.
    fn own_task(&self, ws: &crate::workspace::Workspace, task: &TaskId) -> Result<(), VerifyError> {
        let Some(id) = ws.attempt else {
            return Ok(());
        };
        let Some(attempt) = self.store.read(|conn| tasks::load_attempt(conn, &id))? else {
            return Ok(());
        };
        if attempt.task == *task {
            return Ok(());
        }
        Err(VerifyError::Invalid(format!(
            "workspace {} belongs to task {}, not {task}; a workspace is verified only against its own task. If .aver/workspace.toml was edited, put back its task line: task = \"{}\"",
            ws.id, attempt.task, attempt.task
        )))
    }

    fn workspace_attempt(
        &self,
        caller: &AgentId,
        ws: &crate::workspace::Workspace,
        task: &TaskId,
    ) -> Result<Option<tasks::Attempt>, VerifyError> {
        let Some(id) = ws.attempt else {
            return Ok(None);
        };
        let attempt = self.store.read(|conn| tasks::load_attempt(conn, &id))?;
        Ok(attempt.filter(|a| {
            a.agent == *caller && a.task == *task && a.state == tasks::AttemptState::Open
        }))
    }

    pub(super) fn snapshot(&self, id: &SnapshotId) -> Result<Arc<Snapshot>, VerifyError> {
        Ok(self.workspaces.get_snapshot(id)?)
    }
}

/// The receipt's `skipped` list for checks the change does not affect.
pub(super) fn skipped_checks(checks: &[CheckId]) -> Option<Set<Skipped>> {
    let list: Vec<Skipped> = checks
        .iter()
        .map(|check| Skipped {
            check: check.clone(),
            reason: SkipReason::NotAffected,
        })
        .collect();
    (!list.is_empty()).then(|| Set::new(list).expect("check ids are distinct"))
}

/// Whether a check's entries can prove a criterion, so it runs on the
/// transplant: a task check with a `proves` list, or a check one of whose
/// tests (on any side) carries a tag of this task's criteria.
pub(super) fn can_prove(plan: &Plan, planned: &Planned, sides: [&[ExecutedRun]; 3]) -> bool {
    let criteria = &plan.task.body.criteria;
    !planned.proves.is_empty()
        || sides.iter().flat_map(|runs| runs.iter()).any(|r| {
            r.run
                .tests
                .iter()
                .any(|t| !proves(&Set::empty(), t.id.test_path(), criteria).is_empty())
        })
}

/// Whether a check needs runs of its own on the transplant (§2.5.1): a
/// task check does, since it may name any file; an environment check does
/// when a test file the transplant adds or changes is in its scope (its
/// `covers`, else its `cwd`) and, when its command names test globs, matches
/// one. Otherwise the transplant differs from the base only in files the
/// check does not run, and by the `covers` rule the base's runs are its
/// runs there.
pub(super) fn transplant_needed(plan: &Plan, planned: &Planned) -> bool {
    if planned.source == CheckSource::Task {
        return true;
    }
    let def = &planned.def;
    let cwd = def.cwd.as_ref().map_or("", Path::as_str);
    let globs = command_globs(def);
    plan.transplant_files.iter().any(|file| {
        let in_scope = if def.covers.is_empty() {
            crate::env::within(file, cwd)
        } else {
            def.covers.iter().any(|g| g.covers(file))
        };
        in_scope
            && globs
                .as_ref()
                .is_none_or(|g| g.iter().any(|g| g.covers(file)))
    })
}

/// The test globs a check's command names (`node --test 'test/**/*.test.ts'`),
/// relative to the repository; `None` when it names none, or one Aver
/// cannot read as a path glob (a shell script, an absolute path), so every
/// file in the check's scope counts.
pub(super) fn command_globs(def: &CheckDef) -> Option<Vec<aver_core::PathGlob>> {
    let cwd = def.cwd.as_ref().map(Path::as_str);
    let mut globs = Vec::new();
    for arg in def.argv.iter().skip(1) {
        if arg.starts_with('-') || !arg.contains('*') {
            continue;
        }
        if arg.contains(char::is_whitespace) || arg.starts_with('/') {
            return None;
        }
        let relative = arg.trim_start_matches("./");
        let full = match cwd {
            Some(cwd) => format!("{cwd}/{relative}"),
            None => relative.to_owned(),
        };
        globs.push(full.parse().ok()?);
    }
    (!globs.is_empty()).then_some(globs)
}

/// Whether a check runs on the transplant to show the change even when it
/// proves nothing: it reports per test and the head added or changed test
/// files, whose tests then fail there or not (§1.9.4).
pub(super) fn shows_change(plan: &Plan, planned: &Planned) -> bool {
    planned.def.report.is_some() && !plan.transplant_files.is_empty()
}
