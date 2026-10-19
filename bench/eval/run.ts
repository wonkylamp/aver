// Aver's A/B evaluation: the same agents on the same hard tasks, in a plain
// Git worktree (arm A) and under `aver run` (arm B), scored by hidden
// acceptance tests the agents never see. Scenarios run one at a time; each
// one's results go to results/<label>-<scenario>.json, written after every
// arm, so a rerun with the same label skips what is done. results.md is
// regenerated from every results file at the end.
//
//   make eval                                   # build aver, run everything
//   node bench/eval/run.ts [options]
//
// Options:
//   --set <set>            which scenarios: easy (default; round 1's six), hard
//                          (built to separate the arms on correctness), scale
//                          (many tasks on one repository at once), falsedone
//                          (tasks that look finished and are not), or all
//   --scenarios <a,b>      instead of --set: these scenarios
//   --arms A,B             default: both, A first
//   --agents <n>           agents per arm and task, default 2
//   --label <label>        names the results files, default today's date
//   --fresh                redo scenarios and arms that already have results
//   --timeout <dur>        wall time per agent, default 20m
//   --max-budget-usd <x>   per agent and process, default 5
//   --model <model>        passed to claude in both arms
//   --no-feedback          skip the four feedback questions
//   --work <dir>           repositories, worktrees and logs (default $TMPDIR/aver-eval/<label>)
//   --results <dir>        default bench/eval/results (dry runs: <work>/results)
//   --aver <bin>           default target/release/aver (a .ts file runs with node)
//   --claude <bin>         default claude
//   --dry-run              scripted fake agents instead of claude (fake-agent.ts)
//   --report               only regenerate results.md from the results files
//
// Arm A: per agent, `git worktree add` of the task repository and
// `claude -p` with acceptEdits and the allowlist Aver gives its own agents
// (npm test and every npm script included), minus Aver's commands, plus
// `git stash` (how a plain-checkout agent sees the original code). The
// prompt is the task, its criteria as "Done means" lines (unless the
// scenario gives criteria to arm B only) and "Run the tests with npm test."
// (a Python scenario: `python3 -m unittest`, and `python3` is allowed).
// Its pick: the attempt whose `npm test` is green, else the fewest failing
// tests, then the smallest diff.
// Arm B: `aver run` with the same task and criteria (`-c`), `--agent claude`,
// the same wall time (as both --timeout and --idle, since arm A has no idle
// cut-off) and the same --max-budget-usd. Its pick: Aver's rank 1.
//
// Before the arms, each scenario is validated (validate.ts) and passes a
// difficulty check: a scenario whose reference solution fails its hidden
// tests is aborted, never run on agents. Every attempt is scored by its
// diff, test gaming, `npm test`, NEW failures in the scenario's suites
// (passing on every base round, failing on every rerun of the attempt) and
// the hidden tests.
//
// Scenarios with several tasks merge the picks: one after another until a
// conflict (`merge`), and across all of them (`all`): every pair on its own,
// then the picks in task order skipping each one that conflicts with what is
// merged so far, scored per task on that merge. An attempt *claims done*
// when arm A's agent ends normally with its tests green, or arm B's agent
// ends `done` (its latest receipt PASS); it is *false done* when it claims
// done and fails hidden tests.
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, openSync, closeSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, resolve } from "node:path";
import type { AgentLog } from "./lib/agentlog.ts";
import { parseAgentLog, rating } from "./lib/agentlog.ts";
import type { Baseline, DiffStat, Hidden, Incident, TestRun } from "./lib/score.ts";
import { checkoutRev, commitStaged, gaming, hiddenRate, hiddenTests, isGaming, isPython, materialize, newFailures, npmTest, patch, testCommand } from "./lib/score.ts";
import type { Scenario, ScenarioSet, SetName, SubTask } from "./lib/task.ts";
import { buildRepo, criteriaForA, hiddenSets, loadScenario, scenarioNames, SETS, suitesOf, TASKS_DIR } from "./lib/task.ts";
import { fresh, git, gitEnv, parseDuration, run, runAsync, spawnLogged, today } from "./lib/util.ts";
import type { FlakeRate, Validation } from "./validate.ts";
import { validate } from "./validate.ts";
import { askedFor } from "./lib/asked.ts";

const ROOT = resolve(import.meta.dirname, "..", "..");
const HERE = import.meta.dirname;

export type Arm = "A" | "B";

export type Options = {
  set: SetName;
  scenarios: string[];
  arms: Arm[];
  agents: number;
  label: string;
  fresh: boolean;
  timeoutMs: number;
  maxBudgetUsd: number;
  model: string | null;
  feedback: boolean;
  work: string;
  results: string;
  aver: string;
  claude: string;
  dryRun: boolean;
  tasksDir: string;
  /** Where progress lines go; stderr when unset. The self-tests collect them instead. */
  log?: (line: string) => void;
};

export type Feedback = { rating: number | null; answer: string } | { error: string };

export type Attempt = {
  task: string;
  index: number;
  /** Relative to the work directory. */
  dir: string;
  wall_ms: number;
  timed_out: boolean;
  /** Arm B: the supervisor's last state for the agent (done, budget, timeout, ...). */
  state: string | null;
  /** Arm B: the attempt's place in Aver's ranking. */
  place: number | null;
  log: AgentLog;
  diff: DiffStat;
  gaming: Incident[];
  gamed: boolean;
  npm_test: TestRun;
  /** Tests of the scenario's suites (`<script>: <name>`) that pass on every base round and fail on every rerun here. */
  new_failures: string[];
  hidden: Hidden[];
  hidden_rate: number;
  correct: boolean;
  feedback: Feedback | null;
};

export type Combined = {
  /** Paths two or more picks changed. */
  overlap: string[];
  /** Git's merges of the picks, one after another: clean, or the conflicted paths. */
  merge: "clean" | { conflicts: string[] };
  /** Arm B: `aver combine`'s exit code and first line. */
  aver_combine: { exit: number; summary: string } | null;
  hidden: Hidden[] | null;
  hidden_rate: number;
  /** Merge outcomes across all the picks (absent in results from before it). */
  all?: MergeAll;
};

