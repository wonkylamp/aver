import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { detectFormat, FormatError, readStatement } from "../src/import/formats.ts";
import { BARCLAYS, MONZO, REVOLUT, STARLING } from "../src/testing/statements.ts";

describe("detectFormat", () => {
  const cases: Array<[string, string]> = [
    [MONZO, "monzo"],
    [BARCLAYS, "barclays"],
    [REVOLUT, "revolut"],
    [STARLING, "starling"],
    [`\uFEFF${MONZO}`, "monzo"],
  ];
  for (const [text, want] of cases) test(want, () => assert.equal(detectFormat(text), want));
  test("unknown", () => assert.throws(() => detectFormat("foo,bar\n"), FormatError));
});

describe("readStatement", () => {
  test("Monzo rows carry the bank id", () => {
    const rows = readStatement(MONZO);
    assert.deepEqual(rows[0], { bankId: "tx_0000A1", date: "2026-03-01", payee: "Pret A Manger", amount: -420, memo: undefined, state: "booked" });
    assert.equal(rows[1]!.memo, "March");
  });
  test("Barclays takes the payee from the memo and has no id", () => {
    const rows = readStatement(BARCLAYS);
    assert.deepEqual(rows.map((r) => [r.date, r.payee, r.amount, r.bankId]), [
      ["2026-03-03", "TESCO STORES 2041", -5610, undefined],
      ["2026-03-04", "RENT FLAT 2", -120000, undefined],
    ]);
  });
  test("Revolut states and fees", () => {
    const rows = readStatement(REVOLUT);
    assert.deepEqual(rows.map((r) => r.state), ["booked", "pending", "reverted", "booked"]);
    assert.equal(rows[3]!.amount, 4950);
    assert.equal(rows[0]!.date, "2026-03-05");
  });
  test("Starling keeps the reference as the memo", () => {
    assert.deepEqual(readStatement(STARLING)[0], { date: "2026-03-09", payee: "Octopus Energy", memo: "A-1234", amount: -8800, state: "booked" });
  });
  test("a missing column names the line", () => {
    assert.throws(() => readStatement("Transaction ID,Date,Amount\ntx,01/03/2026,1.00\n", "monzo"), /line 2: no Name column/);
  });
});
