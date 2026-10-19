import assert from "node:assert/strict";
import { test } from "node:test";
import { renderLabel, WIDTH } from "../src/labels.ts";
import { LONDON, shipment } from "../src/testing/samples.ts";

test("a domestic economy label", () => {
  assert.equal(
    renderLabel(shipment()),
    [
      "DISPATCH ECONOMY",
      "",
      "TO: Ada Lovelace",
      "12 Marsham Street",
      "LONDON",
      "SW1P 4DF",
      "",
      "FROM: Dispatch Ltd, LS11 5QX",
      "WEIGHT: 2.5 kg",
      "TRACKING: JD000001244GB",
    ].join("\n"),
  );
});

test("express says so", () => {
  assert.equal(renderLabel(shipment({ service: "express" })).split("\n")[0], "DISPATCH EXPRESS");
});

test("long lines are cut to the printer's width", () => {
  const label = renderLabel(shipment({ to: { ...LONDON, lines: ["Flat 4, The Very Long Building Name Mansions, Westminster"] } }));
  const line = label.split("\n")[3]!;
  assert.equal(line.length, WIDTH);
  assert.ok(line.endsWith("…"));
});

test("postcodes are printed in capitals", () => {
  assert.ok(renderLabel(shipment({ to: { ...LONDON, postcode: "sw1p 4df" } })).includes("\nSW1P 4DF\n"));
});
