// The tables of crates/aver-core/tests/rules.rs, run against the TypeScript
// port the landing page uses.
import assert from "node:assert/strict";
import { test } from "node:test";
import { CAUSES, type Cause, type Transplant, type Verdict } from "../src/receipt.ts";
import {
  changed,
  classify,
  isInfra,
  MAX_RUNS,
  needsRerun,
  unbuiltDependency,
  verdict,
  withProofRuns,
  type Classification,
  type Entry,
  type Outcome,
} from "../src/rules.ts";

const P: Outcome = "pass";
const F: Outcome = "fail";
const I: Outcome = "sandbox";
const C: Outcome = "cap";

test("rerun policy table", () => {
  const cases: [Outcome[], boolean][] = [
    [[], true],
    [[P], false],
    [[F], true],
    [[F, F], true],
    [[F, F, F], false],
    [[F, P], false],
    [[F, F, P], false],
    [[I], true],
    [[I, I], false],
    [[I, P], false],
    [[I, F], true],
    [[F, I], true],
    [[F, I, I], false],
    [[F, I, F], true],
    [[F, I, F, F], false],
    [[F, F, I], true],
    [[F, F, I, I], false],
    [[F, F, I, F], false],
    [[F, F, I, P], false],
    [[I, F, I, F, I, F], false],
    [[C], true],
    [[C, C], false],
    [[C, I], false],
    [[I, C], false],
    [[C, P], false],
    [[F, C, F], true],
  ];
  for (const [runs, expected] of cases) assert.equal(needsRerun(runs), expected, `needsRerun(${runs.join(",")})`);
});

function reachableSequences(): Outcome[][] {
  const done: Outcome[][] = [];
  const open: Outcome[][] = [[]];
  while (open.length > 0) {
    const runs = open.pop()!;
    if (needsRerun(runs)) for (const next of [P, F, I, C]) open.push([...runs, next]);
    else done.push(runs);
  }
  return done;
}

test("the rerun policy always terminates within bounds", () => {
  const sequences = reachableSequences();
  assert.ok(sequences.length > 0);
  for (const runs of sequences) {
    const results = runs.filter((o) => !isInfra(o)).length;
    assert.ok(results <= MAX_RUNS, `${runs} has more than three results`);
    assert.ok(runs.length <= 2 * MAX_RUNS, `${runs} retried infra more than once per run`);
    assert.ok(!runs.some((_, i) => i >= 2 && isInfra(runs[i]) && isInfra(runs[i - 1]) && isInfra(runs[i - 2])), `${runs} retried infra twice in a row`);
    const stableFailure = results === MAX_RUNS && !runs.includes(P);
    const endedInfra = runs.length >= 2 && isInfra(runs.at(-1)) && isInfra(runs.at(-2));
    const ended = runs.at(-1) === P || endedInfra || (runs.includes(P) && runs.includes(F)) || stableFailure;
    assert.ok(ended, `${runs} stopped without a terminal condition`);
  }
});

/**
 * One representative run sequence per side state, plus equivalent sequences
 * that reach the same state through infra retries. `cap` is a side whose
 * every run hit a resource cap; `env` ended in infra any other way.
 */
type SideLabel = "absent" | "pass" | "fail" | "flaky" | "env" | "cap";

const SIDES: [SideLabel, Outcome[][]][] = [
  ["absent", [[]]],
  ["pass", [[P], [I, P]]],
  ["fail", [[F, F, F], [I, F, F, F], [F, I, F, I, F]]],
  ["flaky", [[F, P], [F, F, P], [F, I, P], [C, F, P]]],
  ["env", [[I, I], [F, I, I], [F, F, I, I], [C, I], [I, C], [F, C, C]]],
  ["cap", [[C, C]]],
];

const c = (base: Classification["base"], head: Classification["head"], cause: Cause | null): Classification => ({ base, head, cause });

function expected(base: SideLabel, head: SideLabel): Classification {
  const infra = (s: SideLabel) => s === "env" || s === "cap";
  if (infra(base)) {
    const h = head === "pass" ? "pass" : head === "flaky" ? "flaky" : "fail";
    return c("fail", h, "ENV");
  }
  const b = base === "absent" ? "n/a" : base;
  if (base === "pass" && head === "cap") return c("pass", "fail", "NEW");
  if (infra(head)) return c(b, "fail", "ENV");
  if (head === "absent") return c(b, "fail", "NEW");
  if (base === "flaky" || head === "flaky") return c(b, head === "flaky" ? "flaky" : head, "FLAKY");
  if (head === "fail") return c(b, "fail", base === "fail" ? "PREEXISTING" : "NEW");
  return c(b, "pass", null);
}

