// The config schema: every section, every key and the type of its value.
// docs/config.md is the operator-facing copy of this table; keep them in step.

export type FieldType = "string" | "number" | "boolean" | "list";

export interface FieldSpec {
  readonly type: FieldType;
  readonly description: string;
}

export interface AppConfig {
  server: { host: string; port: number; trustProxy: boolean; allowedOrigins: string[] };
  log: { level: string; json: boolean; color: boolean };
  health: { intervalMs: number; checks: string[] };
  metrics: { enabled: boolean; prefix: string };
  features: { enabled: string[] };
}

export type SectionName = keyof AppConfig;

export const schema: Record<SectionName, Record<string, FieldSpec>> = {
  server: {
    host: { type: "string", description: "Interface to bind" },
    port: { type: "number", description: "TCP port to listen on" },
    trustProxy: { type: "boolean", description: "Take the client address from X-Forwarded-For" },
    allowedOrigins: { type: "list", description: "Origins allowed to call the API from a browser" },
  },
  log: {
    level: { type: "string", description: "Lowest level written: debug, info, warn or error" },
    json: { type: "boolean", description: "One JSON object per line instead of plain text" },
    color: { type: "boolean", description: "Colour the level in plain-text logs" },
  },
  health: {
    intervalMs: { type: "number", description: "Milliseconds between background health runs" },
    checks: { type: "list", description: "Health checks to run, by name" },
  },
  metrics: {
    enabled: { type: "boolean", description: "Serve /metrics" },
    prefix: { type: "string", description: "Prefix for every metric name" },
  },
  features: {
    enabled: { type: "list", description: "Feature flags switched on" },
  },
};

export function hasType(type: FieldType, value: unknown): boolean {
  switch (type) {
    case "string":
      return typeof value === "string";
    case "number":
      return typeof value === "number" && Number.isFinite(value);
    case "boolean":
      return typeof value === "boolean";
    case "list":
      return Array.isArray(value) && value.every((item) => typeof item === "string");
  }
}
