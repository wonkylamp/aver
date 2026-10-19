import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { test } from "node:test";
import { parseHTML } from "linkedom";
import { renderVerification } from "../src/render.ts";
import { verifyReceipt, type Verification } from "../src/verify.ts";
import { EXAMPLE_KEY, example, exampleText, publicKeyText, signedReceipt, vectorKey } from "./fixtures.ts";

const HOSTILE_ID = 'unit::<img src=x onerror=alert(1)>';
const HOSTILE_DETAIL = "</pre><script>alert(2)</script><b>bold</b>";

function render(v: Verification): HTMLElement {
  const { document } = parseHTML("<!doctype html><html><body><main></main></body></html>");
  const doc = document as unknown as Document;
  const main = doc.querySelector("main")!;
  main.append(renderVerification(doc, v));
  return main;
}

async function verification(text: string, key: string): Promise<Verification> {
  const outcome = await verifyReceipt(text, key);
  assert.ok(outcome.ok, outcome.ok ? "" : outcome.error);
  return outcome.verification;
}

async function hostile(): Promise<Verification> {
  const key = await vectorKey();
  const checks = [
    { base: "pass", cause: "NEW", detail: HOSTILE_DETAIL, head: "fail", id: HOSTILE_ID, proves: [], runs: { base: [], head: [] }, source: "env" },
  ];
  const receipt = await signedReceipt(key, { checks, unproven: ["div-basic", "div-zero"] });
  return verification(JSON.stringify(receipt), publicKeyText(key));
}

test("markup in a test id or detail renders as text and creates no element", async () => {
  const v = await hostile();
  assert.equal(v.status, "verified");
  const main = render(v);
  assert.equal(main.querySelectorAll("img, script, b").length, 0);
  assert.ok(main.textContent!.includes(HOSTILE_ID));
  assert.ok(main.textContent!.includes(HOSTILE_DETAIL));
  assert.ok(!main.innerHTML.includes("<img"), "the id is escaped in the serialized DOM");
});

test("long repository strings are cut with a count of what was dropped", async () => {
  const key = await vectorKey();
  const longId = `unit::${"x".repeat(5000)}`;
  const checks = [{ base: "pass", cause: null, head: "pass", id: longId, proves: [], runs: { base: [], head: [] }, source: "env" }];
  const main = render(await verification(JSON.stringify(await signedReceipt(key, { checks })), publicKeyText(key)));
  const shown = main.querySelector(".entry-id code")!.textContent!;
  assert.ok(shown.length < 400, `shown ${shown.length} characters`);
  assert.match(shown, /… \(4706 more characters\)$/);
});

test("the example renders its verdict, criteria and checks grouped by cause", async () => {
  const main = render(await verification(exampleText, EXAMPLE_KEY));
  assert.equal(main.querySelector(".status")!.getAttribute("data-status"), "verified");
  assert.equal(main.querySelector(".verdict-name")!.textContent, "FAIL_OWN");

  const criteria = [...main.querySelectorAll(".criterion")].map((li) => [li.querySelector(".crit-id")!.textContent, li.querySelector(".crit-mark")!.textContent]);
  assert.deepEqual(criteria, [["div-basic", "Proven"], ["div-zero", "Unproven"]]);

  const groups = [...main.querySelectorAll(".group")].map((g) => [
    g.querySelector(".cause-chip")!.textContent,
    [...g.querySelectorAll(".entry-id")].map((e) => e.textContent),
  ]);
  assert.deepEqual(groups, [
    ["NEW", ["unit::math > add returns the sum"]],
    ["PREEXISTING", ["unit::[crit:div-zero] divide throws on division by zero"]],
    ["Passing", ["unit::[crit:div-basic] divide returns the quotient", "unit::math > mul returns the product", "unit::math > sub returns the difference"]],
  ]);
  assert.match(main.querySelector(".group-NEW .entry-meta")!.textContent!, /^pass → fail/);
  assert.equal(example.receipt.verdict, "FAIL_OWN");
});

