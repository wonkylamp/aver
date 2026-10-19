import { readFileSync } from "node:fs";
import { schema, hasType } from "./schema.ts";
import type { AppConfig } from "./schema.ts";
import { cloneDefaults } from "./defaults.ts";

export class ConfigError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ConfigError";
  }
}

export interface LoadOptions {
  /** Path of a JSON config file. Without it (and without `text`) only the defaults apply. */
  file?: string;
  /** Config JSON passed directly instead of a file. */
  text?: string;
}

export function loadConfig(options: LoadOptions = {}): AppConfig {
  const raw = readSource(options);
  return applyFile(cloneDefaults(), raw);
}

function readSource(options: LoadOptions): unknown {
  if (options.text !== undefined) return parse(options.text, "<text>");
  if (options.file === undefined) return {};
  let text: string;
  try {
    text = readFileSync(options.file, "utf8");
  } catch (err) {
    throw new ConfigError(`cannot read config file ${options.file}: ${(err as Error).message}`);
  }
  return parse(text, options.file);
}

function parse(text: string, source: string): unknown {
  try {
    return JSON.parse(text);
  } catch (err) {
    throw new ConfigError(`invalid JSON in ${source}: ${(err as Error).message}`);
  }
}

// Copies every known key whose value has the right type. Anything else in the
// file is ignored, so a typo or a stale key leaves the default in place.
function applyFile(config: AppConfig, raw: unknown): AppConfig {
  if (!isRecord(raw)) return config;
  const target = config as unknown as Record<string, Record<string, unknown>>;
  for (const [section, fields] of Object.entries(schema)) {
    const given = raw[section];
    if (!isRecord(given)) continue;
    for (const [key, spec] of Object.entries(fields)) {
      const value = given[key];
      if (value !== undefined && hasType(spec.type, value)) target[section][key] = value;
    }
  }
  return config;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
