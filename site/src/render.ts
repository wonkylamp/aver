// Every string from a receipt is untrusted: test ids and details come from the
// repository under test. This module writes text only through textContent and
// attribute values only through setAttribute, and never parses HTML.
import { CAUSES, type Cause, type CiInfo, type Listed, type Provenance, type Receipt, type ReceiptCheck, type TestChangeList, type Transplant, type Verdict } from "./receipt.ts";
import type { SignatureCheck, Status, Verification } from "./verify.ts";

const MAX_ID = 300;
const MAX_TEXT = 600;
const MAX_DETAIL = 2048;
const OPEN_PASSING_LIMIT = 25;

type Child = Node | string | null | undefined | false;

function h(doc: Document, tag: string, className: string | null, ...children: Child[]): HTMLElement {
  const node = doc.createElement(tag);
  if (className) node.className = className;
  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    node.append(typeof child === "string" ? doc.createTextNode(child) : child);
  }
  return node;
}

/** Text for display, cut to `max` code points with a note of how much was cut. */
export function clip(value: string, max: number): string {
  if (value.length <= max) return value;
  const points = Array.from(value);
  if (points.length <= max) return value;
  return `${points.slice(0, max).join("")}… (${points.length - max} more characters)`;
}

function code(doc: Document, value: string, max = MAX_ID): HTMLElement {
  const node = h(doc, "code", null, clip(value, max));
  if (value.length > max) node.setAttribute("title", clip(value, 4 * max));
  return node;
}

const STATUS: Record<Status, { label: string; tone: "good" | "bad" | "warn" }> = {
  verified: { label: "Verified", tone: "good" },
  "signature-invalid": { label: "Signature invalid", tone: "bad" },
  "id-mismatch": { label: "Id mismatch", tone: "bad" },
  "key-unreadable": { label: "Key unreadable", tone: "warn" },
  "key-not-provided": { label: "Key not provided", tone: "warn" },
};

function statusSentence(v: Verification): string {
  switch (v.status) {
    case "verified":
      return "The verifier's signature is valid and the content id is the hash of exactly these bytes.";
    case "signature-invalid":
      return v.signature.kind === "invalid"
        ? `Do not trust this receipt: ${v.signature.reason}.`
        : "Do not trust this receipt.";
    case "id-mismatch":
      return "The claimed id is not the hash of this content. The receipt changed after its id was taken, or the id belongs to a different receipt.";
    case "key-unreadable":
      return "The verifier key must be 43 characters of unpadded base64url, a 32-byte Ed25519 public key.";
    case "key-not-provided":
      return "Add the verifier's public key to check the signature. Anyone can compute a content id, so the id alone proves nothing about who issued the receipt.";
  }
}

export function renderVerification(doc: Document, v: Verification): DocumentFragment {
  const out = doc.createDocumentFragment();
  const claim = CLAIM_NOTE[v.status];
  out.append(
    renderStatus(doc, v),
    ...(claim === null ? [] : [h(doc, "p", "claim-note", claim)]),
    renderVerdict(doc, v.receipt, v.status === "signature-invalid" || v.status === "id-mismatch"),
    ...(v.receipt.test_changes === undefined ? [] : [renderTestChanges(doc, v.receipt)]),
    ...(v.receipt.uncovered === undefined ? [] : [renderUncovered(doc, v.receipt.uncovered)]),
    renderCriteria(doc, v.receipt),
    renderChecks(doc, v.receipt),
  );
  return out;
}

export function renderError(doc: Document, message: string): HTMLElement {
  return h(
    doc,
    "section",
    "status tone-bad",
    h(doc, "p", "status-label", "Not a readable receipt"),
    h(doc, "p", "status-text", clip(message, MAX_TEXT)),
  );
}

function idDetail(doc: Document, v: Verification): Child[] {
  if (v.claimedId === null) return [h(doc, "span", "note", " No claimed id was given, so there is nothing to compare.")];
  if (v.claimedId === v.computedId) return [h(doc, "span", "note", " Matches the claimed id.")];
  return [h(doc, "span", "note claimed", "Claimed id: "), code(doc, v.claimedId)];
}

function signatureDetail(doc: Document, signature: SignatureCheck): Child[] {
  switch (signature.kind) {
    case "valid":
      return ["Valid, signed by ", code(doc, signature.signer)];
    case "invalid":
      return [`Invalid: ${signature.reason}.`];
    case "bad-key":
      return ["Not checked: the key is unreadable. The receipt names ", code(doc, signature.signer), " as signer."];
    case "no-key":
      return ["Not checked. The receipt names ", code(doc, signature.signer), " as signer."];
  }
}

