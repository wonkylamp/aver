// A scenario of the evaluation suite: its task.json and the repository its
// history/ layers build.
import { existsSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { fresh, git, gitEnv, overlay } from "./util.ts";

export const TASKS_DIR = resolve(import.meta.dirname, "..", "tasks");

/**
 * The easy set is round 1's six scenarios; the hard set is built to separate
 * the arms on correctness; the scale set runs many tasks on one repository
 * at once; the falsedone set's tasks look finished to a naive agent and are
 * not.
 */
export const SETS = ["easy", "hard", "scale", "falsedone", "all"] as const;
export type SetName = (typeof SETS)[number];
export type ScenarioSet = Exclude<SetName, "all">;

export type Expect = {
  /** Hidden tests on the base plus the overlay: every one passes, or not. */
  hidden?: "all" | "not-all";
  /** Whether the overlay counts as test gaming. */
  gaming?: boolean;
  /** `npm test`: green, red, or the exact number of failing tests. */
  npm_test?: "green" | "red" | number;
  /** Tests of the scenario's suites that pass on every base run and fail on every run with the overlay. */
  new_failures?: number;
};

export type Fake = { behavior: string; delete?: string[]; expect?: Expect };

export type SubTask = {
  id: string;
  statement: string;
  /** `id: text`, as `aver run -c` takes them. */
  criteria: string[];
  /**
   * Who is told the criteria: "both" (arm A as "Done means" lines, arm B
   * as `-c`), or "B" when the criteria stand for what Aver's criteria
   * mechanism adds (derived from the repository's docs) and arm A gets
   * only the statement.
   */
  criteria_for?: "both" | "B";
  /**
   * Tasks whose results this one builds on, for a future `after:` task
   * graph. Not used yet: every task starts on the base at the same time, so
   * a dependent task redoes what it depends on. Checked to name other tasks.
   */
  after?: string[];
  fakes: Fake[];
};

export type Layer = { dir: string; message: string; delete?: string[] };

/** A test known to be flaky on the base, and its failure rate (validated over repeated runs). */
export type Flaky = { file: string; name: string; rate: number };

export type Scenario = {
  name: string;
  dir: string;
  set: ScenarioSet;
  title: string;
  measures: string;
  history: Layer[];
  /** Failing tests in `npm test` on the base, or "flaky". */
  visible_failing: number | "flaky";
  /** A refactor may have to touch existing tests: modified lines are then not gaming. */
  tests_may_change: boolean;
  /** Flaky tests on the base. */
  flaky?: Flaky[];
  /**
   * The npm scripts whose NEW failures are counted (default `test`): a
   * test that passes on every base run and fails on every run of an attempt.
   */
  suites?: string[];
  /** Environment for the scoring runs of `suites` (never given to agents). */
  suite_env?: Record<string, string>;
  tasks: SubTask[];
  /** Tasks whose winners are merged and scored together (hidden/combined/). */
  combined?: { expect_hidden?: "all" | "not-all" };
};

export function scenarioNames(set: SetName = "all", tasksDir = TASKS_DIR): string[] {
  return readdirSync(tasksDir)
    .filter((n) => existsSync(join(tasksDir, n, "task.json")))
    .filter((n) => set === "all" || loadScenario(n, tasksDir).set === set)
    .sort();
}

export function loadScenario(name: string, tasksDir = TASKS_DIR): Scenario {
  const dir = join(tasksDir, name);
  const raw = JSON.parse(readFileSync(join(dir, "task.json"), "utf8")) as Omit<Scenario, "name" | "dir" | "set"> & { set?: ScenarioSet };
  const s: Scenario = { name, dir, ...raw, set: raw.set ?? "easy" };
  if (!SETS.includes(s.set) || (s.set as SetName) === "all") throw new Error(`${name}: set ${String(s.set)}`);
  if (!Array.isArray(s.history) || s.history.length === 0) throw new Error(`${name}: no history`);
  if (!Array.isArray(s.tasks) || s.tasks.length === 0) throw new Error(`${name}: no tasks`);
  for (const t of s.tasks) {
    if (!/^[a-z0-9-]+$/.test(t.id)) throw new Error(`${name}: task id ${t.id}`);
    const ids = new Set<string>();
    for (const c of t.criteria) {
      if (!/^[a-z0-9][a-z0-9._-]*: \S/.test(c)) throw new Error(`${name}: criterion "${c}" is not "id: text"`);
      const id = c.slice(0, c.indexOf(":"));
      if (ids.has(id)) throw new Error(`${name}/${t.id}: criterion id ${id} appears twice`);
      ids.add(id);
    }
    if (t.criteria_for !== undefined && t.criteria_for !== "both" && t.criteria_for !== "B") {
      throw new Error(`${name}/${t.id}: criteria_for ${String(t.criteria_for)}`);
    }
    if (!t.fakes.some((f) => f.behavior === "solution")) throw new Error(`${name}/${t.id}: no solution fake`);
    if (s.set === "hard" && !t.fakes.some((f) => f.behavior !== "solution" && f.expect?.hidden === "not-all")) {
      throw new Error(`${name}/${t.id}: a hard scenario needs a naive fake that fails the hidden tests`);
    }
    if (s.set === "falsedone" && !t.fakes.some((f) => f.behavior !== "solution" && f.expect?.hidden === "not-all" && f.expect.npm_test === "green")) {
      throw new Error(`${name}/${t.id}: a falsedone scenario needs a naive fake whose tests are green and whose hidden tests fail`);
    }
    for (const a of t.after ?? []) {
      if (a === t.id || !s.tasks.some((o) => o.id === a)) throw new Error(`${name}/${t.id}: after names ${a}, not another task`);
    }
  }
  for (const f of s.flaky ?? []) {
    if (!(f.rate > 0 && f.rate < 1)) throw new Error(`${name}: flaky ${f.name} rate ${f.rate}`);
  }
  if (s.combined && !existsSync(join(dir, "hidden", "combined"))) throw new Error(`${name}: combined without hidden/combined`);
  if (s.combined && s.tasks.length < 2) throw new Error(`${name}: combined needs two tasks or more`);
  return s;
}

/** The npm scripts whose NEW failures count. */
export function suitesOf(s: Scenario): string[] {
  return s.suites && s.suites.length > 0 ? s.suites : ["test"];
}

/** The criteria arm A's prompt carries. */
export function criteriaForA(t: SubTask): string[] {
  return t.criteria_for === "B" ? [] : t.criteria;
}

/**
 * Builds the scenario's repository in `dest`: one commit per history layer,
 * with a fixed identity and fixed dates. Returns the base commit.
 */
export function buildRepo(s: Scenario, dest: string): string {
  fresh(dest);
  git(dest, ["init", "-q", "--initial-branch=main"]);
  let at = 1767225600; // 2026-01-01
  for (const layer of s.history) {
    for (const p of layer.delete ?? []) rmSync(join(dest, p), { recursive: true, force: true });
    overlay(join(s.dir, "history", layer.dir), dest);
    git(dest, ["add", "-A"]);
    git(dest, ["commit", "-q", "--allow-empty", "-m", layer.message], gitEnv(at));
    at += 3 * 86400 + 3600;
  }
  return git(dest, ["rev-parse", "HEAD"]).trim();
}

/** Writes a fake behavior's files (and deletions) over `dir`. */
export function applyFake(s: Scenario, taskId: string, fake: Fake, dir: string): void {
  for (const p of fake.delete ?? []) rmSync(join(dir, p), { recursive: true, force: true });
  const src = join(s.dir, "fakes", taskId, fake.behavior);
  if (existsSync(src)) overlay(src, dir);
}

export function hiddenDir(s: Scenario, id: string): string {
  return join(s.dir, "hidden", id);
}

/** The task ids whose hidden tests score a tree of task `id`; "combined" scores all. */
export function hiddenSets(s: Scenario, id: string): string[] {
  if (id !== "combined") return [id];
  return [...s.tasks.map((t) => t.id), "combined"];
}
