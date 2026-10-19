import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { parseHTML } from "linkedom";
import { cycle, decide, PRESETS, runsOf, SLOTS, stopReason, type ExplorerState } from "../src/explorer.ts";
import { CAUSE_RULES, classify, needsRerun, type Outcome } from "../src/rules.ts";

const OUTCOMES: Outcome[] = ["pass", "fail", "cap", "sandbox"];

function* allSlots(length: number): Generator<Outcome[]> {
  if (length === 0) {
    yield [];
    return;
  }
  for (const rest of allSlots(length - 1)) for (const o of OUTCOMES) yield [...rest, o];
}

test("every choice of run outcomes yields a run sequence the rerun policy finishes", () => {
  for (const slots of allSlots(SLOTS)) {
    const runs = runsOf({ exists: true, slots });
    assert.equal(needsRerun(runs), false, `[${slots}] stopped at [${runs}] with a rerun still due`);
    assert.deepEqual(runs, slots.slice(0, runs.length));
  }
});

test("a side without the test has no runs", () => {
  assert.deepEqual(runsOf({ exists: false, slots: Array(SLOTS).fill("fail") }), []);
});

test("the explorer's decision is classify and verdict over the runs it shows", () => {
  for (const preset of PRESETS) {
    const d = decide(preset.state);
    assert.deepEqual(d.classification, classify(d.baseRuns, d.headRuns), preset.id);
    assert.equal(CAUSE_RULES[d.rule]!.cause, d.classification.cause, preset.id);
  }
});

test("each preset shows the case it is named for", () => {
  const got = Object.fromEntries(PRESETS.map((p) => [p.id, decide(p.state)]));
  assert.deepEqual([got.new!.classification.cause, got.new!.verdict, got.new!.headRuns], ["NEW", "FAIL_OWN", ["fail", "fail", "fail"]]);
  assert.deepEqual([got.preexisting!.classification.cause, got.preexisting!.verdict], ["PREEXISTING", "PASS"]);
  assert.deepEqual([got.flaky!.classification.cause, got.flaky!.verdict, got.flaky!.headRuns], ["FLAKY", "PASS", ["fail", "pass"]]);
  assert.deepEqual([got.oom!.classification.cause, got.oom!.rule, got.oom!.verdict, got.oom!.headRuns], ["NEW", 0, "FAIL_OWN", ["cap", "cap"]]);
  assert.deepEqual([got.env!.classification.cause, got.env!.verdict, got.env!.headRuns], ["ENV", "BLOCKED_NOT_OWN", ["sandbox", "sandbox"]]);
  assert.deepEqual([got.deleted!.classification.cause, got.deleted!.rule, got.deleted!.verdict], ["NEW", 2, "FAIL_OWN"]);
  assert.deepEqual([got.proven!.classification.cause, got.proven!.verdict, got.proven!.unproven], [null, "PASS", []]);
});

test("a criterion whose only proof is flaky blocks; one whose proof fails is unproven", () => {
  const flaky: ExplorerState = { ...PRESETS.find((p) => p.id === "flaky")!.state, provesCriterion: true };
  assert.deepEqual([decide(flaky).verdict, decide(flaky).unproven], ["BLOCKED_NOT_OWN", ["div-zero"]]);
  const broken: ExplorerState = { ...PRESETS.find((p) => p.id === "preexisting")!.state, provesCriterion: true };
  assert.equal(decide(broken).verdict, "UNPROVEN");
});

test("pressing a run moves it to the next outcome and the policy reshapes the side", () => {
  let state = PRESETS.find((p) => p.id === "new")!.state;
  state = cycle(state, "head", 0); // fail -> cap: retried, then the stored failures follow
  assert.deepEqual(decide(state).headRuns, ["cap", "fail", "fail", "fail"]);
  state = cycle(state, "head", 0); // cap -> sandbox: the same retry
  assert.deepEqual(decide(state).headRuns, ["sandbox", "fail", "fail", "fail"]);
  state = cycle(state, "head", 0); // sandbox -> pass: one run is enough
  assert.deepEqual(decide(state).headRuns, ["pass"]);
  assert.equal(decide(state).classification.cause, null);
});

test("stop reasons name the condition that ended the side", () => {
  assert.match(stopReason([], "base"), /No such test on the base/);
  assert.match(stopReason([], "head"), /Missing from the head/);
  assert.match(stopReason(["pass"], "head"), /first run/);
  assert.match(stopReason(["cap", "pass"], "head"), /infrastructure retry/);
  assert.match(stopReason(["fail", "pass"], "head"), /disagree/);
  assert.match(stopReason(["fail", "fail", "fail"], "head"), /3 times/);
  assert.match(stopReason(["fail", "sandbox", "sandbox"], "head"), /twice in a row/);
  assert.match(stopReason(["cap", "cap"], "head"), /twice in a row/);
});

test("the landing page lists the cause rules in the order rules.ts tries them", () => {
  const { document } = parseHTML(readFileSync(new URL("../index.html", import.meta.url), "utf8"));
  const items = [...document.querySelectorAll("#explorer .ladder > li")].map((li) => [
    li.querySelector(".rule-when")!.textContent,
    li.querySelector(".rule-then")!.textContent,
  ]);
  assert.deepEqual(
    items,
    CAUSE_RULES.map((r) => [r.when, r.cause ?? "No cause"]),
  );
});
