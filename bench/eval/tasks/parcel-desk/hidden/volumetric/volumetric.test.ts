// #46 on its own: volumetric and chargeable weight.
import assert from "node:assert/strict";
import { test } from "node:test";
import { chargeableKg, volumetricKg } from "../../src/parcel.ts";
import { quote } from "../../src/quote.ts";
import { RateError } from "../../src/rates.ts";
import { BOX, LONDON } from "../../src/testing/samples.ts";

const BULKY = { weightKg: 1, lengthCm: 50, widthCm: 40, heightCm: 30 };

test("volumetric weight uses the service's divisor", () => {
  assert.equal(volumetricKg(BOX, "economy"), 1);
  assert.equal(volumetricKg(BOX, "express"), 1.2);
  assert.equal(volumetricKg(BULKY, "economy"), 10);
  assert.equal(volumetricKg(BULKY, "express"), 12);
});

test("chargeable weight is the greater, rounded up to 0.1 kg", () => {
  assert.equal(chargeableKg(BOX, "economy"), 2.5);
  assert.equal(chargeableKg(BULKY, "express"), 12);
  assert.equal(chargeableKg({ ...BOX, weightKg: 2.01 }, "economy"), 2.1);
  assert.equal(chargeableKg({ weightKg: 0.5, lengthCm: 31, widthCm: 31, heightCm: 31 }, "express"), 6);
  assert.equal(chargeableKg({ weightKg: 0.5, lengthCm: 31, widthCm: 31, heightCm: 31 }, "economy"), 5);
});

test("quotes price bulky parcels by chargeable weight", () => {
  const economy = quote(BULKY, LONDON, "economy", "2026-03-02");
  assert.equal(economy.chargeableKg, 10);
  assert.equal(economy.basePence, 899);
  const express = quote(BULKY, LONDON, "express", "2026-03-02");
  assert.equal(express.chargeableKg, 12);
  assert.equal(express.basePence, 2199);
  assert.deepEqual(express.surcharges, [{ code: "FUEL", pence: 66 }]);
  assert.equal(express.totalPence, 2265 + 453);
});

test("heavy small parcels are priced as before", () => {
  const q = quote(BOX, LONDON, "express", "2026-03-02");
  assert.equal(q.chargeableKg, 2.5);
  assert.equal(q.basePence, 1099);
});

test("a chargeable weight over the last band is a RateError", () => {
  assert.throws(() => quote({ weightKg: 2, lengthCm: 100, widthCm: 50, heightCm: 40 }, LONDON, "express", "2026-03-02"), RateError);
});
