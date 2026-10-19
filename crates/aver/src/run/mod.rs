//! Running agents in disk workspaces (ARCHITECTURE.md §2.10): `aver run`'s
//! supervisor, the agent templates, the brief, the run file, the live view,
//! the comparison, `aver accept`, the Claude Code hooks and the
//! agent-facing verify report; batches of tasks run as one run each.

pub mod accept;
pub mod agent;
pub mod api;
pub mod batch;
pub mod brief;
pub mod combine;
pub mod compare;
pub mod cross;
pub mod feedback;
pub mod hooks;
pub mod limits;
pub mod report;
pub mod rerun;
pub mod state;
pub mod supervisor;
pub mod view;
