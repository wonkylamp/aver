import assert from "node:assert/strict";
import { test } from "node:test";
import { zoneOf } from "../src/zones.ts";
import { BELFAST, INVERNESS, KIRKWALL, LONDON, PARIS } from "../src/testing/samples.ts";

test("zones by postcode area and district", () => {
  assert.equal(zoneOf(LONDON), "mainland");
  assert.equal(zoneOf(BELFAST), "ni");
  assert.equal(zoneOf(INVERNESS), "highlands");
  assert.equal(zoneOf(KIRKWALL), "islands");
  assert.equal(zoneOf(PARIS), "international");
});

test("the district decides within an area", () => {
  assert.equal(zoneOf({ ...KIRKWALL, postcode: "KW1 4AA" }), "highlands");
  assert.equal(zoneOf({ ...KIRKWALL, postcode: "KW17 2AA" }), "islands");
  assert.equal(zoneOf({ ...KIRKWALL, postcode: "PA34 4AA" }), "islands");
  assert.equal(zoneOf({ ...KIRKWALL, postcode: "AB10 1AA" }), "mainland");
});

test("postcodes are matched in any case", () => {
  assert.equal(zoneOf({ ...KIRKWALL, postcode: "kw15 1dh" }), "islands");
});