export type MergeAll = {
  /** Every pair of picks merged on its own: the conflicted paths, empty when clean. */
  pairs: Array<{ a: string; b: string; conflicts: string[] }>;
  /** Tasks whose picks merged, in task order, each onto the merge of those before it. */
  merged: string[];
  /** Tasks whose pick conflicted with the merge so far, and the conflicted paths. */
  skipped: Array<{ task: string; conflicts: string[] }>;
  /** Hidden pass rate on that merge, per task and for "combined". */
  hidden: Record<string, number>;
};

export type ArmResult = {
  arm: Arm;
  complete: boolean;
  wall_ms: number;
  attempts: Attempt[];
  /** Per task: the attempt each arm's own rule picks. */
  picks: Record<string, { index: number | null; why: string }>;
  combined: Combined | null;
  /** Arm B: run ids per task. */
  runs: Record<string, string>;
};

export type ScenarioResult = {
  schema: "aver.eval/v0";
  label: string;
  scenario: string;
  set?: ScenarioSet;
  title: string;
  measures: string;
  dry_run: boolean;
  config: { agents: number; timeout_ms: number; max_budget_usd: number; model: string | null; aver_commit: string; claude_version: string | null };
  hidden_totals: Record<string, number>;
  /** The difficulty check: each scripted behavior's hidden pass rate (the reference must be 100%). */
  difficulty?: Validation["difficulty"];
  /** Declared flaky tests and their measured rates on the base. */
  flaky?: FlakeRate[];
  arms: Partial<Record<Arm, ArmResult>>;
  complete: boolean;
};

export const QUESTIONS_A = `You are being asked the same questions as every agent of this evaluation about the environment you worked in (the plain checkout of the repository, its test suite and the tools you had). Answer from your experience in this session. Do not modify any files and do not run commands that change anything: just answer.

1. What helped and what was friction? Quote the moments (a line of output, a command, an error).
2. On hard tasks (flaky or already-failing test suites, unclear done criteria, other agents editing the same code), how well would this environment serve you? What would you be missing, if anything? Why?
3. Rate this environment from 1 to 10 against "I absolutely need this". Put the number on a line of its own as RATING: <n>/10.
4. What one change would raise your rating the most?`;

/** Claude Code tools for arm A: Aver's allowlist without Aver's commands, plus `git stash` (and `python3` in a Python scenario). */
export function armAllowlist(pkgJson: string, python = false): string[] {
  const commands = [
    "git status", "git diff", "git log", "git show", "git ls-files", "git stash", "node", "npm test", "npm run test",
    "python -m pytest", "python3 -m pytest", "python -m unittest", "python3 -m unittest", "pytest", "cargo test", "go test",
    "ls", "pwd",
  ];
  if (python) commands.push("python3");
  try {
    const scripts = (JSON.parse(pkgJson) as { scripts?: Record<string, string> }).scripts ?? {};
    for (const name of Object.keys(scripts)) if (/^[A-Za-z0-9:._-]{1,64}$/.test(name)) commands.push(`npm run ${name}`);
  } catch {
    // no scripts
  }
  const allow = ["Read", "Edit", "Write", "MultiEdit", "Glob", "Grep", "LS"];
  for (const c of [...new Set(commands)]) allow.push(`Bash(${c})`, `Bash(${c}:*)`);
  return allow;
}

export function promptA(t: SubTask, command = "npm test"): string {
  const criteria = criteriaForA(t);
  const done = criteria.length === 0 ? "" : `\n\nDone means:\n${criteria.map((c) => `- ${c.slice(c.indexOf(":") + 1).trim()}`).join("\n")}`;
  return `${t.statement.trim()}${done}\n\nRun the tests with ${command}.`;
}

/** The `claude -p` arguments of an arm A agent: the same wall time and budget as arm B's. */
export function claudeArgsA(o: Options, t: SubTask, session: string, allow: string, command = "npm test"): string[] {
  return [
    "-p", promptA(t, command),
    "--session-id", session,
    "--output-format", "stream-json", "--verbose",
    "--permission-mode", "acceptEdits",
    "--allowedTools", allow,
    ...claudeCommon(o),
  ];
}

/**
 * The `aver run` arguments for one task of arm B. Arm A has a wall-time
 * limit and no idle cut-off, so `--idle` is the wall time too; the budget
 * is the same `--max-budget-usd` per Claude process.
 */
export function averRunArgs(o: Options, t: SubTask, repo: string, agent: string[]): string[] {
  const secs = `${Math.ceil(o.timeoutMs / 1000)}s`;
  return [
    "run", t.statement.trim(),
    "--repo", repo,
    "--agents", String(o.agents),
    "--timeout", secs,
    "--idle", secs,
    "--no-tui", "--json",
    ...t.criteria.flatMap((c) => ["-c", c]),
    ...agent,
  ];
}

function avercmd(o: Options): [string, string[]] {
  return o.aver.endsWith(".ts") ? ["node", [o.aver]] : [o.aver, []];
}

function aver(o: Options, home: string, args: string[], cwd: string, timeoutMs = 600_000): ReturnType<typeof run> {
  const [cmd, pre] = avercmd(o);
  return run(cmd, [...pre, ...args], { cwd, env: { ...process.env, AVER_HOME: home }, timeoutMs });
}

const fakeAgent = (): string => join(HERE, "fake-agent.ts");

function rel(o: Options, p: string): string {
  return relative(o.work, p) || ".";
}

function claudeCommon(o: Options): string[] {
  const out = ["--max-budget-usd", String(o.maxBudgetUsd)];
  if (o.model) out.push("--model", o.model);
  return out;
}

/** Scores one attempt's directory. */
type Scored = Pick<Attempt, "task" | "diff" | "gaming" | "gamed" | "npm_test" | "new_failures" | "hidden" | "hidden_rate" | "correct">;

