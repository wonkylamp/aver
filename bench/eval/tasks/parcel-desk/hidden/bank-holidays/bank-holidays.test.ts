// #43 on its own: business days skip bank holidays.
import assert from "node:assert/strict";
import { test } from "node:test";
import { addBusinessDays } from "../../src/dates.ts";
import { quote } from "../../src/quote.ts";
import { estimateDelivery } from "../../src/services.ts";
import { BOX, LONDON } from "../../src/testing/samples.ts";

test("Easter: Good Friday and Easter Monday are skipped", () => {
  assert.equal(addBusinessDays("2026-04-02", 1), "2026-04-07");
  assert.equal(addBusinessDays("2026-04-02", 2), "2026-04-08");
  assert.equal(estimateDelivery("2026-04-02", "express"), "2026-04-07");
});

test("Christmas and the substitute Boxing Day", () => {
  assert.equal(addBusinessDays("2026-12-24", 1), "2026-12-29");
  assert.equal(estimateDelivery("2026-12-23", "economy"), "2026-12-30");
});

test("a holiday it starts on is not counted", () => {
  assert.equal(addBusinessDays("2026-05-04", 1), "2026-05-05");
});

test("holidays can be given, and none means weekends only", () => {
  assert.equal(addBusinessDays("2026-04-02", 1, []), "2026-04-03");
  assert.equal(addBusinessDays("2026-03-02", 1, ["2026-03-03"]), "2026-03-04");
});

test("quotes promise the right day", () => {
  assert.equal(quote(BOX, LONDON, "express", "2026-04-02").deliveryBy, "2026-04-07");
  assert.equal(quote(BOX, LONDON, "economy", "2026-08-27").deliveryBy, "2026-09-02");
});
