// Every scenario of a set is sound: it builds, fails as declared on the
// base, its hidden tests fail on the base and pass on the reference
// solution, each scripted fake behaves as its task.json says (in the hard
// set, a naive fake fails the hidden tests), and each declared flaky test
// fails at about its stated rate over 20 runs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { SetName } from "../lib/task.ts";
import { loadScenario, scenarioNames } from "../lib/task.ts";
import { validate } from "../validate.ts";

export function soundness(set: SetName, expected: number): void {
  const names = scenarioNames(set);

  test(`the ${set} set has its ${expected} scenarios`, () => {
    assert.equal(names.length, expected, names.join(", "));
  });

  for (const name of names) {
    test(`scenario ${name} is sound`, async () => {
      const work = mkdtempSync(join(tmpdir(), `aver-eval-${name}-`));
      try {
        const v = await validate(loadScenario(name), work);
        assert.deepEqual(v.problems, []);
        for (const t of loadScenario(name).tasks) assert.ok((v.totals[t.id] ?? 0) > 0, `${t.id} has hidden tests`);
      } finally {
        rmSync(work, { recursive: true, force: true });
      }
    });
  }
}
