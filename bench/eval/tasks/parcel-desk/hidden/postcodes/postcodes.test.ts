// #41 on its own, as docs/shipping.md plans it.
import assert from "node:assert/strict";
import { test } from "node:test";
import { checkAddress, AddressError, isValidPostcode, normalizePostcode } from "../../src/address.ts";
import { renderLabel } from "../../src/labels.ts";
import { manifestRows } from "../../src/manifest.ts";
import { LONDON, shipment } from "../../src/testing/samples.ts";
import { zoneOf } from "../../src/zones.ts";

test("spaces are removed and one goes before the inward code", () => {
  assert.equal(normalizePostcode("sw1a1aa"), "SW1A 1AA");
  assert.equal(normalizePostcode(" ec1a  1bb "), "EC1A 1BB");
  assert.equal(normalizePostcode("m11ae"), "M1 1AE");
  assert.equal(normalizePostcode("S W 1 A 1 A A"), "SW1A 1AA");
  assert.equal(normalizePostcode("ls11 5qx"), "LS11 5QX");
});

test("short strings are only upper-cased without spaces", () => {
  assert.equal(normalizePostcode("ab 1"), "AB1");
  assert.equal(normalizePostcode(""), "");
});

test("only the UK formats are valid", () => {
  for (const pc of ["M1 1AE", "B33 8TH", "CR2 6XH", "DN55 1PT", "W1A 0AX", "EC1A 1BB", "GIR 0AA"]) assert.equal(isValidPostcode(pc), true, pc);
  for (const pc of ["12345", "SW1A1AA", "SW1A 1A", "1W1A 1AA", "SW1A 1AAA", "ABCDE", "Q1 1AA 1"]) assert.equal(isValidPostcode(pc), false, pc);
});

test("addresses with junk postcodes are refused, sloppy ones are fine", () => {
  assert.throws(() => checkAddress({ ...LONDON, postcode: "12345" }), AddressError);
  checkAddress({ ...LONDON, postcode: "sw1p4df" });
});

test("zones, labels and the manifest use the normalized postcode", () => {
  const orkney = { ...LONDON, postcode: "kw151dh" };
  assert.equal(zoneOf(orkney), "islands");
  assert.ok(renderLabel(shipment({ to: orkney })).includes("\nKW15 1DH\n"));
  const rows = manifestRows([shipment({ to: orkney })]);
  assert.equal(rows[1]![rows[0]!.indexOf("postcode")], "KW15 1DH");
});