test("a regression-only receipt verifies and says it has no criteria", async () => {
  const key = await vectorKey();
  const checks = [
    { base: "pass", cause: null, head: "pass", id: "unit::math > adds", proves: [], runs: { base: [], head: [] }, source: "env" },
  ];
  const receipt = await signedReceipt(key, { schema: "aver.receipt/v1", criteria: [], unproven: [], checks, verdict: "PASS" });
  const v = await verification(JSON.stringify(receipt), publicKeyText(key));
  assert.equal(v.status, "verified");
  assert.match(render(v).textContent!, /Regression-only: this task has no criteria/);

  const mislabeled = await signedReceipt(key, { criteria: [], unproven: [], checks, verdict: "PASS" });
  const outcome = await verifyReceipt(JSON.stringify(mislabeled), publicKeyText(key));
  assert.equal(outcome.ok, false);
  assert.match(outcome.ok ? "" : outcome.error, /aver\.receipt\/v1 exactly when its criteria are empty/);
});

test("the viewer source never parses HTML", () => {
  const dir = new URL("../src/", import.meta.url);
  const files = readdirSync(dir, { recursive: true, encoding: "utf8" }).filter((f) => f.endsWith(".ts"));
  assert.ok(files.length > 0);
  for (const file of files) {
    const source = readFileSync(new URL(file, dir), "utf8");
    assert.doesNotMatch(source, /innerHTML|outerHTML|insertAdjacentHTML|document\.write|DOMParser|createContextualFragment/, file);
  }
});

async function v2(changes: Record<string, unknown> = {}): Promise<Verification> {
  const key = await vectorKey();
  const checks = [
    {
      base: "fail",
      cause: null,
      head: "pass",
      id: "unit::[crit:div-basic] divide returns the quotient",
      proves: ["div-basic"],
      runs: { base: [], head: [], transplant: [] },
      source: "env",
      transplant: "fail",
      head_tests: "pass",
    },
    {
      base: "n/a",
      cause: null,
      head: "pass",
      id: "unit::[crit:div-zero] divide throws on division by zero",
      proves: ["div-zero"],
      runs: { base: [], head: [], transplant: [] },
      source: "env",
      transplant: "pass",
      head_tests: "pass",
    },
  ];
  const ci = {
    base_commit: "1".repeat(40),
    head_commit: "2".repeat(40),
    config_path: "aver.toml",
    criteria_text: "- div-basic: divide returns the quotient\n- div-zero: <b>divide</b> throws",
    subject: "<img src=x onerror=alert(3)> acme/calc#9",
  };
  const receipt = await signedReceipt(key, {
    schema: "aver.receipt/v2",
    checks,
    unproven: ["div-zero"],
    verdict: "UNPROVEN",
    ci,
    executor: "docker-runc-v0",
    config: "aver.toml",
    test_changes: {
      files_added: { items: [], omitted: 0 },
      files_modified: { items: ["test/<b>divide</b>.test.js"], omitted: 0 },
      files_deleted: { items: [], omitted: 0 },
      tests_removed: { items: ["unit::gone"], omitted: 3 },
      tests_skipped: { items: [], omitted: 0 },
      harness_changed: { items: [], omitted: 0 },
    },
    ...changes,
  });
  return verification(JSON.stringify(receipt), publicKeyText(key));
}

