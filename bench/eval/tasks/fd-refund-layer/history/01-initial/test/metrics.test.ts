import assert from "node:assert/strict";
import { test } from "node:test";
import { dailyRevenue, ordersIn, summarize, topProducts } from "../src/metrics.ts";
import { formatPence } from "../src/money.ts";
import type { Order } from "../src/orders.ts";
import { isFullyRefunded, orderTotal, refundedTotal } from "../src/orders.ts";
import { MARCH } from "../src/testing/march.ts";

const order = (id: string, placedOn: string, pence: number): Order => ({ id, placedOn, customer: "x", lines: [{ sku: "A", qty: 1, unitPence: pence }], refunds: [] });

test("formatPence groups thousands", () => {
  assert.equal(formatPence(123456), "£1,234.56");
  assert.equal(formatPence(5), "£0.05");
  assert.equal(formatPence(-150), "-£1.50");
});

test("order totals and refunds", () => {
  const o = MARCH.find((x) => x.id === "1004")!;
  assert.equal(orderTotal(o), 8450);
  assert.equal(refundedTotal(o), 1250);
  assert.equal(isFullyRefunded(o), false);
  assert.equal(isFullyRefunded(MARCH.find((x) => x.id === "1007")!), true);
});

test("orders are counted in the month they were placed", () => {
  assert.equal(ordersIn(MARCH, "2026-03").length, 9);
  assert.equal(ordersIn(MARCH, "2026-02").length, 1);
});

test("a month without refunds", () => {
  const orders = [order("1", "2026-05-02", 1000), order("2", "2026-05-02", 2001), order("3", "2026-05-30", 500), order("4", "2026-06-01", 9999)];
  assert.deepEqual(summarize(orders, "2026-05"), { month: "2026-05", orders: 3, grossPence: 3501, revenuePence: 3501, averageOrderPence: 1167 });
  assert.deepEqual(dailyRevenue(orders, "2026-05"), [
    { day: "2026-05-02", revenuePence: 3001 },
    { day: "2026-05-30", revenuePence: 500 },
  ]);
});

test("an empty month", () => {
  assert.deepEqual(summarize(MARCH, "2026-07"), { month: "2026-07", orders: 0, grossPence: 0, revenuePence: 0, averageOrderPence: 0 });
});

test("top products by line amount", () => {
  assert.deepEqual(topProducts(MARCH, "2026-03", 3), [
    { sku: "MUG-01", units: 6, amountPence: 10800 },
    { sku: "POT-02", units: 3, amountPence: 10350 },
    { sku: "KET-11", units: 1, amountPence: 8900 },
  ]);
});
