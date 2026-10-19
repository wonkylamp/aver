import assert from "node:assert/strict";
import { test } from "node:test";
import { csvField, csvRow, toCsv } from "../src/csv.ts";

test("csvField leaves plain values alone", () => {
  assert.equal(csvField("LAMP-01"), "LAMP-01");
  assert.equal(csvField(42), "42");
  assert.equal(csvField(""), "");
});

test("csvField quotes values with commas and line breaks", () => {
  assert.equal(csvField("Lamp, brass"), '"Lamp, brass"');
  assert.equal(csvField("two\nlines"), '"two\nlines"');
});

test("csvField doubles embedded double quotes", () => {
  assert.equal(csvField('12" monitor stand'), '"12"" monitor stand"');
  assert.equal(csvField('say "hi", then leave'), '"say ""hi"", then leave"');
});

test("toCsv uses CRLF and ends with a line break", () => {
  assert.equal(csvRow(["a", 1, "b,c"]), 'a,1,"b,c"');
  assert.equal(
    toCsv(
      ["sku", "qty"],
      [
        ["A", 1],
        ["B", 2],
      ],
    ),
    "sku,qty\r\nA,1\r\nB,2\r\n",
  );
});
