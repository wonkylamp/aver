import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { loadConfig, ConfigError } from "../../src/config/load.ts";

const example = fileURLToPath(new URL("../../config/example.json", import.meta.url));

function rejection(text: string): ConfigError {
  try {
    loadConfig({ text, env: {} });
  } catch (err) {
    assert.ok(err instanceof ConfigError, `expected a ConfigError, got ${String(err)}`);
    return err;
  }
  assert.fail(`expected ${text} to be rejected`);
}

function paths(text: string): string[] {
  const err = rejection(text);
  assert.ok(Array.isArray(err.problems), "ConfigError.problems is a list");
  return err.problems.map((p) => p.path).sort();
}

test("a valid config still loads", () => {
  const config = loadConfig({
    text: JSON.stringify({
      server: { host: "127.0.0.1", port: 9000, trustProxy: true, allowedOrigins: [] },
      log: { level: "debug", json: false },
      health: { intervalMs: 1000, checks: ["store"] },
      metrics: { enabled: false, prefix: "x_" },
      features: { enabled: ["a"] },
    }),
    env: {},
  });
  assert.equal(config.server.port, 9000);
  assert.equal(config.log.json, false);
  assert.deepEqual(config.health.checks, ["store"]);
});

test("the shipped example config is valid", () => {
  assert.equal(loadConfig({ file: example, env: {} }).server.port, 8443);
});

test("an unknown key is rejected with its path", () => {
  assert.deepEqual(paths('{"server": {"prot": 9000}}'), ["server.prot"]);
});

test("an unknown section is rejected with its name", () => {
  assert.deepEqual(paths('{"databse": {"url": "x"}}'), ["databse"]);
});

test("a number given as a string is the wrong type", () => {
  assert.deepEqual(paths('{"server": {"port": "8080"}}'), ["server.port"]);
});

test("a boolean given as a string is the wrong type", () => {
  assert.deepEqual(paths('{"metrics": {"enabled": "true"}}'), ["metrics.enabled"]);
});

test("a list with a non-string item, or a bare string, is the wrong type", () => {
  assert.deepEqual(paths('{"health": {"checks": ["store", 7]}, "features": {"enabled": "beta"}}'), [
    "features.enabled",
    "health.checks",
  ]);
});

test("null is the wrong type", () => {
  assert.deepEqual(paths('{"log": {"level": null}}'), ["log.level"]);
});

test("a section that is not an object is rejected", () => {
  assert.deepEqual(paths('{"metrics": true, "log": ["info"]}'), ["log", "metrics"]);
});

test("every problem is reported, not just the first", () => {
  assert.deepEqual(
    paths('{"server": {"port": "80", "prot": 1, "host": "h"}, "log": {"json": "no"}, "cache": {}}'),
    ["cache", "log.json", "server.port", "server.prot"],
  );
});

test("the message names every bad path and each problem has a message", () => {
  const err = rejection('{"server": {"port": "80", "prot": 1}, "log": {"json": "no"}}');
  for (const path of ["server.port", "server.prot", "log.json"]) {
    assert.ok(err.message.includes(path), `message mentions ${path}: ${err.message}`);
  }
  for (const problem of err.problems) {
    assert.equal(typeof problem.message, "string");
    assert.ok(problem.message.length > 0);
  }
  assert.equal(err.name, "ConfigError");
  assert.ok(err instanceof Error);
});

test("invalid JSON is still a ConfigError, with no content problems", () => {
  const err = rejection('{"server": ');
  assert.deepEqual(err.problems, []);
});
