//! The services one `$AVER_HOME` offers, opened in each process that uses
//! them (ARCHITECTURE.md §2.7): the store, the server key that signs
//! receipts, workspaces, coordination, tasks and the verifier, whose checks
//! run in slots on this machine (§2.4.6). [`crate::run::api::Api`] calls
//! them.

use std::sync::Arc;

use aver_core::{ProvenanceMode, SigningKey};

use crate::agents::AgentName;
use crate::coord::Coord;
use crate::env::{Detection, DockerResolver, ImageResolver, TreeEnv};
use crate::exec::{ExecConfig, Executor};
use crate::git::SecretPolicy;
use crate::keys::{AgentKey, Home, KeyError, ADMIN_KEY, SERVER_KEY};
use crate::place::runner::{RunnerConfig, SlotRunner};
use crate::recover::Running;
use crate::store::Store;
use crate::tasks::Tasks;
use crate::verify::Verifier;
use crate::workspace::Workspaces;

/// The key named `name` in `home`, or the bootstrap admin's.
pub fn key_in(home: &Home, name: Option<&AgentName>) -> Result<AgentKey, KeyError> {
    let admin: AgentName = ADMIN_KEY.parse()?;
    AgentKey::read(&home.key_path(name.unwrap_or(&admin)))
}

pub struct Services {
    pub home: Home,
    pub store: Arc<Store>,
    pub server_key: Arc<SigningKey>,
    pub workspaces: Arc<Workspaces>,
    pub coord: Arc<Coord>,
    pub tasks: Arc<Tasks>,
    pub verifier: Verifier,
    /// What Git import pins environments with.
    pub images: Arc<dyn ImageResolver>,
    /// What a possible secret does to an import.
    pub import_secrets: SecretPolicy,
    /// This process's record in `<home>/running` (§2.7.3), while the
    /// services are open.
    _running: Option<Running>,
}

impl Services {
    /// The home's services, with checks run in this machine's slots,
    /// after recovering what processes that died with them open left
    /// behind ([`crate::recover::on_open`]).
    pub fn open(home: &Home) -> Result<Self, KeyError> {
        let server = AgentKey::read(&home.key_path(&SERVER_KEY.parse()?))?;
        let store = Arc::new(home.open_store()?);
        let (running, _) =
            crate::recover::on_open(home, &store).map_err(|source| KeyError::Io {
                path: home.root().join("running"),
                source,
            })?;
        let (executor, images) = Self::runner(home, &store);
        let mut services = Self::new(home, store, Arc::new(server.signing_key), images, executor);
        services._running = Some(running);
        Ok(services)
    }

    #[cfg(not(feature = "scripted"))]
    fn runner(home: &Home, store: &Arc<Store>) -> (Executor, Arc<dyn ImageResolver>) {
        Self::slots(home, store)
    }

    /// Under the `scripted` feature, `AVER_SCRIPTED=1` runs every check
    /// through the scripted runner, logging to the home, and resolves no
    /// images, as the tests' in-process services do.
    #[cfg(feature = "scripted")]
    fn runner(home: &Home, store: &Arc<Store>) -> (Executor, Arc<dyn ImageResolver>) {
        if std::env::var_os("AVER_SCRIPTED").is_some_and(|v| v == "1") {
            let runner = Arc::new(crate::exec::scripted::Scripted::logging(home.root()));
            return (
                Executor::new(store.clone(), runner, ExecConfig::default()),
                Arc::new(crate::env::StaticResolver::default()),
            );
        }
        Self::slots(home, store)
    }

    fn slots(home: &Home, store: &Arc<Store>) -> (Executor, Arc<dyn ImageResolver>) {
        let runner = SlotRunner::new(
            store.clone(),
            RunnerConfig::for_process(home.root(), ProvenanceMode::Local),
        );
        (
            Executor::slots(store.clone(), runner, ExecConfig::default()),
            Arc::new(DockerResolver),
        )
    }

    /// The services over `store`, pinning imports with `images` and
    /// verifying through `executor` (the tests' scripted runner, say).
    pub fn new(
        home: &Home,
        store: Arc<Store>,
        server_key: Arc<SigningKey>,
        images: Arc<dyn ImageResolver>,
        executor: Executor,
    ) -> Self {
        let workspaces = Arc::new(Workspaces::new(
            store.clone(),
            server_key.clone(),
            Arc::new(TreeEnv(images.clone(), Detection::Off)),
        ));
        let tasks = Arc::new(Tasks::new(
            store.clone(),
            workspaces.clone(),
            server_key.clone(),
        ));
        let coord = Arc::new(Coord::new(store.clone(), workspaces.clone()));
        let verifier = Verifier::new(
            store.clone(),
            workspaces.clone(),
            executor,
            server_key.clone(),
        );
        Self {
            home: home.clone(),
            store,
            server_key,
            workspaces,
            coord,
            tasks,
            verifier,
            images,
            import_secrets: SecretPolicy::RejectHigh,
            _running: None,
        }
    }

    /// The key named `name`, or the bootstrap admin's.
    pub fn key(&self, name: Option<&AgentName>) -> Result<AgentKey, KeyError> {
        key_in(&self.home, name)
    }

    /// The bootstrap admin's signing key, read without opening the store.
    pub fn admin_signing_key(home: &Home) -> Result<SigningKey, KeyError> {
        Ok(key_in(home, None)?.signing_key)
    }

    pub fn workspaces(&self) -> Arc<Workspaces> {
        self.workspaces.clone()
    }

    pub fn coord(&self) -> Arc<Coord> {
        self.coord.clone()
    }

    /// An executor that runs checks in this machine's slots (§2.4.6) under
    /// `mode`, with this process's environment as the one runs inherit.
    pub fn executor(&self, mode: ProvenanceMode) -> Executor {
        self.executor_for(RunnerConfig::for_process(self.home.root(), mode))
    }

    /// [`Self::executor`] with a runner set up by the caller (`aver ci`'s
    /// separate user, slot root and checkout).
    pub fn executor_for(&self, config: RunnerConfig) -> Executor {
        Executor::slots(
            self.store.clone(),
            SlotRunner::new(self.store.clone(), config),
            ExecConfig::default(),
        )
    }
}
