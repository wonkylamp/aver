import assert from "node:assert/strict";
import { test } from "node:test";
import { Group, GroupError } from "../src/group.ts";
import { formatPence, MoneyError, parsePounds } from "../src/money.ts";
import { settle, SettleError } from "../src/settle.ts";

test("pounds and pence", () => {
  assert.equal(parsePounds("12.34"), 1234);
  assert.equal(parsePounds("12.5"), 1250);
  assert.equal(parsePounds("-0.05"), -5);
  assert.equal(parsePounds("7"), 700);
  assert.throws(() => parsePounds("1.234"), MoneyError);
  assert.equal(formatPence(-1250), "-£12.50");
});

test("balances of a weekend away", () => {
  const g = new Group(["ana", "ben", "cy"]);
  g.add({ description: "cabin", paidBy: "ana", amountPence: 30000 });
  g.add({ description: "food", paidBy: "ben", amountPence: 6000 });
  g.add({ description: "kayaks", paidBy: "cy", amountPence: 4000, among: ["ben", "cy"] });
  assert.deepEqual(g.balances(), { ana: 18000, ben: -8000, cy: -10000 });
});

test("uneven shares", () => {
  const g = new Group(["ana", "ben"]);
  g.add({ description: "taxi", paidBy: "ana", amountPence: 3000, shares: [2, 1] });
  assert.deepEqual(g.balances(), { ana: 1000, ben: -1000 });
});

test("strangers and bad groups", () => {
  assert.throws(() => new Group([]), GroupError);
  assert.throws(() => new Group(["a", "a"]), GroupError);
  const g = new Group(["ana", "ben"]);
  assert.throws(() => g.add({ description: "x", paidBy: "zed", amountPence: 100 }), GroupError);
  assert.throws(() => g.add({ description: "x", paidBy: "ana", amountPence: 100, shares: [1] }), GroupError);
});

test("settling up", () => {
  assert.deepEqual(settle({ ana: 18000, ben: -8000, cy: -10000 }), [
    { from: "cy", to: "ana", pence: 10000 },
    { from: "ben", to: "ana", pence: 8000 },
  ]);
  assert.deepEqual(settle({ ana: 0, ben: 0 }), []);
  assert.throws(() => settle({ ana: 1, ben: 0 }), SettleError);
});
