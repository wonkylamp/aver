import { readFileSync } from "node:fs";
import type { AppConfig } from "./schema.ts";
import { cloneDefaults } from "./defaults.ts";
import { validate, formatProblems, typeName } from "./validate.ts";
import type { ConfigProblem } from "./validate.ts";
import { readOverrides } from "./env.ts";
import type { Env, Override } from "./env.ts";

export type { ConfigProblem } from "./validate.ts";
export type { Env } from "./env.ts";

export class ConfigError extends Error {
  /** Every bad path in the config. Empty for errors that are not about its content (unreadable file, invalid JSON). */
  readonly problems: ConfigProblem[];

  constructor(message: string, problems: ConfigProblem[] = []) {
    super(message);
    this.name = "ConfigError";
    this.problems = problems;
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

// Defaults, then the file, then the environment overrides. The file and the
// overrides are validated together, so one ConfigError lists every bad path.
export function loadConfig(options: LoadOptions = {}): AppConfig {
  const env = options.env ?? process.env;
  const raw = readSource(options, env);
  const overrides = readOverrides(env);
  const problems = [...validate(raw), ...overrideProblems(overrides)];
  if (problems.length > 0) throw new ConfigError(formatProblems(problems), problems);
  const config = applyFile(cloneDefaults(), raw as Record<string, Record<string, unknown>>);
  return applyEnv(config, overrides);
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

function overrideProblems(overrides: Override[]): ConfigProblem[] {
  return overrides
    .filter((o) => o.value === undefined)
    .map((o) => ({ path: o.path, message: `${o.variable}=${JSON.stringify(o.raw)} is not ${typeName(o.type)}` }));
}

// `raw` has passed validate(), so every section and key in it is known and
// has the right type.
function applyFile(config: AppConfig, raw: Record<string, Record<string, unknown>>): AppConfig {
  const target = config as unknown as Record<string, Record<string, unknown>>;
  for (const [section, given] of Object.entries(raw)) {
    for (const [key, value] of Object.entries(given)) target[section][key] = value;
  }
  return config;
}

function applyEnv(config: AppConfig, overrides: Override[]): AppConfig {
  const target = config as unknown as Record<string, Record<string, unknown>>;
  for (const override of overrides) target[override.section][override.key] = override.value;
  return config;
}
