// What an agent did, read from its Claude Code stream-json log (one JSON
// object per line; the fake agents write the same shape).
import { existsSync, readFileSync } from "node:fs";

export type AgentLog = {
  /** Tool calls of any kind. */
  tool_calls: number;
  /** Edit, Write, MultiEdit and NotebookEdit calls. */
  edits: number;
  /** `npm test`, `npm run test`, `node --test` or `python3 -m unittest` commands. */
  test_runs: number;
  /** `aver verify` and `aver check` commands. */
  verifies: number;
  checks: number;
  /** `aver lease`, `aver notices` and `aver brief` commands (coordination). */
  coordination: number;
  /** `git stash`, `git checkout` or `git worktree` commands: hand-made comparisons with the original. */
  baseline_by_hand: number;
  /** Tool calls Claude Code refused for want of permission. */
  permission_denials: number;
  /** Bytes Aver put in the agent's context: results of `aver` commands plus hook context. */
  aver_output_bytes: number;
  /** The part of those bytes that is overlap notices ("Other tasks…" sections). */
  notice_bytes: number;
  /** `aver verify` runs with no edit or other command since the previous one (re-reading a verdict). */
  repeat_verifies: number;
  /** Claude Code processes (the first run plus each resume). */
  sessions: number;
  turns: number;
  cost_usd: number | null;
  session_id: string | null;
  /** How the last process ended (`success`, `error_max_turns`, ...). */
  ended: string | null;
  /** The agent's last message. */
  final: string | null;
};

type Line = {
  type?: string;
  subtype?: string;
  session_id?: string;
  total_cost_usd?: number;
  num_turns?: number;
  result?: string;
  permission_denials?: unknown[];
  message?: { content?: Array<Item> | string };
};

type Item = {
  type?: string;
  id?: string;
  name?: string;
  input?: { command?: string };
  tool_use_id?: string;
  content?: unknown;
  text?: string;
};

/** The text of a tool result's `content` (a string or a list of text blocks). */
function resultText(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content.map((c) => (c && typeof c === "object" && typeof (c as { text?: unknown }).text === "string" ? (c as { text: string }).text : "")).join("");
}

/** Every `additionalContext` string in a parsed line, at any depth (hook output). */
function hookContexts(v: unknown, out: string[] = []): string[] {
  if (Array.isArray(v)) {
    for (const x of v) hookContexts(x, out);
  } else if (v && typeof v === "object") {
    for (const [k, x] of Object.entries(v)) {
      if (k === "additionalContext" && typeof x === "string") out.push(x);
      else hookContexts(x, out);
    }
  }
  return out;
}

/** Bytes of the overlap notices in `text`: from each "Other tasks" heading to the next blank line. */
export function noticeBytes(text: string): number {
  let n = 0;
  let at = text.indexOf("Other tasks");
  while (at >= 0) {
    const end = text.indexOf("\n\n", at);
    const stop = end < 0 ? text.length : end;
    n += Buffer.byteLength(text.slice(at, stop));
    at = text.indexOf("Other tasks", stop);
  }
  return n;
}

export function parseAgentLog(path: string): AgentLog {
  const log: AgentLog = {
    tool_calls: 0,
    edits: 0,
    test_runs: 0,
    verifies: 0,
    checks: 0,
    coordination: 0,
    baseline_by_hand: 0,
    permission_denials: 0,
    aver_output_bytes: 0,
    notice_bytes: 0,
    repeat_verifies: 0,
    sessions: 0,
    turns: 0,
    cost_usd: null,
    session_id: null,
    ended: null,
    final: null,
  };
  if (!existsSync(path)) return log;
  // Tool calls whose results are Aver's output, by id.
  const averCalls = new Set<string>();
  // Whether anything but reading a verdict happened since the last verify.
  let sinceVerify: "none" | "quiet" | "busy" = "none";
  for (const text of readFileSync(path, "utf8").split("\n")) {
    if (!text.startsWith("{")) continue;
    let v: Line;
    try {
      v = JSON.parse(text) as Line;
    } catch {
      continue;
    }
    if (v.session_id && !log.session_id) log.session_id = v.session_id;
    for (const context of hookContexts(v)) {
      log.aver_output_bytes += Buffer.byteLength(context);
      log.notice_bytes += noticeBytes(context);
    }
    const raw = v.message?.content;
    const content: Item[] = Array.isArray(raw) ? raw : [];
    if (v.type === "user") {
      for (const item of content) {
        if (item.type !== "tool_result" || !item.tool_use_id || !averCalls.has(item.tool_use_id)) continue;
        const out = resultText(item.content);
        log.aver_output_bytes += Buffer.byteLength(out);
        log.notice_bytes += noticeBytes(out);
      }
    }
    if (v.type === "assistant") {
      for (const item of content) {
        if (item.type !== "tool_use") continue;
        log.tool_calls += 1;
        if (/^(Edit|Write|MultiEdit|NotebookEdit)$/.test(item.name ?? "")) log.edits += 1;
        const cmd = item.name === "Bash" ? (item.input?.command ?? "") : "";
        if (item.id && /\baver\b/.test(cmd)) averCalls.add(item.id);
        const verify = /\baver verify\b/.test(cmd) && !/--last\b/.test(cmd);
        if (verify) {
          if (sinceVerify === "quiet") log.repeat_verifies += 1;
          sinceVerify = "quiet";
        } else if (/^(Edit|Write|MultiEdit|NotebookEdit)$/.test(item.name ?? "") || (cmd !== "" && !/^\s*aver (verify|status|notices|brief|lease)\b/.test(cmd))) {
          sinceVerify = "busy";
        }
        if (/\bnpm (run )?test\b|\bnode\b[^|;&]*--test\b|\bpython3? -m (unittest|pytest)\b/.test(cmd)) log.test_runs += 1;
        if (/\baver verify\b/.test(cmd)) log.verifies += 1;
        if (/\baver check\b/.test(cmd)) log.checks += 1;
        if (/\baver (lease|notices|brief|release)\b/.test(cmd)) log.coordination += 1;
        if (/\bgit (stash|checkout|worktree|restore)\b/.test(cmd)) log.baseline_by_hand += 1;
      }
    }
    if (v.type === "result") {
      log.sessions += 1;
      log.turns += v.num_turns ?? 0;
      if (typeof v.total_cost_usd === "number") log.cost_usd = (log.cost_usd ?? 0) + v.total_cost_usd;
      log.permission_denials += v.permission_denials?.length ?? 0;
      log.ended = v.subtype ?? null;
      log.final = typeof v.result === "string" ? v.result.slice(0, 2000) : null;
    }
  }
  return log;
}

/** The rating in an answer, as `aver feedback` reads it: `RATING: <n>/10`, else the first `<n>/10`. */
export function rating(answer: string): number | null {
  const upper = answer.toUpperCase();
  const labelled = /RATING[:*\s]*?(\d{1,2})\s*\/\s*10/.exec(upper);
  const any = labelled ?? /(\d{1,2})\s*\/\s*10/.exec(upper);
  const n = any ? Number(any[1]) : NaN;
  return n >= 1 && n <= 10 ? n : null;
}