function renderStatus(doc: Document, v: Verification): HTMLElement {
  const { label, tone } = STATUS[v.status];
  const section = h(
    doc,
    "section",
    `status tone-${tone}`,
    h(doc, "p", "status-label", label),
    h(doc, "p", "status-text", statusSentence(v)),
    h(
      doc,
      "dl",
      "proof",
      h(doc, "dt", null, "Content id"),
      h(doc, "dd", null, code(doc, v.computedId), ...idDetail(doc, v)),
      h(doc, "dt", null, "Signature"),
      h(doc, "dd", null, ...signatureDetail(doc, v.signature)),
    ),
  );
  section.setAttribute("role", "status");
  section.dataset.status = v.status;
  return section;
}

const VERDICT_TEXT: Record<Verdict, string> = {
  PASS: "Every criterion is proven and nothing new broke.",
  FAIL_OWN: "The change broke something that passed on the base.",
  BLOCKED_NOT_OWN: "Infrastructure or flakiness blocked the result. It is not the change's fault.",
  UNPROVEN: "Nothing new broke, but at least one criterion has no passing proof.",
  REVIEW: "Nothing broke; the change rewrote test expectations or harness that a person should review.",
};

const CLAIM_NOTE: Record<Status, string | null> = {
  verified: null,
  "signature-invalid": "Everything below is what the receipt claims. Its signature does not back these claims.",
  "id-mismatch": "Everything below is what the receipt claims. It is not the receipt the claimed id names.",
  "key-unreadable": "Everything below is what the receipt claims. Fix the key to check the signature.",
  "key-not-provided": "Everything below is what the receipt claims. Add the verifier key to check the signature.",
};

function renderVerdict(doc: Document, r: Receipt, untrusted: boolean): HTMLElement {
  const fact = (term: string, ...value: Child[]) => [h(doc, "dt", null, term), h(doc, "dd", null, ...value)];
  return h(
    doc,
    "section",
    `verdict verdict-${r.verdict}${untrusted ? " is-untrusted" : ""}`,
    h(doc, "h2", "verdict-name", r.verdict),
    h(doc, "p", "verdict-text", VERDICT_TEXT[r.verdict]),
    h(
      doc,
      "dl",
      "facts",
      ...fact("Task", code(doc, r.task_id)),
      ...fact("Attempt", r.attempt_id === null ? "None (verified outside an attempt)" : code(doc, r.attempt_id)),
      ...fact("Base snapshot", code(doc, r.base_snapshot)),
      ...fact("Head snapshot", code(doc, r.head_snapshot)),
      ...fact(
        "Environment",
        code(doc, r.environment_hash),
        r.env_changed ? " Changed by the head." : " Unchanged from the base.",
      ),
      ...(r.executor === undefined
        ? []
        : r.provenance !== undefined && r.provenance.mode !== "sandbox"
          ? fact("Ran", provenanceText(r.provenance), " (", code(doc, r.executor), ")")
          : fact("Sandbox", sandboxText(r.executor), " (", code(doc, r.executor), ")")),
      ...(r.config === undefined
        ? []
        : fact("Configuration", r.config === "default" ? "No aver.toml on the base: the default environment, one npm test check" : code(doc, "aver.toml"))),
      ...(r.ci === undefined ? [] : ciFacts(doc, r.ci, fact)),
      ...fact("Issued to", code(doc, r.agent_id)),
      ...fact("Verifier", clip(r.verifier, MAX_ID), ", ", clip(r.created_at, MAX_ID)),
      ...fact("Reproduce", code(doc, r.reproduce, MAX_TEXT)),
    ),
  );
}

/** The CI run a v2 receipt records: commits, configuration, subject and criteria, all as the signer wrote them. */
function ciFacts(doc: Document, ci: CiInfo, fact: (term: string, ...value: Child[]) => HTMLElement[]): HTMLElement[] {
  return [
    ...fact(
      "Commits",
      "Head ",
      code(doc, ci.head_commit),
      " against base ",
      code(doc, ci.base_commit),
      ...(ci.requested_base === undefined
        ? []
        : [", their merge base: the requested base ", code(doc, ci.requested_base), " is not an ancestor of the head"]),
    ),
    ...(ci.subject === undefined ? [] : fact("Subject", code(doc, ci.subject))),
    ...fact(
      "Criteria text",
      ci.criteria_text === null ? "None declared: regression-only" : h(doc, "pre", null, clip(ci.criteria_text, MAX_DETAIL)),
    ),
  ];
}

const JUDGING = "judging::";

/** Where a native v3 receipt's runs executed, in words. */
function provenanceText(p: Provenance): string {
  const where = p.mode === "ci" ? "Natively on a CI runner" : "Natively on the verifier's machine, trusting it and the agent";
  const user = p.uid === "separate" ? "as a separate user" : "as the verifier's own user";
  const network = p.network === "off" ? "network off" : "network on";
  return `${where}, ${user}, ${network}; no sandbox`;
}

