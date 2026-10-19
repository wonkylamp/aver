// The cause explorer's state, as pure functions over rules.ts. The visitor
// picks an outcome for every run a side could make; the rerun policy decides
// how many of those runs actually happen.
import type { Cause, Verdict } from "./receipt.ts";
import { classify, firstMatchingRule, isInfra, MAX_RUNS, needsRerun, verdict, type Classification, type Outcome } from "./rules.ts";

/** The longest run sequence the rerun policy allows: three results, each after one infra retry. */
export const SLOTS = 2 * MAX_RUNS;

export const CRITERION = "div-zero";

export interface SideInput {
  /** False when the test has no result on this side: not on the base, or missing from the head's report. */
  exists: boolean;
  /** The outcome each run would have, if it happens. Always SLOTS long. */
  slots: Outcome[];
}

export interface ExplorerState {
  base: SideInput;
  head: SideInput;
  provesCriterion: boolean;
}

export interface Decision {
  baseRuns: Outcome[];
  headRuns: Outcome[];
  classification: Classification;
  rule: number;
  verdict: Verdict;
  unproven: string[];
}

/** The runs a side makes: slots in order, for as long as the rerun policy asks for another. */
export function runsOf(side: SideInput): Outcome[] {
  const runs: Outcome[] = [];
  if (!side.exists) return runs;
  while (runs.length < side.slots.length && needsRerun(runs)) runs.push(side.slots[runs.length]!);
  return runs;
}

export function decide(state: ExplorerState): Decision {
  const baseRuns = runsOf(state.base);
  const headRuns = runsOf(state.head);
  const classification = classify(baseRuns, headRuns);
  const criteria = state.provesCriterion ? [CRITERION] : [];
  const entry = { head: classification.head, cause: classification.cause, proves: criteria };
  const { unproven, verdict: v } = verdict([entry], criteria);
  return { baseRuns, headRuns, classification, rule: firstMatchingRule(baseRuns, headRuns), verdict: v, unproven };
}

/** Why a side stopped running, in plain words. */
export function stopReason(runs: readonly Outcome[], side: "base" | "head"): string {
  if (runs.length === 0) return side === "base" ? "No such test on the base." : "Missing from the head's test report.";
  if (runs.length >= 2 && isInfra(runs.at(-1)) && isInfra(runs.at(-2))) return "Infrastructure failed twice in a row, so it stops.";
  const passes = runs.includes("pass");
  const fails = runs.includes("fail");
  if (passes && fails) return "The runs disagree, so it stops.";
  if (passes) return runs.length === 1 ? "Passed on the first run." : "Passed after an infrastructure retry.";
  return `Failed ${MAX_RUNS} times, so the failure is stable.`;
}

const NEXT: Record<Outcome, Outcome> = { pass: "fail", fail: "cap", cap: "sandbox", sandbox: "pass" };

/** The state after the visitor clicks run `index` of a side: its outcome moves to the next one. */
export function cycle(state: ExplorerState, side: "base" | "head", index: number): ExplorerState {
  const slots = state[side].slots.slice();
  slots[index] = NEXT[slots[index]!];
  return { ...state, [side]: { ...state[side], slots } };
}

function slots(...given: Outcome[]): Outcome[] {
  const last = given.at(-1) ?? "pass";
  return [...given, ...Array.from({ length: SLOTS - given.length }, () => last)];
}

export interface Preset {
  id: string;
  label: string;
  state: ExplorerState;
}

export const PRESETS: readonly Preset[] = [
  {
    id: "new",
    label: "Broken by the change",
    state: { base: { exists: true, slots: slots("pass") }, head: { exists: true, slots: slots("fail") }, provesCriterion: false },
  },
  {
    id: "preexisting",
    label: "Already broken",
    state: { base: { exists: true, slots: slots("fail") }, head: { exists: true, slots: slots("fail") }, provesCriterion: false },
  },
  {
    id: "flaky",
    label: "Flaky",
    state: { base: { exists: true, slots: slots("pass") }, head: { exists: true, slots: slots("fail", "pass") }, provesCriterion: false },
  },
  {
    id: "oom",
    label: "Out of memory",
    state: { base: { exists: true, slots: slots("pass") }, head: { exists: true, slots: slots("cap") }, provesCriterion: false },
  },
  {
    id: "env",
    label: "Runner error",
    state: { base: { exists: true, slots: slots("pass") }, head: { exists: true, slots: slots("sandbox") }, provesCriterion: false },
  },
  {
    id: "deleted",
    label: "Test deleted",
    state: { base: { exists: true, slots: slots("pass") }, head: { exists: false, slots: slots("pass") }, provesCriterion: false },
  },
  {
    id: "proven",
    label: "Criterion proven",
    state: { base: { exists: true, slots: slots("fail") }, head: { exists: true, slots: slots("pass") }, provesCriterion: true },
  },
];

export const CAUSE_TEXT: Record<Cause | "none", string> = {
  NEW: "The change broke it.",
  PREEXISTING: "It was already failing before the change.",
  FLAKY: "It passes and fails on the same code.",
  ENV: "The infrastructure failed, not the code.",
  CHANGED: "The change rewrote this test: the old version fails, the change's own version passes. A person reviews it.",
  none: "Nothing to attribute: the test passes.",
};

export const VERDICT_TEXT: Record<Verdict, string> = {
  PASS: "Every criterion is proven and nothing broke.",
  FAIL_OWN: "The change broke something. The check fails.",
  BLOCKED_NOT_OWN: "Blocked by infrastructure or flakiness, not by the change.",
  UNPROVEN: "Nothing broke, but a criterion has no passing proof.",
  REVIEW: "Nothing broke; the change rewrote test expectations or harness that a person should review.",
};
