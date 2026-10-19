// #50 on its own: the chargeable_kg column (with #46's chargeable weight).
import assert from "node:assert/strict";
import { test } from "node:test";
import { manifestCsv, manifestRows } from "../../src/manifest.ts";
import { chargeableKg } from "../../src/parcel.ts";
import { BOX, shipment } from "../../src/testing/samples.ts";

const BULKY = { weightKg: 1, lengthCm: 50, widthCm: 40, heightCm: 30 };

test("chargeable_kg comes right after weight_kg", () => {
  const header = manifestRows([])[0]!;
  assert.equal(header.indexOf("chargeable_kg"), header.indexOf("weight_kg") + 1);
  assert.ok(manifestCsv([]).startsWith("tracking,service,name,address,town,postcode,country,weight_kg,chargeable_kg"));
});

test("the chargeable weight per service, to one decimal place", () => {
  const rows = manifestRows([shipment({ parcel: BULKY }), shipment({ parcel: BULKY, service: "express" }), shipment({ parcel: BOX }), shipment({ parcel: { ...BOX, weightKg: 2.01 } })]);
  const col = (name: string) => rows[0]!.indexOf(name);
  assert.deepEqual(rows.slice(1).map((r) => [r[col("weight_kg")], r[col("chargeable_kg")]]), [
    ["1.0", "10.0"],
    ["1.0", "12.0"],
    ["2.5", "2.5"],
    ["2.0", "2.1"],
  ]);
});

test("the column is the parcel module's chargeable weight", () => {
  assert.equal(chargeableKg(BULKY, "economy"), 10);
  assert.equal(chargeableKg({ ...BOX, weightKg: 2.01 }, "economy"), 2.1);
});
