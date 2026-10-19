import type { Chart } from "./accounts.ts";

/** One side of an entry: positive cents are debits, negative are credits. */
export interface Posting {
  account: string;
  cents: number;
  memo?: string;
}

export interface Entry {
  id: string;
  date: Date;
  payee: string;
  postings: Posting[];
  reconciled: boolean;
  postedAt: Date;
  reference?: string;
}

export class JournalError extends Error {
  override name = "JournalError";
}

export class Journal {
  readonly #entries: Entry[] = [];
  readonly #chart: Chart;
  readonly #clock: () => Date;
  #seq = 0;

  constructor(chart: Chart, clock: () => Date = () => new Date()) {
    this.#chart = chart;
    this.#clock = clock;
  }

  post(input: { date: Date; payee: string; postings: Posting[]; reference?: string }): Entry {
    if (input.postings.length < 2) throw new JournalError("an entry needs at least two postings");
    let total = 0;
    for (const p of input.postings) {
      if (!Number.isInteger(p.cents) || p.cents === 0) throw new JournalError(`posting amounts are non-zero whole cents: ${p.cents}`);
      if (!this.#chart.has(p.account)) throw new JournalError(`no account ${p.account}`);
      total += p.cents;
    }
    if (total !== 0) throw new JournalError(`entry does not balance: off by ${total} cents`);
    const entry: Entry = {
      id: `je_${String(++this.#seq).padStart(5, "0")}`,
      date: input.date,
      payee: input.payee,
      postings: input.postings.map((p) => ({ ...p })),
      reconciled: false,
      postedAt: this.#clock(),
      reference: input.reference,
    };
    this.#entries.push(entry);
    return entry;
  }

  reconcile(id: string): void {
    const e = this.#entries.find((x) => x.id === id);
    if (!e) throw new JournalError(`no entry ${id}`);
    e.reconciled = true;
  }

  entries(filter: { from?: Date; to?: Date; account?: string } = {}): Entry[] {
    return this.#entries
      .filter((e) => (filter.from === undefined || e.date >= filter.from) && (filter.to === undefined || e.date <= filter.to))
      .filter((e) => filter.account === undefined || e.postings.some((p) => p.account === filter.account))
      .sort((a, b) => a.date.getTime() - b.date.getTime() || a.id.localeCompare(b.id));
  }
}
