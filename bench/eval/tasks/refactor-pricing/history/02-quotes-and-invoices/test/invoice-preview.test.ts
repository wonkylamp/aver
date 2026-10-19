import { test } from "node:test";
import assert from "node:assert/strict";
import { previewInvoice } from "../src/admin/invoice-preview.ts";
import { toLine } from "../src/catalog/catalog.ts";

const order = {
  id: "HB-20260301-000042",
  region: "US-WA",
  customer: { id: "c-trade", member: true },
  lines: [toLine("VARNISH-1L", 4), toLine("BRUSH-50", 12)],
};

test("the preview prices the order like checkout", () => {
  const preview = previewInvoice(order);
  assert.deepEqual(preview.rows, [
    { sku: "VARNISH-1L", qty: 4, discountPct: 3, totalCents: 10860 },
    { sku: "BRUSH-50", qty: 12, discountPct: 8, totalCents: 4968 },
  ]);
  assert.equal(preview.subtotalCents, 15828);
  assert.equal(preview.shippingCents, 0);
  assert.equal(preview.taxCents, 1029);
  assert.equal(preview.totalCents, 16857);
});

test("the text shows every row and the totals", () => {
  const lines = previewInvoice(order).text.split("\n");
  assert.equal(lines[0], "Invoice preview for order HB-20260301-000042 (customer c-trade)");
  assert.equal(lines[2], "VARNISH-1L      x4   -3%       $108.60");
  assert.equal(lines[3], "BRUSH-50       x12   -8%        $49.68");
  assert.equal(lines.at(-1), "Total                          $168.57");
});

test("a line with no discount shows no percentage", () => {
  const preview = previewInvoice({ ...order, region: "US-OR", customer: { id: "c-guest", member: false }, lines: [toLine("FENDER-M", 1)] });
  assert.equal(preview.text.split("\n")[2], "FENDER-M        x1              $34.50");
  assert.equal(preview.shippingCents, 599);
});
