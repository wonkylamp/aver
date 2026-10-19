// #49 on its own: CN22 customs declarations on labels going abroad.
import assert from "node:assert/strict";
import { test } from "node:test";
import { LabelError, renderLabel } from "../../src/labels.ts";
import { BOX, PARIS, shipment } from "../../src/testing/samples.ts";

const GOODS = {
  ...BOX,
  contents: [
    { description: "T-shirt", qty: 2, valuePence: 1500, hsCode: "610910" },
    { description: "Mug", qty: 1, valuePence: 1250 },
  ],
};

test("a parcel abroad ends with its CN22 block", () => {
  const lines = renderLabel(shipment({ to: PARIS, parcel: GOODS })).split("\n");
  const at = lines.indexOf("TRACKING: JD000001244GB");
  assert.deepEqual(lines.slice(at + 1), ["", "CN22 CUSTOMS DECLARATION", "2 x T-shirt 610910 £30.00", "1 x Mug - £12.50", "TOTAL £42.50"]);
  assert.equal(lines[0], "DISPATCH ECONOMY");
  assert.ok(lines.includes("75005 FR"));
});

test("long contents lines are cut like any other", () => {
  const lines = renderLabel(shipment({ to: PARIS, parcel: { ...BOX, contents: [{ description: "Hand-thrown stoneware serving platter", qty: 1, valuePence: 8000 }] } })).split("\n");
  assert.equal(lines.at(-2)!.length, 40);
  assert.ok(lines.at(-2)!.endsWith("…"));
  assert.equal(lines.at(-1), "TOTAL £80.00");
});

test("abroad without contents is a LabelError", () => {
  assert.throws(() => renderLabel(shipment({ to: PARIS })), LabelError);
  assert.throws(() => renderLabel(shipment({ to: PARIS, parcel: { ...BOX, contents: [] } })), LabelError);
});

test("domestic labels have no declaration", () => {
  assert.ok(!renderLabel(shipment({ parcel: GOODS })).includes("CN22"));
});
