// Even splits that add up, refunds included.
import assert from "node:assert/strict";
import { test } from "node:test";
import { Group } from "../../src/group.ts";
import { settle } from "../../src/settle.ts";
import { splitEvenly } from "../../src/split.ts";

test("parts add up and the first people get the extra pennies", () => {
  assert.deepEqual(splitEvenly(1000, 3), [334, 333, 333]);
  assert.deepEqual(splitEvenly(1001, 3), [334, 334, 333]);
  assert.deepEqual(splitEvenly(2, 3), [1, 1, 0]);
  assert.deepEqual(splitEvenly(900, 3), [300, 300, 300]);
});

test("every split adds up and differs by at most a penny", () => {
  for (let total = -50; total <= 50; total++) {
    for (let n = 1; n <= 7; n++) {
      const parts = splitEvenly(total, n);
      assert.equal(parts.reduce((a, b) => a + b, 0), total, `${total}/${n}`);
      assert.ok(Math.max(...parts) - Math.min(...parts) <= 1, `${total}/${n}`);
    }
  }
});

test("a refund splits as the negation of the expense", () => {
  assert.deepEqual(splitEvenly(-1000, 3), [-334, -333, -333]);
  assert.deepEqual(splitEvenly(-1, 2), [-1, 0]);
  for (let total = 1; total <= 40; total++) assert.deepEqual(splitEvenly(-total, 3), splitEvenly(total, 3).map((p) => (p === 0 ? 0 : -p)));
});

test("an expense and its refund leave everyone square", () => {
  const g = new Group(["ana", "ben", "cy"]);
  g.add({ description: "pizza", paidBy: "ana", amountPence: 1000 });
  g.add({ description: "pizza refund", paidBy: "ana", amountPence: -1000 });
  assert.deepEqual(g.balances(), { ana: 0, ben: 0, cy: 0 });
  assert.deepEqual(settle(g.balances()), []);
});

test("a partial refund settles to the penny", () => {
  const g = new Group(["ana", "ben", "cy"]);
  g.add({ description: "tickets", paidBy: "ben", amountPence: 10000 });
  g.add({ description: "one ticket refunded", paidBy: "ben", amountPence: -2501 });
  const b = g.balances();
  assert.deepEqual(b, { ana: -2500, ben: 5000, cy: -2500 });
  assert.deepEqual(settle(b), [
    { from: "ana", to: "ben", pence: 2500 },
    { from: "cy", to: "ben", pence: 2500 },
  ]);
});
