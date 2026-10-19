// A scripted stand-in for an agent CLI, for `run.ts --dry-run`: it writes a
// scenario's fake behavior (a whole-file overlay) into its directory and
// prints Claude Code stream-json lines, so the harness reads its log like a
// real agent's. Agent n takes the task's fakes in turn.
//
//   node fake-agent.ts work --scenario <name> --task <id> --dir <dir> [--index <n>] [--tasks-dir <dir>]
//   node fake-agent.ts feedback <questions>
//
// Under `aver run` (AVER_RUN=1) the index comes from AVER_AGENT_INDEX and
// it runs `aver verify` once, as an agent would (skipped under the stub).
import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { listFiles } from "./lib/util.ts";
import { applyFake, loadScenario, TASKS_DIR } from "./lib/task.ts";
import { join } from "node:path";

function arg(args: string[], name: string): string | undefined {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
}

function emit(v: unknown): void {
  process.stdout.write(JSON.stringify(v) + "\n");
}

function work(args: string[]): void {
  const scenario = loadScenario(arg(args, "--scenario") ?? "", arg(args, "--tasks-dir") ?? TASKS_DIR);
  const task = scenario.tasks.find((t) => t.id === arg(args, "--task"));
  if (!task) throw new Error(`no task ${arg(args, "--task")}`);
  const dir = arg(args, "--dir") ?? process.cwd();
  const index = Number(arg(args, "--index") ?? process.env.AVER_AGENT_INDEX ?? "1");
  const fake = task.fakes[(index - 1) % task.fakes.length]!;
  const session = randomUUID();
  const tool = (name: string, input: Record<string, string>): void =>
    emit({ type: "assistant", session_id: session, message: { content: [{ type: "tool_use", name, input }] } });
  emit({ type: "system", subtype: "init", session_id: session, cwd: dir });
  tool("Bash", { command: "npm test" });
  applyFake(scenario, task.id, fake, dir);
  const src = join(scenario.dir, "fakes", task.id, fake.behavior);
  for (const f of listFiles(src)) tool("Write", { file_path: join(dir, f) });
  let turns = 3;
  if (process.env.AVER_RUN === "1") {
    tool("Bash", { command: "aver verify" });
    turns += 1;
    if (!process.env.AVER_EVAL_STUB) {
      const r = spawnSync("aver", ["verify"], { cwd: dir, encoding: "utf8" });
      process.stderr.write(r.stdout ?? "");
    }
  } else {
    tool("Bash", { command: "npm test" });
  }
  emit({
    type: "result",
    subtype: "success",
    session_id: session,
    num_turns: turns,
    total_cost_usd: 0,
    permission_denials: [],
    result: `Applied the scripted "${fake.behavior}" behavior.`,
  });
}

function feedback(): void {
  process.stdout.write(
    "1. Scripted agent: nothing to quote.\n2. No opinion.\n3. RATING: 5/10\n4. None; this is a fake agent for the harness self-test.\n",
  );
}

const [cmd, ...rest] = process.argv.slice(2);
if (cmd === "work") work(rest);
else if (cmd === "feedback") feedback();
else {
  process.stderr.write("usage: fake-agent.ts work|feedback …\n");
  process.exit(2);
}
