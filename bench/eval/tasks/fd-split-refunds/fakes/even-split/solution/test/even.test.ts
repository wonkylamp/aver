import assert from "node:assert/strict";
import { test } from "node:test";
import { Group } from "../src/group.ts";
import { splitEvenly } from "../src/split.ts";

test("an uneven amount loses no penny", () => {
  assert.deepEqual(splitEvenly(1000, 3), [334, 333, 333]);
  assert.deepEqual(splitEvenly(1001, 3), [334, 334, 333]);
});

test("a refund splits as the negation of the expense", () => {
  assert.deepEqual(splitEvenly(-1000, 3), [-334, -333, -333]);
  const g = new Group(["ana", "ben", "cy"]);
  g.add({ description: "pizza", paidBy: "ana", amountPence: 1000 });
  g.add({ description: "pizza refund", paidBy: "ana", amountPence: -1000 });
  assert.deepEqual(g.balances(), { ana: 0, ben: 0, cy: 0 });
});
