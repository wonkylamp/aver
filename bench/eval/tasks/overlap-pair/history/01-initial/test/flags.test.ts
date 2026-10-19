import { test } from "node:test";
import assert from "node:assert/strict";
import { createFlags } from "../src/flags/flags.ts";

test("listed flags are on, others off", () => {
  const flags = createFlags(["new-dashboard"]);
  assert.equal(flags.isEnabled("new-dashboard"), true);
  assert.equal(flags.isEnabled("dark-mode"), false);
});

test("names are case-insensitive and trimmed", () => {
  const flags = createFlags([" New-Dashboard "]);
  assert.equal(flags.isEnabled("NEW-dashboard"), true);
});

test("list is sorted, unique and skips blanks", () => {
  assert.deepEqual(createFlags(["b", "a", "B", " "]).list(), ["a", "b"]);
});
