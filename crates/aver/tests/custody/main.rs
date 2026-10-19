//! Custody under failure (custody plan §1): each operation is cut at each
//! of its crash points (`aver::fault`), the invariants are audited, the
//! operation runs again without the fault, and they are audited again.
//! Built only with `--features faults` (`make test-custody`).

mod audit;
mod k2;
mod lifecycle;
mod matrix;
#[allow(dead_code)]
mod procs;
mod workdir;
mod world;
