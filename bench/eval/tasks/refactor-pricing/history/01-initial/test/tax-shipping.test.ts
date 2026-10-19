import { test } from "node:test";
import assert from "node:assert/strict";
import { taxCents, taxRateBp } from "../src/tax/rates.ts";
import { shippingCents } from "../src/shipping/shipping.ts";

test("tax is rounded to the nearest cent", () => {
  assert.equal(taxCents(1000, "US-CA"), 73);
  assert.equal(taxCents(1000, "EU-DE"), 190);
  assert.equal(taxCents(1000, "US-OR"), 0);
});

test("an unknown region has no tax rate", () => {
  assert.throws(() => taxRateBp("US-XX"), /no tax rate/);
});

test("US shipping is free from $75", () => {
  assert.equal(shippingCents(7499, "US-CA"), 599);
  assert.equal(shippingCents(7500, "US-CA"), 0);
});

test("EU shipping is free from $150", () => {
  assert.equal(shippingCents(14999, "EU-NL"), 1499);
  assert.equal(shippingCents(15000, "EU-NL"), 0);
});

test("we do not ship everywhere", () => {
  assert.throws(() => shippingCents(100, "AU-NSW"), /do not ship/);
});
