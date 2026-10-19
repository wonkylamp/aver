import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { CsvError, parseCsv, parseCsvObjects } from "../src/csv.ts";

describe("parseCsv", () => {
  const cases: Array<[string, string, string[][]]> = [
    ["simple", "a,b\n1,2\n", [["a", "b"], ["1", "2"]]],
    ["CRLF", "a,b\r\n1,2\r\n", [["a", "b"], ["1", "2"]]],
    ["no final newline", "a,b\n1,2", [["a", "b"], ["1", "2"]]],
    ["quoted comma", 'a\n"x, y"\n', [["a"], ["x, y"]]],
    ["doubled quote", 'a\n"say ""hi"""\n', [["a"], ['say "hi"']]],
    ["quoted newline", 'a\n"one\ntwo"\n', [["a"], ["one\ntwo"]]],
    ["empty fields", "a,b,c\n,,\n", [["a", "b", "c"], ["", "", ""]]],
    ["byte-order mark", "\uFEFFa,b\n1,2\n", [["a", "b"], ["1", "2"]]],
    ["blank lines skipped", "a\n\n1\n\n", [["a"], ["1"]]],
    ["unicode", "payee\nCafé Olé\n", [["payee"], ["Café Olé"]]],
    ["quoted empty", 'a,b\n"",x\n', [["a", "b"], ["", "x"]]],
    ["spaces kept", "a\n  x  \n", [["a"], ["  x  "]]],
  ];
  for (const [name, text, want] of cases) test(name, () => assert.deepEqual(parseCsv(text), want));
  test("semicolons", () => assert.deepEqual(parseCsv("a;b\n1;2\n", ";"), [["a", "b"], ["1", "2"]]));
  test("tabs", () => assert.deepEqual(parseCsv("a\tb\n1\t2\n", "\t"), [["a", "b"], ["1", "2"]]));
  test("unterminated quote", () => assert.throws(() => parseCsv('a\n"oops\n'), CsvError));
  test("stray quote", () => assert.throws(() => parseCsv('a\nx"y\n'), CsvError));
});

describe("parseCsvObjects", () => {
  test("keys from the header, trimmed", () => {
    assert.deepEqual(parseCsvObjects(" Date , Amount\n2026-03-01,4.20\n"), [{ Date: "2026-03-01", Amount: "4.20" }]);
  });
  test("an empty file has no rows", () => assert.deepEqual(parseCsvObjects(""), []));
  test("a short row is an error", () => assert.throws(() => parseCsvObjects("a,b\n1\n"), CsvError));
});
