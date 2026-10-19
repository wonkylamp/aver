// A stand-in for the `aver` binary with just the surface run.ts uses, so the
// harness's self-test runs without the real binary: `init`, `run --agent-cmd
// … --json`, `status <run> --json`, `feedback <run> --resume-cmd …`,
// `accept <n> --run <run> --branch <b>` and `combine <run> <run>… --branch
// <b>`. Directories are plain copies of the base, attempts
// are ranked by index, and nothing is verified.
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, openSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";

const home = process.env.AVER_HOME ?? "";
if (!home) {
  process.stderr.write("AVER_HOME is not set\n");
  process.exit(2);
}

type Agent = {
  index: number;
  template: string;
  attempt: string;
  dir: string;
  state: string;
  log: string;
  usage: Record<string, number>;
  started_at: string;
  ended_at: string | null;
  feedback?: { rating: number | null; file: string };
};
type Run = { id: string; repo: string; base_commit: string; title: string; agents: Agent[]; decided?: { index: number; branch: string } };

function opt(args: string[], name: string): string | undefined {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
}

function quote(s: string): string {
  return `'${s.replace(/'/g, "'\\''")}'`;
}

function git(dir: string, args: string[], env: NodeJS.ProcessEnv = process.env): { status: number; out: string } {
  const r = spawnSync("git", ["-C", dir, ...args], { encoding: "utf8", env });
  return { status: r.status ?? 1, out: (r.stdout ?? "").trim() };
}

const runFile = (id: string): string => join(home, "runs", id, "run.json");
const load = (id: string): Run => JSON.parse(readFileSync(runFile(id), "utf8")) as Run;
const save = (run: Run): void => writeFileSync(runFile(run.id), JSON.stringify(run, null, 2));

async function runCmd(args: string[]): Promise<number> {
  const repo = opt(args, "--repo") ?? ".";
  const n = Number(opt(args, "--agents") ?? "3");
  const template = opt(args, "--agent-cmd");
  if (!template) throw new Error("the stub runs only --agent-cmd agents");
  const id = Math.random().toString(36).slice(2, 8).toUpperCase();
  mkdirSync(join(home, "runs", id), { recursive: true });
  const base = git(repo, ["rev-parse", "HEAD"]).out;
  const run: Run = { id, repo, base_commit: base, title: args[0] ?? "", agents: [] };
  const done = [];
  for (let i = 1; i <= n; i++) {
    const dir = join(home, "work", basename(repo), `${id}-${i}`);
    rmSync(dir, { recursive: true, force: true });
    spawnSync("git", ["clone", "-q", repo, dir]);
    rmSync(join(dir, ".git"), { recursive: true, force: true });
    mkdirSync(join(dir, ".aver"), { recursive: true });
    const log = join(home, "runs", id, `agent-${i}.log`);
    const agent: Agent = {
      index: i,
      template: "custom",
      attempt: `att_${id}_${i}`,
      dir,
      state: "working",
      log,
      usage: {},
      started_at: new Date().toISOString(),
      ended_at: null,
    };
    run.agents.push(agent);
    const cmd = template.replaceAll("{dir}", quote(dir));
    const out = openSync(log, "w");
    const child = spawn("/bin/sh", ["-c", cmd], {
      cwd: dir,
      stdio: ["ignore", out, "inherit"],
      env: { ...process.env, AVER_RUN: "1", AVER_AGENT_INDEX: String(i), AVER_EVAL_STUB: "1" },
    });
    done.push(
      new Promise<void>((resolve) =>
        child.on("exit", (code) => {
          agent.state = code === 0 ? "done" : "failed";
          agent.ended_at = new Date().toISOString();
          resolve();
        }),
      ),
    );
  }
  await Promise.all(done);
  save(run);
  const ranking = run.agents.map((a) => ({ attempt: a.attempt, place: a.index, class: "pass", why: "stub: by index" }));
  if (args.includes("--json")) process.stdout.write(JSON.stringify({ run, ranking }, null, 2) + "\n");
  return 0;
}

function feedback(args: string[]): number {
  const run = load(args[0] ?? "");
  const template = opt(args, "--resume-cmd") ?? "";
  for (const a of run.agents) {
    const r = spawnSync("/bin/sh", ["-c", template.replaceAll("{feedback}", quote("questions"))], { cwd: a.dir, encoding: "utf8" });
    const file = join(home, "runs", run.id, `feedback-${a.index}.md`);
    writeFileSync(file, `# Feedback from agent ${a.index}\n\n## Answer\n\n${r.stdout}`);
    const m = /RATING:\s*(\d+)\/10/.exec(r.stdout);
    a.feedback = { rating: m ? Number(m[1]) : null, file };
  }
  save(run);
  return 0;
}

/** Commits an agent directory's files as a child of the run's base. */
function commitDir(run: Run, dir: string, message: string): string {
  const scratch = mkdtempSync(join(tmpdir(), "fake-aver-"));
  const env = { ...process.env, GIT_INDEX_FILE: join(scratch, "index"), GIT_WORK_TREE: dir };
  git(run.repo, ["read-tree", "--empty"], env);
  git(run.repo, ["--work-tree", dir, "add", "-A", "--", ".", ":!.aver"], env);
  const tree = git(run.repo, ["write-tree"], env).out;
  rmSync(scratch, { recursive: true, force: true });
  return git(run.repo, ["commit-tree", tree, "-p", run.base_commit, "-m", message]).out;
}

function accept(args: string[]): number {
  const run = load(opt(args, "--run") ?? "");
  const index = Number(args[0]);
  const agent = run.agents.find((a) => a.index === index);
  if (!agent) return 2;
  const branch = opt(args, "--branch") ?? `aver/${run.id}`;
  git(run.repo, ["branch", "-f", branch, commitDir(run, agent.dir, run.title)]);
  run.decided = { index, branch };
  save(run);
  return 0;
}

function combine(args: string[]): number {
  const ids: string[] = [];
  for (let i = 0; i < args.length; i++) {
    if (args[i]!.startsWith("--")) i += 1;
    else ids.push(args[i]!);
  }
  const runs = ids.map(load);
  const [first, ...rest] = runs;
  if (!first || rest.length === 0 || runs.some((r) => !r.decided)) return 2;
  let head = git(first.repo, ["rev-parse", first.decided!.branch]).out;
  for (const r of rest) {
    const merged = git(first.repo, ["merge-tree", "--write-tree", head, r.decided!.branch]);
    if (merged.status !== 0) {
      process.stdout.write("Conflicts: nothing was merged or exported.\n");
      return 1;
    }
    const tree = merged.out.split("\n")[0] ?? "";
    head = git(first.repo, ["commit-tree", tree, "-p", head, "-p", r.decided!.branch, "-m", "combine"]).out;
  }
  git(first.repo, ["branch", "-f", opt(args, "--branch") ?? "aver/combine", head]);
  process.stdout.write(`Combined ${runs.length} runs as branch ${opt(args, "--branch")}.\n`);
  return 0;
}

async function main(): Promise<number> {
  const [cmd, ...args] = process.argv.slice(2);
  switch (cmd) {
    case "init":
      mkdirSync(home, { recursive: true });
      return 0;
    case "run":
      return runCmd(args);
    case "status":
      process.stdout.write(JSON.stringify({ run: load(args[0] ?? ""), ranking: [] }) + "\n");
      return 0;
    case "feedback":
      return feedback(args);
    case "accept":
      return accept(args);
    case "combine":
      return combine(args);
    default:
      process.stderr.write(`fake aver: unsupported command ${cmd}\n`);
      return 2;
  }
}

process.exit(await main());
