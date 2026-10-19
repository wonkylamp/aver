import { test } from "node:test";
import assert from "node:assert/strict";
import { loadConfig } from "../src/config/load.ts";
import { createApp } from "../src/app.ts";

const now = () => new Date("2026-01-02T03:04:05.000Z");

function app(configText: string, checks = {}) {
  const lines: string[] = [];
  const config = loadConfig({ text: configText });
  return { lines, app: createApp(config, { sink: { write: (l) => lines.push(l) }, now, checks }) };
}

const get = (path: string, headers: Record<string, string> = {}) => ({ method: "GET", path, headers });

test("healthz reports the configured checks", async () => {
  const { app: a } = app('{"health": {"checks": ["store"]}}', { store: () => {}, queue: () => {} });
  const res = await a.handle(get("/healthz"));
  assert.equal(res.status, 200);
  assert.deepEqual(JSON.parse(res.body).checks, [{ name: "store", ok: true }]);
});

test("healthz is 503 when a check fails", async () => {
  const { app: a } = app("{}", {
    store: () => {},
    queue: () => {
      throw new Error("lagging");
    },
  });
  assert.equal((await a.handle(get("/healthz"))).status, 503);
});

test("metrics can be switched off", async () => {
  const { app: a } = app('{"metrics": {"enabled": false}}');
  assert.equal((await a.handle(get("/metrics"))).status, 404);
});

test("requests are counted by status class with the configured prefix", async () => {
  const { app: a } = app('{"metrics": {"prefix": "gw_"}}');
  await a.handle(get("/flags/x"));
  await a.handle(get("/nope"));
  const body = (await a.handle(get("/metrics"))).body;
  assert.match(body, /gw_requests_total\{code="2xx"\} 1/);
  assert.match(body, /gw_requests_total\{code="4xx"\} 1/);
});

test("flags come from config", async () => {
  const { app: a } = app('{"features": {"enabled": ["new-dashboard"]}}');
  const res = await a.handle(get("/flags/new-dashboard"));
  assert.deepEqual(JSON.parse(res.body), { name: "new-dashboard", enabled: true });
});

test("allowed origins get CORS headers", async () => {
  const { app: a } = app('{"server": {"allowedOrigins": ["https://ops.example"]}}');
  const res = await a.handle(get("/flags/x", { origin: "https://ops.example" }));
  assert.equal(res.headers["access-control-allow-origin"], "https://ops.example");
});

test("each request is logged once", async () => {
  const { app: a, lines } = app("{}");
  await a.handle(get("/flags/x"));
  assert.equal(lines.length, 1);
  assert.deepEqual(JSON.parse(lines[0]), {
    time: "2026-01-02T03:04:05.000Z",
    level: "info",
    msg: "request",
    method: "GET",
    path: "/flags/x",
    status: 200,
  });
});
