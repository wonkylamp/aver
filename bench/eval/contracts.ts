// Writes each scenario's aver.toml into its first history layer
// (lib/contract.ts): builds the repository, runs its tests on the base to
// count them, and writes the contract (except where a later layer writes
// the scenario's own). validate.ts fails a scenario whose
// aver.toml differs from what this writes; rerun it after changing a
// scenario's tests.
//
//   node bench/eval/contracts.ts [scenario ...]
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { CONTRACT_LAYER_PATH, contractToml, ownContract } from "./lib/contract.ts";
import { npmTest } from "./lib/score.ts";
import { buildRepo, loadScenario, scenarioNames } from "./lib/task.ts";

const names = process.argv.slice(2);
const work = mkdtempSync(join(tmpdir(), "aver-eval-contracts-"));
try {
  for (const name of names.length > 0 ? names : scenarioNames()) {
    const s = loadScenario(name);
    if (ownContract(s)) {
      console.log(`${name}: its history writes its own ${CONTRACT_LAYER_PATH}`);
      continue;
    }
    const repo = join(work, name);
    buildRepo(s, repo);
    const ran = await npmTest(repo);
    if (ran.tests === 0) throw new Error(`${name}: the base ran no test`);
    const text = contractToml(repo, ran.tests);
    writeFileSync(join(s.dir, "history", s.history[0]!.dir, CONTRACT_LAYER_PATH), text);
    console.log(`${name}: ${ran.tests} tests on the base, ${text.match(/min_tests = \d+/)![0]}`);
  }
} finally {
  rmSync(work, { recursive: true, force: true });
}
