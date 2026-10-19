// A line-for-line port of crates/aver-core/src/rules.rs, for the landing
// page's cause explorer. The Rust module is the source of truth; the tests in
// test/rules.test.ts repeat its tables so the two cannot drift silently.
import type { BaseStatus, Cause, HeadStatus, Transplant, Verdict } from "./receipt.ts";

/**
 * How a run ended (`aver_core::Ending`): passed, failed, hit a resource cap
 * (out of memory or the timeout), or failed in the sandbox itself (no
 * container, a Docker error, a malformed result). A cap and a sandbox
 * failure are both infrastructure endings.
 */
export type Outcome = "pass" | "fail" | "cap" | "sandbox";

export function isInfra(o: Outcome | undefined): boolean {
  return o === "cap" || o === "sandbox";
}

/** Runs a failing side needs before its result counts as a stable failure. */
export const MAX_RUNS = 3;

export interface Classification {
  base: BaseStatus;
  head: HeadStatus;
  cause: Cause | null;
}

type Agreement = "pass" | "fail" | "flaky";

interface Side {
  agreement: Agreement | null;
  results: number;
  endedInfra: boolean;
}

function sideOf(runs: readonly Outcome[]): Side {
  const passes = runs.filter((o) => o === "pass").length;
  const fails = runs.filter((o) => o === "fail").length;
  const agreement: Agreement | null = passes > 0 && fails > 0 ? "flaky" : passes > 0 ? "pass" : fails > 0 ? "fail" : null;
  return { agreement, results: passes + fails, endedInfra: isInfra(runs.at(-1)) };
}

/**
 * Whether a side needs another run: each side runs once, a failing side
 * reruns until it has three results or disagrees with itself, and an infra
 * run is retried once.
 */
export function needsRerun(runs: readonly Outcome[]): boolean {
  if (runs.length === 0) return true;
  if (runs.length >= 2 && isInfra(runs.at(-1)) && isInfra(runs.at(-2))) return false;
  if (isInfra(runs.at(-1))) return true;
  const side = sideOf(runs);
  return side.agreement === "fail" && side.results < MAX_RUNS;
}

/**
 * Classifies one entry from its base and head runs after reruns. First match
 * wins: every head run hit a resource cap while base passed is NEW, so a
 * change cannot pass off its own exhaustion as infrastructure; any side
 * ended infra is ENV; a head missing from its report is NEW; a side that
 * disagrees with itself is FLAKY; head fails with base pass or n/a is NEW;
 * both fail is PREEXISTING; head passes is no cause.
 */
export function classify(baseRuns: readonly Outcome[], headRuns: readonly Outcome[]): Classification {
  return { ...statuses(baseRuns, headRuns), cause: CAUSE_RULES[firstMatchingRule(baseRuns, headRuns)]!.cause };
}

function statuses(baseRuns: readonly Outcome[], headRuns: readonly Outcome[]): { base: BaseStatus; head: HeadStatus } {
  const base = sideOf(baseRuns);
  const head = sideOf(headRuns);
  const baseStatus: BaseStatus = base.agreement ?? (base.endedInfra ? "fail" : "n/a");
  const headStatus: HeadStatus = head.agreement ?? "fail";
  return { base: baseStatus, head: headStatus };
}

export interface CauseRule {
  /** What the rule tests, in plain words. */
  when: string;
  cause: Cause | null;
}

/** The cause rules of `classify`, in the order they are tried. */
export const CAUSE_RULES: readonly CauseRule[] = [
  { when: "Every head run hit the memory or time limit, and the base passed", cause: "NEW" },
  { when: "Either side ended in an infrastructure failure", cause: "ENV" },
  { when: "The test is missing from the head", cause: "NEW" },
  { when: "Runs on one side disagree", cause: "FLAKY" },
  { when: "The head fails, the base passed or had no such test", cause: "NEW" },
  { when: "Both sides fail", cause: "PREEXISTING" },
  { when: "The head passes", cause: null },
];

/** The index in CAUSE_RULES of the rule that decides this pair of sides. */
export function firstMatchingRule(baseRuns: readonly Outcome[], headRuns: readonly Outcome[]): number {
  const base = sideOf(baseRuns);
  const head = sideOf(headRuns);
  const { base: baseStatus, head: headStatus } = statuses(baseRuns, headRuns);
  const ownExhaustion = headRuns.length > 0 && headRuns.every((r) => r === "cap") && baseStatus === "pass" && !base.endedInfra;
  if (ownExhaustion) return 0;
  if (base.endedInfra || head.endedInfra) return 1;
  if (headRuns.length === 0) return 2;
  if (baseStatus === "flaky" || headStatus === "flaky") return 3;
  if (headStatus === "fail") return baseStatus === "fail" ? 5 : 4;
  return 6;
}

/**
 * An entry's classification once its proof runs are known: a proving entry
 * whose transplant or head-tests run ended in infra cannot show that it
 * needs the change, so a cause-free entry becomes ENV.
 */
export function withProofRuns(c: Classification, transplant: Transplant, headTests: Transplant): Classification {
  return c.cause === null && (transplant === "infra" || headTests === "infra") ? { ...c, cause: "ENV" } : c;
}

