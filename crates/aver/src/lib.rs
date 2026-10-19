//! Aver's services, one module each (ARCHITECTURE.md §2), which every
//! command opens in its own process ([`services`]). Phase 1 provides the
//! object store; git, workspaces, execution, verification, tasks and the CLI
//! build on it.

pub mod adhoc;
pub mod agents;
pub mod atomic;
pub mod cargo_features;
pub mod ci;
pub mod contract;
pub mod coord;
pub mod coverage;
pub mod diagnostics;
pub mod disk;
pub mod env;
pub mod exec;
pub mod fault;
pub mod git;
pub mod idents;
pub mod inline_tests;
pub mod integrate;
pub mod judge;
pub mod judging;
pub mod keys;
pub mod languages;
pub mod ledger;
pub mod place;
pub mod presets;
pub mod progress;
pub mod recover;
pub mod run;
pub mod secrets;
pub mod seen;
pub mod services;
pub mod store;
pub mod style;
pub mod suggest;
pub mod tasks;
/// The judge's public path since before the `judge/` split.
pub use judge as verify;
pub mod workspace;
