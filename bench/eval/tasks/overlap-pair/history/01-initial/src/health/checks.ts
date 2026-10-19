export type Check = () => void | Promise<void>;

export interface CheckResult {
  name: string;
  ok: boolean;
  detail?: string;
}

export interface HealthReport {
  status: "ok" | "degraded";
  checks: CheckResult[];
}

export class HealthRegistry {
  #checks = new Map<string, Check>();

  register(name: string, check: Check): void {
    if (this.#checks.has(name)) throw new Error(`health check already registered: ${name}`);
    this.#checks.set(name, check);
  }

  names(): string[] {
    return [...this.#checks.keys()].sort();
  }

  // Runs the checks named in `enabled`, in that order. A name nobody
  // registered fails, so a typo in the config shows up instead of passing.
  async run(enabled: string[]): Promise<HealthReport> {
    const checks: CheckResult[] = [];
    for (const name of enabled) {
      const check = this.#checks.get(name);
      if (check === undefined) {
        checks.push({ name, ok: false, detail: "not registered" });
        continue;
      }
      try {
        await check();
        checks.push({ name, ok: true });
      } catch (err) {
        checks.push({ name, ok: false, detail: err instanceof Error ? err.message : String(err) });
      }
    }
    return { status: checks.every((c) => c.ok) ? "ok" : "degraded", checks };
  }
}
