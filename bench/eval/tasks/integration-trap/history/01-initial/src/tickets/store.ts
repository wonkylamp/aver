import type { TokenSource } from "./tokens.ts";
import { randomTokens } from "./tokens.ts";

export type TicketStatus = "open" | "pending" | "closed";

export interface TicketMessage {
  from: string;
  text: string;
  at: Date;
  /** customer: the requester; agent: support staff; third-party: anyone else on the thread. */
  kind: "customer" | "agent" | "third-party";
}

export interface Ticket {
  id: number;
  subject: string;
  requester: string;
  replyToken: string;
  status: TicketStatus;
  messages: TicketMessage[];
  createdAt: Date;
  updatedAt: Date;
}

export interface OpenInput {
  requester: string;
  subject: string;
  text: string;
}

export class TicketError extends Error {
  override name = "TicketError";
}

export class TicketStore {
  readonly #tickets = new Map<number, Ticket>();
  readonly #byToken = new Map<string, number>();
  readonly #tokens: TokenSource;
  readonly #clock: () => Date;
  #seq = 1000;

  constructor(options: { tokens?: TokenSource; clock?: () => Date } = {}) {
    this.#tokens = options.tokens ?? randomTokens;
    this.#clock = options.clock ?? (() => new Date());
  }

  open(input: OpenInput): Ticket {
    const subject = input.subject.trim() || "(no subject)";
    const now = this.#clock();
    let token = this.#tokens();
    while (this.#byToken.has(token)) token = this.#tokens();
    const ticket: Ticket = {
      id: ++this.#seq,
      subject,
      requester: input.requester,
      replyToken: token,
      status: "open",
      messages: [{ from: input.requester, text: input.text, at: now, kind: "customer" }],
      createdAt: now,
      updatedAt: now,
    };
    this.#tickets.set(ticket.id, ticket);
    this.#byToken.set(token, ticket.id);
    return ticket;
  }

  get(id: number): Ticket | undefined {
    return this.#tickets.get(id);
  }

  byToken(token: string): Ticket | undefined {
    const id = this.#byToken.get(token);
    return id === undefined ? undefined : this.#tickets.get(id);
  }

  addMessage(id: number, message: Omit<TicketMessage, "at">): Ticket {
    const ticket = this.#tickets.get(id);
    if (!ticket) throw new TicketError(`no ticket ${id}`);
    const now = this.#clock();
    ticket.messages.push({ ...message, at: now });
    ticket.updatedAt = now;
    // A customer writing back reopens the ticket; an agent's reply waits on the customer.
    if (message.kind === "customer") ticket.status = "open";
    else if (message.kind === "agent") ticket.status = "pending";
    return ticket;
  }

  close(id: number): Ticket {
    const ticket = this.#tickets.get(id);
    if (!ticket) throw new TicketError(`no ticket ${id}`);
    ticket.status = "closed";
    ticket.updatedAt = this.#clock();
    return ticket;
  }

  list(filter: { status?: TicketStatus; requester?: string } = {}): Ticket[] {
    return [...this.#tickets.values()].filter(
      (t) => (filter.status === undefined || t.status === filter.status) && (filter.requester === undefined || t.requester === filter.requester),
    );
  }
}
