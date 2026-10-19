import { readFileSync } from "node:fs";
import { schema, hasType } from "./schema.ts";
import type { AppConfig } from "./schema.ts";
import { cloneDefaults } from "./defaults.ts";
import { readOverrides } from "./env.ts";
import type { Env } from "./env.ts";

export type { Env } from "./env.ts";

export class ConfigError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ConfigError";
  }
}

export interface LoadOptions {
  /** Path of a JSON config file. Defaults to $APP_CONFIG_FILE; with neither (and no `text`) only the defaults apply. */
  file?: string;
  /** Config JSON passed directly instead of a file. */
  text?: string;
  /** The environment to read: APP_CONFIG_FILE and the APP_<SECTION>_<KEY> overrides. Defaults to process.env. */
  env?: Env;
}

export function loadConfig(options: LoadOptions = {}): AppConfig {
  const env = options.env ?? process.env;
  const raw = readSource(options, env);
  const config = applyEnv(cloneDefaults(), env);
  return applyFile(config, raw);
}

function readSource(options: LoadOptions, env: Env): unknown {
  if (options.text !== undefined) return parse(options.text, "<text>");
  const file = options.file ?? env.APP_CONFIG_FILE;
  if (file === undefined || file === "") return {};
  let text: string;
  try {
    text = readFileSync(file, "utf8");
  } catch (err) {
    throw new ConfigError(`cannot read config file ${file}: ${(err as Error).message}`);
  }
  return parse(text, file);
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

// An override that cannot be coerced is ignored, like a file value of the
// wrong type (docs/config.md, "Overrides are validated like file values").
function applyEnv(config: AppConfig, env: Env): AppConfig {
  const target = config as unknown as Record<string, Record<string, unknown>>;
  for (const override of readOverrides(env)) {
    if (override.value !== undefined) target[override.section][override.key] = override.value;
  }
  return config;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
