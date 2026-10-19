// #47 on its own: the remote-area surcharge.
import assert from "node:assert/strict";
import { test } from "node:test";
import { quote } from "../../src/quote.ts";
import { BELFAST, BOX, INVERNESS, KIRKWALL, LONDON } from "../../src/testing/samples.ts";
import { remoteSurcharge } from "../../src/zones.ts";

test("the surcharge by zone and chargeable weight", () => {
  assert.equal(remoteSurcharge("highlands", 3), 450);
  assert.equal(remoteSurcharge("highlands", 25), 450);
  assert.equal(remoteSurcharge("islands", 5), 900);
  assert.equal(remoteSurcharge("islands", 10), 900);
  assert.equal(remoteSurcharge("islands", 10.1), 950);
  assert.equal(remoteSurcharge("islands", 12.3), 1050);
  assert.equal(remoteSurcharge("mainland", 20), 0);
  assert.equal(remoteSurcharge("ni", 20), 0);
  assert.equal(remoteSurcharge("international", 20), 0);
});

test("a Highlands quote", () => {
  const q = quote(BOX, INVERNESS, "economy", "2026-03-02");
  assert.equal(q.basePence, 849);
  assert.deepEqual(q.surcharges, [{ code: "FUEL", pence: 25 }, { code: "REMOTE", pence: 450 }]);
  assert.equal(q.netPence, 1324);
  assert.equal(q.vatPence, 265);
  assert.equal(q.totalPence, 1589);
});

test("a heavy parcel to the islands pays per kg over 10", () => {
  const q = quote({ ...BOX, weightKg: 12 }, KIRKWALL, "express", "2026-03-02");
  assert.equal(q.basePence, 3299);
  assert.deepEqual(q.surcharges, [{ code: "FUEL", pence: 99 }, { code: "REMOTE", pence: 1000 }]);
  assert.equal(q.totalPence, 4398 + 880);
});

test("no REMOTE entry elsewhere", () => {
  assert.deepEqual(quote(BOX, LONDON, "economy", "2026-03-02").surcharges.map((s) => s.code), ["FUEL"]);
  assert.deepEqual(quote(BOX, BELFAST, "economy", "2026-03-02").surcharges.map((s) => s.code), ["FUEL"]);
});
