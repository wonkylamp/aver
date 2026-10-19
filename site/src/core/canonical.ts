export type JsonValue = null | boolean | number | string | JsonValue[] | JsonObject;
export interface JsonObject {
  [key: string]: JsonValue;
}

export class CanonicalError extends Error {
  override name = "CanonicalError";
}

const MAX_SAFE = 2n ** 53n - 1n;

/**
 * Canonical JSON per ARCHITECTURE.md §1.1.1: no whitespace, ASCII keys sorted
 * by byte, safe integers only, JCS string escaping. Object properties whose
 * value is `undefined` are omitted, which is how optional fields stay absent.
 */
export function canonicalize(value: unknown): string {
  const out: string[] = [];
  write(out, value);
  return out.join("");
}

export function canonicalBytes(value: unknown): Uint8Array {
  return new TextEncoder().encode(canonicalize(value));
}

/** Parses JSON text strictly, then canonicalizes it. */
export function canonicalizeText(text: string): string {
  return canonicalize(parseStrictJson(text));
}

function write(out: string[], value: unknown): void {
  switch (typeof value) {
    case "boolean":
      out.push(value ? "true" : "false");
      return;
    case "number":
      if (!Number.isSafeInteger(value) || Object.is(value, -0)) {
        throw new CanonicalError(`number ${value} is not an integer within ±(2^53-1)`);
      }
      out.push(String(value));
      return;
    case "string":
      out.push(quote(value));
      return;
    case "object":
      if (value === null) {
        out.push("null");
      } else if (Array.isArray(value)) {
        writeArray(out, value);
      } else {
        writeObject(out, value);
      }
      return;
    default:
      throw new CanonicalError(`${typeof value} is not a JSON value`);
  }
}

function writeArray(out: string[], items: readonly unknown[]): void {
  out.push("[");
  items.forEach((item, i) => {
    if (item === undefined) throw new CanonicalError("arrays cannot hold undefined");
    if (i > 0) out.push(",");
    write(out, item);
  });
  out.push("]");
}

function writeObject(out: string[], obj: object): void {
  const entries = Object.entries(obj).filter(([, v]) => v !== undefined);
  for (const [key] of entries) {
    if (!/^[\x00-\x7f]*$/.test(key)) throw new CanonicalError(`object key ${JSON.stringify(key)} is not ASCII`);
  }
  // ASCII keys compare identically by UTF-16 code unit and by byte.
  entries.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  out.push("{");
  entries.forEach(([key, v], i) => {
    if (i > 0) out.push(",");
    out.push(quote(key), ":");
    write(out, v);
  });
  out.push("}");
}

const SHORT_ESCAPES: Record<string, string> = {
  '"': '\\"',
  "\\": "\\\\",
  "\b": "\\b",
  "\t": "\\t",
  "\n": "\\n",
  "\f": "\\f",
  "\r": "\\r",
};

function quote(s: string): string {
  if (!s.isWellFormed()) throw new CanonicalError("string contains a lone surrogate");
  return `"${s.replace(/["\\\x00-\x1f]/g, (c) => SHORT_ESCAPES[c] ?? `\\u00${c.charCodeAt(0).toString(16).padStart(2, "0")}`)}"`;
}

/**
 * Parses JSON text with the strictness of Aver's hashed-object parser:
 * floats (any fraction or exponent, and `-0`), integers outside ±(2^53-1),
 * duplicate keys and lone surrogates are errors.
 */
export function parseStrictJson(text: string): JsonValue {
  const parser = new StrictParser(text);
  const value = parser.value();
  parser.end();
  return value;
}

class StrictParser {
  private pos = 0;
  private readonly text: string;

  constructor(text: string) {
    this.text = text;
  }

  value(): JsonValue {
    this.ws();
    const c = this.text[this.pos];
    switch (c) {
      case "{":
        return this.object();
      case "[":
        return this.array();
      case '"':
        return this.string();
      case "t":
        return this.literal("true", true);
      case "f":
        return this.literal("false", false);
      case "n":
        return this.literal("null", null);
      default:
        if (c === "-" || (c !== undefined && c >= "0" && c <= "9")) return this.number();
        throw this.error("unexpected character");
    }
  }

  end(): void {
    this.ws();
    if (this.pos !== this.text.length) throw this.error("trailing characters");
  }

  private object(): JsonObject {
    this.pos++;
    const obj: JsonObject = Object.create(null) as JsonObject;
    this.ws();
    if (this.text[this.pos] === "}") {
      this.pos++;
      return obj;
    }
    for (;;) {
      this.ws();
      if (this.text[this.pos] !== '"') throw this.error("expected object key");
      const key = this.string();
      if (Object.hasOwn(obj, key)) throw this.error(`duplicate key ${JSON.stringify(key)}`);
      this.ws();
      this.expect(":");
      obj[key] = this.value();
      this.ws();
      if (this.text[this.pos] === ",") {
        this.pos++;
        continue;
      }
      this.expect("}");
      return obj;
    }
  }

  private array(): JsonValue[] {
    this.pos++;
    const items: JsonValue[] = [];
    this.ws();
    if (this.text[this.pos] === "]") {
      this.pos++;
      return items;
    }
    for (;;) {
      items.push(this.value());
      this.ws();
      if (this.text[this.pos] === ",") {
        this.pos++;
        continue;
      }
      this.expect("]");
      return items;
    }
  }

  private string(): string {
    this.pos++;
    let out = "";
    for (;;) {
      const c = this.text[this.pos];
      if (c === undefined) throw this.error("unterminated string");
      if (c === '"') {
        this.pos++;
        break;
      }
      if (c < " ") throw this.error("unescaped control character in string");
      if (c !== "\\") {
        out += c;
        this.pos++;
        continue;
      }
      const e = this.text[this.pos + 1];
      this.pos += 2;
      const simple: Record<string, string> = { '"': '"', "\\": "\\", "/": "/", b: "\b", f: "\f", n: "\n", r: "\r", t: "\t" };
      if (e !== undefined && e in simple) {
        out += simple[e];
      } else if (e === "u") {
        out += String.fromCharCode(this.hex4());
      } else {
        throw this.error("invalid escape");
      }
    }
    if (!out.isWellFormed()) throw this.error("lone surrogate in string");
    return out;
  }

  private hex4(): number {
    const digits = this.text.slice(this.pos, this.pos + 4);
    if (!/^[0-9a-fA-F]{4}$/.test(digits)) throw this.error("invalid \\u escape");
    this.pos += 4;
    return parseInt(digits, 16);
  }

  private number(): number {
    const match = /^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?/.exec(this.text.slice(this.pos));
    if (!match) throw this.error("invalid number");
    const [token, , fraction, exponent] = match;
    if (fraction !== undefined || exponent !== undefined || token === "-0") {
      throw this.error(`floating point number ${token} is not allowed`);
    }
    const n = BigInt(token);
    if (n > MAX_SAFE || n < -MAX_SAFE) throw this.error(`integer ${token} is outside ±(2^53-1)`);
    this.pos += token.length;
    return Number(n);
  }

  private literal<T extends JsonValue>(word: string, value: T): T {
    if (!this.text.startsWith(word, this.pos)) throw this.error("invalid literal");
    this.pos += word.length;
    return value;
  }

  private expect(c: string): void {
    if (this.text[this.pos] !== c) throw this.error(`expected ${JSON.stringify(c)}`);
    this.pos++;
  }

  private ws(): void {
    while (/[ \t\n\r]/.test(this.text[this.pos] ?? "")) this.pos++;
  }

  private error(message: string): CanonicalError {
    return new CanonicalError(`${message} at offset ${this.pos}`);
  }
}
