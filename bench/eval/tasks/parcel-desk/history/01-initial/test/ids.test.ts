import assert from "node:assert/strict";
import { test } from "node:test";
import { checkDigit, IdError, isValidTrackingNumber, trackingNumber } from "../src/ids.ts";

test("a tracking number is prefix, serial, check digit and GB", () => {
  assert.equal(checkDigit("00000124"), 4);
  assert.equal(trackingNumber(124), "JD000001244GB");
  assert.equal(trackingNumber(124, "JE"), "JE000001244GB");
});

test("tracking numbers it makes are valid", () => {
  assert.equal(isValidTrackingNumber(trackingNumber(124)), true);
  assert.equal(isValidTrackingNumber("JD000001245GB"), false);
  assert.equal(isValidTrackingNumber("JD00000124GB"), false);
});

test("bad serials and prefixes", () => {
  assert.throws(() => trackingNumber(-1), IdError);
  assert.throws(() => trackingNumber(100_000_000), IdError);
  assert.throws(() => trackingNumber(1.5), IdError);
  assert.throws(() => trackingNumber(1, "jd"), IdError);
});
