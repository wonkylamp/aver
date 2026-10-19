import type { JsonObject, JsonValue } from "./core.ts";

/** Every cause, in the order the viewer groups them. CHANGED exists only in v2. */
export const CAUSES = ["NEW", "CHANGED", "ENV", "FLAKY", "PREEXISTING"] as const;
export type Cause = (typeof CAUSES)[number];
export type BaseStatus = "pass" | "fail" | "flaky" | "n/a";
export type HeadStatus = "pass" | "fail" | "flaky";
/** Every verdict. REVIEW exists only in v2. */
export const VERDICTS = ["PASS", "FAIL_OWN", "BLOCKED_NOT_OWN", "UNPROVEN", "REVIEW"] as const;
export type Verdict = (typeof VERDICTS)[number];
export type Transplant = "pass" | "fail" | "absent" | "infra";
export const SCHEMAS = ["aver.receipt/v0", "aver.receipt/v1", "aver.receipt/v2", "aver.receipt/v3"] as const;
export type Schema = (typeof SCHEMAS)[number];

export interface ReceiptCheck {
  id: string;
  base: BaseStatus;
  head: HeadStatus;
  cause: Cause | null;
  proves: string[];
  source: "env" | "task";
  detail?: string;
  /** v2: how a proving entry fared on the base plus the head's new and modified test files. */
  transplant?: Transplant;
  /** v2: how a proving entry fared in the head-tests run (the head with its own versions of the test files). */
  head_tests?: Transplant;
}

/** Up to 20 items of a test_changes list, and how many more there were. */
export interface Listed {
  items: string[];
  omitted: number;
}

export const TEST_CHANGE_LISTS = ["files_added", "files_modified", "files_deleted", "tests_removed", "tests_skipped", "harness_changed"] as const;
export type TestChangeList = (typeof TEST_CHANGE_LISTS)[number];

/** v2: what the head did to the tests and the harness. Every item is repository-derived. */
export type TestChanges = Record<TestChangeList, Listed>;

/** What a CI receipt (v2) records about the run that produced it. */
export interface CiInfo {
  base_commit: string;
  head_commit: string;
  config_path: string | null;
  criteria_text: string | null;
  subject?: string;
  /** The commit --base named when it is not an ancestor of the head; base_commit is then their merge base. */
  requested_base?: string;
}

export interface Criterion {
  id: string;
  text: string;
}

/**
 * The fields of an aver.receipt/v0, v1 or v2 (ARCHITECTURE.md §1.9.3) the
 * viewer shows. v1 is the pre-v2 regression-only receipt: no criteria. v2
 * adds the transplant proof rule and, from `aver ci`, the `ci` object.
 */
export interface Receipt {
  schema: Schema;
  task_id: string;
  attempt_id: string | null;
  agent_id: string;
  base_snapshot: string;
  head_snapshot: string;
  environment_hash: string;
  env_changed: boolean;
  criteria: Criterion[];
  checks: ReceiptCheck[];
  unproven: string[];
  verdict: Verdict;
  reproduce: string;
  verifier: string;
  created_at: string;
  /** v2: the executor profile every run used. */
  executor?: string;
  /** v2: the base's aver.toml, or the default environment. */
  config?: "aver.toml" | "default";
  test_changes?: TestChanges;
  /** v2: changed source files no check covers; with any, the verdict is not PASS or REVIEW. */
  uncovered?: Listed;
  ci?: CiInfo;
  /** v3: where the runs executed. */
  provenance?: Provenance;
  /** v3: how each check that ran was read, by check id. */
  levels?: Record<string, "native" | "junit" | "exit">;
  signature: { alg: "ed25519"; key: string; sig: string };
}

/** v3: natively on the verifier's machine (local), natively in CI, or in the Docker sandbox. */
export interface Provenance {
  mode: "local" | "ci" | "sandbox";
  host_profile?: string;
  network: "off" | "on";
  uid: "separate" | "same";
}

export class ReceiptShapeError extends Error {
  override name = "ReceiptShapeError";
}

