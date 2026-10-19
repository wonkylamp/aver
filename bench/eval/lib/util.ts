// Process, file and Git helpers shared by the evaluation harness.
import { spawn, spawnSync } from "node:child_process";
import type { SpawnSyncOptions } from "node:child_process";
import { cpSync, existsSync, lstatSync, mkdirSync, readdirSync, rmSync } from "node:fs";
import { dirname, join, relative } from "node:path";

export type Ran = { status: number; stdout: string; stderr: string; ms: number; timedOut: boolean };

/** Runs a command to completion; never throws on a non-zero exit. */
export function run(cmd: string, args: string[], opts: SpawnSyncOptions & { timeoutMs?: number } = {}): Ran {
  const start = Date.now();
  const r = spawnSync(cmd, args, {
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
    timeout: opts.timeoutMs,
    killSignal: "SIGKILL",
    ...opts,
  });
  const timedOut = r.error !== undefined && (r.error as NodeJS.ErrnoException).code === "ETIMEDOUT";
  if (r.error && !timedOut) throw new Error(`${cmd} ${args.join(" ")}: ${r.error.message}`);
  return {
    status: r.status ?? (timedOut ? 124 : 1),
    stdout: String(r.stdout ?? ""),
    stderr: String(r.stderr ?? ""),
    ms: Date.now() - start,
    timedOut,
  };
}

/** `run` without blocking, so test runs of several attempts overlap. */
export function runAsync(cmd: string, args: string[], opts: { cwd: string; env?: NodeJS.ProcessEnv; timeoutMs: number }): Promise<Ran> {
  const start = Date.now();
  return new Promise((resolve, reject) => {
    const child = spawn(cmd, args, { cwd: opts.cwd, env: opts.env ?? process.env, stdio: ["ignore", "pipe", "pipe"], detached: true });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8").on("data", (d: string) => (stdout += d));
    child.stderr.setEncoding("utf8").on("data", (d: string) => (stderr += d));
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      try {
        if (child.pid !== undefined) process.kill(-child.pid, "SIGKILL");
      } catch {
        // already gone
      }
    }, opts.timeoutMs);
    child.on("error", (e) => {
      clearTimeout(timer);
      reject(e);
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      resolve({ status: code ?? (timedOut ? 124 : 1), stdout, stderr, ms: Date.now() - start, timedOut });
    });
  });
}

/** Runs a command and throws with its output when it fails. */
export function must(cmd: string, args: string[], opts: SpawnSyncOptions & { timeoutMs?: number } = {}): string {
  const r = run(cmd, args, opts);
  if (r.status !== 0) {
    throw new Error(`${cmd} ${args.join(" ")} exited ${r.status}\n${r.stdout.slice(-2000)}\n${r.stderr.slice(-2000)}`);
  }
  return r.stdout;
}

/** A fixed identity and clock, so the same task always builds the same commits. */
export function gitEnv(seconds = 1767225600): NodeJS.ProcessEnv {
  const date = `${seconds} +0000`;
  return {
    ...process.env,
    GIT_AUTHOR_NAME: "Eval Fixture",
    GIT_AUTHOR_EMAIL: "fixture@example.invalid",
    GIT_COMMITTER_NAME: "Eval Fixture",
    GIT_COMMITTER_EMAIL: "fixture@example.invalid",
    GIT_AUTHOR_DATE: date,
    GIT_COMMITTER_DATE: date,
    GIT_CONFIG_NOSYSTEM: "1",
    GIT_CONFIG_GLOBAL: "/dev/null",
  };
}

export function git(dir: string, args: string[], env: NodeJS.ProcessEnv = gitEnv()): string {
  return must("git", ["-C", dir, ...args], { env });
}

/** Copies `src` over `dest` (whole files), skipping top-level names in `skip`. */
export function overlay(src: string, dest: string, skip: ReadonlySet<string> = new Set()): void {
  mkdirSync(dest, { recursive: true });
  for (const name of readdirSync(src)) {
    if (skip.has(name)) continue;
    cpSync(join(src, name), join(dest, name), { recursive: true, force: true, verbatimSymlinks: true });
  }
}

/** Every regular file under `dir`, relative and sorted. */
export function listFiles(dir: string): string[] {
  const out: string[] = [];
  const walk = (d: string): void => {
    for (const name of readdirSync(d)) {
      const p = join(d, name);
      const st = lstatSync(p);
      if (st.isDirectory()) walk(p);
      else out.push(relative(dir, p));
    }
  };
  if (existsSync(dir)) walk(dir);
  return out.sort();
}

export function fresh(dir: string): string {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  return dir;
}

export function ensureDir(path: string): string {
  mkdirSync(dirname(path), { recursive: true });
  return path;
}

export type Spawned = { status: number; ms: number; timedOut: boolean };

/**
 * Starts a command in its own process group with stdout and stderr in
 * files, and kills the whole group after `timeoutMs`.
 */
export function spawnLogged(
  cmd: string,
  args: string[],
  opts: { cwd: string; env?: NodeJS.ProcessEnv; stdout: number; stderr: number; timeoutMs: number },
): Promise<Spawned> {
  const start = Date.now();
  return new Promise((resolve, reject) => {
    const child = spawn(cmd, args, {
      cwd: opts.cwd,
      env: opts.env ?? process.env,
      stdio: ["ignore", opts.stdout, opts.stderr],
      detached: true,
    });
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      try {
        if (child.pid !== undefined) process.kill(-child.pid, "SIGKILL");
      } catch {
        // already gone
      }
    }, opts.timeoutMs);
    child.on("error", (e) => {
      clearTimeout(timer);
      reject(e);
    });
    child.on("exit", (code, signal) => {
      clearTimeout(timer);
      resolve({ status: code ?? (signal ? 128 : 1), ms: Date.now() - start, timedOut });
    });
  });
}

/** `30m`, `90s`, `1h` or plain seconds, in milliseconds. */
export function parseDuration(text: string): number {
  const m = /^(\d+)(ms|s|m|h)?$/.exec(text.trim());
  if (!m) throw new Error(`not a duration: ${text}`);
  const n = Number(m[1]);
  const unit = m[2] ?? "s";
  return n * { ms: 1, s: 1000, m: 60_000, h: 3_600_000 }[unit]!;
}

export function today(): string {
  return new Date().toISOString().slice(0, 10);
}
