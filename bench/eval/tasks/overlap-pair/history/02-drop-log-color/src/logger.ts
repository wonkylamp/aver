export type Level = "debug" | "info" | "warn" | "error";

const ORDER: Record<Level, number> = { debug: 10, info: 20, warn: 30, error: 40 };

export interface LogSink {
  write(line: string): void;
}

export interface LoggerOptions {
  level: string;
  json: boolean;
  sink: LogSink;
  now?: () => Date;
}

export type Fields = Record<string, unknown>;

export interface Logger {
  debug(msg: string, fields?: Fields): void;
  info(msg: string, fields?: Fields): void;
  warn(msg: string, fields?: Fields): void;
  error(msg: string, fields?: Fields): void;
  child(fields: Fields): Logger;
}

export function parseLevel(text: string): Level {
  const lower = text.trim().toLowerCase();
  if (Object.hasOwn(ORDER, lower)) return lower as Level;
  throw new Error(`unknown log level: ${text}`);
}

export function createLogger(options: LoggerOptions, bound: Fields = {}): Logger {
  const min = ORDER[parseLevel(options.level)];
  const now = options.now ?? (() => new Date());

  const emit = (level: Level, msg: string, fields: Fields = {}): void => {
    if (ORDER[level] < min) return;
    const all = { ...bound, ...fields };
    const time = now().toISOString();
    if (options.json) {
      options.sink.write(JSON.stringify({ time, level, msg, ...all }));
      return;
    }
    const tag = level.toUpperCase().padEnd(5);
    const extra = Object.entries(all)
      .map(([key, value]) => `${key}=${formatValue(value)}`)
      .join(" ");
    options.sink.write(`${time} ${tag} ${msg}${extra ? ` ${extra}` : ""}`);
  };

  return {
    debug: (msg, fields) => emit("debug", msg, fields),
    info: (msg, fields) => emit("info", msg, fields),
    warn: (msg, fields) => emit("warn", msg, fields),
    error: (msg, fields) => emit("error", msg, fields),
    child: (fields) => createLogger(options, { ...bound, ...fields }),
  };
}

function formatValue(value: unknown): string {
  if (typeof value === "string" && value !== "" && !/[\s"=]/.test(value)) return value;
  return JSON.stringify(value);
}
