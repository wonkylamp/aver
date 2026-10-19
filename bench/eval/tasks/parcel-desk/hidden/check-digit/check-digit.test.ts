// #42 on its own: S10 check digits.
import assert from "node:assert/strict";
import { test } from "node:test";
import { checkDigit, isValidTrackingNumber, trackingNumber } from "../../src/ids.ts";

test("a remainder of 1 gives check digit 0", () => {
  assert.equal(checkDigit("00000008"), 0);
  assert.equal(trackingNumber(8), "JD000000080GB");
});

test("a remainder of 0 gives check digit 5", () => {
  assert.equal(checkDigit("00000123"), 5);
  assert.equal(trackingNumber(123), "JD000001235GB");
});

test("the UPU's published example", () => {
  assert.equal(checkDigit("47312482"), 9);
  assert.equal(isValidTrackingNumber("RA473124829GB"), true);
});

test("every tracking number is thirteen characters and valid", () => {
  for (let serial = 0; serial < 2000; serial += 1) {
    const n = trackingNumber(serial);
    assert.equal(n.length, 13, n);
    assert.equal(isValidTrackingNumber(n), true, n);
  }
});

test("a wrong check digit is invalid", () => {
  assert.equal(isValidTrackingNumber("JD000000081GB"), false);
  assert.equal(isValidTrackingNumber("JD000001230GB"), false);
});
