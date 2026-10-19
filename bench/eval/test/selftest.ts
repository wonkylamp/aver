// Helpers of the harness self-tests: scenarios written on the fly and the
// dry-run options (scripted agents, the stub aver).
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { contractText, nodeLine, PYTHON_LINE } from "../lib/contract.ts";
import type { Options } from "../run.ts";

export const HERE = import.meta.dirname;

export function write(root: string, files: Record<string, string>): void {
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), text);
  }
}

export const pkg = JSON.stringify({ name: "mini", version: "1.0.0", private: true, type: "module", scripts: { test: "node --test" } });
export const lock = JSON.stringify({ name: "mini", version: "1.0.0", lockfileVersion: 3, requires: true, packages: { "": { name: "mini", version: "1.0.0" } } });
/** The aver.toml contracts.ts writes for a fixture whose base runs `ran` tests. */
export const contract = (ran: number, python = false): string => contractText(python ? PYTHON_LINE : nodeLine("node --test"), ran);
export const T = 'import { test } from "node:test";\nimport assert from "node:assert/strict";\n';

export function options(work: string, tasks: string): Options {
  return {
    set: "all",
    scenarios: ["mini"],
    arms: ["A", "B"],
    agents: 2,
    label: "selftest",
    fresh: false,
    timeoutMs: 60_000,
    maxBudgetUsd: 1,
    model: null,
    feedback: true,
    work,
    results: join(work, "results"),
    aver: resolve(HERE, "..", "fake-aver.ts"),
    claude: "claude-not-used",
    dryRun: true,
    tasksDir: tasks,
    log: () => {},
  };
}
