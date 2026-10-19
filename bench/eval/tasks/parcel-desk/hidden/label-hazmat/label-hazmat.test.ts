// #48 on its own: hazmat labels.
import assert from "node:assert/strict";
import { test } from "node:test";
import { LabelError, renderLabel } from "../../src/labels.ts";
import { BOX, shipment } from "../../src/testing/samples.ts";

test("the hazmat line follows the DISPATCH line", () => {
  const lines = renderLabel(shipment({ parcel: { ...BOX, hazmat: true } })).split("\n");
  assert.deepEqual(lines.slice(0, 4), ["DISPATCH ECONOMY", "UN3481 LITHIUM ION BATTERIES", "", "TO: Ada Lovelace"]);
  assert.equal(lines.at(-1), "TRACKING: JD000001244GB");
  assert.equal(lines.length, 11);
});

test("other parcels are labelled as before", () => {
  const plain = renderLabel(shipment());
  assert.equal(renderLabel(shipment({ parcel: { ...BOX, hazmat: false } })), plain);
  assert.ok(!plain.includes("UN3481"));
});

test("express does not carry hazmat", () => {
  assert.throws(() => renderLabel(shipment({ service: "express", parcel: { ...BOX, hazmat: true } })), LabelError);
  renderLabel(shipment({ service: "express" }));
});
