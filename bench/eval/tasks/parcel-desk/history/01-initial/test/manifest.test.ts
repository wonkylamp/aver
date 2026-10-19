import assert from "node:assert/strict";
import { test } from "node:test";
import { manifestCsv, manifestRows } from "../src/manifest.ts";
import { BELFAST, PARIS, shipment } from "../src/testing/samples.ts";

const DAY = [shipment(), shipment({ tracking: "JD000000025GB", to: BELFAST, service: "express" }), shipment({ tracking: "JD000000031GB", to: PARIS })];

function cell(rows: string[][], row: number, column: string): string | undefined {
  return rows[row]![rows[0]!.indexOf(column)];
}

test("a header row, then one row per parcel", () => {
  const rows = manifestRows(DAY);
  assert.equal(rows.length, 4);
  assert.deepEqual(rows[0]!.slice(0, 3), ["tracking", "service", "name"]);
});

test("the columns", () => {
  const rows = manifestRows(DAY);
  assert.equal(cell(rows, 1, "tracking"), "JD000001244GB");
  assert.equal(cell(rows, 2, "service"), "express");
  assert.equal(cell(rows, 1, "address"), "12 Marsham Street");
  assert.equal(cell(rows, 2, "postcode"), "BT1 1AA");
  assert.equal(cell(rows, 3, "country"), "FR");
  assert.equal(cell(rows, 3, "postcode"), "75005");
  assert.equal(cell(rows, 1, "weight_kg"), "2.5");
});

test("the CSV file", () => {
  const csv = manifestCsv(DAY);
  assert.ok(csv.startsWith("tracking,service,name,address,town,postcode,country,weight_kg"));
  assert.equal(csv.trimEnd().split("\n").length, 4);
});