const EXPECTED: Record<`${SideLabel}/${SideLabel}`, Classification> = {
  "absent/pass": c("n/a", "pass", null),
  "absent/fail": c("n/a", "fail", "NEW"),
  "absent/flaky": c("n/a", "flaky", "FLAKY"),
  "absent/env": c("n/a", "fail", "ENV"),
  "absent/cap": c("n/a", "fail", "ENV"),
  "absent/absent": c("n/a", "fail", "NEW"),

  "pass/pass": c("pass", "pass", null),
  "pass/fail": c("pass", "fail", "NEW"),
  "pass/flaky": c("pass", "flaky", "FLAKY"),
  "pass/env": c("pass", "fail", "ENV"),
  "pass/cap": c("pass", "fail", "NEW"),
  "pass/absent": c("pass", "fail", "NEW"),

  "fail/pass": c("fail", "pass", null),
  "fail/fail": c("fail", "fail", "PREEXISTING"),
  "fail/flaky": c("fail", "flaky", "FLAKY"),
  "fail/env": c("fail", "fail", "ENV"),
  "fail/cap": c("fail", "fail", "ENV"),
  "fail/absent": c("fail", "fail", "NEW"),

  "flaky/pass": c("flaky", "pass", "FLAKY"),
  "flaky/fail": c("flaky", "fail", "FLAKY"),
  "flaky/flaky": c("flaky", "flaky", "FLAKY"),
  "flaky/env": c("flaky", "fail", "ENV"),
  "flaky/cap": c("flaky", "fail", "ENV"),
  "flaky/absent": c("flaky", "fail", "NEW"),

  "env/pass": c("fail", "pass", "ENV"),
  "env/fail": c("fail", "fail", "ENV"),
  "env/flaky": c("fail", "flaky", "ENV"),
  "env/env": c("fail", "fail", "ENV"),
  "env/cap": c("fail", "fail", "ENV"),
  "env/absent": c("fail", "fail", "ENV"),

  "cap/pass": c("fail", "pass", "ENV"),
  "cap/fail": c("fail", "fail", "ENV"),
  "cap/flaky": c("fail", "flaky", "ENV"),
  "cap/env": c("fail", "fail", "ENV"),
  "cap/cap": c("fail", "fail", "ENV"),
  "cap/absent": c("fail", "fail", "ENV"),
};

test("the table restates the Rust match arm by arm", () => {
  for (const [base] of SIDES) for (const [head] of SIDES) assert.deepEqual(EXPECTED[`${base}/${head}`], expected(base, head), `${base}/${head}`);
});

test("the cause rules table is exhaustive over side states", () => {
  let checked = 0;
  for (const [baseLabel, baseVariants] of SIDES) {
    for (const [headLabel, headVariants] of SIDES) {
      const want = EXPECTED[`${baseLabel}/${headLabel}`];
      for (const base of baseVariants) {
        for (const head of headVariants) {
          assert.deepEqual(classify(base, head), want, `base ${baseLabel} [${base}], head ${headLabel} [${head}]`);
          checked++;
        }
      }
    }
  }
  assert.ok(checked > 25);
});

test("every reachable pair classifies to a legal entry", () => {
  const sequences = reachableSequences();
  for (const base of sequences) {
    for (const head of sequences) {
      const got = classify(base, head);
      const ended = (runs: Outcome[]) => isInfra(runs.at(-1));
      const ownExhaustion: boolean = head.length > 0 && head.every((r) => r === C) && got.base === "pass" && !ended(base);
      assert.equal(got.cause === "ENV", (ended(base) || ended(head)) && !ownExhaustion, `[${base}] [${head}]`);
      if (ownExhaustion) assert.equal(got.cause, "NEW", `[${base}] [${head}]`);
      if (got.cause === null) assert.equal(got.head, "pass", "only a passing head has no cause");
    }
  }
});

const entry = (head: Entry["head"], cause: Cause | null, proves: string[], id?: string): Entry => ({ id, head, cause, proves });
const pass = (proves: string[]): Entry => entry("pass", null, proves);

