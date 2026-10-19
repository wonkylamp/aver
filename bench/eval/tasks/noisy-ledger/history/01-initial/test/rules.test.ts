import assert from "node:assert/strict";
import { setTimeout as sleep } from "node:timers/promises";
import { describe, test } from "node:test";
import type { Rule } from "../src/rules.ts";
import { activeRules, BUILTIN_RULES, categorize, matches, registerRule } from "../src/rules.ts";

const tx = (payee: string, amount = -500, account = "cur") => ({ payee, amount, account });

describe("built-in categories", () => {
  const cases: Array<[string, number, string | undefined]> = [
    ["Tesco Stores", -5610, "groceries"],
    ["Sainsburys S/mkts", -1200, "groceries"],
    ["Aldi", -900, "groceries"],
    ["The Co-op", -450, "groceries"],
    ["Pret A Manger", -420, "eating-out"],
    ["Costa Coffee", -310, "eating-out"],
    ["Café Nero", -295, "eating-out"],
    ["Tfl Travel CH", -280, "transport"],
    ["Uber Trip", -1240, "transport"],
    ["Trainline", -5600, "transport"],
    ["Octopus Energy", -8800, "bills"],
    ["Thames Water", -3100, "bills"],
    ["Council Tax", -15000, "bills"],
    ["Netflix.com", -1099, "subscriptions"],
    ["Spotify", -1199, "subscriptions"],
    ["Rent Flat 2", -120000, "rent"],
    ["Rent A Bike", -1500, undefined],
    ["Payroll ACME LTD", 250000, "income"],
    ["Amazon refund", 1999, "refunds"],
    ["Amazon Mktplace Pmts", -2399, undefined],
  ];
  for (const [payee, amount, want] of cases) {
    test(`${payee} (${amount})`, () => assert.equal(categorize(tx(payee, amount), BUILTIN_RULES.toSorted((a, b) => b.priority - a.priority)), want));
  }
});

describe("matching", () => {
  const rule: Rule = { id: "r", match: { payee: /shop/i, outAtLeast: 1000, account: "cc" }, category: "x", priority: 1 };
  test("every condition must hold", () => {
    assert.equal(matches(rule, tx("Shop", -1000, "cc")), true);
    assert.equal(matches(rule, tx("Shop", -999, "cc")), false);
    assert.equal(matches(rule, tx("Shop", -1000, "cur")), false);
    assert.equal(matches(rule, tx("Store", -1000, "cc")), false);
    assert.equal(matches(rule, tx("Shop", 1000, "cc")), false);
  });
  test("higher priority wins", () => {
    const rules: Rule[] = [
      { id: "a", match: {}, category: "low", priority: 1 },
      { id: "b", match: { payee: /x/ }, category: "high", priority: 9 },
    ].sort((a, b) => b.priority - a.priority);
    assert.equal(categorize(tx("x"), rules), "high");
    assert.equal(categorize(tx("y"), rules), "low");
  });
});

describe("registered rules", { concurrency: true }, () => {
  test("a user rule applies while it is registered", async () => {
    const off = registerRule({ id: "treats", match: { payee: /coffee/i }, category: "treats", priority: 50 });
    await sleep(Math.random() * 10);
    assert.equal(categorize(tx("Costa Coffee")), "treats");
    off();
    assert.equal(activeRules().some((r) => r.id === "treats"), false);
  });
  test("coffee is eating out unless the user says otherwise", async () => {
    await sleep(3 + Math.random() * 10);
    assert.equal(categorize(tx("Costa Coffee")), "eating-out");
  });
  test("rule ids are unique", () => {
    assert.throws(() => registerRule({ id: "groceries", match: {}, category: "x", priority: 1 }), /duplicate rule/);
  });
});
