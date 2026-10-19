# rust-proofs WIP
Done: libtest entries resolved to nested-crate test files (verify.rs `EntryContext::rust_file`), new tests whose transplant failed to build count as evidence, inline test items transplanted (judging.rs), retargeted `[[test]]` targets make base-unseen failures CHANGED not NEW, report hint naming `aver workspace rebase`.
Tests: crates/aver/tests/rust_proofs.rs (8 native cargo scenarios, all pass; 4 fail on main as expected).
Left: aver check/verify in the workspace, gate (fmt, clippy, cargo test --workspace), docs/VERDICTS.md, docs/CLI.md, CLAUDE.md phase bullet.
Next step: finish `aver check`, decide ignore vs not for the native tests, run the gate, then `aver verify` and `aver accept`.