function sandboxText(executor: string): string {
  if (executor === "docker-runsc-v0") return "gVisor";
  if (executor === "docker-runc-v0") return "Plain Docker (runc), without gVisor";
  return "Another executor";
}

/** The test path prefix of the entry of a restored test file that did not compile on the judged head. */
const UNBUILT = "(did not compile)";

const CHANGE_TEXT: Record<TestChangeList, [string, string]> = {
  files_added: ["Added", "test files. They ran as the head wrote them."],
  files_modified: ["Modified", "existing test files. Regressions were judged with the base's versions; the head's versions ran separately, to prove criteria."],
  files_deleted: ["Deleted", "existing test files. They still ran from the base's version."],
  tests_removed: ["Deleted", "existing tests. They still ran from the base's version."],
  tests_skipped: ["Skipped", "existing tests in the head's version. They still ran from the base's version."],
  harness_changed: ["Changed", "frozen harness files or fields (CHANGED, for a person to review). Verification ran the base's version."],
};

/** What the head did to the tests (v2): each non-empty list, with every item as code. */
function renderTestChanges(doc: Document, r: Receipt): HTMLElement {
  const tc = r.test_changes!;
  const lists = (Object.keys(CHANGE_TEXT) as TestChangeList[]).filter((k) => tc[k].items.length + tc[k].omitted > 0);
  const unbuilt = r.checks.filter((c) => c.id.includes(`::${UNBUILT} `)).length;
  const item = (key: TestChangeList, list: Listed) => {
    const [verb, rest] = CHANGE_TEXT[key];
    const total = list.items.length + list.omitted;
    const text =
      unbuilt > 0 && (key === "files_deleted" || key === "tests_removed")
        ? `${verb} ${total} ${rest.replace(" They still ran from the base's version.", "")} ${unbuilt} test ${unbuilt === 1 ? "file" : "files"} did not compile against the new code in ${unbuilt === 1 ? "its" : "their"} base version (CHANGED, needs review), so ${unbuilt === 1 ? "its" : "their"} tests did not run; the remaining tests were judged one by one.`
        : `${verb} ${total} ${rest}`;
    return h(
      doc,
      "li",
      "change",
      h(doc, "p", "change-text", text),
      h(doc, "p", "change-items", ...list.items.flatMap((i, n) => [n > 0 ? ", " : "", code(doc, i)]), list.omitted > 0 ? `, and ${list.omitted} more` : ""),
    );
  };
  return h(
    doc,
    "section",
    "test-changes",
    h(doc, "h2", null, "What the change did to the tests"),
    lists.length === 0
      ? h(doc, "p", "note", "Nothing: no test file or test harness changed, and every test that ran on the base still ran.")
      : h(doc, "ul", "changes", ...lists.map((k) => item(k, tc[k]))),
  );
}

/** Changed source files no check covers (v2), each as code. */
function renderUncovered(doc: Document, list: Listed): HTMLElement {
  const total = list.items.length + list.omitted;
  return h(
    doc,
    "section",
    "uncovered",
    h(doc, "h2", null, "Not covered by any check"),
    h(
      doc,
      "p",
      "note",
      `The change touches ${total} source ${total === 1 ? "file" : "files"} that no check could exercise, so the verdict cannot be PASS.`,
    ),
    h(doc, "p", "change-items", ...list.items.flatMap((i, n) => [n > 0 ? ", " : "", code(doc, i)]), list.omitted > 0 ? `, and ${list.omitted} more` : ""),
  );
}

/**
 * The entries that prove `criterion`: passing on head with no cause, and in
 * v2 also failing (or absent) without the change and passing in the
 * head-tests run, in a receipt whose head changed no judging file.
 */
function provenBy(r: Receipt, criterion: string): ReceiptCheck[] {
  const v2 = r.schema === "aver.receipt/v2" || r.schema === "aver.receipt/v3";
  if (v2 && r.checks.some((c) => c.id.startsWith(JUDGING))) return [];
  return r.checks.filter(
    (c) =>
      c.proves.includes(criterion) &&
      c.head === "pass" &&
      c.cause === null &&
      (!v2 || ((c.transplant === "fail" || c.transplant === "absent") && c.head_tests === "pass")),
  );
}

const TRANSPLANT_TEXT: Record<Transplant, string> = {
  fail: " Fails without the change.",
  absent: " Cannot run without the change.",
  pass: " Passes without the change, so it proves nothing.",
  infra: " Without the change, the run hit an infrastructure failure.",
};

const HEAD_TESTS_TEXT: Record<Transplant, string> = {
  pass: " Passes in the head's own version of the tests.",
  fail: " Fails in the head's own version of the tests, so it proves nothing.",
  absent: " Missing from the head's own version of the tests, so it proves nothing.",
  infra: " The head's own version of the tests hit an infrastructure failure.",
};

