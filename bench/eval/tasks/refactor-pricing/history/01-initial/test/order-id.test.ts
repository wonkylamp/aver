import { test } from "node:test";
import assert from "node:assert/strict";
import { formatOrderId, parseOrderId } from "../src/orders/order-id.ts";

test("order ids carry the date and a zero-padded sequence", () => {
  assert.equal(formatOrderId(42, "2026-03-01"), "HB-20260301-000042");
});

test("order ids round-trip", () => {
  assert.deepEqual(parseOrderId(formatOrderId(7, "2026-12-31")), { date: "2026-12-31", sequence: 7 });
});

test("bad input is rejected", () => {
  assert.throws(() => formatOrderId(0, "2026-03-01"), /out of range/);
  assert.throws(() => formatOrderId(1, "01/03/2026"), /not an ISO date/);
  assert.throws(() => parseOrderId("HB-2026-1"), /not an order id/);
});