/** What scoring needs from the scenario's validation. */
type Judge = { totals: Record<string, number>; baseline: Baseline };

/** Scores one attempt's directory: `npm test` and the NEW failures first, then the hidden tests (which `npm test` would find). */
async function score(s: Scenario, j: Judge, repo: string, base: string, task: string, tree: string, dest: string): Promise<Scored> {
  const diff = materialize(repo, base, tree, dest);
  writeFileSync(`${dest}.diff`, patch(dest, base));
  const incidents = gaming(dest, base);
  const npm = await npmTest(dest);
  const nf = diff.files === 0 ? [] : await newFailures(dest, suitesOf(s), s.suite_env ?? {}, j.baseline, (s.flaky ?? []).map((f) => f.name));
  const hidden = await hiddenTests(s, [task], dest);
  const rate = hiddenRate(hidden, j.totals);
  return { task, diff, gaming: incidents, gamed: isGaming(incidents, s.tests_may_change), npm_test: npm, new_failures: nf, hidden, hidden_rate: rate, correct: rate === 1 };
}

/** Arm A's pick: green `npm test`, else fewest failures, then the smallest non-empty diff. */
export function pickA(attempts: Attempt[]): { index: number | null; why: string } {
  const changed = attempts.filter((a) => a.diff.files > 0);
  if (changed.length === 0) return { index: null, why: "no attempt changed anything" };
  const sorted = [...changed].sort(
    (a, b) =>
      Number(a.npm_test.exit !== 0) - Number(b.npm_test.exit !== 0) ||
      a.npm_test.fail - b.npm_test.fail ||
      a.diff.added + a.diff.removed - (b.diff.added + b.diff.removed) ||
      a.index - b.index,
  );
  const p = sorted[0]!;
  return { index: p.index, why: p.npm_test.exit === 0 ? "npm test green, smallest diff" : `no green attempt; fewest failing (${p.npm_test.fail})` };
}

async function armA(o: Options, s: Scenario, judge: Judge, dir: string): Promise<ArmResult> {
  const repo = join(dir, "repo");
  const base = buildRepo(s, repo);
  const python = isPython(repo);
  const allow = armAllowlist(python ? "{}" : readFileSync(join(repo, "package.json"), "utf8"), python).join(",");
  const command = testCommand(repo);
  type Job = { t: SubTask; i: number; wt: string; logPath: string; session: string };
  const jobs: Job[] = [];
  for (const t of s.tasks) {
    for (let i = 1; i <= o.agents; i++) {
      const wt = join(dir, `${t.id}-${i}`);
      git(repo, ["worktree", "add", "-q", "-b", `eval/${t.id}-${i}`, wt, base]);
      jobs.push({ t, i, wt, logPath: join(dir, `${t.id}-${i}.log`), session: randomUUID() });
    }
  }
  const start = Date.now();
  const ran = await Promise.all(
    jobs.map(async (j) => {
      const out = openSync(j.logPath, "w");
      const err = openSync(`${j.logPath}.stderr`, "w");
      const [cmd, args] = o.dryRun
        ? ["node", [fakeAgent(), "work", "--scenario", s.name, "--tasks-dir", o.tasksDir, "--task", j.t.id, "--dir", j.wt, "--index", String(j.i)]]
        : [o.claude, claudeArgsA(o, j.t, j.session, allow, command)];
      const r = await spawnLogged(cmd, args as string[], { cwd: j.wt, env: agentEnv(), stdout: out, stderr: err, timeoutMs: o.timeoutMs });
      closeSync(out);
      closeSync(err);
      return r;
    }),
  );
  const wall = Date.now() - start;
  const attempts: Attempt[] = await Promise.all(jobs.map(async (j, k) => {
    const r = ran[k]!;
    const sc = await score(s, judge, repo, base, j.t.id, j.wt, join(dir, "score", `${j.t.id}-${j.i}`));
    return {
      ...sc,
      index: j.i,
      dir: rel(o, j.wt),
      wall_ms: r.ms,
      timed_out: r.timedOut,
      state: null,
      place: null,
      log: parseAgentLog(j.logPath),
      feedback: null,
    };
  }));
  if (o.feedback) {
    await Promise.all(
      jobs.map(async (j, k) => {
        const a = attempts[k]!;
        a.feedback = await askA(o, j.wt, a.log.session_id ?? j.session, join(dir, `${j.t.id}-${j.i}.feedback`));
      }),
    );
  }
  const picks: ArmResult["picks"] = {};
  for (const t of s.tasks) picks[t.id] = pickA(attempts.filter((a) => a.task === t.id));
  const combined = s.combined ? await combineA(s, judge.totals, repo, base, dir, picks) : null;
  return { arm: "A", complete: true, wall_ms: wall, attempts, picks, combined, runs: {} };
}

function agentEnv(): NodeJS.ProcessEnv {
  const env = { ...process.env };
  for (const k of Object.keys(env)) if (k.startsWith("AVER_")) delete env[k];
  return env;
}

async function askA(o: Options, cwd: string, session: string, outPath: string): Promise<Feedback> {
  const out = openSync(outPath, "w");
  const err = openSync(`${outPath}.stderr`, "w");
  const [cmd, args] = o.dryRun
    ? ["node", [fakeAgent(), "feedback", QUESTIONS_A]]
    : [
        o.claude,
        ["-p", QUESTIONS_A, "--resume", session, "--output-format", "text", "--permission-mode", "plan", "--disallowedTools", "Edit,Write,MultiEdit,NotebookEdit", ...(o.model ? ["--model", o.model] : [])],
      ];
  const r = await spawnLogged(cmd, args as string[], { cwd, env: agentEnv(), stdout: out, stderr: err, timeoutMs: 600_000 });
  closeSync(out);
  closeSync(err);
  const answer = readFileSync(outPath, "utf8").trim();
  if (r.status !== 0 || answer === "") return { error: `exited ${r.status}${r.timedOut ? " (timeout)" : ""} without an answer` };
  return { rating: rating(answer), answer: answer.slice(0, 8000) };
}

