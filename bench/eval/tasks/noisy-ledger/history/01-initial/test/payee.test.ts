import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { displayPayee, normalizePayee } from "../src/import/payee.ts";

describe("normalizePayee", () => {
  const cases: Array<[string, string]> = [
    ["TESCO STORES 2041", "tesco stores 2041"],
    ["Tesco Stores  2041", "tesco stores 2041"],
    ["  tesco\tstores 2041 ", "tesco stores 2041"],
    ["Pret A Manger", "pret a manger"],
    ["", ""],
  ];
  for (const [raw, want] of cases) test(JSON.stringify(raw), () => assert.equal(normalizePayee(raw), want));
});

describe("displayPayee", () => {
  const cases: Array<[string, string]> = [
    ["TESCO STORES 2041", "Tesco Stores"],
    ["CARD PAYMENT TO TESCO STORES 2041", "Tesco Stores"],
    ["CONTACTLESS PRET A MANGER LONDON GB", "Pret A Manger"],
    ["POS COSTA COFFEE 44120", "Costa Coffee"],
    ["DD THAMES WATER", "Thames Water"],
    ["SO RENT FLAT 2", "Rent Flat 2"],
    ["TFL TRAVEL CH", "Tfl Travel CH"],
    ["uber   trip", "Uber Trip"],
    ["Netflix.com", "Netflix.com"],
    ["SAINSBURYS S/MKTS MANCHESTER", "Sainsburys S/mkts"],
    ["THE CO OP", "The Co OP"],
    ["AMAZON MKTPLACE PMTS", "Amazon Mktplace Pmts"],
    ["VISA SPOTIFY", "Spotify"],
    ["WAITROSE 1234 LEEDS", "Waitrose"],
    ["BOLT.EU/O/2603", "Bolt.eu/o/2603"],
  ];
  for (const [raw, want] of cases) test(JSON.stringify(raw), () => assert.equal(displayPayee(raw), want));
});
