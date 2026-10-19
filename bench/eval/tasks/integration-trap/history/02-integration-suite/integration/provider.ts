// A stand-in for the mail provider: it records what the app sends, and
// delivers customers' replies to the app's inbound webhook the way the real
// provider does, in batches. The batch interval defaults to the real
// provider's (5 s), so the suite sees the same timing as production.
import type { OutboundMail } from "../src/mail/outbound.ts";

export interface Delivery {
  from: string;
  to: string[];
  cc?: string[];
  subject: string;
  text: string;
}

export class SimulatedProvider {
  readonly outbox: OutboundMail[] = [];
  #queue: Delivery[] = [];
  #timer: NodeJS.Timeout | null = null;
  #webhook = "";
  readonly batchMs = Number(process.env.MAIL_BATCH_INTERVAL_MS ?? 5000);

  async send(mail: OutboundMail): Promise<void> {
    this.outbox.push(mail);
  }

  connect(webhookUrl: string): void {
    this.#webhook = webhookUrl;
    this.#timer = setInterval(() => void this.flush(), this.batchMs);
  }

  stop(): void {
    if (this.#timer) clearInterval(this.#timer);
    this.#timer = null;
  }

  /** Queues an inbound message; it reaches the webhook with the next batch. */
  receive(mail: Delivery): void {
    this.#queue.push(mail);
  }

  /** What a mail client sends when the customer hits Reply on `mail`. */
  reply(mail: OutboundMail, from: string, text: string, extra: { cc?: string[] } = {}): void {
    this.receive({ from, to: [mail.replyTo], cc: extra.cc, subject: `Re: ${mail.subject}`, text: `${text}\n\nOn Tue, Relay Desk wrote:\n> ${mail.text.split("\n")[0]}` });
  }

  async flush(): Promise<void> {
    const batch = this.#queue;
    this.#queue = [];
    for (const mail of batch) {
      await fetch(this.#webhook, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(mail) });
    }
  }
}
