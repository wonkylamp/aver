import assert from "node:assert/strict";
import { test } from "node:test";
import { parseCsv, toCsv } from "../src/csv.ts";

test("rows become lines", () => {
  assert.equal(toCsv([["a", "b"], ["1", "2"]]), "a,b\n1,2\n");
});

test("plain fields read back", () => {
  const rows = [["tracking", "town"], ["JD000001244GB", "Leeds"]];
  assert.deepEqual(parseCsv(toCsv(rows)), rows);
});

test("empty fields are kept", () => {
  assert.deepEqual(parseCsv("a,,c\n"), [["a", "", "c"]]);
});
