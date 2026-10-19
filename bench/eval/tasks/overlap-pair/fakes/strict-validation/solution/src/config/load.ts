import { readFileSync } from "node:fs";
import type { AppConfig } from "./schema.ts";
import { cloneDefaults } from "./defaults.ts";
import { validate, formatProblems } from "./validate.ts";
import type { ConfigProblem } from "./validate.ts";

export type { ConfigProblem } from "./validate.ts";

export class ConfigError extends Error {
  /** Every bad path in the config. Empty for errors that are not about its content (unreadable file, invalid JSON). */
  readonly problems: ConfigProblem[];

  constructor(message: string, problems: ConfigProblem[] = []) {
    super(message);
    this.name = "ConfigError";
    this.problems = problems;
  }
}

export type Env = Record<string, string | undefined>;

export interface LoadOptions {
  /** Path of a JSON config file. Defaults to $APP_CONFIG_FILE; with neither (and no `text`) only the defaults apply. */
  file?: string;
  /** Config JSON passed directly instead of a file. */
  text?: string;
  /** The environment to read. Defaults to process.env. */
  env?: Env;
}

// Throws a ConfigError listing every problem when the file does not match the
// schema; nothing is silently dropped.
export function loadConfig(options: LoadOptions = {}): AppConfig {
  const env = options.env ?? process.env;
  const raw = readSource(options, env);
  const problems = validate(raw);
  if (problems.length > 0) throw new ConfigError(formatProblems(problems), problems);
  return applyFile(cloneDefaults(), raw as Record<string, Record<string, unknown>>);
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

// `raw` has passed validate(), so every section and key in it is known and
// has the right type.
function applyFile(config: AppConfig, raw: Record<string, Record<string, unknown>>): AppConfig {
  const target = config as unknown as Record<string, Record<string, unknown>>;
  for (const [section, given] of Object.entries(raw)) {
    for (const [key, value] of Object.entries(given)) target[section][key] = value;
  }
  return config;
}
