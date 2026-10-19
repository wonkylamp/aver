import assert from "node:assert/strict";
import { test } from "node:test";
import { area, AddressError, checkAddress, district, isValidPostcode, normalizePostcode, outward } from "../src/address.ts";
import { LONDON, PARIS } from "../src/testing/samples.ts";

test("postcodes are trimmed and upper-cased", () => {
  assert.equal(normalizePostcode("  sw1a 1aa "), "SW1A 1AA");
  assert.equal(normalizePostcode("LS11 5QX"), "LS11 5QX");
});

test("well-formed postcodes are valid", () => {
  for (const pc of ["SW1A 1AA", "LS11 5QX", "M1 1AE", "B33 8TH", "CR2 6XH", "DN55 1PT"]) assert.equal(isValidPostcode(pc), true, pc);
});

test("outward code, area and district", () => {
  assert.equal(outward("KW15 1DH"), "KW15");
  assert.equal(area("KW15 1DH"), "KW");
  assert.equal(district("KW15 1DH"), 15);
  assert.equal(district("SW1A 1AA"), 1);
});

test("addresses need a name, one to three lines and, at home, a postcode", () => {
  checkAddress(LONDON);
  checkAddress(PARIS);
  assert.throws(() => checkAddress({ ...LONDON, name: " " }), AddressError);
  assert.throws(() => checkAddress({ ...LONDON, lines: [] }), AddressError);
  assert.throws(() => checkAddress({ ...LONDON, lines: ["a", "b", "c", "d"] }), AddressError);
});
