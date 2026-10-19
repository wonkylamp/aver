// The aver.toml every scenario's first history layer carries: arm B's
// `aver run` answers NOT CHECKED without one (ARCHITECTURE.md §1.13.2).
// It runs the scenario's test command with a per-test report and the floor
// `aver check -- <cmd>` would write: 90% of the tests the base runs, at
// least 1. A scenario whose history writes its own aver.toml in a later
// layer (integration-trap's two suites) keeps that one.
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { isPython } from "./score.ts";
import type { Scenario } from "./task.ts";

/** Where the contract lives in a scenario, relative to its directory. */
export const CONTRACT_LAYER_PATH = "aver.toml";

/** Whether a layer after the first writes the scenario's aver.toml. */
export function ownContract(s: Scenario): boolean {
  return s.history.slice(1).some((l) => existsSync(join(s.dir, "history", l.dir, CONTRACT_LAYER_PATH)));
}

/** Why the base's aver.toml is not the contract Aver needs, or null. */
export function contractProblem(s: Scenario, baseDir: string, ran: number): string | null {
  const path = join(baseDir, CONTRACT_LAYER_PATH);
  const fix = `run node bench/eval/contracts.ts ${s.name}`;
  if (!existsSync(path)) return `no ${CONTRACT_LAYER_PATH}; ${fix}`;
  const text = readFileSync(path, "utf8");
  if (ownContract(s)) {
    // The environment shape (argv `run` arrays) reads with min_tests 1, so a base that runs no test would pass.
    return /^run\s*=\s*\[/m.test(text) || !/^min_tests\s*=/m.test(text) ? `${CONTRACT_LAYER_PATH} is not in the contract shape with min_tests` : null;
  }
  return text === contractToml(baseDir, ran) ? null : `${CONTRACT_LAYER_PATH} is stale; ${fix}`;
}

const NODE_REPORTERS = "--test-reporter=spec --test-reporter-destination=stdout --test-reporter=junit --test-reporter-destination=$AVER_OUT/test.xml";

/** `adhoc::floor`: 90% of the tests the base ran, at least 1. */
export function floor(ran: number): number {
  return Math.max(1, Math.floor((ran * 9) / 10));
}

/** A Python scenario's unittest suite under pytest, the only runner there with a per-test report (without one no criterion could be proved). */
export const PYTHON_LINE = "python3 -m pytest -p no:cacheprovider -o junit_family=xunit1 --junitxml=$AVER_OUT/test.xml";

/**
 * A Node scenario's `test` script, which must be `node --test [patterns]`,
 * with the reporters before the patterns: Node reads options after a
 * pattern as more patterns.
 */
export function nodeLine(script: string): string {
  const m = /^node --test(?=\s|$)(.*)$/.exec(script);
  if (!m) throw new Error(`the test script "${script}" is not node --test`);
  return `node --test ${NODE_REPORTERS}${m[1]}`;
}

/** The check's command for the checkout in `dir`. */
export function testLine(dir: string): string {
  if (isPython(dir)) return PYTHON_LINE;
  const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8")) as { scripts?: Record<string, string> };
  return nodeLine(pkg.scripts?.test ?? "");
}

/** The aver.toml text of one check running `line`, on a base that ran `ran` tests. */
export function contractText(line: string, ran: number): string {
  return `[[check]]\nid = "test"\nrun = ${JSON.stringify(line)}\nmin_tests = ${floor(ran)}\n`;
}

/** The aver.toml text for the checkout in `dir`, whose test suite ran `ran` tests on the base. */
export function contractToml(dir: string, ran: number): string {
  return contractText(testLine(dir), ran);
}
