import type { AddressInfo } from "node:net";
import { createApp } from "../src/http/app.ts";
import type { App } from "../src/http/app.ts";
import { SimulatedProvider } from "./provider.ts";

export interface Running {
  app: App;
  provider: SimulatedProvider;
  base: string;
  stop(): Promise<void>;
}

/** The app on a free port with a simulated mail provider, as production wires it (real reply tokens, real clock). */
export async function start(): Promise<Running> {
  const provider = new SimulatedProvider();
  const app = createApp({ mailer: provider });
  await new Promise<void>((resolve) => app.server.listen(0, "127.0.0.1", resolve));
  const { port } = app.server.address() as AddressInfo;
  const base = `http://127.0.0.1:${port}`;
  provider.connect(`${base}/webhooks/inbound`);
  return {
    app,
    provider,
    base,
    async stop() {
      provider.stop();
      await new Promise<void>((resolve) => app.server.close(() => resolve()));
    },
  };
}

export async function call(r: Running, method: string, path: string, body?: unknown): Promise<{ status: number; json: any }> {
  const res = await fetch(`${r.base}${path}`, { method, headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  return { status: res.status, json: await res.json() };
}

/** Polls until `check` returns a value, for up to two provider batches. */
export async function eventually<T>(r: Running, check: () => Promise<T | undefined>): Promise<T | undefined> {
  const deadline = Date.now() + 2 * r.provider.batchMs + 1000;
  for (;;) {
    const v = await check();
    if (v !== undefined || Date.now() > deadline) return v;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}
