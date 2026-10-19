// Expectations follow the date format agreed for #230 (MM/DD'YY, which
// Quicken reads for every year); the code still writes MM/DD/YYYY.
import assert from "node:assert/strict";
import { describe, test } from "node:test";
import type { Account } from "../src/ledger.ts";
import { qifDate, toQif } from "../src/qif.ts";
import { tx } from "../src/testing/tx.ts";

const current: Account = { id: "cur", name: "Current", kind: "current", currency: "GBP", openingBalance: 0 };

describe("QIF", () => {
  test("a dated record", () => {
    const out = toQif(current, [tx("2026-03-14", -123456, "Tesco Stores", { memo: "big shop", category: "groceries" })]);
    assert.equal(out, "!Type:Bank\nD03/14'26\nT-1234.56\nPTesco Stores\nMbig shop\nLgroceries\n^\n");
  });
  test("dates in the 2000s", () => assert.equal(qifDate("2026-03-14"), "03/14'26"));
  test("dates before 2000", () => assert.equal(qifDate("1999-12-31"), "12/31/99"));
  test("account types", () => {
    assert.ok(toQif({ ...current, kind: "credit-card" }, []).startsWith("!Type:CCard"));
    assert.ok(toQif({ ...current, kind: "loan" }, []).startsWith("!Type:Oth L"));
    assert.ok(toQif({ ...current, kind: "cash" }, []).startsWith("!Type:Cash"));
  });
  test("amounts have no thousands separators", () => assert.match(toQif(current, [tx("2026-03-14", 1000000, "x")]), /\nT10000.00\n/));
  test("an empty account is just the header", () => assert.equal(toQif(current, []), "!Type:Bank\n"));
  test("no memo and no category lines when absent", () => assert.doesNotMatch(toQif(current, [tx("2026-03-14", -1, "x")]), /\n[ML]/));
});
