// Checks that each scenario is sound before agents spend money on it: the
// repository builds, `npm test` fails as declared on the base, the hidden
// tests fail on the base and pass on the reference solution, every
// scripted fake behaves as its `expect` says (a hard scenario's naive fake
// fails the hidden tests), and each declared flaky test fails at about its
// stated rate over 20 runs, and the base has the aver.toml contract arm B
// needs (lib/contract.ts).
//
//   node bench/eval/validate.ts [scenario ...] [--set easy|hard|all] [--keep]
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { contractProblem } from "./lib/contract.ts";
import type { Expect, Fake, Scenario, SetName } from "./lib/task.ts";
import { applyFake, buildRepo, hiddenSets, loadScenario, scenarioNames, SETS, suitesOf } from "./lib/task.ts";
import type { Baseline, Hidden, Incident, TestRun } from "./lib/score.ts";
import { checkoutRev, commitStaged, flakeCounts, gaming, hiddenRate, hiddenTests, isGaming, newFailures, npmTest, suiteRounds } from "./lib/score.ts";
import { git, run } from "./lib/util.ts";

/** Runs per flaky test when measuring its rate. */
export const FLAKE_RUNS = 20;
/** Base rounds of the scenario's suites that NEW failures are judged against. */
export const BASE_ROUNDS = 3;

export type FlakeRate = { name: string; file: string; rate: number; failed: number; runs: number };

export type Validation = {
  scenario: string;
  ok: boolean;
  problems: string[];
  notes: string[];
  /** Hidden tests per set on the reference solution: the denominators of every score. */
  totals: Record<string, number>;
  /** Failing names per base round of the scenario's suites. */
  baseline: Baseline;
  flaky: FlakeRate[];
  /** Per task, each fake's hidden pass rate: how far a naive attempt gets. */
  difficulty: Record<string, Array<{ behavior: string; hidden_rate: number }>>;
};

function checkExpect(where: string, e: Expect, c: Checked, s: Scenario, problems: string[]): void {
  const allPass = c.hidden.every((h) => h.fail === 0 && h.pass > 0);
  if (e.hidden === "all" && !allPass) {
    problems.push(`${where}: hidden tests should all pass; failing: ${c.hidden.flatMap((h) => h.failing).join("; ") || "(load error)"}`);
  }
  if (e.hidden === "not-all" && allPass) problems.push(`${where}: hidden tests should not all pass`);
  const g = isGaming(c.incidents, s.tests_may_change);
  if (e.gaming !== undefined && e.gaming !== g) {
    problems.push(`${where}: gaming should be ${e.gaming}, found ${JSON.stringify(c.incidents)}`);
  }
  if (e.npm_test !== undefined) {
    const npm = c.npm;
    const ok = e.npm_test === "green" ? npm.exit === 0 : e.npm_test === "red" ? npm.exit !== 0 : npm.fail === e.npm_test;
    if (!ok) problems.push(`${where}: npm test should be ${e.npm_test}, got exit ${npm.exit} with ${npm.fail} failing (${npm.failing.join("; ")})`);
  }
  if (e.new_failures !== undefined && c.newFailures !== null && c.newFailures.length !== e.new_failures) {
    problems.push(`${where}: should have ${e.new_failures} NEW failure(s), found ${c.newFailures.length}: ${c.newFailures.join("; ")}`);
  }
}

type Checked = { where: string; task: string; behavior: string; dir: string; npm: TestRun; hidden: Hidden[]; incidents: Incident[]; expect: Expect; newFailures: string[] | null };

