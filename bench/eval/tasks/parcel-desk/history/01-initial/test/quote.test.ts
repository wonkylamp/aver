import assert from "node:assert/strict";
import { test } from "node:test";
import { AddressError } from "../src/address.ts";
import { ParcelError } from "../src/parcel.ts";
import { quote } from "../src/quote.ts";
import { BELFAST, BOX, LONDON, PARIS, SMALL } from "../src/testing/samples.ts";

test("a mainland economy quote", () => {
  const q = quote(BOX, LONDON, "economy", "2026-03-02");
  assert.equal(q.zone, "mainland");
  assert.equal(q.chargeableKg, 2.5);
  assert.equal(q.basePence, 649);
  assert.deepEqual(q.surcharges, [{ code: "FUEL", pence: 19 }]);
  assert.equal(q.netPence, 668);
  assert.equal(q.vatPence, 134);
  assert.equal(q.totalPence, 802);
  assert.equal(q.deliveryBy, "2026-03-05");
});

test("express to Northern Ireland", () => {
  const q = quote(SMALL, BELFAST, "express", "2026-03-06");
  assert.equal(q.zone, "ni");
  assert.equal(q.basePence, 999);
  assert.deepEqual(q.surcharges, [{ code: "FUEL", pence: 30 }]);
  assert.equal(q.totalPence, 1029 + 206);
  assert.equal(q.deliveryBy, "2026-03-09");
});

test("abroad", () => {
  const q = quote(BOX, PARIS, "economy", "2026-03-02");
  assert.equal(q.zone, "international");
  assert.equal(q.basePence, 2299);
});

test("bad parcels and addresses are not quoted", () => {
  assert.throws(() => quote({ ...BOX, weightKg: 31 }, LONDON, "economy", "2026-03-02"), ParcelError);
  assert.throws(() => quote(BOX, { ...LONDON, name: "" }, "economy", "2026-03-02"), AddressError);
});
