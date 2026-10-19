// The HTTP API. Plain node:http: a handful of JSON routes do not need a framework.
import { createServer } from "node:http";
import type { IncomingMessage, Server, ServerResponse } from "node:http";
import { AccountError, AccountStore } from "../accounts/store.ts";
import { routeInbound } from "../mail/inbound.ts";
import type { InboundMail } from "../mail/inbound.ts";
import type { Mailer } from "../mail/outbound.ts";
import { agentReplyNotice, openedNotice } from "../mail/outbound.ts";
import type { Ticket } from "../tickets/store.ts";
import { TicketError, TicketStore } from "../tickets/store.ts";

export interface App {
  server: Server;
  accounts: AccountStore;
  tickets: TicketStore;
}

class HttpError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

async function body(req: IncomingMessage): Promise<Record<string, unknown>> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of req) {
    size += (chunk as Buffer).length;
    if (size > 1_000_000) throw new HttpError(413, "body too large");
    chunks.push(chunk as Buffer);
  }
  try {
    const v: unknown = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
    if (typeof v !== "object" || v === null || Array.isArray(v)) throw new Error("not an object");
    return v as Record<string, unknown>;
  } catch {
    throw new HttpError(400, "body must be a JSON object");
  }
}

function str(b: Record<string, unknown>, key: string): string {
  const v = b[key];
  if (typeof v !== "string") throw new HttpError(400, `${key} must be a string`);
  return v;
}

function strs(b: Record<string, unknown>, key: string): string[] {
  const v = b[key];
  if (v === undefined) return [];
  if (!Array.isArray(v) || !v.every((x) => typeof x === "string")) throw new HttpError(400, `${key} must be a list of strings`);
  return v as string[];
}

function send(res: ServerResponse, status: number, value: unknown): void {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(value));
}

function ticketJson(t: Ticket): unknown {
  return { id: t.id, subject: t.subject, requester: t.requester, status: t.status, messages: t.messages.map((m) => ({ from: m.from, kind: m.kind, text: m.text })) };
}

export function createApp(options: { mailer: Mailer; accounts?: AccountStore; tickets?: TicketStore }): App {
  const accounts = options.accounts ?? new AccountStore();
  const tickets = options.tickets ?? new TicketStore();
  const { mailer } = options;

  async function handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const url = new URL(req.url ?? "/", "http://localhost");
    const route = `${req.method} ${url.pathname}`;
    if (route === "POST /signup") {
      const b = await body(req);
      const a = accounts.signup({ email: str(b, "email"), name: str(b, "name"), password: str(b, "password") });
      return send(res, 201, { id: a.id, email: a.email, name: a.name });
    }
    if (route === "POST /login") {
      const b = await body(req);
      const a = accounts.login(str(b, "email"), str(b, "password"));
      return send(res, 200, { id: a.id, email: a.email });
    }
    if (route === "POST /tickets") {
      const b = await body(req);
      const t = tickets.open({ requester: str(b, "requester"), subject: str(b, "subject"), text: str(b, "text") });
      await mailer.send(openedNotice(t));
      return send(res, 201, ticketJson(t));
    }
    if (route === "POST /webhooks/inbound") {
      const b = await body(req);
      const mail: InboundMail = { from: str(b, "from"), to: strs(b, "to"), cc: strs(b, "cc"), subject: str(b, "subject"), text: str(b, "text") };
      const routed = routeInbound(mail, tickets);
      if (routed.action === "dropped") return send(res, 202, { action: "dropped", reason: routed.reason });
      if (routed.action === "opened") await mailer.send(openedNotice(routed.ticket));
      return send(res, 200, { action: routed.action, ticket: routed.ticket.id });
    }
    const reply = /^\/tickets\/(\d+)\/replies$/.exec(url.pathname);
    if (req.method === "POST" && reply) {
      const b = await body(req);
      const t = tickets.addMessage(Number(reply[1]), { from: str(b, "agent"), text: str(b, "text"), kind: "agent" });
      await mailer.send(agentReplyNotice(t, str(b, "agent"), str(b, "text")));
      return send(res, 200, ticketJson(t));
    }
    const one = /^\/tickets\/(\d+)$/.exec(url.pathname);
    if (req.method === "GET" && one) {
      const t = tickets.get(Number(one[1]));
      if (!t) throw new HttpError(404, "no such ticket");
      return send(res, 200, ticketJson(t));
    }
    const close = /^\/tickets\/(\d+)\/close$/.exec(url.pathname);
    if (req.method === "POST" && close) return send(res, 200, ticketJson(tickets.close(Number(close[1]))));
    throw new HttpError(404, `no route ${route}`);
  }

  const server = createServer((req, res) => {
    handle(req, res).catch((err: unknown) => {
      if (err instanceof HttpError) return send(res, err.status, { error: err.message });
      if (err instanceof AccountError) {
        const status = { invalid: 400, duplicate: 409, credentials: 401 }[err.code];
        return send(res, status, { error: err.message, code: err.code });
      }
      if (err instanceof TicketError) return send(res, 404, { error: err.message });
      send(res, 500, { error: "internal error" });
    });
  });
  return { server, accounts, tickets };
}