test("a v2 CI receipt verifies and shows its commits, subject, criteria text and transplant results as text", async () => {
  const v = await v2();
  assert.equal(v.status, "verified");
  assert.equal(v.receipt.schema, "aver.receipt/v2");
  const main = render(v);
  const text = main.textContent!;
  assert.ok(text.includes("1".repeat(40)) && text.includes("2".repeat(40)));
  assert.ok(text.includes("<img src=x onerror=alert(3)> acme/calc#9"));
  assert.ok(text.includes("- div-zero: <b>divide</b> throws"));
  assert.equal(main.querySelectorAll("img, b").length, 0);
  const transplant = [...main.querySelectorAll(".entry-transplant")].map((e) => e.textContent);
  assert.deepEqual(transplant.sort(), [" Fails without the change.", " Passes without the change, so it proves nothing."]);
  const proofs = [...main.querySelectorAll(".crit-proof")].map((e) => e.textContent);
  assert.equal(proofs.length, 1, "only the entry that fails without the change proves");
  assert.ok(proofs[0]!.includes("div-basic"));
  assert.ok(text.includes("Plain Docker (runc), without gVisor"), text);
  const changes = [...main.querySelectorAll(".test-changes .change")].map((e) => e.textContent);
  assert.equal(changes.length, 2);
  assert.ok(changes[0]!.startsWith("Modified 1 existing test files.") && changes[0]!.includes("test/<b>divide</b>.test.js"));
  assert.ok(changes[1]!.startsWith("Deleted 4 existing tests.") && changes[1]!.endsWith("unit::gone, and 3 more"));
});

test("deleted tests whose base file did not compile are not said to have run", async () => {
  const v = await v2({
    verdict: "REVIEW",
    unproven: [],
    checks: [
      {
        base: "pass",
        cause: "CHANGED",
        head: "fail",
        id: "rust::(did not compile) crates/calc/tests/gone.rs",
        proves: [],
        runs: { base: [], head: [], transplant: [] },
        source: "env",
        detail: "crates/calc/tests/gone.rs: its base version does not compile against the new code",
      },
    ],
  });
  assert.equal(v.status, "verified");
  const changes = [...render(v).querySelectorAll(".test-changes .change")].map((e) => e.textContent!);
  const deleted = changes.find((c) => c.startsWith("Deleted 4 existing tests."))!;
  assert.ok(!deleted.includes("still ran"), deleted);
  assert.ok(deleted.includes("1 test file did not compile against the new code in its base version (CHANGED, needs review), so its tests did not run; the remaining tests were judged one by one."), deleted);
});

test("a merge base used in place of the requested base is shown", async () => {
  const v = await v2({
    ci: { base_commit: "1".repeat(40), head_commit: "2".repeat(40), config_path: null, criteria_text: null, requested_base: "3".repeat(40) },
  });
  assert.equal(v.status, "verified");
  const text = render(v).textContent!;
  assert.ok(text.includes(`the requested base ${"3".repeat(40)} is not an ancestor of the head`), text);
});

test("a v2 proof needs the head's own version of the test to pass", async () => {
  const v = await v2({
    unproven: ["div-basic", "div-zero"],
    checks: [
      {
        base: "fail",
        cause: null,
        head: "pass",
        id: "unit::[crit:div-basic] divide returns the quotient",
        proves: ["div-basic"],
        runs: { base: [], head: [], transplant: [], head_tests: [] },
        source: "env",
        transplant: "fail",
        head_tests: "fail",
      },
    ],
  });
  const main = render(v);
  assert.equal(main.querySelectorAll(".crit-proof").length, 0);
  assert.ok(main.querySelector(".entry-head-tests")!.textContent!.includes("so it proves nothing"));
});

test("a v2 receipt whose head changed a judging file shows no proof", async () => {
  const v = await v2({
    unproven: ["div-basic", "div-zero"],
    verdict: "UNPROVEN",
    checks: [
      {
        base: "pass",
        cause: "CHANGED",
        head: "fail",
        id: "judging::package.json",
        proves: [],
        runs: { base: [], head: [] },
        source: "env",
        detail: "modified judging file package.json",
      },
      {
        base: "fail",
        cause: null,
        head: "pass",
        id: "unit::[crit:div-basic] divide returns the quotient",
        proves: ["div-basic"],
        runs: { base: [], head: [], transplant: [] },
        source: "env",
        transplant: "fail",
        head_tests: "pass",
      },
    ],
  });
  assert.equal(render(v).querySelectorAll(".crit-proof").length, 0);
});

