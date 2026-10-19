import assert from "node:assert/strict";
import { test } from "node:test";
import { planInstalments } from "../src/instalments.ts";

test("planInstalments spreads the remainder over the earliest instalments", () => {
  assert.deepEqual(planInstalments(10_000, 3, "2024-01-31", 30), [
    { dueDate: "2024-01-31", amount: 3_334 },
    { dueDate: "2024-03-01", amount: 3_333 },
    { dueDate: "2024-03-31", amount: 3_333 },
  ]);
});

test("planInstalments with a single instalment", () => {
  assert.deepEqual(planInstalments(999, 1, "2024-06-01", 30), [{ dueDate: "2024-06-01", amount: 999 }]);
  assert.throws(() => planInstalments(999, 0, "2024-06-01", 30), RangeError);
});
