// Start/stop timers, the reason durations are now seconds: a timer stopped
// after 14m 50s is 890 s, not 15 minutes.
import type { Entry, Role } from "./types.ts";

export class TimerError extends Error {
  override name = "TimerError";
}

interface Running {
  person: string;
  client: string;
  role: Role;
  start: Date;
  note?: string;
}

export class Timers {
  readonly #running = new Map<string, Running>();
  readonly #clock: () => Date;
  #seq = 0;

  constructor(clock: () => Date = () => new Date()) {
    this.#clock = clock;
  }

  start(person: string, client: string, role: Role, note?: string): void {
    if (this.#running.has(person)) throw new TimerError(`${person} already has a timer running`);
    this.#running.set(person, { person, client, role, start: this.#clock(), note });
  }

  stop(person: string, billable = true): Entry {
    const r = this.#running.get(person);
    if (!r) throw new TimerError(`${person} has no timer running`);
    this.#running.delete(person);
    const seconds = Math.round((this.#clock().getTime() - r.start.getTime()) / 1000);
    return { id: `t${++this.#seq}`, person: r.person, client: r.client, role: r.role, start: r.start, seconds, billable, note: r.note };
  }

  running(person: string): boolean {
    return this.#running.has(person);
  }
}
