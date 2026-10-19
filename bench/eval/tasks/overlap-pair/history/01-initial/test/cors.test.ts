import { test } from "node:test";
import assert from "node:assert/strict";
import { withCors } from "../src/http/cors.ts";
import { text } from "../src/http/router.ts";

const req = (origin?: string) => ({
  method: "GET",
  path: "/",
  headers: origin === undefined ? {} : { origin },
});

test("an allowed origin gets the CORS headers", () => {
  const res = withCors(["https://a.example"], req("https://a.example"), text(200, "ok"));
  assert.equal(res.headers["access-control-allow-origin"], "https://a.example");
  assert.equal(res.headers.vary, "Origin");
});

test("other origins and requests without one are left alone", () => {
  assert.equal(withCors(["https://a.example"], req("https://b.example"), text(200, "ok")).headers["access-control-allow-origin"], undefined);
  assert.equal(withCors(["*"], req(), text(200, "ok")).headers["access-control-allow-origin"], undefined);
});

test("a wildcard echoes the caller's origin", () => {
  const res = withCors(["*"], req("https://c.example"), text(200, "ok"));
  assert.equal(res.headers["access-control-allow-origin"], "https://c.example");
});
