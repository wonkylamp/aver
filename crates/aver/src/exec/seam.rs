//! The executor's in-process runner seam: [`Executor::new`] takes a
//! [`Sandbox`], which gets each run's command, environment, limits and the
//! snapshot's files, and returns how it ended. The test suite's scripted
//! runner is its only implementation, so verification can be tested
//! without running commands.
//!
//! [`Executor::new`]: super::Executor::new

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use aver_core::{Path, TreeId};

use super::output::{SandboxLimits, SandboxOutput};
use crate::store::{Store, StoreError};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One command to run.
#[derive(Debug, Clone)]
pub struct SandboxRequest {
    /// Names the run: `<snapshot>:<check>:<index>`.
    pub label: String,
    pub argv: Vec<String>,
    /// Working directory relative to the snapshot root.
    pub cwd: Option<Path>,
    /// The complete process environment; nothing else is inherited.
    pub env: BTreeMap<String, String>,
    pub limits: SandboxLimits,
    /// The snapshot's files by path: a regular file's bytes, a symlink's
    /// target.
    pub input: Arc<BTreeMap<String, Vec<u8>>>,
    /// Commands run in order before `argv`, each an argv (§2.4.1). All of
    /// them and `argv` run; the run's exit code is `argv`'s unless that is 0
    /// and a setup command failed.
    pub setup: Vec<Vec<String>>,
}

/// The runner itself failed; the command's result is unknown.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxError {
    #[error("malformed sandbox result: {0}")]
    Malformed(String),
    #[error("invalid sandbox request: {0}")]
    Invalid(String),
    #[error("the disk is full ({0})")]
    DiskFull(String),
}

/// Runs one command per request and reports how it ended.
pub trait Sandbox: Send + Sync {
    /// The executor profile id recorded on every run and keyed in the cache.
    fn executor(&self) -> &str;

    fn run(&self, request: SandboxRequest) -> BoxFuture<'_, Result<SandboxOutput, SandboxError>>;
}

/// The files of the tree `root`, by path, for a [`SandboxRequest`].
pub fn tree_input(store: &Store, root: &TreeId) -> Result<BTreeMap<String, Vec<u8>>, StoreError> {
    let mut out = BTreeMap::new();
    for (path, file) in crate::place::workdir::tree_files(store, root)? {
        out.insert(path, store.get_raw(&file.blob)?);
    }
    Ok(out)
}
