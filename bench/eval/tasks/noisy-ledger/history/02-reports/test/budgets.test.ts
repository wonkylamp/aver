import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { budgetMonths, overBudget, spentIn } from "../src/budgets.ts";
import { tx } from "../src/testing/tx.ts";

const food = [
  tx("2026-01-05", -12000, "Tesco", { category: "groceries" }),
  tx("2026-01-20", -9000, "Aldi", { category: "groceries" }),
  tx("2026-02-03", -25000, "Tesco", { category: "groceries" }),
  tx("2026-02-10", 1500, "Tesco refund", { category: "groceries" }),
  tx("2026-03-01", -5000, "Lidl", { category: "groceries" }),
  tx("2026-03-02", -3000, "Pret", { category: "eating-out" }),
];

describe("spentIn", () => {
  const cases: Array<[string, string, number]> = [
    ["groceries", "2026-01", 21000],
    ["groceries", "2026-02", 25000],
    ["groceries", "2026-03", 5000],
    ["eating-out", "2026-03", 3000],
    ["groceries", "2026-04", 0],
  ];
  for (const [category, month, want] of cases) test(`${category} in ${month}`, () => assert.equal(spentIn(food, category, month), want));
});

describe("budgetMonths", () => {
  test("without rollover every month starts fresh", () => {
    const months = budgetMonths({ category: "groceries", limit: 20000, rollover: false }, food, "2026-01", "2026-03");
    assert.deepEqual(months.map((m) => [m.month, m.carried, m.left]), [
      ["2026-01", 0, -1000],
      ["2026-02", 0, -5000],
      ["2026-03", 0, 15000],
    ]);
  });
  test("with rollover the remainder carries over", () => {
    const months = budgetMonths({ category: "groceries", limit: 20000, rollover: true }, food, "2026-01", "2026-03");
    assert.deepEqual(months.map((m) => [m.month, m.carried, m.left]), [
      ["2026-01", 0, -1000],
      ["2026-02", -1000, -6000],
      ["2026-03", -6000, 9000],
    ]);
  });
  test("overBudget lists the months that went over", () => {
    const months = budgetMonths({ category: "groceries", limit: 20000, rollover: false }, food, "2026-01", "2026-03");
    assert.deepEqual(overBudget(months).map((m) => m.month), ["2026-01", "2026-02"]);
  });
  test("a single month", () => {
    assert.equal(budgetMonths({ category: "eating-out", limit: 5000, rollover: true }, food, "2026-03", "2026-03").length, 1);
  });
  test("a budget across the new year continues into January", () => {
    const xmas = [tx("2025-12-20", -30000, "Waitrose", { category: "groceries" }), tx("2026-01-04", -8000, "Tesco", { category: "groceries" })];
    const months = budgetMonths({ category: "groceries", limit: 20000, rollover: true }, xmas, "2025-11", "2026-01");
    assert.deepEqual(months.map((m) => m.month), ["2025-11", "2025-12", "2026-01"]);
    assert.equal(months.at(-1)!.left, 20000 + 10000 - 8000);
  });
  test("a year of months has twelve entries", () => {
    assert.equal(budgetMonths({ category: "groceries", limit: 1, rollover: false }, [], "2026-01", "2026-12").length, 12);
    assert.equal(budgetMonths({ category: "groceries", limit: 1, rollover: false }, [], "2025-07", "2026-06").length, 12);
  });
});
