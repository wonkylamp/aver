//! In-process services over a temporary `$AVER_HOME` with the scripted
//! sandbox: workspaces, the executor, the verifier and tasks, wired the way
//! `Services::open` wires them.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::sync::Arc;

use aver::agents::{self, AgentKind, Capability, NewAgent};
use aver::env::{StaticResolver, TreeEnv};
use aver::exec::seam::Sandbox;
use aver::exec::{ExecConfig, Executor};
use aver::keys::{self, AgentKey, Home};
use aver::store::Store;
use aver::tasks::Tasks;
use aver::verify::{Head, Request, Verified, Verifier};
use aver::workspace::{EditOp, EditRequest, Workspaces};
use aver_core::*;
use tempfile::TempDir;

use crate::common::put_files;
use crate::scripted::Scripted;

pub const PINNED_IMAGE: &str =
    "node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392";

/// An `aver.toml` with one JUnit check `unit` scripted by `tests.json`.
pub fn unit_toml(extra: &str) -> String {
    format!(
        "[env]\nimage = \"{PINNED_IMAGE}\"\n\n[[check]]\nid = \"unit\"\nrun = [\"junit\", \"unit.xml\", \"tests.json\"]\nreport = {{ format = \"junit\", path = \"/aver/out/unit.xml\" }}\n{extra}"
    )
}

/// A contract running a Cargo workspace's tests, as `aver check -- cargo
/// test --workspace` writes it.
pub const CARGO_CONTRACT: &str = "[[check]]\nid = \"cargo\"\nrun = \"CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test --workspace --no-fail-fast\"\nreport = \"libtest\"\nmin_tests = 1\n";
/// A contract running a Go module's tests.
pub const GO_CONTRACT: &str =
    "[[check]]\nid = \"go\"\nrun = \"go test -json ./...\"\nreport = \"go-json\"\nmin_tests = 1\n";

pub struct World {
    pub dir: TempDir,
    pub home: Home,
    pub store: Arc<Store>,
    pub server: AgentKey,
    pub admin: AgentKey,
    pub executor: Executor,
    pub workspaces: Arc<Workspaces>,
    pub verifier: Verifier,
    pub tasks: Tasks,
}

impl World {
    pub fn new() -> Self {
        Self::with_sandbox(Arc::new(Scripted::default()))
    }

    pub fn with_sandbox(sandbox: Arc<dyn Sandbox>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::new(dir.path().join("home"));
        let init = keys::init(&home).unwrap();
        let store = Arc::new(home.open_store().unwrap());
        let server_key = Arc::new(init.server.signing_key.clone());
        let workspaces = Arc::new(Workspaces::new(
            store.clone(),
            server_key.clone(),
            Arc::new(TreeEnv(
                Arc::new(StaticResolver::default()),
                aver::env::Detection::Off,
            )),
        ));
        let executor = Executor::new(store.clone(), sandbox, ExecConfig::default());
        let verifier = Verifier::new(
            store.clone(),
            workspaces.clone(),
            executor.clone(),
            server_key.clone(),
        );
        let tasks = Tasks::new(store.clone(), workspaces.clone(), server_key);
        Self {
            dir,
            home,
            store,
            server: init.server,
            admin: init.admin,
            executor,
            workspaces,
            verifier,
            tasks,
        }
    }

    pub fn agent(&self, name: &str, caps: &[Capability]) -> AgentKey {
        let key = AgentKey::generate(name.parse().unwrap());
        agents::register(
            &self.store,
            NewAgent {
                public_key: key.signing_key.verifying_key(),
                name: key.name.clone(),
                kind: AgentKind::Agent,
                capabilities: caps.iter().copied().collect::<BTreeSet<_>>(),
                issued_by: Some(self.admin.id()),
            },
        )
        .unwrap();
        key
    }

    /// A snapshot of `files` under the environment its `aver.toml` gives.
    pub fn snapshot(&self, files: &[(&str, &str)]) -> SnapshotId {
        self.snapshot_of_root(put_files(&self.store, files))
    }