/**
 * An entry's classification once the head's own version of its test is
 * known (CHANGED): a NEW entry becomes CHANGED when the head modified or
 * deleted the existing test file that defines it (`fileChanged`) and the
 * head's own version of the test passed or no longer exists (`own` is
 * `pass` or `absent`). The base's copy failing on the head is then a test
 * expectation the change rewrote, which only a human can tell from
 * test-gaming. If the head's own version fails too, or could not run
 * (`fail`, `infra` or no head-tests run), it stays NEW.
 */
export function changed(c: Classification, fileChanged: boolean, own: Transplant | undefined): Classification {
  const rewritten = fileChanged && (own === "pass" || own === "absent");
  return c.cause === "NEW" && rewritten ? { ...c, cause: "CHANGED" } : c;
}

/**
 * An entry's classification once the head's dependency builds are known: a
 * NEW entry becomes ENV when a dependency the head added needs its install
 * script to build or download a native binding, the network-less rebuild
 * failed on the head, and the entry's failing run names that dependency
 * (`unbuilt`). The sandbox, not the change, stops it: it has no network and
 * no compiler. A head that imports a package it never declared is not this
 * case and stays NEW.
 */
export function unbuiltDependency(c: Classification, unbuilt: boolean): Classification {
  return c.cause === "NEW" && unbuilt ? { ...c, cause: "ENV" } : c;
}

/** How a receipt decides that a criterion is proven: v0 and v1 on the head alone, v2 by the transplant. */
export type ProofRule = "head-passes" | "transplant";

/** The fields of a receipt entry that the verdict reads. */
export interface Entry {
  id?: string;
  head: HeadStatus;
  cause: Cause | null;
  proves: readonly string[];
  transplant?: Transplant;
  head_tests?: Transplant;
}

/**
 * Whether `entry` proves the criteria in its `proves` under `rule`: it
 * passes on head with no cause and, in v2, failed or was absent on the
 * transplant tree and passed in the head-tests run.
 */
export function proves(entry: Entry, rule: ProofRule): boolean {
  return (
    entry.head === "pass" &&
    entry.cause === null &&
    (rule === "head-passes" || ((entry.transplant === "fail" || entry.transplant === "absent") && entry.head_tests === "pass"))
  );
}

/** Whether the receipt has a judging-file entry. */
export function judgingChanged(entries: readonly Entry[]): boolean {
  return entries.some((e) => e.id !== undefined && e.id.startsWith("judging::"));
}

/**
 * Whether `entry` is a command-level entry: a whole check judged by its exit
 * code. Its id is the check id alone; per-test, exit, judging and
 * protected-path entries all contain `::`.
 */
export function isCommandLevel(entry: Entry): boolean {
  return entry.id !== undefined && !entry.id.includes("::");
}

/**
 * Whether `entry` leaves the change's effect unknown (v2): a command-level
 * entry that fails on both sides, or an entry that was flaky on the base and
 * failed on every head run.
 */
export function inconclusive(entry: Entry, rule: ProofRule): boolean {
  if (rule !== "transplant") return false;
  if (entry.cause === "PREEXISTING") return isCommandLevel(entry);
  if (entry.cause === "FLAKY") return entry.head === "fail";
  return false;
}

/**
 * The unproven criteria (in criteria order) and the verdict. A criterion is
 * proven when an entry that proves it passes `proves` and, in v2, no judging
 * file changed. First match wins: any NEW is FAIL_OWN; any ENV, or in v2 any
 * inconclusive entry, is BLOCKED_NOT_OWN; nothing unproven is REVIEW when an
 * entry is CHANGED (v2 only) and PASS otherwise; an unproven criterion whose
 * provers are all FLAKY is BLOCKED_NOT_OWN, unless (v2) a judging file
 * changed; otherwise UNPROVEN. So UNPROVEN
 * outranks REVIEW: a human is asked to review changed test expectations only
 * once nothing else is left to do.
 */
export function verdict(
  entries: readonly Entry[],
  criteria: readonly string[],
  rule: ProofRule = "head-passes",
): { unproven: string[]; verdict: Verdict } {
  const judging = rule === "transplant" && judgingChanged(entries);
  const provers = (id: string) => entries.filter((e) => e.proves.includes(id));
  const unproven = criteria.filter((id) => judging || !provers(id).some((e) => proves(e, rule)));
  const anyCause = (cause: Cause) => entries.some((e) => e.cause === cause);
  // With a judging change nothing proves, flaky or not: the reason is the
  // harness edit, so such a receipt is UNPROVEN, not BLOCKED_NOT_OWN.
  const blockedByFlaky = !judging && unproven.some((id) => {
    const list = provers(id);
    return list.length > 0 && list.every((e) => e.cause === "FLAKY");
  });
  let result: Verdict;
  if (anyCause("NEW")) result = "FAIL_OWN";
  else if (anyCause("ENV") || entries.some((e) => inconclusive(e, rule))) result = "BLOCKED_NOT_OWN";
  else if (unproven.length === 0 && anyCause("CHANGED")) result = "REVIEW";
  else if (unproven.length === 0) result = "PASS";
  else if (blockedByFlaky) result = "BLOCKED_NOT_OWN";
  else result = "UNPROVEN";
  return { unproven, verdict: result };
}
