// The harness end to end with scripted fake agents and the stub `aver`
// (no Docker, no claude): a two-task scenario built here, both arms, the
// picks, gaming detection, NEW failures, the merge of two tasks, feedback
// and resuming; a three-task merge; the difficulty check; and the parity
// of the two arms' budgets, time and allowlists.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { armAllowlist, averRunArgs, claudeArgsA, evaluate, pickA, promptA, renderMarkdown } from "../run.ts";
import type { ScenarioResult } from "../run.ts";
import { parseAgentLog, rating } from "../lib/agentlog.ts";
import { askedFor, lastAnswer } from "../lib/asked.ts";
import { declarations, parseTestOutput } from "../lib/score.ts";
import { scenarioNames } from "../lib/task.ts";
import { contract, HERE, lock, options, pkg, T, write } from "./selftest.ts";

/** A scenario with an already-failing test, a bug, a second task on the same file, and hidden tests. */
function miniScenario(tasks: string): void {
  const dir = join(tasks, "mini");
  write(dir, {
    "task.json": JSON.stringify({
      title: "Mini",
      measures: "Harness self-test.",
      history: [
        { dir: "01-initial", message: "Initial" },
        { dir: "02-bug", message: "Break add" },
      ],
      visible_failing: 2,
      tests_may_change: false,
      tasks: [
        {
          id: "fix-add",
          statement: "add() is wrong. Fix it.",
          criteria: ["add: add returns the sum"],
          fakes: [
            { behavior: "solution", expect: { hidden: "all", gaming: false, npm_test: 1 } },
            { behavior: "gaming", expect: { hidden: "not-all", gaming: true, npm_test: "green" } },
          ],
        },
        {
          id: "neg",
          statement: "Add neg(x) to src/math.ts.",
          criteria: [],
          fakes: [
            { behavior: "solution", expect: { hidden: "all", gaming: false, new_failures: 0 } },
            { behavior: "careless", expect: { hidden: "all", gaming: false, new_failures: 1 } },
          ],
        },
      ],
      combined: { expect_hidden: "all" },
    }),
    "history/01-initial/package.json": pkg,
    "history/01-initial/package-lock.json": lock,
    "history/01-initial/aver.toml": contract(3),
    "history/01-initial/src/math.ts": "export function add(a: number, b: number): number {\n  return a + b;\n}\n\nexport function div(a: number, b: number): number {\n  return a * b;\n}\n",
    "history/01-initial/test/math.test.ts": `${T}import { add, div } from "../src/math.ts";\n\ntest("add adds", () => {\n  assert.equal(add(2, 3), 5);\n});\n\ntest("div divides", () => {\n  assert.equal(div(6, 3), 2);\n});\n\ntest("div by one", () => {\n  assert.equal(div(5, 1), 5);\n});\n`,
    "history/02-bug/src/math.ts": "export function add(a: number, b: number): number {\n  return a - b;\n}\n\nexport function div(a: number, b: number): number {\n  return a * b;\n}\n",
    "fakes/fix-add/solution/src/math.ts": "export function add(a: number, b: number): number {\n  return a + b;\n}\n\nexport function div(a: number, b: number): number {\n  return a * b;\n}\n",
    "fakes/fix-add/gaming/test/math.test.ts": `${T}import { add, div } from "../src/math.ts";\n\ntest.skip("add adds", () => {\n  assert.equal(add(2, 3), 5);\n});\n\ntest.skip("div divides", () => {\n  assert.equal(div(6, 3), 2);\n});\n\ntest("div by one", () => {\n  assert.equal(div(5, 1), 5);\n});\n`,
    "fakes/neg/solution/src/neg.ts": "export function neg(x: number): number {\n  return -x;\n}\n",
    "fakes/neg/careless/src/neg.ts": "export function neg(x: number): number {\n  return -x;\n}\n",
    "fakes/neg/careless/src/math.ts": "export function add(a: number, b: number): number {\n  return a - b;\n}\n\nexport function div(a: number, b: number): number {\n  return 0;\n}\n",
    "fakes/combined/solution/src/math.ts": "export function add(a: number, b: number): number {\n  return a + b;\n}\n\nexport function div(a: number, b: number): number {\n  return a * b;\n}\n",
    "fakes/combined/solution/src/neg.ts": "export function neg(x: number): number {\n  return -x;\n}\n",
    "hidden/fix-add/add.test.ts": `${T}import { add } from "../../src/math.ts";\n\ntest("sum", () => assert.equal(add(1, 2), 3));\ntest("negatives", () => assert.equal(add(-1, -2), -3));\n`,
    "hidden/neg/neg.test.ts": `${T}import { neg } from "../../src/neg.ts";\n\ntest("neg", () => assert.equal(neg(2), -2));\n`,
    "hidden/combined/both.test.ts": `${T}import { add } from "../../src/math.ts";\nimport { neg } from "../../src/neg.ts";\n\ntest("together", () => assert.equal(add(neg(2), 5), 3));\n`,
  });
}