    /// A snapshot of `files` under `contract` as their `aver.toml`, unless
    /// they have one of their own.
    pub fn snapshot_under(&self, contract: &str, files: &[(&str, &str)]) -> SnapshotId {
        let mut files = files.to_vec();
        if !files.iter().any(|(p, _)| *p == "aver.toml") {
            files.push(("aver.toml", contract));
        }
        self.snapshot(&files)
    }

    /// A snapshot of the directory `dir` under the environment its
    /// `aver.toml` gives.
    pub fn snapshot_of_dir(&self, dir: &std::path::Path) -> SnapshotId {
        self.snapshot_of_root(crate::common::put_dir(&self.store, dir))
    }

    fn snapshot_of_root(&self, root: TreeId) -> SnapshotId {
        let env = aver::env::environment(
            &*self.store,
            &root,
            &StaticResolver::default(),
            aver::env::Detection::Off,
        )
        .unwrap();
        self.store.put(&env).unwrap();
        self.store.put(&Snapshot::new(root, env.id())).unwrap()
    }

    /// A task over `criteria`; none makes a regression-only `aver.task/v2`.
    pub fn task_body(&self, author: &AgentKey, base: SnapshotId, criteria: &[&str]) -> TaskBody {
        let criteria = Criteria::new(
            criteria
                .iter()
                .map(|id| Criterion {
                    id: id.parse().unwrap(),
                    text: format!("criterion {id}"),
                })
                .collect(),
        )
        .unwrap();
        TaskBody {
            schema: TaskSchema::for_task(&criteria, &Set::empty()),
            title: "Implement divide".parse().unwrap(),
            intent: "Make divide work".parse().unwrap(),
            base,
            target: None,
            criteria,
            checks: Set::empty(),
            protected_paths: Set::empty(),
            author: author.id(),
            created_at: Timestamp::now(),
        }
    }

    pub fn create_task(&self, body: TaskBody) -> TaskId {
        let author = if body.author == self.admin.id() {
            &self.admin.signing_key
        } else {
            panic!("create_task signs as the admin only")
        };
        self.tasks.create(&Task::sign(body, author)).unwrap()
    }

    pub async fn verify(
        &self,
        caller: &AgentKey,
        task: TaskId,
        head: SnapshotId,
        base: SnapshotId,
    ) -> Verified {
        self.verifier
            .verify(
                caller.id(),
                Request {
                    task,
                    head: Head::Snapshot {
                        head,
                        base: Some(base),
                    },
                    attempt: None,
                    fresh: false,
                    all: false,
                    progress: None,
                    ci: None,
                    adhoc: None,
                    reason: None,
                },
            )
            .await
            .unwrap()
    }

    /// Writes `files` into workspace `ws` as one batch.
    pub fn write(&self, owner: &AgentKey, ws: &WorkspaceId, files: &[(&str, &str)]) {
        let version = self.workspaces.get(&owner.id(), ws).unwrap().version;
        let ops = files
            .iter()
            .map(|(path, content)| {
                serde_json::from_value::<EditOp>(serde_json::json!({
                    "op": "write", "path": path, "content": content
                }))
                .unwrap()
            })
            .collect();
        self.workspaces
            .edit(
                &owner.id(),
                ws,
                EditRequest {
                    if_version: version,
                    ops,
                },
            )
            .unwrap();
    }
}

/// Entries of a receipt as `id -> (base, head, cause)` strings, for compact asserts.
pub fn summary(receipt: &Receipt) -> Vec<(String, String, String, String)> {
    receipt
        .body
        .checks
        .iter()
        .map(|c| {
            let json = serde_json::to_value(c).unwrap();
            (
                c.id.to_string(),
                json["base"].as_str().unwrap().to_owned(),
                json["head"].as_str().unwrap().to_owned(),
                json["cause"].as_str().unwrap_or("null").to_owned(),
            )
        })
        .collect()
}

pub fn entry<'a>(receipt: &'a Receipt, id: &str) -> &'a ReceiptCheck {
    receipt
        .body
        .checks
        .get(id)
        .unwrap_or_else(|| panic!("no entry {id} in {:#?}", summary(receipt)))
}

pub fn proves(check: &ReceiptCheck) -> Vec<String> {
    check.proves.iter().map(ToString::to_string).collect()
}
