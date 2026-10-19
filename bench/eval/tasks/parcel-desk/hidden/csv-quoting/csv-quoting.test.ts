// #44 on its own: CSV quoting.
import assert from "node:assert/strict";
import { test } from "node:test";
import { parseCsv, toCsv } from "../../src/csv.ts";
import { manifestCsv } from "../../src/manifest.ts";
import { LONDON, shipment } from "../../src/testing/samples.ts";

test("fields with commas, quotes or line breaks are quoted", () => {
  assert.equal(toCsv([["Flat 2, 10 High St", 'a 5" box', "two\nlines", "plain"]]), '"Flat 2, 10 High St","a 5"" box","two\nlines",plain\n');
  assert.equal(toCsv([["carriage\rreturn"]]), '"carriage\rreturn"\n');
});

test("plain fields are written as they are", () => {
  assert.equal(toCsv([["a", "", "c d"]]), "a,,c d\n");
});

test("quoted fields read back", () => {
  const rows = [["name", "address"], ["O'Brien, Pat", 'The "Old" Mill\nBack Lane'], ["", "x"]];
  assert.deepEqual(parseCsv(toCsv(rows)), rows);
  assert.deepEqual(parseCsv('"a,b",c\r\n"d""e",f\r\n'), [["a,b", "c"], ['d"e', "f"]]);
});

test("a manifest with a comma in an address keeps its columns", () => {
  const csv = manifestCsv([shipment({ to: { ...LONDON, name: "Lovelace, Ada", lines: ["Flat 2, 10 High St"] } })]);
  const rows = parseCsv(csv);
  assert.equal(rows.length, 2);
  assert.equal(rows[1]!.length, rows[0]!.length);
  assert.equal(rows[1]![rows[0]!.indexOf("name")], "Lovelace, Ada");
  assert.equal(rows[1]![rows[0]!.indexOf("address")], "Flat 2, 10 High St");
});
