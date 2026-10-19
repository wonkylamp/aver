import { test } from "node:test";
import assert from "node:assert/strict";
import { Router, json, text } from "../src/http/router.ts";

const get = (path: string) => ({ method: "GET", path, headers: {} });

test("routes by method and path", async () => {
  const router = new Router().add("GET", "/ping", () => text(200, "pong"));
  const res = await router.handle(get("/ping"));
  assert.equal(res.status, 200);
  assert.equal(res.body, "pong");
});

test("path parameters are decoded", async () => {
  const router = new Router().add("GET", "/flags/:name", (_req, params) => json(200, params));
  const res = await router.handle(get("/flags/new%20ui"));
  assert.deepEqual(JSON.parse(res.body), { name: "new ui" });
});

test("the query string is not part of the path", async () => {
  const router = new Router().add("GET", "/ping", () => text(200, "pong"));
  assert.equal((await router.handle(get("/ping?verbose=1"))).status, 200);
});

test("unknown paths are 404 and wrong methods are 405", async () => {
  const router = new Router().add("GET", "/ping", () => text(200, "pong"));
  assert.equal((await router.handle(get("/nope"))).status, 404);
  assert.equal((await router.handle({ method: "POST", path: "/ping", headers: {} })).status, 405);
});

test("a malformed escape does not match", async () => {
  const router = new Router().add("GET", "/flags/:name", () => text(200, "ok"));
  assert.equal((await router.handle(get("/flags/%E0%A4%A"))).status, 404);
});
