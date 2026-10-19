// The ten tickets together, as docs/shipping.md says they fit: run on the
// merge of every task's pick.
import assert from "node:assert/strict";
import { test } from "node:test";
import { parseCsv } from "../../src/csv.ts";
import { isValidTrackingNumber, trackingNumber } from "../../src/ids.ts";
import { renderLabel } from "../../src/labels.ts";
import { manifestCsv } from "../../src/manifest.ts";
import { quote } from "../../src/quote.ts";
import { BOX, INVERNESS, KIRKWALL, LONDON, PARIS, shipment } from "../../src/testing/samples.ts";
import { parseFeed, statusOf } from "../../src/tracking.ts";

test("#41 + #47: a sloppy Orkney postcode is on the islands and pays REMOTE", () => {
  const q = quote(BOX, { ...KIRKWALL, postcode: "kw151dh" }, "economy", "2026-03-02");
  assert.equal(q.zone, "islands");
  assert.deepEqual(q.surcharges, [{ code: "FUEL", pence: 30 }, { code: "REMOTE", pence: 900 }]);
  assert.equal(q.totalPence, 1929 + 386);
});

test("#46 + #47: the islands' per-kg surcharge is on chargeable weight", () => {
  const q = quote({ weightKg: 3, lengthCm: 60, widthCm: 50, heightCm: 40 }, KIRKWALL, "express", "2026-03-02");
  assert.equal(q.chargeableKg, 24);
  assert.equal(q.basePence, 4299);
  assert.deepEqual(q.surcharges, [{ code: "FUEL", pence: 129 }, { code: "REMOTE", pence: 1600 }]);
  assert.equal(q.totalPence, 6028 + 1206);
});

test("#43 + #47: Highlands express over Easter", () => {
  const q = quote(BOX, INVERNESS, "express", "2026-04-02");
  assert.equal(q.deliveryBy, "2026-04-07");
  assert.deepEqual(q.surcharges.map((s) => s.code), ["FUEL", "REMOTE"]);
});

test("#48 + #49: a hazmat parcel abroad has both, in their places", () => {
  const parcel = { ...BOX, hazmat: true, contents: [{ description: "Torch", qty: 1, valuePence: 2500, hsCode: "851310" }] };
  const lines = renderLabel(shipment({ to: PARIS, parcel })).split("\n");
  assert.deepEqual(lines.slice(0, 3), ["DISPATCH ECONOMY", "UN3481 LITHIUM ION BATTERIES", ""]);
  assert.deepEqual(lines.slice(-4), ["", "CN22 CUSTOMS DECLARATION", "1 x Torch 851310 £25.00", "TOTAL £25.00"]);
});

test("#41 + #44 + #46 + #50: the manifest reads back with its columns", () => {
  const to = { ...LONDON, name: "Lovelace, Ada", lines: ["Flat 2, 10 High St"], postcode: "sw1p4df" };
  const bulky = { weightKg: 1, lengthCm: 50, widthCm: 40, heightCm: 30 };
  const rows = parseCsv(manifestCsv([shipment({ to, parcel: bulky, service: "express" })]));
  const col = (name: string) => rows[1]![rows[0]!.indexOf(name)];
  assert.equal(rows[1]!.length, rows[0]!.length);
  assert.equal(col("name"), "Lovelace, Ada");
  assert.equal(col("postcode"), "SW1P 4DF");
  assert.equal(col("weight_kg"), "1.0");
  assert.equal(col("chargeable_kg"), String(quote(bulky, to, "express", "2026-03-02").chargeableKg.toFixed(1)));
  assert.equal(col("chargeable_kg"), "12.0");
});

test("#42 + #45: every new tracking number is valid, and returns are tracked", () => {
  for (let serial = 0; serial < 50; serial += 1) assert.ok(isValidTrackingNumber(trackingNumber(serial)));
  assert.equal(statusOf(parseFeed("2026-03-05T10:00:00+00:00 RD Leeds\n2026-03-04T10:00:00+00:00 RS Leeds\n")), "returned");
});