/** Merges the picks one after another (as a person would with `git merge`) and scores the result. */
async function mergeAndScore(s: Scenario, totals: Record<string, number>, repo: string, revs: string[], dest: string): Promise<Pick<Combined, "merge" | "hidden" | "hidden_rate">> {
  const [first, ...rest] = revs;
  checkoutRev(repo, first!, dest);
  for (const rev of rest) {
    const m = run("git", ["-C", dest, "merge", "--no-edit", "-q", rev], { env: gitEnv() });
    if (m.status !== 0) {
      const conflicts = git(dest, ["diff", "--name-only", "--diff-filter=U"]).split("\n").filter(Boolean);
      return { merge: { conflicts }, hidden: null, hidden_rate: 0 };
    }
  }
  const hidden = await hiddenTests(s, hiddenSets(s, "combined"), dest);
  return { merge: "clean", hidden, hidden_rate: hiddenRate(hidden, totals) };
}

/**
 * Merge outcomes across all the picks: each pair on its own, then all of
 * them in task order, each onto the merge so far and skipped when it
 * conflicts, and the hidden tests of every task on that merge.
 */
export async function mergeAll(s: Scenario, totals: Record<string, number>, repo: string, picks: Array<{ task: string; rev: string }>, dest: string): Promise<MergeAll> {
  const pairs: MergeAll["pairs"] = [];
  picks.forEach((a, i) => {
    for (const b of picks.slice(i + 1)) pairs.push({ a: a.task, b: b.task, conflicts: mergeTree(repo, a.rev, b.rev).conflicts });
  });
  const merged: string[] = [];
  const skipped: MergeAll["skipped"] = [];
  let head: string | null = null;
  for (const p of picks) {
    if (head === null) {
      head = p.rev;
      merged.push(p.task);
      continue;
    }
    const m = mergeTree(repo, head, p.rev);
    if (m.tree === null) {
      skipped.push({ task: p.task, conflicts: m.conflicts });
      continue;
    }
    head = git(repo, ["commit-tree", m.tree, "-p", head, "-p", p.rev, "-m", `merge ${p.task}`]).trim();
    merged.push(p.task);
  }
  const hidden: Record<string, number> = {};
  if (head !== null) {
    checkoutRev(repo, head, dest);
    for (const h of await hiddenTests(s, hiddenSets(s, "combined"), dest)) hidden[h.set] = hiddenRate([h], totals);
  }
  return { pairs, merged, skipped, hidden };
}

/** `git merge-tree` of two commits: the merged tree, or null and the conflicted paths. */
function mergeTree(repo: string, a: string, b: string): { tree: string | null; conflicts: string[] } {
  const r = run("git", ["-C", repo, "merge-tree", "--write-tree", "--name-only", "--no-messages", a, b], { env: gitEnv() });
  const lines = r.stdout.split("\n").map((l) => l.trim());
  if (r.status === 0) return { tree: lines[0]!, conflicts: [] };
  if (r.status !== 1) throw new Error(`git merge-tree ${a} ${b}: ${r.stderr}`);
  return { tree: null, conflicts: [...new Set(lines.slice(1).filter(Boolean))].sort() };
}

/** Each task's pick, in task order, or null when a task has none. */
function pickedTasks(s: Scenario, picks: ArmResult["picks"]): Array<{ t: SubTask; index: number }> | null {
  const out: Array<{ t: SubTask; index: number }> = [];
  for (const t of s.tasks) {
    const index = picks[t.id]?.index;
    if (index == null) return null;
    out.push({ t, index });
  }
  return out;
}

async function combineA(s: Scenario, totals: Record<string, number>, repo: string, base: string, dir: string, picks: ArmResult["picks"]): Promise<Combined | null> {
  const picked = pickedTasks(s, picks);
  if (!picked) return null;
  const dirs = picked.map((p) => join(dir, "score", `${p.t.id}-${p.index}`));
  const overlap = overlapOf(dirs, base);
  const heads = picked.map((p, k) => {
    const h = commitStaged(dirs[k]!, `${p.t.id} pick`);
    git(repo, ["fetch", "-q", dirs[k]!, `${h}:refs/heads/eval/pick-${p.t.id}`]);
    return h;
  });
  const r = await mergeAndScore(s, totals, repo, heads, join(dir, "score", "combined"));
  const all = await mergeAll(s, totals, repo, picked.map((p, k) => ({ task: p.t.id, rev: heads[k]! })), join(dir, "score", "merge-all"));
  return { overlap, aver_combine: null, ...r, all };
}

/** Paths that two or more of the scoring checkouts changed. */
function overlapOf(dirs: string[], base: string): string[] {
  const seen = new Map<string, number>();
  for (const d of dirs) {
    for (const p of new Set(git(d, ["diff", "--cached", "--name-only", base]).split("\n").filter(Boolean))) seen.set(p, (seen.get(p) ?? 0) + 1);
  }
  return [...seen].filter(([, n]) => n >= 2).map(([p]) => p).sort();
}

type AverRun = {
  run: {
    id: string;
    agents: Array<{
      index: number;
      attempt: string;
      dir: string;
      state: string;
      log: string;
      usage?: { cost_usd?: number };
      started_at: string;
      ended_at: string | null;
      feedback?: { rating: number | null; file: string };
    }>;
  };
  ranking: Array<{ attempt: string; place: number; why?: string; tie?: { with: string; by: string } }>;
};

