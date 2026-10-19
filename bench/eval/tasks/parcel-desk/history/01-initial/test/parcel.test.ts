import assert from "node:assert/strict";
import { test } from "node:test";
import { checkParcel, formatKg, girthCm, ParcelError, volumeCm3 } from "../src/parcel.ts";
import { BOX } from "../src/testing/samples.ts";

test("girth and volume", () => {
  assert.equal(girthCm(BOX), 30 + 40 + 20);
  assert.equal(volumeCm3(BOX), 6000);
});

test("the carrier's limits", () => {
  checkParcel(BOX);
  assert.throws(() => checkParcel({ ...BOX, weightKg: 30.5 }), ParcelError);
  assert.throws(() => checkParcel({ ...BOX, weightKg: 0 }), ParcelError);
  assert.throws(() => checkParcel({ ...BOX, lengthCm: 151 }), ParcelError);
  assert.throws(() => checkParcel({ ...BOX, lengthCm: 140, widthCm: 50, heightCm: 31 }), ParcelError);
  assert.throws(() => checkParcel({ ...BOX, contents: [{ description: "Mug", qty: 0, valuePence: 900 }] }), ParcelError);
});

test("formatKg", () => {
  assert.equal(formatKg(2.5), "2.5 kg");
  assert.equal(formatKg(12), "12.0 kg");
});
