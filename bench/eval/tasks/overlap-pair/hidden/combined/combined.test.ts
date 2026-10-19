import { test } from "node:test";
import assert from "node:assert/strict";
import { loadConfig, ConfigError } from "../../src/config/load.ts";

function rejection(options: { text?: string; env: Record<string, string> }): ConfigError {
  try {
    loadConfig(options);
  } catch (err) {
    assert.ok(err instanceof ConfigError, `expected a ConfigError, got ${String(err)}`);
    return err;
  }
  assert.fail(`expected ${JSON.stringify(options)} to be rejected`);
}

const paths = (err: ConfigError) => err.problems.map((p) => p.path).sort();

test("a number override that is not a number is rejected at the key's path", () => {
  const err = rejection({ text: '{"server": {"port": 9000}}', env: { APP_SERVER_PORT: "eighty" } });
  assert.deepEqual(paths(err), ["server.port"]);
  assert.ok(err.message.includes("server.port"), err.message);
});

test("an empty number override is rejected, not read as zero", () => {
  const err = rejection({ env: { APP_HEALTH_INTERVAL_MS: "" } });
  assert.deepEqual(paths(err), ["health.intervalMs"]);
});

test("a boolean override outside true/false/1/0 is rejected", () => {
  const err = rejection({ env: { APP_SERVER_TRUST_PROXY: "yes" } });
  assert.deepEqual(paths(err), ["server.trustProxy"]);
});

test("file problems and override problems come in one error", () => {
  const err = rejection({
    text: '{"server": {"prot": 1}, "metrics": {"enabled": "on"}}',
    env: { APP_SERVER_PORT: "80a", APP_LOG_JSON: "maybe" },
  });
  assert.deepEqual(paths(err), ["log.json", "metrics.enabled", "server.port", "server.prot"]);
  for (const path of ["log.json", "metrics.enabled", "server.port", "server.prot"]) {
    assert.ok(err.message.includes(path), `message mentions ${path}: ${err.message}`);
  }
});

test("valid overrides on a valid file load with the overrides applied", () => {
  const config = loadConfig({
    text: '{"server": {"port": 9000}, "features": {"enabled": ["a"]}}',
    env: { APP_SERVER_PORT: "9100", APP_FEATURES_ENABLED: "b, c" },
  });
  assert.equal(config.server.port, 9100);
  assert.deepEqual(config.features.enabled, ["b", "c"]);
});

test("unknown APP_ variables stay ignored under strict validation", () => {
  const config = loadConfig({ text: "{}", env: { APP_SERVER_PROT: "1", APP_DEBUG: "true", APP_CONFIG_FILE: "" } });
  assert.equal(config.server.port, 8080);
});
