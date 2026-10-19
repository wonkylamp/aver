import assert from "node:assert/strict";
import { test } from "node:test";
import { exportVendors } from "../src/export/vendor-export.ts";
import type { Vendor } from "../src/vendors.ts";
import { dueDate, validateVendor, VendorError } from "../src/vendors.ts";

const base: Vendor = { id: "v1", name: "Acme Ltd", paymentTermsDays: 30, active: true, balanceCents: 0 };

test("vendors need a name, a sane VAT number and terms", () => {
  validateVendor(base);
  assert.throws(() => validateVendor({ ...base, name: " " }), VendorError);
  assert.throws(() => validateVendor({ ...base, vatNumber: "123" }), VendorError);
  assert.throws(() => validateVendor({ ...base, paymentTermsDays: 365 }), VendorError);
  validateVendor({ ...base, vatNumber: "GB123456789" });
});

test("due dates follow the payment terms", () => {
  assert.deepEqual(dueDate(new Date("2026-01-31T00:00:00Z"), base), new Date("2026-03-02T00:00:00Z"));
});

test("the vendor export is sorted by name", () => {
  const csv = exportVendors([{ ...base, id: "v2", name: "Zeta GmbH" }, base, { ...base, id: "v3", name: "beta srl" }]);
  const names = csv.trim().split(/\r?\n/).slice(1).map((l) => l.split(",")[0]);
  assert.deepEqual(names, ["Acme Ltd", "beta srl", "Zeta GmbH"]);
});