async function armB(o: Options, s: Scenario, judge: Judge, dir: string): Promise<ArmResult> {
  const repo = join(dir, "repo");
  const base = buildRepo(s, repo);
  git(repo, ["config", "user.name", "Eval Fixture"]);
  git(repo, ["config", "user.email", "fixture@example.invalid"]);
  const home = join(dir, "home");
  mkdirSync(home, { recursive: true });
  const init = aver(o, home, ["init"], repo);
  if (init.status !== 0) throw new Error(`aver init: ${init.stderr}`);
  const agentArgs = o.dryRun
    ? (t: SubTask): string[] => [
        "--agent-cmd",
        `node ${shq(fakeAgent())} work --scenario ${shq(s.name)} --tasks-dir ${shq(o.tasksDir)} --task ${shq(t.id)} --dir {dir}`,
      ]
    : (): string[] => ["--agent", "claude", ...claudeCommon(o)];
  const [cmd, pre] = avercmd(o);
  const start = Date.now();
  const outs = await Promise.all(
    s.tasks.map(async (t) => {
      const outPath = join(dir, `run-${t.id}.json`);
      const out = openSync(outPath, "w");
      const err = openSync(join(dir, `run-${t.id}.log`), "w");
      const args = [...pre, ...averRunArgs(o, t, repo, agentArgs(t))];
      const r = await spawnLogged(cmd, args, { cwd: repo, env: { ...process.env, AVER_HOME: home }, stdout: out, stderr: err, timeoutMs: o.timeoutMs + 20 * 60_000 });
      closeSync(out);
      closeSync(err);
      const text = readFileSync(outPath, "utf8");
      const json = text.slice(text.indexOf("{"));
      if (!json.startsWith("{")) throw new Error(`aver run for ${t.id} printed no JSON (exit ${r.status}); see ${join(dir, `run-${t.id}.log`)}`);
      return { t, r, parsed: JSON.parse(json) as AverRun };
    }),
  );
  const wall = Date.now() - start;
  const picks: ArmResult["picks"] = {};
  const runs: Record<string, string> = {};
  const scoring: Array<Promise<Attempt>> = [];
  for (const { t, parsed } of outs) {
    runs[t.id] = parsed.run.id;
    const place = new Map(parsed.ranking.map((r) => [r.attempt, r.place]));
    for (const a of parsed.run.agents) {
      scoring.push(
        score(s, judge, repo, base, t.id, a.dir, join(dir, "score", `${t.id}-${a.index}`)).then((sc) => {
          const log = parseAgentLog(a.log);
          if (a.usage?.cost_usd !== undefined) log.cost_usd = a.usage.cost_usd;
          return {
            ...sc,
            index: a.index,
            dir: rel(o, a.dir),
            wall_ms: a.ended_at ? Date.parse(a.ended_at) - Date.parse(a.started_at) : 0,
            timed_out: a.state === "timeout",
            state: a.state,
            place: place.get(a.attempt) ?? null,
            log,
            feedback: null,
          };
        }),
      );
    }
    const first = parsed.ranking.find((r) => r.place === 1);
    const winner = parsed.run.agents.find((a) => a.attempt === first?.attempt);
    const tied = parsed.run.agents.find((a) => a.attempt === first?.tie?.with);
    const why = first?.why ? `Aver's rank 1: ${first.why}` : "Aver's rank 1";
    const tie = first?.tie && tied ? `; tie on evidence with #${tied.index}, decided by ${first.tie.by.replace("_", " ")}` : "";
    picks[t.id] = winner ? { index: winner.index, why: why + tie } : { index: null, why: "no ranking" };
  }
  const attempts = await Promise.all(scoring);
  if (o.feedback) {
    await Promise.all(
      outs.map(async ({ t, parsed }) => {
        const args = ["feedback", parsed.run.id, ...(o.dryRun ? ["--resume-cmd", `node ${shq(fakeAgent())} feedback {feedback}`] : [])];
        const [cmd, pre] = avercmd(o);
        const r = await runAsync(cmd, [...pre, ...args], { cwd: repo, env: { ...process.env, AVER_HOME: home }, timeoutMs: 15 * 60_000 });
        const status = aver(o, home, ["status", parsed.run.id, "--json"], repo);
        const after = status.status === 0 ? (JSON.parse(status.stdout.slice(status.stdout.indexOf("{"))) as AverRun) : null;
        for (const a of attempts.filter((x) => x.task === t.id)) {
          const f = after?.run.agents.find((x) => x.index === a.index)?.feedback;
          if (!f) {
            a.feedback = { error: `aver feedback exited ${r.status}: ${(r.stdout + r.stderr).trim().slice(-300)}` };
            continue;
          }
          const text = existsSync(f.file) ? readFileSync(f.file, "utf8") : "";
          const answer = text.includes("## Answer") ? text.slice(text.indexOf("## Answer") + 9).trim() : text;
          a.feedback = { rating: f.rating, answer: answer.slice(0, 8000) };
        }
      }),
    );
  }
  const combined = s.combined ? await combineB(o, s, judge.totals, repo, base, dir, home, runs, picks) : null;
  return { arm: "B", complete: true, wall_ms: wall, attempts, picks, combined, runs };
}

async function combineB(o: Options, s: Scenario, totals: Record<string, number>, repo: string, base: string, dir: string, home: string, runs: Record<string, string>, picks: ArmResult["picks"]): Promise<Combined | null> {
  const picked = pickedTasks(s, picks);
  if (!picked) return null;
  const overlap = overlapOf(picked.map((p) => join(dir, "score", `${p.t.id}-${p.index}`)), base);
  for (const { t, index } of picked) {
    const r = aver(o, home, ["accept", String(index), "--run", runs[t.id]!, "--branch", `eval/accepted-${t.id}`, "--force"], repo);
    if (r.status !== 0) throw new Error(`aver accept ${index} --run ${runs[t.id]}: ${r.stdout}${r.stderr}`);
  }
  const c = aver(o, home, ["combine", ...picked.map((p) => runs[p.t.id]!), "--branch", "eval/combined"], repo, 30 * 60_000);
  writeFileSync(join(dir, "combine.out"), c.stdout + c.stderr);
  const summary = (c.stdout.split("\n").find((l) => l.trim() !== "") ?? "").trim();
  const dest = join(dir, "score", "combined");
  const revs = picked.map((p) => git(repo, ["rev-parse", `eval/accepted-${p.t.id}`]).trim());
  let scored: Pick<Combined, "merge" | "hidden" | "hidden_rate">;
  if (c.status === 0) {
    checkoutRev(repo, git(repo, ["rev-parse", "eval/combined"]).trim(), dest);
    const hidden = await hiddenTests(s, hiddenSets(s, "combined"), dest);
    scored = { merge: "clean", hidden, hidden_rate: hiddenRate(hidden, totals) };
  } else {
    scored = await mergeAndScore(s, totals, repo, revs, dest);
  }
  const all = await mergeAll(s, totals, repo, picked.map((p, k) => ({ task: p.t.id, rev: revs[k]! })), join(dir, "score", "merge-all"));
  return { overlap, aver_combine: { exit: c.status, summary }, ...scored, all };
}

