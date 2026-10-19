export type Labels = Record<string, string>;

interface Series {
  kind: "counter" | "gauge";
  values: Map<string, number>;
}

const NAME = /^[a-zA-Z_:][a-zA-Z0-9_:]*$/;

export class Metrics {
  #prefix: string;
  #series = new Map<string, Series>();

  constructor(prefix: string) {
    this.#prefix = prefix;
  }

  inc(name: string, labels: Labels = {}, by = 1): void {
    if (by < 0) throw new Error(`counter ${name} cannot decrease`);
    const series = this.#get(name, "counter");
    const key = labelKey(labels);
    series.values.set(key, (series.values.get(key) ?? 0) + by);
  }

  set(name: string, value: number, labels: Labels = {}): void {
    this.#get(name, "gauge").values.set(labelKey(labels), value);
  }

  value(name: string, labels: Labels = {}): number | undefined {
    return this.#series.get(this.#prefix + name)?.values.get(labelKey(labels));
  }

  render(): string {
    const lines: string[] = [];
    for (const name of [...this.#series.keys()].sort()) {
      const series = this.#series.get(name)!;
      lines.push(`# TYPE ${name} ${series.kind}`);
      for (const key of [...series.values.keys()].sort()) {
        lines.push(`${name}${key} ${series.values.get(key)}`);
      }
    }
    return lines.join("\n") + "\n";
  }

  #get(name: string, kind: Series["kind"]): Series {
    const full = this.#prefix + name;
    if (!NAME.test(full)) throw new Error(`invalid metric name: ${full}`);
    let series = this.#series.get(full);
    if (series === undefined) {
      series = { kind, values: new Map() };
      this.#series.set(full, series);
    } else if (series.kind !== kind) {
      throw new Error(`${full} is a ${series.kind}, not a ${kind}`);
    }
    return series;
  }
}

function labelKey(labels: Labels): string {
  const entries = Object.entries(labels).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  if (entries.length === 0) return "";
  const body = entries.map(([k, v]) => `${k}="${v.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`).join(",");
  return `{${body}}`;
}