test("verdict table", () => {
  const cases: [string, Entry[], string[], Verdict][] = [
    ["all proven", [pass(["a"]), pass(["b"])], [], "PASS"],
    ["one entry proves both", [pass(["a", "b"])], [], "PASS"],
    ["NEW wins over everything", [pass(["a", "b"]), entry("fail", "ENV", []), entry("fail", "NEW", [])], [], "FAIL_OWN"],
    ["ENV blocks even when all proven", [pass(["a", "b"]), entry("fail", "ENV", [])], [], "BLOCKED_NOT_OWN"],
    ["no prover for b", [pass(["a"])], ["b"], "UNPROVEN"],
    ["no entries at all", [], ["a", "b"], "UNPROVEN"],
    ["b's only prover is flaky", [pass(["a"]), entry("flaky", "FLAKY", ["b"])], ["b"], "BLOCKED_NOT_OWN"],
    ["head passes but base was flaky", [pass(["a"]), entry("pass", "FLAKY", ["b"])], ["b"], "BLOCKED_NOT_OWN"],
    [
      "b's provers are flaky and preexisting",
      [pass(["a"]), entry("flaky", "FLAKY", ["b"]), entry("fail", "PREEXISTING", ["b"])],
      ["b"],
      "UNPROVEN",
    ],
    ["preexisting failure that proves nothing does not block", [pass(["a", "b"]), entry("fail", "PREEXISTING", [])], [], "PASS"],
    ["one passing prover is enough", [pass(["a"]), entry("fail", "PREEXISTING", ["b"]), pass(["b"])], [], "PASS"],
    ["flaky entries elsewhere do not block a fully proven task", [pass(["a", "b"]), entry("flaky", "FLAKY", [])], [], "PASS"],
  ];
  for (const [name, entries, unproven, want] of cases) {
    assert.deepEqual(verdict(entries, ["a", "b"], "head-passes"), { unproven, verdict: want }, name);
  }
});

test("an inconclusive entry blocks a v2 verdict", () => {
  const flakyBase = (head: Entry["head"]): Entry => entry(head, "FLAKY", [], "unit::slow");
  const cases: [string, Entry[], Verdict, Verdict][] = [
    ["a command-level check red on both sides hides the change", [entry("fail", "PREEXISTING", [], "test")], "BLOCKED_NOT_OWN", "PASS"],
    [
      "a per-test pre-existing failure does not block",
      [entry("fail", "PREEXISTING", [], "unit::legacy"), entry("pass", null, [], "unit::adds")],
      "PASS",
      "PASS",
    ],
    ["an exit entry red on both sides does not block", [entry("fail", "PREEXISTING", [], "unit::(exit)")], "PASS", "PASS"],
    ["flaky on the base and failing on every head run blocks", [flakyBase("fail")], "BLOCKED_NOT_OWN", "PASS"],
    ["flaky on both sides does not block", [flakyBase("flaky")], "PASS", "PASS"],
    ["NEW still wins", [entry("fail", "PREEXISTING", [], "test"), entry("fail", "NEW", [], "lint")], "FAIL_OWN", "FAIL_OWN"],
  ];
  for (const [name, entries, v2, v1] of cases) {
    assert.equal(verdict(entries, [], "transplant").verdict, v2, `v2: ${name}`);
    assert.equal(verdict(entries, [], "head-passes").verdict, v1, `v0 and v1 keep their meaning: ${name}`);
  }
});

test("unproven follows criteria order", () => {
  assert.deepEqual(verdict([], ["c", "a", "b"], "transplant"), { unproven: ["c", "a", "b"], verdict: "UNPROVEN" });
});

const moved = (head: Entry["head"], cause: Cause | null, proves: string[], transplant: Transplant, headTests: Transplant = "pass"): Entry => ({
  head,
  cause,
  proves,
  transplant,
  head_tests: headTests,
});

test("transplant verdict table", () => {
  const free: Classification = { base: "fail", head: "pass", cause: null };
  const cases: [string, Entry[], boolean, Verdict][] = [
    ["fails on the transplant, passes at head", [moved("pass", null, ["a"], "fail")], true, "PASS"],
    ["absent from the transplant counts as failing there", [moved("pass", null, ["a"], "absent")], true, "PASS"],
    ["passes on the transplant: proves nothing", [moved("pass", null, ["a"], "pass")], false, "UNPROVEN"],
    ["no transplant result: proves nothing in v2", [entry("pass", null, ["a"])], false, "UNPROVEN"],
    ["one proving entry is enough", [moved("pass", null, ["a"], "pass"), moved("pass", null, ["a"], "fail")], true, "PASS"],
    [
      "a judging change proves nothing",
      [moved("pass", null, ["a"], "fail"), entry("fail", "CHANGED", [], "judging::package.json")],
      false,
      "UNPROVEN",
    ],
    ["a transplant that ended in infra is ENV", [moved("pass", withProofRuns(free, "infra", "pass").cause, ["a"], "infra")], false, "BLOCKED_NOT_OWN"],
    ["fails in the head's version of the tests: proves nothing", [moved("pass", null, ["a"], "fail", "fail")], false, "UNPROVEN"],
    ["absent from the head's version of the tests: proves nothing", [moved("pass", null, ["a"], "absent", "absent")], false, "UNPROVEN"],
    [
      "a head-tests run that ended in infra is ENV",
      [moved("pass", withProofRuns({ ...free, base: "n/a" }, "fail", "infra").cause, ["a"], "fail", "infra")],
      false,
      "BLOCKED_NOT_OWN",
    ],
  ];
  for (const [name, entries, proven, want] of cases) {
    const got = verdict(entries, ["a"], "transplant");
    assert.deepEqual([got.unproven.length === 0, got.verdict], [proven, want], name);
  }
});