function shq(s: string): string {
  return /^[\w./=:,+@%-]+$/.test(s) ? s : `'${s.replace(/'/g, "'\\''")}'`;
}

function versionOf(o: Options): { aver_commit: string; claude_version: string | null } {
  const commit = run("git", ["-C", ROOT, "rev-parse", "--short=12", "HEAD"]).stdout.trim();
  const dirty = run("git", ["-C", ROOT, "status", "--porcelain", "--", "crates"]).stdout.trim() !== "";
  const claude = o.dryRun ? null : run(o.claude, ["--version"]).stdout.trim() || null;
  return { aver_commit: commit + (dirty ? "-dirty" : ""), claude_version: claude };
}

export async function evaluate(o: Options): Promise<ScenarioResult[]> {
  const say = o.log ?? log;
  mkdirSync(o.results, { recursive: true });
  const out: ScenarioResult[] = [];
  const failed: string[] = [];
  for (const name of o.scenarios) {
    try {
      const r = await evaluateOne(o, name);
      out.push(r);
    } catch (e) {
      // One broken scenario must not cost the rest of an unattended run; a rerun resumes it.
      failed.push(`${name}: ${e instanceof Error ? e.message : String(e)}`);
      say(`${name}: FAILED, moving on: ${failed.at(-1)}`);
    }
  }
  writeFileSync(join(o.results, "..", "results.md"), renderMarkdown(loadResults(o.results)));
  if (failed.length > 0) throw new Error(`${failed.length} scenario(s) failed; rerun the same command to resume them:\n${failed.join("\n")}`);
  return out;
}

async function evaluateOne(o: Options, name: string): Promise<ScenarioResult> {
  const say = o.log ?? log;
  const s = loadScenario(name, o.tasksDir);
  const file = join(o.results, `${o.label}-${name}.json`);
  const prior = existsSync(file) && !o.fresh ? (JSON.parse(readFileSync(file, "utf8")) as ScenarioResult) : null;
  if (prior?.complete && o.arms.every((a) => prior.arms[a]?.complete)) {
    say(`${name}: done already (${file}); --fresh redoes it`);
    return prior;
  }
  say(`${name}: validating the scenario`);
  const v = await validate(s, fresh(join(o.work, name, "validate")));
  say(`${name}: difficulty check`);
  const d = difficultyCheck(v);
  if (!d.ok) throw new Error(`difficulty check failed, scenario aborted before any agent ran: ${d.why}`);
  say(`${name}: ${d.summary}`);
  if (!v.ok) throw new Error(`${name} is not sound:\n  ${v.problems.join("\n  ")}`);
  const result: ScenarioResult = prior ?? {
    schema: "aver.eval/v0",
    label: o.label,
    scenario: name,
    set: s.set,
    title: s.title,
    measures: s.measures,
    dry_run: o.dryRun,
    config: { agents: o.agents, timeout_ms: o.timeoutMs, max_budget_usd: o.maxBudgetUsd, model: o.model, ...versionOf(o) },
    hidden_totals: v.totals,
    difficulty: v.difficulty,
    flaky: v.flaky,
    arms: {},
    complete: false,
  };
  const judge: Judge = { totals: v.totals, baseline: v.baseline };
  for (const arm of o.arms) {
    if (result.arms[arm]?.complete && !o.fresh) {
      say(`${name}: arm ${arm} done already`);
      continue;
    }
    say(`${name}: arm ${arm}, ${o.agents} agent(s) per task, ${s.tasks.length} task(s)`);
    const dir = fresh(join(o.work, name, arm));
    result.arms[arm] = arm === "A" ? await armA(o, s, judge, dir) : await armB(o, s, judge, dir);
    writeFileSync(file, JSON.stringify(result, null, 2) + "\n");
    say(`${name}: arm ${arm} ${summaryLine(result.arms[arm]!)}`);
  }
  result.complete = o.arms.every((a) => result.arms[a]?.complete);
  writeFileSync(file, JSON.stringify(result, null, 2) + "\n");
  return result;
}

/**
 * The reference solution of every task must pass all of its hidden tests:
 * otherwise the scenario measures its own bugs, and it is aborted. The
 * summary says how far each scripted naive behavior gets.
 */
export function difficultyCheck(v: Pick<Validation, "difficulty" | "problems">): { ok: boolean; why: string; summary: string } {
  const failing: string[] = [];
  const parts: string[] = [];
  for (const [task, behaviors] of Object.entries(v.difficulty)) {
    const ref = behaviors.find((b) => b.behavior === "solution");
    if (!ref || ref.hidden_rate !== 1) failing.push(`${task}: reference solution passes ${pct(ref?.hidden_rate ?? 0)} of its hidden tests`);
    const others = behaviors.filter((b) => b.behavior !== "solution").map((b) => `${b.behavior} ${pct(b.hidden_rate)}`);
    parts.push(`${task}: reference ${pct(ref?.hidden_rate ?? 0)}${others.length ? `, ${others.join(", ")}` : ""}`);
  }
  const combined = v.problems.filter((p) => p.startsWith("combined/solution:"));
  failing.push(...combined);
  return { ok: failing.length === 0, why: failing.join("; "), summary: `hidden pass rate per behavior: ${parts.join("; ")}` };
}

function log(line: string): void {
  process.stderr.write(`[eval ${new Date().toISOString().slice(11, 19)}] ${line}\n`);
}

