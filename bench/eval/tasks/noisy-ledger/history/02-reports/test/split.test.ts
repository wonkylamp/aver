import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { Ledger } from "../src/ledger.ts";
import { SplitError, splitAmounts, splitTransaction } from "../src/split.ts";

describe("splitAmounts", () => {
  const cases: Array<[string, number, Array<{ amount?: number; share?: number }>, number[]]> = [
    ["even halves", -1000, [{}, {}], [-500, -500]],
    ["even thirds keep every penny", -1000, [{}, {}, {}], [-334, -333, -333]],
    ["a fixed part and the rest", -5610, [{ amount: -1200 }, {}], [-1200, -4410]],
    ["fixed parts only", -1000, [{ amount: -400 }, { amount: -600 }], [-400, -600]],
    ["weighted shares", -1000, [{ share: 3 }, { share: 1 }], [-750, -250]],
    ["weighted shares with rounding", -1001, [{ share: 2 }, { share: 1 }], [-667, -334]],
    ["income splits too", 900, [{}, {}], [450, 450]],
  ];
  for (const [name, total, parts, want] of cases) {
    test(name, () => {
      const got = splitAmounts(total, parts.map((p) => ({ category: "x", ...p })));
      assert.deepEqual(got, want);
      assert.equal(got.reduce((a, b) => a + b, 0), total);
    });
  }
  test("one part is not a split", () => assert.throws(() => splitAmounts(-100, [{ category: "x" }]), SplitError));
  test("fixed parts must add up", () => assert.throws(() => splitAmounts(-100, [{ category: "x", amount: -50 }, { category: "y", amount: -40 }]), SplitError));
});

test("splitTransaction replaces the transaction with its parts", () => {
  const l = new Ledger();
  l.addAccount({ id: "cur", name: "Current", kind: "current", currency: "GBP", openingBalance: 0 });
  const t = l.add({ account: "cur", date: "2026-03-03", amount: -5610, payee: "Tesco Stores", tags: ["weekly"], source: { kind: "manual" } });
  const parts = splitTransaction(l, t.id, [{ category: "groceries" }, { category: "household", amount: -1200 }]);
  assert.deepEqual(parts.map((p) => [p.category, p.amount, p.tags]), [
    ["groceries", -4410, ["weekly", "split"]],
    ["household", -1200, ["weekly", "split"]],
  ]);
  assert.equal(l.balance("cur"), -5610);
  assert.equal(l.get(t.id), undefined);
});
