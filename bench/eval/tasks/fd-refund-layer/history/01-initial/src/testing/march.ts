// March 2026's orders (and their neighbours), copied from production for the tests.
import type { Order } from "../orders.ts";

export const MARCH: Order[] = [
  { id: "0998", placedOn: "2026-02-27", customer: "alice", lines: [{ sku: "MUG-01", qty: 2, unitPence: 1800 }], refunds: [{ on: "2026-03-02", pence: 1800, reason: "one arrived broken" }] },
  { id: "1001", placedOn: "2026-03-01", customer: "bob", lines: [{ sku: "TEA-04", qty: 3, unitPence: 899 }], refunds: [] },
  { id: "1002", placedOn: "2026-03-01", customer: "carol", lines: [{ sku: "MUG-01", qty: 1, unitPence: 1800 }, { sku: "TEA-04", qty: 1, unitPence: 899 }], refunds: [] },
  { id: "1003", placedOn: "2026-03-04", customer: "dan", lines: [{ sku: "POT-02", qty: 1, unitPence: 3450 }], refunds: [] },
  { id: "1004", placedOn: "2026-03-04", customer: "erin", lines: [{ sku: "MUG-01", qty: 4, unitPence: 1800 }, { sku: "COS-09", qty: 2, unitPence: 625 }], refunds: [{ on: "2026-03-09", pence: 1250, reason: "coasters in the wrong colour" }] },
  { id: "1005", placedOn: "2026-03-09", customer: "frank", lines: [{ sku: "TEA-04", qty: 2, unitPence: 899 }], refunds: [] },
  { id: "1006", placedOn: "2026-03-14", customer: "gina", lines: [{ sku: "POT-02", qty: 2, unitPence: 3450 }], refunds: [] },
  { id: "1007", placedOn: "2026-03-14", customer: "hal", lines: [{ sku: "KET-11", qty: 1, unitPence: 8900 }], refunds: [{ on: "2026-03-20", pence: 8900, reason: "returned unopened" }] },
  { id: "1008", placedOn: "2026-03-22", customer: "ivy", lines: [{ sku: "COS-09", qty: 4, unitPence: 625 }, { sku: "MUG-01", qty: 1, unitPence: 1800 }], refunds: [] },
  { id: "1009", placedOn: "2026-03-31", customer: "jon", lines: [{ sku: "TEA-04", qty: 1, unitPence: 899 }], refunds: [] },
  { id: "1010", placedOn: "2026-04-01", customer: "kim", lines: [{ sku: "MUG-01", qty: 1, unitPence: 1800 }], refunds: [] },
];
