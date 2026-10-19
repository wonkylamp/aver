import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { incomeAndSpending, spendingByCategory, topPayees } from "../src/reports.ts";
import { tx } from "../src/testing/tx.ts";

const march = [
  tx("2026-03-01", 250000, "Payroll", { category: "income" }),
  tx("2026-03-02", -120000, "Rent Flat 2", { category: "rent" }),
  tx("2026-03-03", -5610, "Tesco Stores", { category: "groceries" }),
  tx("2026-03-09", -4200, "Tesco Stores", { category: "groceries" }),
  tx("2026-03-10", -420, "Pret A Manger", { category: "eating-out" }),
  tx("2026-03-11", -310, "Costa Coffee", { category: "eating-out" }),
  tx("2026-03-12", -50000, "To savings", { category: "transfer" }),
  tx("2026-03-15", -2399, "Amazon"),
  tx("2026-04-01", -9999, "Tesco Stores", { category: "groceries" }),
];

describe("spendingByCategory", () => {
  const got = spendingByCategory(march, "2026-03");
  const cases: Array<[string, number, number]> = [
    ["rent", 120000, 1],
    ["transfer", 50000, 1],
    ["groceries", 9810, 2],
    ["uncategorised", 2399, 1],
    ["eating-out", 730, 2],
  ];
  for (const [category, spent, count] of cases) {
    test(category, () => assert.deepEqual(got.find((c) => c.category === category), { category, spent, count }));
  }
  test("largest first", () => assert.deepEqual(got.map((c) => c.category), ["rent", "transfer", "groceries", "uncategorised", "eating-out"]));
  test("income is not spending", () => assert.equal(got.some((c) => c.category === "income"), false));
});

describe("topPayees", () => {
  test("by total spent", () => {
    assert.deepEqual(topPayees(march, "2026-03", 3), [
      { payee: "Rent Flat 2", spent: 120000 },
      { payee: "To savings", spent: 50000 },
      { payee: "Tesco Stores", spent: 9810 },
    ]);
  });
  test("an empty month", () => assert.deepEqual(topPayees(march, "2026-05"), []));
});

describe("incomeAndSpending", () => {
  test("transfers are neither", () => assert.deepEqual(incomeAndSpending(march, "2026-03"), { income: 250000, spending: 132939, net: 117061 }));
  test("another month", () => assert.deepEqual(incomeAndSpending(march, "2026-04"), { income: 0, spending: 9999, net: -9999 }));
});