function summaryLine(a: ArmResult): string {
  const picked = Object.entries(a.picks).map(([t, p]) => {
    const at = a.attempts.find((x) => x.task === t && x.index === p.index);
    return `${t}: pick #${p.index ?? "-"} hidden ${at ? pct(at.hidden_rate) : "-"}`;
  });
  return `${fmtMs(a.wall_ms)}; ${picked.join("; ")}`;
}

export function loadResults(dir: string): ScenarioResult[] {
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((f) => f.endsWith(".json"))
    .sort()
    .map((f) => JSON.parse(readFileSync(join(dir, f), "utf8")) as ScenarioResult);
}

const pct = (x: number): string => `${Math.round(x * 100)}%`;
const kb = (bytes: number): string => `${(bytes / 1024).toFixed(1)} KB`;
function fmtMs(ms: number): string {
  const s = Math.round(ms / 1000);
  return s >= 60 ? `${Math.floor(s / 60)}m${String(s % 60).padStart(2, "0")}s` : `${s}s`;
}
const mean = (xs: number[]): number => (xs.length === 0 ? 0 : xs.reduce((a, b) => a + b, 0) / xs.length);

/** Whether an attempt's agent called its work finished: A ended normally with its tests green, B ended done (receipt PASS). */
export function claimedDone(arm: Arm, a: Pick<Attempt, "timed_out" | "npm_test" | "state" | "log">): boolean {
  return arm === "A" ? !a.timed_out && a.log.ended === "success" && a.npm_test.exit === 0 : a.state === "done";
}

/** One cell of the merge across all picks: how many merged, conflicting pairs, and tasks whose hidden tests all pass on it. */
export function mergeAllCell(all: MergeAll | undefined, tasks: number): string {
  if (!all) return "-";
  const bad = all.pairs.filter((p) => p.conflicts.length > 0);
  const skipped = all.skipped.map((x) => `${x.task} (${x.conflicts.join(", ")})`).join("; ");
  const green = Object.entries(all.hidden).filter(([set, rate]) => set !== "combined" && rate === 1).length;
  const combined = all.hidden.combined === undefined ? "" : `, combined hidden ${pct(all.hidden.combined)}`;
  const pairs = bad.length ? `: ${bad.map((p) => `${p.a}+${p.b}`).join(", ")}` : "";
  return `merged ${all.merged.length}/${tasks}${skipped ? `, skipped ${skipped}` : ""}; conflicting pairs ${bad.length}/${all.pairs.length}${pairs}; tasks green on it ${green}/${tasks}${combined}`;
}

/** One row per scenario and arm, then the coordination rows and the feedback ratings. */
export function renderMarkdown(results: ScenarioResult[]): string {
  const lines = [
    "# Aver evaluation results",
    "",
    "Generated by `bench/eval/run.ts` from `bench/eval/results/*.json`. Arm A: plain `git worktree` + `claude -p`; arm B: `aver run`. *Pick* is the attempt each arm's own rule chooses (A: `npm test` green, else fewest failures, then smallest diff; B: Aver's rank 1); hidden tests are the scenario's acceptance tests, never shown to agents. *Gaming*: attempts that removed, skipped or weakened an existing test or changed the test script (*pick gamed*: picks that did). *Pick NEW*: tests of the scenario's suites that pass on every base run and fail on every rerun of the pick. *False done*: the pick fails hidden tests although its arm called it good (A: `npm test` green; B: verdict PASS).",
    "",
    "| Label | Set | Scenario | Arm | Agents | Pick hidden | Pick correct | Any correct | Mean hidden | Gaming | Pick gamed | Pick NEW | False done | Wall (arm) | Mean agent time | Cost | Test runs / agent | Verifies / agent | Repeat verifies | Edits / agent | Denials / agent | Aver output / agent | Notices / agent | Rating |",
    "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|",
  ];
  const coord: string[] = [];
  const doneRows: string[] = [];
  for (const r of results) {
    for (const arm of ["A", "B"] as const) {
      const a = r.arms[arm];
      if (!a) continue;
      const picked = Object.entries(a.picks).map(([t, p]) => a.attempts.find((x) => x.task === t && x.index === p.index));
      const pickRates = picked.map((p) => p?.hidden_rate ?? 0);
      const falseDone = picked.filter((p) => p && !p.correct && (arm === "A" ? p.npm_test.exit === 0 : p.state === "done")).length;
      const tasks = new Set(a.attempts.map((x) => x.task));
      const anyCorrect = [...tasks].filter((t) => a.attempts.some((x) => x.task === t && x.correct)).length;
      const costs = a.attempts.map((x) => x.log.cost_usd).filter((c): c is number => c !== null);
      const ratings = a.attempts.flatMap((x) => (x.feedback && "rating" in x.feedback && x.feedback.rating !== null ? [x.feedback.rating] : []));
      const pickNew = picked.reduce((n, p) => n + (p?.new_failures?.length ?? 0), 0);
      lines.push(
        `| ${r.label}${r.dry_run ? " (dry run)" : ""} | ${r.set ?? "easy"} | ${r.scenario} | ${arm} | ${a.attempts.length} | ${pct(mean(pickRates))} | ${picked.filter((p) => p?.correct).length}/${picked.length} | ${anyCorrect}/${tasks.size} | ${pct(mean(a.attempts.map((x) => x.hidden_rate)))} | ${a.attempts.filter((x) => x.gamed).length} | ${picked.filter((p) => p?.gamed).length}/${picked.length} | ${pickNew} | ${falseDone} | ${fmtMs(a.wall_ms)} | ${fmtMs(mean(a.attempts.map((x) => x.wall_ms)))} | ${costs.length ? `$${costs.reduce((x, y) => x + y, 0).toFixed(2)}` : "-"} | ${mean(a.attempts.map((x) => x.log.test_runs)).toFixed(1)} | ${arm === "B" ? mean(a.attempts.map((x) => x.log.verifies)).toFixed(1) : "-"} | ${arm === "B" ? a.attempts.reduce((n, x) => n + (x.log.repeat_verifies ?? 0), 0) : "-"} | ${mean(a.attempts.map((x) => x.log.edits)).toFixed(1)} | ${mean(a.attempts.map((x) => x.log.permission_denials)).toFixed(1)} | ${kb(mean(a.attempts.map((x) => x.log.aver_output_bytes ?? 0)))} | ${kb(mean(a.attempts.map((x) => x.log.notice_bytes ?? 0)))} | ${ratings.length ? `${mean(ratings).toFixed(1)}/10` : "-"} |`,
      );
      if (a.combined) {
        const c = a.combined;
        const merge = c.merge === "clean" ? "clean" : `conflicts in ${c.merge.conflicts.join(", ")}`;
        const avc = c.aver_combine ? `; \`aver combine\` exit ${c.aver_combine.exit} (${c.aver_combine.summary})` : "";
        coord.push(`| ${r.label} | ${r.scenario} | ${arm} | ${c.overlap.join(", ") || "-"} | ${merge}${avc} | ${c.hidden ? pct(c.hidden_rate) : "-"} | ${mergeAllCell(c.all, tasks.size)} | ${mean(a.attempts.map((x) => x.log.coordination)).toFixed(1)} |`);
      }
      const claimed = a.attempts.filter((x) => claimedDone(arm, x));
      const falsely = claimed.filter((x) => !x.correct);
      doneRows.push(`| ${r.label} | ${r.set ?? "easy"} | ${r.scenario} | ${arm} | ${a.attempts.length} | ${claimed.length} | ${falsely.length} | ${claimed.length ? pct(falsely.length / claimed.length) : "-"} |`);
    }
  }
  if (coord.length) {
    lines.push(
      "",
      "## Parallel tasks on one repository",
      "",
      "*Merge of the picks*: one after another, stopping at the first conflict. *All picks*: every pair merged on its own, then the picks in task order with each one that conflicts skipped, and the tasks whose hidden tests all pass on that merge.",
      "",
      "| Label | Scenario | Arm | Files two or more picks changed | Merge of the picks | Hidden on the merge | All picks | Coordination commands / agent |",
      "|---|---|---|---|---|---|---|---|",
      ...coord,
    );
  }
  if (doneRows.length) {
    lines.push(
      "",
      "## False done",
      "",
      "An attempt *claims done* when its agent ends normally with its tests green (A) or ends `done`, its latest receipt PASS (B). *False done*: it claims done and fails hidden tests. The rate is false done over claimed done.",
      "",
      "| Label | Set | Scenario | Arm | Attempts | Claimed done | False done | False done rate |",
      "|---|---|---|---|---|---|---|---|",
      ...doneRows,
    );
  }
  const asked = askedFor(results.flatMap((r) => (["A", "B"] as const).flatMap((arm) => (r.arms[arm]?.attempts ?? []).map((x) => ({ arm, feedback: x.feedback })))));
  if (asked.length) lines.push("", ...asked);
  return lines.join("\n") + "\n";
}

