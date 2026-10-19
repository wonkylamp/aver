// The web app parses a statement twice, for the preview and then for the
// import; this cache keeps the parsed rows in between.
import { createHash } from "node:crypto";
import type { BankFormat, StatementRow } from "./formats.ts";
import { readStatement } from "./formats.ts";

interface Entry {
  rows: StatementRow[];
  at: number;
}

export class StatementCache {
  readonly #entries = new Map<string, Entry>();
  readonly #ttlMs: number;
  readonly #max: number;
  hits = 0;
  misses = 0;

  constructor(options: { ttlMs?: number; max?: number } = {}) {
    this.#ttlMs = options.ttlMs ?? 60_000;
    this.#max = options.max ?? 20;
  }

  static key(text: string, format?: BankFormat, currency?: string): string {
    return createHash("sha256").update(`${format ?? "auto"}\0${currency ?? ""}\0${text}`).digest("hex");
  }

  has(text: string, format?: BankFormat, currency?: string): boolean {
    const e = this.#entries.get(StatementCache.key(text, format, currency));
    return e !== undefined && Date.now() - e.at <= this.#ttlMs;
  }

  read(text: string, format?: BankFormat, currency?: string): StatementRow[] {
    const key = StatementCache.key(text, format, currency);
    const e = this.#entries.get(key);
    if (e && Date.now() - e.at <= this.#ttlMs) {
      this.hits += 1;
      return e.rows.map((r) => ({ ...r }));
    }
    this.misses += 1;
    const rows = readStatement(text, format, currency);
    this.#entries.delete(key);
    this.#entries.set(key, { rows, at: Date.now() });
    while (this.#entries.size > this.#max) this.#entries.delete(this.#entries.keys().next().value!);
    return rows.map((r) => ({ ...r }));
  }

  get size(): number {
    return this.#entries.size;
  }
}
