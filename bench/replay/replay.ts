// Replays agent pull requests through `aver ci`, one at a time, and appends
// one JSON line per pull request to the results file.
//
//   AVER_REPLAY_WORK=/some/dir node bench/replay/replay.ts [options]
//
// Options:
//   --manifest <file>    default $AVER_REPLAY_MANIFEST, else bench/replay/manifest.json
//                        when it exists (it is private and may be absent)
//   --results <file>     default bench/replay/results.jsonl (appended to)
//   --aver <binary>      default target/release/aver
//   --only <id,id,...>   run only these manifest ids
//   --ranked             run only entries with a rank (the pilot shortlist)
//   --limit <n>          stop after n pull requests have run
//   --timeout-min <n>    wall-clock limit per pull request, default 30
//   --base recorded|merge-parent   which base to verify against, default recorded
//   --rerun              run entries that already have a result line
//   --max-load <n>       before each pull request, wait until the 1-minute load
//                        average is below n, so other workloads on the machine
//                        cannot turn into timeouts (default: no wait)
//
// The work directory holds untrusted downloads and outputs:
//   repos/<owner>__<name>.git   bare clones (never checked out)
//   home/                       one persistent AVER_HOME for every run
//   out/<id>/                   receipt.json, summary.md, public-key.txt,
//                               aver.log and runs/<check>.<side>.<n>.log (every
//                               run's output, written by aver ci itself)
//
// Nothing from a cloned repository runs on the host: this script only calls
// git plumbing on the bare clones, and `aver ci` runs their code in sandboxes.
// Paths written to the results file are relative to the work directory.
import { spawn, spawnSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { loadavg } from "node:os";
import { join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";

type Entry = {
  id: string;
  rank: number | null;
  repo: string;
  pr: number;
  agent: string;
  state: string;
  base: string;
  head: string;
  skip?: string;
};
type Check = {
  id: string;
  base: string;
  head: string;
  cause: string | null;
  runs: { base: string[]; head: string[] };
  detail?: string;
};

const TEST_FILE = /(^|\/)(test|tests|__tests__|spec)\/|\.(test|spec)\.[cm]?[jt]sx?$/;

/** A receipt entry for one test, not a whole check, an exit code or a harness change. */
function isTestEntry(id: string): boolean {
  return id.includes("::") && !id.startsWith("judging::") && !id.startsWith("protected::") && !id.endsWith("::(exit)");
}
const CLI_OWNER_LABEL = "label=aver.owner=aver-cli";

function args(): Map<string, string> {
  const out = new Map<string, string>();
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i++) {
    const key = argv[i].replace(/^--/, "");
    const next = argv[i + 1];
    if (next === undefined || next.startsWith("--")) out.set(key, "true");
    else out.set(key, argv[++i]);
  }
  return out;
}

// The manifest lists pull requests from a research dataset and is not
// published, so a checkout may lack the in-repo default.
function manifestPath(flag: string | undefined, env: string | undefined, inRepo: string): string {
  const given = flag ?? (env || undefined);
  if (given !== undefined) {
    const file = resolve(given);
    if (!existsSync(file)) throw new Error(`manifest not found: ${file} (from ${flag !== undefined ? "--manifest" : "AVER_REPLAY_MANIFEST"})`);
    return file;
  }
  if (existsSync(inRepo)) return inRepo;
  throw new Error(`no replay manifest: pass --manifest <file> or set AVER_REPLAY_MANIFEST (${inRepo} does not exist)`);
}

function git(repo: string, ...a: string[]): { ok: boolean; out: string; err: string } {
  const r = spawnSync("git", ["-C", repo, ...a], { encoding: "utf8", maxBuffer: 256 << 20 });
  return { ok: r.status === 0, out: (r.stdout ?? "").trim(), err: (r.stderr ?? "").trim() };
}

function must(repo: string, ...a: string[]): string {
  const r = git(repo, ...a);
  if (!r.ok) throw new Error(`git ${a.join(" ")}: ${r.err}`);
  return r.out;
}

function hasCommit(repo: string, sha: string): boolean {
  return git(repo, "cat-file", "-e", `${sha}^{commit}`).ok;
}

/** Bare clone plus the pull request head, fetched read-only from GitHub. */
function prepare(work: string, e: Entry): string {
  const bare = join(work, "repos", `${e.repo.replace("/", "__")}.git`);
  if (!existsSync(bare)) {
    mkdirSync(join(work, "repos"), { recursive: true });
    const r = spawnSync("git", ["clone", "--bare", "--quiet", `https://github.com/${e.repo}.git`, bare], {
      encoding: "utf8",
      stdio: ["ignore", "inherit", "inherit"],
    });
    if (r.status !== 0) throw new Error(`clone ${e.repo} failed`);
  }
  if (!hasCommit(bare, e.head)) must(bare, "fetch", "--quiet", "origin", `+pull/${e.pr}/head:refs/replay/pr-${e.pr}`);
  if (!hasCommit(bare, e.base)) must(bare, "fetch", "--quiet", "origin", e.base);
  if (!hasCommit(bare, e.head)) throw new Error(`head ${e.head} not reachable after fetch`);
  return bare;
}

/**
 * The first parent of the commit that landed a merged pull request, found from
 * local history: a merge commit whose second parent is the head, else a commit
 * on the default branch's first-parent line whose subject ends in "(#N)"
 * (squash) or starts "Merge pull request #N".
 */
function mergeParent(bare: string, e: Entry): { sha: string | null; how: string } {
  if (e.state !== "merged") return { sha: null, how: "not merged" };
  const log = git(bare, "log", "--first-parent", "--format=%H %P%x09%s", "HEAD", "-n", "20000");
  if (!log.ok) return { sha: null, how: "no default branch" };
  for (const line of log.out.split("\n")) {
    const [shas, subject = ""] = line.split("\t");
    const [, p1, p2] = shas.split(" ");
    if (p2 === e.head) return { sha: p1, how: "merge commit" };
    if (subject.startsWith(`Merge pull request #${e.pr} `) && p1) return { sha: p1, how: "merge commit (subject)" };
    if (new RegExp(`\\(#${e.pr}\\)\\s*$`).test(subject) && p1) return { sha: p1, how: "squash commit (subject)" };
  }
  return { sha: null, how: "not found on default branch" };
}

function changes(bare: string, from: string, to: string): { files: number; tests: string[]; testsDeleted: string[] } {
  const r = git(bare, "diff", "--name-status", "--no-renames", from, to);
  const tests: string[] = [];
  const testsDeleted: string[] = [];
  let files = 0;
  for (const line of r.out.split("\n").filter(Boolean)) {
    const [status, path] = line.split("\t");
    files++;
    if (TEST_FILE.test(path)) {
      tests.push(path);
      if (status === "D") testsDeleted.push(path);
    }
  }
  return { files, tests, testsDeleted };
}

function testScript(bare: string, rev: string): string | null {
  const r = git(bare, "show", `${rev}:package.json`);
  if (!r.ok) return null;
  try {
    return JSON.parse(r.out)?.scripts?.test ?? null;
  } catch {
    return null;
  }
}

/**
 * Removes the check sandboxes a killed `aver ci` left behind. Other Aver CLIs on
 * the same Docker use the same owner label, so only containers running a
 * snapshot stored in this replay's home are removed.
 */
function removeLeftovers(homeDb: string): void {
  const ps = spawnSync("docker", ["ps", "-a", "--filter", CLI_OWNER_LABEL, "--format", '{{.ID}} {{.Label "aver.run"}}'], { encoding: "utf8" });
  if (ps.status !== 0 || !existsSync(homeDb)) return;
  const db = new DatabaseSync(homeDb, { readOnly: true });
  const known = db.prepare("SELECT 1 FROM objects WHERE id = ?");
  const ids = ps.stdout
    .split("\n")
    .map((l) => l.trim().split(" "))
    .filter(([id, label]) => id && label?.startsWith("snap_") && known.get(label.split(":")[0]) !== undefined)
    .map(([id]) => id);
  db.close();
  if (ids.length) spawnSync("docker", ["rm", "-f", ...ids]);
}

/** Runs `aver ci` with a wall-clock limit; on expiry kills its process group and its sandboxes. */
function runCi(averBin: string, argv: string[], logFile: string, timeoutMs: number, homeDb: string): Promise<{ code: number | null; timedOut: boolean; ms: number }> {
  return new Promise((done) => {
    const started = Date.now();
    const log: Buffer[] = [];
    const child = spawn(averBin, argv, { stdio: ["ignore", "pipe", "pipe"], detached: true });
    child.stdout.on("data", (d: Buffer) => log.push(d));
    child.stderr.on("data", (d: Buffer) => log.push(d));
    let timedOut = false;
    const group = (sig: NodeJS.Signals) => {
      try {
        process.kill(-child.pid!, sig);
      } catch {}
    };
    const timer = setTimeout(() => {
      timedOut = true;
      group("SIGTERM");
      setTimeout(() => group("SIGKILL"), 10_000).unref();
    }, timeoutMs);
    child.on("close", (code) => {
      clearTimeout(timer);
      writeFileSync(logFile, Buffer.concat(log));
      if (timedOut) removeLeftovers(homeDb);
      done({ code, timedOut, ms: Date.now() - started });
    });
  });
}

async function main(): Promise<void> {
  const opt = args();
  const root = resolve(import.meta.dirname, "../..");
  const work = process.env.AVER_REPLAY_WORK;
  if (!work) throw new Error("set AVER_REPLAY_WORK to the work directory");
  const manifestFile = manifestPath(opt.get("manifest"), process.env.AVER_REPLAY_MANIFEST, join(root, "bench/replay/manifest.json"));
  const resultsFile = resolve(opt.get("results") ?? join(root, "bench/replay/results.jsonl"));
  const averBin = resolve(opt.get("aver") ?? join(root, "target/release/aver"));
  const timeoutMs = Number(opt.get("timeout-min") ?? 30) * 60_000;
  const baseMode = opt.get("base") ?? "recorded";
  const only = opt.get("only")?.split(",");
  const limit = Number(opt.get("limit") ?? Infinity);
  const home = join(work, "home");
  const maxLoad = opt.has("max-load") ? Number(opt.get("max-load")) : Infinity;

  const entries: Entry[] = JSON.parse(readFileSync(manifestFile, "utf8")).prs;
  const done = new Set<string>();
  if (existsSync(resultsFile) && !opt.has("rerun")) {
    for (const line of readFileSync(resultsFile, "utf8").split("\n").filter(Boolean)) done.add(JSON.parse(line).id);
  }
  const aver_commit = spawnSync("git", ["-C", root, "rev-parse", "HEAD"], { encoding: "utf8" }).stdout.trim();

  let ran = 0;
  for (const e of entries) {
    if (only && !only.includes(e.id)) continue;
    if (!only && opt.has("ranked") && e.rank === null) continue;
    if (done.has(e.id)) continue;
    if (ran >= limit) break;
    const base_record = { id: e.id, repo: e.repo, pr: e.pr, agent: e.agent, state: e.state, aver_commit, date: new Date().toISOString() };
    if (e.skip) {
      appendFileSync(resultsFile, JSON.stringify({ ...base_record, status: "skipped", reason: e.skip }) + "\n");
      console.log(`skip ${e.id}: ${e.skip}`);
      continue;
    }
    ran++;
    if (loadavg()[0] >= maxLoad) {
      console.log(`    waiting for the load average (${loadavg()[0].toFixed(1)}) to drop below ${maxLoad}`);
      while (loadavg()[0] >= maxLoad) await new Promise((r) => setTimeout(r, 30_000));
    }
    const loadAtStart = Number(loadavg()[0].toFixed(1));
    console.log(`==> ${e.id} (load ${loadAtStart})`);
    let bare: string;
    try {
      bare = prepare(work, e);
    } catch (err) {
      appendFileSync(resultsFile, JSON.stringify({ ...base_record, status: "fetch_error", reason: String(err) }) + "\n");
      continue;
    }
    const merge = mergeParent(bare, e);
    const mergeBase = git(bare, "merge-base", e.base, e.head).out || null;
    const baseIsAncestor = git(bare, "merge-base", "--is-ancestor", e.base, e.head).ok;
    const base = baseMode === "merge-parent" && merge.sha ? merge.sha : e.base;
    const pr_changes = changes(bare, mergeBase ?? e.base, e.head);
    const verified_changes = changes(bare, base, e.head);

    const outRel = join("out", e.id);
    const outDir = join(work, outRel);
    mkdirSync(outDir, { recursive: true });
    const argv = ["ci", "--repo", bare, "--base", base, "--head", e.head, "--home", home, "--out-dir", outDir, "--fail-on", "never", "--sandbox", "auto"];
    const run = await runCi(averBin, argv, join(outDir, "aver.log"), timeoutMs, join(home, "aver.db"));

    let receipt: {
      receipt_id: string;
      receipt: { verdict: string; env_changed: boolean; checks: Check[]; ci?: { base_commit: string; requested_base?: string } };
    } | null = null;
    try {
      receipt = JSON.parse(readFileSync(join(outDir, "receipt.json"), "utf8"));
    } catch {}
    let importNotes: { secrets: unknown[]; placeholders: unknown[] } | null = null;
    try {
      importNotes = JSON.parse(readFileSync(join(outDir, "diagnostics.json"), "utf8")).import ?? null;
    } catch {}
    const checks = receipt?.receipt.checks ?? [];
    const count = (cause: string | null) => checks.filter((c) => c.cause === cause).length;
    let summary = "";
    try {
      summary = readFileSync(join(outDir, "summary.md"), "utf8");
    } catch {}

    const result = {
      ...base_record,
      status: run.timedOut ? "timeout" : receipt ? "receipt" : "error",
      exit_code: run.code,
      wall_ms: run.ms,
      load_at_start: loadAtStart,
      base_used: base,
      base_mode: baseMode,
      recorded_base: e.base,
      head: e.head,
      merge_parent: merge.sha,
      merge_parent_found_by: merge.how,
      merge_base: mergeBase,
      base_is_ancestor_of_head: baseIsAncestor,
      pr_files_changed: pr_changes.files,
      pr_test_files: pr_changes.tests,
      pr_test_files_deleted: pr_changes.testsDeleted,
      verified_files_changed: verified_changes.files,
      test_script_base: testScript(bare, base),
      test_script_head: testScript(bare, e.head),
      has_aver_toml: git(bare, "cat-file", "-e", `${e.head}:aver.toml`).ok,
      receipt_id: receipt?.receipt_id ?? null,
      verdict: receipt?.receipt.verdict ?? null,
      env_changed: receipt?.receipt.env_changed ?? null,
      entries: checks.length,
      counts: { NEW: count("NEW"), CHANGED: count("CHANGED"), PREEXISTING: count("PREEXISTING"), FLAKY: count("FLAKY"), ENV: count("ENV"), pass: count(null) },
      base_failed: checks.some((c) => c.base === "fail" || c.base === "flaky"),
      command_level: checks.length > 0 && checks.every((c) => !c.id.includes("::")),
      // Entries for single tests: both sides produced a machine-readable report.
      per_test_entries: checks.filter((c) => isTestEntry(c.id)).length,
      // Tests that ran and passed on both sides: the project's tests really ran.
      tests_passing_both: checks.filter((c) => isTestEntry(c.id) && c.base === "pass" && c.head === "pass").length,
      exit_entry: checks.some((c) => c.id.endsWith("::(exit)")),
      base_compared: receipt?.receipt.ci?.base_commit ?? null,
      base_moved_to_merge_base: receipt?.receipt.ci?.requested_base !== undefined,
      import_secrets: importNotes?.secrets.length ?? 0,
      import_placeholders: importNotes?.placeholders.length ?? 0,
      failing: checks
        .filter((c) => c.cause !== null)
        .slice(0, 50)
        .map((c) => ({ id: c.id, base: c.base, head: c.head, cause: c.cause, detail: (c.detail ?? "").slice(-400) })),
      error: receipt ? null : summary.slice(0, 2000),
      out_dir: outRel,
    };
    appendFileSync(resultsFile, JSON.stringify(result) + "\n");
    console.log(`    ${result.status} verdict=${result.verdict} ${JSON.stringify(result.counts)} ${Math.round(run.ms / 1000)}s`);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(2);
});