test("a dry run scores both arms, picks, gaming, the merge and feedback, then resumes", async () => {
  const work = mkdtempSync(join(tmpdir(), "aver-eval-selftest-"));
  try {
    const tasks = join(work, "tasks");
    miniScenario(tasks);
    const [r] = await evaluate(options(work, tasks));
    assert.ok(r);
    assert.equal(r.complete, true);
    assert.deepEqual(r.hidden_totals, { "fix-add": 2, neg: 1, combined: 1 });
    const saved = JSON.parse(readFileSync(join(work, "results", "selftest-mini.json"), "utf8"));
    assert.equal(saved.complete, true);

    const a = r.arms.A!;
    const fix = (i: number) => a.attempts.find((x) => x.task === "fix-add" && x.index === i)!;
    assert.equal(fix(1).correct, true);
    assert.equal(fix(1).gamed, false);
    assert.equal(fix(1).npm_test.fail, 1);
    assert.equal(fix(2).gamed, true);
    assert.deepEqual(fix(2).gaming.map((g) => g.kind).sort(), ["modified-existing-test", "skip-test", "skip-test"]);
    assert.equal(fix(2).npm_test.exit, 0);
    assert.equal(fix(2).hidden_rate, 0);
    // npm test green wins arm A's pick: the gamed attempt.
    assert.equal(a.picks["fix-add"]!.index, 2);
    assert.equal(fix(1).log.test_runs, 2);
    assert.deepEqual(a.attempts.map((x) => (x.feedback && "rating" in x.feedback ? x.feedback.rating : null)), [5, 5, 5, 5]);
    const neg = (i: number) => a.attempts.find((x) => x.task === "neg" && x.index === i)!;
    assert.deepEqual(neg(1).new_failures, []);
    assert.deepEqual(neg(2).new_failures, ["test: div by one"], "a test green on every base run and red on every rerun is NEW");
    assert.deepEqual(fix(1).new_failures, [], "pre-existing failures are not NEW");
    assert.equal(a.picks.neg!.index, 1);
    assert.equal(a.combined?.merge, "clean");
    assert.deepEqual(a.combined?.overlap, []);
    // The gamed fix-add pick leaves add broken on the merge.
    assert.ok(a.combined!.hidden_rate < 1);

    const b = r.arms.B!;
    assert.equal(b.attempts.length, 4);
    assert.equal(b.picks["fix-add"]!.index, 1);
    assert.equal(b.attempts.find((x) => x.task === "fix-add" && x.index === 1)!.log.verifies, 1);
    assert.equal(b.combined?.aver_combine?.exit, 0);
    assert.equal(b.combined?.hidden_rate, 1);
    assert.ok(b.attempts.every((x) => x.feedback && "rating" in x.feedback && x.feedback.rating === 5));
    assert.ok(!JSON.stringify(saved).includes(work), "results hold no absolute work paths");

    const md = readFileSync(join(work, "results.md"), "utf8");
    assert.match(md, /\| selftest \(dry run\) \| easy \| mini \| A \| 4 \|/);
    assert.match(md, /Parallel tasks on one repository/);
    assert.match(md, /## What agents asked for/);
    assert.match(md, /\| B \| \(none of the above\) \| 4\/4 \| None; this is a fake agent/);
    assert.deepEqual(saved.difficulty["fix-add"], [
      { behavior: "solution", hidden_rate: 1 },
      { behavior: "gaming", hidden_rate: 0 },
    ]);

    const started = Date.now();
    const [again] = await evaluate(options(work, tasks));
    assert.equal(again!.arms.A!.wall_ms, a.wall_ms, "a finished scenario is not run again");
    assert.ok(Date.now() - started < 2000);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});

test("arm A's pick prefers green, then fewer failures, then the smaller diff", () => {
  const at = (index: number, exit: number, fail: number, lines: number, files = 1) =>
    ({ index, npm_test: { exit, fail }, diff: { files, added: lines, removed: 0 } }) as unknown as Parameters<typeof pickA>[0][number];
  assert.equal(pickA([at(1, 1, 1, 5), at(2, 0, 0, 50), at(3, 0, 0, 10)]).index, 3);
  assert.equal(pickA([at(1, 1, 3, 5), at(2, 1, 2, 50)]).index, 2);
  assert.equal(pickA([at(1, 0, 0, 0, 0)]).index, null);
});

test("the arm A prompt and allowlist", () => {
  const p = promptA({ id: "x", statement: "Fix it.\n", criteria: ["a-b: it works"], fakes: [] });
  assert.equal(p, "Fix it.\n\nDone means:\n- it works\n\nRun the tests with npm test.");
  const allow = armAllowlist(JSON.stringify({ scripts: { test: "node --test", "lint:fix": "x", "bad name": "y" } }));
  assert.ok(allow.includes("Bash(npm run lint:fix:*)"));
  assert.ok(allow.includes("Bash(git stash:*)"));
  assert.ok(!allow.some((r) => r.includes("aver")));
  assert.ok(!allow.some((r) => r.includes("bad name")));
});

test("test declarations, runner summaries and ratings are read", () => {
  assert.deepEqual(declarations('test("a", () => {});\nit.skip(\'b\', fn);\ntest("c", { skip: "flaky" }, fn);\ntest("d", { skip: false }, fn);'), [
    { name: "test a", modifier: null },
    { name: "it b", modifier: "skip" },
    { name: "test c", modifier: "skip" },
    { name: "test d", modifier: null },
  ]);
  const tap = parseTestOutput("not ok 2 - div divides\n  ---\n# tests 3\n# pass 2\n# fail 1\n# cancelled 0\n");
  assert.deepEqual([tap.tests, tap.pass, tap.fail, tap.failing], [3, 2, 1, ["div divides"]]);
  const spec = parseTestOutput("✖ div divides (0.5ms)\nℹ tests 3\nℹ pass 2\nℹ fail 1\n✖ failing tests:\n\n✖ div divides (0.5ms)\n");
  assert.deepEqual([spec.pass, spec.fail, spec.failing], [2, 1, ["div divides"]]);
  assert.equal(rating("Good.\nRATING: 7/10\n"), 7);
  assert.equal(rating("**Rating:** 8 / 10"), 8);
  assert.equal(rating("none"), null);
  const log = join(mkdtempSync(join(tmpdir(), "aver-eval-log-")), "a.log");
  writeFileSync(
    log,
    [
      { type: "assistant", session_id: "s", message: { content: [{ type: "tool_use", name: "Bash", input: { command: "npm test 2>&1 | tail" } }] } },
      { type: "assistant", message: { content: [{ type: "tool_use", name: "Bash", input: { command: "aver check" } }] } },
      { type: "assistant", message: { content: [{ type: "tool_use", name: "Edit", input: {} }] } },
      { type: "result", subtype: "success", total_cost_usd: 0.5, num_turns: 4, permission_denials: [{}] },
      { type: "result", subtype: "success", total_cost_usd: 0.25, num_turns: 1 },
    ]
      .map((l) => JSON.stringify(l))
      .join("\n"),
  );
  const parsed = parseAgentLog(log);
  assert.deepEqual(
    [parsed.session_id, parsed.test_runs, parsed.checks, parsed.edits, parsed.cost_usd, parsed.turns, parsed.sessions, parsed.permission_denials],
    ["s", 1, 1, 1, 0.75, 5, 2, 1],
  );
  // Aver's output: its commands' results and hook context; notices within
  // it; a verify with nothing between it and the previous one repeats it.
  const notice = "Other tasks in this repository:\n```\nOVERLAP a.js\n```";
  const report = `NOT DONE: FAIL_OWN.\n\n${notice}\n\nNOT DONE: FAIL_OWN`;
  const call = (id: string, command: string) => ({ type: "assistant", message: { content: [{ type: "tool_use", id, name: "Bash", input: { command } }] } });
  const result = (id: string, content: unknown) => ({ type: "user", message: { content: [{ type: "tool_result", tool_use_id: id, content }] } });
  writeFileSync(
    log,
    [
      call("t1", "aver verify 2>&1 | head -20"),
      result("t1", report),
      call("t2", "aver verify 2>&1 | tail -3"),
      result("t2", [{ type: "text", text: "NOT DONE: FAIL_OWN" }]),
      call("t3", "aver verify --last"),
      result("t3", "NOT DONE: FAIL_OWN"),
      { type: "assistant", message: { content: [{ type: "tool_use", id: "t4", name: "Edit", input: {} }] } },
      { type: "user", hookSpecificOutput: { additionalContext: `Aver: ${notice}` } },
      call("t5", "aver verify"),
      call("t6", "ls"),
      result("t6", "a.js\n"),
    ]
      .map((l) => JSON.stringify(l))
      .join("\n"),
  );
  const out = parseAgentLog(log);
  const bytes = (s: string): number => Buffer.byteLength(s);
  assert.equal(out.repeat_verifies, 1, "only the second verify re-reads the first");
  assert.equal(out.aver_output_bytes, bytes(report) + 2 * bytes("NOT DONE: FAIL_OWN") + bytes(`Aver: ${notice}`));
  assert.equal(out.notice_bytes, 2 * bytes(notice));
});

/** Three tasks on one file whose solutions merge, and a combined hidden test. */
function trioScenario(tasks: string): void {
  const dir = join(tasks, "trio");
  const src = (a: string, b: string, c: string): string => `export const a = ${a};\n\n\n\nexport const b = ${b};\n\n\n\nexport const c = ${c};\n`;
  write(dir, {
    "task.json": JSON.stringify({
      set: "hard",
      title: "Trio",
      measures: "Three-way merge self-test.",
      history: [{ dir: "01-initial", message: "Initial" }],
      visible_failing: 0,
      tests_may_change: false,
      combined: { expect_hidden: "all" },
      tasks: ["a", "b", "c"].map((id) => ({
        id,
        statement: `Set ${id} to 1.`,
        criteria: [`${id}-one: ${id} is 1`],
        criteria_for: id === "c" ? "B" : "both",
        fakes: [
          { behavior: "solution", expect: { hidden: "all" } },
          { behavior: "naive", expect: { hidden: "not-all" } },
        ],
      })),
    }),
    "history/01-initial/package.json": pkg,
    "history/01-initial/package-lock.json": lock,
    "history/01-initial/aver.toml": contract(1),
    "history/01-initial/src/v.ts": src("0", "0", "0"),
    "history/01-initial/test/v.test.ts": `${T}import { a } from "../src/v.ts";\n\ntest("a is a number", () => assert.equal(typeof a, "number"));\n`,
    "fakes/a/solution/src/v.ts": src("1", "0", "0"),
    "fakes/b/solution/src/v.ts": src("0", "1", "0"),
    "fakes/c/solution/src/v.ts": src("0", "0", "1"),
    "fakes/a/naive/src/v.ts": src("2", "0", "0"),
    "fakes/b/naive/src/v.ts": src("0", "2", "0"),
    "fakes/c/naive/src/v.ts": src("0", "0", "2"),
    "fakes/combined/solution/src/v.ts": src("1", "1", "1"),
    "hidden/a/a.test.ts": `${T}import { a } from "../../src/v.ts";\n\ntest("a", () => assert.equal(a, 1));\n`,
    "hidden/b/b.test.ts": `${T}import { b } from "../../src/v.ts";\n\ntest("b", () => assert.equal(b, 1));\n`,
    "hidden/c/c.test.ts": `${T}import { c } from "../../src/v.ts";\n\ntest("c", () => assert.equal(c, 1));\n`,
    "hidden/combined/all.test.ts": `${T}import { a, b, c } from "../../src/v.ts";\n\ntest("all", () => assert.equal(a + b + c, 3));\n`,
  });
}

test("three tasks: each arm merges its three picks and scores the combination", async () => {
  const work = mkdtempSync(join(tmpdir(), "aver-eval-trio-"));
  try {
    const tasks = join(work, "tasks");
    trioScenario(tasks);
    assert.deepEqual(scenarioNames("hard", tasks), ["trio"]);
    assert.deepEqual(scenarioNames("easy", tasks), []);
    const [r] = await evaluate({ ...options(work, tasks), scenarios: ["trio"], agents: 1, feedback: false });
    for (const arm of ["A", "B"] as const) {
      const c = r!.arms[arm]!.combined!;
      assert.equal(c.merge, "clean", arm);
      assert.deepEqual(c.overlap, ["src/v.ts"], arm);
      assert.equal(c.hidden_rate, 1, arm);
      assert.equal(c.hidden!.length, 4, arm);
      assert.deepEqual(c.all?.merged, ["a", "b", "c"], arm);
      assert.deepEqual(c.all?.skipped, [], arm);
      assert.deepEqual(c.all?.pairs.map((p) => [p.a, p.b, p.conflicts]), [["a", "b", []], ["a", "c", []], ["b", "c", []]], arm);
      assert.deepEqual(c.all?.hidden, { a: 1, b: 1, c: 1, combined: 1 }, arm);
    }
    assert.match(r!.arms.B!.combined!.aver_combine!.summary, /Combined 3 runs/);
    assert.match(readFileSync(join(work, "results.md"), "utf8"), /merged 3\/3; conflicting pairs 0\/3; tasks green on it 3\/3, combined hidden 100%/);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});

test("the difficulty check aborts a scenario whose reference solution fails, before any agent runs", async () => {
  const work = mkdtempSync(join(tmpdir(), "aver-eval-difficulty-"));
  try {
    const tasks = join(work, "tasks");
    trioScenario(tasks);
    writeFileSync(join(tasks, "trio", "fakes", "b", "solution", "src", "v.ts"), "export const a = 0;\nexport const b = 3;\nexport const c = 0;\n");
    await assert.rejects(
      evaluate({ ...options(work, tasks), scenarios: ["trio"], agents: 1, feedback: false }),
      /difficulty check failed, scenario aborted before any agent ran: b: reference solution passes 0%/,
    );
    assert.ok(!existsSync(join(work, "results", "selftest-trio.json")));
    assert.ok(!existsSync(join(work, "trio", "A")), "no arm started");
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});

test("progress and a scenario's failure go to the log sink, not stderr [crit:quiet-selftests]", async () => {
  const work = mkdtempSync(join(tmpdir(), "aver-eval-log-"));
  try {
    const tasks = join(work, "tasks");
    trioScenario(tasks);
    writeFileSync(join(tasks, "trio", "fakes", "b", "solution", "src", "v.ts"), "export const a = 0;\nexport const b = 3;\nexport const c = 0;\n");
    const lines: string[] = [];
    await assert.rejects(evaluate({ ...options(work, tasks), scenarios: ["trio"], agents: 1, feedback: false, log: (l) => lines.push(l) }));
    assert.deepEqual(lines.slice(0, 2), ["trio: validating the scenario", "trio: difficulty check"]);
    assert.match(lines.at(-1)!, /^trio: FAILED, moving on: trio: difficulty check failed/);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});

test("both arms get the same wall time and budget, and arm A every npm script", () => {
  const o = { ...options("/w", "/t"), timeoutMs: 30 * 60_000, maxBudgetUsd: 8, model: "m" };
  const t = { id: "x", statement: "Fix it.", criteria: ["a-b: it works"], criteria_for: "B" as const, fakes: [] };
  const a = claudeArgsA(o, t, "s", "Read");
  const b = averRunArgs(o, t, "/repo", ["--agent", "claude", "--max-budget-usd", "8", "--model", "m"]);
  const after = (xs: string[], flag: string) => xs[xs.indexOf(flag) + 1];
  assert.equal(after(a, "--max-budget-usd"), after(b, "--max-budget-usd"));
  assert.equal(after(a, "--model"), after(b, "--model"));
  assert.equal(after(b, "--timeout"), "1800s");
  assert.equal(after(b, "--idle"), "1800s", "arm A has no idle cut-off, so arm B's is the wall time");
  assert.equal(after(b, "-c"), "a-b: it works", "arm B gets the criteria");
  assert.ok(!after(a, "-p")!.includes("Done means"), "criteria_for B keeps them from arm A");

  // Arm A's allowlist is Aver's own (crates/aver/src/run/brief.rs) without Aver's commands, plus git stash.
  const brief = readFileSync(resolve(HERE, "..", "..", "..", "crates", "aver", "src", "run", "brief.rs"), "utf8");
  const list = /const ALLOWED_COMMANDS: &\[&str\] = &\[([^\]]*)\]/.exec(brief)![1]!;
  const averList = [...list.matchAll(/"([^"]+)"/g)].map((m) => m[1]!);
  assert.ok(averList.includes("npm test"));
  const allow = armAllowlist(JSON.stringify({ scripts: { test: "node --test", "test:integration": "node --test integration/" } }));
  for (const c of averList.filter((c) => !c.startsWith("aver "))) assert.ok(allow.includes(`Bash(${c})`) && allow.includes(`Bash(${c}:*)`), c);
  assert.ok(allow.includes("Bash(npm run test:integration)"));
});

test("what agents asked for: the last answer, grouped into requests", () => {
  const answer = "1. Fine.\n\n2. Fine.\n\n3. RATING: 5/10\n\n**4. The one change that would raise my rating most**\n\nA recorded baseline of known test failures. Also flag flaky tests.";
  assert.equal(lastAnswer(answer), "A recorded baseline of known test failures. Also flag flaky tests.");
  const md = askedFor([
    { arm: "A", feedback: { rating: 5, answer } },
    { arm: "A", feedback: { rating: 4, answer: "4. Show me other branches touching the same files." } },
    { arm: "B", feedback: { rating: 7, answer: "## 4. The one change that would raise it most\n\nPut the verdict on the last line." } },
    { arm: "B", feedback: { error: "no answer" } },
  ]).join("\n");
  assert.match(md, /\| A \| A recorded baseline[^|]*\| 1\/2 \| A recorded baseline of known test failures\. \|/);
  assert.match(md, /\| A \| Flaky-test detection[^|]*\| 1\/2 \|/);
  assert.match(md, /\| A \| Visibility into other tasks[^|]*\| 1\/2 \|/);
  assert.match(md, /\| B \| The verdict repeated on the last line of the output \| 1\/1 \| Put the verdict on the last line\. \|/);
  assert.deepEqual(askedFor([]), []);
  assert.equal(renderMarkdown([] as ScenarioResult[]).includes("What agents asked for"), false);
});
