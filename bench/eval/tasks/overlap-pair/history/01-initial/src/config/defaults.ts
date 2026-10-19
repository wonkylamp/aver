import type { AppConfig } from "./schema.ts";

export const defaults: AppConfig = {
  server: { host: "0.0.0.0", port: 8080, trustProxy: false, allowedOrigins: [] },
  log: { level: "info", json: true, color: false },
  health: { intervalMs: 15000, checks: ["store", "queue"] },
  metrics: { enabled: true, prefix: "beacon_" },
  features: { enabled: [] },
};

// The loader writes into the config it returns, and the lists would otherwise
// be shared with every other caller.
export function cloneDefaults(): AppConfig {
  return structuredClone(defaults);
}
