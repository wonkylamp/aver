import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { parseQuery, QueryError, search } from "../src/search.ts";
import { tx } from "../src/testing/tx.ts";

const txs = [
  tx("2026-03-01", -420, "Pret A Manger", { category: "eating-out", tags: ["work"] }),
  tx("2026-03-02", -310, "Costa Coffee", { category: "eating-out" }),
  tx("2026-03-03", -5610, "Tesco Stores", { category: "groceries", memo: "weekly shop" }),
  tx("2026-03-04", -295, "Café Nero", { category: "eating-out" }),
  tx("2026-03-05", -120000, "Rent Flat 2", { category: "rent", account: "joint" }),
  tx("2026-03-06", 250000, "Payroll ACME", { category: "income" }),
  tx("2026-03-07", -1240, "Uber Trip", { category: "transport", tags: ["work"] }),
];

describe("parseQuery", () => {
  test("words and filters", () => {
    assert.deepEqual(parseQuery("Coffee category:Eating-Out after:2026-03-01 amount>3"), { words: ["coffee"], category: "eating-out", after: "2026-03-01", minAmount: 300 });
  });
  test("amount below", () => assert.deepEqual(parseQuery("amount<10.50"), { words: [], maxAmount: 1050 }));
  test("unknown filter", () => assert.throws(() => parseQuery("colour:red"), QueryError));
  test("bad amount", () => assert.throws(() => parseQuery("amount>lots"), QueryError));
  test("empty", () => assert.deepEqual(parseQuery("   "), { words: [] }));
});

describe("search", () => {
  const cases: Array<[string, string[]]> = [
    ["coffee", ["Costa Coffee"]],
    ["COFFEE", ["Costa Coffee"]],
    ["shop", ["Tesco Stores"]],
    ["category:eating-out", ["Pret A Manger", "Costa Coffee", "Café Nero"]],
    ["tag:work", ["Pret A Manger", "Uber Trip"]],
    ["account:joint", ["Rent Flat 2"]],
    ["after:2026-03-05", ["Payroll ACME", "Uber Trip"]],
    ["before:2026-03-02", ["Pret A Manger"]],
    ["amount>10", ["Tesco Stores", "Rent Flat 2", "Payroll ACME", "Uber Trip"]],
    ["amount<4", ["Costa Coffee", "Café Nero"]],
    ["tag:work amount>5", ["Uber Trip"]],
    ["pret manger", ["Pret A Manger"]],
    ["nothing-matches", []],
    ["café", ["Café Nero"]],
    ["cafe", ["Café Nero"]],
    ["CAFÉ nero", ["Café Nero"]],
    ["category:eating-out cafe", ["Café Nero"]],
  ];
  for (const [q, want] of cases) test(JSON.stringify(q), () => assert.deepEqual(search(txs, q).map((t) => t.payee), want));
});
