# Example receipt

`calc-fail-own.json` is a real receipt, in the `{receipt_id, receipt}` envelope that `aver verify --json` prints (without its other fields). It was produced on 2026-10-12 with a throwaway `$AVER_HOME`:

1. `aver init`, then `aver import` of `fixtures/calc` as the base, with `--ref trunk`.
2. `aver import` of a copy of `fixtures/calc` whose `src/math.js` implements `divide` as `a / b` (no zero check) and changes `add` to return `a - b`.
3. `aver task create` with the "Implement divide" task from the README.
4. `aver verify --task … --head <head snapshot> --base <base snapshot>`.

The receipt id is the one the Rust object store recorded. The verifier's public key is `6k8CJxp-0fIP_Cl8v64ZNVg3PNsmrFY4YBpLFLWflOo`. Its secret key was discarded with that `$AVER_HOME`, so nobody can sign another receipt with it.
