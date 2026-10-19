import { test } from "node:test";
import assert from "node:assert/strict";
import { loadConfig } from "../../src/config/load.ts";
import { defaults } from "../../src/config/defaults.ts";

const file = JSON.stringify({
  server: { port: 9000, trustProxy: true, allowedOrigins: ["https://file.example"] },
  log: { level: "warn" },
  health: { intervalMs: 20000 },
});

test("a number override beats the file value", () => {
  const config = loadConfig({ text: file, env: { APP_SERVER_PORT: "9100" } });
  assert.equal(config.server.port, 9100);
});

test("an override beats the default when the file does not set the key", () => {
  const config = loadConfig({ text: "{}", env: { APP_METRICS_PREFIX: "gw_" } });
  assert.equal(config.metrics.prefix, "gw_");
});

test("overrides apply with no file at all", () => {
  const config = loadConfig({ env: { APP_SERVER_HOST: "10.0.0.5" } });
  assert.equal(config.server.host, "10.0.0.5");
  assert.equal(config.server.port, defaults.server.port);
});

test("keys the environment does not override keep the file value", () => {
  const config = loadConfig({ text: file, env: { APP_SERVER_PORT: "9100" } });
  assert.equal(config.server.trustProxy, true);
  assert.equal(config.log.level, "warn");
  assert.equal(config.health.intervalMs, 20000);
});

test("camelCase keys map to UPPER_SNAKE_CASE variables", () => {
  const config = loadConfig({
    text: file,
    env: { APP_HEALTH_INTERVAL_MS: "250", APP_SERVER_TRUST_PROXY: "false", APP_SERVER_ALLOWED_ORIGINS: "https://env.example" },
  });
  assert.equal(config.health.intervalMs, 250);
  assert.equal(config.server.trustProxy, false);
  assert.deepEqual(config.server.allowedOrigins, ["https://env.example"]);
});

test("numbers are coerced, allowing surrounding whitespace and decimals", () => {
  assert.equal(loadConfig({ env: { APP_SERVER_PORT: " 8081 " } }).server.port, 8081);
  assert.equal(loadConfig({ env: { APP_HEALTH_INTERVAL_MS: "1500.5" } }).health.intervalMs, 1500.5);
  assert.equal(typeof loadConfig({ env: { APP_SERVER_PORT: "8081" } }).server.port, "number");
});

test("booleans accept true, false, 1 and 0 in any case", () => {
  const cases: Array<[string, boolean]> = [
    ["true", true],
    ["TRUE", true],
    ["True", true],
    ["1", true],
    ["false", false],
    ["FALSE", false],
    ["0", false],
  ];
  for (const [raw, want] of cases) {
    assert.equal(loadConfig({ env: { APP_METRICS_ENABLED: raw } }).metrics.enabled, want, `APP_METRICS_ENABLED=${raw}`);
  }
});

test("lists split on commas, trim items and drop empty ones", () => {
  const config = loadConfig({ text: file, env: { APP_FEATURES_ENABLED: " beta, new-dashboard ,,dark-mode, " } });
  assert.deepEqual(config.features.enabled, ["beta", "new-dashboard", "dark-mode"]);
});

test("an empty list override is the empty list, replacing the file's list", () => {
  const config = loadConfig({ text: file, env: { APP_SERVER_ALLOWED_ORIGINS: "" } });
  assert.deepEqual(config.server.allowedOrigins, []);
});

test("a string override is used as is", () => {
  assert.equal(loadConfig({ env: { APP_LOG_LEVEL: "debug" } }).log.level, "debug");
});

test("variables that name no schema key are ignored", () => {
  const config = loadConfig({
    env: { APP_DEBUG: "1", APP_SERVER_PROT: "1", APP_LOGGING_LEVEL: "debug", SERVER_PORT: "1", APP_CONFIG_FILE: "" },
  });
  assert.deepEqual(config, defaults);
});

test("overrides never leak into the shared defaults or later loads", () => {
  loadConfig({ env: { APP_HEALTH_CHECKS: "store,queue,upstream", APP_SERVER_PORT: "1" } });
  assert.deepEqual(defaults.health.checks, ["store", "queue"]);
  assert.equal(defaults.server.port, 8080);
  assert.deepEqual(loadConfig({ env: {} }), defaults);
});
