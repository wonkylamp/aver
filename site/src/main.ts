import { renderError, renderVerification } from "./render.ts";
import { verifyReceipt } from "./verify.ts";

// A real receipt from the calc fixture and the public half of the throwaway
// verifier key that signed it (see examples/README.md).
const EXAMPLE_URL = "examples/calc-fail-own.json";
const EXAMPLE_KEY = "6k8CJxp-0fIP_Cl8v64ZNVg3PNsmrFY4YBpLFLWflOo";
// The largest receipt the viewer reads.
const MAX_INPUT_BYTES = 32 * 1024 * 1024;

function byId<T extends HTMLElement>(id: string, type: { new (): T }): T {
  const node = document.getElementById(id);
  if (!(node instanceof type)) throw new Error(`receipt.html is missing #${id}`);
  return node;
}

const form = byId("receipt-form", HTMLFormElement);
const text = byId("receipt-text", HTMLTextAreaElement);
const key = byId("key", HTMLInputElement);
const file = byId("file", HTMLInputElement);
const drop = byId("drop", HTMLElement);
const result = byId("result", HTMLElement);
const message = byId("input-message", HTMLElement);

let latest = 0;

async function check(): Promise<void> {
  const run = ++latest;
  const input = text.value.trim();
  if (input === "") {
    result.replaceChildren();
    say("Paste a receipt, drop a file, or load the example.");
    return;
  }
  say("");
  const outcome = await verifyReceipt(input, key.value);
  if (run !== latest) return;
  result.replaceChildren(outcome.ok ? renderVerification(document, outcome.verification) : renderError(document, outcome.error));
}

function say(note: string): void {
  message.textContent = note;
}

async function load(source: Promise<string>, keyValue?: string): Promise<void> {
  say("Loading…");
  try {
    text.value = await source;
  } catch (e) {
    say(e instanceof Error ? e.message : String(e));
    return;
  }
  if (keyValue !== undefined) key.value = keyValue;
  await check();
}

async function fetchText(url: string): Promise<string> {
  let target: URL;
  try {
    target = new URL(url, location.href);
  } catch {
    throw new Error(`Could not read ${url} as a URL.`);
  }
  if (target.protocol !== "https:" && target.protocol !== "http:") {
    throw new Error(`Only http and https URLs can be loaded, not ${target.protocol}`);
  }
  let response: Response;
  try {
    response = await fetch(target, { credentials: "omit", referrerPolicy: "no-referrer", cache: "no-store" });
  } catch {
    throw new Error(`Could not fetch ${target.href}. The server may be unreachable or may not allow cross-origin requests.`);
  }
  if (!response.ok) throw new Error(`Fetching ${target.href} failed with HTTP ${response.status}.`);
  const declared = Number(response.headers.get("content-length") ?? "0");
  const tooLarge = "That file is larger than 32 MiB, the largest receipt the viewer reads.";
  if (declared > MAX_INPUT_BYTES) throw new Error(tooLarge);
  const body = await response.text();
  if (body.length > MAX_INPUT_BYTES) throw new Error(tooLarge);
  return body;
}

async function readFile(f: File): Promise<string> {
  if (f.size > MAX_INPUT_BYTES) throw new Error(`${f.name} is larger than 32 MiB, the largest receipt the viewer reads.`);
  return f.text();
}

function forgeVerdict(): void {
  const source = text.value.trim();
  if (source === "") {
    say("Load a receipt first, then forge its verdict.");
    return;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(source);
  } catch {
    say("The text is not JSON, so there is no verdict to change.");
    return;
  }
  const envelope = parsed as { receipt?: { verdict?: unknown }; verdict?: unknown };
  const target = typeof envelope.receipt === "object" && envelope.receipt !== null ? envelope.receipt : envelope;
  const forged = target.verdict === "PASS" ? "FAIL_OWN" : "PASS";
  target.verdict = forged;
  text.value = JSON.stringify(parsed, null, 2);
  void check().then(() => say(`Changed the verdict to ${forged} without signing again.`));
}

form.addEventListener("submit", (event) => {
  event.preventDefault();
  void check();
});
key.addEventListener("change", () => void check());
file.addEventListener("change", () => {
  const chosen = file.files?.[0];
  if (chosen) void load(readFile(chosen));
});
byId("example", HTMLButtonElement).addEventListener("click", () => void load(fetchText(EXAMPLE_URL), EXAMPLE_KEY));
byId("forge", HTMLButtonElement).addEventListener("click", forgeVerdict);

drop.addEventListener("dragover", (event) => {
  event.preventDefault();
  drop.classList.add("is-over");
});
drop.addEventListener("dragleave", () => drop.classList.remove("is-over"));
drop.addEventListener("drop", (event) => {
  event.preventDefault();
  drop.classList.remove("is-over");
  const dropped = event.dataTransfer?.files[0];
  if (dropped) {
    void load(readFile(dropped));
  } else {
    const plain = event.dataTransfer?.getData("text/plain") ?? "";
    if (plain !== "") void load(Promise.resolve(plain));
  }
});

const params = new URLSearchParams(location.search);
const keyParam = params.get("key");
if (keyParam !== null) key.value = keyParam;
const urlParam = params.get("url");
if (urlParam !== null) void load(fetchText(urlParam));