test("a proof run infra only turns a cause-free entry into ENV", () => {
  const free: Classification = { base: "pass", head: "fail", cause: null };
  const ok: Transplant[] = ["pass", "fail", "absent"];
  for (const t of ok) {
    for (const h of ok) assert.deepEqual(withProofRuns(free, t, h), free);
    assert.equal(withProofRuns(free, "infra", t).cause, "ENV");
    assert.equal(withProofRuns(free, t, "infra").cause, "ENV");
  }
  assert.equal(withProofRuns({ ...free, cause: "NEW" }, "infra", "infra").cause, "NEW", "causes still compare the head with the plain base");
});

const ALL_CAUSES: (Cause | null)[] = [null, ...CAUSES];

test("changed turns NEW into CHANGED only for a changed test file whose own version passes or is gone", () => {
  const owns: (Transplant | undefined)[] = [undefined, "pass", "fail", "absent", "infra"];
  let checked = 0;
  for (const cause of ALL_CAUSES) {
    for (const fileChanged of [false, true]) {
      for (const own of owns) {
        const c: Classification = { base: "pass", head: "fail", cause };
        const rewritten = cause === "NEW" && fileChanged && (own === "pass" || own === "absent");
        assert.deepEqual(changed(c, fileChanged, own), rewritten ? { ...c, cause: "CHANGED" } : c, `${cause} file_changed=${fileChanged} own=${own}`);
        checked++;
      }
    }
  }
  assert.equal(checked, ALL_CAUSES.length * 2 * owns.length);
});

test("unbuiltDependency turns NEW into ENV only when the failing run names an unbuilt dependency", () => {
  for (const cause of ALL_CAUSES) {
    const c: Classification = { base: "pass", head: "fail", cause };
    assert.deepEqual(unbuiltDependency(c, false), c, `${cause} built`);
    assert.deepEqual(unbuiltDependency(c, true), cause === "NEW" ? { ...c, cause: "ENV" } : c, `${cause} unbuilt`);
  }
});

test("REVIEW verdict table", () => {
  const rewrote = entry("fail", "CHANGED", [], "unit::math > add rounds");
  const proving = moved("pass", null, ["a"], "fail");
  const cases: [string, Entry[], string[], string[], Verdict][] = [
    ["a CHANGED entry alone is REVIEW", [rewrote], [], [], "REVIEW"],
    ["CHANGED with every criterion proven is REVIEW", [rewrote, proving], ["a"], [], "REVIEW"],
    ["NEW outranks CHANGED", [rewrote, entry("fail", "NEW", [], "unit::b")], [], [], "FAIL_OWN"],
    ["ENV outranks CHANGED", [rewrote, entry("fail", "ENV", [], "unit::b")], [], [], "BLOCKED_NOT_OWN"],
    ["an inconclusive entry outranks CHANGED", [rewrote, entry("fail", "PREEXISTING", [], "test")], [], [], "BLOCKED_NOT_OWN"],
    ["an unproven criterion outranks CHANGED", [rewrote], ["a"], ["a"], "UNPROVEN"],
    ["CHANGED with a flaky prover is BLOCKED_NOT_OWN", [rewrote, entry("flaky", "FLAKY", ["a"], "unit::c")], ["a"], ["a"], "BLOCKED_NOT_OWN"],
    ["a judging CHANGED entry with no criteria is REVIEW", [entry("fail", "CHANGED", [], "judging::jest.config.js")], [], [], "REVIEW"],
    [
      "a judging CHANGED entry blocks every proof, so a criterion is UNPROVEN",
      [proving, entry("fail", "CHANGED", [], "judging::package.json#scripts")],
      ["a"],
      ["a"],
      "UNPROVEN",
    ],
    [
      "per-test PREEXISTING and FLAKY do not hide a review",
      [rewrote, entry("fail", "PREEXISTING", [], "unit::old"), entry("flaky", "FLAKY", [], "unit::slow")],
      [],
      [],
      "REVIEW",
    ],
    [
      "a judging change outranks a flaky prover: nothing proves, so UNPROVEN",
      [entry("flaky", "FLAKY", ["a"], "unit::c"), entry("fail", "CHANGED", [], "judging::jest.config.js")],
      ["a"],
      ["a"],
      "UNPROVEN",
    ],
    ["nothing changed", [proving], ["a"], [], "PASS"],
  ];
  for (const [name, entries, criteria, unproven, want] of cases) {
    assert.deepEqual(verdict(entries, criteria, "transplant"), { unproven, verdict: want }, name);
  }
});