function renderCriteria(doc: Document, r: Receipt): HTMLElement {
  const unproven = new Set(r.unproven);
  const items = r.criteria.map((c) => {
    const proven = !unproven.has(c.id);
    const proofs = provenBy(r, c.id);
    return h(
      doc,
      "li",
      proven ? "criterion is-proven" : "criterion is-unproven",
      h(doc, "span", "crit-mark", proven ? "Proven" : "Unproven"),
      h(doc, "span", "crit-id", code(doc, c.id)),
      h(doc, "span", "crit-text", clip(c.text, MAX_TEXT)),
      proofs.length > 0 && h(doc, "span", "crit-proof", "By ", code(doc, proofs[0]!.id), proofs.length > 1 ? ` and ${proofs.length - 1} more` : ""),
    );
  });
  return h(
    doc,
    "section",
    "criteria",
    h(doc, "h2", null, "Criteria"),
    items.length === 0 ? h(doc, "p", "note", "Regression-only: this task has no criteria, so the verdict is PASS unless a check fails only with the change (NEW), for infrastructure reasons (ENV), or in a way that hides the change (a check without per-test results failing on both sides). It is REVIEW when the change rewrote test expectations or harness (CHANGED).") : h(doc, "ul", "criteria-list", ...items),
  );
}

const GROUP_TEXT: Record<Cause | "passing", string> = {
  NEW: "Fails only with the change.",
  CHANGED:
    "Test expectations or harness this change rewrote: the old version fails on the new code, the change's own version passes. A person reviews these.",
  ENV: "Infrastructure failed: out of memory, a timeout or a sandbox error.",
  FLAKY: "Runs on the same side disagreed with each other.",
  PREEXISTING: "Already failing on the base.",
  passing: "Passing on the head.",
};

function renderChecks(doc: Document, r: Receipt): HTMLElement {
  const groups: { key: Cause | "passing"; checks: ReceiptCheck[] }[] = [
    ...CAUSES.map((cause) => ({ key: cause, checks: r.checks.filter((c) => c.cause === cause) })),
    { key: "passing" as const, checks: r.checks.filter((c) => c.cause === null) },
  ];
  const counts = groups.filter((g) => g.checks.length > 0).map((g) => `${g.checks.length} ${g.key === "passing" ? "passing" : g.key}`);
  const section = h(
    doc,
    "section",
    "checks",
    h(doc, "h2", null, "Checks"),
    h(doc, "p", "note", r.checks.length === 0 ? "No check entries." : `${r.checks.length} entries: ${counts.join(", ")}.`),
  );
  for (const group of groups) {
    if (group.checks.length === 0) continue;
    const body = h(
      doc,
      "div",
      "group-body",
      h(doc, "p", "group-text", GROUP_TEXT[group.key]),
      h(doc, "ol", "entries", ...group.checks.map((c) => renderEntry(doc, c))),
    );
    const title = h(doc, "span", `cause-chip cause-${group.key}`, group.key === "passing" ? "Passing" : group.key);
    const count = h(doc, "span", "group-count", String(group.checks.length));
    if (group.key === "passing") {
      const details = h(doc, "details", "group group-passing", h(doc, "summary", "group-head", title, count), body);
      if (group.checks.length <= OPEN_PASSING_LIMIT) details.setAttribute("open", "");
      section.append(details);
    } else {
      section.append(h(doc, "div", `group group-${group.key}`, h(doc, "h3", "group-head", title, count), body));
    }
  }
  return section;
}

function renderEntry(doc: Document, c: ReceiptCheck): HTMLElement {
  return h(
    doc,
    "li",
    "entry",
    h(doc, "p", "entry-id", code(doc, c.id)),
    h(
      doc,
      "p",
      "entry-meta",
      h(doc, "span", `run run-${c.base.replace("/", "")}`, c.base),
      h(doc, "span", "arrow", " → "),
      h(doc, "span", `run run-${c.head}`, c.head),
      ".",
      c.proves.length > 0 && h(doc, "span", "entry-proves", " Proves ", ...c.proves.flatMap((p, i) => [i > 0 ? ", " : "", code(doc, p)]), "."),
      h(doc, "span", "entry-source", c.source === "task" ? " Defined by the task." : " Defined by the project."),
      c.transplant !== undefined && h(doc, "span", "entry-transplant", TRANSPLANT_TEXT[c.transplant]),
      c.head_tests !== undefined && h(doc, "span", "entry-head-tests", HEAD_TESTS_TEXT[c.head_tests]),
    ),
    c.detail !== undefined &&
      h(doc, "details", "entry-detail", h(doc, "summary", null, "Failure detail"), h(doc, "pre", null, clip(c.detail, MAX_DETAIL))),
  );
}
