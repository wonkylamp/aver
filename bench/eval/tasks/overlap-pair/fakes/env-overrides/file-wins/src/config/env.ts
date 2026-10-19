import { schema } from "./schema.ts";
import type { FieldType } from "./schema.ts";

export type Env = Record<string, string | undefined>;

export interface Override {
  section: string;
  key: string;
  /** Dotted config path, e.g. "server.port". */
  path: string;
  /** The variable it came from, e.g. "APP_SERVER_PORT". */
  variable: string;
  type: FieldType;
  raw: string;
  /** The coerced value, or undefined when `raw` does not fit the key's type. */
  value: unknown;
}

// health.intervalMs -> APP_HEALTH_INTERVAL_MS
export function variableName(section: string, key: string): string {
  const snake = key.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toUpperCase();
  return `APP_${section.toUpperCase()}_${snake}`;
}

// Every schema key that has its variable set, in schema order. Variables that
// name no key are not looked at: the environment is shared with other tools.
export function readOverrides(env: Env): Override[] {
  const overrides: Override[] = [];
  for (const [section, fields] of Object.entries(schema)) {
    for (const [key, spec] of Object.entries(fields)) {
      const variable = variableName(section, key);
      const raw = env[variable];
      if (raw === undefined) continue;
      overrides.push({
        section,
        key,
        path: `${section}.${key}`,
        variable,
        type: spec.type,
        raw,
        value: coerce(spec.type, raw),
      });
    }
  }
  return overrides;
}

export function coerce(type: FieldType, raw: string): unknown {
  switch (type) {
    case "string":
      return raw;
    case "number": {
      if (raw.trim() === "") return undefined;
      const n = Number(raw);
      return Number.isFinite(n) ? n : undefined;
    }
    case "boolean": {
      const lower = raw.trim().toLowerCase();
      if (lower === "true" || lower === "1") return true;
      if (lower === "false" || lower === "0") return false;
      return undefined;
    }
    case "list":
      return raw
        .split(",")
        .map((item) => item.trim())
        .filter((item) => item !== "");
  }
}
