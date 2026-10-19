import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { loadConfig, ConfigError } from "../src/config/load.ts";
import { defaults } from "../src/config/defaults.ts";

const example = fileURLToPath(new URL("../config/example.json", import.meta.url));
const missing = fileURLToPath(new URL("../config/does-not-exist.json", import.meta.url));

test("no file gives the defaults", () => {
  assert.deepEqual(loadConfig({ env: {} }), defaults);
});

test("file values replace defaults key by key", () => {
  const config = loadConfig({ text: '{"server": {"port": 9000}, "log": {"level": "debug"}}' });
  assert.equal(config.server.port, 9000);
  assert.equal(config.server.host, "0.0.0.0");
  assert.equal(config.log.level, "debug");
  assert.equal(config.log.json, true);
});

test("a list in the file replaces the default list", () => {
  const config = loadConfig({ text: '{"health": {"checks": ["store"]}}' });
  assert.deepEqual(config.health.checks, ["store"]);
});

test("the example config loads", () => {
  const config = loadConfig({ file: example, env: {} });
  assert.equal(config.server.port, 8443);
  assert.equal(config.server.trustProxy, true);
  assert.deepEqual(config.server.allowedOrigins, ["https://status.example.org"]);
  assert.deepEqual(config.health.checks, ["store", "queue", "upstream"]);
  assert.deepEqual(config.features.enabled, ["new-dashboard"]);
});

test("APP_CONFIG_FILE names the file when no path is given", () => {
  const config = loadConfig({ env: { APP_CONFIG_FILE: example } });
  assert.equal(config.server.port, 8443);
});

test("an explicit file wins over APP_CONFIG_FILE", () => {
  assert.throws(() => loadConfig({ file: missing, env: { APP_CONFIG_FILE: example } }), ConfigError);
});

test("invalid JSON is a ConfigError naming the source", () => {
  assert.throws(
    () => loadConfig({ text: '{"server": ' }),
    (err: unknown) => err instanceof ConfigError && err.message.includes("<text>"),
  );
});

test("a missing file is a ConfigError naming the file", () => {
  assert.throws(
    () => loadConfig({ file: missing }),
    (err: unknown) => err instanceof ConfigError && err.message.includes("does-not-exist.json"),
  );
});

test("loading never changes the shared defaults", () => {
  const config = loadConfig({ text: '{"features": {"enabled": ["beta"]}}' });
  config.health.checks.push("extra");
  assert.deepEqual(defaults.features.enabled, []);
  assert.deepEqual(defaults.health.checks, ["store", "queue"]);
  assert.deepEqual(loadConfig({ env: {} }).health.checks, ["store", "queue"]);
});

test("APP_<SECTION>_<KEY> overrides the file value", () => {
  const config = loadConfig({ text: '{"server": {"port": 9000}}', env: { APP_SERVER_PORT: "9100" } });
  assert.equal(config.server.port, 9100);
});

test("camelCase keys use UPPER_SNAKE_CASE variables", () => {
  const config = loadConfig({
    env: { APP_HEALTH_INTERVAL_MS: "500", APP_SERVER_TRUST_PROXY: "TRUE", APP_SERVER_ALLOWED_ORIGINS: "https://a.example, https://b.example" },
  });
  assert.equal(config.health.intervalMs, 500);
  assert.equal(config.server.trustProxy, true);
  assert.deepEqual(config.server.allowedOrigins, ["https://a.example", "https://b.example"]);
});

test("list overrides drop empty items", () => {
  assert.deepEqual(loadConfig({ env: { APP_FEATURES_ENABLED: "a,, b ," } }).features.enabled, ["a", "b"]);
  assert.deepEqual(loadConfig({ env: { APP_HEALTH_CHECKS: "" } }).health.checks, []);
});

test("variables that name no key are ignored", () => {
  assert.deepEqual(loadConfig({ env: { APP_DEBUG: "1", APP_SERVER_PROT: "1" } }), defaults);
});