async function checkFake(s: Scenario, repo: string, base: string, work: string, task: string, f: Fake, sets: string[], baseline: Promise<Baseline>): Promise<Checked> {
  const dir = join(work, `${task}-${f.behavior}`);
  checkoutRev(repo, base, dir);
  applyFake(s, task, f, dir);
  git(dir, ["add", "-A"]);
  const incidents = gaming(dir, base);
  const npm = await npmTest(dir);
  const nf = f.expect?.new_failures !== undefined ? await newFailures(dir, suitesOf(s), s.suite_env ?? {}, await baseline, (s.flaky ?? []).map((f) => f.name)) : null;
  const hidden = await hiddenTests(s, sets, dir);
  return { where: `${task}/${f.behavior}`, task, behavior: f.behavior, dir, npm, hidden, incidents, expect: f.expect ?? {}, newFailures: nf };
}

/** Flaky means it both fails and passes; the band is wide because 20 runs are few (binomial sd ~0.1 at p=0.3). */
function inBand(failed: number, runs: number, rate: number): boolean {
  return failed > 0 && failed < runs && Math.abs(failed / runs - rate) <= 0.25;
}

/**
 * Each declared flaky test's failure rate over FLAKE_RUNS runs of its file
 * on the base. A count outside the band gets FLAKE_RUNS more runs and is
 * judged on all of them, so a 1-in-50 draw does not fail the self-test.
 */
async function measureFlakes(s: Scenario, baseDir: string, problems: string[]): Promise<FlakeRate[]> {
  const out: FlakeRate[] = [];
  const files = [...new Set((s.flaky ?? []).map((f) => f.file))];
  const counts = new Map(await Promise.all(files.map(async (file) => [file, await flakeCounts(baseDir, file, FLAKE_RUNS)] as const)));
  for (const f of s.flaky ?? []) {
    let failed = counts.get(f.file)?.get(f.name) ?? 0;
    let runs = FLAKE_RUNS;
    if (!inBand(failed, runs, f.rate)) {
      failed += (await flakeCounts(baseDir, f.file, FLAKE_RUNS)).get(f.name) ?? 0;
      runs += FLAKE_RUNS;
    }
    out.push({ name: f.name, file: f.file, rate: f.rate, failed, runs });
    if (!inBand(failed, runs, f.rate)) {
      problems.push(`flaky "${f.name}" (${f.file}) should fail about ${Math.round(f.rate * 100)}% of runs, failed ${failed}/${runs}`);
    }
  }
  return out;
}