function parseArgs(argv: string[]): Options {
  const get = (name: string): string | undefined => {
    const i = argv.indexOf(name);
    return i >= 0 ? argv[i + 1] : undefined;
  };
  const dryRun = argv.includes("--dry-run");
  const label = get("--label") ?? today();
  const work = resolve(get("--work") ?? join(process.env.TMPDIR ?? tmpdir(), "aver-eval", label));
  const tasksDir = resolve(get("--tasks-dir") ?? TASKS_DIR);
  const set = (get("--set") ?? "easy") as SetName;
  if (!SETS.includes(set)) throw new Error(`no set ${set}: use ${SETS.join(", ")}`);
  return {
    set,
    scenarios: get("--scenarios")?.split(",").filter(Boolean) ?? scenarioNames(set, tasksDir),
    arms: (get("--arms")?.split(",") ?? ["A", "B"]).map((a) => {
      if (a !== "A" && a !== "B") throw new Error(`no arm ${a}`);
      return a;
    }),
    agents: Number(get("--agents") ?? "2"),
    label,
    fresh: argv.includes("--fresh"),
    timeoutMs: parseDuration(get("--timeout") ?? "20m"),
    maxBudgetUsd: Number(get("--max-budget-usd") ?? "5"),
    model: get("--model") ?? null,
    feedback: !argv.includes("--no-feedback"),
    work,
    results: resolve(get("--results") ?? (dryRun ? join(work, "results") : join(HERE, "results"))),
    aver: resolve(get("--aver") ?? join(ROOT, "target", "release", "aver")),
    claude: get("--claude") ?? "claude",
    dryRun,
    tasksDir,
  };
}

function preflight(o: Options): void {
  const [cmd, pre] = avercmd(o);
  if (run(cmd, [...pre, "--help"]).status !== 0 && !o.aver.endsWith(".ts")) throw new Error(`no aver at ${o.aver}; run make eval or cargo build --release -p aver`);
  if (!o.dryRun && run(o.claude, ["--version"]).status !== 0) throw new Error(`no ${o.claude} on PATH`);
}

async function main(): Promise<void> {
  const o = parseArgs(process.argv.slice(2));
  if (process.argv.includes("--report")) {
    writeFileSync(join(o.results, "..", "results.md"), renderMarkdown(loadResults(o.results)));
    log(`wrote ${join(o.results, "..", "results.md")} from ${o.results}`);
    return;
  }
  preflight(o);
  log(`label ${o.label}, set ${o.set}, scenarios ${o.scenarios.join(", ")}, arms ${o.arms.join("")}, ${o.agents} agent(s), work ${o.work}, results ${o.results}${o.dryRun ? ", DRY RUN" : ""}`);
  await evaluate(o);
  log(`wrote ${o.results} and ${join(o.results, "..", "results.md")}`);
}

if (import.meta.main) {
  main().catch((e: unknown) => {
    process.stderr.write(`eval: ${e instanceof Error ? e.message : String(e)}\n`);
    process.exit(2);
  });
}
