//! Execution places: where a check's commands run and how
//! the processes they start are managed. [`proc`] starts, bounds and reaps
//! a run's processes; [`workdir`] swaps one directory between trees;
//! [`sides`] keeps a cloned directory per side of a verification;
//! [`runner`] runs a check in one, and [`host`] names the host's tools.

pub mod host;
pub mod proc;
pub mod runner;
pub mod sides;
pub mod workdir;