export async function validate(s: Scenario, work: string): Promise<Validation> {
  const problems: string[] = [];
  const notes: string[] = [];
  const totals: Record<string, number> = {};
  const repo = join(work, "repo");
  const base = buildRepo(s, repo);
  const files = git(repo, ["ls-files"]).split("\n").filter(Boolean).length;
  notes.push(`${files} files, ${s.history.length} commits`);

  const baseDir = join(work, "base");
  checkoutRev(repo, base, baseDir);
  const suites = suitesOf(s);
  // Its own checkout: the hidden tests run in baseDir meanwhile.
  const roundsDir = join(work, "base-rounds");
  checkoutRev(repo, base, roundsDir);
  const baseline = suiteRounds(roundsDir, suites, s.suite_env ?? {}, BASE_ROUNDS);
  const sets = [...s.tasks.map((t) => t.id), ...(s.combined ? ["combined"] : [])];
  const fakes = [
    ...s.tasks.flatMap((t) => t.fakes.map((f) => checkFake(s, repo, base, work, t.id, f, [t.id], baseline))),
    ...(s.combined ? [checkFake(s, repo, base, work, "combined", { behavior: "solution", expect: { hidden: s.combined.expect_hidden ?? "all" } }, hiddenSets(s, "combined"), baseline)] : []),
  ];
  const npm = await npmTest(baseDir);
  const baseHidden = await hiddenTests(s, sets, baseDir);
  const checked = await Promise.all(fakes);
  const rounds = await baseline;
  const flaky = await measureFlakes(s, baseDir, problems);

  if (typeof s.visible_failing === "number" && npm.fail !== s.visible_failing) {
    problems.push(`base: npm test should have ${s.visible_failing} failing, got ${npm.fail} (${npm.failing.join("; ")})`);
  }
  const contract = contractProblem(s, baseDir, npm.tests);
  if (contract) problems.push(`base: ${contract}`);
  notes.push(`base npm test: ${npm.tests} tests, ${npm.pass} pass, ${npm.fail} fail`);
  if (suites.length > 1 || suites[0] !== "test") notes.push(`base suites ${suites.join(", ")}: ${rounds.map((r) => r.length).join("/")} failing per round`);
  for (const f of flaky) notes.push(`flaky "${f.name}": failed ${f.failed}/${f.runs} (stated ${Math.round(f.rate * 100)}%)`);
  for (const h of baseHidden) if (h.fail === 0) problems.push(`base: hidden/${h.set} should fail somewhere on the base`);
  for (const c of checked) {
    if (c.behavior === "solution") {
      for (const h of c.hidden) if (h.set === c.task) totals[h.set] = h.tests;
    }
  }
  const difficulty: Validation["difficulty"] = {};
  for (const c of checked) {
    checkExpect(c.where, c.expect, c, s, problems);
    const rate = hiddenRate(c.hidden, totals);
    if (c.task !== "combined") (difficulty[c.task] ??= []).push({ behavior: c.behavior, hidden_rate: rate });
    const h = c.hidden.map((x) => `${x.pass}/${x.pass + x.fail}`).join(" ");
    const nf = c.newFailures === null ? "" : `, ${c.newFailures.length} NEW`;
    notes.push(`${c.where}: hidden ${h}, npm test ${c.npm.fail} failing${nf}, gaming ${isGaming(c.incidents, s.tests_may_change)}`);
  }
  if (s.combined) {
    const solutions = checked.filter((c) => c.behavior === "solution" && c.task !== "combined");
    if (solutions.length >= 2) notes.push(`a merge of the ${solutions.length} solutions ${mergeClean(solutions.map((c) => c.dir)) ? "is clean" : "conflicts"}`);
  }
  return { scenario: s.name, ok: problems.length === 0, problems, notes, totals, baseline: rounds, flaky, difficulty };
}

/** Whether the staged trees of scoring checkouts merge without conflict, one after another. */
export function mergeClean(dirs: string[]): boolean {
  const [first, ...rest] = dirs;
  if (!first) return true;
  for (const d of dirs) commitStaged(d, "attempt");
  let tree = git(first, ["rev-parse", "HEAD"]).trim();
  for (const d of rest) {
    git(first, ["fetch", "-q", d, "HEAD"]);
    const r = run("git", ["-C", first, "merge-tree", "--write-tree", tree, "FETCH_HEAD"]);
    if (r.status !== 0) return false;
    const merged = r.stdout.split("\n")[0]!.trim();
    tree = git(first, ["commit-tree", merged, "-p", tree, "-p", "FETCH_HEAD", "-m", "merge"]).trim();
  }
  return true;
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const keep = args.includes("--keep");
  const setAt = args.indexOf("--set");
  const set = (setAt >= 0 ? args[setAt + 1] : "all") as SetName;
  if (!SETS.includes(set)) throw new Error(`no set ${set}`);
  const names = args.filter((a, i) => !a.startsWith("--") && args[i - 1] !== "--set");
  const work = mkdtempSync(join(tmpdir(), "aver-eval-validate-"));
  let failed = 0;
  for (const name of names.length > 0 ? names : scenarioNames(set)) {
    const v = await validate(loadScenario(name), join(work, name));
    console.log(`${v.ok ? "ok  " : "FAIL"} ${name}`);
    for (const n of v.notes) console.log(`     ${n}`);
    for (const p of v.problems) console.log(`  !! ${p}`);
    if (!v.ok) failed += 1;
  }
  if (keep) console.log(`kept ${work}`);
  else rmSync(work, { recursive: true, force: true });
  process.exit(failed === 0 ? 0 : 1);
}

if (import.meta.main) await main();