export function readReceipt(value: JsonValue): Receipt {
  const o = object(value, "the receipt");
  const schema = oneOf(o, "schema", SCHEMAS);
  const criteria = array(o, "criteria");
  const checks = array(o, "checks").map((c, i) => readCheck(object(c, `checks[${i}]`)));
  const v3Fields = o.provenance !== undefined || o.levels !== undefined;
  if (v3Fields !== (schema === "aver.receipt/v3")) {
    throw new ReceiptShapeError("provenance and levels exist exactly in aver.receipt/v3");
  }
  if (schema !== "aver.receipt/v2" && schema !== "aver.receipt/v3") {
    if ((criteria.length === 0) !== (schema === "aver.receipt/v1")) {
      throw new ReceiptShapeError("a v0 or v1 receipt carries aver.receipt/v1 exactly when its criteria are empty");
    }
    const v2Fields = ["ci", "executor", "config", "test_changes", "uncovered"].some((k) => o[k] !== undefined) || o.verdict === "REVIEW";
    if (v2Fields || checks.some((c) => c.cause === "CHANGED" || c.transplant !== undefined || c.head_tests !== undefined)) {
      throw new ReceiptShapeError(
        "ci, executor, config, test_changes, uncovered, transplant, head_tests, cause CHANGED and verdict REVIEW exist only in aver.receipt/v2",
      );
    }
  }
  const signature = object(o.signature, "signature");
  const receipt: Receipt = {
    schema,
    task_id: string(o, "task_id"),
    attempt_id: o.attempt_id === null || o.attempt_id === undefined ? null : string(o, "attempt_id"),
    agent_id: string(o, "agent_id"),
    base_snapshot: string(o, "base_snapshot"),
    head_snapshot: string(o, "head_snapshot"),
    environment_hash: string(o, "environment_hash"),
    env_changed: boolean(o, "env_changed"),
    criteria: criteria.map((c, i) => {
      const crit = object(c, `criteria[${i}]`);
      return { id: string(crit, "id"), text: string(crit, "text") };
    }),
    checks,
    unproven: array(o, "unproven").map((u, i) => asString(u, `unproven[${i}]`)),
    verdict: oneOf(o, "verdict", VERDICTS),
    reproduce: string(o, "reproduce"),
    verifier: string(o, "verifier"),
    created_at: string(o, "created_at"),
    signature: {
      alg: oneOf(signature, "alg", ["ed25519"]),
      key: string(signature, "key"),
      sig: string(signature, "sig"),
    },
  };
  if (o.ci !== undefined) receipt.ci = readCi(object(o.ci, "ci"));
  if (o.executor !== undefined) receipt.executor = string(o, "executor");
  if (o.config !== undefined) receipt.config = oneOf(o, "config", ["aver.toml", "default"]);
  if (o.test_changes !== undefined) receipt.test_changes = readTestChanges(object(o.test_changes, "test_changes"));
  if (o.uncovered !== undefined) receipt.uncovered = readListed(object(o.uncovered, "uncovered"), "uncovered");
  if (o.provenance !== undefined) {
    const p = object(o.provenance, "provenance");
    receipt.provenance = {
      mode: oneOf(p, "mode", ["local", "ci", "sandbox"]),
      network: oneOf(p, "network", ["off", "on"]),
      uid: oneOf(p, "uid", ["separate", "same"]),
    };
    if (p.host_profile !== undefined) receipt.provenance.host_profile = string(p, "host_profile");
  }
  if (o.levels !== undefined) {
    const l = object(o.levels, "levels");
    receipt.levels = {};
    for (const id of Object.keys(l)) receipt.levels[id] = oneOf(l, id, ["native", "junit", "exit"]);
  }
  return receipt;
}

function readListed(l: JsonObject, field: string): Listed {
  const omitted = l.omitted;
  if (typeof omitted !== "number" || !Number.isInteger(omitted) || omitted < 0) {
    throw new ReceiptShapeError(`${field}.omitted must be a count`);
  }
  return { items: array(l, "items").map((v, i) => asString(v, `${field}.items[${i}]`)), omitted };
}

function readTestChanges(t: JsonObject): TestChanges {
  const read = (key: TestChangeList): Listed => {
    const l = object(t[key], `test_changes.${key}`);
    const omitted = l.omitted;
    if (typeof omitted !== "number" || !Number.isInteger(omitted) || omitted < 0) {
      throw new ReceiptShapeError(`test_changes.${key}.omitted must be a count`);
    }
    return { items: array(l, "items").map((v, i) => asString(v, `test_changes.${key}.items[${i}]`)), omitted };
  };
  return Object.fromEntries(TEST_CHANGE_LISTS.map((k) => [k, read(k)])) as TestChanges;
}

function readCi(c: JsonObject): CiInfo {
  const nullable = (key: string) => (c[key] === null ? null : string(c, key));
  const ci: CiInfo = {
    base_commit: string(c, "base_commit"),
    head_commit: string(c, "head_commit"),
    config_path: nullable("config_path"),
    criteria_text: nullable("criteria_text"),
  };
  if (c.subject !== undefined) ci.subject = string(c, "subject");
  if (c.requested_base !== undefined) ci.requested_base = string(c, "requested_base");
  return ci;
}

function readCheck(c: JsonObject): ReceiptCheck {
  const check: ReceiptCheck = {
    id: string(c, "id"),
    base: oneOf(c, "base", ["pass", "fail", "flaky", "n/a"]),
    head: oneOf(c, "head", ["pass", "fail", "flaky"]),
    cause: c.cause === null ? null : oneOf(c, "cause", CAUSES),
    proves: array(c, "proves").map((p, i) => asString(p, `proves[${i}]`)),
    source: oneOf(c, "source", ["env", "task"]),
  };
  if (c.detail !== undefined) check.detail = string(c, "detail");
  if (c.transplant !== undefined) check.transplant = oneOf(c, "transplant", ["pass", "fail", "absent", "infra"]);
  if (c.head_tests !== undefined) check.head_tests = oneOf(c, "head_tests", ["pass", "fail", "absent", "infra"]);
  return check;
}

function object(value: JsonValue | undefined, what: string): JsonObject {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new ReceiptShapeError(`${what} must be an object`);
  }
  return value;
}

function array(o: JsonObject, key: string): JsonValue[] {
  const v = o[key];
  if (!Array.isArray(v)) throw new ReceiptShapeError(`${key} must be an array`);
  return v;
}

function string(o: JsonObject, key: string): string {
  return asString(o[key], key);
}

function asString(v: JsonValue | undefined, what: string): string {
  if (typeof v !== "string") throw new ReceiptShapeError(`${what} must be a string`);
  return v;
}

function boolean(o: JsonObject, key: string): boolean {
  const v = o[key];
  if (typeof v !== "boolean") throw new ReceiptShapeError(`${key} must be true or false`);
  return v;
}

function oneOf<const T extends string>(o: JsonObject, key: string, allowed: readonly T[]): T {
  const v = o[key];
  if (typeof v === "string" && (allowed as readonly string[]).includes(v)) return v as T;
  throw new ReceiptShapeError(`${key} must be one of ${allowed.join(", ")}, found ${describe(v)}`);
}

function describe(v: JsonValue | undefined): string {
  if (v === undefined) return "nothing";
  const text = JSON.stringify(v);
  return text.length > 60 ? `${text.slice(0, 60)}…` : text;
}