test("a REVIEW receipt groups its CHANGED entries for a person to review", async () => {
  const v = await v2({
    criteria: [],
    unproven: [],
    verdict: "REVIEW",
    checks: [
      {
        base: "pass",
        cause: "CHANGED",
        head: "fail",
        id: "unit::math > <b>add</b> rounds",
        proves: [],
        runs: { base: [], head: [], head_tests: [] },
        source: "env",
        head_tests: "pass",
      },
      { base: "pass", cause: null, head: "pass", id: "unit::math > mul", proves: [], runs: { base: [], head: [] }, source: "env" },
    ],
  });
  assert.equal(v.status, "verified");
  const main = render(v);
  assert.equal(main.querySelector(".verdict-name")!.textContent, "REVIEW");
  assert.match(main.querySelector(".verdict-text")!.textContent!, /a person should review/);
  const groups = [...main.querySelectorAll(".group")].map((g) => g.querySelector(".cause-chip")!.textContent);
  assert.deepEqual(groups, ["CHANGED", "Passing"]);
  assert.match(main.querySelector(".group-CHANGED .group-text")!.textContent!, /A person reviews these/);
  assert.ok(main.querySelector(".group-CHANGED .entry-id")!.textContent!.includes("<b>add</b>"));
  assert.equal(main.querySelectorAll("b").length, 0);
});

test("changed files no check covers are listed as text", async () => {
  const v = await v2({
    criteria: [],
    unproven: [],
    verdict: "UNPROVEN",
    uncovered: { items: ["native/<b>lib</b>.rs"], omitted: 2 },
  });
  assert.equal(v.status, "verified");
  assert.deepEqual(v.receipt.uncovered, { items: ["native/<b>lib</b>.rs"], omitted: 2 });
  const main = render(v);
  const section = main.querySelector(".uncovered")!;
  assert.match(section.textContent!, /3 source files that no check could exercise/);
  assert.ok(section.querySelector("code")!.textContent!.includes("<b>lib</b>"));
  assert.match(section.textContent!, /and 2 more/);
  assert.equal(main.querySelectorAll("b").length, 0);
});

test("CHANGED and REVIEW in a v0 receipt are refused", async () => {
  const key = await vectorKey();
  const changed = [{ base: "pass", cause: "CHANGED", head: "fail", id: "unit::a", proves: [], runs: { base: [], head: [] }, source: "env" }];
  for (const fields of [{ checks: changed, verdict: "FAIL_OWN" }, { verdict: "REVIEW" }]) {
    const outcome = await verifyReceipt(JSON.stringify(await signedReceipt(key, fields)), publicKeyText(key));
    assert.ok(!outcome.ok, JSON.stringify(fields));
    assert.match(outcome.error, /cause CHANGED and verdict REVIEW exist only in aver.receipt\/v2/);
  }
});

test("v2 fields in a v0 receipt and unknown versions are refused", async () => {
  const key = await vectorKey();
  const withCi = await signedReceipt(key, { ci: { base_commit: "1".repeat(40), head_commit: "2".repeat(40), config_path: null, criteria_text: null } });
  const outcome = await verifyReceipt(JSON.stringify(withCi), publicKeyText(key));
  assert.ok(!outcome.ok);
  assert.match(outcome.error, /only in aver.receipt\/v2/);
  const withExecutor = await signedReceipt(key, { executor: "docker-runsc-v0" });
  const refusedExecutor = await verifyReceipt(JSON.stringify(withExecutor), publicKeyText(key));
  assert.ok(!refusedExecutor.ok);
  const future = await signedReceipt(key, { schema: "aver.receipt/v4" });
  const refused = await verifyReceipt(JSON.stringify(future), publicKeyText(key));
  assert.ok(!refused.ok);
  assert.match(refused.error, /schema must be one of/);
});
