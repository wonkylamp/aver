import assert from "node:assert/strict";
import { test } from "node:test";
import { bandPrice, RATE_CARD, RateError } from "../src/rates.ts";
import { ZONES } from "../src/zones.ts";

test("the first band at or above the weight", () => {
  assert.equal(bandPrice("economy", "mainland", 1), 349);
  assert.equal(bandPrice("economy", "mainland", 1.1), 449);
  assert.equal(bandPrice("express", "islands", 20), 3299);
  assert.equal(bandPrice("express", "international", 0.2), 2199);
});

test("over 30 kg there is no band", () => {
  assert.throws(() => bandPrice("economy", "mainland", 30.1), RateError);
});

test("every service and zone has six bands, dearer as they go", () => {
  for (const service of ["economy", "express"] as const) {
    for (const zone of ZONES) {
      const bands = RATE_CARD[service][zone];
      assert.equal(bands.length, 6);
      for (let i = 1; i < bands.length; i++) assert.ok(bands[i]!.pence > bands[i - 1]!.pence);
    }
  }
});
